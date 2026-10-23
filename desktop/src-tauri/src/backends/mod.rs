#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod asr;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub mod ffmpeg;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod geo;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod jpeg;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod models;
#[cfg(any(target_os = "linux", target_os = "windows", all(target_os = "macos", test)))]
mod nlu;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub mod portable;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod utterance;
#[cfg(target_os = "windows")]
mod windows;

use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::{Arc, Mutex, OnceLock},
};

use bridgething_companion::{
  api::{CompanionSession, ModelPlatform, VoiceModelPaths},
  backend::{
    AudioBackend, ConnectivityInbox, ConnectivityMonitor, GeoProvider, HostClock, HostEnvironment, ImageScaler,
    LogLevel, LogSink, MediaSessionBackend, MediaTranscoder, ModelArtifactValidator, NluModelRunner,
    NotificationBackend, SecretStore, SpeechRecognizer, StreamBackend, VolumeBackend,
  },
  store::{JsonFile, stored},
};
use tauri::async_runtime::JoinHandle;
use tokio::sync::watch;

#[derive(Default)]
pub struct Platform {
  pub geo: Option<Arc<dyn GeoProvider>>,
  pub notifications: Option<Arc<dyn NotificationBackend>>,
  pub media_sessions: Option<Arc<dyn MediaSessionBackend>>,
  pub stream: Option<Arc<dyn StreamBackend>>,
  pub audio: Option<Arc<dyn AudioBackend>>,
  pub volume: Option<Arc<dyn VolumeBackend>>,
  pub connectivity: Option<Arc<dyn ConnectivityMonitor>>,
  pub image: Option<Arc<dyn ImageScaler>>,
  pub transcoder: Option<Arc<dyn MediaTranscoder>>,
  pub speech: Option<Arc<dyn SpeechRecognizer>>,
  pub nlu: Option<Arc<dyn NluModelRunner>>,
  pub model_validator: Option<Arc<dyn ModelArtifactValidator>>,
  pub model_platform: Option<ModelPlatform>,
  pub models: ModelPaths,
}

impl Platform {
  pub fn detect(config_dir: &Path) -> Self {
    #[cfg(target_os = "macos")]
    {
      macos::platform(config_dir)
    }
    #[cfg(target_os = "linux")]
    {
      linux::platform(config_dir)
    }
    #[cfg(target_os = "windows")]
    {
      windows::platform(config_dir)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
      let _ = config_dir;
      Self::default()
    }
  }
}

#[derive(Clone)]
pub struct Online(watch::Sender<bool>);

impl Default for Online {
  fn default() -> Self {
    Self(watch::Sender::new(true))
  }
}

impl Online {
  pub fn watch(&self) -> watch::Receiver<bool> {
    self.0.subscribe()
  }

  pub fn tee(&self, monitor: Arc<dyn ConnectivityMonitor>) -> Arc<dyn ConnectivityMonitor> {
    Arc::new(Teed {
      monitor,
      online: self.clone(),
      pump: Mutex::new(None),
    })
  }

  fn changed(&self, online: bool) {
    self
      .0
      .send_if_modified(|held| std::mem::replace(held, online) != online);
  }
}

struct Teed {
  monitor: Arc<dyn ConnectivityMonitor>,
  online: Online,
  pump: Mutex<Option<JoinHandle<()>>>,
}

impl ConnectivityMonitor for Teed {
  fn start(&self, inbox: Arc<ConnectivityInbox>) {
    let (tee, mut edges) = ConnectivityInbox::channel();
    let online = self.online.clone();
    let pump = tauri::async_runtime::spawn(async move {
      while let Some(edge) = edges.recv().await {
        online.changed(edge);
        inbox.on_changed(edge);
      }
    });
    if let Some(previous) = self.pump.lock().unwrap().replace(pump) {
      previous.abort();
    }
    self.monitor.start(tee);
  }

  fn stop(&self) {
    self.monitor.stop();
    if let Some(pump) = self.pump.lock().unwrap().take() {
      pump.abort();
    }
  }
}

#[derive(Clone, Default)]
pub struct ModelPaths(Arc<OnceLock<Box<dyn Fn() -> VoiceModelPaths + Send + Sync>>>);

impl ModelPaths {
  pub fn bind(&self, session: &Arc<CompanionSession>) {
    let session = Arc::downgrade(session);
    self.answered_by(move || {
      session
        .upgrade()
        .map(|session| session.voice_model_paths())
        .unwrap_or(VoiceModelPaths {
          nlu_bundle_dir: None,
          asr_weights: None,
        })
    });
  }

  pub fn answered_by(&self, resolve: impl Fn() -> VoiceModelPaths + Send + Sync + 'static) {
    let _ = self.0.set(Box::new(resolve));
  }

