use std::{
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_delivery::transfer::AckWindow;
use tracing_subscriber::layer::SubscriberExt;
use uuid::Uuid;

const IMMEDIATE: Duration = Duration::from_millis(200);

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<String>>>);

impl Captured {
  fn matching(&self, needle: &str) -> Vec<String> {
    self
      .0
      .lock()
      .unwrap()
      .iter()
      .filter(|line| line.contains(needle))
      .cloned()
      .collect()
  }
}

struct CaptureLayer(Captured);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CaptureLayer {
  fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
    let mut line = String::new();
    event.record(&mut Visit(&mut line));
    self.0.0.lock().unwrap().push(line);
  }
}

struct Visit<'a>(&'a mut String);

impl tracing::field::Visit for Visit<'_> {
  fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
    use std::fmt::Write as _;
    let _ = write!(self.0, " {}={:?}", field.name(), value);
  }
}

#[tokio::test]
async fn a_peer_that_stops_acking_announces_itself_with_the_gap() {
  let captured = Captured::default();
  tracing::subscriber::set_global_default(tracing_subscriber::registry().with(CaptureLayer(captured.clone())))
    .expect("this binary owns the subscriber");

  let window = AckWindow::new();
  let id = Uuid::now_v7();
  window.note(id, 1);

  assert!(
    window.await_window(id, 16, 64, IMMEDIATE).await.is_ok(),
    "inside the window"
  );
  assert!(
    captured.matching("stopped acking").is_empty(),
    "a healthy window must stay silent, or a field log is unreadable"
  );

  assert!(window.await_window(id, 4096, 64, IMMEDIATE).await.is_err());
  let seen = captured.matching("stopped acking").join("\n");
  assert!(seen.contains("abandoned"), "got {seen:?}");
  assert!(seen.contains("blocked_ms"), "the gap has to be in the line: {seen:?}");
  assert!(
    seen.contains("acked"),
    "the offset reached has to be in the line: {seen:?}"
  );
}
