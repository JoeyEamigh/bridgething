use std::sync::Arc;

use tokio::sync::mpsc;

/// How lit the device screen should be, as the host sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostDisplay {
  /// The display is on and the user is around.
  Active,
  /// The host is dimming its display.
  Dim,
  /// The display is asleep.
  Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceReport {
  pub display: HostDisplay,
  /// Seconds since the user last touched an input device.
  pub idle_s: u32,
}

/// What the host knows about its display and its user. The host reports facts only; the device decides
/// what to do with them from its own settings. `snapshot` is read on every heartbeat, and `start`
/// reports the moment `display` changes.
pub trait PresenceBackend: Send + Sync {
  fn start(&self, inbox: Arc<PresenceInbox>);
  fn stop(&self);
  fn snapshot(&self) -> PresenceReport;
}

pub struct PresenceInbox {
  tx: mpsc::UnboundedSender<PresenceReport>,
}

impl PresenceInbox {
  pub fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<PresenceReport>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(Self { tx }), rx)
  }
}

impl PresenceInbox {
  pub fn on_changed(&self, display: HostDisplay, idle_s: u32) {
    let _ = self.tx.send(PresenceReport { display, idle_s });
  }
}
