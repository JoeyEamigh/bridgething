mod arbiter;

use std::{
  collections::{HashMap, HashSet},
  sync::{Arc, Mutex},
  time::Duration,
};

pub use arbiter::{AuthorityHold, NowPlayingHub, NowPlayingSink};
use bridgething_gateway::{OutboundLink, OutboundLinkExt};
use libbridgething::{
  AudioCapabilities, GatewayCapabilities, GatewayInfo, MusicProvider, NetworkInfo, NluResolvedIntent,
  SurfaceAvailability, gateway::GatewayToBridgeCapabilitiesMsgEvent,
};
use tokio::time::Instant;

use crate::{
  api::{CapabilityFlags, HostInfo},
  dispatch::audio::VolumeAuthority,
  provider::{
    PlayerTransport, Provider, ProviderError, ProviderLink, ProviderRegistry, ResumeTarget,
    system_media::SystemMediaProvider,
  },
  voice::dispatcher::{CatalogError, VoiceCatalogResolver},
};

pub const AUTO_RESUME_COOLDOWN: Duration = Duration::from_secs(300);

struct Attached {
  providers: HashMap<String, Arc<dyn Provider>>,
  priority: Vec<String>,
  last_played_from: Option<String>,
}

struct ResumeGate {
  cooldown: Duration,
  enabled: HashMap<String, bool>,
  last_resume_at: HashMap<String, Instant>,
  targets: HashMap<String, ResumeTarget>,
  connected: HashSet<String>,
}

struct Announced {
  host: HostInfo,
  flags: CapabilityFlags,
  geo_usable: bool,
}

pub struct Hub {
  link: Arc<dyn OutboundLink>,
  now_playing: NowPlayingHub,
  attached: Mutex<Attached>,
  system: Mutex<Option<Arc<SystemMediaProvider>>>,
  resume: Mutex<ResumeGate>,
  announced: Mutex<Announced>,
}

impl Hub {
  pub fn new(link: Arc<dyn OutboundLink>, host: HostInfo, flags: CapabilityFlags, host_owns_volume: bool) -> Arc<Self> {
    Arc::new(Self {
      now_playing: NowPlayingHub::init(link.clone(), host_owns_volume),
      link,
      attached: Mutex::new(Attached {
        providers: HashMap::new(),
        priority: Vec::new(),
        last_played_from: None,
      }),
      system: Mutex::new(None),
      resume: Mutex::new(ResumeGate {
        cooldown: AUTO_RESUME_COOLDOWN,
        enabled: HashMap::new(),
        last_resume_at: HashMap::new(),
        targets: HashMap::new(),
        connected: HashSet::new(),
      }),
      announced: Mutex::new(Announced {
        host,
        flags,
        geo_usable: true,
      }),
    })
  }

  pub fn start(&self) {
    self.now_playing.start();
  }

  pub fn now_playing(&self) -> &NowPlayingHub {
    &self.now_playing
  }

  pub fn sink(&self) -> NowPlayingSink {
    self.now_playing.sink()
  }

  pub async fn attach(&self, provider: Arc<dyn Provider>) -> Result<(), ProviderError> {
    let id = provider.name().to_owned();
    let displaced = self.attached.lock().unwrap().providers.get(&id).cloned();
    if displaced.is_some_and(|held| !Arc::ptr_eq(&held, &provider)) {
      self.detach(&id).await;
    }
    let link = ProviderLink {
      sink: self.now_playing.sink(),
      outbound: self.link.clone(),
    };
    provider.attach(link).await?;
    self
      .attached
      .lock()
      .unwrap()
      .providers
      .insert(id.clone(), provider.clone());
    provider.set_resume_target(self.effective_resume_target());
    self.now_playing.register(&id, provider as Arc<dyn PlayerTransport>);
    self.announce().await;
    Ok(())
  }

