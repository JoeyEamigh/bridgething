use std::{
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use bridgething_companion::api::{DeviceMeta, LogOrigin, SessionEvent, SessionEventSink};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Notify;

use crate::known_device::KnownDevices;

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
}

impl Relay {
  pub fn new(hints: Arc<dyn HintSink>, wake: Arc<Notify>, known: Arc<KnownDevices>) -> Arc<Self> {
    let (tx, rx) = std::sync::mpsc::channel();
    let sink = Arc::clone(&hints);
    std::thread::spawn(move || coalesce_logs(rx, sink));
    Arc::new(Self {
      hints,
      logs: tx,
      wake,
      known,
    })
  }

  fn adopt(&self, device_id: &str, meta: &DeviceMeta) {
    if meta.serial_number.is_empty() {
      return;
    }
    self
      .known
      .seen(&meta.serial_number, device_id, meta.nickname.as_deref());
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
