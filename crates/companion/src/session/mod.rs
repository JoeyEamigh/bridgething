pub mod handlers;
pub mod link;
pub mod models;
pub mod observer;
pub mod ota;

use std::{
  collections::{HashMap, HashSet},
  sync::{Arc, Mutex},
};

use bridgething_delivery::{
  blob::{FsBlobStore, FsSlotIndex},
  bundle::{BundlePlatform, fetch::ArtifactFetch},
  log::DeviceLogRing,
  ota::service::{OtaService, OtaServiceDeps},
  seam::{BlobStore, Clock, SlotIndex, SystemClock},
  serve::{net::NetDispatcher, tunnel::TunnelDispatcher},
  transfer::{AckSink, TransferReceiver},
  webapp::WebappResourceService,
};
pub use bridgething_gateway::GatewayProtocol;
use bridgething_gateway::{
  Gateway, OutboundLink, OutboundLinkExt, SdkError,
  routing::{Routing, spawn_routing},
};
use bridgething_io::HttpExecutor;
use bridgething_sdk_runtime::Connector;
use libbridgething::{
  CompanionAuthorityScope, TimeInfo,
  gateway::{GatewayToBridgeTransferMsgEvent, TransferAck, VolumeChanged},
};
use tokio::{sync::mpsc, task::JoinHandle};
use uuid::Uuid;

use crate::{
  api::{
    AuthKind, AuthState, CapabilityFlags, CompanionBackends, CompanionConfig, CompanionDebug, CompanionError,
    DeviceAutoResume, DeviceResumeTarget, ModelPlatform, OtaPollConfig, PeerLinkStatus, ProviderInfo, ProviderTokens,
    ServiceHealth, ServiceHealthKind, SessionEvent, SessionEventSink, SessionHostInfo, SessionPeer, SessionSnapshot,
    VoiceDebug, VoiceModelPaths, VoiceModelState, VoiceModelStatus,
  },
  backend::{
    AlwaysAllows, ConnectivityInbox, ForeignHttp, ForeignModelValidator, ForeignTransferPolicy, ForeignWs, HostClock,
    LinkDevice, LinkEvent, LinkInbox, LinkTransport, PrepareEvent, PrepareSink, VolumeInbox, VolumeLevel,
  },
  dispatch::{
    asset::AssetDispatcher, audio::AudioDispatcher, extension::ExtensionDispatcher, geo::GeoDispatcher,
    library::LibraryDispatcher, lyrics::LyricsDispatcher, notifications::NotificationDispatcher,
    phone::PhoneDispatcher, player::PlayerDispatcher, system::SystemDispatcher, webapp::WebappDispatcher,
  },
  hub::Hub,
  provider::{
    Provider, ProviderAuthState, ProviderError, ProviderRegistry, ResumeTarget,
    catalog::{AppleMusicEntry, ProviderCatalog, SpotifyEntry},
    system_media::SystemMediaProvider,
  },
  session::{handlers::Peer, link::LinkConnector, models::VoiceModels, observer::SessionObserver, ota::OtaLink},
  voice::{
    controller::{ArmedModel, VoiceController, VoiceControllerConfig},
    dispatcher::{VoiceDispatcher, VoiceDispatcherDeps},
    inference::BundleInference,
  },
};

struct Link {
  gateway: Gateway,
  peer: Arc<Peer>,
  token: Uuid,
  _routing: Routing,
}

#[derive(Default)]
struct LogStream {
  enabled: bool,
  tokens: HashMap<String, String>,
}

const LOG_RING_CAPACITY: usize = 2000;
const COMPANION_UPDATE_DIR: &str = "companion-update";
const LOG_BACKFILL_LINES: u32 = 1000;
const LOG_BACKFILL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Default)]
struct Broadcast {
  links: Mutex<HashMap<String, Gateway>>,
}

impl Broadcast {
  fn adopt(&self, device_id: &str, gateway: Gateway) {
    self.links.lock().unwrap().insert(device_id.to_owned(), gateway);
  }

  fn release(&self, device_id: &str) {
    self.links.lock().unwrap().remove(device_id);
  }
}

#[async_trait::async_trait]
impl OutboundLink for Broadcast {
  async fn send_data(
    &self,
    meta: libbridgething::wire::MsgMeta,
    data: libbridgething::gateway::GatewayToBridgeMsgData,
    priority: libbridgething::Priority,
  ) -> Result<(), SdkError> {
    let mut links: Vec<(String, Gateway)> = self
      .links
      .lock()
      .unwrap()
      .iter()
      .map(|(device_id, gateway)| (device_id.clone(), gateway.clone()))
      .collect();
    let Some((last_id, last)) = links.pop() else {
      return Err(SdkError::Disconnected);
    };
    let mut delivered = false;
    for (device_id, gateway) in links {
      match gateway.send_data(meta.clone(), data.clone(), priority).await {
        Ok(()) => delivered = true,
        Err(failure) => tracing::warn!(%device_id, ?failure, "a peer missed a fanned-out frame"),
      }
    }
    match last.send_data(meta, data, priority).await {
      Ok(()) => Ok(()),
      Err(failure) => {
        tracing::warn!(device_id = %last_id, ?failure, "a peer missed a fanned-out frame");
        if delivered { Ok(()) } else { Err(failure) }
      }
    }
  }
}

