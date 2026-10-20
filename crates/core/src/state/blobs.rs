use std::{
  collections::{HashMap, HashSet, hash_map::Entry},
  path::{Path, PathBuf},
};

use libbridgething::WebappError;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use tokio::{
  sync::{mpsc, oneshot},
  task::JoinHandle,
};
use tokio_util::{
  bytes::Bytes,
  sync::{CancellationToken, DropGuard},
};
use uuid::Uuid;

use super::{KvStore, StateError, StateResult, WebappRegistry};
use crate::{
  asset::{AssetCache, DISK_FREE_HEADROOM_BYTES},
  bluetooth::Address,
  digest::sha256_file_as_read,
  paths,
  transfer::{BeginOutcome, ChunkOutcome, ChunkedTransfer, TransferError},
};

pub const BLOB_MAX_BYTES: u32 = 32 * 1024 * 1024;
pub const WEBAPP_BLOB_QUOTA_BYTES: u64 = 64 * 1024 * 1024;
pub const BLOB_KEY_MAX_BYTES: usize = 256;
pub const BLOB_URL_PREFIX: &str = "/.bridgething/blob/";
const BLOB_MIME_MAX_BYTES: usize = 255;
const BLOB_TRANSFER_BUDGET_BYTES: u64 = 4 * BLOB_MAX_BYTES as u64;
const COMMAND_MAILBOX_CAPACITY: usize = 16;
const SHA256_HEX_LEN: usize = 64;
const KEY_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

