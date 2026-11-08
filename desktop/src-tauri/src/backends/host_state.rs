use std::{
  collections::BTreeMap,
  sync::{
    Arc, Mutex,
    mpsc::{self, RecvTimeoutError},
  },
  thread,
  time::Duration,
};

use bridgething_companion::backend::{HostStateBackend, HostStateInbox};

const POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwaySource {
  Locked,
  Displays,
  Sleep,
}

impl AwaySource {
  fn bit(self) -> u8 {
    match self {
      Self::Locked => 1,
      Self::Displays => 2,
      Self::Sleep => 4,
    }
  }
}

#[derive(Default)]
struct AwayState {
  inbox: Option<Arc<HostStateInbox>>,
  sources: u8,
  reported: Option<bool>,
}

impl AwayState {
  fn report(&mut self) {
    let away = self.sources != 0;
    if let Some(inbox) = &self.inbox
      && self.reported != Some(away)
    {
      self.reported = Some(away);
      inbox.on_away(away);
    }
  }
}

#[derive(Default)]
pub struct Away(Mutex<AwayState>);

impl Away {
  pub fn set(&self, source: AwaySource, on: bool) {
    let mut held = self.0.lock().unwrap();
    if on {
      held.sources |= source.bit();
    } else {
      held.sources &= !source.bit();
    }
    held.report();
  }

  pub fn sleep(&self, within: Duration) {
    let inbox = {
      let mut held = self.0.lock().unwrap();
      held.sources |= AwaySource::Sleep.bit();
      held.reported = Some(true);
      held.inbox.clone()
    };
    if let Some(inbox) = inbox {
      inbox.on_sleeping(u32::try_from(within.as_millis()).unwrap_or(u32::MAX));
    }
  }

  fn attach(&self, inbox: Arc<HostStateInbox>) {
    let mut held = self.0.lock().unwrap();
    held.inbox = Some(inbox);
    held.reported = None;
    held.report();
  }

  fn detach(&self) {
    let mut held = self.0.lock().unwrap();
    held.inbox = None;
    held.sources = 0;
  }
}

#[derive(Default)]
struct Crossings(BTreeMap<u32, Option<bool>>);

impl Crossings {
  fn replace(&mut self, thresholds: &[u32]) {
    self.0 = thresholds.iter().map(|after_s| (*after_s, None)).collect();
  }

  fn forget(&mut self) {
    self.0.values_mut().for_each(|held| *held = None);
  }

  fn observe(&mut self, idle_s: u32) -> Vec<(u32, bool)> {
    self
      .0
      .iter_mut()
      .filter_map(|(after_s, held)| {
        let idle = idle_s >= *after_s;
        (*held != Some(idle)).then(|| {
          *held = Some(idle);
          (*after_s, idle)
        })
      })
      .collect()
  }

  fn next_look(&self, idle_s: u32) -> Option<Duration> {
    if self.0.values().any(|held| *held == Some(true)) {
      return Some(POLL);
    }
    let soonest = self.0.keys().map(|after_s| after_s.saturating_sub(idle_s)).min()?;
    Some(POLL.max(Duration::from_secs(soonest.into())))
  }
}

struct IdleState {
  inbox: Option<Arc<HostStateInbox>>,
  crossings: Crossings,
}

impl IdleState {
  fn look(&mut self, idle_s: u32) -> Option<Duration> {
    let crossed = self.crossings.observe(idle_s);
    if let Some(inbox) = &self.inbox {
      for (after_s, idle) in crossed {
        inbox.on_idle(after_s, idle);
      }
    }
    self.crossings.next_look(idle_s)
  }
}

pub struct IdleEdges {
  idle_seconds: fn() -> u32,
  state: Arc<Mutex<IdleState>>,
  poller: Mutex<Option<mpsc::Sender<bool>>>,
}

