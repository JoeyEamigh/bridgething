use std::{
  collections::{HashMap, VecDeque},
  future::Future,
  net::SocketAddr,
  pin::Pin,
  sync::{Arc, Mutex, RwLock, Weak},
  time::SystemTime,
};

use libbridgething::{LogEntry, LogLevel, LogSource};
use tokio::{sync::broadcast, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{Event, Subscriber, field::Visit};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};
use uuid::Uuid;

use crate::bluetooth::Address;

const RING_CAPACITY: usize = 512;
const BROADCAST_CAPACITY: usize = 256;

const TAP_TARGET_DENYLIST: &[&str] = &[
  "bridgething::state::log_tap",
  "libbridgething::protocol",
  "bridgething::rfcomm::frame",
  "bridgething::net",
  "bridgething::ws::connection::send",
];

fn tap_denied(target: &str) -> bool {
  TAP_TARGET_DENYLIST.iter().any(|prefix| target.starts_with(prefix))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogOwner {
  Client(SocketAddr),
  Gateway(Option<Address>),
}

pub type LogSink = Box<dyn Fn(Arc<LogEntry>) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

pub type VerbosityControl = Box<dyn Fn(Option<LogLevel>) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct LogTap {
  inner: Arc<LogTapInner>,
}

#[derive(Debug)]
struct LogTapInner {
  ring: Mutex<VecDeque<LogEntry>>,
  events: broadcast::Sender<Arc<LogEntry>>,
  subscribers: RwLock<HashMap<Uuid, SubscriberEntry>>,
  tuning: Mutex<Tuning>,
}

#[derive(derive_more::Debug, Default)]
struct Tuning {
  #[debug(skip)]
  control: Option<VerbosityControl>,
  applied: Option<LogLevel>,
}

#[derive(Debug)]
struct SubscriberEntry {
  owner: LogOwner,
  verbosity: Option<LogLevel>,
  cancel: CancellationToken,
  _handle: JoinHandle<()>,
}

impl LogTapInner {
  fn wanted_verbosity(&self) -> Option<LogLevel> {
    self
      .subscribers
      .read()
      .expect("log tap subscribers poisoned")
      .values()
      .filter_map(|sub| sub.verbosity)
      .min()
  }

  fn retune(&self) {
    let mut tuning = self.tuning.lock().expect("log tap tuning poisoned");
    let wanted = self.wanted_verbosity();
    if tuning.applied == wanted {
      return;
    }
    tuning.applied = wanted;
    if let Some(control) = tuning.control.as_ref() {
      control(wanted);
    }
  }

  fn forget(&self, token: Uuid) {
    let removed = self
      .subscribers
      .write()
      .expect("log tap subscribers poisoned")
      .remove(&token);
    if removed.is_some() {
      self.retune();
    }
  }
}

#[derive(Debug, Clone)]
pub struct LogTapLayer {
  inner: Arc<LogTapInner>,
}

impl LogTap {
  pub fn new() -> (Self, LogTapLayer) {
    let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
    let inner = Arc::new(LogTapInner {
      ring: Mutex::new(VecDeque::with_capacity(RING_CAPACITY)),
      events,
      subscribers: RwLock::new(HashMap::new()),
      tuning: Mutex::new(Tuning::default()),
    });
    let layer = LogTapLayer { inner: inner.clone() };
    (Self { inner }, layer)
  }

  pub fn tail(&self, _source: LogSource, levels: &[LogLevel], filter: Option<&str>, max_lines: u32) -> Vec<LogEntry> {
    let guard = self.inner.ring.lock().expect("log tap ring poisoned");
    let mut out: Vec<LogEntry> = guard
      .iter()
      .rev()
      .filter(|entry| matches(entry, levels, filter))
      .take(max_lines as usize)
      .cloned()
      .collect();
    out.reverse();
    out
  }

  pub fn subscribe(
    &self,
    owner: LogOwner,
    sink: LogSink,
    _source: LogSource,
    levels: Vec<LogLevel>,
    filter: Option<String>,
    verbosity: Option<LogLevel>,
  ) -> Uuid {
    let token = Uuid::now_v7();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let mut rx = self.inner.events.subscribe();
    let tap: Weak<LogTapInner> = Arc::downgrade(&self.inner);
    let mut subscribers = self.inner.subscribers.write().expect("log tap subscribers poisoned");
    let handle = tokio::spawn(async move {
      loop {
        tokio::select! {
          biased;
          _ = cancel_clone.cancelled() => return,
          recv = rx.recv() => match recv {
            Ok(entry) => {
              if !matches(&entry, &levels, filter.as_deref()) {
                continue;
              }
              if !sink(entry).await {
                tracing::trace!("log tap subscriber send failed; closing subscription");
                break;
              }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
              tracing::trace!(%skipped, "log tap subscriber lagged");
              continue;
            }
            Err(broadcast::error::RecvError::Closed) => break,
          }
        }
      }
      if let Some(tap) = tap.upgrade() {
        tap.forget(token);
      }
    });
    subscribers.insert(
      token,
      SubscriberEntry {
        owner,
        verbosity,
        cancel,
        _handle: handle,
      },
    );
    drop(subscribers);
    self.inner.retune();
    token
  }

  pub fn set_verbosity(&self, owner: LogOwner, token: Uuid, verbosity: Option<LogLevel>) -> bool {
    let found = {
      let mut subscribers = self.inner.subscribers.write().expect("log tap subscribers poisoned");
      match subscribers.get_mut(&token).filter(|sub| sub.owner == owner) {
        Some(sub) => {
          sub.verbosity = verbosity;
          true
        }
        None => false,
      }
    };
    if found {
      self.inner.retune();
    }
    found
  }

  pub fn verbosity(&self) -> Option<LogLevel> {
    self.inner.wanted_verbosity()
  }

  pub fn unsubscribe(&self, token: Uuid) -> bool {
    let removed = self
      .inner
      .subscribers
      .write()
      .expect("log tap subscribers poisoned")
      .remove(&token);
    let Some(sub) = removed else {
      return false;
    };
    sub.cancel.cancel();
    self.inner.retune();
    true
  }

  pub fn drain_for_owner(&self, owner: LogOwner) -> Vec<Uuid> {
    let mut guard = self.inner.subscribers.write().expect("log tap subscribers poisoned");
    let tokens: Vec<Uuid> = guard
      .iter()
      .filter_map(|(token, sub)| (sub.owner == owner).then_some(*token))
      .collect();
    for token in &tokens {
      if let Some(sub) = guard.remove(token) {
        sub.cancel.cancel();
      }
    }
    drop(guard);
    self.inner.retune();
    tokens
  }
}

impl LogTapLayer {
  pub fn bind_verbosity(&self, control: impl Fn(Option<LogLevel>) + Send + Sync + 'static) {
    let mut tuning = self.inner.tuning.lock().expect("log tap tuning poisoned");
    let wanted = self.inner.wanted_verbosity();
    if wanted.is_some() {
      control(wanted);
    }
    *tuning = Tuning {
      control: Some(Box::new(control)),
      applied: wanted,
    };
  }
}

impl<S> Layer<S> for LogTapLayer
where
  S: Subscriber + for<'a> LookupSpan<'a>,
{
  fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
    let metadata = event.metadata();
    if tap_denied(metadata.target()) {
      return;
    }
    let level = match *metadata.level() {
      tracing::Level::TRACE => LogLevel::Trace,
      tracing::Level::DEBUG => LogLevel::Debug,
      tracing::Level::INFO => LogLevel::Info,
      tracing::Level::WARN => LogLevel::Warn,
      tracing::Level::ERROR => LogLevel::Error,
    };
    let mut visitor = MessageVisitor::default();
    event.record(&mut visitor);
    let entry = LogEntry {
      ts_unix_s: u32::try_from(
        SystemTime::now()
          .duration_since(SystemTime::UNIX_EPOCH)
          .map(|d| d.as_secs())
          .unwrap_or(0),
      )
      .unwrap_or(u32::MAX),
      level,
      target: metadata.target().to_string(),
      message: visitor.message,
    };

    let arc = Arc::new(entry.clone());
    {
      let mut guard = self.inner.ring.lock().expect("log tap ring poisoned");
      if guard.len() == RING_CAPACITY {
        guard.pop_front();
      }
      guard.push_back(entry);
    }
    let _ = self.inner.events.send(arc);
  }
}

