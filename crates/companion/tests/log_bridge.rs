#[path = "rig/backends.rs"]
mod backends;
#[path = "rig/secrets.rs"]
mod secrets;

use std::sync::{Arc, Mutex};

use backends::{Heard, Offline, RigHost};
use bridgething_companion::{
  api::{CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo},
  backend::{LogLevel, LogSink},
  session::Session,
};
use secrets::MemorySecrets;

#[derive(Default)]
struct Recorded(Mutex<Vec<(LogLevel, String, String)>>);

impl LogSink for Recorded {
  fn on_line(&self, level: LogLevel, target: String, message: String) {
    self.0.lock().unwrap().push((level, target, message));
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_core_tracing_line_reaches_the_host_log_sink() {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let sink = Arc::new(Recorded::default());
  let backends = CompanionBackends {
    link: None,
    host: Arc::new(RigHost),
    http: Arc::new(Offline),
    ws: Arc::new(Offline),
    secrets: Arc::new(MemorySecrets::default()),
    log: sink.clone(),
    audio: None,
    volume: None,
    geo: None,
    notifications: None,
    phone: None,
    media_sessions: None,
    speech: None,
    nlu: None,
    apple_music: None,
    image: None,
    model_validator: None,
    transfer_policy: None,
    connectivity: None,
    device_waker: None,
    extensions: None,
  };
  let _session = Session::new(
    CompanionConfig {
      host: HostInfo {
        app_name: "log-test".into(),
        app_version: "0.0.0".into(),
        os_name: "linux".into(),
        os_version: "0".into(),
        host_identifier: "log-test".into(),
      },
      capabilities: CapabilityFlags {
        geo: false,
        notifications: false,
        net_fetch: false,
        net_ws: false,
        audio_tts: false,
        voice_model: false,
      },
      state_dir: spool.path().to_string_lossy().into_owned(),
      cache_dir: spool.path().to_string_lossy().into_owned(),
      model_platform: None,
      spotify: None,
    },
    backends,
    Arc::new(Heard::default()),
    Arc::new(Offline),
  );

  tracing::info!(target: "bridgething_companion::probe", detail = 7, "the line under test");
  tracing::trace!(target: "bridgething_companion::probe", "trace stays local");

  let held = sink.0.lock().unwrap();
  assert_eq!(held.len(), 1, "info forwards and trace does not, saw {held:?}");
  assert_eq!(held[0].0, LogLevel::Info);
  assert_eq!(held[0].1, "bridgething_companion::probe");
  assert_eq!(held[0].2, "the line under test detail=7");
}