pub struct Session {
  config: CompanionConfig,
  backends: CompanionBackends,
  clock: Arc<dyn Clock>,
  fetch: Arc<dyn ArtifactFetch>,
  observer: Arc<SessionObserver>,
  ota: Arc<OtaService>,
  log_ring: Arc<DeviceLogRing>,
  models: Option<Arc<VoiceModels>>,
  models_watch: Mutex<Option<JoinHandle<()>>>,
  voice: Arc<VoiceController>,
  voice_bundle: tokio::sync::Mutex<Option<std::path::PathBuf>>,
  catalog: ProviderCatalog,
  providers: Mutex<Vec<Arc<dyn Provider>>>,
  provider_gate: tokio::sync::Mutex<()>,
  connected: Mutex<HashSet<String>>,
  auth: Arc<Mutex<HashMap<String, ProviderAuthState>>>,
  priority: Mutex<Vec<String>>,
  caps: Arc<Mutex<CapabilityFlags>>,
  log_stream: Mutex<LogStream>,
  poll_config: Mutex<Option<OtaPollConfig>>,
  blobs: Arc<FsBlobStore>,
  resource_slots: Arc<FsSlotIndex>,
  hub: Arc<Hub>,
  broadcast: Arc<Broadcast>,
  extensions: Option<Arc<ExtensionDispatcher>>,
  links: Mutex<HashMap<String, Link>>,
  inbox: Mutex<Option<JoinHandle<()>>>,
  connectivity_pump: Mutex<Option<JoinHandle<()>>>,
  volume_pump: Mutex<Option<JoinHandle<()>>>,
  arbitration: Mutex<Option<JoinHandle<()>>>,
  updates: Mutex<Option<JoinHandle<()>>>,
}

impl Session {
  pub fn new(
    config: CompanionConfig,
    backends: CompanionBackends,
    events: Arc<dyn SessionEventSink>,
    fetch: Arc<dyn ArtifactFetch>,
  ) -> Arc<Self> {
    crate::backend::log::forward_tracing(backends.log.clone());
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let ota = OtaService::new(OtaServiceDeps {
      clock: clock.clone(),
      fetch: fetch.clone(),
      cache_dir: std::path::PathBuf::from(&config.cache_dir),
      data_dir: Some(std::path::PathBuf::from(&config.state_dir)),
    });
    let models = config
      .model_platform
      .zip(backends.model_validator.clone())
      .map(|(platform, validator)| {
        let policy = match backends.transfer_policy.clone() {
          Some(policy) => {
            Arc::new(ForeignTransferPolicy::new(policy)) as Arc<dyn bridgething_delivery::seam::TransferPolicy>
          }
          None => Arc::new(AlwaysAllows),
        };
        VoiceModels::new(
          match platform {
            ModelPlatform::Ios => BundlePlatform::Ios,
            ModelPlatform::Android => BundlePlatform::Android,
            ModelPlatform::Macos => BundlePlatform::Macos,
            ModelPlatform::Linux => BundlePlatform::Linux,
            ModelPlatform::Windows => BundlePlatform::Windows,
          },
          std::path::PathBuf::from(&config.state_dir),
          config.capabilities.voice_model,
          fetch.clone(),
          policy,
          Arc::new(ForeignModelValidator::new(validator)),
        )
      });
    let cache_dir = std::path::PathBuf::from(&config.cache_dir);
    let _ = std::fs::remove_dir_all(std::path::Path::new(&config.state_dir).join(COMPANION_UPDATE_DIR));
    let log_ring = Arc::new(DeviceLogRing::new(LOG_RING_CAPACITY, clock.clone()));
    let catalog = ProviderCatalog::new(
      config
        .spotify
        .clone()
        .map(|spotify| {
          SpotifyEntry::new(
            spotify,
            backends.http.clone(),
            backends.ws.clone(),
            backends.secrets.clone(),
            backends.image.clone(),
            backends.device_waker.clone(),
          ) as Arc<dyn crate::provider::catalog::CatalogEntry>
        })
        .into_iter()
        .chain(backends.apple_music.clone().map(|backend| {
          AppleMusicEntry::new(
            backend,
            backends.http.clone(),
            backends.secrets.clone(),
            backends.image.clone(),
          ) as Arc<dyn crate::provider::catalog::CatalogEntry>
        }))
        .collect(),
    );
    let broadcast = Arc::new(Broadcast::default());
    let hub = Hub::new(
      broadcast.clone() as Arc<dyn OutboundLink>,
      config.host.clone(),
      config.capabilities,
      backends.volume.is_some(),
    );
    let extensions = backends.extensions.clone().map(ExtensionDispatcher::new);
    Arc::new(Self {
      caps: Arc::new(Mutex::new(config.capabilities)),
      config,
      backends,
      clock,
      fetch,
      observer: SessionObserver::new(events, log_ring.clone()),
      ota,
      log_ring,
      models,
      models_watch: Mutex::new(None),
      voice: Arc::new(VoiceController::new(None, VoiceControllerConfig::default())),
      voice_bundle: tokio::sync::Mutex::new(None),
      catalog,
      providers: Mutex::new(Vec::new()),
      provider_gate: tokio::sync::Mutex::new(()),
      connected: Mutex::new(HashSet::new()),
      auth: Arc::new(Mutex::new(HashMap::new())),
      priority: Mutex::new(Vec::new()),
      log_stream: Mutex::new(LogStream::default()),
      poll_config: Mutex::new(None),
      blobs: Arc::new(FsBlobStore::new(cache_dir.join("blobs"))),
      resource_slots: Arc::new(FsSlotIndex::new(cache_dir.join("slots.json"))),
      hub,
      broadcast,
      extensions,
      links: Mutex::new(HashMap::new()),
      inbox: Mutex::new(None),
      connectivity_pump: Mutex::new(None),
      volume_pump: Mutex::new(None),
      arbitration: Mutex::new(None),
      updates: Mutex::new(None),
    })
  }

  pub fn ota(&self) -> &Arc<OtaService> {
    &self.ota
  }

  pub fn observer(&self) -> &Arc<SessionObserver> {
    &self.observer
  }

  pub fn ws(&self) -> Arc<dyn crate::backend::WsTransport> {
    self.backends.ws.clone()
  }

  pub fn log_ring(&self) -> &Arc<DeviceLogRing> {
    &self.log_ring
  }

  pub fn hub(&self) -> &Arc<Hub> {
    &self.hub
  }

  pub fn peer_for(&self, device_id: &str) -> Option<Arc<Peer>> {
    self.links.lock().unwrap().get(device_id).map(|link| link.peer.clone())
  }

