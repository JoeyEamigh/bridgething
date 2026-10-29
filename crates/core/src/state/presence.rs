use std::{
  sync::{Arc, Mutex},
  time::Duration,
};

use libbridgething::{
  DisplayState, HostPresence, PresenceInfo, PresenceSource,
  client::{BridgeToClientPresenceMsg, PresenceSnapshot},
  wire::MsgMeta,
};
use tokio::{sync::Notify, task::JoinHandle, time::Instant};

use crate::{als::AlsManager, net::WireEventBus};

const FAILSAFE_FLOOR: Duration = Duration::from_secs(60);
const HEARTBEAT_MISSES: u32 = 3;
const LOCAL_WAKE: Duration = Duration::from_secs(30);
const DIM_CAP: f32 = 0.2;

#[derive(Debug, Default)]
struct Inner {
  host: Option<(HostPresence, Instant)>,
  wake_until: Option<Instant>,
  published: PresenceInfo,
}

#[derive(Debug, Clone)]
pub struct PresenceManager {
  inner: Arc<Mutex<Inner>>,
  poke: Arc<Notify>,
  als: AlsManager,
  bus: WireEventBus,
}

fn failsafe_timeout(host: &HostPresence) -> Option<Duration> {
  let beat = Duration::from_secs(u64::from(host.heartbeat_s?)) * HEARTBEAT_MISSES;
  Some(beat.max(FAILSAFE_FLOOR))
}

fn evaluate(inner: &Inner, now: Instant) -> (PresenceInfo, Option<Instant>) {
  let Some((host, seen)) = &inner.host else {
    return (PresenceInfo::default(), None);
  };
  let failsafe_at = failsafe_timeout(host).map(|timeout| *seen + timeout);
  let (mut display, mut source, mut deadline) = match failsafe_at {
    Some(at) if now >= at => (DisplayState::Off, PresenceSource::Failsafe, None),
    _ => (host.display, PresenceSource::Host, failsafe_at),
  };
  if display != DisplayState::Active
    && let Some(until) = inner.wake_until
    && now < until
  {
    display = DisplayState::Active;
    source = PresenceSource::LocalWake;
    deadline = Some(deadline.map_or(until, |d| d.min(until)));
  }
  (PresenceInfo { display, source }, deadline)
}

fn cap_for(display: DisplayState) -> f32 {
  match display {
    DisplayState::Active => 1.0,
    DisplayState::Dim => DIM_CAP,
    DisplayState::Off => 0.0,
  }
}

impl PresenceManager {
  pub fn init(bus: WireEventBus, als: AlsManager) -> (Self, JoinHandle<()>) {
    let manager = Self {
      inner: Arc::new(Mutex::new(Inner::default())),
      poke: Arc::new(Notify::new()),
      als,
      bus,
    };
    let handle = tokio::spawn(manager.clone().run());
    (manager, handle)
  }

  pub fn snapshot(&self) -> PresenceSnapshot {
    PresenceSnapshot {
      presence: self.inner.lock().unwrap().published,
    }
  }

  pub fn update(&self, host: HostPresence) {
    tracing::debug!(?host, "presence: host update");
    self.inner.lock().unwrap().host = Some((host, Instant::now()));
    self.poke.notify_one();
  }

  pub fn touch(&self) {
    let mut guard = self.inner.lock().unwrap();
    if guard.host.is_none() {
      return;
    }
    let now = Instant::now();
    let lit = guard.published.display == DisplayState::Active;
    guard.wake_until = Some(now + LOCAL_WAKE);
    drop(guard);
    if !lit {
      self.poke.notify_one();
    }
  }

  async fn run(self) {
    loop {
      let deadline = self.reevaluate().await;
      match deadline {
        Some(at) => {
          tokio::select! {
            () = self.poke.notified() => {}
            () = tokio::time::sleep_until(at) => {}
          }
        }
        None => self.poke.notified().await,
      }
    }
  }

