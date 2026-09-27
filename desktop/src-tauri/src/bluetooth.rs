use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  thread::JoinHandle,
};

#[cfg(target_os = "macos")]
use std::{
  io::{BufRead, BufReader},
  path::{Path, PathBuf},
  process::{Child, Command, Stdio},
  sync::mpsc,
  thread,
  time::Duration,
};

use bridgething_delivery::discovery::Endpoint;
#[cfg(target_os = "macos")]
use tokio::sync::Notify;

#[cfg(target_os = "macos")]
use crate::hints::{ENDPOINTS, Hint, HintSink};

pub const BLUETOOTH_URL: &str = "ws://127.0.0.1:8893/";
#[cfg(target_os = "macos")]
const READY_LINE: &str = "BRIDGETHING_BLUETOOTH_RELAY_READY";

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
    let mut child: Option<Child> = None;
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
        match Command::new(&binary).arg("--probe").status() {
          Ok(status) if status.success() => match spawn_ready(&binary) {
            Ok(started) => {
              child = Some(started);
              self.set_active(true, &hints, &wake);
            }
            Err(error) => tracing::warn!(%error, "the Bluetooth relay did not become ready"),
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

#[cfg(target_os = "macos")]
fn spawn_ready(binary: &Path) -> Result<Child, String> {
  let mut child = Command::new(binary)
    .arg("--parent-pid")
    .arg(std::process::id().to_string())
    .stdout(Stdio::piped())
    .spawn()
    .map_err(|error| error.to_string())?;
  let stdout = child.stdout.take().ok_or("the relay has no readiness pipe")?;
  let (sender, receiver) = mpsc::sync_channel(1);
  thread::spawn(move || {
    let mut line = String::new();
    let outcome = BufReader::new(stdout).read_line(&mut line);
    let _ = sender.send((outcome, line));
  });
  match receiver.recv_timeout(Duration::from_secs(5)) {
    Ok((Ok(length), line)) if length > 0 && line.trim() == READY_LINE => Ok(child),
    other => {
      let _ = child.kill();
      let _ = child.wait();
      Err(format!("readiness handshake failed: {other:?}"))
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
