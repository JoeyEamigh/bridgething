#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod asr;
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
mod portable;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod utterance;
#[cfg(target_os = "windows")]
mod windows;

use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::{Arc, OnceLock},
};

use bridgething_companion::{
  api::{CompanionSession, ModelPlatform, VoiceModelPaths},
  backend::{
    AudioBackend, ConnectivityMonitor, GeoProvider, HostClock, HostEnvironment, ImageScaler, LogLevel, LogSink,
    MediaSessionBackend, ModelArtifactValidator, NluModelRunner, NotificationBackend, SecretStore, SpeechRecognizer,
  },
};

use crate::store::{JsonFile, stored};

#[derive(Default)]
pub struct Platform {
  pub geo: Option<Arc<dyn GeoProvider>>,
  pub notifications: Option<Arc<dyn NotificationBackend>>,
  pub media_sessions: Option<Arc<dyn MediaSessionBackend>>,
  pub audio: Option<Arc<dyn AudioBackend>>,
  pub connectivity: Option<Arc<dyn ConnectivityMonitor>>,
  pub image: Option<Arc<dyn ImageScaler>>,
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
