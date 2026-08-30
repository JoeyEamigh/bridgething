pub mod asset;
pub mod audio;
pub mod extension;
pub mod geo;
pub mod library;
pub mod lyrics;
pub mod notifications;
pub mod phone;
pub mod player;
pub mod system;
pub mod webapp;

use std::{
  future::Future,
  sync::{Arc, Mutex},
};

use bridgething_gateway::{HandlerError, Reply};
use futures::future::BoxFuture;
use libbridgething::{
  BridgeThingMeta, OtaError, OtaFinished, OtaProgress, WebappInfo,
  gateway::{OtaAssetRange, OtaAssetRangeAbandon, OtaAssetRangeRejected, OtaAssetRangeReply, TransferAck},
};
use tokio::{sync::mpsc, task::JoinHandle};
use uuid::Uuid;

pub(crate) struct Serial {
  tx: mpsc::UnboundedSender<BoxFuture<'static, ()>>,
  task: JoinHandle<()>,
}

impl Serial {
  pub(crate) fn spawn() -> Self {
    let (tx, mut rx) = mpsc::unbounded_channel::<BoxFuture<'static, ()>>();
    let task = tokio::spawn(async move {
      while let Some(job) = rx.recv().await {
        job.await;
      }
    });
    Self { tx, task }
  }

  pub(crate) fn push(&self, job: impl Future<Output = ()> + Send + 'static) {
    let _ = self.tx.send(Box::pin(job));
  }
}

impl Drop for Serial {
  fn drop(&mut self) {
    self.task.abort();
  }
}

#[derive(Default)]
pub(crate) struct Relay(Mutex<Option<JoinHandle<()>>>);

impl Relay {
  pub(crate) fn hold(&self, task: JoinHandle<()>) {
    if let Some(previous) = self.0.lock().unwrap().replace(task) {
      previous.abort();
    }
  }

  pub(crate) fn release(&self) {
    if let Some(task) = self.0.lock().unwrap().take() {
      task.abort();
    }
  }
}

impl Drop for Relay {
  fn drop(&mut self) {
    self.release();
  }
}

pub(crate) async fn ask<B, T>(backend: &Arc<B>, call: impl FnOnce(&B) -> T + Send + 'static) -> Option<T>
where
  B: ?Sized + Send + Sync + 'static,
  T: Send + 'static,
{
  let held = backend.clone();
  match tokio::task::spawn_blocking(move || call(&held)).await {
    Ok(said) => Some(said),
    Err(failure) => {
      tracing::warn!(%failure, "a platform backend call did not return");
      None
    }
  }
}

pub(crate) async fn tell<B>(backend: &Arc<B>, call: impl FnOnce(&B) + Send + 'static)
where
  B: ?Sized + Send + Sync + 'static,
{
  ask(backend, call).await;
}

#[async_trait::async_trait]
pub trait OtaInbound: Send + Sync {
  async fn asset_range(
    &self,
    id: Uuid,
    request: OtaAssetRange,
  ) -> Result<Reply<OtaAssetRangeReply>, HandlerError<OtaAssetRangeRejected>>;
  fn asset_range_abandon(&self, payload: OtaAssetRangeAbandon);
  fn progress(&self, payload: OtaProgress);
  fn error(&self, payload: OtaError);
  fn finished(&self, payload: OtaFinished);
  fn nickname_changed(&self, nickname: Option<String>) -> Option<BridgeThingMeta>;
  fn device_meta(&self, meta: BridgeThingMeta);
  fn transfer_ack(&self, ack: TransferAck);
  fn webapp_installed(&self, info: WebappInfo);
}