#[derive(Default)]
struct MessageVisitor {
  message: String,
}

impl Visit for MessageVisitor {
  fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
    if field.name() == "message" {
      use std::fmt::Write;
      if !self.message.is_empty() {
        self.message.push(' ');
      }
      let _ = write!(self.message, "{value:?}");
    }
  }

  fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
    if field.name() == "message" {
      if !self.message.is_empty() {
        self.message.push(' ');
      }
      self.message.push_str(value);
    }
  }
}

fn matches(entry: &LogEntry, levels: &[LogLevel], filter: Option<&str>) -> bool {
  if !levels.is_empty() && !levels.contains(&entry.level) {
    return false;
  }
  if let Some(needle) = filter
    && !needle.is_empty()
    && !entry.target.contains(needle)
    && !entry.message.contains(needle)
  {
    return false;
  }
  true
}

#[cfg(test)]
mod tests {
  use super::*;

  fn entry(level: LogLevel, target: &str, message: &str) -> LogEntry {
    LogEntry {
      ts_unix_s: 0,
      level,
      target: target.into(),
      message: message.into(),
    }
  }

  #[test]
  fn matches_empty_levels_passes_all() {
    let e = entry(LogLevel::Trace, "bridgething::net", "hello");
    assert!(matches(&e, &[], None));
  }