  pub fn gateway_for(&self, device_id: &str) -> Option<Gateway> {
    self
      .links
      .lock()
      .unwrap()
      .get(device_id)
      .map(|link| link.gateway.clone())
  }

  pub fn is_linked(&self, device_id: &str) -> bool {
    self.links.lock().unwrap().contains_key(device_id)
  }

  pub fn linked_ids(&self) -> Vec<String> {
    self.links.lock().unwrap().keys().cloned().collect()
  }

  pub fn device_ids(&self) -> Vec<String> {
    self.links.lock().unwrap().keys().cloned().collect()
  }

  fn live_gateways(&self) -> Vec<(String, Gateway)> {
    self
      .links
      .lock()
      .unwrap()
      .iter()
      .map(|(device_id, link)| (device_id.clone(), link.gateway.clone()))
      .collect()
  }

  pub async fn add_provider(self: &Arc<Self>, provider: Arc<dyn Provider>) -> Result<(), ProviderError> {
    let _gate = self.provider_gate.lock().await;
    self.observe_auth(&provider);
    self.providers.lock().unwrap().push(provider.clone());
    let attached = self.hub.attach(provider).await;
    self.providers_changed();
    attached
  }

  fn observe_auth(self: &Arc<Self>, provider: &Arc<dyn Provider>) {
    let id = provider.name().to_owned();
    let held = self.auth.clone();
    let session = Arc::downgrade(self);
    provider.set_auth_observer(Some(Arc::new(move |state| {
      let authenticated = matches!(state, ProviderAuthState::Authenticated);
      held.lock().unwrap().insert(id.clone(), state);
      if let Some(session) = session.upgrade() {
        if authenticated {
          session.connected.lock().unwrap().insert(id.clone());
          if let Some(entry) = session.catalog.get(&id) {
            entry.mark_connected();
          }
        }
        session.providers_changed();
      }
    })));
  }

  fn registered(&self, id: &str) -> Option<Arc<dyn Provider>> {
    self
      .providers
      .lock()
      .unwrap()
      .iter()
      .find(|provider| provider.name() == id)
      .cloned()
  }

  pub async fn connect_provider(self: &Arc<Self>, id: &str) -> Result<(), CompanionError> {
    tracing::info!(
      id,
      registered = self.registered(id).is_some(),
      "provider connect requested"
    );
    let _gate = self.provider_gate.lock().await;
    self.connect_locked(id).await
  }

  async fn connect_locked(self: &Arc<Self>, id: &str) -> Result<(), CompanionError> {
    let entry = self
      .catalog
      .get(id)
      .ok_or_else(|| CompanionError::Device(format!("unknown provider {id}")))?;
    let provider = match self.registered(id) {
      Some(provider) => provider,
      None => {
        let provider = entry.build();
        self.providers.lock().unwrap().push(provider.clone());
        provider
      }
    };
    if entry.has_credentials() {
      self.connected.lock().unwrap().insert(id.to_owned());
    }
    self.observe_auth(&provider);
    let attached = self.hub.attach(provider).await;
    self.providers_changed();
    attached.map_err(|error| CompanionError::Device(error.to_string()))
  }

  pub async fn complete_provider_auth(
    self: &Arc<Self>,
    id: &str,
    tokens: ProviderTokens,
  ) -> Result<(), CompanionError> {
    let entry = self
      .catalog
      .get(id)
      .ok_or_else(|| CompanionError::Device(format!("unknown provider {id}")))?;
    entry.adopt_tokens(tokens);
    self.connect_provider(id).await
  }

  pub async fn cancel_auth(&self, id: &str) {
    self.remove_provider(id).await;
    self.providers_changed();
  }

  pub async fn disconnect_provider(&self, id: &str) {
    self.remove_provider(id).await;
    if let Some(entry) = self.catalog.get(id) {
      entry.clear_credentials();
    }
    self.providers_changed();
  }

  async fn remove_provider(&self, id: &str) {
    let _gate = self.provider_gate.lock().await;
    let removed = {
      let mut providers = self.providers.lock().unwrap();
      providers
        .iter()
        .position(|provider| provider.name() == id)
        .map(|at| providers.remove(at))
    };
    self.connected.lock().unwrap().remove(id);
    self.auth.lock().unwrap().remove(id);
    self.hub.detach(id).await;
    if let Some(provider) = removed {
      provider.detach().await;
    }
  }

  fn restore_providers(self: &Arc<Self>) {
    let session = self.clone();
    tokio::spawn(async move {
      let _gate = session.provider_gate.lock().await;
      for entry in session.catalog.entries() {
        if session.registered(entry.id()).is_none()
          && entry.has_credentials()
          && let Err(error) = session.connect_locked(entry.id()).await
        {
          tracing::warn!(%error, id = entry.id(), "a stored sign-in did not restore");
        }
      }
    });
  }

  pub async fn set_provider_priority(&self, ids: Vec<String>) {
    *self.priority.lock().unwrap() = ids.clone();
    self.hub.set_priority(ids).await;
    self.providers_changed();
  }

  fn providers_changed(&self) {
    self.observer.emit(SessionEvent::ProvidersChanged {
      providers: self.provider_infos(),
    });
  }

  pub fn voice_models(&self) -> Option<&Arc<VoiceModels>> {
    self.models.as_ref()
  }

  pub fn ensure_voice_models(&self) {
    if let Some(models) = self.models.clone() {
      tokio::spawn(async move { models.ensure().await });
    }
  }

  pub fn download_voice_models(&self) {
    if let Some(models) = self.models.clone() {
      tokio::spawn(async move { models.download_now().await });
    }
  }

  pub fn voice_controller(&self) -> &Arc<VoiceController> {
    &self.voice
  }

  pub fn large_transfers_allowed(&self) -> bool {
    self
      .backends
      .transfer_policy
      .as_ref()
      .is_none_or(|policy| policy.allows_large_transfer())
  }