type Slot = (Uuid, String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobMeta {
  pub sha256: String,
  pub mime: String,
  pub size: u32,
}

#[derive(Debug)]
pub enum PutBegin {
  Stored { size: u32 },
  Stream { offset: u64, upload: BlobUpload },
}

#[derive(Debug)]
pub enum BlobEvent {
  Stored {
    webapp: Uuid,
    key: String,
    meta: BlobMeta,
    changed: bool,
  },
  Failed {
    webapp: Uuid,
    key: String,
    sha256: String,
    error: WebappError,
  },
  Replaced {
    webapp: Uuid,
    key: String,
    sha256: String,
    uploader: Address,
  },
  Removed {
    webapp: Uuid,
    key: String,
  },
}

#[derive(Debug)]
pub struct BlobUpload {
  pub webapp: Uuid,
  pub key: String,
  pub meta: BlobMeta,
  pub uploader: Address,
  chunk_id: String,
  attempt: u64,
  cancel: CancellationToken,
  _exited: DropGuard,
}

impl BlobUpload {
  pub async fn cancelled(&self) {
    self.cancel.cancelled().await
  }
}

#[derive(Debug)]
struct InFlight {
  attempt: u64,
  chunk_id: String,
  meta: BlobMeta,
  uploader: Address,
  cancel: CancellationToken,
  exited: CancellationToken,
}

impl InFlight {
  async fn stop(&self) {
    self.cancel.cancel();
    self.exited.cancelled().await;
  }
}

#[derive(Debug, Clone, Copy)]
enum Takeover {
  Resumes,
  Streams,
  Stored,
}

type Verdict = std::io::Result<(String, std::fs::Metadata)>;

#[derive(Debug)]
struct BeginPut {
  webapp: Uuid,
  key: String,
  meta: BlobMeta,
  uploader: Option<Address>,
  verdict: Option<Verdict>,
  reply: oneshot::Sender<Result<PutBegin, WebappError>>,
}

#[derive(Debug)]
enum Command {
  Begin(BeginPut),
  Finish {
    webapp: Uuid,
    key: String,
    attempt: u64,
    outcome: Result<PathBuf, TransferError>,
    reply: oneshot::Sender<()>,
  },
  Delete {
    webapp: Uuid,
    key: String,
    reply: oneshot::Sender<StateResult<()>>,
  },
  Purge {
    webapp: Uuid,
    reply: oneshot::Sender<StateResult<()>>,
  },
}

#[derive(Debug, Clone)]
struct BlobIndex {
  root: PathBuf,
  kv: KvStore,
}

impl BlobIndex {
  async fn list(&self, webapp: Uuid) -> StateResult<Vec<(String, BlobMeta)>> {
    let mut entries: Vec<(String, BlobMeta)> = self
      .kv
      .blob_list(webapp)
      .await?
      .into_iter()
      .filter_map(|(key, raw)| match serde_json::from_str::<BlobMeta>(&raw) {
        Ok(meta) => Some((key, meta)),
        Err(err) => {
          tracing::warn!(%webapp, %key, ?err, "unreadable blob record; skipping");
          None
        }
      })
      .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
  }

  async fn record(&self, webapp: Uuid, key: &str) -> StateResult<Option<BlobMeta>> {
    Ok(
      self
        .kv
        .blob_get(webapp, key)
        .await?
        .and_then(|raw| serde_json::from_str::<BlobMeta>(&raw).ok()),
    )
  }

  fn webapp_dir(&self, webapp: Uuid) -> PathBuf {
    self.root.join(webapp.simple().to_string())
  }

  fn blob_path(&self, webapp: Uuid, sha256: &str) -> PathBuf {
    self.webapp_dir(webapp).join(sha256)
  }

  async fn reconcile(&self) -> StateResult<Vec<BlobEvent>> {
    let mut records: HashMap<Uuid, Vec<(String, Option<BlobMeta>)>> = HashMap::new();
    for (webapp, key, raw) in self.kv.blob_rows().await? {
      records
        .entry(webapp)
        .or_default()
        .push((key, serde_json::from_str(&raw).ok()));
    }
    let mut dirs = tokio::fs::read_dir(&self.root).await?;
    while let Some(entry) = dirs.next_entry().await? {
      if let Some(webapp) = entry.file_name().to_str().and_then(webapp_of_dir)
        && entry.file_type().await?.is_dir()
      {
        records.entry(webapp).or_default();
      }
    }

    let mut dropped = Vec::new();
    for (webapp, rows) in records {
      if let Err(err) = self.reconcile_webapp(webapp, rows, &mut dropped).await {
        tracing::warn!(%webapp, ?err, "could not reconcile blob content with its records");
      }
    }
    Ok(dropped)
  }

  async fn reconcile_webapp(
    &self,
    webapp: Uuid,
    rows: Vec<(String, Option<BlobMeta>)>,
    dropped: &mut Vec<BlobEvent>,
  ) -> StateResult<()> {
    let dir = self.webapp_dir(webapp);
    let mut on_disk: HashMap<String, u64> = HashMap::new();
    match tokio::fs::read_dir(&dir).await {
      Ok(mut entries) => {
        while let Some(entry) = entries.next_entry().await? {
          let found = entry.metadata().await?;
          if let Some(name) = entry.file_name().to_str()
            && found.is_file()
            && is_store_name(name)
          {
            on_disk.insert(name.to_string(), found.len());
          }
        }
      }
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
      Err(err) => return Err(err.into()),
    }

    let mut referenced: HashSet<&str> = HashSet::new();
    for (key, meta) in &rows {
      match meta {
        Some(meta) if on_disk.get(&meta.sha256) == Some(&u64::from(meta.size)) => {
          referenced.insert(meta.sha256.as_str());
        }
        _ => {
          tracing::warn!(%webapp, %key, "dropping a blob record whose content is missing or damaged");
          self.kv.blob_delete(webapp, key).await?;
          dropped.push(BlobEvent::Removed {
            webapp,
            key: key.clone(),
          });
        }
      }
    }
    for name in on_disk.keys().filter(|name| !referenced.contains(name.as_str())) {
      tracing::info!(%webapp, %name, "collecting blob content no record references");
      if let Err(err) = tokio::fs::remove_file(dir.join(name)).await {
        tracing::warn!(%webapp, %name, ?err, "failed to collect unreferenced blob content");
      }
    }
    if referenced.is_empty() {
      let _ = tokio::fs::remove_dir(&dir).await;
    }
    Ok(())
  }
}

#[derive(Debug)]
pub struct WebappBlobs {
  index: BlobIndex,
  transfers: ChunkedTransfer,
  commands: mpsc::Sender<Command>,
  _transfer_handle: JoinHandle<()>,
  _actor_handle: JoinHandle<()>,
}

impl WebappBlobs {
  pub async fn init(
    root: PathBuf,
    transfers_dir: PathBuf,
    kv: KvStore,
    assets: AssetCache,
    webapps: WebappRegistry,
    events: mpsc::Sender<BlobEvent>,
  ) -> StateResult<Self> {
    tokio::fs::create_dir_all(&root).await?;
    let index = BlobIndex { root, kv };
    let dropped = index.reconcile().await?;
    let (transfers, transfer_handle) = ChunkedTransfer::init(transfers_dir, BLOB_TRANSFER_BUDGET_BYTES, Vec::new())
      .await?
      .spawn();
    let (commands, mailbox) = mpsc::channel(COMMAND_MAILBOX_CAPACITY);
    let actor = WebappBlobsActor {
      index: index.clone(),
      transfers: transfers.clone(),
      assets,
      webapps,
      events,
      requeue: commands.downgrade(),
      inflight: HashMap::new(),
      next_attempt: 0,
    };
    Ok(Self {
      index,
      transfers,
      commands,
      _transfer_handle: transfer_handle,
      _actor_handle: tokio::spawn(actor.run(mailbox, dropped)),
    })
  }

  pub async fn list(&self, webapp: Uuid) -> StateResult<Vec<(String, BlobMeta)>> {
    self.index.list(webapp).await
  }

  pub async fn get(&self, webapp: Uuid, key: &str) -> StateResult<Option<(PathBuf, BlobMeta)>> {
    Ok(
      self
        .index
        .record(webapp, key)
        .await?
        .map(|meta| (self.index.blob_path(webapp, &meta.sha256), meta)),
    )
  }

  pub async fn begin_put(
    &self,
    webapp: Uuid,
    key: String,
    mime: String,
    size: u32,
    sha256: Option<String>,
    uploader: Option<Address>,
  ) -> Result<PutBegin, WebappError> {
    validate_key(&key)?;
    let Some(sha256) = sha256 else {
      return Err(WebappError::InvalidBlobPut {
        key,
        reason: "the transfer must carry a sha256".into(),
      });
    };
    let sha256 = normalize_sha256(&key, &sha256)?;
    validate_mime(&key, &mime)?;
    if size == 0 {
      return Err(WebappError::InvalidBlobPut {
        key,
        reason: "a blob cannot be empty".into(),
      });
    }
    if size > BLOB_MAX_BYTES {
      return Err(WebappError::BlobTooLarge {
        key,
        max_bytes: BLOB_MAX_BYTES,
      });
    }
    let meta = BlobMeta { sha256, mime, size };
    self
      .request(|reply| {
        Command::Begin(BeginPut {
          webapp,
          key,
          meta,
          uploader,
          verdict: None,
          reply,
        })
      })
      .await
      .unwrap_or_else(|| Err(internal(StateError::BlobStoreStopped)))
  }

  pub async fn accept(&self, upload: &BlobUpload, offset: u32, bytes: Bytes) -> Result<ChunkOutcome, TransferError> {
    self
      .transfers
      .accept_chunk(upload.chunk_id.clone(), offset as u64, bytes)
      .await
  }

  pub async fn finish(&self, upload: BlobUpload, outcome: Result<PathBuf, TransferError>) {
    let settled = self.request(|reply| Command::Finish {
      webapp: upload.webapp,
      key: upload.key.clone(),
      attempt: upload.attempt,
      outcome,
      reply,
    });
    tokio::select! {
      biased;
      _ = upload.cancel.cancelled() => {}
      _ = settled => {}
    }
  }

  pub async fn delete(&self, webapp: Uuid, key: String) -> StateResult<()> {
    self
      .request(|reply| Command::Delete { webapp, key, reply })
      .await
      .unwrap_or(Err(StateError::BlobStoreStopped))
  }

  pub async fn purge(&self, webapp: Uuid) -> StateResult<()> {
    self
      .request(|reply| Command::Purge { webapp, reply })
      .await
      .unwrap_or(Err(StateError::BlobStoreStopped))
  }

  async fn request<T>(&self, command: impl FnOnce(oneshot::Sender<T>) -> Command) -> Option<T> {
    let (reply, answer) = oneshot::channel();
    self.commands.send(command(reply)).await.ok()?;
    answer.await.ok()
  }
}

struct WebappBlobsActor {
  index: BlobIndex,
  transfers: ChunkedTransfer,
  assets: AssetCache,
  webapps: WebappRegistry,
  events: mpsc::Sender<BlobEvent>,
  requeue: mpsc::WeakSender<Command>,
  inflight: HashMap<Slot, InFlight>,
  next_attempt: u64,
}

impl WebappBlobsActor {
  async fn run(mut self, mut mailbox: mpsc::Receiver<Command>, dropped: Vec<BlobEvent>) {
    for event in dropped {
      self.emit(event).await;
    }
    tracing::debug!("webapp blob store running");
    while let Some(command) = mailbox.recv().await {
      self.inflight.retain(|_, upload| !upload.exited.is_cancelled());
      match command {
        Command::Begin(put) => self.begin(put).await,
        Command::Finish {
          webapp,
          key,
          attempt,
          outcome,
          reply,
        } => {
          self.finish(webapp, key, attempt, outcome).await;
          let _ = reply.send(());
        }
        Command::Delete { webapp, key, reply } => {
          let _ = reply.send(self.delete(webapp, key).await);
        }
        Command::Purge { webapp, reply } => {
          let _ = reply.send(self.purge(webapp).await);
        }
      }
    }
    tracing::debug!("webapp blob store: command channel closed, exiting");
  }

  async fn begin(&mut self, mut put: BeginPut) {
    let verdict = put.verdict.take();
    let answer = match self.examine(&put, verdict).await {
      Ok(None) => return self.hash_stored(put),
      Ok(Some(stored)) => self.admit(put.webapp, put.key, put.meta, put.uploader, stored).await,
      Err(err) => Err(err),
    };
    let _ = put.reply.send(answer);
  }

  async fn examine(&mut self, put: &BeginPut, verdict: Option<Verdict>) -> Result<Option<bool>, WebappError> {
    if self.webapps.bundle(put.webapp).await.is_none() {
      return Err(WebappError::WebappNotFound {
        id: put.webapp.to_string(),
      });
    }
    self.verify_stored(put.webapp, &put.key, &put.meta, verdict).await
  }

  fn hash_stored(&self, put: BeginPut) {
    let path = self.index.blob_path(put.webapp, &put.meta.sha256);
    let requeue = self.requeue.clone();
    tokio::spawn(async move {
      let verdict = Some(sha256_file_as_read(&path).await);
      let Some(commands) = requeue.upgrade() else {
        return;
      };
      let _ = commands.send(Command::Begin(BeginPut { verdict, ..put })).await;
    });
  }

  async fn admit(
    &mut self,
    webapp: Uuid,
    key: String,
    meta: BlobMeta,
    uploader: Option<Address>,
    stored: bool,
  ) -> Result<PutBegin, WebappError> {
    self.check_quota(webapp, &key, &meta).await?;
    let slot = (webapp, key);
    if stored {
      let changed = self.record(webapp, &slot.1, &meta).await?;
      self.replace_inflight(&slot, uploader, &meta, Takeover::Stored).await;
      return Ok(self.stored(slot, meta, changed).await);
    }

    let Some(uploader) = uploader else {
      return Err(WebappError::BlobTransferFailed {
        key: slot.1,
        reason: "a streamed upload needs an addressed peer".into(),
      });
    };
    if let Err(err) = self.assets.reserve_disk(meta.size as u64).await {
      tracing::warn!(?err, %webapp, key = %slot.1, "blob put: asset cache reserve_disk failed; checking free space anyway");
    }
    let needed = (meta.size as u64).saturating_add(DISK_FREE_HEADROOM_BYTES as u64);
    if paths::partition_free_bytes(&self.index.root) < needed {
      return Err(WebappError::BlobStorageFull {
        key: slot.1,
        needed_bytes: meta.size,
      });
    }

    let chunk_id = chunk_id(webapp, &slot.1, &meta);
    let resumes = self.inflight.get(&slot).is_some_and(|prior| prior.chunk_id == chunk_id);
    if resumes {
      self
        .replace_inflight(&slot, Some(uploader), &meta, Takeover::Resumes)
        .await;
    }
    let begun = self
      .transfers
      .begin(chunk_id.clone(), meta.size as u64, Some(meta.sha256.clone()))
      .await
      .map_err(|err| WebappError::BlobTransferFailed {
        key: slot.1.clone(),
        reason: err.to_string(),
      })?;
    match begun {
      BeginOutcome::AlreadyComplete { path } => {
        let changed = self.land(webapp, &slot.1, &meta, &path, &chunk_id).await?;
        self
          .replace_inflight(&slot, Some(uploader), &meta, Takeover::Stored)
          .await;
        Ok(self.stored(slot, meta, changed).await)
      }
      BeginOutcome::Resume { offset } => {
        self
          .replace_inflight(&slot, Some(uploader), &meta, Takeover::Streams)
          .await;
        let upload = self.register(slot, meta, chunk_id, uploader);
        Ok(PutBegin::Stream { offset, upload })
      }
    }
  }

  async fn finish(&mut self, webapp: Uuid, key: String, attempt: u64, outcome: Result<PathBuf, TransferError>) {
    let Entry::Occupied(registered) = self.inflight.entry((webapp, key)) else {
      return;
    };
    if registered.get().attempt != attempt {
      return;
    }
    let ((webapp, key), upload) = registered.remove_entry();
    let landed = match outcome {
      Ok(path) => self.land(webapp, &key, &upload.meta, &path, &upload.chunk_id).await,
      Err(err) => {
        let _ = self.transfers.abandon(upload.chunk_id).await;
        Err(WebappError::BlobTransferFailed {
          key: key.clone(),
          reason: err.to_string(),
        })
      }
    };
    match landed {
      Ok(changed) => {
        self
          .emit(BlobEvent::Stored {
            webapp,
            key,
            meta: upload.meta,
            changed,
          })
          .await
      }
      Err(error) => {
        self
          .emit(BlobEvent::Failed {
            webapp,
            key,
            sha256: upload.meta.sha256,
            error,
          })
          .await
      }
    }
  }

  async fn delete(&mut self, webapp: Uuid, key: String) -> StateResult<()> {
    let slot = (webapp, key);
    self.cancel(&slot, "the key was deleted").await;
    let (_, key) = slot;
    self.abandon_partials(key_chunks(webapp, &key)).await;
    let Some(raw) = self.index.kv.blob_get(webapp, &key).await? else {
      return Ok(());
    };
    self.index.kv.blob_delete(webapp, &key).await?;
    if let Ok(meta) = serde_json::from_str::<BlobMeta>(&raw) {
      self.collect_unreferenced(webapp, &meta.sha256).await;
    }
    self.emit(BlobEvent::Removed { webapp, key }).await;
    Ok(())
  }

  async fn purge(&mut self, webapp: Uuid) -> StateResult<()> {
    let slots: Vec<Slot> = self
      .inflight
      .keys()
      .filter(|(owner, _)| *owner == webapp)
      .cloned()
      .collect();
    for slot in slots {
      self.cancel(&slot, "the webapp was uninstalled").await;
    }
    self.abandon_partials(webapp_chunks(webapp)).await;
    self.index.kv.blob_purge(webapp).await?;
    if let Err(err) = tokio::fs::remove_dir_all(self.index.webapp_dir(webapp)).await
      && err.kind() != std::io::ErrorKind::NotFound
    {
      tracing::warn!(%webapp, ?err, "failed to remove the blob dir of an uninstalled webapp");
    }
    Ok(())
  }

  async fn abandon_partials(&self, prefix: String) {
    if let Err(err) = self.transfers.abandon_matching(prefix.clone()).await {
      tracing::warn!(%prefix, ?err, "could not abandon the retained partials of removed blobs");
    }
  }

  async fn verify_stored(
    &mut self,
    webapp: Uuid,
    key: &str,
    meta: &BlobMeta,
    verdict: Option<Verdict>,
  ) -> Result<Option<bool>, WebappError> {
    let path = self.index.blob_path(webapp, &meta.sha256);
    let unreadable = |err: std::io::Error| {
      tracing::warn!(%webapp, sha256 = %meta.sha256, ?err, "stored blob content is unreadable; refusing the put");
      internal(format!("the content stored under this sha256 could not be read: {err}"))
    };
    let found = match tokio::fs::metadata(&path).await {
      Ok(found) => found,
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
        self.drop_content(webapp, &meta.sha256).await.map_err(internal)?;
        return Ok(Some(false));
      }
      Err(err) => return Err(unreadable(err)),
    };
    let actual = match verdict {
      None => return Ok(None),
      Some(Err(err)) => return Err(unreadable(err)),
      Some(Ok((actual, hashed))) if same_file(&found, &hashed) => actual,
      Some(Ok(_)) => return Ok(None),
    };
    if actual != meta.sha256 {
      tracing::warn!(%webapp, sha256 = %meta.sha256, %actual, "stored blob content no longer matches its digest");
      self.drop_content(webapp, &meta.sha256).await.map_err(internal)?;
      return Ok(Some(false));
    }
    if found.len() != u64::from(meta.size) {
      return Err(WebappError::InvalidBlobPut {
        key: key.to_string(),
        reason: format!("the content stored under this sha256 is {} bytes", found.len()),
      });
    }
    Ok(Some(true))
  }

  async fn drop_content(&mut self, webapp: Uuid, sha256: &str) -> StateResult<()> {
    if let Err(err) = tokio::fs::remove_file(self.index.blob_path(webapp, sha256)).await
      && err.kind() != std::io::ErrorKind::NotFound
    {
      return Err(err.into());
    }
    for (key, meta) in self.index.list(webapp).await? {
      if meta.sha256 == sha256 {
        tracing::warn!(%webapp, %key, "dropping a blob record whose content is missing or damaged");
        self.index.kv.blob_delete(webapp, &key).await?;
        self.emit(BlobEvent::Removed { webapp, key }).await;
      }
    }
    Ok(())
  }

  async fn land(
    &mut self,
    webapp: Uuid,
    key: &str,
    meta: &BlobMeta,
    source: &Path,
    chunk_id: &str,
  ) -> Result<bool, WebappError> {
    let landed = self.place(webapp, key, meta, source).await;
    if !matches!(landed, Err(WebappError::BlobQuotaExceeded { .. })) {
      let _ = self.transfers.abandon(chunk_id.to_string()).await;
    }
    landed
  }

  async fn place(&mut self, webapp: Uuid, key: &str, meta: &BlobMeta, source: &Path) -> Result<bool, WebappError> {
    if self.webapps.bundle(webapp).await.is_none() {
      return Err(WebappError::WebappNotFound { id: webapp.to_string() });
    }
    self.check_quota(webapp, key, meta).await?;

    let dir = self.index.webapp_dir(webapp);
    if !tokio::fs::try_exists(&dir).await.map_err(internal)? {
      tokio::fs::create_dir_all(&dir).await.map_err(internal)?;
      paths::fsync(&self.index.root).await.map_err(internal)?;
    }
    paths::rename_or_copy(source, &dir.join(&meta.sha256))
      .await
      .map_err(internal)?;
    let recorded = self.record(webapp, key, meta).await;
    if recorded.is_err() {
      self.collect_unreferenced(webapp, &meta.sha256).await;
    }
    recorded
  }

  async fn record(&mut self, webapp: Uuid, key: &str, meta: &BlobMeta) -> Result<bool, WebappError> {
    let previous = self.index.record(webapp, key).await.map_err(internal)?;
    let encoded = serde_json::to_string(meta).map_err(internal)?;
    self.index.kv.blob_set(webapp, key, encoded).await.map_err(internal)?;
    match previous {
      Some(previous) if previous.sha256 != meta.sha256 => {
        self.collect_unreferenced(webapp, &previous.sha256).await;
        Ok(true)
      }
      Some(previous) => Ok(previous != *meta),
      None => Ok(true),
    }
  }

  async fn collect_unreferenced(&self, webapp: Uuid, sha256: &str) {
    match self.index.list(webapp).await {
      Ok(records) if records.iter().any(|(_, meta)| meta.sha256 == sha256) => {}
      Ok(_) => {
        if let Err(err) = tokio::fs::remove_file(self.index.blob_path(webapp, sha256)).await
          && err.kind() != std::io::ErrorKind::NotFound
        {
          tracing::warn!(%webapp, %sha256, ?err, "failed to remove an unreferenced blob");
        }
      }
      Err(err) => tracing::warn!(%webapp, %sha256, ?err, "could not tell whether a blob is still referenced"),
    }
  }

  async fn check_quota(&self, webapp: Uuid, key: &str, meta: &BlobMeta) -> Result<(), WebappError> {
    let mut sizes: HashMap<String, u64> = self
      .index
      .list(webapp)
      .await
      .map_err(internal)?
      .into_iter()
      .filter(|(existing, _)| existing != key)
      .map(|(_, meta)| (meta.sha256, meta.size as u64))
      .collect();
    sizes.insert(meta.sha256.clone(), meta.size as u64);
    if sizes.values().sum::<u64>() > WEBAPP_BLOB_QUOTA_BYTES {
      return Err(WebappError::BlobQuotaExceeded {
        key: key.to_string(),
        max_bytes: WEBAPP_BLOB_QUOTA_BYTES as u32,
      });
    }
    Ok(())
  }

  async fn replace_inflight(&mut self, slot: &Slot, by: Option<Address>, newer: &BlobMeta, takeover: Takeover) {
    let Some(prior) = self.inflight.remove(slot) else {
      return;
    };
    prior.stop().await;
    if !matches!(takeover, Takeover::Resumes) {
      let _ = self.transfers.abandon(prior.chunk_id).await;
    }
    let settled_by_newer = matches!(takeover, Takeover::Stored) && prior.meta.sha256 == newer.sha256;
    if by != Some(prior.uploader) && !settled_by_newer {
      self
        .emit(BlobEvent::Replaced {
          webapp: slot.0,
          key: slot.1.clone(),
          sha256: prior.meta.sha256,
          uploader: prior.uploader,
        })
        .await;
    }
  }

  async fn cancel(&mut self, slot: &Slot, reason: &str) {
    let Some(upload) = self.inflight.remove(slot) else {
      return;
    };
    upload.stop().await;
    let _ = self.transfers.abandon(upload.chunk_id).await;
    self
      .emit(BlobEvent::Failed {
        webapp: slot.0,
        key: slot.1.clone(),
        sha256: upload.meta.sha256,
        error: WebappError::BlobTransferFailed {
          key: slot.1.clone(),
          reason: reason.to_string(),
        },
      })
      .await;
  }

  fn register(&mut self, slot: Slot, meta: BlobMeta, chunk_id: String, uploader: Address) -> BlobUpload {
    self.next_attempt += 1;
    let cancel = CancellationToken::new();
    let exited = CancellationToken::new();
    self.inflight.insert(
      slot.clone(),
      InFlight {
        attempt: self.next_attempt,
        chunk_id: chunk_id.clone(),
        meta: meta.clone(),
        uploader,
        cancel: cancel.clone(),
        exited: exited.clone(),
      },
    );
    let (webapp, key) = slot;
    BlobUpload {
      webapp,
      key,
      meta,
      uploader,
      chunk_id,
      attempt: self.next_attempt,
      cancel,
      _exited: exited.drop_guard(),
    }
  }

  async fn stored(&mut self, slot: Slot, meta: BlobMeta, changed: bool) -> PutBegin {
    let size = meta.size;
    let (webapp, key) = slot;
    self
      .emit(BlobEvent::Stored {
        webapp,
        key,
        meta,
        changed,
      })
      .await;
    PutBegin::Stored { size }
  }

  async fn emit(&self, event: BlobEvent) {
    let _ = self.events.send(event).await;
  }
}

