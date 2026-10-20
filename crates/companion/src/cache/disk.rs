use std::{
  collections::{BTreeMap, HashMap},
  fs, io,
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
  time::{Duration, SystemTime},
};

use sha2::{Digest, Sha256};

const MAGIC: &[u8; 4] = b"BTC1";
const TEMP_SUFFIX: &str = ".tmp";
const TORN_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct DiskCache {
  inner: Arc<Inner>,
}

struct Inner {
  root: PathBuf,
  budget: u64,
  index: Mutex<Index>,
}

#[derive(Default)]
struct Index {
  entries: HashMap<PathBuf, Slot>,
  recency: BTreeMap<u64, PathBuf>,
  held: u64,
  tick: u64,
}

struct Slot {
  bytes: u64,
  tick: u64,
}

impl Index {
  fn touch(&mut self, rel: &Path, bytes: u64) {
    self.tick += 1;
    let tick = self.tick;
    match self.entries.get_mut(rel) {
      Some(slot) => {
        self.recency.remove(&slot.tick);
        self.held = self.held - slot.bytes + bytes;
        slot.bytes = bytes;
        slot.tick = tick;
      }
      None => {
        self.held += bytes;
        self.entries.insert(rel.to_owned(), Slot { bytes, tick });
      }
    }
    self.recency.insert(tick, rel.to_owned());
  }

  fn forget(&mut self, rel: &Path) {
    if let Some(slot) = self.entries.remove(rel) {
      self.recency.remove(&slot.tick);
      self.held -= slot.bytes;
    }
  }

  fn oldest(&self) -> Option<PathBuf> {
    self.recency.values().next().cloned()
  }
}

fn scope_dir(scope: &str) -> String {
  scope
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
    .collect()
}

fn relative(scope: &str, key: &str) -> PathBuf {
  let digest = Sha256::digest(key.as_bytes());
  let mut name = String::with_capacity(digest.len() * 2);
  for byte in digest {
    name.push_str(&format!("{byte:02x}"));
  }
  Path::new(&scope_dir(scope)).join(name)
}

fn remove(path: &Path) {
  match fs::remove_file(path) {
    Ok(()) => {}
    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
    Err(error) => tracing::debug!(path = %path.display(), %error, "cache file did not delete"),
  }
}

impl DiskCache {
  pub fn new(root: impl Into<PathBuf>, budget: u64) -> Self {
    Self {
      inner: Arc::new(Inner {
        root: root.into(),
        budget,
        index: Mutex::new(Index::default()),
      }),
    }
  }

  pub fn open(root: impl Into<PathBuf>, budget: u64) -> Self {
    let cache = Self::new(root, budget);
    cache.scan();
    cache
  }

  pub fn root(&self) -> &Path {
    &self.inner.root
  }

  pub fn held_bytes(&self) -> u64 {
    self.inner.index.lock().unwrap().held
  }

  pub fn scan(&self) {
    let mut found = Vec::new();
    if let Ok(scopes) = fs::read_dir(&self.inner.root) {
      for scope in scopes.flatten() {
        let Ok(files) = fs::read_dir(scope.path()) else {
          continue;
        };
        for file in files.flatten() {
          let path = file.path();
          let Ok(meta) = file.metadata() else { continue };
          if !meta.is_file() {
            continue;
          }
          let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
          if path.to_string_lossy().ends_with(TEMP_SUFFIX) {
            if used.elapsed().is_ok_and(|age| age > TORN_AFTER) {
              remove(&path);
            }
            continue;
          }
          let Ok(rel) = path.strip_prefix(&self.inner.root) else {
            continue;
          };
          found.push((used, rel.to_owned(), meta.len()));
        }
      }
    }
    found.sort_by_key(|(used, _, _)| *used);
    let mut index = self.inner.index.lock().unwrap();
    let live = std::mem::take(&mut *index);
    for (_, rel, bytes) in found {
      if !live.entries.contains_key(&rel) {
        index.touch(&rel, bytes);
      }
    }
    for rel in live.recency.values() {
      index.touch(rel, live.entries[rel].bytes);
    }
    self.trim(&mut index);
  }

