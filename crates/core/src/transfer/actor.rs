use std::{
  collections::HashMap,
  path::{Path, PathBuf},
  time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
  fs::{File, OpenOptions},
  io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
  sync::{mpsc, oneshot},
};
use tokio_util::bytes::Bytes;

use super::{
  BeginOutcome, ChunkOutcome, STALE_TIMEOUT, SWEEP_INTERVAL, TRANSFER_DISK_BUDGET_BYTES, TransferError, safe_filename,
};

#[derive(Debug)]
pub(super) enum Command {
  Begin {
    id: String,
    expected_size: u64,
    expected_sha256: Option<String>,
    ack: oneshot::Sender<Result<BeginOutcome, TransferError>>,
  },
  AcceptChunk {
    id: String,
    offset: u64,
    bytes: Bytes,
    last: bool,
    ack: oneshot::Sender<Result<ChunkOutcome, TransferError>>,
  },
  Abandon {
    id: String,
    ack: oneshot::Sender<Result<(), TransferError>>,
  },
  HashCompleted {
    id: String,
    result: Result<String, TransferError>,
  },
  BeginVerified {
    id: String,
    result: Result<String, TransferError>,
  },
}

#[derive(Debug, Serialize, Deserialize)]
struct Meta {
  id: String,
  expected_size: u64,
  #[serde(skip_serializing_if = "Option::is_none")]
  expected_sha256: Option<String>,
  partial_path: PathBuf,
  #[serde(default)]
  complete: bool,
}

#[derive(Debug)]
enum WriteOutcome {
  Continue { received: u64 },
  HashPending { partial_path: PathBuf },
}

#[derive(Debug)]
enum FastPath {
  Reject(TransferError),
  Resume(u64),
  VerifyComplete(PathBuf),
  Fresh,
}

#[derive(Debug)]
struct Transfer {
  id: String,
  expected_size: u64,
  expected_sha256: Option<String>,
  received: u64,
  last_touched_unix: i64,
  partial_path: PathBuf,
  meta_path: PathBuf,
  file: Option<File>,
  complete: bool,
}

pub(super) struct ChunkedTransferActor {
  transfers_dir: PathBuf,
  transfers: HashMap<String, Transfer>,
  total_disk_bytes: u64,
  cmd_rx: mpsc::Receiver<Command>,
  self_tx: mpsc::Sender<Command>,
  pending_completions: HashMap<String, oneshot::Sender<Result<ChunkOutcome, TransferError>>>,
  pending_begin_verifies: HashMap<String, oneshot::Sender<Result<BeginOutcome, TransferError>>>,
}

impl ChunkedTransferActor {
  pub(super) async fn bootstrap(
    transfers_dir: PathBuf,
    sweep_dirs: Vec<PathBuf>,
    cmd_rx: mpsc::Receiver<Command>,
    self_tx: mpsc::Sender<Command>,
  ) -> Result<Self, TransferError> {
    let mut transfers = HashMap::new();
    let mut total = 0u64;

    let mut entries = tokio::fs::read_dir(&transfers_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
      let path = entry.path();
      if path.extension().and_then(|s| s.to_str()) != Some("meta") {
        continue;
      }
      match load_recovered_transfer(&path).await {
        Ok(Some(transfer)) => {
          tracing::debug!(
            id = %transfer.id,
            received = transfer.received,
            expected = transfer.expected_size,
            complete = transfer.complete,
            "transfer: recovered on bootstrap",
          );
          total = total.saturating_add(transfer.received);
          transfers.insert(transfer.id.clone(), transfer);
        }
        Ok(None) => {
          let _ = tokio::fs::remove_file(&path).await;
        }
        Err(err) => {
          tracing::warn!(?err, meta = %path.display(), "transfer: failed to recover; deleting");
          let _ = tokio::fs::remove_file(&path).await;
          if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let partial = transfers_dir.join(format!("{stem}.partial"));
            let _ = tokio::fs::remove_file(&partial).await;
          }
        }
      }
    }