  fn prepare_speech(&self) {
    let Some(recognizer) = self.backends.speech.clone() else {
      return;
    };
    let (sink, mut events) = PrepareSink::channel();
    tokio::task::spawn_blocking(move || recognizer.prepare(sink));
    tokio::spawn(async move {
      while let Some(event) = events.recv().await {
        match event {
          PrepareEvent::Progress { received, total } => {
            tracing::debug!(received, total, "the speech assets are downloading");
          }
          PrepareEvent::Ready => tracing::info!("the speech recognizer is ready"),
          PrepareEvent::Failed { reason } => tracing::warn!(%reason, "the speech recognizer is unavailable"),
        }
      }
    });
  }

  async fn refresh_voice_model(&self) {
    let mut armed = self.voice_bundle.lock().await;
    let installed = self.models.as_ref().and_then(|models| models.nlu_bundle());
    if *armed == installed {
      return;
    }
    let Some(runner) = self.backends.nlu.clone() else {
      return;
    };
    let Some(bundle) = installed else {
      self.voice.set_model(None);
      *armed = None;
      return;
    };

    let loaded = {
      let bundle = bundle.clone();
      tokio::task::spawn_blocking(move || BundleInference::load(&bundle, runner)).await
    };
    match loaded {
      Ok(Ok(inference)) => {
        let rejection = inference.rejection();
        self.voice.set_model(Some(ArmedModel {
          client: Arc::new(inference),
          bundle: Some(bundle.display().to_string()),
          rejection,
        }));
        *armed = Some(bundle.clone());
        tracing::info!(bundle = %bundle.display(), "the nlu model is armed");
      }
      Ok(Err(error)) => {
        self.voice.set_model(None);
        *armed = None;
        tracing::warn!(%error, bundle = %bundle.display(), "the installed nlu bundle did not load");
      }
      Err(error) => {
        *armed = None;
        tracing::warn!(%error, "loading the nlu bundle did not finish");
      }
    }
  }

  pub fn start(self: &Arc<Self>) {
    self.hub.start();
    self.mirror_system_media();
    if let Some(extensions) = &self.extensions {
      extensions.start();
    }
    if let Some(previous) = self.arbitration.lock().unwrap().replace(self.watch_arbitration()) {
      previous.abort();
    }
    if let Some(previous) = self.updates.lock().unwrap().replace(self.watch_updates()) {
      previous.abort();
    }
    self.restore_providers();
    self.prepare_speech();
    if let Some(models) = &self.models {
      let session = Arc::downgrade(self);
      let watch = models.watch(Arc::new(move |state| {
        let Some(session) = session.upgrade() else {
          return;
        };
        tokio::spawn(async move {
          session.refresh_voice_model().await;
          if state.status == VoiceModelStatus::Ready {
            session.prepare_speech();
          }
          session.observer.emit(SessionEvent::VoiceModelStateChanged { state });
        });
      }));
      if let Some(previous) = self.models_watch.lock().unwrap().replace(watch) {
        previous.abort();
      }
      let armed = self.clone();
      tokio::spawn(async move { armed.refresh_voice_model().await });
      self.ensure_voice_models();
    }
    if let Some(monitor) = self.backends.connectivity.clone() {
      let (inbox, edges) = ConnectivityInbox::channel();
      let session = self.clone();
      let pump = tokio::spawn(async move { session.pump_connectivity(edges).await });
      if let Some(previous) = self.connectivity_pump.lock().unwrap().replace(pump) {
        previous.abort();
      }
      tokio::task::spawn_blocking(move || monitor.start(inbox));
    }
    if let Some(volume) = self.backends.volume.clone() {
      let (inbox, levels) = VolumeInbox::channel();
      let session = self.clone();
      let pump = tokio::spawn(async move { session.pump_volume(levels).await });
      if let Some(previous) = self.volume_pump.lock().unwrap().replace(pump) {
        previous.abort();
      }
      tokio::task::spawn_blocking(move || volume.start(inbox));
    }
    let Some(transport) = self.backends.link.clone() else {
      return;
    };
    let (inbox, events) = LinkInbox::channel();
    let session = self.clone();
    let pump = tokio::spawn(async move { session.pump(events).await });
    if let Some(previous) = self.inbox.lock().unwrap().replace(pump) {
      previous.abort();
    }
    tokio::task::spawn_blocking(move || transport.start(inbox));
  }

  fn mirror_system_media(self: &Arc<Self>) {
    let Some(backend) = self.backends.media_sessions.clone() else {
      return;
    };
    let hub = self.hub.clone();
    let owned = Arc::downgrade(&self.hub);
    tokio::spawn(async move {
      let source = SystemMediaProvider::new(
        backend,
        Arc::new(move || {
          owned
            .upgrade()
            .map(|hub| hub.provider_app_bundles())
            .unwrap_or_default()
        }),
      );
      hub.detach_system().await;
      if let Err(error) = hub.attach_system(source).await {
        tracing::warn!(%error, "the system media mirror did not attach");
      }
    });
  }

  async fn pump_volume(self: Arc<Self>, mut levels: mpsc::UnboundedReceiver<VolumeLevel>) {
    while let Some(level) = levels.recv().await {
      for (_, gateway) in self.live_gateways() {
        let _ = gateway
          .audio()
          .volume_changed(VolumeChanged {
            level: level.level,
            muted: level.muted,
          })
          .await;
      }
    }
  }

  async fn pump_connectivity(self: Arc<Self>, mut edges: mpsc::UnboundedReceiver<bool>) {
    while let Some(online) = edges.recv().await {
      let providers = self.providers.lock().unwrap().clone();
      for provider in providers {
        provider.connectivity_changed(online).await;
      }
      if online {
        self.ensure_voice_models();
      }
    }
  }