  pub fn nlu_bundle(&self) -> Option<PathBuf> {
    self.resolve(|paths| paths.nlu_bundle_dir)
  }

  pub fn asr_weights(&self) -> Option<PathBuf> {
    self.resolve(|paths| paths.asr_weights)
  }

  fn resolve(&self, pick: impl FnOnce(VoiceModelPaths) -> Option<String>) -> Option<PathBuf> {
    pick(self.0.get()?()).map(PathBuf::from)
  }
}

pub struct DesktopHost;

impl HostEnvironment for DesktopHost {
  fn clock(&self) -> HostClock {
    let now = chrono::Local::now();
    HostClock {
      tz_iana: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned()),
      locale: locale(),
      unix_seconds: now.timestamp().max(0) as u64,
      utc_offset_minutes: (now.offset().local_minus_utc() / 60) as i16,
      dst_offset_minutes: 0,
    }
  }
}

fn locale() -> String {
  let raw = std::env::var("LC_ALL")
    .or_else(|_| std::env::var("LC_MESSAGES"))
    .or_else(|_| std::env::var("LANG"))
    .unwrap_or_default();
  let tag = raw.split(['.', '@']).next().unwrap_or("").replace('_', "-");
  if tag.is_empty() || tag == "C" || tag == "POSIX" {
    "en-US".to_owned()
  } else {
    tag
  }
}

pub struct FileSecrets(JsonFile<BTreeMap<String, String>>);

impl FileSecrets {
  pub fn open(config_dir: &Path) -> Self {
    let path = config_dir.join("secrets.json");
    let held = stored(&path).unwrap_or_default();
    Self(JsonFile::new(path, "secret store", held))
  }
}

impl SecretStore for FileSecrets {
  fn get(&self, key: String) -> Option<String> {
    self.0.read(|held| held.get(&key).cloned())
  }

  fn set(&self, key: String, value: String) {
    self.0.write(|held| {
      held.insert(key, value);
    });
  }

  fn remove(&self, key: String) {
    self.0.write(|held| {
      held.remove(&key);
    });
  }

  fn get_blob(&self, key: String) -> Option<Vec<u8>> {
    self.get(key).map(String::into_bytes)
  }
}

pub struct TracingLog;

impl LogSink for TracingLog {
  fn on_line(&self, level: LogLevel, target: String, message: String) {
    match level {
      LogLevel::Trace => tracing::trace!(target: "bridgething::core", %target, "{message}"),
      LogLevel::Debug => tracing::debug!(target: "bridgething::core", %target, "{message}"),
      LogLevel::Info => tracing::info!(target: "bridgething::core", %target, "{message}"),
      LogLevel::Warn => tracing::warn!(target: "bridgething::core", %target, "{message}"),
      LogLevel::Error => tracing::error!(target: "bridgething::core", %target, "{message}"),
    }
  }
}

#[cfg(test)]
pub mod probe {
  use std::path::PathBuf;

  use bridgething_companion::api::VoiceModelPaths;

  use super::ModelPaths;

  pub fn fixtures() -> PathBuf {
    PathBuf::from(std::env::var("BRIDGETHING_VOICE_FIXTURES").expect("a directory of fetched voice artifacts"))
  }

  pub fn armed(paths: VoiceModelPaths) -> ModelPaths {
    let handle = ModelPaths::default();
    handle.answered_by(move || paths.clone());
    handle
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::*;

  #[derive(Default)]
  struct Radio(Mutex<Option<Arc<ConnectivityInbox>>>);

  impl ConnectivityMonitor for Radio {
    fn start(&self, inbox: Arc<ConnectivityInbox>) {
      *self.0.lock().unwrap() = Some(inbox);
    }

    fn stop(&self) {
      self.0.lock().unwrap().take();
    }
  }

  #[tokio::test]
  async fn the_companion_and_the_shell_both_hear_the_network_come_back() {
    let radio = Arc::new(Radio::default());
    let online = Online::default();
    let mut watching = online.watch();
    let teed = online.tee(radio.clone());
    let (inbox, mut companion) = ConnectivityInbox::channel();
    teed.start(inbox);
    let edge = |up: bool| radio.0.lock().unwrap().as_ref().unwrap().on_changed(up);

    edge(false);
    edge(true);

    for expected in [false, true] {
      let heard = tokio::time::timeout(Duration::from_secs(5), companion.recv()).await;
      assert_eq!(
        heard.ok().flatten(),
        Some(expected),
        "the companion still gets every edge"
      );
    }
    tokio::time::timeout(Duration::from_secs(5), watching.changed())
      .await
      .expect("the shell hears the edge")
      .expect("the sender is alive");
    assert!(*watching.borrow_and_update());
  }
}