  fn trim(&self, index: &mut Index) {
    while index.held > self.inner.budget {
      let Some(rel) = index.oldest() else { break };
      index.forget(&rel);
      remove(&self.inner.root.join(&rel));
    }
  }

  pub fn load(&self, scope: &str, key: &str) -> Option<Vec<u8>> {
    let rel = relative(scope, key);
    let path = self.inner.root.join(&rel);
    let raw = match fs::read(&path) {
      Ok(raw) => raw,
      Err(_) => {
        self.inner.index.lock().unwrap().forget(&rel);
        return None;
      }
    };
    let Some(body) = raw.strip_prefix(MAGIC.as_slice()) else {
      self.inner.index.lock().unwrap().forget(&rel);
      remove(&path);
      return None;
    };
    let body = body.to_vec();
    if let Ok(file) = fs::File::options().write(true).open(&path) {
      let _ = file.set_modified(SystemTime::now());
    }
    self.inner.index.lock().unwrap().touch(&rel, raw.len() as u64);
    Some(body)
  }

  pub fn store(&self, scope: &str, key: &str, body: &[u8]) {
    let bytes = (MAGIC.len() + body.len()) as u64;
    if bytes > self.inner.budget {
      return;
    }
    let rel = relative(scope, key);
    let path = self.inner.root.join(&rel);
    let Some(dir) = path.parent() else { return };
    if !self.inner.root.exists() {
      *self.inner.index.lock().unwrap() = Index::default();
    }
    if let Err(error) = fs::create_dir_all(dir) {
      tracing::debug!(dir = %dir.display(), %error, "cache dir did not create");
      return;
    }
    let temp = dir.join(format!(
      "{}.{}{TEMP_SUFFIX}",
      rel.file_name().map(|name| name.to_string_lossy()).unwrap_or_default(),
      uuid::Uuid::now_v7().simple()
    ));
    let mut framed = Vec::with_capacity(bytes as usize);
    framed.extend_from_slice(MAGIC);
    framed.extend_from_slice(body);
    let written = fs::write(&temp, &framed).and_then(|()| fs::rename(&temp, &path));
    if let Err(error) = written {
      tracing::debug!(path = %path.display(), %error, "cache entry did not write");
      remove(&temp);
      return;
    }
    let mut index = self.inner.index.lock().unwrap();
    index.touch(&rel, bytes);
    self.trim(&mut index);
  }

  pub fn evict(&self, scope: &str, key: &str) {
    let rel = relative(scope, key);
    self.inner.index.lock().unwrap().forget(&rel);
    remove(&self.inner.root.join(rel));
  }

  pub fn purge(&self, scope: &str) {
    let dir = PathBuf::from(scope_dir(scope));
    {
      let mut index = self.inner.index.lock().unwrap();
      let doomed: Vec<PathBuf> = index
        .entries
        .keys()
        .filter(|rel| rel.starts_with(&dir))
        .cloned()
        .collect();
      for rel in doomed {
        index.forget(&rel);
      }
    }
    match fs::remove_dir_all(self.inner.root.join(&dir)) {
      Ok(()) => {}
      Err(error) if error.kind() == io::ErrorKind::NotFound => {}
      Err(error) => tracing::debug!(scope, %error, "cache scope did not purge"),
    }
  }

  pub async fn get(&self, scope: &str, key: &str) -> Option<Vec<u8>> {
    let cache = self.clone();
    let (scope, key) = (scope.to_owned(), key.to_owned());
    tokio::task::spawn_blocking(move || cache.load(&scope, &key))
      .await
      .ok()
      .flatten()
  }