  pub async fn stop(&self) {
    for held in [
      &self.models_watch,
      &self.inbox,
      &self.connectivity_pump,
      &self.volume_pump,
      &self.arbitration,
      &self.updates,
    ] {
      if let Some(task) = held.lock().unwrap().take() {
        task.abort();
      }
    }
    if let Some(monitor) = self.backends.connectivity.clone() {
      let _ = tokio::task::spawn_blocking(move || monitor.stop()).await;
    }
    if let Some(volume) = self.backends.volume.clone() {
      let _ = tokio::task::spawn_blocking(move || volume.stop()).await;
    }
    for device_id in self.device_ids() {
      self.teardown(&device_id).await;
    }
    if let Some(extensions) = &self.extensions {
      extensions.stop().await;
    }
    self.hub.detach_all().await;
    self.hub.detach_system().await;
    let Some(transport) = self.backends.link.clone() else {
      return;
    };
    let _ = tokio::task::spawn_blocking(move || transport.stop()).await;
  }

  pub async fn connect_direct<C>(self: &Arc<Self>, device: LinkDevice, connector: C)
  where
    C: Connector<GatewayProtocol> + Send + 'static,
  {
    self.bring_up(&device, connector).await;
  }

  pub async fn direct_disconnected(&self, device_id: &str) {
    if self.teardown(device_id).await {
      self.observer.peer_disconnected(device_id);
    }
  }

  async fn pump(self: Arc<Self>, mut events: mpsc::UnboundedReceiver<LinkEvent>) {
    let Some(transport) = self.backends.link.clone() else {
      return;
    };
    let mut feeds: HashMap<String, link::LinkFeed> = HashMap::new();
    while let Some(event) = events.recv().await {
      match event {
        LinkEvent::Connected(device) => {
          let feed = self.adopt(&device, transport.clone()).await;
          feeds.insert(device.id, feed);
        }
        LinkEvent::Bytes { device_id, bytes } => {
          if let Some(held) = feeds.get(&device_id) {
            let _ = held.bytes.send(bytes);
          }
        }
        LinkEvent::WriteComplete { device_id } => {
          if let Some(held) = feeds.get(&device_id) {
            let _ = held.credit.send(link::LinkWrite::Complete);
          }
        }
        LinkEvent::SendFailed { device_id } => {
          if let Some(held) = feeds.remove(&device_id) {
            let _ = held.credit.send(link::LinkWrite::Failed);
          }
        }
        LinkEvent::Disconnected { device_id } => {
          feeds.remove(&device_id);
          if self.teardown(&device_id).await {
            self.observer.peer_disconnected(&device_id);
          }
        }
        LinkEvent::LinkFailed {
          device_id,
          name,
          reason,
        } => {
          feeds.remove(&device_id);
          self.teardown(&device_id).await;
          self.observer.peer_link_failed(SessionPeer {
            id: device_id,
            name,
            status: PeerLinkStatus::LinkFailed,
            link_error: Some(reason),
          });
        }
      }
    }
  }

  async fn adopt(self: &Arc<Self>, device: &LinkDevice, transport: Arc<dyn LinkTransport>) -> link::LinkFeed {
    let (connector, feed) = LinkConnector::open(&device.id, transport);
    self.bring_up(device, connector).await;
    feed
  }

  async fn bring_up<C>(self: &Arc<Self>, device: &LinkDevice, connector: C)
  where
    C: Connector<GatewayProtocol> + Send + 'static,
  {
    tracing::info!(device_id = %device.id, name = %device.name, "a peer link is coming up");
    self.teardown(&device.id).await;

    let (gateway, inbound) = Gateway::spawn_subscribed(connector);
    let outbound: Arc<dyn OutboundLink> = Arc::new(gateway.clone());
    let peer = self.assemble(&outbound, &device.id);
    peer.net.start();
    let routing = spawn_routing(gateway.clone(), peer.clone(), inbound);
    self.ota.adopt(&device.id, gateway.clone()).await;
    self.broadcast.adopt(&device.id, gateway.clone());

    let token = Uuid::now_v7();
    self.links.lock().unwrap().insert(
      device.id.clone(),
      Link {
        gateway: gateway.clone(),
        peer: peer.clone(),
        token,
        _routing: routing,
      },
    );

    let watching = self.clone();
    let watched_id = device.id.clone();
    let dying = gateway.connection().closed();
    tokio::spawn(async move {
      dying.await;
      watching.link_died(&watched_id, token).await;
    });

    let session = self.clone();
    let device = device.clone();
    tokio::spawn(async move {
      session.begin(&gateway, &peer, &device).await;
      session.sync_log_stream(&device.id, &gateway).await;
      session.sync_webapps(&gateway, &device.id).await;
      tracing::info!(device_id = %device.id, "a peer link is up");
    });
  }

  async fn sync_webapps(&self, gateway: &Gateway, device_id: &str) {
    let Ok(list) = gateway.webapp().list().await else {
      return;
    };
    let active = gateway.webapp().get_active().await.ok().and_then(|active| {
      active.id.map(|id| crate::api::ActiveWebapp {
        id: id.to_string(),
        name: active.name,
      })
    });
    self.observer.webapps_listed(
      device_id,
      list.webapps.into_iter().map(observer::webapp).collect(),
      active,
    );
  }

  async fn sync_log_stream(&self, device_id: &str, gateway: &Gateway) {
    {
      let held = self.log_stream.lock().unwrap();
      if !held.enabled || held.tokens.contains_key(device_id) {
        return;
      }
    }
    self.observer.hold_logs();
    let subscribed = gateway
      .system()
      .logs_subscribe(libbridgething::gateway::LogsSubscribe {
        source: libbridgething::LogSource::Daemon,
        levels: Vec::new(),
        filter: None,
      })
      .await;
    let Ok(reply) = subscribed else {
      self.observer.backfill_logs(Vec::new());
      return;
    };
    let orphaned = {
      let mut held = self.log_stream.lock().unwrap();
      if held.enabled && !held.tokens.contains_key(device_id) {
        held.tokens.insert(device_id.to_owned(), reply.token);
        None
      } else {
        Some(reply.token)
      }
    };
    if let Some(token) = orphaned {
      self.observer.backfill_logs(Vec::new());
      release_log_tap(gateway, token).await;
      return;
    }
    let tailed = tokio::time::timeout(
      LOG_BACKFILL_DEADLINE,
      gateway.system().logs_tail(libbridgething::gateway::LogsTail {
        source: libbridgething::LogSource::Daemon,
        levels: Vec::new(),
        filter: None,
        max_lines: LOG_BACKFILL_LINES,
      }),
    )
    .await;
    self.observer.backfill_logs(match tailed {
      Ok(Ok(reply)) => reply.entries,
      _ => Vec::new(),
    });
  }

