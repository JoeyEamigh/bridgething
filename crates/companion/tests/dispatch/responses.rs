use std::{sync::Arc, time::Duration};

pub struct Responses {
  pub dir: tempfile::TempDir,
  pub online: Arc<std::sync::atomic::AtomicBool>,
  cache: bridgething_companion::cache::response::ResponseCache,
}

impl Responses {
  pub fn new() -> Self {
    let dir = tempfile::tempdir().unwrap();
    let online = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let cache = Self::over(dir.path(), online.clone());
    Self { dir, online, cache }
  }

  fn over(
    dir: &std::path::Path,
    online: Arc<std::sync::atomic::AtomicBool>,
  ) -> bridgething_companion::cache::response::ResponseCache {
    bridgething_companion::cache::response::ResponseCache::new(
      bridgething_companion::cache::disk::DiskCache::open(dir.join("responses"), 1 << 20),
      online,
      Duration::from_millis(200),
    )
  }

  pub fn cache(&self) -> bridgething_companion::cache::response::ResponseCache {
    self.cache.clone()
  }

  pub fn reopened(&self) -> bridgething_companion::cache::response::ResponseCache {
    Self::over(self.dir.path(), self.online.clone())
  }

  pub fn go_offline(&self) {
    self.online.store(false, std::sync::atomic::Ordering::SeqCst);
  }
}
