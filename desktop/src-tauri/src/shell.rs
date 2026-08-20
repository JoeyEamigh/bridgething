use std::{
  path::{Path, PathBuf},
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
};

use bridgething_companion::{
  api::{
    CapabilityFlags, CompanionBackends, CompanionConfig, CompanionSession, HostInfo, ModelPlatform,
    SpotifyProviderConfig,
  },
  backend::{GeoProvider, LinkDevice, NativeHttp, NativeWs},
};
use bridgething_delivery::{blob::FsBlobStore, ota::service::OtaService, webapp::WebappResourceService};
use bridgething_gateway::{Gateway, connect_ws, transport::WsConnector};
use tokio::sync::Notify;

use crate::{
  backends::{DesktopHost, FileSecrets, Platform, TracingLog},
  capabilities::Capabilities,
  hints::{Hint, HintSink, Relay},
  known_device::{KnownDevice, KnownDevices},
};

pub const DEFAULT_GATEWAY_URL: &str = "ws://127.0.0.1:8892/";

const GATEWAY_URL_ENV: &str = "BRIDGETHING_GATEWAY_URL";

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
  #[error("no writable application directories")]
  NoAppDirs,
  #[error("the daemon gateway at {url} did not accept a connection: {reason}")]
  Connect { url: String, reason: String },
  #[error("no link to a daemon")]
  NotConnected,
}

pub struct DesktopPaths {
  pub state_dir: PathBuf,
  pub cache_dir: PathBuf,
  pub config_dir: PathBuf,
}

impl DesktopPaths {
  pub fn xdg() -> Result<Self, ShellError> {
    let dirs = directories::ProjectDirs::from("com", "bridgething", "bridgething").ok_or(ShellError::NoAppDirs)?;
    Ok(Self {
      state_dir: dirs.data_dir().to_path_buf(),
      cache_dir: dirs.cache_dir().to_path_buf(),
      config_dir: dirs.config_dir().to_path_buf(),
    })
  }

  pub fn under(root: &Path) -> Self {
    Self {
      state_dir: root.join("state"),
      cache_dir: root.join("cache"),
      config_dir: root.join("config"),
    }
  }

  pub fn installed_nlu_bundle(&self) -> Option<PathBuf> {
    let root = self.state_dir.join("bridgething-nlu");
    let current = std::fs::read_to_string(root.join("current")).ok()?;
    Some(root.join(current.trim()))
  }
}

pub struct ShellConfig {
  pub gateway_url: String,
  pub paths: DesktopPaths,
}

impl ShellConfig {
  pub fn new(gateway_url: impl Into<String>, paths: DesktopPaths) -> Self {
    Self {
      gateway_url: gateway_url.into(),
      paths,
    }
  }

  pub fn from_env() -> Result<Self, ShellError> {
    Ok(Self::new(
      std::env::var(GATEWAY_URL_ENV).unwrap_or_else(|_| DEFAULT_GATEWAY_URL.to_owned()),
      DesktopPaths::xdg()?,
    ))
  }
}

pub struct Shell {
  session: Arc<CompanionSession>,
  gateway_url: String,
  selected: Mutex<Option<String>>,
  hints: Arc<dyn HintSink>,
  capabilities: Capabilities,
  known: KnownDevices,
  wake: Arc<Notify>,
  support: CapabilityFlags,
  geo: Option<Arc<dyn GeoProvider>>,
  log_streaming: AtomicBool,
}

fn capability_support(model_platform: Option<ModelPlatform>, backends: &CompanionBackends) -> CapabilityFlags {
  CapabilityFlags {
    geo: backends.geo.is_some(),
    notifications: backends.notifications.is_some(),
    net_fetch: true,
    net_ws: true,
    audio_tts: backends.audio.is_some(),
    voice_model: model_platform.is_some() && backends.model_validator.is_some(),
  }
}

fn spotify_provider() -> Option<SpotifyProviderConfig> {
  option_env!("BRIDGETHING_AUTH_PSK")
    .filter(|psk| !psk.is_empty())
    .map(|psk| SpotifyProviderConfig {
      worker_base: spotify::auth::DEFAULT_WORKER_BASE.to_owned(),
      psk: psk.to_owned(),
    })
}