    sweep_unreferenced(&transfers_dir, &sweep_dirs, &transfers).await;

    tracing::info!(
      transfers = transfers.len(),
      bytes = total,
      "chunked transfer actor bootstrapped"
    );

    Ok(Self {
      transfers_dir,
      transfers,
      total_disk_bytes: total,
      cmd_rx,
      self_tx,
      pending_completions: HashMap::new(),
      pending_begin_verifies: HashMap::new(),
    })
  }

  pub(super) async fn run(mut self) {
    let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tracing::debug!("chunked transfer actor running");
    loop {
      tokio::select! {
        cmd = self.cmd_rx.recv() => match cmd {
          Some(cmd) => self.handle(cmd).await,
          None => {
            tracing::debug!("chunked transfer actor: command channel closed, exiting");
            return;
          }
        },
        _ = sweep.tick() => self.sweep_stale().await,
      }
    }
  }

  async fn handle(&mut self, cmd: Command) {
    match cmd {
      Command::Begin {
        id,
        expected_size,
        expected_sha256,
        ack,
      } => self.handle_begin(id, expected_size, expected_sha256, ack).await,
      Command::AcceptChunk {
        id,
        offset,
        bytes,
        last,
        ack,
      } => self.handle_chunk(id, offset, bytes, last, ack).await,
      Command::Abandon { id, ack } => {
        let result = self.handle_abandon(id).await;
        let _ = ack.send(result);
      }
      Command::HashCompleted { id, result } => self.handle_hash_completed(id, result).await,
      Command::BeginVerified { id, result } => self.handle_begin_verified(id, result).await,
    }
  }

  async fn handle_begin(
    &mut self,
    id: String,
    expected_size: u64,
    expected_sha256: Option<String>,
    ack: oneshot::Sender<Result<BeginOutcome, TransferError>>,
  ) {
    match self.begin_fast_path(&id, expected_size, &expected_sha256) {
      FastPath::Reject(err) => {
        let _ = ack.send(Err(err));
      }
      FastPath::Resume(offset) => {
        let _ = ack.send(Ok(BeginOutcome::Resume { offset }));
      }
      FastPath::VerifyComplete(partial_path) => {
        self.pending_begin_verifies.insert(id.clone(), ack);
        let self_tx = self.self_tx.clone();
        tokio::spawn(async move {
          let result = hash_file(&partial_path).await;
          let _ = self_tx.send(Command::BeginVerified { id, result }).await;
        });
      }
      FastPath::Fresh => {
        let result = self.begin_fresh(id, expected_size, expected_sha256).await;
        let _ = ack.send(result.map(|offset| BeginOutcome::Resume { offset }));
      }
    }
  }

  fn begin_fast_path(&mut self, id: &str, expected_size: u64, expected_sha256: &Option<String>) -> FastPath {
    if expected_size > TRANSFER_DISK_BUDGET_BYTES {
      return FastPath::Reject(TransferError::TooLarge {
        id: id.to_string(),
        size: expected_size,
      });
    }

    if self.pending_completions.contains_key(id) || self.pending_begin_verifies.contains_key(id) {
      return FastPath::Reject(TransferError::ConflictingBegin { id: id.to_string() });
    }

    if let Some(existing) = self.transfers.get_mut(id) {
      if existing.expected_size != expected_size || &existing.expected_sha256 != expected_sha256 {
        return FastPath::Reject(TransferError::ConflictingBegin { id: id.to_string() });
      }
      existing.last_touched_unix = unix_now();
      if existing.complete || existing.received == existing.expected_size {
        return FastPath::VerifyComplete(existing.partial_path.clone());
      }
      return FastPath::Resume(existing.received);
    }

    FastPath::Fresh
  }

  async fn handle_begin_verified(&mut self, id: String, result: Result<String, TransferError>) {
    let Some(ack) = self.pending_begin_verifies.remove(&id) else {
      tracing::debug!(%id, "begin verify arrived after abandon; discarding");
      return;
    };

    let Some(transfer) = self.transfers.get_mut(&id) else {
      let _ = ack.send(Err(TransferError::UnknownTransfer { id }));
      return;
    };

    let verified = match result {
      Ok(actual) => match transfer.expected_sha256.as_deref() {
        Some(expected) => actual.eq_ignore_ascii_case(expected),
        None => true,
      },
      Err(err) => {
        let _ = ack.send(Err(err));
        return;
      }
    };

    if verified {
      transfer.file = None;
      let path = transfer.partial_path.clone();
      if !transfer.complete {
        transfer.complete = true;
        if let Err(err) = write_meta(&transfer.meta_path.clone(), &meta_from(transfer)).await {
          tracing::warn!(%id, ?err, "failed to persist complete marker; artifact re-verifies after a restart");
        }
      }
      let _ = ack.send(Ok(BeginOutcome::AlreadyComplete { path }));
      return;
    }

    tracing::warn!(%id, "retained complete artifact failed re-verification; restarting fresh");
    let expected_size = transfer.expected_size;
    let expected_sha256 = transfer.expected_sha256.clone();
    let _ = self.handle_abandon(id.clone()).await;
    let result = self.begin_fresh(id, expected_size, expected_sha256).await;
    let _ = ack.send(result.map(|offset| BeginOutcome::Resume { offset }));
  }

  async fn begin_fresh(
    &mut self,
    id: String,
    expected_size: u64,
    expected_sha256: Option<String>,
  ) -> Result<u64, TransferError> {
    let projected = self.total_disk_bytes.saturating_add(expected_size);
    if projected > TRANSFER_DISK_BUDGET_BYTES {
      self.evict_until_under(expected_size).await;
      let still_projected = self.total_disk_bytes.saturating_add(expected_size);
      if still_projected > TRANSFER_DISK_BUDGET_BYTES {
        return Err(TransferError::BudgetExceeded { id });
      }
    }

    let stem = safe_filename(&id);
    let partial_path = self.transfers_dir.join(format!("{stem}.partial"));
    let meta_path = self.transfers_dir.join(format!("{stem}.meta"));

    let _ = tokio::fs::remove_file(&partial_path).await;
    let file = OpenOptions::new()
      .create_new(true)
      .append(true)
      .open(&partial_path)
      .await?;

    let transfer = Transfer {
      id: id.clone(),
      expected_size,
      expected_sha256,
      received: 0,
      last_touched_unix: unix_now(),
      partial_path,
      meta_path: meta_path.clone(),
      file: Some(file),
      complete: false,
    };
    write_meta(&meta_path, &meta_from(&transfer)).await?;
    self.transfers.insert(id, transfer);
    Ok(0)
  }

  async fn handle_chunk(
    &mut self,
    id: String,
    offset: u64,
    bytes: Bytes,
    last: bool,
    ack: oneshot::Sender<Result<ChunkOutcome, TransferError>>,
  ) {
    match self.handle_chunk_write(&id, offset, bytes, last).await {
      Ok(WriteOutcome::Continue { received }) => {
        let _ = ack.send(Ok(ChunkOutcome::Continue { received }));
      }
      Ok(WriteOutcome::HashPending { partial_path }) => {
        self.pending_completions.insert(id.clone(), ack);
        let self_tx = self.self_tx.clone();
        tokio::spawn(async move {
          let result = hash_file(&partial_path).await;
          let _ = self_tx.send(Command::HashCompleted { id, result }).await;
        });
      }
      Err(err) => {
        let _ = ack.send(Err(err));
      }
    }
  }

  async fn handle_chunk_write(
    &mut self,
    id: &str,
    offset: u64,
    bytes: Bytes,
    last: bool,
  ) -> Result<WriteOutcome, TransferError> {
    let transfer = self
      .transfers
      .get_mut(id)
      .ok_or_else(|| TransferError::UnknownTransfer { id: id.to_string() })?;

    if offset != transfer.received {
      return Err(TransferError::OffsetMismatch {
        id: id.to_string(),
        expected: transfer.received,
        got: offset,
      });
    }
    let chunk_len = bytes.len() as u64;
    if transfer.received.saturating_add(chunk_len) > transfer.expected_size {
      return Err(TransferError::SizeOverflow {
        id: id.to_string(),
        expected_size: transfer.expected_size,
        received: transfer.received,
        chunk_len,
      });
    }

    if transfer.file.is_none() {
      transfer.file = Some(OpenOptions::new().append(true).open(&transfer.partial_path).await?);
    }
    let file = transfer.file.as_mut().expect("file opened above");
    file.write_all(&bytes).await?;
    transfer.received += chunk_len;
    transfer.last_touched_unix = unix_now();
    self.total_disk_bytes = self.total_disk_bytes.saturating_add(chunk_len);

    if !last {
      return Ok(WriteOutcome::Continue {
        received: transfer.received,
      });
    }

    if transfer.received != transfer.expected_size {
      return Err(TransferError::SizeMismatch {
        id: id.to_string(),
        expected_size: transfer.expected_size,
        received: transfer.received,
      });
    }

    if let Some(mut file) = transfer.file.take() {
      file.flush().await?;
    }

    Ok(WriteOutcome::HashPending {
      partial_path: transfer.partial_path.clone(),
    })
  }

  async fn handle_hash_completed(&mut self, id: String, result: Result<String, TransferError>) {
    let Some(ack) = self.pending_completions.remove(&id) else {
      tracing::debug!(%id, "hash completion arrived after abandon; discarding");
      return;
    };

    let actual_sha = match result {
      Ok(s) => s,
      Err(err) => {
        let _ = ack.send(Err(err));
        return;
      }
    };

    let Some(transfer_ref) = self.transfers.get(&id) else {
      let _ = ack.send(Err(TransferError::UnknownTransfer { id }));
      return;
    };

    if let Some(expected) = transfer_ref.expected_sha256.as_deref()
      && !actual_sha.eq_ignore_ascii_case(expected)
    {
      let _ = ack.send(Err(TransferError::HashMismatch {
        id,
        expected: expected.to_string(),
        actual: actual_sha,
      }));
      return;
    }

    let transfer = self.transfers.get_mut(&id).expect("present above");
    transfer.complete = true;
    transfer.last_touched_unix = unix_now();
    let path = transfer.partial_path.clone();
    if let Err(err) = write_meta(&transfer.meta_path.clone(), &meta_from(transfer)).await {
      tracing::warn!(%id, ?err, "failed to persist complete marker; artifact re-transfers after a restart");
    }
    let _ = ack.send(Ok(ChunkOutcome::Completed { path }));
  }

  async fn handle_abandon(&mut self, id: String) -> Result<(), TransferError> {
    if let Some(ack) = self.pending_completions.remove(&id) {
      let _ = ack.send(Err(TransferError::UnknownTransfer { id: id.clone() }));
    }
    if let Some(ack) = self.pending_begin_verifies.remove(&id) {
      let _ = ack.send(Err(TransferError::UnknownTransfer { id: id.clone() }));
    }
    if let Some(transfer) = self.transfers.remove(&id) {
      self.total_disk_bytes = self.total_disk_bytes.saturating_sub(transfer.received);
      let _ = tokio::fs::remove_file(&transfer.partial_path).await;
      let _ = tokio::fs::remove_file(&transfer.meta_path).await;
    }
    Ok(())
  }

  async fn evict_until_under(&mut self, incoming_size: u64) {
    while self.total_disk_bytes.saturating_add(incoming_size) > TRANSFER_DISK_BUDGET_BYTES {
      let Some(victim_id) = self
        .transfers
        .iter()
        .min_by_key(|(_, t)| t.last_touched_unix)
        .map(|(id, _)| id.clone())
      else {
        break;
      };
      tracing::warn!(
        id = %victim_id,
        bytes = self.total_disk_bytes,
        "transfer: evicting oldest in-flight to free disk budget"
      );
      let _ = self.handle_abandon(victim_id).await;
    }
  }

  async fn sweep_stale(&mut self) {
    let now = unix_now();
    let stale: Vec<String> = self
      .transfers
      .iter()
      .filter(|(_, t)| is_stale(now, t.last_touched_unix))
      .map(|(id, _)| id.clone())
      .collect();
    for id in stale {
      tracing::info!(%id, "transfer: GCing stale partial");
      let _ = self.handle_abandon(id).await;
    }
  }
}