  async fn reevaluate(&self) -> Option<Instant> {
    let (info, deadline, changed) = {
      let mut guard = self.inner.lock().unwrap();
      let (info, deadline) = evaluate(&guard, Instant::now());
      let changed = info != guard.published;
      guard.published = info;
      (info, deadline, changed)
    };
    if changed {
      tracing::info!(display = ?info.display, source = ?info.source, "presence: screen state changed");
      if let Err(err) = self.als.set_presence_cap(cap_for(info.display)).await {
        tracing::warn!(?err, "presence: could not set the backlight cap");
      }
      let event = BridgeToClientPresenceMsg::Changed(PresenceSnapshot { presence: info });
      if let Err(errors) = self.bus.broadcast(event, MsgMeta::Event).await {
        tracing::debug!(clients = errors.len(), "presence: broadcast reached no client");
      }
    }
    deadline
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn inner_with(display: DisplayState, heartbeat_s: Option<u16>, seen: Instant) -> Inner {
    Inner {
      host: Some((
        HostPresence {
          display,
          idle_s: None,
          heartbeat_s,
        },
        seen,
      )),
      ..Inner::default()
    }
  }

  fn state(inner: &Inner, at: Instant) -> (DisplayState, PresenceSource) {
    let (info, _) = evaluate(inner, at);
    (info.display, info.source)
  }

  #[test]
  fn unmanaged_until_a_host_reports() {
    let (info, deadline) = evaluate(&Inner::default(), Instant::now());
    assert_eq!(
      (info.display, info.source),
      (DisplayState::Active, PresenceSource::None)
    );
    assert!(deadline.is_none());
  }

  #[test]
  fn a_report_without_a_heartbeat_never_expires() {
    let t0 = Instant::now();
    let inner = inner_with(DisplayState::Dim, None, t0);
    let (info, deadline) = evaluate(&inner, t0 + Duration::from_secs(86_400));
    assert_eq!((info.display, info.source), (DisplayState::Dim, PresenceSource::Host));
    assert!(deadline.is_none());
  }

  #[test]
  fn a_missed_heartbeat_blanks_the_screen() {
    let t0 = Instant::now();
    let inner = inner_with(DisplayState::Active, Some(10), t0);
    let (info, deadline) = evaluate(&inner, t0 + Duration::from_secs(10));
    assert_eq!(
      (info.display, info.source),
      (DisplayState::Active, PresenceSource::Host)
    );
    assert_eq!(deadline, Some(t0 + FAILSAFE_FLOOR));
    assert_eq!(
      state(&inner, t0 + FAILSAFE_FLOOR),
      (DisplayState::Off, PresenceSource::Failsafe)
    );
  }

  #[test]
  fn a_slow_heartbeat_stretches_the_failsafe() {
    let t0 = Instant::now();
    let inner = inner_with(DisplayState::Active, Some(40), t0);
    assert_eq!(state(&inner, t0 + Duration::from_secs(119)).0, DisplayState::Active);
    assert_eq!(state(&inner, t0 + Duration::from_secs(120)).0, DisplayState::Off);
  }

  #[test]
  fn a_button_press_lights_a_dark_screen_until_it_lapses() {
    let t0 = Instant::now();
    let mut inner = inner_with(DisplayState::Off, Some(10), t0);
    inner.wake_until = Some(t0 + LOCAL_WAKE);
    let (info, deadline) = evaluate(&inner, t0 + Duration::from_secs(5));
    assert_eq!(
      (info.display, info.source),
      (DisplayState::Active, PresenceSource::LocalWake)
    );
    assert_eq!(deadline, Some(t0 + LOCAL_WAKE));
    assert_eq!(
      state(&inner, t0 + LOCAL_WAKE + Duration::from_secs(1)),
      (DisplayState::Off, PresenceSource::Host)
    );
  }

  #[test]
  fn a_button_press_also_overrides_the_failsafe() {
    let t0 = Instant::now();
    let mut inner = inner_with(DisplayState::Active, Some(10), t0);
    inner.wake_until = Some(t0 + Duration::from_secs(90));
    assert_eq!(state(&inner, t0 + Duration::from_secs(70)).0, DisplayState::Active);
    assert_eq!(state(&inner, t0 + Duration::from_secs(95)).0, DisplayState::Off);
  }

  #[test]
  fn each_state_has_its_own_cap() {
    assert_eq!(cap_for(DisplayState::Active), 1.0);
    assert_eq!(cap_for(DisplayState::Dim), DIM_CAP);
    assert_eq!(cap_for(DisplayState::Off), 0.0);
  }
}