impl Shell {
  pub fn create(config: ShellConfig, hints: Arc<dyn HintSink>) -> Result<Arc<Self>, ShellError> {
    for dir in [
      &config.paths.state_dir,
      &config.paths.cache_dir,
      &config.paths.config_dir,
    ] {
      std::fs::create_dir_all(dir).map_err(|_| ShellError::NoAppDirs)?;
    }

    let platform = Platform::detect();
    let geo = platform.geo.clone();
    let models = platform.models.clone();
    let model_platform = platform.model_platform;

    let backends = CompanionBackends {
      link: None,
      host: Arc::new(DesktopHost),
      http: Arc::new(NativeHttp::default()),
      ws: Arc::new(NativeWs::new()),
      secrets: Arc::new(FileSecrets::open(&config.paths.config_dir)),
      log: Arc::new(TracingLog),
      audio: platform.audio,
      volume: None,
      geo: platform.geo,
      notifications: platform.notifications,
      phone: None,
      media_sessions: None,
      speech: platform.speech,
      nlu: platform.nlu,
      apple_music: None,
      image: platform.image,
      model_validator: platform.model_validator,
      transfer_policy: None,
      connectivity: platform.connectivity,
      device_waker: None,
    };

    let support = capability_support(model_platform, &backends);
    let capabilities = Capabilities::open(&config.paths.config_dir, support);
    let known = KnownDevices::open(&config.paths.config_dir);
    let wake = Arc::new(Notify::new());

    let session = CompanionSession::create(
      CompanionConfig {
        host: HostInfo {
          app_name: "bridgething desktop".into(),
          app_version: env!("CARGO_PKG_VERSION").into(),
          os_name: std::env::consts::OS.into(),
          os_version: String::new(),
          host_identifier: host_identifier(&config.paths.config_dir),
        },
        capabilities: capabilities.get(),
        state_dir: config.paths.state_dir.to_string_lossy().into_owned(),
        cache_dir: config.paths.cache_dir.to_string_lossy().into_owned(),
        model_platform,
        spotify: spotify_provider(),
      },
      backends,
      Relay::new(Arc::clone(&hints), Arc::clone(&wake)),
    );
    models.bind(&session);

    Ok(Arc::new(Self {
      session,
      gateway_url: config.gateway_url,
      selected: Mutex::new(None),
      hints,
      capabilities,
      known,
      wake,
      support,
      geo,
      log_streaming: AtomicBool::new(false),
    }))
  }

  pub async fn set_log_streaming(&self, enabled: bool) {
    self.log_streaming.store(enabled, Ordering::Relaxed);
    self.session.set_device_log_streaming(enabled).await;
  }

  pub fn log_streaming(&self) -> bool {
    self.log_streaming.load(Ordering::Relaxed)
  }

  pub fn capability_support(&self) -> CapabilityFlags {
    self.support
  }

  pub async fn set_capability_flags(&self, flags: CapabilityFlags) {
    let woken = !self.capabilities.get().geo && flags.geo;
    self.capabilities.set(flags);
    self.session.set_capability_flags(flags).await;
    if woken && let Some(geo) = self.geo.clone() {
      let _ = tokio::task::spawn_blocking(move || geo.request_authorization()).await;
    }
  }

  pub async fn start(&self) {
    if let Err(error) = self.session.start().await {
      tracing::warn!(%error, "the companion session did not start cleanly");
    }
  }

  pub fn announce(&self, hint: Hint) {
    self.hints.emit(hint);
  }

  pub fn session(&self) -> &Arc<CompanionSession> {
    &self.session
  }

  pub fn ota(&self) -> &Arc<OtaService> {
    self.session.ota()
  }

  pub fn gateway_url(&self) -> &str {
    &self.gateway_url
  }

  pub fn peer(&self) -> Option<String> {
    let held = self.selected.lock().unwrap().clone();
    if let Some(device_id) = held
      && self.session.session().is_linked(&device_id)
    {
      return Some(device_id);
    }
    match self.linked_ids().as_slice() {
      [only] => Some(only.clone()),
      _ => None,
    }
  }

