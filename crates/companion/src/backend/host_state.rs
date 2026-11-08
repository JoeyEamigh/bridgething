use std::{
  sync::{Arc, Condvar, Mutex},
  time::Duration,
};

use tokio::sync::mpsc;

/// Desktop only. `start` reports away (locked, displays off or dimmed, or about to sleep) at once and on every change.
/// `watch_idle` replaces the idle thresholds; report each one at once and on every crossing. A device's reply waits
/// up to a quarter second for a new threshold's first report. Before the computer sleeps, call `on_sleeping` and hold
/// sleep until it returns.
#[uniffi::export(with_foreign)]
pub trait HostStateBackend: Send + Sync {
  fn start(&self, inbox: Arc<HostStateInbox>);
  fn stop(&self);
  fn watch_idle(&self, thresholds_s: Vec<u32>);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEdge {
  Away(bool),
  Idle { after_s: u32, idle: bool },
  Sleeping(u64),
}

#[derive(Default)]
struct Sleeps {
  asked: u64,
  settled: u64,
}

#[derive(uniffi::Object)]
pub struct HostStateInbox {
  tx: mpsc::UnboundedSender<HostEdge>,
  sleeps: Mutex<Sleeps>,
  settled: Condvar,
}

impl HostStateInbox {
  pub fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<HostEdge>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
      Arc::new(Self {
        tx,
        sleeps: Mutex::default(),
        settled: Condvar::new(),
      }),
      rx,
    )
  }

  /// Every device has answered or given up on the away report carried by `HostEdge::Sleeping(sleep)`.
  pub fn settle(&self, sleep: u64) {
    let mut sleeps = self.sleeps.lock().unwrap();
    sleeps.settled = sleeps.settled.max(sleep);
    self.settled.notify_all();
  }
}

#[uniffi::export]
impl HostStateInbox {
  pub fn on_away(&self, away: bool) {
    let _ = self.tx.send(HostEdge::Away(away));
  }

  pub fn on_idle(&self, after_s: u32, idle: bool) {
    let _ = self.tx.send(HostEdge::Idle { after_s, idle });
  }

  /// Reports away because the computer is about to sleep, then blocks until every linked device has acked the report
  /// or given up waiting for its ack, or until `within_ms` passes.
  pub fn on_sleeping(&self, within_ms: u32) {
    let mut sleeps = self.sleeps.lock().unwrap();
    sleeps.asked += 1;
    let sleep = sleeps.asked;
    if self.tx.send(HostEdge::Sleeping(sleep)).is_err() {
      return;
    }
    let _ = self
      .settled
      .wait_timeout_while(sleeps, Duration::from_millis(within_ms.into()), |sleeps| {
        sleeps.settled < sleep
      });
  }
}