  pub async fn put(&self, scope: &str, key: &str, body: Vec<u8>) {
    let cache = self.clone();
    let (scope, key) = (scope.to_owned(), key.to_owned());
    let _ = tokio::task::spawn_blocking(move || cache.store(&scope, &key, &body)).await;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn cache(budget: u64) -> (tempfile::TempDir, DiskCache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = DiskCache::open(dir.path().join("root"), budget);
    (dir, cache)
  }

  #[test]
  fn a_stored_entry_loads_back() {
    let (_dir, cache) = cache(1 << 20);
    cache.store("art", "https://i.scdn.co/image/ab", b"jpeg");
    assert_eq!(cache.load("art", "https://i.scdn.co/image/ab").unwrap(), b"jpeg");
    assert!(cache.load("art", "https://i.scdn.co/image/cd").is_none());
    assert!(cache.load("other", "https://i.scdn.co/image/ab").is_none());
  }

  #[test]
  fn the_least_recently_used_entry_goes_first_when_the_budget_overflows() {
    let (_dir, cache) = cache(3 * (4 + 100));
    cache.store("s", "a", &[1; 100]);
    cache.store("s", "b", &[2; 100]);
    cache.store("s", "c", &[3; 100]);
    assert!(cache.load("s", "a").is_some());

    cache.store("s", "d", &[4; 100]);

    assert!(cache.load("s", "b").is_none(), "b was the coldest");
    assert!(cache.load("s", "a").is_some());
    assert!(cache.load("s", "c").is_some());
    assert!(cache.load("s", "d").is_some());
    assert_eq!(cache.held_bytes(), 3 * 104);
  }

  #[test]
  fn an_entry_larger_than_the_budget_is_never_kept() {
    let (_dir, cache) = cache(64);
    cache.store("s", "big", &[0; 100]);
    assert!(cache.load("s", "big").is_none());
    assert_eq!(cache.held_bytes(), 0);
  }

  #[test]
  fn entries_and_their_recency_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    {
      let cache = DiskCache::open(&root, 1 << 20);
      cache.store("s", "old", &[1; 100]);
      std::thread::sleep(std::time::Duration::from_millis(20));
      cache.store("s", "new", &[2; 100]);
    }

    let reopened = DiskCache::open(&root, 2 * 104);
    assert_eq!(reopened.held_bytes(), 2 * 104);
    std::thread::sleep(std::time::Duration::from_millis(20));
    reopened.store("s", "newest", &[3; 100]);

    assert!(
      reopened.load("s", "old").is_none(),
      "the reopened index kept mtime order"
    );
    assert_eq!(reopened.load("s", "new").unwrap(), vec![2; 100]);
    assert_eq!(reopened.load("s", "newest").unwrap(), vec![3; 100]);
  }

  #[test]
  fn a_reopen_shrinks_to_a_smaller_budget_and_sweeps_torn_writes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    {
      let cache = DiskCache::open(&root, 1 << 20);
      cache.store("s", "a", &[1; 100]);
      cache.store("s", "b", &[2; 100]);
    }
    let torn = root.join("s").join("deadbeef.0123.tmp");
    fs::write(&torn, b"half").unwrap();
    fs::File::options()
      .write(true)
      .open(&torn)
      .unwrap()
      .set_modified(SystemTime::now() - Duration::from_secs(3600))
      .unwrap();
    let writing = root.join("s").join("cafef00d.4567.tmp");
    fs::write(&writing, b"in flight").unwrap();

    let reopened = DiskCache::open(&root, 104);