impl IdleEdges {
  pub fn new(idle_seconds: fn() -> u32) -> Self {
    Self {
      idle_seconds,
      state: Arc::new(Mutex::new(IdleState {
        inbox: None,
        crossings: Crossings::default(),
      })),
      poller: Mutex::new(None),
    }
  }

  fn start(&self, inbox: Arc<HostStateInbox>) {
    self.stop();
    {
      let mut held = self.state.lock().unwrap();
      held.inbox = Some(inbox);
      held.crossings.forget();
    }
    let (wake, woken) = mpsc::channel();
    let state = self.state.clone();
    let idle_seconds = self.idle_seconds;
    match thread::Builder::new()
      .name("bridgething-idle".to_owned())
      .spawn(move || poll(state, idle_seconds, woken))
    {
      Ok(_) => *self.poller.lock().unwrap() = Some(wake),
      Err(error) => tracing::warn!(%error, "the idle poller could not be started"),
    }
  }

  fn stop(&self) {
    if let Some(poller) = self.poller.lock().unwrap().take() {
      let _ = poller.send(false);
    }
    self.state.lock().unwrap().inbox = None;
  }

  fn watch(&self, thresholds: &[u32]) {
    {
      let mut held = self.state.lock().unwrap();
      held.crossings.replace(thresholds);
      if held.inbox.is_some() {
        held.look((self.idle_seconds)());
      }
    }
    if let Some(poller) = self.poller.lock().unwrap().as_ref() {
      let _ = poller.send(true);
    }
  }
}

fn poll(state: Arc<Mutex<IdleState>>, idle_seconds: fn() -> u32, woken: mpsc::Receiver<bool>) {
  loop {
    let next = state.lock().unwrap().look(idle_seconds());
    let running = match next {
      Some(wait) => match woken.recv_timeout(wait) {
        Ok(running) => running,
        Err(RecvTimeoutError::Timeout) => true,
        Err(RecvTimeoutError::Disconnected) => false,
      },
      None => woken.recv().unwrap_or(false),
    };
    if !running {
      return;
    }
  }
}

pub trait AwayWatch: Send + Sync {
  fn watch(&self, away: Arc<Away>);
  fn unwatch(&self);
}

pub struct HostSignals<W> {
  away: Arc<Away>,
  idle: IdleEdges,
  watcher: W,
}

impl<W: AwayWatch> HostSignals<W> {
  pub fn new(watcher: W, idle_seconds: fn() -> u32) -> Self {
    Self {
      away: Arc::default(),
      idle: IdleEdges::new(idle_seconds),
      watcher,
    }
  }
}

impl<W: AwayWatch> HostStateBackend for HostSignals<W> {
  fn start(&self, inbox: Arc<HostStateInbox>) {
    self.watcher.unwatch();
    self.away.detach();
    self.watcher.watch(self.away.clone());
    self.away.attach(inbox.clone());
    self.idle.start(inbox);
  }

  fn stop(&self) {
    self.idle.stop();
    self.watcher.unwatch();
    self.away.detach();
  }

  fn watch_idle(&self, thresholds_s: Vec<u32>) {
    self.idle.watch(&thresholds_s);
  }
}

#[cfg(test)]
mod tests {
  use std::sync::atomic::{AtomicU32, Ordering};

  use bridgething_companion::backend::HostEdge;
  use tokio::sync::mpsc::UnboundedReceiver;

  use super::*;

  static IDLE_S: AtomicU32 = AtomicU32::new(0);

  fn fake_idle() -> u32 {
    IDLE_S.load(Ordering::SeqCst)
  }

  fn drained(edges: &mut UnboundedReceiver<HostEdge>) -> Vec<HostEdge> {
    std::iter::from_fn(|| edges.try_recv().ok()).collect()
  }

  #[derive(Default)]
  struct Seeded {
    locked: bool,
    watching: Mutex<Option<Arc<Away>>>,
  }

