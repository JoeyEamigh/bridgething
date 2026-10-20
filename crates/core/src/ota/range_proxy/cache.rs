use std::{
  collections::{BTreeMap, HashMap, VecDeque},
  io,
  ops::Bound,
  os::unix::fs::FileExt,
  path::{Path, PathBuf},
  sync::{Arc, RwLock, RwLockReadGuard},
};

use libbridgething::{RangePart, RangeSpec};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};
use tokio_util::bytes::Bytes;

use crate::{asset::AssetCache, digest::sha256_hex};

const META_FILE: &str = "meta.json";
const HEADER_LEN: u64 = 8;
const MAX_RECORD_LEN: u32 = 8 * 1024 * 1024;
const RESERVE_STEP: u64 = 32 * 1024 * 1024;
const FAILURE_VALVE: u32 = 2;

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheMeta {
  update_id: String,
  write_failures: u32,
  assets: BTreeMap<String, AssetMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AssetMeta {
  total: u32,
  last_ranges: Vec<RangePart>,
}

pub(super) struct RangeCache {
  dir: PathBuf,
  update_id: String,
  assets: AssetCache,
  logs: Mutex<HashMap<String, Arc<AssetLog>>>,
  meta: Mutex<CacheMeta>,
}

impl RangeCache {
  pub(super) async fn open(dir: &Path, update_id: &str, assets: AssetCache) -> io::Result<Self> {
    tokio::fs::create_dir_all(dir).await?;
    let meta = match read_meta(dir).await {
      Some(meta) if meta.update_id == update_id => meta,
      found => {
        if let Some(stale) = &found {
          tracing::info!(stale = %stale.update_id, new = %update_id, "range cache: discarding another update's cache");
        }
        wipe(dir).await?;
        CacheMeta {
          update_id: update_id.to_string(),
          ..Default::default()
        }
      }
    };
    write_meta(dir, &meta).await?;
    Ok(Self {
      dir: dir.to_path_buf(),
      update_id: update_id.to_string(),
      assets,
      logs: Mutex::new(HashMap::new()),
      meta: Mutex::new(meta),
    })
  }

  pub(super) fn update_id(&self) -> &str {
    &self.update_id
  }

  pub(super) async fn asset_log(&self, asset: &str) -> io::Result<Arc<AssetLog>> {
    let mut logs = self.logs.lock().await;
    if let Some(log) = logs.get(asset) {
      return Ok(log.clone());
    }
    let log = AssetLog::open(self.dir.join(log_name(asset)), self.assets.clone()).await?;
    let log = Arc::new(log);
    logs.insert(asset.to_string(), log.clone());
    Ok(log)
  }

  pub(super) async fn store_ranges(&self, asset: String, last_ranges: Vec<RangePart>, total: u32) {
    let mut meta = self.meta.lock().await;
    meta.assets.insert(asset, AssetMeta { total, last_ranges });
    if let Err(err) = write_meta(&self.dir, &meta).await {
      tracing::warn!(?err, "range cache: could not persist remembered ranges");
    }
  }

  pub(super) async fn load_ranges(&self, asset: &str) -> Option<(Vec<RangePart>, u32)> {
    let meta = self.meta.lock().await;
    meta.assets.get(asset).map(|a| (a.last_ranges.clone(), a.total))
  }
}

pub(super) async fn clear(dir: &Path, update_id: &str) {
  let Some(meta) = read_meta(dir).await else {
    return;
  };
  if meta.update_id != update_id {
    return;
  }
  tracing::info!(%update_id, "range cache: clearing");
  if let Err(err) = wipe(dir).await {
    tracing::warn!(?err, "range cache: clear failed");
  }
}

pub(super) async fn note_write_failure(dir: &Path, update_id: &str) -> bool {
  let Some(mut meta) = read_meta(dir).await else {
    return false;
  };
  if meta.update_id != update_id {
    return false;
  }
  meta.write_failures += 1;
  if meta.write_failures >= FAILURE_VALVE {
    tracing::warn!(%update_id, failures = meta.write_failures, "range cache: failure valve tripped; clearing");
    if let Err(err) = wipe(dir).await {
      tracing::warn!(?err, "range cache: valve clear failed");
    }
    return true;
  }
  if let Err(err) = write_meta(dir, &meta).await {
    tracing::warn!(?err, "range cache: could not persist failure count");
  }
  false
}

fn log_name(asset: &str) -> String {
  format!("{}.log", &sha256_hex(asset.as_bytes())[..16])
}

async fn read_meta(dir: &Path) -> Option<CacheMeta> {
  let bytes = tokio::fs::read(dir.join(META_FILE)).await.ok()?;
  match serde_json::from_slice(&bytes) {
    Ok(meta) => Some(meta),
    Err(err) => {
      tracing::warn!(?err, "range cache: meta unreadable; treating the cache as absent");
      None
    }
  }
}

async fn write_meta(dir: &Path, meta: &CacheMeta) -> io::Result<()> {
  let bytes = serde_json::to_vec(meta).map_err(io::Error::other)?;
  tokio::fs::write(dir.join(META_FILE), bytes).await
}

async fn wipe(dir: &Path) -> io::Result<()> {
  if let Err(err) = tokio::fs::remove_dir_all(dir).await
    && err.kind() != io::ErrorKind::NotFound
  {
    return Err(err);
  }
  tokio::fs::create_dir_all(dir).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
  log_off: u64,
  len: u32,
}

#[derive(Debug, Default)]
pub(super) struct CacheIndex {
  spans: BTreeMap<u32, Span>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CacheSeg {
  Cached { log_off: u64, len: u32 },
  Missing { zck_start: u32, len: u32 },
}

impl CacheIndex {
  pub(super) fn insert(&mut self, zck_start: u32, log_off: u64, len: u32) {
    let start = zck_start as u64;
    let end = start + len as u64;
    if len == 0 || end > u32::MAX as u64 {
      return;
    }

    if let Some((&head_start, &head)) = self.spans.range(..=zck_start).next_back() {
      let head_end = head_start as u64 + head.len as u64;
      if head_end > start {
        self.spans.remove(&head_start);
        if (head_start as u64) < start {
          self.spans.insert(
            head_start,
            Span {
              log_off: head.log_off,
              len: (start - head_start as u64) as u32,
            },
          );
        }
        if head_end > end {
          self.spans.insert(
            end as u32,
            Span {
              log_off: head.log_off + (end - head_start as u64),
              len: (head_end - end) as u32,
            },
          );
        }
      }
    }

    let overlapping: Vec<u32> = self
      .spans
      .range((Bound::Included(zck_start), Bound::Excluded(end as u32)))
      .map(|(k, _)| *k)
      .collect();
    for key in overlapping {
      let span = self.spans.remove(&key).expect("key came from the map");
      let span_end = key as u64 + span.len as u64;
      if span_end > end {
        self.spans.insert(
          end as u32,
          Span {
            log_off: span.log_off + (end - key as u64),
            len: (span_end - end) as u32,
          },
        );
      }
    }

    self.spans.insert(zck_start, Span { log_off, len });
  }

  pub(super) fn segments(&self, zck_start: u32, len: u32) -> Vec<CacheSeg> {
    let mut out = Vec::new();
    let end = zck_start as u64 + len as u64;
    let mut pos = zck_start as u64;
    while pos < end {
      let hit = self
        .spans
        .range(..=(pos as u32))
        .next_back()
        .filter(|(k, s)| **k as u64 + s.len as u64 > pos);
      match hit {
        Some((k, span)) => {
          let within = pos - *k as u64;
          let take = (span.len as u64 - within).min(end - pos);
          out.push(CacheSeg::Cached {
            log_off: span.log_off + within,
            len: take as u32,
          });
          pos += take;
        }
        None => {
          let next = self
            .spans
            .range((Bound::Excluded(pos as u32), Bound::Unbounded))
            .next()
            .map(|(k, _)| *k as u64)
            .unwrap_or(end)
            .min(end);
          out.push(CacheSeg::Missing {
            zck_start: pos as u32,
            len: (next - pos) as u32,
          });
          pos = next;
        }
      }
    }
    out
  }

  #[cfg(test)]
  pub(super) fn cached_bytes(&self) -> u64 {
    self.spans.values().map(|s| s.len as u64).sum()
  }

  #[cfg(test)]
  pub(super) fn spans(&self) -> Vec<(u32, u64, u32)> {
    self.spans.iter().map(|(k, s)| (*k, s.log_off, s.len)).collect()
  }
}

#[derive(Debug)]
struct AppendState {
  end: u64,
  reserved_steps: u64,
}

#[derive(Debug)]
pub(super) struct AssetLog {
  file: Arc<std::fs::File>,
  append: Mutex<AppendState>,
  index: RwLock<CacheIndex>,
  assets: AssetCache,
}

impl AssetLog {
  async fn open(path: PathBuf, assets: AssetCache) -> io::Result<Self> {
    let (file, index, end) = tokio::task::spawn_blocking(move || scan(&path))
      .await
      .map_err(io::Error::other)??;
    Ok(Self {
      file: Arc::new(file),
      append: Mutex::new(AppendState {
        end,
        reserved_steps: end / RESERVE_STEP,
      }),
      index: RwLock::new(index),
      assets,
    })
  }

  pub(super) fn index(&self) -> RwLockReadGuard<'_, CacheIndex> {
    self.index.read().expect("range cache index lock poisoned")
  }

  pub(super) async fn append(&self, zck_start: u32, bytes: &[u8]) -> io::Result<u64> {
    let len = u32::try_from(bytes.len()).map_err(|_| io::Error::other("range record longer than u32"))?;
    if len == 0 || len > MAX_RECORD_LEN {
      return Err(io::Error::other("range record length out of bounds"));
    }
    let mut record = Vec::with_capacity(HEADER_LEN as usize + bytes.len());
    record.extend_from_slice(&zck_start.to_le_bytes());
    record.extend_from_slice(&len.to_le_bytes());
    record.extend_from_slice(bytes);

    let mut append = self.append.lock().await;
    let at = append.end;
    let file = self.file.clone();
    let total = record.len() as u64;
    tokio::task::spawn_blocking(move || file.write_all_at(&record, at))
      .await
      .map_err(io::Error::other)??;
    append.end = at + total;
    let steps = append.end / RESERVE_STEP;
    let grew = steps > append.reserved_steps;
    if grew {
      append.reserved_steps = steps;
    }
    drop(append);

    if grew {
      let assets = self.assets.clone();
      tokio::spawn(async move {
        if let Err(err) = assets.reserve_disk(RESERVE_STEP).await {
          tracing::warn!(?err, "range cache: reserve_disk failed as the log grew");
        }
      });
    }

    self
      .index
      .write()
      .expect("range cache index lock poisoned")
      .insert(zck_start, at + HEADER_LEN, len);
    Ok(at + HEADER_LEN)
  }

  pub(super) async fn read_at(&self, log_off: u64, len: usize) -> io::Result<Bytes> {
    let file = self.file.clone();
    tokio::task::spawn_blocking(move || {
      let mut buf = vec![0u8; len];
      file.read_exact_at(&mut buf, log_off)?;
      Ok(Bytes::from(buf))
    })
    .await
    .map_err(io::Error::other)?
  }
}

fn scan(path: &Path) -> io::Result<(std::fs::File, CacheIndex, u64)> {
  let file = std::fs::OpenOptions::new()
    .create(true)
    .read(true)
    .write(true)
    .truncate(false)
    .open(path)?;
  let size = file.metadata()?.len();
  let mut index = CacheIndex::default();
  let mut pos = 0u64;
  let mut header = [0u8; HEADER_LEN as usize];
  while pos + HEADER_LEN <= size {
    file.read_exact_at(&mut header, pos)?;
    let zck_start = u32::from_le_bytes(header[..4].try_into().expect("4 bytes"));
    let len = u32::from_le_bytes(header[4..].try_into().expect("4 bytes"));
    if len == 0 || len > MAX_RECORD_LEN || pos + HEADER_LEN + len as u64 > size {
      break;
    }
    index.insert(zck_start, pos + HEADER_LEN, len);
    pos += HEADER_LEN + len as u64;
  }
  if pos != size {
    tracing::info!(
      path = %path.display(),
      dropped = size - pos,
      "range cache: truncating a torn tail record"
    );
    file.set_len(pos)?;
  }
  Ok((file, index, pos))
}

#[derive(Debug, Clone)]
enum FetchState {
  Open { records: usize },
  Finished { records: usize },
  Failed { reason: String },
}

#[derive(Debug, Clone, Copy)]
struct Rec {
  log_off: u64,
  len: u32,
}

#[derive(Debug)]
pub(super) struct FetchWriter {
  log: Arc<AssetLog>,
  gaps: VecDeque<RangeSpec>,
  recs: Arc<RwLock<Vec<Rec>>>,
  state_tx: watch::Sender<FetchState>,
}

#[derive(Debug)]
pub(super) struct FetchReader {
  log: Arc<AssetLog>,
  recs: Arc<RwLock<Vec<Rec>>>,
  state_rx: watch::Receiver<FetchState>,
  idx: usize,
  within: u32,
}

pub(super) fn fetch_channel(log: Arc<AssetLog>, gaps: Vec<RangeSpec>) -> (FetchWriter, FetchReader) {
  let recs = Arc::new(RwLock::new(Vec::new()));
  let (state_tx, state_rx) = watch::channel(FetchState::Open { records: 0 });
  (
    FetchWriter {
      log: log.clone(),
      gaps: gaps.into(),
      recs: recs.clone(),
      state_tx,
    },
    FetchReader {
      log,
      recs,
      state_rx,
      idx: 0,
      within: 0,
    },
  )
}

impl FetchWriter {
  pub(super) async fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
    let mut rest = bytes;
    while !rest.is_empty() {
      let Some(gap) = self.gaps.front_mut() else {
        return Err(io::Error::other("companion bytes overshoot the planned gaps"));
      };
      let take = rest.len().min(gap.length as usize);
      let log_off = self.log.append(gap.start, &rest[..take]).await?;
      let records = {
        let mut recs = self.recs.write().expect("range fetch record lock poisoned");
        recs.push(Rec {
          log_off,
          len: take as u32,
        });
        recs.len()
      };
      gap.start += take as u32;
      gap.length -= take as u32;
      if gap.length == 0 {
        self.gaps.pop_front();
      }
      rest = &rest[take..];
      let _ = self.state_tx.send(FetchState::Open { records });
    }
    Ok(())
  }

  pub(super) fn finish(self) {
    let records = self.recs.read().expect("range fetch record lock poisoned").len();
    let _ = self.state_tx.send(FetchState::Finished { records });
  }

  pub(super) fn fail(self, reason: impl Into<String>) {
    let _ = self.state_tx.send(FetchState::Failed { reason: reason.into() });
  }
}

