mod actor;
pub mod art;
pub mod builtin;
pub mod storage;
pub mod wait;

use std::{path::PathBuf, sync::Arc, time::Duration};

pub use actor::{AssetCacheEvent, Retention};
use libbridgething::AssetRetention;
use sea_orm::{DatabaseConnection, DbErr};
use tokio::{
  sync::{broadcast, mpsc, oneshot},
  task::JoinHandle,
};
use tokio_util::bytes::Bytes;

pub const MEMORY_BUDGET_BYTES: usize = 8 * 1024 * 1024;
pub const DISK_BUDGET_BYTES: usize = 512 * 1024 * 1024;
pub const DISK_FREE_HEADROOM_BYTES: usize = 128 * 1024 * 1024;
const TTL_SWEEP_INTERVAL: Duration = Duration::from_secs(15);
const EVENT_BROADCAST_CAPACITY: usize = 64;
const COMMAND_MAILBOX_CAPACITY: usize = 16;

#[derive(Debug, Clone)]
pub struct CachedAsset {
  pub bytes: Bytes,
  pub mime: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AssetCache {
  inner: Arc<AssetCacheInner>,
}

#[derive(Debug)]
struct AssetCacheInner {
  cmd_tx: mpsc::Sender<actor::Command>,
  events_tx: broadcast::Sender<AssetCacheEvent>,
}

impl AssetCache {
  pub async fn init(db: DatabaseConnection, blobs_dir: PathBuf) -> Result<AssetCachePending, AssetError> {
    let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_MAILBOX_CAPACITY);
    let (events_tx, _) = broadcast::channel(EVENT_BROADCAST_CAPACITY);

    let actor = actor::AssetActor::new(db, blobs_dir, cmd_rx, events_tx.clone());
    let actor = actor.bootstrap().await?;

    Ok(AssetCachePending {
      actor,
      handle: Self {
        inner: Arc::new(AssetCacheInner { cmd_tx, events_tx }),
      },
    })
  }

  pub async fn insert(
    &self,
    id: String,
    bytes: Bytes,
    mime: Option<String>,
    retention: AssetRetention,
  ) -> Result<(), AssetError> {
    self
      .insert_internal(id, bytes, mime, Retention::from_wire(retention))
      .await
  }

  pub async fn insert_internal(
    &self,
    id: String,
    bytes: Bytes,
    mime: Option<String>,
    retention: Retention,
  ) -> Result<(), AssetError> {
    let (ack, rx) = oneshot::channel();
    self
      .inner
      .cmd_tx
      .send(actor::Command::Insert {
        id,
        bytes,
        mime,
        retention,
        ack,
      })
      .await
      .map_err(|_| AssetError::CacheClosed)?;
    rx.await.map_err(|_| AssetError::CacheClosed)?
  }

  pub async fn insert_from_path(
    &self,
    id: String,
    source: PathBuf,
    mime: Option<String>,
    retention: AssetRetention,
  ) -> Result<(), AssetError> {
    let (ack, rx) = oneshot::channel();
    self
      .inner
      .cmd_tx
      .send(actor::Command::InsertFromPath {
        id,
        source,
        mime,
        retention: Retention::from_wire(retention),
        ack,
      })
      .await
      .map_err(|_| AssetError::CacheClosed)?;
    rx.await.map_err(|_| AssetError::CacheClosed)?
  }

  pub async fn set_retention(&self, id: &str, retention: Retention) -> Result<(), AssetError> {
    let (ack, rx) = oneshot::channel();
    self
      .inner
      .cmd_tx
      .send(actor::Command::SetRetention {
        id: id.to_string(),
        retention,
        ack,
      })
      .await
      .map_err(|_| AssetError::CacheClosed)?;
    rx.await.map_err(|_| AssetError::CacheClosed)?
  }

  pub async fn contains(&self, id: &str) -> Result<bool, AssetError> {
    let (reply, rx) = oneshot::channel();
    self
      .inner
      .cmd_tx
      .send(actor::Command::Contains {
        id: id.to_string(),
        reply,
      })
      .await
      .map_err(|_| AssetError::CacheClosed)?;
    rx.await.map_err(|_| AssetError::CacheClosed)
  }

  pub async fn get(&self, id: &str) -> Result<Option<CachedAsset>, AssetError> {
    let (reply, rx) = oneshot::channel();
    self
      .inner
      .cmd_tx
      .send(actor::Command::Get {
        id: id.to_string(),
        reply,
      })
      .await
      .map_err(|_| AssetError::CacheClosed)?;
    rx.await.map_err(|_| AssetError::CacheClosed)
  }

  pub async fn clear_all(&self) -> Result<(), AssetError> {
    let (ack, rx) = oneshot::channel();
    self
      .inner
      .cmd_tx
      .send(actor::Command::ClearAll { ack })
      .await
      .map_err(|_| AssetError::CacheClosed)?;
    rx.await.map_err(|_| AssetError::CacheClosed)?
  }

  pub async fn clear(&self, id: &str) -> Result<(), AssetError> {
    let (ack, rx) = oneshot::channel();
    self
      .inner
      .cmd_tx
      .send(actor::Command::Clear {
        id: id.to_string(),
        ack,
      })
      .await
      .map_err(|_| AssetError::CacheClosed)?;
    rx.await.map_err(|_| AssetError::CacheClosed)?
  }

  pub fn subscribe(&self) -> broadcast::Receiver<AssetCacheEvent> {
    self.inner.events_tx.subscribe()
  }

  pub async fn reserve_disk(&self, need_bytes: u64) -> Result<(), AssetError> {
    let (ack, rx) = oneshot::channel();
    self
      .inner
      .cmd_tx
      .send(actor::Command::ReserveDisk { need_bytes, ack })
      .await
      .map_err(|_| AssetError::CacheClosed)?;
    rx.await.map_err(|_| AssetError::CacheClosed)
  }
}

pub struct AssetCachePending {
  actor: actor::AssetActor,
  handle: AssetCache,
}

impl AssetCachePending {
  pub fn spawn(self) -> (AssetCache, JoinHandle<()>) {
    let join = tokio::spawn(self.actor.run());
    (self.handle, join)
  }
}

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
  #[error("asset cache database error: {0}")]
  Db(#[from] DbErr),
  #[error("asset cache actor channel closed")]
  CacheClosed,
  #[error("asset cache io error: {0}")]
  Io(#[from] std::io::Error),
  #[error("disk pin rejected: would exceed the disk budget after evicting all Ttl entries")]
  DiskBudgetExceeded,
}
