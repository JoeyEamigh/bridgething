use std::{
  collections::{BTreeMap, BTreeSet, HashMap},
  convert::Infallible,
  future::Future,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_gateway::{Gateway, RequestFailure};
use libbridgething::gateway::{HostState, HostWatch};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
  backend::{HostEdge, HostStateBackend, HostStateInbox},
  dispatch::system::HostWatchSink,
};

const REPORT_WAIT: Duration = Duration::from_millis(250);
const ACK_WAIT: Duration = Duration::from_secs(2);

#[async_trait::async_trait]
pub(crate) trait HostStateLink: Send + Sync {
  async fn report(&self, state: HostState) -> Result<(), RequestFailure<Infallible>>;
}

#[async_trait::async_trait]
impl HostStateLink for Gateway {
  async fn report(&self, state: HostState) -> Result<(), RequestFailure<Infallible>> {
    self.system().host_state(state).await.map(|_| ())
  }
}

enum Command {
  Start,
  Stop(oneshot::Sender<()>),
  Link {
    device_id: String,
    link: Arc<dyn HostStateLink>,
  },
  Watch {
    device_id: String,
    watch: HostWatch,
  },
  Follow(bool),
  Forget(String),
}

pub(crate) struct HostReports {
  commands: mpsc::UnboundedSender<Command>,
  unstarted: Mutex<Option<(Reporter, mpsc::UnboundedReceiver<Command>)>>,
}

impl HostReports {
  pub(crate) fn new(backend: Arc<dyn HostStateBackend>, follow: bool) -> Arc<Self> {
    let (commands, inbound) = mpsc::unbounded_channel();
    let reporter = Reporter {
      backend,
      inbox: None,
      edges: None,
      follow,
      away: false,
      sleeps: Vec::new(),
      idle: BTreeMap::new(),
      thresholds: BTreeSet::new(),
      devices: HashMap::new(),
    };
    Arc::new(Self {
      commands,
      unstarted: Mutex::new(Some((reporter, inbound))),
    })
  }

  pub(crate) fn start(&self) {
    if let Some((reporter, inbound)) = self.unstarted.lock().unwrap().take() {
      tokio::spawn(reporter.run(inbound));
    }
    let _ = self.commands.send(Command::Start);
  }

  pub(crate) async fn stop(&self) {
    if self.unstarted.lock().unwrap().is_some() {
      return;
    }
    let (done, stopped) = oneshot::channel();
    if self.commands.send(Command::Stop(done)).is_ok() {
      let _ = stopped.await;
    }
  }

  pub(crate) fn follow(&self, enabled: bool) {
    let _ = self.commands.send(Command::Follow(enabled));
  }

  pub(crate) fn link(self: &Arc<Self>, device_id: &str, link: Arc<dyn HostStateLink>) -> Arc<dyn HostWatchSink> {
    let _ = self.commands.send(Command::Link {
      device_id: device_id.to_owned(),
      link,
    });
    Arc::new(HostLink {
      reports: self.clone(),
      device_id: device_id.to_owned(),
    })
  }

  pub(crate) fn forget(&self, device_id: &str) {
    let _ = self.commands.send(Command::Forget(device_id.to_owned()));
  }
}

struct HostLink {
  reports: Arc<HostReports>,
  device_id: String,
}

impl HostWatchSink for HostLink {
  fn watch(&self, watch: HostWatch) {
    let _ = self.reports.commands.send(Command::Watch {
      device_id: self.device_id.clone(),
      watch,
    });
  }
}

struct Courier {
  outbox: mpsc::UnboundedSender<(u64, HostState)>,
  settled: watch::Receiver<u64>,
  queued: u64,
}

impl Courier {
  fn spawn(device_id: String, link: Arc<dyn HostStateLink>) -> Self {
    let (outbox, reports) = mpsc::unbounded_channel();
    let (settle, settled) = watch::channel(0);
    tokio::spawn(deliver(device_id, link, reports, settle));
    Self {
      outbox,
      settled,
      queued: 0,
    }
  }

  fn send(&mut self, state: HostState) {
    self.queued += 1;
    let _ = self.outbox.send((self.queued, state));
  }

