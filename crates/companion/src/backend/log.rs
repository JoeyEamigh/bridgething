use std::{
  cell::Cell,
  fmt::Write as _,
  path::Path,
  sync::{Arc, Mutex, OnceLock},
};

use bridgething_delivery::log::{
  DeviceLogRing, LogOrigin as RingOrigin,
  store::{Level as StoreLevel, Limits as StoreLimits, LogStore as DurableStore},
};
use tracing_subscriber::layer::SubscriberExt;

use crate::api::{CompanionError, LogOrigin, SessionEvent, SessionEventSink};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum LogLevel {
  Trace,
  Debug,
  Info,
  Warn,
  Error,
}

pub(crate) fn ring_level(level: LogLevel) -> bridgething_delivery::seam::LogLevel {
  match level {
    LogLevel::Trace => bridgething_delivery::seam::LogLevel::Trace,
    LogLevel::Debug => bridgething_delivery::seam::LogLevel::Debug,
    LogLevel::Info => bridgething_delivery::seam::LogLevel::Info,
    LogLevel::Warn => bridgething_delivery::seam::LogLevel::Warn,
    LogLevel::Error => bridgething_delivery::seam::LogLevel::Error,
  }
}

pub(crate) fn api_level(level: bridgething_delivery::seam::LogLevel) -> LogLevel {
  match level {
    bridgething_delivery::seam::LogLevel::Trace => LogLevel::Trace,
    bridgething_delivery::seam::LogLevel::Debug => LogLevel::Debug,
    bridgething_delivery::seam::LogLevel::Info => LogLevel::Info,
    bridgething_delivery::seam::LogLevel::Warn => LogLevel::Warn,
    bridgething_delivery::seam::LogLevel::Error => LogLevel::Error,
  }
}

#[uniffi::export(with_foreign)]
pub trait LogSink: Send + Sync {
  fn on_line(&self, level: LogLevel, target: String, message: String);
}

static FORWARD: OnceLock<Mutex<Option<Arc<dyn LogSink>>>> = OnceLock::new();

thread_local! {
  static FORWARDING: Cell<bool> = const { Cell::new(false) };
}

fn forward_slot() -> &'static Mutex<Option<Arc<dyn LogSink>>> {
  FORWARD.get_or_init(|| Mutex::new(None))
}

pub(crate) fn forward_tracing(sink: Arc<dyn LogSink>) {
  *forward_slot().lock().unwrap() = Some(sink);
  static INSTALLED: OnceLock<()> = OnceLock::new();
  INSTALLED.get_or_init(|| {
    let subscriber = tracing_subscriber::registry().with(SinkLayer);
    let _ = tracing::subscriber::set_global_default(subscriber);
  });
}

struct SinkLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for SinkLayer {
  fn enabled(&self, metadata: &tracing::Metadata<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) -> bool {
    *metadata.level() <= tracing::Level::DEBUG
  }

  fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
    if FORWARDING.with(Cell::get) {
      return;
    }
    let Some(sink) = forward_slot().lock().unwrap().clone() else {
      return;
    };
    let mut line = RenderedLine::default();
    event.record(&mut line);
    let level = match *event.metadata().level() {
      tracing::Level::ERROR => LogLevel::Error,
      tracing::Level::WARN => LogLevel::Warn,
      tracing::Level::INFO => LogLevel::Info,
      _ => LogLevel::Debug,
    };
    FORWARDING.with(|flag| flag.set(true));
    sink.on_line(level, event.metadata().target().to_owned(), line.into_message());
    FORWARDING.with(|flag| flag.set(false));
  }
}

#[derive(Default)]
struct RenderedLine {
  message: String,
  extras: String,
}

impl RenderedLine {
  fn into_message(self) -> String {
    if self.message.is_empty() {
      self.extras.trim_start().to_owned()
    } else {
      format!("{}{}", self.message, self.extras)
    }
  }
}

impl tracing::field::Visit for RenderedLine {
  fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
    if field.name() == "message" {
      self.message.push_str(value);
    } else {
      let _ = write!(self.extras, " {}={}", field.name(), value);
    }
  }

  fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
    if field.name() == "message" {
      let _ = write!(self.message, "{value:?}");
    } else {
      let _ = write!(self.extras, " {}={:?}", field.name(), value);
    }
  }
}

