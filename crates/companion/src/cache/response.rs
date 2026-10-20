use std::{
  collections::HashMap,
  future::Future,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::Notify;

use super::disk::DiskCache;

pub const REVALIDATE_BUDGET: Duration = Duration::from_millis(1_000);

type Slot = (String, String);
type Refreshing = Arc<Mutex<HashMap<Slot, Arc<Refresh>>>>;

#[derive(Default)]
struct Refresh {
  stale_served: AtomicBool,
}

enum Handoff<T, E> {
  Waiting,
  Abandoned,
  Done(Result<T, E>),
}

#[derive(Clone)]
pub struct ResponseCache {
  store: DiskCache,
  online: Arc<AtomicBool>,
  budget: Duration,
  refreshing: Refreshing,
}

fn encode<T: Serialize>(value: &T) -> Option<Vec<u8>> {
  serde_json::to_vec(value).ok()
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
  serde_json::from_slice(bytes).ok()
}

impl ResponseCache {
  pub fn new(store: DiskCache, online: Arc<AtomicBool>, budget: Duration) -> Self {
    Self {
      store,
      online,
      budget,
      refreshing: Arc::new(Mutex::new(HashMap::new())),
    }
  }

  pub fn purge(&self, scope: &str) {
    self.store.purge(scope);
  }

  async fn cached<T: DeserializeOwned>(&self, scope: &str, key: &str) -> Option<(T, Vec<u8>)> {
    let bytes = self.store.get(scope, key).await?;
    match decode(&bytes) {
      Some(value) => Some((value, bytes)),
      None => {
        self.store.evict(scope, key);
        None
      }
    }
  }

  pub async fn revalidate<K, T, E, F, C>(&self, scope: &str, key: &K, fetch: F, changed: C) -> Result<T, E>
  where
    K: Serialize + ?Sized,
    T: Serialize + DeserializeOwned + Send + 'static,
    E: Send + 'static,
    F: Future<Output = Result<T, E>> + Send + 'static,
    C: Future<Output = ()> + Send + 'static,
  {
    let Some(key) = serde_json::to_string(key).ok() else {
      return fetch.await;
    };
    let Some((stale, stale_bytes)) = self.cached::<T>(scope, &key).await else {
      let value = fetch.await?;
      if let Some(bytes) = encode(&value) {
        self.store.put(scope, &key, bytes).await;
      }
      return Ok(value);
    };
    if !self.online.load(Ordering::SeqCst) {
      return Ok(stale);
    }
    let slot = (scope.to_owned(), key);
    let refresh = Arc::new(Refresh::default());
    {
      let mut refreshing = self.refreshing.lock().unwrap();
      if let Some(running) = refreshing.get(&slot) {
        running.stale_served.store(true, Ordering::SeqCst);
        return Ok(stale);
      }
      refreshing.insert(slot.clone(), refresh.clone());
    }
    let handoff = Arc::new((Mutex::new(Handoff::Waiting), Notify::new()));
    let store = self.store.clone();
    let refreshing = self.refreshing.clone();
    let landed = handoff.clone();
    tokio::spawn(async move {
      let fresh = fetch.await;
      let differs = match fresh.as_ref().ok().and_then(encode) {
        Some(bytes) => {
          let differs = bytes != stale_bytes;
          store.put(&slot.0, &slot.1, bytes).await;
          differs
        }
        None => false,
      };
      refreshing.lock().unwrap().remove(&slot);
      let abandoned = {
        let mut state = landed.0.lock().unwrap();
        match *state {
          Handoff::Waiting => {
            *state = Handoff::Done(fresh);
            false
          }
          _ => true,
        }
      };
      landed.1.notify_one();
      if differs && (abandoned || refresh.stale_served.load(Ordering::SeqCst)) {
        changed.await;
      }
    });
    let _ = tokio::time::timeout(self.budget, handoff.1.notified()).await;
    let outcome = std::mem::replace(&mut *handoff.0.lock().unwrap(), Handoff::Abandoned);
    match outcome {
      Handoff::Done(Ok(fresh)) => Ok(fresh),
      Handoff::Done(Err(_)) | Handoff::Waiting | Handoff::Abandoned => Ok(stale),
    }
  }

  pub async fn immutable<K, T, E, F>(&self, scope: &str, key: &K, fetch: F) -> Result<Option<T>, E>
  where
    K: Serialize + ?Sized,
    T: Serialize + DeserializeOwned,
    F: Future<Output = Result<Option<T>, E>>,
  {
    let Some(key) = serde_json::to_string(key).ok() else {
      return fetch.await;
    };
    if let Some((hit, _)) = self.cached(scope, &key).await {
      return Ok(Some(hit));
    }
    let Some(value) = fetch.await? else {
      return Ok(None);
    };
    if let Some(bytes) = encode(&value) {
      self.store.put(scope, &key, bytes).await;
    }
    Ok(Some(value))
  }
}

#[cfg(test)]
mod tests {
  use std::sync::atomic::AtomicUsize;

  use tokio::sync::oneshot;

  use super::*;

  struct Rig {
    _dir: tempfile::TempDir,
    online: Arc<AtomicBool>,
    cache: ResponseCache,
  }

  fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let online = Arc::new(AtomicBool::new(true));
    let cache = ResponseCache::new(
      DiskCache::open(dir.path().join("responses"), 1 << 20),
      online.clone(),
      Duration::from_millis(200),
    );
    Rig {
      _dir: dir,
      online,
      cache,
    }
  }

  async fn page(cache: &ResponseCache, value: Result<String, String>) -> Result<String, String> {
    cache
      .revalidate("spotify", "root", async move { value }, async {})
      .await
  }

  #[tokio::test]
  async fn a_first_fetch_is_stored_and_served_live() {
    let rig = rig();
    assert_eq!(page(&rig.cache, Ok("one".into())).await.unwrap(), "one");
    assert_eq!(page(&rig.cache, Ok("two".into())).await.unwrap(), "two");
  }

  #[tokio::test]
  async fn a_miss_with_a_failing_fetch_reports_the_failure() {
    let rig = rig();
    assert_eq!(page(&rig.cache, Err("offline".into())).await.unwrap_err(), "offline");
  }

  #[tokio::test]
  async fn a_failed_refresh_serves_the_stale_copy() {
    let rig = rig();
    page(&rig.cache, Ok("seen".into())).await.unwrap();
    assert_eq!(page(&rig.cache, Err("no route".into())).await.unwrap(), "seen");
  }

  #[tokio::test]
  async fn offline_serves_the_cache_without_touching_the_network() {
    let rig = rig();
    page(&rig.cache, Ok("seen".into())).await.unwrap();
    rig.online.store(false, Ordering::SeqCst);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();

    let served = rig
      .cache
      .revalidate(
        "spotify",
        "root",
        async move {
          counted.fetch_add(1, Ordering::SeqCst);
          Ok::<_, String>("fresh".to_string())
        },
        async {},
      )
      .await
      .unwrap();

    assert_eq!(served, "seen");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
  }

  #[tokio::test]
  async fn a_slow_network_serves_stale_and_the_refresh_lands_for_next_time() {
    let rig = rig();
    page(&rig.cache, Ok("old".into())).await.unwrap();
    let (release, gate) = oneshot::channel::<()>();

    let served = rig
      .cache
      .revalidate(
        "spotify",
        "root",
        async move {
          let _ = gate.await;
          Ok::<_, String>("new".to_string())
        },
        async {},
      )
      .await
      .unwrap();
    assert_eq!(served, "old");

    release.send(()).unwrap();
    for _ in 0..100 {
      if rig.cache.refreshing.lock().unwrap().is_empty() {
        break;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    rig.online.store(false, Ordering::SeqCst);
    assert_eq!(page(&rig.cache, Ok("unused".into())).await.unwrap(), "new");
  }

  #[tokio::test]
  async fn one_refresh_runs_per_key_while_the_network_is_slow() {
    let rig = rig();
    page(&rig.cache, Ok("old".into())).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (release, gate) = oneshot::channel::<()>();
    let counted = calls.clone();
    rig
      .cache
      .revalidate(
        "spotify",
        "root",
        async move {
          counted.fetch_add(1, Ordering::SeqCst);
          let _ = gate.await;
          Ok::<_, String>("new".to_string())
        },
        async {},
      )
      .await
      .unwrap();

    let counted = calls.clone();
    let second = rig
      .cache
      .revalidate(
        "spotify",
        "root",
        async move {
          counted.fetch_add(1, Ordering::SeqCst);
          Ok::<_, String>("racing".to_string())
        },
        async {},
      )
      .await
      .unwrap();

    assert_eq!(second, "old");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(release);
  }

  fn announcer() -> (
    Arc<AtomicUsize>,
    impl Fn() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>>,
  ) {
    let fired = Arc::new(AtomicUsize::new(0));
    let counted = fired.clone();
    (fired, move || {
      let counted = counted.clone();
      Box::pin(async move {
        counted.fetch_add(1, Ordering::SeqCst);
      })
    })
  }

  async fn settle(cache: &ResponseCache) {
    for _ in 0..100 {
      if cache.refreshing.lock().unwrap().is_empty() {
        break;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
  }

  async fn late_refresh(rig: &Rig, fresh: &str) -> usize {
    page(&rig.cache, Ok("old".into())).await.unwrap();
    let (fired, announce) = announcer();
    let (release, gate) = oneshot::channel::<()>();
    let fresh = fresh.to_owned();
    let served = rig
      .cache
      .revalidate(
        "spotify",
        "root",
        async move {
          let _ = gate.await;
          Ok::<_, String>(fresh)
        },
        announce(),
      )
      .await
      .unwrap();
    assert_eq!(served, "old");
    release.send(()).unwrap();
    settle(&rig.cache).await;
    fired.load(Ordering::SeqCst)
  }

  #[tokio::test]
  async fn a_late_refresh_that_differs_from_what_was_served_announces_the_change() {
    assert_eq!(late_refresh(&rig(), "new").await, 1);
  }

  #[tokio::test]
  async fn a_late_refresh_with_identical_bytes_stays_silent() {
    assert_eq!(late_refresh(&rig(), "old").await, 0);
  }

  #[tokio::test]
  async fn a_refresh_that_lands_inside_the_budget_is_served_and_never_announced() {
    let rig = rig();
    page(&rig.cache, Ok("old".into())).await.unwrap();
    let (fired, announce) = announcer();

    let served = rig
      .cache
      .revalidate(
        "spotify",
        "root",
        async { Ok::<_, String>("new".to_string()) },
        announce(),
      )
      .await
      .unwrap();
    settle(&rig.cache).await;

    assert_eq!(served, "new");
    assert_eq!(fired.load(Ordering::SeqCst), 0);
  }

  #[tokio::test]
  async fn a_failed_late_refresh_stays_silent() {
    let rig = rig();
    page(&rig.cache, Ok("old".into())).await.unwrap();
    let (fired, announce) = announcer();
    let (release, gate) = oneshot::channel::<()>();
    rig
      .cache
      .revalidate(
        "spotify",
        "root",
        async move {
          let _ = gate.await;
          Err::<String, _>("no route".to_string())
        },
        announce(),
      )
      .await
      .unwrap();
    release.send(()).unwrap();
    settle(&rig.cache).await;

    assert_eq!(fired.load(Ordering::SeqCst), 0);
  }

  #[tokio::test]
  async fn a_caller_served_stale_while_another_waited_for_the_refresh_still_hears_the_change() {
    let rig = rig();
    page(&rig.cache, Ok("old".into())).await.unwrap();
    let (fired, announce) = announcer();
    let (release, gate) = oneshot::channel::<()>();
    let first_cache = rig.cache.clone();
    let first = tokio::spawn(async move {
      first_cache
        .revalidate(
          "spotify",
          "root",
          async move {
            let _ = gate.await;
            Ok::<_, String>("new".to_string())
          },
          announce(),
        )
        .await
    });
    for _ in 0..100 {
      if !rig.cache.refreshing.lock().unwrap().is_empty() {
        break;
      }
      tokio::time::sleep(Duration::from_millis(1)).await;
    }

    let second = page(&rig.cache, Ok("unused".into())).await.unwrap();
    release.send(()).unwrap();

    assert_eq!(second, "old");
    assert_eq!(first.await.unwrap().unwrap(), "new");
    settle(&rig.cache).await;
    assert_eq!(fired.load(Ordering::SeqCst), 1);
  }

  #[tokio::test]
  async fn scopes_and_keys_do_not_bleed() {
    let rig = rig();
    page(&rig.cache, Ok("spotify root".into())).await.unwrap();
    rig.online.store(false, Ordering::SeqCst);

    let other = rig
      .cache
      .revalidate(
        "apple-music",
        "root",
        async { Err::<String, _>("offline".to_string()) },
        async {},
      )
      .await;
    assert_eq!(other.unwrap_err(), "offline");

    rig.cache.purge("spotify");
    assert_eq!(page(&rig.cache, Err("offline".into())).await.unwrap_err(), "offline");
  }

  #[tokio::test]
  async fn an_undecodable_entry_is_a_miss() {
    let rig = rig();
    rig.cache.store.store("spotify", "\"root\"", b"not json");
    assert_eq!(page(&rig.cache, Ok("live".into())).await.unwrap(), "live");
  }

  #[tokio::test]
  async fn immutable_entries_never_refetch() {
    let rig = rig();
    let calls = Arc::new(AtomicUsize::new(0));
    for _ in 0..3 {
      let counted = calls.clone();
      let lyrics = rig
        .cache
        .immutable("lyrics", "track", async move {
          counted.fetch_add(1, Ordering::SeqCst);
          Ok::<_, String>(Some("la la".to_string()))
        })
        .await;
      assert_eq!(lyrics.unwrap().unwrap(), "la la");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
  }

  #[tokio::test]
  async fn an_immutable_miss_or_failure_is_not_remembered() {
    let rig = rig();
    let missed = rig
      .cache
      .immutable("lyrics", "track", async { Ok::<Option<String>, String>(None) })
      .await;
    assert!(missed.unwrap().is_none());
    let failed = rig
      .cache
      .immutable("lyrics", "track", async {
        Err::<Option<String>, _>("offline".to_string())
      })
      .await;
    assert_eq!(failed.unwrap_err(), "offline");
    let found = rig
      .cache
      .immutable("lyrics", "track", async { Ok::<_, String>(Some("found".to_string())) })
      .await;
    assert_eq!(found.unwrap().unwrap(), "found");
  }

  #[tokio::test]
  async fn a_reopened_store_serves_what_the_last_run_saw() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("responses");
    let first = ResponseCache::new(
      DiskCache::open(&root, 1 << 20),
      Arc::new(AtomicBool::new(true)),
      Duration::from_millis(200),
    );
    page(&first, Ok("before restart".into())).await.unwrap();
    drop(first);

    let second = ResponseCache::new(
      DiskCache::open(&root, 1 << 20),
      Arc::new(AtomicBool::new(false)),
      Duration::from_millis(200),
    );
    assert_eq!(page(&second, Err("offline".into())).await.unwrap(), "before restart");
  }
}