fn is_stale(now: i64, last_touched: i64) -> bool {
  now.saturating_sub(last_touched) >= STALE_TIMEOUT.as_secs() as i64
}

async fn load_recovered_transfer(meta_path: &Path) -> Result<Option<Transfer>, TransferError> {
  let raw = tokio::fs::read(meta_path).await?;
  let meta: Meta = serde_json::from_slice(&raw)?;

  let partial_path = meta.partial_path.clone();
  let Ok(fs_meta) = tokio::fs::metadata(&partial_path).await else {
    return Ok(None);
  };

  let actual_size = fs_meta.len();
  if actual_size > meta.expected_size {
    let f = OpenOptions::new().write(true).open(&partial_path).await?;
    f.set_len(meta.expected_size).await?;
  }
  let received = std::cmp::min(actual_size, meta.expected_size);
  let last_touched_unix = fs_meta
    .modified()
    .ok()
    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    .map(|d| d.as_secs() as i64)
    .unwrap_or_else(unix_now);

  Ok(Some(Transfer {
    id: meta.id,
    expected_size: meta.expected_size,
    expected_sha256: meta.expected_sha256,
    received,
    last_touched_unix,
    partial_path,
    meta_path: meta_path.to_path_buf(),
    file: None,
    complete: meta.complete && received == meta.expected_size,
  }))
}