  pub async fn detach(&self, id: &str) {
    let removed = {
      let mut attached = self.attached.lock().unwrap();
      let removed = attached.providers.remove(id);
      if attached.last_played_from.as_deref() == Some(id) {
        attached.last_played_from = None;
      }
      removed
    };
    let Some(provider) = removed else { return };
    self.now_playing.unregister(id);
    self.now_playing.sink().clear_source(id);
    provider.detach().await;
    self.announce().await;
  }

  pub async fn detach_all(&self) {
    for id in self.attached_ids() {
      self.detach(&id).await;
    }
  }

  pub fn attached_ids(&self) -> Vec<String> {
    self.attached.lock().unwrap().providers.keys().cloned().collect()
  }

  pub async fn set_priority(&self, ids: Vec<String>) {
    self.attached.lock().unwrap().priority = ids;
    self.announce().await;
  }

  pub fn mark_played_from(&self, id: &str) {
    self.attached.lock().unwrap().last_played_from = Some(id.to_owned());
  }

  pub fn last_played_from(&self) -> Option<String> {
    self.attached.lock().unwrap().last_played_from.clone()
  }

  fn ordered_ids(&self) -> Vec<String> {
    let attached = self.attached.lock().unwrap();
    let ranked: Vec<String> = attached
      .priority
      .iter()
      .filter(|id| attached.providers.contains_key(*id))
      .cloned()
      .collect();
    let mut rest: Vec<String> = attached
      .providers
      .keys()
      .filter(|id| !ranked.contains(id))
      .cloned()
      .collect();
    rest.sort();
    ranked.into_iter().chain(rest).collect()
  }

  fn provider(&self, id: &str) -> Option<Arc<dyn Provider>> {
    self.attached.lock().unwrap().providers.get(id).cloned()
  }

  pub async fn attach_system(&self, source: Arc<SystemMediaProvider>) -> Result<(), ProviderError> {
    let link = ProviderLink {
      sink: self.now_playing.sink(),
      outbound: self.link.clone(),
    };
    source.attach(link).await?;
    self
      .now_playing
      .register(source.name(), source.clone() as Arc<dyn PlayerTransport>);
    *self.system.lock().unwrap() = Some(source);
    Ok(())
  }

  pub async fn detach_system(&self) {
    let source = self.system.lock().unwrap().take();
    let Some(source) = source else { return };
    self.now_playing.unregister(source.name());
    source.detach().await;
  }

  fn system(&self) -> Option<Arc<SystemMediaProvider>> {
    self.system.lock().unwrap().clone()
  }

  pub fn provider_app_bundles(&self) -> Vec<String> {
    let mut bundles: Vec<String> = self
      .attached
      .lock()
      .unwrap()
      .providers
      .values()
      .flat_map(|provider| provider.app_bundles())
      .collect();
    bundles.sort();
    bundles.dedup();
    bundles
  }

  pub fn attached_schemes(&self) -> Vec<String> {
    let mut seen = Vec::new();
    for id in self.ordered_ids() {
      let Some(provider) = self.provider(&id) else { continue };
      for scheme in provider.uri_schemes() {
        if !seen.contains(&scheme) {
          seen.push(scheme);
        }
      }
    }
    seen
  }

  pub fn set_capability_flags_sync(&self, flags: CapabilityFlags) {
    self.announced.lock().unwrap().flags = flags;
  }

  pub async fn set_capability_flags(&self, flags: CapabilityFlags) {
    self.set_capability_flags_sync(flags);
    self.announce().await;
  }

  pub async fn set_geo_usable(&self, usable: bool) {
    let changed = {
      let mut announced = self.announced.lock().unwrap();
      let changed = announced.geo_usable != usable;
      announced.geo_usable = usable;
      changed
    };
    if changed {
      self.announce().await;
    }
  }