  pub async fn set_device_log_streaming(&self, enabled: bool) {
    let tokens = {
      let mut held = self.log_stream.lock().unwrap();
      if held.enabled == enabled {
        return;
      }
      held.enabled = enabled;
      std::mem::take(&mut held.tokens)
    };
    if enabled {
      for (device_id, gateway) in self.live_gateways() {
        self.sync_log_stream(&device_id, &gateway).await;
      }
      return;
    }
    for (device_id, token) in tokens {
      let Some(gateway) = self.gateway_for(&device_id) else {
        continue;
      };
      release_log_tap(&gateway, token).await;
    }
  }

  async fn begin(&self, gateway: &Gateway, peer: &Arc<Peer>, device: &LinkDevice) {
    self
      .hub
      .set_geo_usable(match &peer.geo {
        Some(geo) => geo.can_provide_location().await,
        None => false,
      })
      .await;
    self.hub.peer_connected(&device.id).await;
    self.push_time(gateway).await;
    self.push_volume(gateway).await;

    if let Some(geo) = &peer.geo {
      geo.start().await;
    }
    if let Some(notifications) = &peer.notifications {
      notifications.start().await;
    }
    if let Some(phone) = &peer.phone {
      phone.start().await;
      phone.announce().await;
    }

    if let Some(extensions) = &self.extensions {
      extensions.peer_connected(&device.id, &device.name, gateway).await;
    }

    self.observer.peer_connected(SessionPeer {
      id: device.id.clone(),
      name: device.name.clone(),
      status: PeerLinkStatus::Connected,
      link_error: None,
    });
  }

  pub async fn push_volume(&self, gateway: &Gateway) {
    let Some(volume) = self.backends.volume.clone() else {
      return;
    };
    let Ok(level) = tokio::task::spawn_blocking(move || volume.snapshot()).await else {
      return;
    };
    let _ = gateway
      .audio()
      .volume_changed(VolumeChanged {
        level: level.level,
        muted: level.muted,
      })
      .await;
  }

  pub async fn push_time(&self, gateway: &Gateway) {
    let host = self.backends.host.clone();
    let Ok(clock) = tokio::task::spawn_blocking(move || host.clock()).await else {
      return;
    };
    let _ = gateway.time().snapshot(time_info(clock)).await;
  }

  pub async fn time_changed(&self) {
    for (_, gateway) in self.live_gateways() {
      self.push_time(&gateway).await;
    }
  }

  fn assemble(self: &Arc<Self>, outbound: &Arc<dyn OutboundLink>, device_id: &str) -> Arc<Peer> {
    let registry = self.hub.clone() as Arc<dyn ProviderRegistry>;
    let http = HttpExecutor::new(Arc::new(ForeignHttp::new(self.backends.http.clone())));

    let audio = (self.backends.audio.is_some() || self.backends.volume.is_some()).then(|| {
      let dispatcher = AudioDispatcher::new(
        self.backends.audio.clone(),
        self.backends.volume.clone(),
        outbound.clone(),
      );
      dispatcher.set_volume_authority(Some(self.hub.clone()));
      dispatcher
    });

    let observer = self.observer.clone();
    let geo = self.backends.geo.clone().map(|provider| {
      let hub = self.hub.clone();
      GeoDispatcher::new(
        provider,
        outbound.clone(),
        Arc::new(move |usable| {
          let hub = hub.clone();
          tokio::spawn(async move { hub.set_geo_usable(usable).await });
        }),
      )
    });

    let notifications = self.backends.notifications.clone().map(|backend| {
      let caps = self.caps.clone();
      NotificationDispatcher::new(
        backend,
        outbound.clone(),
        Arc::new(move || caps.lock().unwrap().notifications),
      )
    });

    let voice = self.backends.speech.clone().map(|recognizer| {
      VoiceDispatcher::new(VoiceDispatcherDeps {
        recognizer: Some(recognizer),
        controller: self.voice.clone(),
        link: outbound.clone(),
        resolver: Some(self.hub.clone()),
        observer: observer.clone(),
        device_id: device_id.to_owned(),
      })
    });

    Arc::new(Peer {
      device_id: device_id.to_owned(),
      asset: AssetDispatcher::new(registry.clone(), outbound.clone(), self.clock.clone()),
      audio,
      geo,
      library: LibraryDispatcher::new(registry.clone(), outbound.clone()),
      lyrics: LyricsDispatcher::new(
        registry.clone(),
        crate::lyrics::lrclib::LrclibResolver::new(http.clone()),
      ),
      net: NetDispatcher::new(
        outbound.clone(),
        http,
        Arc::new(ForeignWs::new(self.backends.ws.clone())),
      ),
      notifications,
      phone: self
        .backends
        .phone
        .clone()
        .map(|backend| PhoneDispatcher::new(backend, outbound.clone())),
      player: PlayerDispatcher::new(self.hub.clone(), outbound.clone()),
      system: SystemDispatcher::new(OtaLink::new(self.ota.clone(), device_id), observer.clone()),
      tunnel: TunnelDispatcher::new(outbound.clone(), self.clock.clone()),
      voice,
      webapp: WebappDispatcher::new(registry, observer.clone(), device_id),
      extensions: self.extensions.clone(),
      receiver: TransferReceiver::new(Arc::new(LinkAcks(outbound.clone())), self.clock.clone()),
      ota: OtaLink::new(self.ota.clone(), device_id),
      observer: observer.clone(),
    })
  }