  fn settled(&self) -> impl Future<Output = ()> + Send + 'static {
    let mut settled = self.settled.clone();
    let queued = self.queued;
    async move {
      let _ = settled.wait_for(|report| *report >= queued).await;
    }
  }
}

async fn deliver(
  device_id: String,
  link: Arc<dyn HostStateLink>,
  mut reports: mpsc::UnboundedReceiver<(u64, HostState)>,
  settled: watch::Sender<u64>,
) {
  while let Some((report, state)) = reports.recv().await {
    match tokio::time::timeout(ACK_WAIT, link.report(state)).await {
      Ok(Ok(())) => tracing::trace!(%device_id, ?state, "the device took in the host state"),
      Ok(Err(failure)) => tracing::warn!(%device_id, ?state, ?failure, "the device did not take in the host state"),
      Err(_) => tracing::warn!(%device_id, ?state, "the device did not ack the host state within {ACK_WAIT:?}"),
    }
    settled.send_replace(report);
  }
}

struct LinkedDevice {
  courier: Courier,
  idle_after_s: Option<u32>,
  asked: bool,
  sent: Option<HostState>,
}

enum Event {
  Command(Option<Command>),
  Edge(Option<HostEdge>),
}

struct Reporter {
  backend: Arc<dyn HostStateBackend>,
  inbox: Option<Arc<HostStateInbox>>,
  edges: Option<mpsc::UnboundedReceiver<HostEdge>>,
  follow: bool,
  away: bool,
  sleeps: Vec<u64>,
  idle: BTreeMap<u32, bool>,
  thresholds: BTreeSet<u32>,
  devices: HashMap<String, LinkedDevice>,
}

async fn next_edge(edges: &mut Option<mpsc::UnboundedReceiver<HostEdge>>) -> Option<HostEdge> {
  match edges {
    Some(edges) => edges.recv().await,
    None => std::future::pending().await,
  }
}