  pub fn compose_capabilities(&self) -> GatewayCapabilities {
    let (host, flags, geo_usable) = {
      let announced = self.announced.lock().unwrap();
      (announced.host.clone(), announced.flags, announced.geo_usable)
    };
    let library = ProviderRegistry::library(self);
    let supports_targets = self
      .attached
      .lock()
      .unwrap()
      .providers
      .values()
      .any(|provider| provider.supports_playback_targets());
    GatewayCapabilities {
      gateway: GatewayInfo {
        address: host.host_identifier,
        name: host.app_name.clone(),
        os_name: host.os_name,
        app_name: host.app_name,
        app_version: host.app_version,
        adapter_version: String::new(),
        lib_version: env!("CARGO_PKG_VERSION").to_string(),
        libbridgething_version: env!("CARGO_PKG_VERSION").to_string(),
      },
      uri_schemes: self.attached_schemes(),
      network: NetworkInfo::default(),
      available: SurfaceAvailability {
        geo: flags.geo && geo_usable,
        notifications: flags.notifications,
        net_fetch: flags.net_fetch,
        net_ws: flags.net_ws,
        audio_tts: flags.audio_tts,
        lyrics: true,
        playback_targets: supports_targets,
        forward: false,
      },
      audio: AudioCapabilities::default(),
      music_provider: library
        .map(|provider| provider.music_provider())
        .unwrap_or(MusicProvider::None),
    }
  }

  pub async fn announce(&self) {
    let caps = self.compose_capabilities();
    if let Err(failure) = self
      .link
      .event(GatewayToBridgeCapabilitiesMsgEvent::Announce(caps))
      .await
    {
      tracing::warn!(?failure, "the capabilities announce did not reach the peer");
    }
  }

  pub fn set_device_auto_resume(&self, device_id: &str, enabled: bool) {
    self
      .resume
      .lock()
      .unwrap()
      .enabled
      .insert(device_id.to_owned(), enabled);
  }

  pub fn set_auto_resume_cooldown(&self, cooldown: Duration) {
    self.resume.lock().unwrap().cooldown = cooldown;
  }

  pub fn auto_resume_prefs(&self) -> HashMap<String, bool> {
    self.resume.lock().unwrap().enabled.clone()
  }

  pub fn set_device_resume_target(&self, device_id: &str, target: ResumeTarget) {
    self.resume.lock().unwrap().targets.insert(device_id.to_owned(), target);
    self.push_resume_target();
  }

  pub fn resume_target_prefs(&self) -> HashMap<String, ResumeTarget> {
    self.resume.lock().unwrap().targets.clone()
  }

  pub fn default_resume_target(&self) -> ResumeTarget {
    match self
      .announced
      .lock()
      .unwrap()
      .host
      .os_name
      .to_ascii_lowercase()
      .as_str()
    {
      "ios" | "android" => ResumeTarget::PhoneOnly,
      _ => ResumeTarget::AnySpeaker,
    }
  }

  fn effective_resume_target(&self) -> ResumeTarget {
    let fallback = self.default_resume_target();
    let resume = self.resume.lock().unwrap();
    let all_any_speaker = !resume.connected.is_empty()
      && resume
        .connected
        .iter()
        .all(|id| resume.targets.get(id).copied().unwrap_or(fallback) == ResumeTarget::AnySpeaker);
    if all_any_speaker {
      ResumeTarget::AnySpeaker
    } else {
      ResumeTarget::PhoneOnly
    }
  }

  fn push_resume_target(&self) {
    let target = self.effective_resume_target();
    for id in self.ordered_ids() {
      if let Some(provider) = self.provider(&id) {
        provider.set_resume_target(target);
      }
    }
  }

  pub fn peer_disconnected(&self, device_id: &str) {
    self.resume.lock().unwrap().connected.remove(device_id);
    self.push_resume_target();
  }

  fn allow_auto_resume(&self, device_id: &str) -> bool {
    let mut resume = self.resume.lock().unwrap();
    if !resume.enabled.get(device_id).copied().unwrap_or(true) {
      tracing::info!(%device_id, "auto-resume off; skipping connect resume");
      return false;
    }
    if let Some(at) = resume.last_resume_at.get(device_id)
      && at.elapsed() < resume.cooldown
    {
      tracing::info!(%device_id, "auto-resumed recently; skipping connect resume");
      return false;
    }
    resume.last_resume_at.insert(device_id.to_owned(), Instant::now());
    true
  }