  #[test]
  fn matches_filters_by_level() {
    let e = entry(LogLevel::Trace, "x", "y");
    assert!(!matches(&e, &[LogLevel::Info, LogLevel::Warn], None));
    assert!(matches(&e, &[LogLevel::Trace], None));
  }

  #[test]
  fn matches_substring_against_target_or_message() {
    let e = entry(LogLevel::Info, "bridgething::als", "brightness changed");
    assert!(matches(&e, &[], Some("als")));
    assert!(matches(&e, &[], Some("brightness")));
    assert!(!matches(&e, &[], Some("nope")));
  }

  #[test]
  fn tap_denied_matches_self_and_gateway_traffic_targets() {
    assert!(tap_denied("bridgething::state::log_tap"));
    assert!(tap_denied("libbridgething::protocol::encode"));
    assert!(tap_denied("libbridgething::protocol::decode"));
    assert!(tap_denied("bridgething::rfcomm::frame"));
    assert!(tap_denied("bridgething::net::connection"));
    assert!(tap_denied("bridgething::net::connman"));
    assert!(tap_denied("bridgething::ws::connection::send"));
    assert!(!tap_denied("bridgething::bluetooth::rfcomm"));
    assert!(!tap_denied("bridgething::rfcomm::decode"));
    assert!(!tap_denied("bridgething::als"));
  }

