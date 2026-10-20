use std::{future::Future, hash::Hash, num::NonZeroUsize};

use lru::LruCache;
use tokio::{
  sync::Mutex,
  time::{Duration, Instant},
};

#[derive(Debug)]
pub struct GenerationCache<K: Hash + Eq, V> {
  inner: Mutex<LruCache<K, Entry<V>>>,
}

#[derive(Debug)]
struct Entry<V> {
  generation: Option<u64>,
  expires_at: Option<Instant>,
  value: V,
}

impl<V> Entry<V> {
  fn is_fresh(&self, generation: Option<u64>) -> bool {
    self.generation == generation && self.expires_at.is_none_or(|at| Instant::now() < at)
  }
}

impl<K: Hash + Eq, V: Clone> GenerationCache<K, V> {
  pub fn new(capacity: NonZeroUsize) -> Self {
    Self {
      inner: Mutex::new(LruCache::new(capacity)),
    }
  }

  pub async fn forget(&self, key: &K) {
    self.inner.lock().await.pop(key);
  }

  pub async fn get_or_fetch<F, Fut, E>(
    &self,
    key: K,
    generation: Option<u64>,
    ttl: Option<Duration>,
    fetch: F,
  ) -> Result<V, E>
  where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<V, E>>,
  {
    {
      let mut cache = self.inner.lock().await;
      if let Some(entry) = cache.get(&key)
        && entry.is_fresh(generation)
      {
        return Ok(entry.value.clone());
      }
    }

    let value = fetch().await?;
    self.inner.lock().await.put(
      key,
      Entry {
        generation,
        expires_at: ttl.map(|ttl| Instant::now() + ttl),
        value: value.clone(),
      },
    );
    Ok(value)
  }
}
