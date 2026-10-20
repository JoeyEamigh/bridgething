use std::sync::Arc;

pub struct Art {
  _dir: tempfile::TempDir,
  cache: Arc<bridgething_companion::provider::art::ArtCache>,
}

impl Art {
  pub fn new(scaler: Option<Arc<dyn bridgething_companion::backend::ImageScaler>>) -> Self {
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(bridgething_companion::provider::art::ArtCache::new(
      bridgething_companion::cache::disk::DiskCache::open(dir.path().join("art"), 1 << 20),
      scaler,
    ));
    Self { _dir: dir, cache }
  }

  pub fn cache(&self) -> Arc<bridgething_companion::provider::art::ArtCache> {
    self.cache.clone()
  }
}
