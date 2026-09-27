use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  thread::JoinHandle,
};

#[cfg(target_os = "macos")]
use std::{path::PathBuf, thread, time::Duration};

use bridgething_delivery::discovery::Endpoint;
#[cfg(target_os = "macos")]
use tokio::sync::Notify;

#[cfg(target_os = "macos")]
use crate::hints::{ENDPOINTS, Hint, HintSink};

pub const BLUETOOTH_URL: &str = "ws://127.0.0.1:8893/";

pub struct BluetoothRelay {
  active: AtomicBool,
  stopping: AtomicBool,
  worker: Mutex<Option<JoinHandle<()>>>,
}

impl BluetoothRelay {
  #[cfg(target_os = "macos")]
  pub fn start(binary: PathBuf, hints: Arc<dyn HintSink>, wake: Arc<Notify>) -> Arc<Self> {
    let relay = Arc::new(Self {
      active: AtomicBool::new(false),
      stopping: AtomicBool::new(false),
      worker: Mutex::new(None),
    });
    let worker = Arc::clone(&relay);
    let handle = thread::spawn(move || worker.drive(binary, hints, wake));
    *relay.worker.lock().unwrap() = Some(handle);
    relay
  }

  #[cfg(not(target_os = "macos"))]
  pub fn unavailable() -> Arc<Self> {
    Arc::new(Self {
      active: AtomicBool::new(false),
      stopping: AtomicBool::new(false),
      worker: Mutex::new(None),
    })
  }

  pub fn endpoint(&self) -> Option<Endpoint> {
    self.active.load(Ordering::Acquire).then(|| Endpoint {
      id: "local-bluetooth".into(),
      url: BLUETOOTH_URL.into(),
      host: "localhost".into(),
      nickname: Some("Car Thing (Bluetooth)".into()),
      serial: None,
      browsed: true,
    })
  }

  pub fn stop(&self) {
    self.stopping.store(true, Ordering::Release);
    if let Some(worker) = self.worker.lock().unwrap().take() {
      let _ = worker.join();
    }
  }

  #[cfg(target_os = "macos")]
  fn drive(self: Arc<Self>, binary: PathBuf, hints: Arc<dyn HintSink>, wake: Arc<Notify>) {
    let mut child: Option<std::process::Child> = None;
    while !self.stopping.load(Ordering::Acquire) {
      if let Some(running) = child.as_mut() {
        match running.try_wait() {
          Ok(None) => {}
          outcome => {
            tracing::debug!(?outcome, "the Bluetooth relay stopped");
            child = None;
            self.set_active(false, &hints, &wake);
          }
        }
      }
      if child.is_none() {
        match std::process::Command::new(&binary).arg("--probe").status() {
          Ok(status) if status.success() => match std::process::Command::new(&binary)
            .arg("--parent-pid")
            .arg(std::process::id().to_string())
            .spawn()
          {
            Ok(started) => {
              child = Some(started);
              self.set_active(true, &hints, &wake);
            }
            Err(error) => tracing::warn!(%error, "the Bluetooth relay did not start"),
          },
          Ok(_) => {}
          Err(error) => tracing::warn!(%error, "the paired Car Thing probe did not run"),
        }
      }
      thread::sleep(Duration::from_secs(3));
    }
    if let Some(mut child) = child {
      let _ = child.kill();
      let _ = child.wait();
    }
    self.set_active(false, &hints, &wake);
  }

  #[cfg(target_os = "macos")]
  fn set_active(&self, active: bool, hints: &Arc<dyn HintSink>, wake: &Arc<Notify>) {
    if self.active.swap(active, Ordering::AcqRel) != active {
      hints.emit(Hint::bare(ENDPOINTS));
      wake.notify_one();
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn inactive_relay_does_not_advertise_a_device() {
    let relay = BluetoothRelay {
      active: AtomicBool::new(false),
      stopping: AtomicBool::new(false),
      worker: Mutex::new(None),
    };
    assert!(relay.endpoint().is_none());
    relay.active.store(true, Ordering::Release);
    assert_eq!(relay.endpoint().unwrap().url, BLUETOOTH_URL);
  }
}
