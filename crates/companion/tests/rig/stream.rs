use std::sync::{Arc, Mutex};

use bridgething_companion::backend::{StreamBackend, StreamSink};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamCall {
  Play(String),
  Pause,
  Resume,
  Stop,
}

#[derive(Default)]
pub struct FakeStreamBackend {
  pub calls: Mutex<Vec<StreamCall>>,
  pub sinks: Mutex<Vec<Arc<StreamSink>>>,
}

impl FakeStreamBackend {
  pub fn new() -> Arc<Self> {
    Arc::new(Self::default())
  }

  pub fn calls(&self) -> Vec<StreamCall> {
    self.calls.lock().unwrap().clone()
  }

  pub fn last_sink(&self) -> Option<Arc<StreamSink>> {
    self.sinks.lock().unwrap().last().cloned()
  }
}

impl StreamBackend for FakeStreamBackend {
  fn play(&self, url: String, sink: Arc<StreamSink>) {
    self.calls.lock().unwrap().push(StreamCall::Play(url));
    self.sinks.lock().unwrap().push(sink);
  }

  fn pause(&self) {
    self.calls.lock().unwrap().push(StreamCall::Pause);
  }

  fn resume(&self) {
    self.calls.lock().unwrap().push(StreamCall::Resume);
  }

  fn stop(&self) {
    self.calls.lock().unwrap().push(StreamCall::Stop);
  }
}