  pub fn linked_ids(&self) -> Vec<String> {
    self.session.session().linked_ids()
  }

  pub fn select(&self, device_id: Option<String>) {
    *self.selected.lock().unwrap() = device_id;
    self.hints.emit(Hint::bare(crate::hints::PEERS));
  }

  pub fn link(&self) -> Result<Gateway, ShellError> {
    let device_id = self.peer().ok_or(ShellError::NotConnected)?;
    self.session.gateway_for(&device_id).ok_or(ShellError::NotConnected)
  }

  pub async fn connect(&self, url: Option<String>) -> Result<String, ShellError> {
    let url = url.unwrap_or_else(|| self.gateway_url.clone());
    tracing::info!(%url, "the user asked the shell for a link");
    let device_id = self.dial(url, None).await?;
    *self.selected.lock().unwrap() = Some(device_id.clone());
    Ok(device_id)
  }

  pub async fn dial(&self, url: String, label: Option<String>) -> Result<String, ShellError> {
    tracing::debug!(%url, "the shell is dialing a gateway");
    let ws = connect_ws(&url).await.map_err(|error| ShellError::Connect {
      url: url.clone(),
      reason: error.to_string(),
    })?;
    let device = LinkDevice {
      id: url,
      name: label.clone().unwrap_or_else(|| "bridgething daemon".into()),
    };
    self.session.connect_direct(device.clone(), WsConnector::new(ws)).await;
    self.known.record(&device.id, label.as_deref());
    self.hints.emit(Hint::bare(crate::hints::KNOWN_DEVICES));
    Ok(device.id)
  }

  pub async fn disconnect(&self, device_id: Option<String>) {
    let Some(device_id) = device_id.or_else(|| self.peer()) else {
      return;
    };
    tracing::info!(%device_id, "the shell is dropping a link the user asked it to drop");
    if self.selected.lock().unwrap().as_deref() == Some(device_id.as_str()) {
      *self.selected.lock().unwrap() = None;
    }
    self.set_auto_connect(&device_id, false);
    self.session.disconnect_direct(&device_id).await;
  }

  pub fn known_devices(&self) -> Vec<KnownDevice> {
    self.known.list()
  }

  pub fn auto_connect_targets(&self, discovered: &[String]) -> Vec<String> {
    self.known.wanted(discovered)
  }

  pub fn set_auto_connect(&self, url: &str, enabled: bool) {
    if !self.known.set_auto_connect(url, enabled) {
      return;
    }
    self.hints.emit(Hint::bare(crate::hints::KNOWN_DEVICES));
    if enabled {
      self.wake.notify_one();
    }
  }

  pub fn forget_device(&self, url: &str) {
    if self.known.forget(url) {
      self.hints.emit(Hint::bare(crate::hints::KNOWN_DEVICES));
    }
  }

  pub fn is_linked(&self, device_id: &str) -> bool {
    self.session.session().is_linked(device_id)
  }

  pub fn wake(&self) -> Arc<Notify> {
    Arc::clone(&self.wake)
  }

  pub fn resources(&self) -> Result<WebappResourceService, ShellError> {
    let device_id = self.peer().ok_or(ShellError::NotConnected)?;
    self
      .session
      .session()
      .resources_for(&device_id)
      .ok_or(ShellError::NotConnected)
  }

  pub fn blobs(&self) -> &Arc<FsBlobStore> {
    self.session.session().blobs()
  }
}

fn host_identifier(config_dir: &Path) -> String {
  let path = config_dir.join("host-id");
  if let Ok(held) = std::fs::read_to_string(&path) {
    let held = held.trim();
    if !held.is_empty() {
      return held.to_owned();
    }
  }
  let fresh = uuid::Uuid::now_v7().to_string();
  if let Err(error) = std::fs::write(&path, &fresh) {
    tracing::warn!(%error, path = %path.display(), "the host identifier could not be kept; a fresh one is used each launch");
  }
  fresh
}