  fn resume_winner(&self) -> Option<String> {
    let attached = self.attached.lock().unwrap();
    if let Some(id) = self.now_playing.current_source()
      && attached.providers.contains_key(&id)
    {
      return Some(id);
    }
    if let Some(id) = &attached.last_played_from
      && attached.providers.contains_key(id)
    {
      return Some(id.clone());
    }
    drop(attached);
    self.ordered_ids().into_iter().next()
  }

  pub async fn peer_connected(&self, device_id: &str) {
    self.announce().await;
    self.now_playing.on_connect();
    self.resume.lock().unwrap().connected.insert(device_id.to_owned());
    self.push_resume_target();
    let allow = self.allow_auto_resume(device_id);
    let winner = self.resume_winner();
    for id in self.ordered_ids() {
      if let Some(provider) = self.provider(&id) {
        provider
          .handle_peer_connected(allow && winner.as_deref() == Some(&id))
          .await;
      }
    }
  }

  pub async fn resumed(&self) {
    for id in self.ordered_ids() {
      if let Some(provider) = self.provider(&id) {
        provider.resumed().await;
      }
    }
  }
}

impl ProviderRegistry for Hub {
  fn library(&self) -> Option<Arc<dyn Provider>> {
    let last = self.attached.lock().unwrap().last_played_from.clone();
    if let Some(id) = last
      && let Some(provider) = self.provider(&id)
    {
      return Some(provider);
    }
    self.ordered_ids().into_iter().next().and_then(|id| self.provider(&id))
  }

  fn audible(&self) -> Option<Arc<dyn Provider>> {
    self.now_playing.current_source().and_then(|id| self.provider(&id))
  }

  fn for_uri(&self, uri: &str) -> Option<Arc<dyn Provider>> {
    let scheme = uri.split(':').next()?.to_ascii_lowercase();
    self
      .ordered_ids()
      .into_iter()
      .filter_map(|id| self.provider(&id))
      .find(|provider| {
        provider
          .uri_schemes()
          .iter()
          .any(|claimed| claimed.eq_ignore_ascii_case(&scheme))
      })
      .or_else(|| {
        self
          .system()
          .filter(|source| source.uri_schemes().contains(&scheme))
          .map(|source| source as Arc<dyn Provider>)
      })
  }

  fn all(&self) -> Vec<Arc<dyn Provider>> {
    self
      .ordered_ids()
      .into_iter()
      .filter_map(|id| self.provider(&id))
      .chain(self.system().map(|source| source as Arc<dyn Provider>))
      .collect()
  }
}

#[async_trait::async_trait]
impl VolumeAuthority for Hub {
  async fn owns_volume(&self) -> bool {
    match self.audible().or_else(|| ProviderRegistry::library(self)) {
      Some(provider) => provider.owns_volume().await,
      None => false,
    }
  }

  async fn volume_up(&self) -> Result<f32, String> {
    let provider = self.volume_target().ok_or("no provider owns volume")?;
    provider.volume_up().await.map_err(|error| error.to_string())
  }

  async fn volume_down(&self) -> Result<f32, String> {
    let provider = self.volume_target().ok_or("no provider owns volume")?;
    provider.volume_down().await.map_err(|error| error.to_string())
  }

  async fn set_volume(&self, level: f32) -> Result<f32, String> {
    let provider = self.volume_target().ok_or("no provider owns volume")?;
    provider.set_volume(level).await.map_err(|error| error.to_string())
  }
}

#[async_trait::async_trait]
impl VoiceCatalogResolver for Hub {
  async fn decorate(&self, resolved: NluResolvedIntent) -> Result<NluResolvedIntent, CatalogError> {
    match ProviderRegistry::library(self).and_then(|provider| provider.voice_resolver()) {
      Some(resolver) => resolver.decorate(resolved).await,
      None => Ok(resolved),
    }
  }
}

impl Hub {
  fn volume_target(&self) -> Option<Arc<dyn Provider>> {
    self.audible().or_else(|| ProviderRegistry::library(self))
  }
}