  fn watch_updates(&self) -> JoinHandle<()> {
    let mut changes = self.ota.store_changes();
    let observer = self.observer.clone();
    tokio::spawn(async move {
      loop {
        match changes.recv().await {
          Ok(change) => observer.update_store_changed(change),
          Err(tokio::sync::broadcast::error::RecvError::Lagged(dropped)) => {
            tracing::warn!(dropped, "the host fell behind the update store");
          }
          Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
      }
    })
  }

  fn watch_arbitration(&self) -> JoinHandle<()> {
    let mut arbitrated = self.hub.now_playing().arbitrated();
    let observer = self.observer.clone();
    tokio::spawn(async move {
      while let Ok(state) = arbitrated.recv().await {
        observer.now_playing_changed(state);
      }
    })
  }

  async fn teardown(&self, device_id: &str) -> bool {
    let link = self.links.lock().unwrap().remove(device_id);
    let held = link.is_some();
    self.release(device_id, link).await;
    held
  }

  async fn link_died(&self, device_id: &str, token: Uuid) {
    let link = {
      let mut held = self.links.lock().unwrap();
      match held.get(device_id) {
        Some(link) if link.token == token => held.remove(device_id),
        _ => return,
      }
    };
    self.release(device_id, link).await;
    self.observer.peer_disconnected(device_id);
  }

  async fn release(&self, device_id: &str, link: Option<Link>) {
    self.log_stream.lock().unwrap().tokens.remove(device_id);
    self.broadcast.release(device_id);
    self.hub.peer_disconnected(device_id);
    let Some(link) = link else { return };
    tracing::info!(%device_id, "a peer link is being torn down");
    if let Some(extensions) = &self.extensions {
      extensions.peer_disconnected(device_id);
    }
    self.ota.release(device_id).await;
    link.peer.net.stop();
    link.peer.tunnel.stop();
    link.peer.receiver.stop();
    if let Some(audio) = &link.peer.audio {
      audio.stop().await;
    }
    if let Some(geo) = &link.peer.geo {
      geo.stop().await;
    }
    if let Some(notifications) = &link.peer.notifications {
      notifications.stop().await;
    }
    if let Some(phone) = &link.peer.phone {
      phone.stop().await;
    }
    if let Some(voice) = &link.peer.voice {
      voice.stop();
    }
  }

  pub fn capability_flags(&self) -> CapabilityFlags {
    *self.caps.lock().unwrap()
  }

  pub async fn set_capability_flags(&self, flags: CapabilityFlags) {
    *self.caps.lock().unwrap() = flags;
    if let Some(models) = &self.models {
      models.set_enabled(flags.voice_model).await;
    }
    self.hub.set_capability_flags(flags).await;
  }

  pub fn set_device_auto_resume(&self, device_id: &str, enabled: bool) {
    self.hub.set_device_auto_resume(device_id, enabled);
  }

  pub fn default_resume_target(&self) -> ResumeTarget {
    self.hub.default_resume_target()
  }

  pub fn set_device_resume_target(&self, device_id: &str, target: ResumeTarget) {
    self.hub.set_device_resume_target(device_id, target);
  }

  pub async fn set_ota_poll_config(&self, config: Option<OtaPollConfig>) {
    *self.poll_config.lock().unwrap() = config.clone();
    let mapped = config.map(|config| bridgething_delivery::ota::poll::OtaPollConfig {
      root_url: config
        .root_url
        .unwrap_or_else(|| bridgething_delivery::ota::poll::OtaPollConfig::default().root_url),
      interval_seconds: config.interval_seconds.max(60),
      auto_push: config.auto_push,
    });
    self.ota.set_poll_config(mapped).await;
  }

  pub fn resources_for(&self, device_id: &str) -> Option<WebappResourceService> {
    let peer = self.peer_for(device_id)?;
    Some(
      WebappResourceService::new(
        self.blobs.clone() as Arc<dyn BlobStore>,
        self.resource_slots.clone() as Arc<dyn SlotIndex>,
        peer.receiver().clone(),
      )
      .with_fetch(self.fetch.clone(), self.cache_dir().join("webapp-resource")),
    )
  }

  pub fn blobs(&self) -> &Arc<FsBlobStore> {
    &self.blobs
  }

  pub fn fetch(&self) -> &Arc<dyn ArtifactFetch> {
    &self.fetch
  }

  pub fn cache_dir(&self) -> std::path::PathBuf {
    std::path::PathBuf::from(&self.config.cache_dir)
  }

  pub fn companion_update_dir(&self) -> std::path::PathBuf {
    std::path::PathBuf::from(&self.config.state_dir).join(COMPANION_UPDATE_DIR)
  }
}

struct LinkAcks(Arc<dyn OutboundLink>);

impl AckSink for LinkAcks {
  fn ack(&self, ack: TransferAck) {
    let link = self.0.clone();
    tokio::spawn(async move {
      let _ = link.event(GatewayToBridgeTransferMsgEvent::Ack(ack)).await;
    });
  }
}

async fn release_log_tap(gateway: &Gateway, token: String) {
  let _ = gateway
    .system()
    .logs_unsubscribe(libbridgething::gateway::LogsUnsubscribe { token })
    .await;
}

fn time_info(clock: HostClock) -> TimeInfo {
  TimeInfo {
    tz_iana: Some(clock.tz_iana),
    locale: Some(clock.locale),
    wall_clock_unix_s: Some(u32::try_from(clock.unix_seconds).unwrap_or(u32::MAX)),
    utc_offset_minutes: Some(clock.utc_offset_minutes),
    dst_offset_minutes: Some(clock.dst_offset_minutes),
  }
}

impl Session {
  pub fn provider_infos(&self) -> Vec<ProviderInfo> {
    let attached = self.hub.attached_ids();
    let auth = self.auth.lock().unwrap().clone();
    let connected = self.connected.lock().unwrap().clone();
    let registered = self.providers.lock().unwrap().clone();
    let info = |id: String, display_name: String, live: bool| ProviderInfo {
      available: true,
      connected: live && (connected.contains(&id) || attached.contains(&id)),
      auth_state: if live {
        auth.get(&id).map_or_else(idle_auth, project_auth)
      } else {
        idle_auth()
      },
      service_health: ServiceHealth {
        kind: ServiceHealthKind::Ok,
        retry_after_seconds: None,
      },
      display_name,
      id,
    };
    let mut infos: Vec<ProviderInfo> = self
      .catalog
      .entries()
      .iter()
      .map(|entry| {
        let live = registered.iter().any(|provider| provider.name() == entry.id());
        info(entry.id().to_owned(), entry.display_name().to_owned(), live)
      })
      .collect();
    for provider in &registered {
      if self.catalog.get(provider.name()).is_none() {
        infos.push(info(
          provider.name().to_owned(),
          provider.display_name().to_owned(),
          true,
        ));
      }
    }
    infos
  }

