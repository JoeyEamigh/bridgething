use std::{
  cell::Cell,
  fmt::Write as _,
  path::Path,
  sync::{Arc, Mutex, OnceLock},
};

use bridgething_companion::{
  backend::{LogInbox, LogLevel},
  store::{JsonFile, stored},
};
use tracing_subscriber::{
  EnvFilter,
  layer::{Context, Layer, SubscriberExt as _},
  reload,
  util::SubscriberInitExt as _,
};

use crate::hints::DEVICE_TARGET;

const PENDING: usize = 512;

const DEFAULT_FILTER: &str = "warn,bridgething_desktop=debug,bridgething_companion=debug,\
bridgething_delivery=debug,bridgething_gateway=info,bridgething=info";
const VERBOSE_FILTER: &str = "warn,bridgething_desktop=trace,bridgething_companion=trace,\
bridgething_delivery=trace,bridgething_gateway=trace,bridgething=trace";

fn quiet() -> String {
  std::env::var("RUST_LOG")
    .ok()
    .filter(|held| !held.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_FILTER.to_owned())
}

fn filter(verbose: bool) -> EnvFilter {
  EnvFilter::new(if verbose { VERBOSE_FILTER.to_owned() } else { quiet() })
}

pub struct Verbosity {
  swap: Box<dyn Fn(bool) + Send + Sync>,
  kept: JsonFile<bool>,
}

impl Verbosity {
  fn new(kept: JsonFile<bool>, swap: impl Fn(bool) + Send + Sync + 'static) -> Self {
    Self {
      swap: Box::new(swap),
      kept,
    }
  }

  pub fn set(&self, verbose: bool) {
    (self.swap)(verbose);
    self.kept.set(verbose);
  }

  pub fn get(&self) -> bool {
    self.kept.read(|verbose| *verbose)
  }
}

fn kept(config_dir: Option<&Path>) -> JsonFile<bool> {
  const NOUN: &str = "log verbosity";
  match config_dir {
    Some(dir) => {
      let path = dir.join("verbose-logging.json");
      let held = stored(&path).unwrap_or(false);
      JsonFile::new(path, NOUN, held)
    }
    None => JsonFile::in_memory(NOUN, false),
  }
}

pub fn install(config_dir: Option<&Path>) -> Arc<Verbosity> {
  let kept = kept(config_dir);
  let (layer, handle) = reload::Layer::new(filter(kept.read(|verbose| *verbose)));
  tracing_subscriber::registry()
    .with(layer)
    .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
    .with(RingLayer)
    .init();
  Arc::new(Verbosity::new(kept, move |verbose| {
    if let Err(error) = handle.reload(filter(verbose)) {
      tracing::warn!(%error, "the log filter could not be swapped");
    }
  }))
}

struct Line {
  level: LogLevel,
  target: String,
  message: String,
}

#[derive(Default)]
struct Sink {
  inbox: Option<Arc<LogInbox>>,
  waiting: Vec<Line>,
}

fn sink() -> &'static Mutex<Sink> {
  static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();
  SINK.get_or_init(Mutex::default)
}

thread_local! {
  static PUSHING: Cell<bool> = const { Cell::new(false) };
}

pub fn without_capture<T>(work: impl FnOnce() -> T) -> T {
  PUSHING.with(|flag| flag.set(true));
  let done = work();
  PUSHING.with(|flag| flag.set(false));
  done
}

pub fn attach(inbox: Arc<LogInbox>) {
  let waiting = {
    let mut held = sink().lock().unwrap();
    held.inbox = Some(Arc::clone(&inbox));
    std::mem::take(&mut held.waiting)
  };
  for line in waiting {
    inbox.push(line.level, line.target, line.message);
  }
}

struct RingLayer;

impl<S: tracing::Subscriber> Layer<S> for RingLayer {
  fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
    let target = event.metadata().target();
    if target == DEVICE_TARGET || PUSHING.with(Cell::get) {
      return;
    }

    let mut rendered = Rendered::default();
    event.record(&mut rendered);
    let line = Line {
      level: match *event.metadata().level() {
        tracing::Level::ERROR => LogLevel::Error,
        tracing::Level::WARN => LogLevel::Warn,
        tracing::Level::INFO => LogLevel::Info,
        tracing::Level::DEBUG => LogLevel::Debug,
        tracing::Level::TRACE => LogLevel::Trace,
      },
      target: target.to_owned(),
      message: rendered.done(),
    };

    let inbox = {
      let mut held = sink().lock().unwrap();
      match held.inbox.clone() {
        Some(inbox) => inbox,
        None => {
          if held.waiting.len() < PENDING {
            held.waiting.push(line);
          }
          return;
        }
      }
    };

    without_capture(|| inbox.push(line.level, line.target, line.message));
  }
}

#[derive(Default)]
struct Rendered {
  message: String,
  fields: String,
}

impl Rendered {
  fn done(self) -> String {
    if self.message.is_empty() {
      self.fields.trim_start().to_owned()
    } else {
      format!("{}{}", self.message, self.fields)
    }
  }
}

impl tracing::field::Visit for Rendered {
  fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
    if field.name() == "message" {
      self.message.push_str(value);
    } else {
      let _ = write!(self.fields, " {}={}", field.name(), value);
    }
  }

  fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
    if field.name() == "message" {
      let _ = write!(self.message, "{value:?}");
    } else {
      let _ = write!(self.fields, " {}={:?}", field.name(), value);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn verbose_logging_survives_a_relaunch() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let swapped = Arc::new(Mutex::new(Vec::new()));

    let first = Verbosity::new(kept(Some(dir.path())), {
      let swapped = Arc::clone(&swapped);
      move |verbose| swapped.lock().unwrap().push(verbose)
    });
    assert!(!first.get(), "a fresh host logs quietly");
    first.set(true);

    assert_eq!(
      *swapped.lock().unwrap(),
      vec![true],
      "the live filter follows the toggle"
    );
    assert!(
      kept(Some(dir.path())).read(|verbose| *verbose),
      "the next launch starts verbose"
    );
  }
}
