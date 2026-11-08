use std::{
  collections::BTreeMap,
  io::ErrorKind,
  os::fd::{AsRawFd, RawFd},
  sync::Arc,
  time::{Duration, Instant},
};

use bridgething_companion::backend::HostStateInbox;
use tokio::{
  io::{Interest, unix::AsyncFd},
  sync::watch,
};
use wayland_client::{
  ConnectError, Connection, Dispatch, DispatchError, Proxy, QueueHandle,
  backend::WaylandError,
  delegate_noop,
  globals::{BindError, GlobalError, GlobalListContents, registry_queue_init},
  protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
  ext_idle_notification_v1::{self, ExtIdleNotificationV1},
  ext_idle_notifier_v1::ExtIdleNotifierV1,
};

const QUIET_MS: u32 = 1_000;
const QUIET: Duration = Duration::from_millis(QUIET_MS as u64);

#[derive(Debug, thiserror::Error)]
enum Lost {
  #[error(transparent)]
  Connect(#[from] ConnectError),
  #[error(transparent)]
  Globals(#[from] GlobalError),
  #[error(transparent)]
  Bind(#[from] BindError),
  #[error(transparent)]
  Wire(#[from] WaylandError),
  #[error(transparent)]
  Dispatch(#[from] DispatchError),
  #[error(transparent)]
  Io(#[from] std::io::Error),
}

pub async fn watch(inbox: Arc<HostStateInbox>, mut thresholds: watch::Receiver<Vec<u32>>) {
  if let Err(error) = notified(&inbox, &mut thresholds).await {
    tracing::warn!(%error, "no wayland idle notifier; idle thresholds are never reported");
  }
}

async fn notified(inbox: &HostStateInbox, thresholds: &mut watch::Receiver<Vec<u32>>) -> Result<(), Lost> {
  let connection = Connection::connect_to_env()?;
  let (globals, mut queue) = registry_queue_init::<Seat>(&connection)?;
  let handle = queue.handle();
  let notifier: ExtIdleNotifierV1 = globals.bind(&handle, 1..=2, ())?;
  let seat: WlSeat = globals.bind(&handle, 1..=1, ())?;
  let _notification = if notifier.version() >= 2 {
    notifier.get_input_idle_notification(QUIET_MS, &seat, &handle, ())
  } else {
    notifier.get_idle_notification(QUIET_MS, &seat, &handle, ())
  };
  let socket = AsyncFd::with_interest(Socket(connection.backend().poll_fd().as_raw_fd()), Interest::READABLE)?;

  let mut seen = Seat::default();
  let mut quiet = Quiet::default();
  report(inbox, quiet.watch(&thresholds.borrow_and_update(), Instant::now()));

  loop {
    queue.dispatch_pending(&mut seen)?;
    let now = Instant::now();
    for idled in seen.changes.drain(..) {
      report(inbox, if idled { quiet.quiet(now) } else { quiet.stirred(now) });
    }
    report(inbox, quiet.settle(now));

    match queue.flush() {
      Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => {}
      flushed => flushed?,
    }
    let Some(read) = queue.prepare_read() else { continue };
    let deadline = quiet.deadline();

    tokio::select! {
      ready = socket.readable() => {
        let mut ready = ready?;
        match read.read() {
          Ok(_) => {}
          Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => ready.clear_ready(),
          Err(error) => return Err(error.into()),
        }
      }
      changed = thresholds.changed() => {
        if changed.is_err() {
          return Ok(());
        }
        report(inbox, quiet.watch(&thresholds.borrow_and_update(), Instant::now()));
      }
      () = wake(deadline) => {}
    }
  }
}

fn report(inbox: &HostStateInbox, edges: Vec<(u32, bool)>) {
  for (after_s, idle) in edges {
    inbox.on_idle(after_s, idle);
  }
}

async fn wake(deadline: Option<Instant>) {
  match deadline {
    Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
    None => std::future::pending().await,
  }
}

struct Socket(RawFd);

impl AsRawFd for Socket {
  fn as_raw_fd(&self) -> RawFd {
    self.0
  }
}

#[derive(Default)]
struct Seat {
  changes: Vec<bool>,
}

impl Dispatch<WlRegistry, GlobalListContents> for Seat {
  fn event(
    _: &mut Self,
    _: &WlRegistry,
    _: <WlRegistry as Proxy>::Event,
    _: &GlobalListContents,
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
  }
}

delegate_noop!(Seat: ignore WlSeat);
delegate_noop!(Seat: ExtIdleNotifierV1);

impl Dispatch<ExtIdleNotificationV1, ()> for Seat {
  fn event(
    seen: &mut Self,
    _: &ExtIdleNotificationV1,
    event: ext_idle_notification_v1::Event,
    _: &(),
    _: &Connection,
    _: &QueueHandle<Self>,
  ) {
    match event {
      ext_idle_notification_v1::Event::Idled => seen.changes.push(true),
      ext_idle_notification_v1::Event::Resumed => seen.changes.push(false),
      _ => {}
    }
  }
}

#[derive(Default)]
struct Quiet {
  since: Option<Instant>,
  thresholds: BTreeMap<u32, bool>,
}

impl Quiet {
  fn watch(&mut self, thresholds: &[u32], now: Instant) -> Vec<(u32, bool)> {
    self.thresholds = thresholds
      .iter()
      .map(|&after_s| (after_s, reached(self.since, after_s, now)))
      .collect();
    self
      .thresholds
      .iter()
      .map(|(&after_s, &idle)| (after_s, idle))
      .collect()
  }

  fn quiet(&mut self, now: Instant) -> Vec<(u32, bool)> {
    self.since = Some(now.checked_sub(QUIET).unwrap_or(now));
    self.settle(now)
  }

  fn stirred(&mut self, now: Instant) -> Vec<(u32, bool)> {
    self.since = None;
    self.settle(now)
  }

  fn settle(&mut self, now: Instant) -> Vec<(u32, bool)> {
    let mut edges = Vec::new();
    for (&after_s, held) in &mut self.thresholds {
      let idle = reached(self.since, after_s, now);
      if *held != idle {
        *held = idle;
        edges.push((after_s, idle));
      }
    }
    edges
  }

  fn deadline(&self) -> Option<Instant> {
    let since = self.since?;
    self
      .thresholds
      .iter()
      .filter(|&(_, &idle)| !idle)
      .map(|(&after_s, _)| since + Duration::from_secs(after_s.into()))
      .min()
  }
}

fn reached(since: Option<Instant>, after_s: u32, now: Instant) -> bool {
  since.is_some_and(|since| now.saturating_duration_since(since) >= Duration::from_secs(after_s.into()))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn at(origin: Instant, seconds: u64) -> Instant {
    origin + Duration::from_secs(seconds)
  }

  #[test]
  fn thresholds_read_active_until_the_seat_goes_quiet() {
    let origin = Instant::now();
    let mut quiet = Quiet::default();
    assert_eq!(quiet.watch(&[60, 5, 60], origin), vec![(5, false), (60, false)]);
    assert_eq!(quiet.deadline(), None, "an active seat has nothing to wait for");
    assert!(quiet.settle(at(origin, 3_600)).is_empty());
  }

  #[test]
  fn quiet_counts_from_the_last_input_and_crosses_each_threshold_once() {
    let origin = Instant::now();
    let mut quiet = Quiet::default();
    quiet.watch(&[5, 60], origin);

    assert!(quiet.quiet(at(origin, 1)).is_empty());
    assert_eq!(quiet.deadline(), Some(at(origin, 5)));
    assert_eq!(quiet.settle(at(origin, 5)), vec![(5, true)]);
    assert!(quiet.settle(at(origin, 6)).is_empty());
    assert_eq!(quiet.deadline(), Some(at(origin, 60)));
    assert_eq!(quiet.settle(at(origin, 61)), vec![(60, true)]);
    assert_eq!(quiet.deadline(), None);

    assert_eq!(quiet.stirred(at(origin, 70)), vec![(5, false), (60, false)]);
    assert!(quiet.stirred(at(origin, 71)).is_empty());
  }

  #[test]
  fn new_thresholds_read_the_quiet_already_underway() {
    let origin = Instant::now();
    let mut quiet = Quiet::default();
    quiet.watch(&[600], origin);
    quiet.quiet(at(origin, 1));

    assert_eq!(quiet.watch(&[30, 900], at(origin, 120)), vec![(30, true), (900, false)]);
    assert_eq!(quiet.deadline(), Some(at(origin, 900)));
  }
}