  pub fn voice_model_paths(&self) -> VoiceModelPaths {
    VoiceModelPaths {
      nlu_bundle_dir: self
        .models
        .as_ref()
        .and_then(|models| models.nlu_bundle())
        .map(|path| path.display().to_string()),
      asr_weights: self
        .models
        .as_ref()
        .and_then(|models| models.asr_weights())
        .map(|path| path.display().to_string()),
    }
  }

  pub fn debug(&self) -> CompanionDebug {
    let authority = self.hub.now_playing().authority();
    let holds = |scope: CompanionAuthorityScope| authority.scopes.contains(&scope);
    let mut auto_resume: Vec<DeviceAutoResume> = self
      .hub
      .auto_resume_prefs()
      .into_iter()
      .map(|(device_id, enabled)| DeviceAutoResume { device_id, enabled })
      .collect();
    auto_resume.sort_by(|left, right| left.device_id.cmp(&right.device_id));
    let mut resume_targets: Vec<DeviceResumeTarget> = self
      .hub
      .resume_target_prefs()
      .into_iter()
      .map(|(device_id, target)| DeviceResumeTarget { device_id, target })
      .collect();
    resume_targets.sort_by(|left, right| left.device_id.cmp(&right.device_id));
    let mut attached_providers = self.hub.attached_ids();
    attached_providers.sort();
    let mut linked_devices = self.linked_ids();
    linked_devices.sort();

    CompanionDebug {
      authority_playback_held: holds(CompanionAuthorityScope::NowPlayingPlayback),
      authority_metadata_held: holds(CompanionAuthorityScope::NowPlayingMetadata),
      authority_volume_held: holds(CompanionAuthorityScope::Volume),
      authority_app_bundle: authority.app_bundle,
      arbitrated_source: self.hub.now_playing().current_source(),
      library_source: ProviderRegistry::library(self.hub.as_ref()).map(|provider| provider.name().to_owned()),
      last_played_from: self.hub.last_played_from(),
      attached_providers,
      attached_schemes: self.hub.attached_schemes(),
      linked_devices,
      auto_resume,
      resume_targets,
      voice: VoiceDebug {
        has_model: self.voice.has_model(),
        armed_bundle: self.voice.armed_bundle(),
        transfer_allowed: self.large_transfers_allowed(),
        paths: self.voice_model_paths(),
      },
    }
  }

  pub async fn snapshot(&self) -> SessionSnapshot {
    let providers = self.provider_infos();
    let priority = self.priority.lock().unwrap().clone();
    let library = ProviderRegistry::library(self.hub.as_ref()).map(|provider| provider.name().to_owned());
    let ota_runs = self.ota.retained_runs().await;
    let ota_available = self.ota.retained_available().await;
    let ota_poll = self.ota.retained_poll_status().await;

    SessionSnapshot {
      host_info: SessionHostInfo {
        app_name: self.config.host.app_name.clone(),
        app_version: self.config.host.app_version.clone(),
        os_name: self.config.host.os_name.clone(),
        os_version: self.config.host.os_version.clone(),
        host_identifier: self.config.host.host_identifier.clone(),
        lib_version: env!("CARGO_PKG_VERSION").to_string(),
        libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
        adapter_version: String::new(),
      },
      providers,
      provider_priority: priority,
      library_provider: library,
      peers: self.observer.peers(),
      ancs_auth_statuses: self.observer.ancs_statuses(),
      now_playing: self.observer.now_playing(),
      device_meta: self.observer.device_metas(),
      capability_flags: *self.caps.lock().unwrap(),
      voice_model: self.models.as_ref().map_or(
        VoiceModelState {
          status: VoiceModelStatus::Absent,
          received_bytes: 0,
          total_bytes: 0,
          version: None,
          error: None,
        },
        |models| models.state(),
      ),
      ota_poll_config: self.poll_config.lock().unwrap().clone(),
      webapps: self.observer.webapps(),
      ota_runs: ota_runs.into_iter().map(Into::into).collect(),
      ota_available: ota_available.into_iter().map(Into::into).collect(),
      ota_poll: ota_poll.into(),
    }
  }
}

fn idle_auth() -> AuthState {
  AuthState {
    kind: AuthKind::Idle,
    user_code: None,
    verification_url: None,
    verification_url_complete: None,
    message: None,
  }
}

fn project_auth(state: &ProviderAuthState) -> AuthState {
  match state {
    ProviderAuthState::Pending {
      user_code,
      verification_url,
      verification_url_complete,
    } => AuthState {
      kind: AuthKind::Pending,
      user_code: user_code.clone(),
      verification_url: verification_url.clone(),
      verification_url_complete: verification_url_complete.clone(),
      message: None,
    },
    ProviderAuthState::Authenticated => AuthState {
      kind: AuthKind::Authenticated,
      ..idle_auth()
    },
    ProviderAuthState::Failed { reason } => AuthState {
      kind: AuthKind::Failed,
      message: Some(reason.clone()),
      ..idle_auth()
    },
  }
}
