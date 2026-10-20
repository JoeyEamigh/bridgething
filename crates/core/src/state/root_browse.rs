use std::{future::Future, num::NonZeroUsize};

use libbridgething::BrowseResult;
use tokio::time::Duration;

use super::cache::GenerationCache;

pub type RootBrowseShape = (Option<u32>, Option<u32>);

const MAX_ENTRIES: NonZeroUsize = NonZeroUsize::new(8).expect("nonzero cap");

#[derive(Debug)]
pub struct RootBrowseCache {
  inner: GenerationCache<RootBrowseShape, BrowseResult>,
}

impl Default for RootBrowseCache {
  fn default() -> Self {
    Self {
      inner: GenerationCache::new(MAX_ENTRIES),
    }
  }
}

impl RootBrowseCache {
  pub async fn get_or_fetch<F, Fut, E>(
    &self,
    shape: RootBrowseShape,
    generation: u64,
    ttl: Duration,
    fetch: F,
  ) -> Result<BrowseResult, E>
  where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<BrowseResult, E>>,
  {
    self.inner.get_or_fetch(shape, Some(generation), Some(ttl), fetch).await
  }

  pub async fn forget(&self, shape: RootBrowseShape) {
    self.inner.forget(&shape).await;
  }
}

#[cfg(test)]
mod tests {
  use std::sync::atomic::{AtomicU32, Ordering};

  use super::*;

  fn result() -> BrowseResult {
    BrowseResult {
      entries: Vec::new(),
      total: Some(7),
      has_more: false,
    }
  }

  const LONG_TTL: Duration = Duration::from_secs(3600);
  const FULL: RootBrowseShape = (None, None);
  const SLIM: RootBrowseShape = (Some(10), None);

  #[tokio::test]
  async fn same_generation_within_ttl_reuses_cache() {
    let cache = RootBrowseCache::default();
    let calls = AtomicU32::new(0);
    let fetch = || {
      cache.get_or_fetch(FULL, 0, LONG_TTL, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ()>(result())
      })
    };
    fetch().await.unwrap();
    fetch().await.unwrap();
    assert_eq!(
      calls.load(Ordering::SeqCst),
      1,
      "second call at the same generation within ttl reuses the cache"
    );
  }

  #[tokio::test]
  async fn forgetting_a_shape_refetches_only_that_shape() {
    let cache = RootBrowseCache::default();
    let calls = AtomicU32::new(0);
    let fetch = |shape| {
      cache.get_or_fetch(shape, 0, LONG_TTL, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ()>(result())
      })
    };
    fetch(FULL).await.unwrap();
    fetch(SLIM).await.unwrap();

    cache.forget(SLIM).await;
    fetch(FULL).await.unwrap();
    fetch(SLIM).await.unwrap();

    assert_eq!(
      calls.load(Ordering::SeqCst),
      3,
      "only the forgotten shape went back to the phone"
    );
  }

  #[tokio::test]
  async fn distinct_shapes_cache_independently() {
    let cache = RootBrowseCache::default();
    let calls = AtomicU32::new(0);
    let fetch = |shape| {
      cache.get_or_fetch(shape, 0, LONG_TTL, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ()>(result())
      })
    };
    fetch(FULL).await.unwrap();
    fetch(SLIM).await.unwrap();
    fetch(FULL).await.unwrap();
    fetch(SLIM).await.unwrap();
    assert_eq!(
      calls.load(Ordering::SeqCst),
      2,
      "each request shape fetches once and then hits its own entry"
    );
  }

  #[tokio::test]
  async fn new_generation_refetches() {
    let cache = RootBrowseCache::default();
    let calls = AtomicU32::new(0);
    cache
      .get_or_fetch(FULL, 0, LONG_TTL, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ()>(result())
      })
      .await
      .unwrap();
    cache
      .get_or_fetch(FULL, 1, LONG_TTL, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ()>(result())
      })
      .await
      .unwrap();
    assert_eq!(
      calls.load(Ordering::SeqCst),
      2,
      "a new generation invalidates and refetches"
    );
  }

  #[tokio::test]
  async fn new_generation_drops_every_shape() {
    let cache = RootBrowseCache::default();
    let calls = AtomicU32::new(0);
    let fetch = |shape, generation| {
      cache.get_or_fetch(shape, generation, LONG_TTL, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ()>(result())
      })
    };
    fetch(FULL, 0).await.unwrap();
    fetch(SLIM, 0).await.unwrap();
    fetch(FULL, 1).await.unwrap();
    fetch(SLIM, 1).await.unwrap();
    assert_eq!(
      calls.load(Ordering::SeqCst),
      4,
      "a generation bump invalidates stale entries of every shape"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_stalled_fetch_does_not_block_a_different_shape() {
    let cache = RootBrowseCache::default();
    let slow = cache.get_or_fetch(FULL, 0, LONG_TTL, || async {
      tokio::time::sleep(Duration::from_secs(30)).await;
      Ok::<_, ()>(result())
    });
    let fast = tokio::time::timeout(
      Duration::from_millis(50),
      cache.get_or_fetch(SLIM, 0, LONG_TTL, || async { Ok::<_, ()>(result()) }),
    );
    let (_, fast) = tokio::join!(slow, fast);
    assert!(
      fast.is_ok(),
      "a stalled companion fetch for one shape must not stall lookups of another"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn expired_ttl_refetches_same_generation() {
    let cache = RootBrowseCache::default();
    let calls = AtomicU32::new(0);
    let ttl = Duration::from_secs(60);
    let fetch = || {
      cache.get_or_fetch(FULL, 0, ttl, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ()>(result())
      })
    };
    fetch().await.unwrap();
    fetch().await.unwrap();
    assert_eq!(
      calls.load(Ordering::SeqCst),
      1,
      "within ttl, same generation, reuses cache"
    );

    tokio::time::advance(Duration::from_secs(61)).await;
    fetch().await.unwrap();
    assert_eq!(
      calls.load(Ordering::SeqCst),
      2,
      "past ttl at the same generation forces a refetch (recommendation/library drift backstop)"
    );
  }
}
