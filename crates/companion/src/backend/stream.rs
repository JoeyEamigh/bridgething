use std::sync::Arc;

use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
  Started,
  Stopped { error: Option<String> },
}

/// Native playback of a raw http(s) media URL on the phone. Webapps run in the
/// on-device kiosk and the Car Thing has no speaker, so a webapp that wants to
/// play a stream (internet radio, a podcast episode, ambient audio) hands the
/// URL to this backend and the phone's native player takes it from there.
#[uniffi::export(with_foreign)]
pub trait StreamBackend: Send + Sync {
  fn play(&self, url: String, sink: Arc<StreamSink>);
  fn pause(&self);
  fn resume(&self);
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
  pub fn on_started(&self) {
    let _ = self.tx.send(StreamEvent::Started);
  }

  pub fn on_stopped(&self, error: Option<String>) {
    let _ = self.tx.send(StreamEvent::Stopped { error });
  }
}
