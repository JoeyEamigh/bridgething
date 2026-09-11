use std::sync::{Arc, Mutex};

use bridgething_companion::backend::{StreamBackend, StreamPresentation, StreamSink, StreamSource};

pub const APP_BUNDLE: &str = "com.bridgething.test";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamCall {
  Play(String),
  Present(StreamPresentation),
  Pause,
  Resume,
  SeekTo(u32),
  Stop,
}

#[derive(Default)]
pub struct FakeStreamBackend {
  pub calls: Mutex<Vec<StreamCall>>,
  pub sources: Mutex<Vec<StreamSource>>,
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

  pub fn last_source(&self) -> Option<StreamSource> {
    self.sources.lock().unwrap().last().cloned()
  }

  pub fn transport_calls(&self) -> Vec<StreamCall> {
    self
      .calls()
      .into_iter()
      .filter(|call| !matches!(call, StreamCall::Present(_)))
      .collect()
  }

  pub fn presentations(&self) -> Vec<StreamPresentation> {
    self
      .calls()
      .into_iter()
      .filter_map(|call| match call {
        StreamCall::Present(presentation) => Some(presentation),
        _ => None,
      })
      .collect()
  }
}

impl StreamBackend for FakeStreamBackend {
  fn app_bundle(&self) -> String {
    APP_BUNDLE.into()
  }

  fn play(&self, source: StreamSource, sink: Arc<StreamSink>) {
    self.calls.lock().unwrap().push(StreamCall::Play(source.url.clone()));
    self.sources.lock().unwrap().push(source);
    self.sinks.lock().unwrap().push(sink);
  }

  fn present(&self, presentation: StreamPresentation) {
    self.calls.lock().unwrap().push(StreamCall::Present(presentation));
  }

  fn pause(&self) {
    self.calls.lock().unwrap().push(StreamCall::Pause);
  }

  fn resume(&self) {
    self.calls.lock().unwrap().push(StreamCall::Resume);
  }

  fn seek_to(&self, position_ms: u32) {
    self.calls.lock().unwrap().push(StreamCall::SeekTo(position_ms));
  }

  fn stop(&self) {
    self.calls.lock().unwrap().push(StreamCall::Stop);
  }
}
