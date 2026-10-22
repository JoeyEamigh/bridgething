use std::{
  collections::{HashMap, HashSet},
  path::{Path, PathBuf},
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use libbridgething::AssetRetention;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::bytes::Bytes;

use super::{
  AssetError, CachedAsset, DISK_BUDGET_BYTES, DISK_FREE_HEADROOM_BYTES, MEMORY_BUDGET_BYTES, TTL_SWEEP_INTERVAL,
  storage::{AssetActiveModel, AssetColumn, AssetEntity},
};
use crate::digest::sha256_hex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
  Memory,
  Disk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifetime {
  Pinned,
  Lru,
  Ttl(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
  pub(crate) tier: Tier,
  pub(crate) lifetime: Lifetime,
}

impl Retention {
  pub const MEM_LRU: Retention = Retention {
    tier: Tier::Memory,
    lifetime: Lifetime::Lru,
  };
  pub const MEM_PINNED: Retention = Retention {
    tier: Tier::Memory,
    lifetime: Lifetime::Pinned,
  };
  pub const DISK_PINNED: Retention = Retention {
    tier: Tier::Disk,
    lifetime: Lifetime::Pinned,
  };
  pub const DISK_LRU: Retention = Retention {
    tier: Tier::Disk,
    lifetime: Lifetime::Lru,
  };

  pub fn disk_ttl(seconds: u32) -> Retention {
    Retention {
      tier: Tier::Disk,
      lifetime: Lifetime::Ttl(seconds),
    }
  }

  pub fn from_wire(w: AssetRetention) -> Retention {
    match w {
      AssetRetention::Lru => Retention::MEM_LRU,
      AssetRetention::Pinned => Retention::MEM_PINNED,
      AssetRetention::Ttl(t) => Retention {
        tier: Tier::Memory,
        lifetime: Lifetime::Ttl(t.seconds),
      },
      AssetRetention::Persistent => Retention::DISK_PINNED,
    }
  }

  fn ttl_seconds(&self) -> Option<u32> {
    match self.lifetime {
      Lifetime::Ttl(s) => Some(s),
      _ => None,
    }
  }

  fn is_pinned(&self) -> bool {
    matches!(self.lifetime, Lifetime::Pinned)
  }

  fn is_disk_persisted(&self) -> bool {
    self.tier == Tier::Disk && !matches!(self.lifetime, Lifetime::Ttl(_))
  }
}

#[derive(Debug, Clone)]
pub enum AssetCacheEvent {
  Ready { id: String },
  Cleared { id: String },
}

#[derive(Debug)]
pub(super) enum Command {
  Insert {
    id: String,
    bytes: Bytes,
    mime: Option<String>,
    retention: Retention,
    ack: oneshot::Sender<Result<(), AssetError>>,
  },
  InsertFromPath {
    id: String,
    source: PathBuf,
    mime: Option<String>,
    retention: Retention,
    ack: oneshot::Sender<Result<(), AssetError>>,
  },
  SetRetention {
    id: String,
    retention: Retention,
    ack: oneshot::Sender<Result<(), AssetError>>,
  },
  Get {
    id: String,
    reply: oneshot::Sender<Option<CachedAsset>>,
  },
  Contains {
    id: String,
    reply: oneshot::Sender<bool>,
  },
  #[cfg(feature = "test-tap")]
  IsDiskPinned {
    id: String,
    reply: oneshot::Sender<bool>,
  },
  Clear {
    id: String,
    ack: oneshot::Sender<Result<(), AssetError>>,
  },
  ReserveDisk {
    need_bytes: u64,
    ack: oneshot::Sender<()>,
  },
}

#[derive(Debug)]
enum EntryStorage {
  Memory(Bytes),
  Disk(PathBuf),
}

#[derive(Debug)]
struct Entry {
  retention: Retention,
  mime: Option<String>,
  byte_len: usize,
  accessed_at: i64,
  lru_seq: u64,
  ttl_deadline: Option<Instant>,
  storage: EntryStorage,
}

impl Entry {
  fn tier(&self) -> Tier {
    match self.storage {
      EntryStorage::Memory(_) => Tier::Memory,
      EntryStorage::Disk(_) => Tier::Disk,
    }
  }
}

pub(super) struct AssetActor {
  blobs_dir: PathBuf,
  entries: HashMap<String, Entry>,
  memory_byte_total: usize,
  disk_byte_total: usize,
  lru_clock: u64,
  dirty_persist: HashSet<String>,
  db: DatabaseConnection,
  cmd_rx: mpsc::Receiver<Command>,
  events_tx: broadcast::Sender<AssetCacheEvent>,
}

impl AssetActor {
  pub(super) fn new(
    db: DatabaseConnection,
    blobs_dir: PathBuf,
    cmd_rx: mpsc::Receiver<Command>,
    events_tx: broadcast::Sender<AssetCacheEvent>,
  ) -> Self {
    Self {
      blobs_dir,
      entries: HashMap::new(),
      memory_byte_total: 0,
      disk_byte_total: 0,
      lru_clock: 0,
      dirty_persist: HashSet::new(),
      db,
      cmd_rx,
      events_tx,
    }
  }

  fn next_lru_seq(&mut self) -> u64 {
    self.lru_clock = self.lru_clock.wrapping_add(1);
    self.lru_clock
  }

  pub(super) async fn bootstrap(mut self) -> Result<Self, AssetError> {
    tokio::fs::create_dir_all(&self.blobs_dir).await.ok();
    let rows = AssetEntity::find()
      .order_by_asc(AssetColumn::AccessedAt)
      .all(&self.db)
      .await?;

    let mut live_blob_names: HashSet<String> = HashSet::new();
    for row in rows {
      let path = PathBuf::from(&row.path);
      if !path.exists() {
        tracing::warn!(id = %row.id, path = %row.path, "asset cache: persistent row references missing file; deleting row");
        let _ = AssetEntity::delete_by_id(row.id.clone()).exec(&self.db).await;
        continue;
      }
      if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        live_blob_names.insert(name.to_string());
      }
      let byte_len = row.byte_len.max(0) as usize;
      self.disk_byte_total = self.disk_byte_total.saturating_add(byte_len);
      let lru_seq = self.next_lru_seq();
      self.entries.insert(
        row.id,
        Entry {
          retention: if row.pinned {
            Retention::DISK_PINNED
          } else {
            Retention::DISK_LRU
          },
          mime: row.mime,
          byte_len,
          accessed_at: row.accessed_at,
          lru_seq,
          ttl_deadline: None,
          storage: EntryStorage::Disk(path),
        },
      );
    }
    self.sweep_orphan_blobs(&live_blob_names).await;
    self.evict_until_under_disk_budget().await;
    tracing::debug!(
      entries = self.entries.len(),
      disk_bytes = self.disk_byte_total,
      "asset cache bootstrapped"
    );
    Ok(self)
  }

  async fn sweep_orphan_blobs(&self, live_blob_names: &HashSet<String>) {
    let Ok(mut dir) = tokio::fs::read_dir(&self.blobs_dir).await else {
      return;
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
      let name = entry.file_name();
      let Some(name) = name.to_str() else { continue };
      if live_blob_names.contains(name) {
        continue;
      }
      tracing::debug!(blob = %name, "asset cache: deleting orphan blob (no backing row)");
      let _ = tokio::fs::remove_file(entry.path()).await;
    }
  }

  pub(super) async fn run(mut self) {
    let mut sweep = tokio::time::interval(TTL_SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
      tokio::select! {
        cmd = self.cmd_rx.recv() => match cmd {
          Some(cmd) => self.handle(cmd).await,
          None => {
            tracing::debug!("asset cache actor: command channel closed, exiting");
            return;
          }
        },
        _ = sweep.tick() => {
          self.ttl_sweep().await;
          self.flush_persist_touches().await;
        }
      }
    }
  }

  async fn handle(&mut self, cmd: Command) {
    match cmd {
      Command::Insert {
        id,
        bytes,
        mime,
        retention,
        ack,
      } => {
        let result = self.handle_insert(id, bytes, mime, retention).await;
        let _ = ack.send(result);
      }
      Command::InsertFromPath {
        id,
        source,
        mime,
        retention,
        ack,
      } => {
        let result = self.handle_insert_from_path(id, source, mime, retention).await;
        let _ = ack.send(result);
      }
      Command::SetRetention { id, retention, ack } => {
        let result = self.handle_set_retention(id, retention).await;
        let _ = ack.send(result);
      }
      Command::Get { id, reply } => {
        let result = self.handle_get(id).await;
        let _ = reply.send(result);
      }
      Command::Contains { id, reply } => {
        let _ = reply.send(self.entries.contains_key(&id));
      }
      #[cfg(feature = "test-tap")]
      Command::IsDiskPinned { id, reply } => {
        let pinned = self
          .entries
          .get(&id)
          .is_some_and(|entry| entry.tier() == Tier::Disk && entry.retention.is_pinned());
        let _ = reply.send(pinned);
      }
      Command::Clear { id, ack } => {
        let result = self.handle_clear(id).await;
        let _ = ack.send(result);
      }
      Command::ReserveDisk { need_bytes, ack } => {
        self.reserve_disk(need_bytes).await;
        let _ = ack.send(());
      }
    }
  }

  fn ttl_deadline_for(retention: Retention) -> Option<Instant> {
    retention
      .ttl_seconds()
      .map(|s| Instant::now() + Duration::from_secs(s.max(1) as u64))
  }

  async fn handle_insert(
    &mut self,
    id: String,
    bytes: Bytes,
    mime: Option<String>,
    retention: Retention,
  ) -> Result<(), AssetError> {
    if bytes.is_empty() {
      tracing::debug!(%id, "asset cache: ignoring 0-byte insert");
      return Ok(());
    }
    let byte_len = bytes.len();
    let now = unix_now();

    self.evict_entry(&id).await;

    match retention.tier {
      Tier::Memory => {
        self.memory_byte_total = self.memory_byte_total.saturating_add(byte_len);
        let lru_seq = self.next_lru_seq();
        self.entries.insert(
          id.clone(),
          Entry {
            retention,
            mime,
            byte_len,
            accessed_at: now,
            lru_seq,
            ttl_deadline: Self::ttl_deadline_for(retention),
            storage: EntryStorage::Memory(bytes),
          },
        );
        self.evict_until_under_memory_budget().await;
      }
      Tier::Disk => {
        let dest = self.blobs_dir.join(safe_blob_name(&id));
        if let Some(parent) = dest.parent() {
          tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::write(&dest, &bytes).await?;
        self.disk_byte_total = self.disk_byte_total.saturating_add(byte_len);
        if retention.is_disk_persisted() {
          self
            .persist_write(&id, &dest, mime.as_deref(), byte_len, now, retention.is_pinned())
            .await?;
        }
        let lru_seq = self.next_lru_seq();
        self.entries.insert(
          id.clone(),
          Entry {
            retention,
            mime,
            byte_len,
            accessed_at: now,
            lru_seq,
            ttl_deadline: Self::ttl_deadline_for(retention),
            storage: EntryStorage::Disk(dest),
          },
        );
        self.make_disk_room(&id, retention.is_pinned()).await?;
      }
    }

    let _ = self.events_tx.send(AssetCacheEvent::Ready { id });
    Ok(())
  }

  async fn handle_insert_from_path(
    &mut self,
    id: String,
    source: PathBuf,
    mime: Option<String>,
    retention: Retention,
  ) -> Result<(), AssetError> {
    let byte_len = tokio::fs::metadata(&source).await?.len() as usize;
    if byte_len == 0 {
      tracing::debug!(%id, "asset cache: ignoring 0-byte insert_from_path");
      tokio::fs::remove_file(&source).await.ok();
      return Ok(());
    }
    let now = unix_now();

    self.evict_entry(&id).await;

    match retention.tier {
      Tier::Disk => {
        let dest = self.blobs_dir.join(safe_blob_name(&id));
        if let Some(parent) = dest.parent() {
          tokio::fs::create_dir_all(parent).await.ok();
        }
        if dest.exists() {
          tokio::fs::remove_file(&dest).await.ok();
        }
        crate::paths::rename_or_copy(&source, &dest).await?;
        self.disk_byte_total = self.disk_byte_total.saturating_add(byte_len);
        if retention.is_disk_persisted() {
          self
            .persist_write(&id, &dest, mime.as_deref(), byte_len, now, retention.is_pinned())
            .await?;
        }
        let lru_seq = self.next_lru_seq();
        self.entries.insert(
          id.clone(),
          Entry {
            retention,
            mime,
            byte_len,
            accessed_at: now,
            lru_seq,
            ttl_deadline: Self::ttl_deadline_for(retention),
            storage: EntryStorage::Disk(dest),
          },
        );
        self.make_disk_room(&id, retention.is_pinned()).await?;
      }
      Tier::Memory => {
        let bytes = tokio::fs::read(&source).await?;
        tokio::fs::remove_file(&source).await.ok();
        let bytes = Bytes::from(bytes);
        self.memory_byte_total = self.memory_byte_total.saturating_add(byte_len);
        let lru_seq = self.next_lru_seq();
        self.entries.insert(
          id.clone(),
          Entry {
            retention,
            mime,
            byte_len,
            accessed_at: now,
            lru_seq,
            ttl_deadline: Self::ttl_deadline_for(retention),
            storage: EntryStorage::Memory(bytes),
          },
        );
        self.evict_until_under_memory_budget().await;
      }
    }

    let _ = self.events_tx.send(AssetCacheEvent::Ready { id });
    Ok(())
  }

  async fn handle_set_retention(&mut self, id: String, retention: Retention) -> Result<(), AssetError> {
    let disk_action = {
      let Some(entry) = self.entries.get_mut(&id) else {
        return Ok(());
      };
      if entry.tier() != retention.tier {
        tracing::warn!(
          id = %id,
          "asset cache: set_retention would change tier; ignoring (use clear + reinsert to move tiers)"
        );
        return Ok(());
      }
      entry.retention = retention;
      entry.ttl_deadline = Self::ttl_deadline_for(retention);
      match &entry.storage {
        EntryStorage::Disk(path) => Some((path.clone(), entry.mime.clone(), entry.byte_len, entry.accessed_at)),
        EntryStorage::Memory(_) => None,
      }
    };

    if let Some((dest, mime, byte_len, now)) = disk_action {
      if retention.is_disk_persisted() {
        self
          .persist_write(&id, &dest, mime.as_deref(), byte_len, now, retention.is_pinned())
          .await?;
      } else if let Err(err) = AssetEntity::delete_by_id(id.clone()).exec(&self.db).await {
        tracing::warn!(?err, id = %id, "asset cache: failed to drop row on disk demote");
      }
    }
    Ok(())
  }

  async fn handle_get(&mut self, id: String) -> Option<CachedAsset> {
    let now = unix_now();
    let lru_seq = self.next_lru_seq();

    let (path, mime) = {
      let entry = self.entries.get_mut(&id)?;
      entry.accessed_at = now;
      entry.lru_seq = lru_seq;
      let mime = entry.mime.clone();
      match &entry.storage {
        EntryStorage::Memory(bytes) => {
          return Some(CachedAsset {
            bytes: bytes.clone(),
            mime,
          });
        }
        EntryStorage::Disk(p) => (p.clone(), mime),
      }
    };

    let raw = match tokio::fs::read(&path).await {
      Ok(b) => b,
      Err(err) => {
        tracing::warn!(?err, id = %id, path = %path.display(), "asset cache: disk blob unreadable");
        return None;
      }
    };

    self.dirty_persist.insert(id);

    Some(CachedAsset {
      bytes: Bytes::from(raw),
      mime,
    })
  }

  async fn handle_clear(&mut self, id: String) -> Result<(), AssetError> {
    if self.entries.contains_key(&id) {
      self.evict_entry(&id).await;
      let _ = self.events_tx.send(AssetCacheEvent::Cleared { id });
    }
    Ok(())
  }

  async fn evict_entry(&mut self, id: &str) -> bool {
    let Some(entry) = self.entries.remove(id) else {
      return false;
    };
    match &entry.storage {
      EntryStorage::Memory(_) => {
        self.memory_byte_total = self.memory_byte_total.saturating_sub(entry.byte_len);
      }
      EntryStorage::Disk(path) => {
        self.disk_byte_total = self.disk_byte_total.saturating_sub(entry.byte_len);
        self.dirty_persist.remove(id);
        let _ = tokio::fs::remove_file(path).await;
        if entry.retention.is_disk_persisted()
          && let Err(err) = AssetEntity::delete_by_id(id.to_string()).exec(&self.db).await
        {
          tracing::warn!(?err, id = %id, "asset cache: failed to delete persistent row");
        }
      }
    }
    true
  }

  async fn evict_until_under_memory_budget(&mut self) {
    while self.memory_byte_total > MEMORY_BUDGET_BYTES {
      let Some(victim) = self.pick_memory_victim() else {
        tracing::error!(
          total = self.memory_byte_total,
          budget = MEMORY_BUDGET_BYTES,
          "asset cache: memory budget exceeded but no eviction candidate"
        );
        break;
      };
      let pinned_warning = matches!(
        self.entries.get(&victim).map(|e| e.retention.lifetime),
        Some(Lifetime::Pinned)
      );
      if pinned_warning {
        tracing::warn!(
          id = %victim,
          total = self.memory_byte_total,
          budget = MEMORY_BUDGET_BYTES,
          "asset cache: evicting pinned asset under emergency memory pressure"
        );
      }
      self.evict_entry(&victim).await;
      let _ = self.events_tx.send(AssetCacheEvent::Cleared { id: victim });
    }
  }

  async fn make_disk_room(&mut self, new_id: &str, new_is_pinned: bool) -> Result<(), AssetError> {
    self.evict_until_under_disk_budget().await;
    if self.disk_byte_total > DISK_BUDGET_BYTES && new_is_pinned {
      tracing::warn!(
        id = %new_id,
        total = self.disk_byte_total,
        budget = DISK_BUDGET_BYTES,
        "asset cache: rejecting disk pin that exceeds budget after evicting all Ttl"
      );
      self.evict_entry(new_id).await;
      return Err(AssetError::DiskBudgetExceeded);
    }
    Ok(())
  }

  async fn evict_until_under_disk_budget(&mut self) {
    self.enforce_disk_limits(DISK_FREE_HEADROOM_BYTES as u64).await;
  }

  async fn reserve_disk(&mut self, need_bytes: u64) {
    let floor = (DISK_FREE_HEADROOM_BYTES as u64).saturating_add(need_bytes);
    self.enforce_disk_limits(floor).await;
  }

  async fn enforce_disk_limits(&mut self, min_free: u64) {
    let mut free = crate::paths::partition_free_bytes(&self.blobs_dir);
    while disk_over_limit(self.disk_byte_total as u64, free, DISK_BUDGET_BYTES as u64, min_free) {
      let Some(victim) = self.pick_disk_evictable_victim() else {
        break;
      };
      let freed = self.entries.get(&victim).map_or(0, |e| e.byte_len) as u64;
      tracing::warn!(
        id = %victim,
        total = self.disk_byte_total,
        free,
        budget = DISK_BUDGET_BYTES,
        min_free,
        "asset cache: evicting disk asset under disk pressure"
      );
      self.evict_entry(&victim).await;
      let _ = self.events_tx.send(AssetCacheEvent::Cleared { id: victim });
      free = free.saturating_add(freed);
    }
    while free < min_free {
      let Some(victim) = self.pick_disk_pinned_victim() else {
        break;
      };
      let freed = self.entries.get(&victim).map_or(0, |e| e.byte_len) as u64;
      tracing::warn!(
        id = %victim,
        free,
        min_free,
        "asset cache: evicting pinned disk asset under emergency free-space pressure"
      );
      self.evict_entry(&victim).await;
      let _ = self.events_tx.send(AssetCacheEvent::Cleared { id: victim });
      free = free.saturating_add(freed);
    }
  }

  fn pick_memory_victim(&self) -> Option<String> {
    let mut best_lru: Option<(&str, u128)> = None;
    let mut best_ttl: Option<(&str, u64)> = None;
    let mut best_pinned: Option<(&str, u64)> = None;

    for (id, entry) in &self.entries {
      if !matches!(entry.storage, EntryStorage::Memory(_)) {
        continue;
      }
      match entry.retention.lifetime {
        Lifetime::Lru => {
          let age = self.lru_clock.saturating_sub(entry.lru_seq) as u128;
          let score = age.saturating_mul(entry.byte_len as u128).max(1);
          match best_lru {
            Some((_, s)) if score <= s => {}
            _ => best_lru = Some((id.as_str(), score)),
          }
        }
        Lifetime::Ttl(_) => match best_ttl {
          Some((_, t)) if entry.lru_seq >= t => {}
          _ => best_ttl = Some((id.as_str(), entry.lru_seq)),
        },
        Lifetime::Pinned => match best_pinned {
          Some((_, t)) if entry.lru_seq >= t => {}
          _ => best_pinned = Some((id.as_str(), entry.lru_seq)),
        },
      }
    }

    best_lru
      .map(|(id, _)| id.to_string())
      .or_else(|| best_ttl.map(|(id, _)| id.to_string()))
      .or_else(|| best_pinned.map(|(id, _)| id.to_string()))
  }

  fn pick_disk_evictable_victim(&self) -> Option<String> {
    self
      .entries
      .iter()
      .filter(|(_, e)| matches!(e.storage, EntryStorage::Disk(_)) && !matches!(e.retention.lifetime, Lifetime::Pinned))
      .min_by_key(|(_, e)| (e.accessed_at, e.lru_seq))
      .map(|(id, _)| id.clone())
  }

  fn pick_disk_pinned_victim(&self) -> Option<String> {
    self
      .entries
      .iter()
      .filter(|(_, e)| matches!(e.storage, EntryStorage::Disk(_)) && matches!(e.retention.lifetime, Lifetime::Pinned))
      .min_by_key(|(_, e)| (e.accessed_at, e.lru_seq))
      .map(|(id, _)| id.clone())
  }

  async fn ttl_sweep(&mut self) {
    let now = Instant::now();
    let expired: Vec<String> = self
      .entries
      .iter()
      .filter_map(|(id, entry)| match entry.ttl_deadline {
        Some(deadline) if deadline <= now => Some(id.clone()),
        _ => None,
      })
      .collect();
    for id in expired {
      self.evict_entry(&id).await;
      let _ = self.events_tx.send(AssetCacheEvent::Cleared { id });
    }
  }

  async fn persist_write(
    &self,
    id: &str,
    path: &Path,
    mime: Option<&str>,
    byte_len: usize,
    now: i64,
    pinned: bool,
  ) -> Result<(), AssetError> {
    let model = AssetActiveModel {
      id: Set(id.to_string()),
      path: Set(path.to_string_lossy().into_owned()),
      mime: Set(mime.map(str::to_string)),
      byte_len: Set(byte_len as i64),
      inserted_at: Set(now),
      accessed_at: Set(now),
      pinned: Set(pinned),
    };
    AssetEntity::insert(model)
      .on_conflict(
        sea_orm::sea_query::OnConflict::column(AssetColumn::Id)
          .update_columns([
            AssetColumn::Path,
            AssetColumn::Mime,
            AssetColumn::ByteLen,
            AssetColumn::InsertedAt,
            AssetColumn::AccessedAt,
            AssetColumn::Pinned,
          ])
          .to_owned(),
      )
      .exec(&self.db)
      .await?;
    Ok(())
  }

  async fn flush_persist_touches(&mut self) {
    if self.dirty_persist.is_empty() {
      return;
    }
    let ids: Vec<String> = self.dirty_persist.drain().collect();
    let now = unix_now();
    if let Err(err) = AssetEntity::update_many()
      .col_expr(AssetColumn::AccessedAt, sea_orm::sea_query::Expr::value(now))
      .filter(AssetColumn::Id.is_in(ids))
      .exec(&self.db)
      .await
    {
      tracing::warn!(?err, "asset cache: failed to flush persistent accessed_at batch");
    }
  }
}

fn safe_blob_name(id: &str) -> String {
  sha256_hex(id.as_bytes())
}

fn disk_over_limit(current_bytes: u64, free_bytes: u64, budget: u64, headroom: u64) -> bool {
  current_bytes > budget || free_bytes < headroom
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

  fn temp_blobs() -> PathBuf {
    let p = std::env::temp_dir().join(format!("bridgething-asset-test-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&p).unwrap();
    p
  }

  async fn fresh() -> AssetActor {
    let db = crate::db::open(None).await.unwrap();
    let (events_tx, _) = broadcast::channel(16);
    let (_cmd_tx, cmd_rx) = mpsc::channel(16);
    AssetActor::new(db, temp_blobs(), cmd_rx, events_tx)
      .bootstrap()
      .await
      .unwrap()
  }

  #[tokio::test]
  async fn lru_insert_and_get_round_trips() {
    let mut a = fresh().await;
    a.handle_insert(
      "a/1".into(),
      Bytes::from_static(b"hello"),
      Some("text/plain".into()),
      Retention::MEM_LRU,
    )
    .await
    .unwrap();
    let got = a.handle_get("a/1".into()).await.unwrap();
    assert_eq!(&got.bytes[..], b"hello");
    assert_eq!(got.mime.as_deref(), Some("text/plain"));
  }

  #[tokio::test]
  async fn ttl_expires_on_sweep() {
    let mut a = fresh().await;
    a.handle_insert(
      "t/1".into(),
      Bytes::from_static(b"x"),
      None,
      Retention {
        tier: Tier::Memory,
        lifetime: Lifetime::Ttl(1),
      },
    )
    .await
    .unwrap();
    assert!(a.handle_get("t/1".into()).await.is_some());
    if let Some(e) = a.entries.get_mut("t/1") {
      e.ttl_deadline = Some(Instant::now() - Duration::from_secs(1));
    }
    a.ttl_sweep().await;
    assert!(a.handle_get("t/1".into()).await.is_none());
  }

  #[tokio::test]
  async fn persistent_via_insert_from_path_survives_restart() {
    let blobs = temp_blobs();
    let db = crate::db::open(None).await.unwrap();
    let (events_tx, _) = broadcast::channel(16);
    let (_cmd_tx, cmd_rx) = mpsc::channel(16);
    let mut a = AssetActor::new(db.clone(), blobs.clone(), cmd_rx, events_tx)
      .bootstrap()
      .await
      .unwrap();

    let source = std::env::temp_dir().join(format!("src-{}", uuid::Uuid::now_v7()));
    tokio::fs::write(&source, b"persistent-payload").await.unwrap();
    a.handle_insert_from_path(
      "p/1".into(),
      source,
      Some("application/octet-stream".into()),
      Retention::DISK_PINNED,
    )
    .await
    .unwrap();
    drop(a);

    let (events_tx2, _) = broadcast::channel(16);
    let (_cmd_tx2, cmd_rx2) = mpsc::channel(16);
    let mut a2 = AssetActor::new(db, blobs, cmd_rx2, events_tx2)
      .bootstrap()
      .await
      .unwrap();
    let got = a2.handle_get("p/1".into()).await.unwrap();
    assert_eq!(&got.bytes[..], b"persistent-payload");
  }

  #[tokio::test]
  async fn small_disk_ttl_insert_serves_from_disk_then_sweeps() {
    let mut a = fresh().await;
    a.handle_insert(
      "lib/root/0".into(),
      Bytes::from_static(b"grid-thumb"),
      Some("image/jpeg".into()),
      Retention::disk_ttl(60),
    )
    .await
    .unwrap();
    assert_eq!(a.memory_byte_total, 0);
    let got = a.handle_get("lib/root/0".into()).await.unwrap();
    assert_eq!(&got.bytes[..], b"grid-thumb");
    if let Some(e) = a.entries.get_mut("lib/root/0") {
      e.ttl_deadline = Some(Instant::now() - Duration::from_secs(1));
    }
    a.ttl_sweep().await;
    assert!(a.handle_get("lib/root/0".into()).await.is_none());
  }

  #[tokio::test]
  async fn disk_ttl_writes_no_row_so_restart_drops_it() {
    let blobs = temp_blobs();
    let db = crate::db::open(None).await.unwrap();
    let (events_tx, _) = broadcast::channel(16);
    let (_cmd_tx, cmd_rx) = mpsc::channel(16);
    let mut a = AssetActor::new(db.clone(), blobs.clone(), cmd_rx, events_tx)
      .bootstrap()
      .await
      .unwrap();
    a.handle_insert(
      "lib/ephemeral".into(),
      Bytes::from_static(b"ttl-bytes"),
      None,
      Retention::disk_ttl(600),
    )
    .await
    .unwrap();
    drop(a);

    let (events_tx2, _) = broadcast::channel(16);
    let (_cmd_tx2, cmd_rx2) = mpsc::channel(16);
    let mut a2 = AssetActor::new(db, blobs.clone(), cmd_rx2, events_tx2)
      .bootstrap()
      .await
      .unwrap();
    assert!(a2.handle_get("lib/ephemeral".into()).await.is_none());
    assert!(!blobs.join(safe_blob_name("lib/ephemeral")).exists());
  }

  #[tokio::test]
  async fn age_times_size_evicts_large_stale_before_small() {
    let mut a = fresh().await;
    let big = Bytes::from(vec![0u8; MEMORY_BUDGET_BYTES / 2]);
    a.handle_insert("big".into(), big, None, Retention::MEM_LRU)
      .await
      .unwrap();
    let small = Bytes::from(vec![1u8; 4 * 1024]);
    for i in 0..8 {
      a.handle_insert(format!("small/{i}"), small.clone(), None, Retention::MEM_LRU)
        .await
        .unwrap();
    }
    let big2 = Bytes::from(vec![2u8; MEMORY_BUDGET_BYTES / 2]);
    a.handle_insert("big2".into(), big2, None, Retention::MEM_LRU)
      .await
      .unwrap();
    assert!(a.handle_get("big".into()).await.is_none(), "stale large evicted");
    for i in 0..8 {
      assert!(
        a.handle_get(format!("small/{i}")).await.is_some(),
        "small thumb {i} survives"
      );
    }
  }

  #[tokio::test]
  async fn pinned_survives_lru_pressure() {
    let mut a = fresh().await;
    let big = Bytes::from(vec![0u8; MEMORY_BUDGET_BYTES / 2]);
    a.handle_insert("pin".into(), big.clone(), None, Retention::MEM_PINNED)
      .await
      .unwrap();
    a.handle_insert("a".into(), big.clone(), None, Retention::MEM_LRU)
      .await
      .unwrap();
    a.handle_insert("b".into(), big.clone(), None, Retention::MEM_LRU)
      .await
      .unwrap();
    assert!(a.handle_get("pin".into()).await.is_some());
    assert!(a.handle_get("a".into()).await.is_none());
  }

  #[tokio::test]
  async fn set_retention_pins_and_demotes_same_tier() {
    let mut a = fresh().await;
    a.handle_insert("head".into(), Bytes::from_static(b"art"), None, Retention::MEM_LRU)
      .await
      .unwrap();
    a.handle_set_retention("head".into(), Retention::MEM_PINNED)
      .await
      .unwrap();
    assert!(matches!(
      a.entries.get("head").unwrap().retention.lifetime,
      Lifetime::Pinned
    ));
    a.handle_set_retention("head".into(), Retention::MEM_LRU).await.unwrap();
    assert!(matches!(
      a.entries.get("head").unwrap().retention.lifetime,
      Lifetime::Lru
    ));
  }

  #[tokio::test]
  async fn set_retention_refuses_tier_change() {
    let mut a = fresh().await;
    a.handle_insert("m".into(), Bytes::from_static(b"x"), None, Retention::MEM_LRU)
      .await
      .unwrap();
    a.handle_set_retention("m".into(), Retention::DISK_PINNED)
      .await
      .unwrap();
    assert_eq!(a.entries.get("m").unwrap().tier(), Tier::Memory);
  }

  async fn fresh_with_events() -> (AssetActor, broadcast::Receiver<AssetCacheEvent>) {
    let db = crate::db::open(None).await.unwrap();
    let (events_tx, _) = broadcast::channel(16);
    let (_cmd_tx, cmd_rx) = mpsc::channel(16);
    let actor = AssetActor::new(db, temp_blobs(), cmd_rx, events_tx.clone())
      .bootstrap()
      .await
      .unwrap();
    (actor, events_tx.subscribe())
  }

  #[tokio::test]
  async fn empty_memory_insert_is_not_stored() {
    let mut a = fresh().await;
    a.handle_insert(
      "iap2/art/3bb6205e3a0018fe/134".into(),
      Bytes::new(),
      Some("image/jpeg".into()),
      Retention::MEM_LRU,
    )
    .await
    .unwrap();
    assert!(a.handle_get("iap2/art/3bb6205e3a0018fe/134".into()).await.is_none());
  }

  #[tokio::test]
  async fn empty_memory_insert_emits_no_ready() {
    let (mut a, mut events) = fresh_with_events().await;
    a.handle_insert("e/1".into(), Bytes::new(), None, Retention::MEM_LRU)
      .await
      .unwrap();
    assert!(matches!(events.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
  }

  #[tokio::test]
  async fn empty_insert_preserves_existing_good_asset() {
    let mut a = fresh().await;
    a.handle_insert("k".into(), Bytes::from_static(b"good"), None, Retention::MEM_LRU)
      .await
      .unwrap();
    a.handle_insert("k".into(), Bytes::new(), None, Retention::MEM_LRU)
      .await
      .unwrap();
    let got = a
      .handle_get("k".into())
      .await
      .expect("existing asset survives an empty insert");
    assert_eq!(&got.bytes[..], b"good");
  }

  #[tokio::test]
  async fn empty_insert_from_path_is_not_stored() {
    let mut a = fresh().await;
    let source = std::env::temp_dir().join(format!("empty-src-{}", uuid::Uuid::now_v7()));
    tokio::fs::write(&source, b"").await.unwrap();
    a.handle_insert_from_path("c/empty".into(), source, Some("image/jpeg".into()), Retention::MEM_LRU)
      .await
      .unwrap();
    assert!(a.handle_get("c/empty".into()).await.is_none());
  }

  #[tokio::test]
  async fn empty_persistent_insert_from_path_writes_no_blob() {
    let blobs = temp_blobs();
    let db = crate::db::open(None).await.unwrap();
    let (events_tx, _) = broadcast::channel(16);
    let (_cmd_tx, cmd_rx) = mpsc::channel(16);
    let mut a = AssetActor::new(db, blobs.clone(), cmd_rx, events_tx)
      .bootstrap()
      .await
      .unwrap();
    let source = std::env::temp_dir().join(format!("empty-persist-{}", uuid::Uuid::now_v7()));
    tokio::fs::write(&source, b"").await.unwrap();
    a.handle_insert_from_path("p/empty".into(), source, None, Retention::DISK_PINNED)
      .await
      .unwrap();
    assert!(a.handle_get("p/empty".into()).await.is_none());
    assert!(
      !blobs.join(safe_blob_name("p/empty")).exists(),
      "no persistent blob should be written for a 0-byte asset"
    );
  }

  #[tokio::test]
  async fn disk_lru_survives_restart_as_lru() {
    let blobs = temp_blobs();
    let db = crate::db::open(None).await.unwrap();
    let (events_tx, _) = broadcast::channel(16);
    let (_cmd_tx, cmd_rx) = mpsc::channel(16);
    let mut a = AssetActor::new(db.clone(), blobs.clone(), cmd_rx, events_tx)
      .bootstrap()
      .await
      .unwrap();
    a.handle_insert(
      "spotify/img/248/abc".into(),
      Bytes::from_static(b"pulled-art"),
      Some("image/jpeg".into()),
      Retention::DISK_LRU,
    )
    .await
    .unwrap();
    assert_eq!(a.memory_byte_total, 0, "disk-lru art holds no resident memory");
    drop(a);

    let (events_tx2, _) = broadcast::channel(16);
    let (_cmd_tx2, cmd_rx2) = mpsc::channel(16);
    let mut a2 = AssetActor::new(db, blobs, cmd_rx2, events_tx2)
      .bootstrap()
      .await
      .unwrap();
    let got = a2.handle_get("spotify/img/248/abc".into()).await.unwrap();
    assert_eq!(&got.bytes[..], b"pulled-art");
    assert!(
      matches!(
        a2.entries.get("spotify/img/248/abc").unwrap().retention.lifetime,
        Lifetime::Lru
      ),
      "reconstructed as lru, not pinned"
    );
  }

  #[tokio::test]
  async fn disk_lru_evicts_oldest_and_never_pinned() {
    let mut a = fresh().await;
    a.handle_insert(
      "preset/0".into(),
      Bytes::from_static(b"keep"),
      None,
      Retention::DISK_PINNED,
    )
    .await
    .unwrap();
    a.handle_insert("art/old".into(), Bytes::from_static(b"old"), None, Retention::DISK_LRU)
      .await
      .unwrap();
    a.handle_insert("art/new".into(), Bytes::from_static(b"new"), None, Retention::DISK_LRU)
      .await
      .unwrap();
    a.entries.get_mut("preset/0").unwrap().accessed_at = 100;
    a.entries.get_mut("art/old").unwrap().accessed_at = 10;
    a.entries.get_mut("art/new").unwrap().accessed_at = 50;
    assert_eq!(a.pick_disk_evictable_victim().as_deref(), Some("art/old"));
    a.evict_entry("art/old").await;
    a.entries.get_mut("preset/0").unwrap().accessed_at = 1;
    assert_eq!(a.pick_disk_evictable_victim().as_deref(), Some("art/new"));
  }

  const MB: u64 = 1024 * 1024;

  #[test]
  fn disk_over_limit_trips_on_budget_alone() {
    assert!(disk_over_limit(600 * MB, 4096 * MB, 512 * MB, 128 * MB));
  }

  #[test]
  fn disk_over_limit_trips_on_free_floor_alone() {
    assert!(disk_over_limit(10 * MB, 64 * MB, 512 * MB, 128 * MB));
  }

  #[test]
  fn disk_over_limit_satisfied_when_under_budget_and_above_floor() {
    assert!(!disk_over_limit(400 * MB, 256 * MB, 512 * MB, 128 * MB));
    assert!(!disk_over_limit(512 * MB, 128 * MB, 512 * MB, 128 * MB));
  }

  #[test]
  fn disk_over_limit_models_reserve_floor() {
    let headroom = 128 * MB;
    let need = 300 * MB;
    let reserve_floor = headroom + need;
    assert!(!disk_over_limit(50 * MB, 200 * MB, 512 * MB, headroom));
    assert!(disk_over_limit(50 * MB, 200 * MB, 512 * MB, reserve_floor));
  }

  #[tokio::test]
  async fn pinned_disk_victim_is_oldest_pinned_and_none_without_pins() {
    let mut a = fresh().await;
    assert!(a.pick_disk_pinned_victim().is_none());
    a.handle_insert("art/lru".into(), Bytes::from_static(b"lru"), None, Retention::DISK_LRU)
      .await
      .unwrap();
    assert!(a.pick_disk_pinned_victim().is_none());
    a.handle_insert(
      "preset/a".into(),
      Bytes::from_static(b"a"),
      None,
      Retention::DISK_PINNED,
    )
    .await
    .unwrap();
    a.handle_insert(
      "preset/b".into(),
      Bytes::from_static(b"b"),
      None,
      Retention::DISK_PINNED,
    )
    .await
    .unwrap();
    a.entries.get_mut("preset/a").unwrap().accessed_at = 5;
    a.entries.get_mut("preset/b").unwrap().accessed_at = 50;
    assert_eq!(a.pick_disk_pinned_victim().as_deref(), Some("preset/a"));
  }
}