impl Reporter {
  async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
    loop {
      let event = tokio::select! {
        command = commands.recv() => Event::Command(command),
        edge = next_edge(&mut self.edges) => Event::Edge(edge),
      };
      match event {
        Event::Command(None) => return,
        Event::Command(Some(command)) => self.command(command).await,
        Event::Edge(Some(edge)) => {
          self.apply(edge);
          self.publish();
        }
        Event::Edge(None) => self.edges = None,
      }
    }
  }

  async fn command(&mut self, command: Command) {
    match command {
      Command::Start => {
        let (inbox, edges) = HostStateInbox::channel();
        self.inbox = Some(inbox.clone());
        self.edges = Some(edges);
        self.sleeps.clear();
        let backend = self.backend.clone();
        let _ = tokio::task::spawn_blocking(move || backend.start(inbox)).await;
        self.push_thresholds().await;
      }
      Command::Stop(done) => {
        self.inbox = None;
        self.edges = None;
        self.sleeps.clear();
        let backend = self.backend.clone();
        let _ = tokio::task::spawn_blocking(move || backend.stop()).await;
        let _ = done.send(());
        return;
      }
      Command::Link { device_id, link } => {
        self.devices.insert(
          device_id.clone(),
          LinkedDevice {
            courier: Courier::spawn(device_id, link),
            idle_after_s: None,
            asked: false,
            sent: None,
          },
        );
        self.retune().await;
      }
      Command::Watch { device_id, watch } => {
        let Some(device) = self.devices.get_mut(&device_id) else {
          return;
        };
        device.idle_after_s = watch.idle_after_s;
        device.asked = true;
        device.sent = None;
        self.retune().await;
      }
      Command::Follow(enabled) => self.follow = enabled,
      Command::Forget(device_id) => {
        if self.devices.remove(&device_id).is_none() {
          return;
        }
        self.retune().await;
      }
    }
    self.publish();
  }

  async fn retune(&mut self) {
    let wanted: BTreeSet<u32> = self.devices.values().filter_map(|device| device.idle_after_s).collect();
    if wanted == self.thresholds {
      return;
    }
    self.thresholds = wanted;
    self.idle.retain(|after_s, _| self.thresholds.contains(after_s));
    self.push_thresholds().await;
  }

  async fn push_thresholds(&mut self) {
    if self.edges.is_none() {
      return;
    }
    let backend = self.backend.clone();
    let thresholds = self.thresholds.iter().copied().collect();
    let _ = tokio::task::spawn_blocking(move || backend.watch_idle(thresholds)).await;
    let deadline = tokio::time::Instant::now() + REPORT_WAIT;
    while self.thresholds.iter().any(|after_s| !self.idle.contains_key(after_s)) {
      let Some(edges) = self.edges.as_mut() else { return };
      match tokio::time::timeout_at(deadline, edges.recv()).await {
        Ok(Some(edge)) => self.apply(edge),
        Ok(None) => self.edges = None,
        Err(_) => return,
      }
    }
  }

  fn apply(&mut self, edge: HostEdge) {
    match edge {
      HostEdge::Away(away) => self.away = away,
      HostEdge::Idle { after_s, idle } => {
        if self.thresholds.contains(&after_s) {
          self.idle.insert(after_s, idle);
        }
      }
      HostEdge::Sleeping(sleep) => {
        self.away = true;
        self.sleeps.push(sleep);
      }
    }
  }

  fn publish(&mut self) {
    let away = self.follow.then_some(self.away);
    for device in self.devices.values_mut() {
      if !(self.follow || device.asked || device.sent.is_some()) {
        continue;
      }
      let state = HostState {
        away,
        idle: device
          .idle_after_s
          .and_then(|after_s| self.idle.get(&after_s).copied())
          .unwrap_or(false),
      };
      if device.sent == Some(state) {
        continue;
      }
      device.sent = Some(state);
      device.courier.send(state);
    }
    self.settle_sleeps();
  }

  fn settle_sleeps(&mut self) {
    let Some(sleep) = self.sleeps.drain(..).max() else {
      return;
    };
    let Some(inbox) = self.inbox.clone() else {
      return;
    };
    let settled: Vec<_> = self.devices.values().map(|device| device.courier.settled()).collect();
    tokio::spawn(async move {
      for device in settled {
        device.await;
      }
      inbox.settle(sleep);
    });
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const QUIET: Duration = Duration::from_millis(100);

  #[derive(Default)]
  struct FakeHost {
    inbox: Mutex<Option<Arc<HostStateInbox>>>,
    away: Mutex<bool>,
    idle_s: Mutex<u32>,
    watched: Mutex<Vec<Vec<u32>>>,
    answers_later: bool,
  }

  impl FakeHost {
    fn inbox(&self) -> Arc<HostStateInbox> {
      self.inbox.lock().unwrap().clone().expect("started")
    }

    fn watched(&self) -> Vec<Vec<u32>> {
      self.watched.lock().unwrap().clone()
    }
  }

  impl HostStateBackend for FakeHost {
    fn start(&self, inbox: Arc<HostStateInbox>) {
      inbox.on_away(*self.away.lock().unwrap());
      *self.inbox.lock().unwrap() = Some(inbox);
    }

    fn stop(&self) {
      self.inbox.lock().unwrap().take();
    }

    fn watch_idle(&self, thresholds_s: Vec<u32>) {
      let idle_s = *self.idle_s.lock().unwrap();
      if let Some(inbox) = self.inbox.lock().unwrap().clone() {
        let thresholds = thresholds_s.clone();
        let answer = move || {
          for after_s in thresholds {
            inbox.on_idle(after_s, idle_s >= after_s);
          }
        };
        if self.answers_later {
          std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            answer();
          });
        } else {
          answer();
        }
      }
      self.watched.lock().unwrap().push(thresholds_s);
    }
  }

  struct Device {
    heard: Mutex<Vec<HostState>>,
    acks: watch::Sender<bool>,
  }

  impl Default for Device {
    fn default() -> Self {
      Self {
        heard: Mutex::default(),
        acks: watch::Sender::new(true),
      }
    }
  }

  impl Device {
    fn holding_acks() -> Arc<Self> {
      let device = Self::default();
      device.acks.send_replace(false);
      Arc::new(device)
    }

    fn release_acks(&self) {
      self.acks.send_replace(true);
    }

    fn heard(&self) -> Vec<HostState> {
      self.heard.lock().unwrap().clone()
    }

    async fn hears(&self, count: usize) -> Vec<HostState> {
      for _ in 0..200 {
        if self.heard.lock().unwrap().len() >= count {
          break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
      }
      tokio::time::sleep(QUIET).await;
      self.heard()
    }
  }

  #[async_trait::async_trait]
  impl HostStateLink for Device {
    async fn report(&self, state: HostState) -> Result<(), RequestFailure<Infallible>> {
      self.heard.lock().unwrap().push(state);
      let mut acks = self.acks.subscribe();
      let _ = acks.wait_for(|flowing| *flowing).await;
      Ok(())
    }
  }

  fn sleeping(inbox: Arc<HostStateInbox>, within_ms: u32) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || inbox.on_sleeping(within_ms))
  }

  async fn still_waiting(sleep: &tokio::task::JoinHandle<()>) -> bool {
    tokio::time::sleep(QUIET).await;
    !sleep.is_finished()
  }

  fn state(away: Option<bool>, idle: bool) -> HostState {
    HostState { away, idle }
  }

  fn watch(idle_after_s: Option<u32>) -> HostWatch {
    HostWatch { idle_after_s }
  }

  async fn started(host: &Arc<FakeHost>, follow: bool) -> Arc<HostReports> {
    let reports = HostReports::new(host.clone(), follow);
    reports.start();
    for _ in 0..200 {
      if host.inbox.lock().unwrap().is_some() {
        return reports;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the backend never started");
  }

  #[test]
  fn reports_built_outside_a_runtime_start_once_the_session_does() {
    let host = Arc::new(FakeHost::default());
    let reports = HostReports::new(host.clone(), false);
    let runtime = tokio::runtime::Builder::new_current_thread()
      .enable_all()
      .build()
      .expect("runtime");
    runtime.block_on(async {
      tokio::time::timeout(Duration::from_secs(1), reports.stop())
        .await
        .expect("stopping reports that never started returns at once");
      reports.start();
      for _ in 0..200 {
        if host.inbox.lock().unwrap().is_some() {
          return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
      }
      panic!("the backend never started");
    });
  }

  #[tokio::test]
  async fn a_following_host_reports_away_when_a_device_links_without_being_asked() {
    let host = Arc::new(FakeHost::default());
    *host.away.lock().unwrap() = true;
    let reports = started(&host, true).await;
    let device = Arc::new(Device::default());

    reports.link("a", device.clone());

    assert_eq!(device.hears(1).await, vec![state(Some(true), false)]);
  }

  #[tokio::test]
  async fn a_host_that_does_not_follow_stays_silent_until_asked_and_never_reports_away() {
    let host = Arc::new(FakeHost::default());
    *host.away.lock().unwrap() = true;
    *host.idle_s.lock().unwrap() = 600;
    let reports = started(&host, false).await;
    let device = Arc::new(Device::default());

    let sink = reports.link("a", device.clone());
    assert_eq!(
      device.hears(1).await,
      vec![],
      "nothing was asked and nothing is followed"
    );

    sink.watch(watch(Some(300)));
    host.inbox().on_away(false);
    host.inbox().on_away(true);
    assert_eq!(device.hears(2).await, vec![state(None, true)]);
  }

  #[tokio::test]
  async fn every_watch_is_answered_at_once_even_when_nothing_changed() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, false).await;
    let device = Arc::new(Device::default());
    let sink = reports.link("a", device.clone());

    sink.watch(watch(None));
    sink.watch(watch(None));

    assert_eq!(device.hears(2).await, vec![state(None, false), state(None, false)]);
  }

  #[tokio::test]
  async fn a_new_threshold_is_answered_with_what_the_host_reports_for_it() {
    let host = Arc::new(FakeHost::default());
    *host.idle_s.lock().unwrap() = 600;
    let reports = started(&host, false).await;
    let device = Arc::new(Device::default());

    reports.link("a", device.clone()).watch(watch(Some(300)));

    assert_eq!(
      device.hears(1).await,
      vec![state(None, true)],
      "the reply carries the idle state, not a placeholder corrected a moment later"
    );
  }

  #[tokio::test]
  async fn a_backend_that_answers_from_its_own_thread_still_gets_one_exact_reply() {
    let host = Arc::new(FakeHost {
      answers_later: true,
      ..FakeHost::default()
    });
    *host.idle_s.lock().unwrap() = 600;
    let reports = started(&host, false).await;
    let device = Arc::new(Device::default());

    reports.link("a", device.clone()).watch(watch(Some(300)));

    assert_eq!(device.hears(2).await, vec![state(None, true)]);
  }

  #[tokio::test]
  async fn a_backend_that_never_answers_a_threshold_does_not_hold_the_reply() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, false).await;
    host.inbox.lock().unwrap().take();
    let device = Arc::new(Device::default());

    reports.link("a", device.clone()).watch(watch(Some(300)));

    assert_eq!(device.hears(1).await, vec![state(None, false)]);
  }

  #[tokio::test]
  async fn away_reaches_every_device_and_idle_only_the_ones_that_asked_each_when_it_changes() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, true).await;
    let away_only = Arc::new(Device::default());
    let idle_too = Arc::new(Device::default());
    reports.link("away", away_only.clone());
    reports.link("idle", idle_too.clone()).watch(watch(Some(60)));
    away_only.hears(1).await;
    idle_too.hears(2).await;

    let inbox = host.inbox();
    inbox.on_away(true);
    inbox.on_away(true);
    inbox.on_idle(60, true);
    inbox.on_idle(60, true);
    inbox.on_idle(120, true);
    inbox.on_away(false);

    assert_eq!(
      away_only.hears(3).await,
      vec![
        state(Some(false), false),
        state(Some(true), false),
        state(Some(false), false)
      ]
    );
    assert_eq!(
      idle_too.hears(5).await,
      vec![
        state(Some(false), false),
        state(Some(false), false),
        state(Some(true), false),
        state(Some(true), true),
        state(Some(false), true)
      ]
    );
  }

  #[tokio::test]
  async fn turning_following_on_or_off_tells_every_linked_device_once() {
    let host = Arc::new(FakeHost::default());
    *host.away.lock().unwrap() = true;
    let reports = started(&host, false).await;
    let quiet = Arc::new(Device::default());
    let watching = Arc::new(Device::default());
    reports.link("quiet", quiet.clone());
    reports.link("watching", watching.clone()).watch(watch(Some(60)));
    watching.hears(1).await;

    reports.follow(true);
    assert_eq!(quiet.hears(1).await, vec![state(Some(true), false)]);
    reports.follow(false);

    assert_eq!(
      quiet.hears(2).await,
      vec![state(Some(true), false), state(None, false)],
      "a computer that stops following says so, and the device lights"
    );
    assert_eq!(
      watching.hears(3).await,
      vec![state(None, false), state(Some(true), false), state(None, false)]
    );
  }

  #[tokio::test]
  async fn nothing_goes_out_while_nothing_changes() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, true).await;
    let device = Arc::new(Device::default());
    reports.link("a", device.clone()).watch(watch(Some(30)));
    device.hears(2).await;

    host.inbox().on_away(false);
    host.inbox().on_idle(30, false);

    assert_eq!(
      device.hears(3).await,
      vec![state(Some(false), false), state(Some(false), false)]
    );
  }

  #[tokio::test]
  async fn the_host_watches_the_union_of_thresholds_and_drops_a_forgotten_device() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, false).await;
    let short = Arc::new(Device::default());
    let long = Arc::new(Device::default());
    reports.link("short", short.clone()).watch(watch(Some(60)));
    reports.link("long", long.clone()).watch(watch(Some(600)));
    long.hears(1).await;
    assert_eq!(host.watched().last(), Some(&vec![60, 600]));

    host.inbox().on_idle(60, true);
    assert_eq!(short.hears(2).await, vec![state(None, false), state(None, true)]);
    assert_eq!(long.heard(), vec![state(None, false)], "600s has not passed");

    reports.forget("short");
    long.hears(2).await;
    assert_eq!(host.watched().last(), Some(&vec![600]));

    host.inbox().on_idle(60, false);
    host.inbox().on_idle(600, true);
    assert_eq!(long.hears(2).await, vec![state(None, false), state(None, true)]);
    assert_eq!(short.heard().len(), 2, "a forgotten device hears nothing more");
  }

  #[tokio::test]
  async fn going_to_sleep_waits_until_every_device_has_acked_the_away_report() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, true).await;
    let quick = Arc::new(Device::default());
    let slow = Device::holding_acks();
    reports.link("quick", quick.clone());
    reports.link("slow", slow.clone());
    quick.hears(1).await;

    let sleep = sleeping(host.inbox(), 10_000);

    assert!(
      still_waiting(&sleep).await,
      "sleep went ahead before the slow device acked"
    );
    assert_eq!(
      quick.heard(),
      [state(Some(false), false), state(Some(true), false)],
      "a device that holds its acks does not hold up another"
    );
    assert_eq!(
      slow.heard(),
      [state(Some(false), false)],
      "a device hears one report at a time, the next only once the last is acked"
    );

    slow.release_acks();
    tokio::time::timeout(Duration::from_secs(1), sleep)
      .await
      .expect("sleep goes ahead once every device has acked")
      .expect("the sleeping call returns");
    assert_eq!(slow.heard(), [state(Some(false), false), state(Some(true), false)]);
  }

  #[tokio::test]
  async fn a_device_that_stops_acking_holds_sleep_only_until_the_report_times_out() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, true).await;
    let device = Arc::new(Device::default());
    reports.link("a", device.clone());
    device.hears(1).await;
    device.acks.send_replace(false);
    let asleep_from = std::time::Instant::now();

    sleeping(host.inbox(), 10_000).await.expect("the sleeping call returns");

    let held = asleep_from.elapsed();
    assert!(
      held >= ACK_WAIT,
      "sleep went ahead before the ack wait ran out: {held:?}"
    );
    assert!(held < ACK_WAIT * 2, "a lost ack held sleep past its wait: {held:?}");

    host.inbox().on_away(false);
    assert_eq!(
      device.hears(3).await,
      [
        state(Some(false), false),
        state(Some(true), false),
        state(Some(false), false)
      ],
      "a report that timed out does not stop the next one"
    );
  }

  #[tokio::test]
  async fn the_backend_budget_bounds_the_wait_for_a_device_that_never_acks() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, true).await;
    let silent = Device::holding_acks();
    reports.link("silent", silent.clone());
    silent.hears(1).await;
    let asleep_from = std::time::Instant::now();

    sleeping(host.inbox(), 100).await.expect("the sleeping call returns");

    assert!(
      asleep_from.elapsed() < ACK_WAIT,
      "the backend's budget was ignored: {:?}",
      asleep_from.elapsed()
    );
  }

  #[tokio::test]
  async fn going_to_sleep_with_nothing_to_tell_returns_at_once() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, false).await;
    let silent = Device::holding_acks();
    reports.link("silent", silent.clone());
    let asleep_from = std::time::Instant::now();

    sleeping(host.inbox(), 10_000).await.expect("the sleeping call returns");

    assert!(asleep_from.elapsed() < Duration::from_secs(1));
    assert_eq!(silent.heard(), [], "a host that does not follow never reports away");
  }

  #[tokio::test]
  async fn a_stopped_session_stops_the_host_and_sends_nothing() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, true).await;
    let device = Arc::new(Device::default());
    reports.link("a", device.clone());
    device.hears(1).await;

    reports.stop().await;

    assert!(host.inbox.lock().unwrap().is_none(), "the backend was stopped");
    assert_eq!(device.hears(2).await, vec![state(Some(false), false)]);
  }

  #[tokio::test]
  async fn a_restart_hands_the_backend_the_thresholds_again() {
    let host = Arc::new(FakeHost::default());
    let reports = started(&host, true).await;
    let device = Arc::new(Device::default());
    reports.link("a", device.clone()).watch(watch(Some(45)));
    device.hears(2).await;
    reports.stop().await;
    let before = host.watched().len();

    *host.away.lock().unwrap() = true;
    reports.start();

    assert_eq!(
      device.hears(3).await,
      vec![
        state(Some(false), false),
        state(Some(false), false),
        state(Some(true), false)
      ]
    );
    assert_eq!(host.watched()[before..], [vec![45]]);
  }
}