  impl AwayWatch for Seeded {
    fn watch(&self, away: Arc<Away>) {
      away.set(AwaySource::Locked, self.locked);
      *self.watching.lock().unwrap() = Some(away);
    }

    fn unwatch(&self) {
      self.watching.lock().unwrap().take();
    }
  }

  #[test]
  fn away_is_the_or_of_its_sources_and_only_flips_are_reported() {
    let away = Away::default();
    let (inbox, mut edges) = HostStateInbox::channel();
    away.attach(inbox);

    away.set(AwaySource::Displays, true);
    away.set(AwaySource::Locked, true);
    away.set(AwaySource::Displays, false);
    away.set(AwaySource::Locked, false);
    away.set(AwaySource::Sleep, false);

    assert_eq!(
      drained(&mut edges),
      [HostEdge::Away(false), HostEdge::Away(true), HostEdge::Away(false)]
    );
  }

  #[test]
  fn going_to_sleep_reports_away_and_holds_until_the_report_settles_or_the_budget_runs_out() {
    let away = Arc::new(Away::default());
    let (inbox, mut edges) = HostStateInbox::channel();
    away.attach(inbox.clone());
    drained(&mut edges);

    let sleeping = {
      let away = away.clone();
      thread::spawn(move || away.sleep(Duration::from_secs(10)))
    };
    assert_eq!(edges.blocking_recv(), Some(HostEdge::Sleeping(1)));
    thread::sleep(Duration::from_millis(200));
    assert!(!sleeping.is_finished(), "sleep went ahead before the report settled");
    inbox.settle(1);
    sleeping.join().expect("sleep returns once the report settles");

    let held_from = std::time::Instant::now();
    away.sleep(Duration::from_millis(50));
    assert!(
      held_from.elapsed() < Duration::from_secs(1),
      "a report that never settles holds sleep only for the budget"
    );

    away.set(AwaySource::Sleep, false);
    assert_eq!(drained(&mut edges), [HostEdge::Sleeping(2), HostEdge::Away(false)]);
  }

  #[test]
  fn crossings_report_each_threshold_once_per_flip() {
    let mut crossings = Crossings::default();
    crossings.replace(&[30, 300]);

    assert_eq!(crossings.observe(10), [(30, false), (300, false)]);
    assert_eq!(crossings.observe(20), []);
    assert_eq!(crossings.observe(31), [(30, true)]);
    assert_eq!(crossings.observe(301), [(300, true)]);
    assert_eq!(crossings.observe(0), [(30, false), (300, false)]);
  }

  #[test]
  fn the_poller_sleeps_until_the_soonest_crossing_and_polls_while_any_is_idle() {
    let mut crossings = Crossings::default();
    assert_eq!(crossings.next_look(0), None, "nothing to watch parks the poller");

    crossings.replace(&[30, 300]);
    crossings.observe(10);
    assert_eq!(crossings.next_look(10), Some(Duration::from_secs(20)));

    crossings.observe(30);
    assert_eq!(
      crossings.next_look(30),
      Some(POLL),
      "an idle threshold flips back on any input"
    );
  }

  #[test]
  fn a_backend_reports_away_and_every_threshold_before_returning() {
    IDLE_S.store(120, Ordering::SeqCst);
    let signals = HostSignals::new(
      Seeded {
        locked: true,
        ..Seeded::default()
      },
      fake_idle,
    );
    let (inbox, mut edges) = HostStateInbox::channel();

    signals.start(inbox);
    assert_eq!(drained(&mut edges), [HostEdge::Away(true)]);

    signals.watch_idle(vec![60, 600]);
    assert_eq!(
      drained(&mut edges),
      [
        HostEdge::Idle {
          after_s: 60,
          idle: true
        },
        HostEdge::Idle {
          after_s: 600,
          idle: false
        },
      ]
    );

    signals.stop();
    assert!(signals.watcher.watching.lock().unwrap().is_none());
  }
}
