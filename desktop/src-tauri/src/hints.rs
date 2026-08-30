use std::{
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use bridgething_companion::api::{DeviceMeta, DeviceWebappsEntry, LogOrigin, SessionEvent, SessionEventSink};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::{extensions::Extensions, known_device::KnownDevices};

pub const SESSION: &str = "invalidate:session";
pub const ENDPOINTS: &str = "invalidate:endpoints";
pub const RESYNC: &str = "invalidate:all";
pub const PROVIDERS: &str = "invalidate:providers";
pub const PEERS: &str = "invalidate:peers";
pub const NOW_PLAYING: &str = "invalidate:now-playing";
pub const ANCS: &str = "invalidate:ancs";
pub const DEVICE_META: &str = "invalidate:device-meta";
pub const WEBAPPS: &str = "invalidate:webapps";
pub const WEBAPP_DOC: &str = "invalidate:webapp-doc";
pub const VOICE_MODEL: &str = "invalidate:voice-model";
pub const OTA_RUNS: &str = "invalidate:ota-runs";
pub const OTA_AVAILABLE: &str = "invalidate:ota-available";
pub const OTA_POLL: &str = "invalidate:ota-poll";
pub const LOGS: &str = "invalidate:logs";
pub const KNOWN_DEVICES: &str = "invalidate:known-devices";
pub const EXTENSIONS: &str = "invalidate:extensions";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Invalidation {
  pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
  pub name: &'static str,
  pub id: Option<String>,
}

impl Hint {
  pub fn bare(name: &'static str) -> Self {
    Self { name, id: None }
  }

  pub fn about(name: &'static str, id: impl Into<String>) -> Self {
    Self {
      name,
      id: Some(id.into()),
    }
  }
}

pub trait HintSink: Send + Sync {
  fn emit(&self, hint: Hint);
}

#[derive(Clone)]
pub struct Visibility(Arc<AtomicBool>);

impl Visibility {
  pub fn shown() -> Self {
    Self(Arc::new(AtomicBool::new(true)))
  }

  pub fn hidden() -> Self {
    Self(Arc::new(AtomicBool::new(false)))
  }

  pub fn set(&self, visible: bool) {
    self.0.store(visible, Ordering::Relaxed);
  }

  pub fn get(&self) -> bool {
    self.0.load(Ordering::Relaxed)
  }
}

pub struct WindowHints<R: Runtime> {
  handle: AppHandle<R>,
  visible: Visibility,
}

impl<R: Runtime> WindowHints<R> {
  pub fn new(handle: AppHandle<R>, visible: Visibility) -> Self {
    Self { handle, visible }
  }
}

impl<R: Runtime> HintSink for WindowHints<R> {
  fn emit(&self, hint: Hint) {
    if !self.visible.get() {
      return;
    }
    if let Err(error) = self.handle.emit(hint.name, Invalidation { id: hint.id }) {
      tracing::warn!(%error, name = hint.name, "an invalidation hint could not be emitted");
    }
  }
}

pub struct Relay {
  hints: Arc<dyn HintSink>,
  logs: std::sync::mpsc::Sender<()>,
  wake: Arc<Notify>,
  known: Arc<KnownDevices>,
  extensions: Arc<Extensions>,
}

impl Relay {
  pub fn new(
    hints: Arc<dyn HintSink>,
    wake: Arc<Notify>,
    known: Arc<KnownDevices>,
    extensions: Arc<Extensions>,
  ) -> Arc<Self> {
    let (tx, rx) = std::sync::mpsc::channel();
    let sink = Arc::clone(&hints);
    std::thread::spawn(move || coalesce_logs(rx, sink));
    Arc::new(Self {
      hints,
      logs: tx,
      wake,
      known,
      extensions,
    })
  }

  fn adopt(&self, device_id: &str, meta: &DeviceMeta) {
    if meta.serial_number.is_empty() {
      return;
    }
    self
      .known
      .seen(&meta.serial_number, device_id, meta.nickname.as_deref());
    self.extensions.identified(device_id, &meta.serial_number);
    self.hints.emit(Hint::bare(KNOWN_DEVICES));
  }
}

fn coalesce_logs(rx: std::sync::mpsc::Receiver<()>, hints: Arc<dyn HintSink>) {
  const WINDOW: Duration = Duration::from_millis(150);

  while rx.recv().is_ok() {
    while rx.recv_timeout(WINDOW).is_ok() {}
    crate::logs::without_capture(|| hints.emit(Hint::bare(LOGS)));
  }
}

impl SessionEventSink for Relay {
  fn on_event(&self, event: SessionEvent) {
    if matches!(event, SessionEvent::Log { .. }) {
      let _ = self.logs.send(());
    }
    if let SessionEvent::DeviceMetaChanged { device_id, meta } = &event {
      self.adopt(device_id, meta);
    }
    if let SessionEvent::WebappsChanged { entry } = &event
      && entry.listed
    {
      self.extensions.reconcile(&entry.device_id, holding(entry));
    }
    if let SessionEvent::PeerDisconnected { device_id } = &event {
      self.extensions.link_gone(device_id);
    }
    if matches!(
      event,
      SessionEvent::PeerDisconnected { .. } | SessionEvent::PeerLinkFailed { .. }
    ) {
      self.wake.notify_one();
    }
    if let Some(hint) = hint_for(event) {
      self.hints.emit(hint);
    }
  }
}

fn holding(entry: &DeviceWebappsEntry) -> std::collections::BTreeSet<Uuid> {
  entry
    .webapps
    .iter()
    .filter_map(|webapp| Uuid::parse_str(&webapp.id).ok())
    .collect()
}

pub fn hint_for(event: SessionEvent) -> Option<Hint> {
  Some(match event {
    SessionEvent::ProvidersChanged { .. } => Hint::bare(PROVIDERS),
    SessionEvent::PeerConnected { peer } | SessionEvent::PeerLinkFailed { peer } => Hint::about(PEERS, peer.id),
    SessionEvent::PeerDisconnected { device_id } => Hint::about(PEERS, device_id),
    SessionEvent::NowPlayingChanged { .. } => Hint::bare(NOW_PLAYING),
    SessionEvent::AncsAuthStatusChanged { device_id, .. } => Hint::about(ANCS, device_id),
    SessionEvent::DeviceMetaChanged { device_id, .. } => Hint::about(DEVICE_META, device_id),
    SessionEvent::WebappsChanged { entry } => Hint::about(WEBAPPS, entry.device_id),
    SessionEvent::WebappDocChanged {
      device_id, webapp_id, ..
    } => Hint::about(WEBAPP_DOC, format!("{device_id}/{webapp_id}")),
    SessionEvent::VoiceModelStateChanged { .. } => Hint::bare(VOICE_MODEL),
    SessionEvent::VoiceTurnChanged { .. } | SessionEvent::CompanionUpdateProgress { .. } => return None,
    SessionEvent::OtaRunChanged { run } => Hint::about(OTA_RUNS, run.device_id),
    SessionEvent::OtaAvailableChanged { available } => Hint::about(OTA_AVAILABLE, available.device_id),
    SessionEvent::OtaPollChanged { .. } => Hint::bare(OTA_POLL),
    SessionEvent::Resumed => Hint::bare(SESSION),
    SessionEvent::Log {
      origin,
      level,
      target,
      message,
    } => {
      if origin == LogOrigin::Device {
        log(level, &target, &message);
      }
      return None;
    }
  })
}

pub const DEVICE_TARGET: &str = "bridgething::device";

fn log(level: bridgething_companion::backend::LogLevel, target: &str, message: &str) {
  use bridgething_companion::backend::LogLevel;
  match level {
    LogLevel::Trace => tracing::trace!(target: DEVICE_TARGET, %target, "{message}"),
    LogLevel::Debug => tracing::debug!(target: DEVICE_TARGET, %target, "{message}"),
    LogLevel::Info => tracing::info!(target: DEVICE_TARGET, %target, "{message}"),
    LogLevel::Warn => tracing::warn!(target: DEVICE_TARGET, %target, "{message}"),
    LogLevel::Error => tracing::error!(target: DEVICE_TARGET, %target, "{message}"),
  }
}

#[cfg(test)]
mod tests {
  use std::time::Instant;

  use bridgething_companion::api::{DeviceWebappsEntry, WebappInfo, WebappRole, WebappSource};
  use bridgething_io::{HttpDownloadSink, HttpExecutor, HttpRequest, HttpSink, HttpTransport};

  use super::*;
  use crate::{
    extensions::{Deps, sample_bundle},
    settings::Authorize,
  };

  const WEATHER: Uuid = Uuid::from_u128(0x2f0c_1a4b_0000_4000_8000_0000_0000_0001);
  const SETTLE: Duration = Duration::from_secs(10);

  struct Deaf;

  impl HintSink for Deaf {
    fn emit(&self, _hint: Hint) {}
  }

  struct Offline;

  impl HttpTransport for Offline {
    fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
      sink.fail("this test never reaches the network".to_owned());
    }

    fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      sink.on_failed("this test never reaches the network".to_owned());
    }
  }

  fn webapps_changed(listed: bool) -> SessionEvent {
    SessionEvent::WebappsChanged {
      entry: DeviceWebappsEntry {
        device_id: "sn-1".to_owned(),
        webapps: Vec::new(),
        active: None,
        listed,
      },
    }
  }

  fn listing(device: &str, holding: Uuid) -> SessionEvent {
    SessionEvent::WebappsChanged {
      entry: DeviceWebappsEntry {
        device_id: device.to_owned(),
        webapps: vec![WebappInfo {
          id: holding.to_string(),
          name: "weather".to_owned(),
          source: WebappSource::Installed,
          role: WebappRole::Standard,
          version: "1.0.0".to_owned(),
          provenance: None,
          description: None,
          icon_hash: None,
          settings_hash: None,
          overlay_hash: None,
          config: Vec::new(),
          permissions: Vec::new(),
          extension: None,
        }],
        active: None,
        listed: true,
      },
    }
  }

  fn named(device: &str, serial: &str) -> SessionEvent {
    SessionEvent::DeviceMetaChanged {
      device_id: device.to_owned(),
      meta: DeviceMeta {
        daemon_version: "1.0.0".to_owned(),
        libbridgething_version: "1.0.0".to_owned(),
        image_version: "1.0.0".to_owned(),
        app_name: "bridgething".to_owned(),
        os_name: "superbird".to_owned(),
        os_version: "1.0.0".to_owned(),
        channel: "dev".to_owned(),
        model_name: "car thing".to_owned(),
        serial_number: serial.to_owned(),
        nickname: None,
      },
    }
  }

  async fn wait_for(
    extensions: &Extensions,
    mut done: impl FnMut(&[crate::extensions::ExtensionEntry]) -> bool,
  ) -> bool {
    let deadline = Instant::now() + SETTLE;
    loop {
      if done(&extensions.list()) {
        return true;
      }
      if Instant::now() >= deadline {
        return false;
      }
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
  }

  #[tokio::test]
  async fn only_a_real_listing_decides_whether_an_extension_is_gone() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).expect("the state directory");

    let extensions = Extensions::init(&state);
    extensions.adopt(Some("sn-1"), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    extensions.spawn(Deps {
      http: HttpExecutor::new(Arc::new(Offline)),
      authorize: Arc::new(Authorize::default()),
      open_url: Arc::new(|_| Ok(())),
      hints: Arc::new(Deaf),
    });
    assert!(
      wait_for(&extensions, |held| held.len() == 1).await,
      "the supervisor picked the stored extension up"
    );

    let relay = Relay::new(
      Arc::new(Deaf),
      Arc::new(Notify::new()),
      Arc::new(KnownDevices::open(dir.path())),
      Arc::clone(&extensions),
    );

    relay.on_event(webapps_changed(false));
    extensions.set_enabled(WEATHER, false);
    assert!(
      wait_for(&extensions, |held| held.first().is_some_and(|entry| !entry.enabled)).await,
      "an active-webapp push lands before the device's list is read, so an empty entry there is not an uninstall"
    );

    relay.on_event(webapps_changed(true));
    assert!(
      wait_for(&extensions, <[_]>::is_empty).await,
      "the daemon's own listing says the webapp is gone, and that is what reaps the sidecar"
    );
  }

  #[tokio::test]
  async fn a_device_that_answered_at_two_addresses_lets_go_of_its_extension_when_it_is_forgotten() {
    const SERIAL: &str = "8558R481Q61R";
    const FIRST: &str = "ws://bridgething.local:8892/";
    const SECOND: &str = "ws://192.168.7.2:8892/";

    let dir = tempfile::tempdir().expect("a scratch directory");
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).expect("the state directory");

    let extensions = Extensions::init(&state);
    extensions.adopt(None, &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    extensions.spawn(Deps {
      http: HttpExecutor::new(Arc::new(Offline)),
      authorize: Arc::new(Authorize::default()),
      open_url: Arc::new(|_| Ok(())),
      hints: Arc::new(Deaf),
    });
    assert!(
      wait_for(&extensions, |held| held.len() == 1).await,
      "the supervisor picked the stored extension up"
    );

    let known = Arc::new(KnownDevices::open(dir.path()));
    let relay = Relay::new(
      Arc::new(Deaf),
      Arc::new(Notify::new()),
      Arc::clone(&known),
      Arc::clone(&extensions),
    );

    relay.on_event(named(FIRST, SERIAL));
    relay.on_event(listing(FIRST, WEATHER));
    relay.on_event(SessionEvent::PeerDisconnected {
      device_id: FIRST.to_owned(),
    });

    relay.on_event(named(SECOND, SERIAL));
    relay.on_event(listing(SECOND, WEATHER));

    let remembered = known.list();
    assert_eq!(remembered.len(), 1, "the same serial at a new address is one device");
    assert_eq!(remembered[0].id, SERIAL);

    extensions.forget_device(&remembered[0].id);
    assert!(
      wait_for(&extensions, <[_]>::is_empty).await,
      "a claim keyed on an address the device has since moved off is a claim nothing can drop, and the \
       sidecar outlives every app that could reach it"
    );
  }
}
