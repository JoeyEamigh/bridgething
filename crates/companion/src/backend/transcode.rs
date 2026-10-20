use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TranscodeRequest {
  pub source: String,
  pub destination: String,
  pub cover_width: u32,
  pub cover_height: u32,
  pub max_bitrate: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct TranscodedVideo {
  pub width: u32,
  pub height: u32,
  pub duration_ms: u64,
}

#[uniffi::export(with_foreign)]
pub trait TranscodeJob: Send + Sync {
  fn cancel(&self);
}

#[uniffi::export(with_foreign)]
pub trait MediaTranscoder: Send + Sync {
  fn transcode(&self, request: TranscodeRequest, sink: Arc<TranscodeSink>) -> Arc<dyn TranscodeJob>;
  fn poster(&self, source: String) -> Option<Vec<u8>>;
}

#[derive(uniffi::Object)]
pub struct TranscodeSink {
  tx: Mutex<Option<oneshot::Sender<Result<TranscodedVideo, String>>>>,
}

impl TranscodeSink {
  pub fn channel() -> (Arc<Self>, oneshot::Receiver<Result<TranscodedVideo, String>>) {
    let (tx, rx) = oneshot::channel();
    (
      Arc::new(Self {
        tx: Mutex::new(Some(tx)),
      }),
      rx,
    )
  }

  fn settle(&self, result: Result<TranscodedVideo, String>) {
    if let Some(tx) = self.tx.lock().unwrap().take() {
      let _ = tx.send(result);
    }
  }
}

#[uniffi::export]
impl TranscodeSink {
  pub fn complete(&self, video: TranscodedVideo) {
    self.settle(Ok(video));
  }

  pub fn fail(&self, reason: String) {
    self.settle(Err(reason));
  }
}