async fn sweep_unreferenced(transfers_dir: &Path, sweep_dirs: &[PathBuf], transfers: &HashMap<String, Transfer>) {
  let known: std::collections::HashSet<PathBuf> = transfers
    .values()
    .flat_map(|t| [t.partial_path.clone(), t.meta_path.clone()])
    .collect();
  let mut dirs: Vec<&Path> = vec![transfers_dir];
  dirs.extend(sweep_dirs.iter().map(PathBuf::as_path));
  dirs.dedup();
  for dir in dirs {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
      continue;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
      let path = entry.path();
      if path.extension().and_then(|s| s.to_str()) == Some("meta") || known.contains(&path) {
        continue;
      }
      tracing::info!(path = %path.display(), "transfer: removing unreferenced file on bootstrap");
      let _ = tokio::fs::remove_file(&path).await;
    }
  }
}

async fn write_meta(meta_path: &Path, meta: &Meta) -> Result<(), TransferError> {
  let bytes = serde_json::to_vec(meta)?;
  tokio::fs::write(meta_path, bytes).await?;
  Ok(())
}

async fn hash_file(path: &Path) -> Result<String, TransferError> {
  let mut file = File::open(path).await?;
  file.seek(std::io::SeekFrom::Start(0)).await?;
  let mut hasher = Sha256::new();
  let mut buf = vec![0u8; 64 * 1024];
  loop {
    let n = file.read(&mut buf).await?;
    if n == 0 {
      break;
    }
    hasher.update(&buf[..n]);
  }
  Ok(hex::encode(hasher.finalize()))
}

fn meta_from(t: &Transfer) -> Meta {
  Meta {
    id: t.id.clone(),
    expected_size: t.expected_size,
    expected_sha256: t.expected_sha256.clone(),
    partial_path: t.partial_path.clone(),
    complete: t.complete,
  }
}

fn unix_now() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs() as i64)
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_future_touched_transfer_is_never_stale() {
    let now = 1_700_000_000i64;
    assert!(
      !is_stale(now, now + 300),
      "clock behind mtime must read fresh, not wrapped"
    );
    assert!(!is_stale(now, now));
    assert!(!is_stale(now, now - STALE_TIMEOUT.as_secs() as i64 + 1));
    assert!(is_stale(now, now - STALE_TIMEOUT.as_secs() as i64));
  }
}