    assert_eq!(reopened.held_bytes(), 104);
    assert!(!torn.exists());
    assert!(writing.exists(), "a write that may still be landing is left alone");
  }

  #[test]
  fn a_late_scan_keeps_what_this_run_already_touched_as_the_hottest() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    {
      let cache = DiskCache::open(&root, 1 << 20);
      cache.store("s", "old-a", &[1; 100]);
      cache.store("s", "old-b", &[2; 100]);
    }

    let cache = DiskCache::new(&root, 2 * 104);
    assert_eq!(cache.load("s", "old-a").unwrap(), vec![1; 100]);
    cache.store("s", "new", &[3; 100]);
    cache.scan();

    assert_eq!(cache.held_bytes(), 2 * 104);
    assert!(
      cache.load("s", "old-b").is_none(),
      "the untouched entry was the coldest"
    );
    assert!(cache.load("s", "old-a").is_some());
    assert!(cache.load("s", "new").is_some());
  }

  #[test]
  fn a_file_the_os_deleted_is_a_miss_and_leaves_the_accounting_honest() {
    let (_dir, cache) = cache(1 << 20);
    cache.store("s", "a", b"one");
    cache.store("s", "b", b"two");
    fs::remove_file(cache.root().join(relative("s", "a"))).unwrap();

    assert!(cache.load("s", "a").is_none());
    assert_eq!(cache.held_bytes(), 4 + 3);
    assert_eq!(cache.load("s", "b").unwrap(), b"two");
  }

  #[test]
  fn the_whole_root_vanishing_mid_session_reads_as_misses_and_writes_recreate_it() {
    let (_dir, cache) = cache(1 << 20);
    cache.store("s", "a", b"one");
    cache.store("t", "b", b"two");
    fs::remove_dir_all(cache.root()).unwrap();

    assert!(cache.load("s", "a").is_none());
    assert!(cache.load("t", "b").is_none());

    cache.store("s", "c", b"three");
    assert_eq!(cache.load("s", "c").unwrap(), b"three");
    assert_eq!(cache.held_bytes(), 4 + 5);
  }

  #[test]
  fn a_vanished_root_resets_the_index_on_the_next_write() {
    let (_dir, cache) = cache(1 << 20);
    cache.store("s", "a", &[1; 100]);
    cache.store("s", "b", &[2; 100]);
    fs::remove_dir_all(cache.root()).unwrap();

    cache.store("s", "c", b"x");

    assert_eq!(
      cache.held_bytes(),
      5,
      "phantom entries from before the purge are not counted"
    );
  }

  #[test]
  fn a_scope_dir_vanishing_is_a_miss() {
    let (_dir, cache) = cache(1 << 20);
    cache.store("s", "a", b"one");
    fs::remove_dir_all(cache.root().join("s")).unwrap();

    assert!(cache.load("s", "a").is_none());
    cache.store("s", "a", b"again");
    assert_eq!(cache.load("s", "a").unwrap(), b"again");
  }

  #[test]
  fn a_corrupt_file_is_a_miss_and_is_removed() {
    let (_dir, cache) = cache(1 << 20);
    cache.store("s", "a", b"one");
    let path = cache.root().join(relative("s", "a"));
    fs::write(&path, b"garbage").unwrap();

    assert!(cache.load("s", "a").is_none());
    assert!(!path.exists());
  }

  #[test]
  fn purge_drops_one_scope_and_spares_the_rest() {
    let (_dir, cache) = cache(1 << 20);
    cache.store("spotify", "a", b"one");
    cache.store("apple-music", "a", b"two");

    cache.purge("spotify");

    assert!(cache.load("spotify", "a").is_none());
    assert_eq!(cache.load("apple-music", "a").unwrap(), b"two");
    assert_eq!(cache.held_bytes(), 4 + 3);
  }

  #[test]
  fn a_root_that_cannot_be_created_degrades_to_misses() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("file");
    fs::write(&blocker, b"not a dir").unwrap();
    let cache = DiskCache::open(blocker.join("root"), 1 << 20);

    cache.store("s", "a", b"one");

    assert!(cache.load("s", "a").is_none());
    assert_eq!(cache.held_bytes(), 0);
  }

  #[test]
  fn scope_names_stay_one_path_component() {
    assert_eq!(scope_dir("../etc"), "___etc");
    assert_eq!(scope_dir("apple-music"), "apple-music");
  }

  #[tokio::test]
  async fn the_async_arms_reach_the_same_store() {
    let (_dir, cache) = cache(1 << 20);
    cache.put("s", "a", b"one".to_vec()).await;
    assert_eq!(cache.get("s", "a").await.unwrap(), b"one");
    assert!(cache.get("s", "b").await.is_none());
  }
}
