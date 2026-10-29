use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  thread::{self, JoinHandle},
  time::Duration,
};

use bridgething_companion::backend::{HostDisplay, PresenceBackend, PresenceInbox, PresenceReport};
use objc2_core_graphics::{CGDisplayIsAsleep, CGEventSource, CGEventSourceStateID, CGEventType, CGMainDisplayID};

const POLL: Duration = Duration::from_secs(1);

const ANY_INPUT: CGEventType = CGEventType(u32::MAX);

#[derive(Default)]
pub struct MacPresence {
  stop: Arc<AtomicBool>,
  worker: Mutex<Option<JoinHandle<()>>>,
}

fn idle_seconds() -> u32 {
  let secs = CGEventSource::seconds_since_last_event_type(CGEventSourceStateID::CombinedSessionState, ANY_INPUT);
  if secs.is_finite() && secs > 0.0 { secs as u32 } else { 0 }
}

fn read() -> PresenceReport {
  let display = if CGDisplayIsAsleep(CGMainDisplayID()) {
    HostDisplay::Off
  } else {
    HostDisplay::Active
  };
  PresenceReport {
    display,
    idle_s: idle_seconds(),
  }
}

impl PresenceBackend for MacPresence {
  fn start(&self, inbox: Arc<PresenceInbox>) {
    self.stop.store(false, Ordering::SeqCst);
    let stop = self.stop.clone();
    let worker = thread::Builder::new()
      .name("presence".to_owned())
      .spawn(move || {
        let mut last = None;
        while !stop.load(Ordering::SeqCst) {
          let report = read();
          if last != Some(report.display) {
            last = Some(report.display);
            inbox.on_changed(report.display, report.idle_s);
          }
          thread::sleep(POLL);
        }
      })
      .ok();
    if let Some(previous) = std::mem::replace(&mut *self.worker.lock().unwrap(), worker) {
      let _ = previous.join();
    }
  }

  fn stop(&self) {
    self.stop.store(true, Ordering::SeqCst);
    if let Some(worker) = self.worker.lock().unwrap().take() {
      let _ = worker.join();
    }
  }

  fn snapshot(&self) -> PresenceReport {
    read()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reads_a_plausible_state() {
    let report = MacPresence::default().snapshot();
    assert!(report.idle_s < 60 * 60 * 24 * 365);
  }
}
