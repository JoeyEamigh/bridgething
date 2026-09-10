use std::sync::Arc;

use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum StreamStatus {
  Buffering,
  Playing,
  Paused,
  Ended,
  Failed { reason: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct StreamMetadata {
  pub title: Option<String>,
  pub artist: Option<String>,
  pub album: Option<String>,
  pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct StreamTiming {
  pub position_ms: u32,
  pub duration_ms: Option<u32>,
  pub seekable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StreamSource {
  pub url: String,
  pub live: bool,
  pub station: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
  Status(StreamStatus),
  Metadata(StreamMetadata),
  Timing(StreamTiming),
}

#[uniffi::export(with_foreign)]
pub trait StreamBackend: Send + Sync {
  fn app_bundle(&self) -> String;
  fn play(&self, source: StreamSource, sink: Arc<StreamSink>);
  fn pause(&self);
  fn resume(&self);
  fn seek_to(&self, position_ms: u32);
  fn stop(&self);
}

#[derive(uniffi::Object)]
pub struct StreamSink {
  tx: mpsc::UnboundedSender<StreamEvent>,
}

impl StreamSink {
  pub fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<StreamEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(Self { tx }), rx)
  }
}

#[uniffi::export]
impl StreamSink {
  pub fn on_status(&self, status: StreamStatus) {
    let _ = self.tx.send(StreamEvent::Status(status));
  }

  pub fn on_metadata(&self, metadata: StreamMetadata) {
    let _ = self.tx.send(StreamEvent::Metadata(metadata));
  }

  pub fn on_timing(&self, timing: StreamTiming) {
    let _ = self.tx.send(StreamEvent::Timing(timing));
  }
}