  #[test]
  fn layer_excludes_denied_targets_from_capture() {
    use tracing_subscriber::prelude::*;
    let (tap, layer) = LogTap::new();
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
      tracing::info!(target: "bridgething::als", "kept-normal");
      tracing::trace!(target: "bridgething::state::log_tap", "self-feedback");
      tracing::trace!(target: "libbridgething::protocol::encode", "codec-noise");
      tracing::trace!(target: "bridgething::rfcomm::frame", "gateway-traffic");
      tracing::trace!(target: "bridgething::ws::connection::send", "ws-send-feedback");
      tracing::trace!(target: "bridgething::net::connman", "net-send-feedback");
    });
    let messages: Vec<String> = tap
      .inner
      .ring
      .lock()
      .unwrap()
      .iter()
      .map(|e| e.message.clone())
      .collect();
    assert!(
      messages.iter().any(|m| m == "kept-normal"),
      "normal events must still be captured: {messages:?}"
    );
    assert!(
      messages.iter().all(|m| ![
        "self-feedback",
        "codec-noise",
        "gateway-traffic",
        "ws-send-feedback",
        "net-send-feedback"
      ]
      .contains(&m.as_str())),
      "denied targets must never enter the tap: {messages:?}"
    );
  }

  fn quiet_sink() -> LogSink {
    Box::new(|_| Box::pin(async { true }) as Pin<Box<dyn Future<Output = bool> + Send>>)
  }

  fn tuned() -> (LogTap, Arc<Mutex<Vec<Option<LogLevel>>>>) {
    let (tap, layer) = LogTap::new();
    let applied = Arc::new(Mutex::new(Vec::new()));
    let record = applied.clone();
    layer.bind_verbosity(move |verbosity| record.lock().unwrap().push(verbosity));
    (tap, applied)
  }

  fn open(tap: &LogTap, owner: LogOwner, verbosity: Option<LogLevel>) -> Uuid {
    tap.subscribe(owner, quiet_sink(), LogSource::Daemon, Vec::new(), None, verbosity)
  }

  fn desktop() -> LogOwner {
    LogOwner::Gateway(Some(Address([0xFE, 0xFE, 0, 0, 0, 1])))
  }

  fn phone() -> LogOwner {
    LogOwner::Gateway(Some(Address([0xFE, 0xED, 0, 0, 0, 2])))
  }

  #[tokio::test]
  async fn a_raised_stream_reverts_when_it_unsubscribes() {
    let (tap, applied) = tuned();
    let token = open(&tap, desktop(), Some(LogLevel::Trace));
    assert_eq!(tap.verbosity(), Some(LogLevel::Trace));
    assert!(tap.unsubscribe(token));
    assert_eq!(tap.verbosity(), None);
    assert_eq!(*applied.lock().unwrap(), vec![Some(LogLevel::Trace), None]);
  }

  #[tokio::test]
  async fn a_raised_stream_reverts_when_its_link_drops() {
    let (tap, applied) = tuned();
    open(&tap, desktop(), Some(LogLevel::Debug));
    tap.drain_for_owner(desktop());
    assert_eq!(tap.verbosity(), None);
    assert_eq!(applied.lock().unwrap().last(), Some(&None));
  }

  #[tokio::test]
  async fn the_most_verbose_live_stream_wins_and_the_next_one_takes_over() {
    let (tap, applied) = tuned();
    open(&tap, phone(), Some(LogLevel::Debug));
    let loud = open(&tap, desktop(), Some(LogLevel::Trace));
    assert_eq!(tap.verbosity(), Some(LogLevel::Trace));
    tap.unsubscribe(loud);
    assert_eq!(tap.verbosity(), Some(LogLevel::Debug));
    assert_eq!(
      *applied.lock().unwrap(),
      vec![Some(LogLevel::Debug), Some(LogLevel::Trace), Some(LogLevel::Debug)]
    );
  }

  #[tokio::test]
  async fn a_stream_at_the_default_never_touches_the_filter() {
    let (tap, applied) = tuned();
    let token = open(&tap, desktop(), None);
    tap.unsubscribe(token);
    assert!(applied.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn retuning_a_live_stream_moves_the_filter_and_only_its_owner_may() {
    let (tap, _applied) = tuned();
    let token = open(&tap, desktop(), None);
    assert!(!tap.set_verbosity(phone(), token, Some(LogLevel::Trace)));
    assert_eq!(tap.verbosity(), None);
    assert!(tap.set_verbosity(desktop(), token, Some(LogLevel::Debug)));
    assert_eq!(tap.verbosity(), Some(LogLevel::Debug));
    assert!(tap.set_verbosity(desktop(), token, None));
    assert_eq!(tap.verbosity(), None);
  }

  #[tokio::test]
  async fn a_stream_whose_sink_dies_stops_holding_the_filter_up() {
    let (tap, applied) = tuned();
    let dead: LogSink = Box::new(|_| Box::pin(async { false }) as Pin<Box<dyn Future<Output = bool> + Send>>);
    tap.subscribe(
      desktop(),
      dead,
      LogSource::Daemon,
      Vec::new(),
      None,
      Some(LogLevel::Trace),
    );
    let _ = tap
      .inner
      .events
      .send(Arc::new(entry(LogLevel::Info, "bridgething::als", "wake the sink")));
    let reverted = tokio::time::timeout(std::time::Duration::from_secs(2), async {
      while tap.verbosity().is_some() {
        tokio::task::yield_now().await;
      }
    })
    .await;
    assert!(reverted.is_ok(), "a dead sink kept the daemon at trace");
    assert_eq!(applied.lock().unwrap().last(), Some(&None));
  }

  #[tokio::test]
  async fn tail_returns_recent_first_within_max_lines() {
    let (tap, _layer) = LogTap::new();
    {
      let mut guard = tap.inner.ring.lock().unwrap();
      for i in 0..10 {
        guard.push_back(entry(LogLevel::Info, "t", &format!("m{i}")));
      }
    }
    let out = tap.tail(LogSource::Daemon, &[], None, 3);
    let messages: Vec<&str> = out.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, vec!["m7", "m8", "m9"]);
  }
}