fn same_file(found: &std::fs::Metadata, hashed: &std::fs::Metadata) -> bool {
  use std::os::unix::fs::MetadataExt;
  let stamp = |meta: &std::fs::Metadata| (meta.dev(), meta.ino(), meta.size(), meta.mtime(), meta.mtime_nsec());
  stamp(found) == stamp(hashed)
}

pub fn blob_url(key: &str, sha256: &str) -> String {
  format!(
    "{BLOB_URL_PREFIX}{}?v={sha256}",
    utf8_percent_encode(key, KEY_ENCODE_SET)
  )
}

fn webapp_chunks(webapp: Uuid) -> String {
  format!("blob:{}:", webapp.simple())
}

fn key_chunks(webapp: Uuid, key: &str) -> String {
  format!("{}{}:", webapp_chunks(webapp), utf8_percent_encode(key, KEY_ENCODE_SET))
}

fn chunk_id(webapp: Uuid, key: &str, meta: &BlobMeta) -> String {
  format!("{}{}:{}", key_chunks(webapp, key), meta.sha256, meta.size)
}

fn webapp_of_dir(name: &str) -> Option<Uuid> {
  Uuid::try_parse(name)
    .ok()
    .filter(|webapp| webapp.simple().to_string() == name)
}

fn is_sha256(name: &str) -> bool {
  name.len() == SHA256_HEX_LEN && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_store_name(name: &str) -> bool {
  let staged = name
    .strip_prefix('.')
    .and_then(|rest| rest.strip_suffix(".staged"))
    .and_then(|rest| rest.split_once('.'))
    .is_some_and(|(sha256, _)| is_sha256(sha256));
  staged || is_sha256(name)
}

fn validate_key(key: &str) -> Result<(), WebappError> {
  let reason = if key.is_empty() {
    "a blob key cannot be empty".to_string()
  } else if key.len() > BLOB_KEY_MAX_BYTES {
    format!("a blob key is at most {BLOB_KEY_MAX_BYTES} bytes")
  } else if key.chars().any(char::is_control) {
    "a blob key cannot contain control characters".to_string()
  } else if key == "." || key == ".." {
    "a blob key cannot be a dot segment".to_string()
  } else {
    return Ok(());
  };
  Err(WebappError::InvalidBlobKey {
    key: key.to_string(),
    reason,
  })
}

fn validate_mime(key: &str, mime: &str) -> Result<(), WebappError> {
  let valid = !mime.is_empty()
    && mime.len() <= BLOB_MIME_MAX_BYTES
    && mime.contains('/')
    && mime.bytes().all(|b| b.is_ascii_graphic() || b == b' ');
  if valid {
    return Ok(());
  }
  Err(WebappError::InvalidBlobPut {
    key: key.to_string(),
    reason: format!("{mime:?} is not a usable content type"),
  })
}

fn normalize_sha256(key: &str, sha256: &str) -> Result<String, WebappError> {
  let normalized = sha256.to_ascii_lowercase();
  if is_sha256(&normalized) {
    return Ok(normalized);
  }
  Err(WebappError::InvalidBlobPut {
    key: key.to_string(),
    reason: "sha256 must be 64 hex characters".into(),
  })
}

fn internal(err: impl std::fmt::Display) -> WebappError {
  WebappError::Internal {
    reason: err.to_string(),
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use tempfile::TempDir;

  use super::*;
  use crate::{digest::sha256_hex, state::storage::WebappProvenanceStore};

  const PHONE: Address = Address([0xFE, 0xED, 0, 0, 0, 1]);
  const DESKTOP: Address = Address([0xFE, 0xED, 0, 0, 0, 2]);

  struct Rig {
    blobs: WebappBlobs,
    webapp: Uuid,
    _dir: TempDir,
  }

  struct Content {
    bytes: Bytes,
    sha256: String,
  }

  fn content(bytes: &'static [u8]) -> Content {
    Content {
      bytes: Bytes::from_static(bytes),
      sha256: sha256_hex(bytes),
    }
  }

  async fn rig() -> (Rig, mpsc::Receiver<BlobEvent>) {
    let dir = TempDir::new().expect("tempdir");
    let db = crate::db::open(None).await.expect("in-memory db");
    let webapp = Uuid::now_v7();
    let bundle = dir.path().join("webapps").join(webapp.simple().to_string());
    std::fs::create_dir_all(&bundle).expect("bundle dir");
    std::fs::write(bundle.join("index.html"), b"<h1>blobs</h1>").expect("index");
    std::fs::write(
      bundle.join("manifest.json"),
      format!(r#"{{"id":"{webapp}","name":"blobs","version":"0.1.0"}}"#),
    )
    .expect("manifest");
    let webapps = WebappRegistry::init(
      dir.path().join("webapps"),
      dir.path().join("builtin"),
      WebappProvenanceStore::new(db.clone()),
    )
    .await
    .expect("registry");
    std::fs::create_dir_all(dir.path().join("assets")).expect("asset dir");
    let (assets, _asset_handle) = AssetCache::init(db.clone(), dir.path().join("assets"))
      .await
      .expect("asset cache")
      .spawn();
    let (events_tx, events) = mpsc::channel(64);
    let blobs = WebappBlobs::init(
      dir.path().join("blobs"),
      dir.path().join("blob-transfers"),
      KvStore::new(db),
      assets,
      webapps,
      events_tx,
    )
    .await
    .expect("blob store");
    (
      Rig {
        blobs,
        webapp,
        _dir: dir,
      },
      events,
    )
  }

  impl Rig {
    async fn put(&self, key: &str, content: &Content, uploader: Option<Address>) -> Result<PutBegin, WebappError> {
      self
        .blobs
        .begin_put(
          self.webapp,
          key.into(),
          "image/gif".into(),
          content.bytes.len() as u32,
          Some(content.sha256.clone()),
          uploader,
        )
        .await
    }

    async fn stream(&self, key: &str, content: &Content, uploader: Address) -> BlobUpload {
      match self.put(key, content, Some(uploader)).await.expect("the put begins") {
        PutBegin::Stream { offset: 0, upload } => upload,
        other => panic!("expected a fresh stream, got {other:?}"),
      }
    }

    async fn feed(&self, upload: &BlobUpload, content: &Content) -> PathBuf {
      match self
        .blobs
        .accept(upload, 0, content.bytes.clone())
        .await
        .expect("accept")
      {
        ChunkOutcome::Completed { path } => path,
        other => panic!("one fragment completes the transfer, got {other:?}"),
      }
    }

    async fn sha_under(&self, key: &str) -> Option<String> {
      self
        .blobs
        .get(self.webapp, key)
        .await
        .expect("record")
        .map(|(_, meta)| meta.sha256)
    }
  }

  #[tokio::test]
  async fn a_finish_from_a_replaced_upload_changes_nothing() {
    let (rig, mut events) = rig().await;
    let (older, newer) = (content(b"the older bytes"), content(b"the newer bytes"));
    let replaced = rig.stream("bg", &older, PHONE).await;
    let landed = rig.feed(&replaced, &older).await;
    let (attempt, told_to_stop) = (replaced.attempt, replaced.cancel.clone());

    let (upload, ()) = tokio::join!(rig.stream("bg", &newer, DESKTOP), async {
      told_to_stop.cancelled().await;
      drop(replaced);
    });
    let (reply, answer) = oneshot::channel();
    rig
      .blobs
      .commands
      .send(Command::Finish {
        webapp: rig.webapp,
        key: "bg".into(),
        attempt,
        outcome: Ok(landed),
        reply,
      })
      .await
      .expect("the store is running");
    answer.await.expect("the stale finish is answered");
    let landed = rig.feed(&upload, &newer).await;
    rig.blobs.finish(upload, Ok(landed)).await;

    assert!(matches!(
      events.recv().await,
      Some(BlobEvent::Replaced { sha256, uploader: PHONE, .. }) if sha256 == older.sha256
    ));
    assert!(matches!(
      events.recv().await,
      Some(BlobEvent::Stored { meta, changed: true, .. }) if meta.sha256 == newer.sha256
    ));
    assert_eq!(rig.sha_under("bg").await, Some(newer.sha256));
  }

  #[tokio::test]
  async fn a_takeover_of_the_same_bytes_resumes_after_the_last_fragment_the_older_loop_wrote() {
    let (rig, mut events) = rig().await;
    let whole = content(b"a first fragment and then a second one");
    let (first, second) = (whole.bytes.slice(..16), whole.bytes.slice(16..));
    let older = rig.stream("bg", &whole, PHONE).await;
    let told_to_stop = older.cancel.clone();

    let (begun, ()) = tokio::join!(rig.put("bg", &whole, Some(DESKTOP)), async {
      told_to_stop.cancelled().await;
      rig
        .blobs
        .accept(&older, 0, first.clone())
        .await
        .expect("the fragment in hand still lands");
      drop(older);
    });
    let PutBegin::Stream { offset, upload } = begun.expect("the takeover begins") else {
      panic!("the transfer is not complete yet");
    };
    assert_eq!(offset as usize, first.len());
    let landed = match rig.blobs.accept(&upload, offset as u32, second).await.expect("accept") {
      ChunkOutcome::Completed { path } => path,
      other => panic!("the second fragment completes the transfer, got {other:?}"),
    };
    rig.blobs.finish(upload, Ok(landed)).await;

    assert!(matches!(
      events.recv().await,
      Some(BlobEvent::Replaced { uploader: PHONE, .. })
    ));
    assert!(matches!(
      events.recv().await,
      Some(BlobEvent::Stored { meta, .. }) if meta.sha256 == whole.sha256
    ));
  }

  #[tokio::test]
  async fn an_upload_that_ended_on_its_own_is_no_longer_in_flight() {
    let (rig, mut events) = rig().await;
    let (parked, next) = (
      content(b"an upload the companion walked away from"),
      content(b"what came next"),
    );
    drop(rig.stream("bg", &parked, PHONE).await);

    rig.blobs.delete(rig.webapp, "bg".into()).await.expect("delete");
    let upload = rig.stream("bg", &next, DESKTOP).await;
    let landed = rig.feed(&upload, &next).await;
    rig.blobs.finish(upload, Ok(landed)).await;

    assert!(
      matches!(
        events.recv().await,
        Some(BlobEvent::Stored { meta, .. }) if meta.sha256 == next.sha256
      ),
      "nothing was cancelled or replaced, so the store is the first thing announced"
    );
  }

  #[tokio::test]
  async fn a_peer_without_an_address_can_put_only_what_the_store_already_holds() {
    let (rig, mut events) = rig().await;
    let (held, fresh) = (content(b"bytes the store holds"), content(b"bytes it has never seen"));
    let upload = rig.stream("held", &held, PHONE).await;
    let landed = rig.feed(&upload, &held).await;
    rig.blobs.finish(upload, Ok(landed)).await;
    assert!(matches!(events.recv().await, Some(BlobEvent::Stored { .. })));

    let begun = rig
      .put("copy", &held, None)
      .await
      .expect("stored content needs no stream");
    assert!(matches!(begun, PutBegin::Stored { size } if size as usize == held.bytes.len()));
    assert!(matches!(
      events.recv().await,
      Some(BlobEvent::Stored { key, changed: true, .. }) if key == "copy"
    ));

    let refused = rig
      .put("fresh", &fresh, None)
      .await
      .expect_err("nowhere to ack a stream");
    assert!(
      matches!(refused, WebappError::BlobTransferFailed { .. }),
      "got {refused:?}"
    );
  }

  #[tokio::test]
  async fn a_refused_put_leaves_the_upload_it_would_have_replaced_running() {
    let (rig, mut events) = rig().await;
    let (running, refused) = (content(b"the upload in flight"), content(b"a put that cannot stream"));
    let upload = rig.stream("bg", &running, PHONE).await;

    rig
      .put("bg", &refused, None)
      .await
      .expect_err("nowhere to ack a stream");
    assert!(!upload.cancel.is_cancelled());
    let landed = rig.feed(&upload, &running).await;
    rig.blobs.finish(upload, Ok(landed)).await;

    assert!(matches!(
      events.recv().await,
      Some(BlobEvent::Stored { meta, .. }) if meta.sha256 == running.sha256
    ));
    assert_eq!(rig.sha_under("bg").await, Some(running.sha256));
  }

  #[tokio::test]
  async fn stored_content_that_cannot_be_read_refuses_the_put_and_leaves_the_store_alone() {
    let (rig, mut events) = rig().await;
    let shared = content(b"content two keys point at");
    let upload = rig.stream("first", &shared, PHONE).await;
    let landed = rig.feed(&upload, &shared).await;
    rig.blobs.finish(upload, Ok(landed)).await;
    rig.put("second", &shared, None).await.expect("the copy dedupes");
    for _ in 0..2 {
      assert!(matches!(events.recv().await, Some(BlobEvent::Stored { .. })));
    }
    let stored = rig.blobs.index.blob_path(rig.webapp, &shared.sha256);
    std::fs::remove_file(&stored).expect("the stored file");
    std::fs::create_dir(&stored).expect("a directory reads back as an io error");

    let refused = rig
      .put("third", &shared, Some(DESKTOP))
      .await
      .expect_err("an unreadable file proves nothing about the content");

    assert!(matches!(refused, WebappError::Internal { .. }), "got {refused:?}");
    assert!(stored.is_dir(), "the store left what it could not read in place");
    for key in ["first", "second"] {
      assert_eq!(rig.sha_under(key).await, Some(shared.sha256.clone()), "{key}");
    }
    assert_eq!(rig.sha_under("third").await, None);
    assert!(events.try_recv().is_err(), "no key was announced as removed");
  }

  const PATIENT: Duration = Duration::from_secs(3);

  struct StoredFifo {
    path: PathBuf,
  }

  impl StoredFifo {
    fn plant(path: PathBuf) -> Self {
      std::fs::remove_file(&path).expect("the stored file");
      let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("mkfifo runs");
      assert!(made.success(), "a fifo stands in for the stored copy");
      Self { path }
    }

    fn writer(&self) -> std::io::Result<std::fs::File> {
      use std::os::unix::fs::OpenOptionsExt;
      std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&self.path)
    }

    async fn once_hashing(&self) -> std::fs::File {
      let deadline = tokio::time::Instant::now() + PATIENT;
      loop {
        match self.writer() {
          Ok(writer) => return writer,
          Err(_) if tokio::time::Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(5)).await,
          Err(err) => panic!("nothing started hashing the stored copy: {err}"),
        }
      }
    }
  }

  impl Drop for StoredFifo {
    fn drop(&mut self) {
      drop(self.writer());
    }
  }

  fn finish_hashing(mut writer: std::fs::File, bytes: &[u8]) {
    use std::io::Write;
    writer.write_all(bytes).expect("the hash reads what the fifo holds");
  }

  async fn landed(rig: &Rig, key: &str, body: &Content) {
    let upload = rig.stream(key, body, PHONE).await;
    let path = rig.feed(&upload, body).await;
    rig.blobs.finish(upload, Ok(path)).await;
  }

  #[tokio::test]
  async fn hashing_a_stored_copy_holds_up_no_other_blob_command() {
    let (rig, mut events) = rig().await;
    let (shared, other) = (
      content(b"content the store already holds"),
      content(b"an unrelated upload"),
    );
    landed(&rig, "first", &shared).await;
    assert!(matches!(events.recv().await, Some(BlobEvent::Stored { .. })));
    let stored = StoredFifo::plant(rig.blobs.index.blob_path(rig.webapp, &shared.sha256));

    let checking = rig.put("second", &shared, Some(DESKTOP));
    let others = async {
      let hashing = stored.once_hashing().await;
      tokio::time::timeout(PATIENT, async {
        rig.blobs.delete(rig.webapp, "absent".into()).await.expect("delete");
        landed(&rig, "other", &other).await;
      })
      .await
      .expect("the store answers while a stored copy is being hashed");
      finish_hashing(hashing, b"bytes that are not the stored content");
    };
    let (checked, ()) = tokio::time::timeout(PATIENT, async { tokio::join!(checking, others) })
      .await
      .expect("the put settles once the hash does");

    assert!(
      matches!(checked, Ok(PutBegin::Stream { offset: 0, .. })),
      "a damaged copy is dropped and streamed again, got {checked:?}"
    );
    assert_eq!(rig.sha_under("other").await, Some(other.sha256));
    assert_eq!(
      rig.sha_under("first").await,
      None,
      "the damaged copy's key went with it"
    );
  }

  #[tokio::test]
  async fn a_hash_of_content_replaced_while_it_ran_is_not_trusted() {
    let (rig, mut events) = rig().await;
    let shared = content(b"content two uploads carry");
    let late = rig.stream("late", &shared, PHONE).await;
    landed(&rig, "first", &shared).await;
    assert!(matches!(events.recv().await, Some(BlobEvent::Stored { .. })));
    let stored = StoredFifo::plant(rig.blobs.index.blob_path(rig.webapp, &shared.sha256));

    let checking = rig.put("third", &shared, Some(DESKTOP));
    let replacing = async {
      let hashing = stored.once_hashing().await;
      let path = rig.feed(&late, &shared).await;
      rig.blobs.finish(late, Ok(path)).await;
      finish_hashing(hashing, b"what the replaced copy held");
    };
    let (checked, ()) = tokio::time::timeout(PATIENT, async { tokio::join!(checking, replacing) })
      .await
      .expect("the put settles once the hash does");

    assert!(
      matches!(checked, Ok(PutBegin::Stored { size }) if size as usize == shared.bytes.len()),
      "the copy that landed meanwhile is hashed again and holds the content, got {checked:?}"
    );
    for key in ["first", "late", "third"] {
      assert_eq!(rig.sha_under(key).await, Some(shared.sha256.clone()), "{key}");
    }
  }

  #[tokio::test]
  async fn deleting_a_key_drops_its_retained_partial_and_spares_a_key_it_prefixes() {
    let (rig, _events) = rig().await;
    let body = content(b"a partial the uploader walked away from");
    let half = body.bytes.len() / 2;
    for key in ["a", "a:b"] {
      let upload = rig.stream(key, &body, PHONE).await;
      rig
        .blobs
        .accept(&upload, 0, body.bytes.slice(..half))
        .await
        .expect("the first half lands");
    }

    rig.blobs.delete(rig.webapp, "a".into()).await.expect("delete");

    assert!(matches!(
      rig.put("a", &body, Some(PHONE)).await,
      Ok(PutBegin::Stream { offset: 0, .. })
    ));
    assert!(
      matches!(rig.put("a:b", &body, Some(PHONE)).await, Ok(PutBegin::Stream { offset, .. }) if offset as usize == half),
      "a key that only shares a prefix keeps its partial"
    );
  }

  #[tokio::test]
  async fn purging_a_webapp_drops_its_retained_partials() {
    let (rig, _events) = rig().await;
    let body = content(b"a partial an uninstalled webapp left behind");
    let upload = rig.stream("bg", &body, PHONE).await;
    rig
      .blobs
      .accept(&upload, 0, body.bytes.slice(..8))
      .await
      .expect("a fragment lands");
    drop(upload);

    rig.blobs.purge(rig.webapp).await.expect("purge");

    assert!(matches!(
      rig.put("bg", &body, Some(PHONE)).await,
      Ok(PutBegin::Stream { offset: 0, .. })
    ));
  }

  #[test]
  fn a_key_with_a_separator_in_it_never_names_another_keys_chunks() {
    let webapp = Uuid::now_v7();
    let meta = BlobMeta {
      sha256: "ab".repeat(32),
      mime: "image/gif".into(),
      size: 10,
    };
    assert!(!chunk_id(webapp, "a:b", &meta).starts_with(&key_chunks(webapp, "a")));
    assert!(chunk_id(webapp, "a:b", &meta).starts_with(&key_chunks(webapp, "a:b")));
    assert!(chunk_id(webapp, "a:b", &meta).starts_with(&webapp_chunks(webapp)));
    assert!(!chunk_id(Uuid::now_v7(), "a", &meta).starts_with(&webapp_chunks(webapp)));
  }

  #[test]
  fn urls_percent_encode_the_key_and_pin_the_digest() {
    assert_eq!(blob_url("bg/hero gif", "ab"), "/.bridgething/blob/bg%2Fhero%20gif?v=ab");
    assert_eq!(blob_url("a-b_c.d~e", "ab"), "/.bridgething/blob/a-b_c.d~e?v=ab");
  }

  #[test]
  fn keys_are_bounded_and_printable() {
    assert!(validate_key("background").is_ok());
    assert!(validate_key("").is_err());
    assert!(validate_key("a\nb").is_err());
    assert!(validate_key(&"k".repeat(BLOB_KEY_MAX_BYTES)).is_ok());
    assert!(validate_key(&"k".repeat(BLOB_KEY_MAX_BYTES + 1)).is_err());
  }

  #[test]
  fn keys_a_url_would_collapse_are_refused() {
    assert!(validate_key(".").is_err());
    assert!(validate_key("..").is_err());
    for fetchable in ["...", ".hidden", "a.", "a/../b", "./a", "../a"] {
      assert!(validate_key(fetchable).is_ok(), "{fetchable}");
      assert!(
        !blob_url(fetchable, "ab").contains("/../") && !blob_url(fetchable, "ab").contains("/./"),
        "{fetchable}"
      );
    }
  }

  #[test]
  fn digests_must_be_hex_and_normalize_to_lowercase() {
    let upper = "AB".repeat(32);
    assert_eq!(normalize_sha256("k", &upper).unwrap(), "ab".repeat(32));
    assert!(normalize_sha256("k", "../../etc/passwd").is_err());
    assert!(normalize_sha256("k", &"g".repeat(64)).is_err());
  }

  #[test]
  fn content_types_must_be_header_safe() {
    assert!(validate_mime("k", "image/gif").is_ok());
    assert!(validate_mime("k", "video/mp4; codecs=avc1").is_ok());
    assert!(validate_mime("k", "gif").is_err());
    assert!(validate_mime("k", "image/gif\r\nx: y").is_err());
  }

  #[test]
  fn uploads_that_disagree_on_size_never_share_a_partial() {
    let webapp = Uuid::now_v7();
    let meta = |size| BlobMeta {
      sha256: "ab".repeat(32),
      mime: "image/gif".into(),
      size,
    };
    assert_eq!(chunk_id(webapp, "bg", &meta(10)), chunk_id(webapp, "bg", &meta(10)));
    assert_ne!(chunk_id(webapp, "bg", &meta(10)), chunk_id(webapp, "bg", &meta(11)));
  }

  #[test]
  fn the_startup_sweep_only_claims_names_the_store_writes() {
    let sha256 = "ab".repeat(32);
    let webapp = Uuid::now_v7();
    assert!(is_store_name(&sha256));
    assert!(is_store_name(&format!(".{sha256}.{}.staged", webapp.simple())));
    for foreign in [
      "index.html",
      "manifest.json",
      ".staged",
      &sha256.to_uppercase(),
      &sha256[1..],
    ] {
      assert!(!is_store_name(foreign), "{foreign}");
    }
    assert_eq!(webapp_of_dir(&webapp.simple().to_string()), Some(webapp));
    assert_eq!(webapp_of_dir(&webapp.hyphenated().to_string()), None);
    assert_eq!(webapp_of_dir("notes.txt"), None);
  }
}