#[derive(uniffi::Object)]
pub struct LogInbox {
  ring: Arc<DeviceLogRing>,
  events: Arc<dyn SessionEventSink>,
}

impl LogInbox {
  pub(crate) fn new(ring: Arc<DeviceLogRing>, events: Arc<dyn SessionEventSink>) -> Self {
    Self { ring, events }
  }
}

#[uniffi::export]
impl LogInbox {
  pub fn push(&self, level: LogLevel, target: String, message: String) {
    let record = self.ring.push(RingOrigin::Host, ring_level(level), &target, &message);
    self.events.on_event(SessionEvent::Log {
      origin: LogOrigin::Host,
      level,
      target,
      message,
      ts_unix_ms: record.ts_unix_ms,
    });
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LogStoreLevel {
  Trace,
  Debug,
  Info,
  Notice,
  Warn,
  Error,
  Fatal,
}

fn store_level(level: LogStoreLevel) -> StoreLevel {
  match level {
    LogStoreLevel::Trace => StoreLevel::Trace,
    LogStoreLevel::Debug => StoreLevel::Debug,
    LogStoreLevel::Info => StoreLevel::Info,
    LogStoreLevel::Notice => StoreLevel::Notice,
    LogStoreLevel::Warn => StoreLevel::Warn,
    LogStoreLevel::Error => StoreLevel::Error,
    LogStoreLevel::Fatal => StoreLevel::Fatal,
  }
}

fn archive_level(level: StoreLevel) -> LogStoreLevel {
  match level {
    StoreLevel::Trace => LogStoreLevel::Trace,
    StoreLevel::Debug => LogStoreLevel::Debug,
    StoreLevel::Info => LogStoreLevel::Info,
    StoreLevel::Notice => LogStoreLevel::Notice,
    StoreLevel::Warn => LogStoreLevel::Warn,
    StoreLevel::Error => LogStoreLevel::Error,
    StoreLevel::Fatal => LogStoreLevel::Fatal,
  }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LogArchive {
  pub id: String,
  pub started_at_ms: u64,
  pub bytes: u64,
  pub pinned: bool,
  pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LogStoreLine {
  pub ts_unix_ms: u64,
  pub level: LogStoreLevel,
  pub label: String,
  pub message: String,
}

#[derive(uniffi::Object)]
pub struct LogStore {
  store: DurableStore,
}

#[uniffi::export]
impl LogStore {
  #[uniffi::constructor]
  pub fn install(root: String) -> Arc<Self> {
    let store = DurableStore::new(StoreLimits::default());
    store.install(Path::new(&root));
    Arc::new(Self { store })
  }

  pub fn record(&self, level: LogStoreLevel, label: String, message: String) {
    self.store.record(store_level(level), &label, &message);
  }

  pub fn write(&self, line: String) {
    self.store.write(&line);
  }

  pub fn archives(&self) -> Vec<LogArchive> {
    self
      .store
      .archives()
      .into_iter()
      .map(|archive| LogArchive {
        id: archive.id,
        started_at_ms: archive.started_at_ms,
        bytes: archive.bytes,
        pinned: archive.pinned,
        current: archive.current,
      })
      .collect()
  }

  pub fn read(&self, id: String, limit: u32) -> Vec<LogStoreLine> {
    self
      .store
      .read(&id, limit as usize)
      .into_iter()
      .map(|line| LogStoreLine {
        ts_unix_ms: line.ts_unix_ms,
        level: archive_level(line.level),
        label: line.label,
        message: line.message,
      })
      .collect()
  }

  pub fn retained_bytes(&self) -> u64 {
    self.store.retained_bytes()
  }

  pub fn delete(&self, id: String) {
    self.store.delete(&id);
  }

  pub fn clear(&self) {
    self.store.clear();
  }

  pub fn export_to(&self, target: String, id: Option<String>) -> Result<String, CompanionError> {
    self
      .store
      .export_to(Path::new(&target), id.as_deref())
      .map(|path| path.display().to_string())
      .map_err(|failure| CompanionError::Device(format!("{target}: {failure}")))
  }

  pub fn flush(&self) {
    self.store.flush();
  }
}