impl FetchReader {
  pub(super) async fn next(&mut self, max: usize) -> io::Result<Bytes> {
    loop {
      let state = self.state_rx.borrow_and_update().clone();
      let (records, finished) = match &state {
        FetchState::Open { records } => (*records, false),
        FetchState::Finished { records } => (*records, true),
        FetchState::Failed { reason } => return Err(io::Error::other(reason.clone())),
      };
      if self.idx < records {
        let rec = self.recs.read().expect("range fetch record lock poisoned")[self.idx];
        let take = ((rec.len - self.within) as usize).min(max).max(1);
        let bytes = self.log.read_at(rec.log_off + self.within as u64, take).await?;
        self.within += take as u32;
        if self.within == rec.len {
          self.idx += 1;
          self.within = 0;
        }
        return Ok(bytes);
      }
      if finished {
        return Err(io::Error::other("companion stream finished before requested bytes"));
      }
      if self.state_rx.changed().await.is_err() {
        return Err(io::Error::other("companion stream dropped without terminal state"));
      }
    }
  }
}

#[cfg(test)]
pub(super) mod tests {
  use std::time::Duration;

  use super::*;

  pub(crate) fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("bridgething-range-cache-test-{}", uuid::Uuid::now_v7()))
  }

  pub(crate) async fn assets() -> AssetCache {
    let db = crate::db::open(None).await.unwrap();
    let dir = temp_dir().join("assets");
    AssetCache::init(db, dir).await.unwrap().spawn().0
  }

  async fn open_cache(dir: &Path, update_id: &str) -> RangeCache {
    RangeCache::open(dir, update_id, assets().await).await.unwrap()
  }

  #[tokio::test]
  async fn append_reopen_rebuilds_the_index() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("payload.zck").await.unwrap();
    log.append(1000, b"hello").await.unwrap();
    log.append(4096, b"world!").await.unwrap();
    drop(cache);

    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("payload.zck").await.unwrap();
    assert_eq!(log.index().cached_bytes(), 11);
    let segs = log.index().segments(1000, 5);
    let CacheSeg::Cached { log_off, len } = segs[0] else {
      panic!("expected a cached segment, got {segs:?}");
    };
    assert_eq!(len, 5);
    assert_eq!(&log.read_at(log_off, 5).await.unwrap()[..], b"hello");
  }

  #[tokio::test]
  async fn torn_tail_record_is_dropped_and_the_rest_survives() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("a.zck").await.unwrap();
    log.append(0, b"keepme").await.unwrap();
    log.append(64, b"tornrecord").await.unwrap();
    drop(cache);

    let path = dir.join(log_name("a.zck"));
    let len = tokio::fs::metadata(&path).await.unwrap().len();
    let torn = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    torn.set_len(len - 4).unwrap();
    drop(torn);

    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("a.zck").await.unwrap();
    assert_eq!(log.index().cached_bytes(), 6, "only the intact record survives");
    assert_eq!(
      tokio::fs::metadata(&path).await.unwrap().len(),
      HEADER_LEN + 6,
      "the torn record is truncated off"
    );

    log.append(64, b"again").await.unwrap();
    let segs = log.index().segments(64, 5);
    let CacheSeg::Cached { log_off, .. } = segs[0] else {
      panic!("expected the re-appended record to be cached, got {segs:?}");
    };
    assert_eq!(&log.read_at(log_off, 5).await.unwrap()[..], b"again");
  }

  #[tokio::test]
  async fn a_different_update_id_wipes_the_cache() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    cache
      .asset_log("a.zck")
      .await
      .unwrap()
      .append(0, b"stale")
      .await
      .unwrap();
    cache
      .store_ranges("a.zck".into(), vec![RangePart { start: 0, length: 5 }], 900)
      .await;
    drop(cache);

    let cache = open_cache(&dir, "u2").await;
    assert!(cache.load_ranges("a.zck").await.is_none());
    assert_eq!(cache.asset_log("a.zck").await.unwrap().index().cached_bytes(), 0);
  }

  #[tokio::test]
  async fn remembered_ranges_survive_a_reopen() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    let parts = vec![
      RangePart { start: 10, length: 20 },
      RangePart {
        start: 900,
        length: 100,
      },
    ];
    cache.store_ranges("a.zck".into(), parts.clone(), 4096).await;
    drop(cache);

    let cache = open_cache(&dir, "u1").await;
    assert_eq!(cache.load_ranges("a.zck").await, Some((parts, 4096)));
  }

  #[tokio::test]
  async fn clear_only_touches_the_named_update() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    cache
      .asset_log("a.zck")
      .await
      .unwrap()
      .append(0, b"bytes")
      .await
      .unwrap();
    drop(cache);

    clear(&dir, "other").await;
    assert!(dir.join(META_FILE).exists(), "a foreign id must not clear the cache");

    clear(&dir, "u1").await;
    assert!(!dir.join(META_FILE).exists());
  }

  #[tokio::test]
  async fn two_write_failures_clear_the_cache() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    cache
      .asset_log("a.zck")
      .await
      .unwrap()
      .append(0, b"bytes")
      .await
      .unwrap();
    drop(cache);
    let log_path = dir.join(log_name("a.zck"));

    assert!(!note_write_failure(&dir, "u1").await);
    assert!(log_path.exists(), "one failure keeps the cache");

    assert!(note_write_failure(&dir, "u1").await);
    assert!(!log_path.exists(), "two consecutive failures clear it");
  }

  #[tokio::test]
  async fn overlapping_records_resolve_to_the_newest_bytes() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("a.zck").await.unwrap();
    log.append(0, b"aaaaaaaaaa").await.unwrap();
    log.append(4, b"bbbb").await.unwrap();

    let mut got = Vec::new();
    let segs = log.index().segments(0, 10);
    for seg in segs {
      let CacheSeg::Cached { log_off, len } = seg else {
        panic!("expected full coverage, got {seg:?}");
      };
      got.extend_from_slice(&log.read_at(log_off, len as usize).await.unwrap());
    }
    assert_eq!(&got[..], b"aaaabbbbaa");
  }

  #[tokio::test]
  async fn segments_split_around_a_hole() {
    let mut index = CacheIndex::default();
    index.insert(100, 8, 50);
    index.insert(200, 66, 50);
    assert_eq!(
      index.segments(100, 150),
      vec![
        CacheSeg::Cached { log_off: 8, len: 50 },
        CacheSeg::Missing {
          zck_start: 150,
          len: 50
        },
        CacheSeg::Cached { log_off: 66, len: 50 },
      ]
    );
  }

  #[tokio::test]
  async fn fetch_reader_tails_the_writer() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("a.zck").await.unwrap();
    let gaps = vec![RangeSpec { start: 0, length: 6 }, RangeSpec { start: 1000, length: 5 }];
    let (mut w, mut r) = fetch_channel(log.clone(), gaps);
    w.append(b"hello ").await.unwrap();
    assert_eq!(&r.next(1024).await.unwrap()[..], b"hello ");
    let bg = tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(50)).await;
      w.append(b"world").await.unwrap();
      w.finish();
    });
    assert_eq!(&r.next(1024).await.unwrap()[..], b"world");
    bg.await.unwrap();

    assert_eq!(
      log.index().segments(1000, 5),
      vec![CacheSeg::Cached { log_off: 22, len: 5 }],
      "fetched bytes land in the cache under their zck offsets"
    );
  }

  #[tokio::test]
  async fn fetch_writer_splits_fragments_across_gap_boundaries() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("a.zck").await.unwrap();
    let gaps = vec![RangeSpec { start: 0, length: 4 }, RangeSpec { start: 4096, length: 4 }];
    let (mut w, _r) = fetch_channel(log.clone(), gaps);
    w.append(b"abcdefgh").await.unwrap();

    let first = log.index().segments(0, 4);
    let second = log.index().segments(4096, 4);
    let CacheSeg::Cached { log_off: a, .. } = first[0] else {
      panic!("expected cached, got {first:?}")
    };
    let CacheSeg::Cached { log_off: b, .. } = second[0] else {
      panic!("expected cached, got {second:?}")
    };
    assert_eq!(&log.read_at(a, 4).await.unwrap()[..], b"abcd");
    assert_eq!(&log.read_at(b, 4).await.unwrap()[..], b"efgh");
  }

  #[tokio::test]
  async fn fail_propagates_to_a_pending_reader() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("a.zck").await.unwrap();
    let (w, mut r) = fetch_channel(log, vec![RangeSpec { start: 0, length: 4 }]);
    let bg = tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(50)).await;
      w.fail("companion gave up");
    });
    let err = r.next(16).await.unwrap_err();
    assert!(err.to_string().contains("companion gave up"));
    bg.await.unwrap();
  }

  #[tokio::test]
  async fn finish_short_of_the_read_position_errors() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("a.zck").await.unwrap();
    let (mut w, mut r) = fetch_channel(log, vec![RangeSpec { start: 0, length: 4 }]);
    w.append(b"xy").await.unwrap();
    w.finish();
    assert_eq!(&r.next(2).await.unwrap()[..], b"xy");
    assert!(r.next(1).await.is_err());
  }

  #[tokio::test]
  async fn dropped_writer_without_terminal_errors() {
    let dir = temp_dir();
    let cache = open_cache(&dir, "u1").await;
    let log = cache.asset_log("a.zck").await.unwrap();
    let (w, mut r) = fetch_channel(log, vec![RangeSpec { start: 0, length: 4 }]);
    drop(w);
    assert!(r.next(1).await.is_err());
  }
}
