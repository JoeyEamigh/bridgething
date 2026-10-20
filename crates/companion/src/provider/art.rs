use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
};

use bridgething_io::{HttpExecutor, HttpMethod, HttpRequest};
use sha2::{Digest, Sha256};

use crate::{
  backend::{ImageScaler, ImageTarget, sniff_and_scale},
  cache::disk::DiskCache,
};

const JPEG_QUALITY: f32 = 0.6;
const FETCH_TIMEOUT_MS: u32 = 6_000;
const MASTERS: &str = "master";
const SCALED: &str = "scaled";

async fn downsample(scaler: Option<Arc<dyn ImageScaler>>, data: Vec<u8>, max_edge: u32) -> Option<Vec<u8>> {
  let scaler = scaler?;
  let target = ImageTarget::Thumbnail {
    max_edge,
    quality: JPEG_QUALITY,
  };
  tokio::task::spawn_blocking(move || sniff_and_scale(scaler.as_ref(), data, target))
    .await
    .ok()
    .flatten()
    .map(|scaled| scaled.bytes)
}

#[async_trait::async_trait]
pub trait ArtOrigin: Send + Sync {
  async fn fetch(&self, url: &str) -> Option<Vec<u8>>;
}

#[async_trait::async_trait]
impl ArtOrigin for HttpExecutor {
  async fn fetch(&self, url: &str) -> Option<Vec<u8>> {
    let response = self
      .execute(HttpRequest {
        method: HttpMethod::Get,
        url: url.to_owned(),
        headers: Vec::new(),
        body: Vec::new(),
        timeout_ms: FETCH_TIMEOUT_MS,
      })
      .await
      .ok()?;
    response.ok().then_some(response.body)
  }
}

pub struct ArtCache {
  store: DiskCache,
  scaler: Option<Arc<dyn ImageScaler>>,
  turns: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ArtCache {
  pub fn new(store: DiskCache, scaler: Option<Arc<dyn ImageScaler>>) -> Self {
    Self {
      store,
      scaler,
      turns: Mutex::new(HashMap::new()),
    }
  }

  pub fn adopt(&self, bytes: Vec<u8>) -> String {
    let digest = Sha256::digest(&bytes);
    let mut key = String::from("embedded:");
    for byte in &digest[..12] {
      key.push_str(&format!("{byte:02x}"));
    }
    self.store.store(MASTERS, &key, &bytes);
    key
  }

  fn turn(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
    self.turns.lock().unwrap().entry(key.to_owned()).or_default().clone()
  }

  fn release(&self, key: &str, turn: Arc<tokio::sync::Mutex<()>>) {
    let mut turns = self.turns.lock().unwrap();
    drop(turn);
    if turns.get(key).is_some_and(|held| Arc::strong_count(held) == 1) {
      turns.remove(key);
    }
  }

  pub async fn master(&self, origin: &dyn ArtOrigin, url: &str) -> Option<Vec<u8>> {
    let turn = self.turn(url);
    let found = {
      let _held = turn.lock().await;
      self.master_held(origin, url).await
    };
    self.release(url, turn);
    found
  }

  async fn master_held(&self, origin: &dyn ArtOrigin, url: &str) -> Option<Vec<u8>> {
    if let Some(hit) = self.store.get(MASTERS, url).await {
      return Some(hit);
    }
    let body = origin.fetch(url).await?;
    self.store.put(MASTERS, url, body.clone()).await;
    Some(body)
  }

  pub async fn scaled(&self, origin: &dyn ArtOrigin, url: &str, max_edge: u32) -> Option<Vec<u8>> {
    let key = format!("{max_edge} {url}");
    if let Some(hit) = self.store.get(SCALED, &key).await {
      return Some(hit);
    }
    let master = self.master(origin, url).await?;
    let turn = self.turn(&key);
    let found = {
      let _held = turn.lock().await;
      match self.store.get(SCALED, &key).await {
        Some(hit) => Some(hit),
        None => {
          let scaled = downsample(self.scaler.clone(), master, max_edge).await;
          if let Some(bytes) = &scaled {
            self.store.put(SCALED, &key, bytes.clone()).await;
          }
          scaled
        }
      }
    };
    self.release(&key, turn);
    found
  }
}

pub trait ArtResolver: Send + Sync {
  fn asset_id(&self, source: &str, max_edge: u32) -> Option<String>;
  fn url(&self, source: &str) -> String;
}

#[derive(Debug, Clone, Copy)]
pub struct ImageAssetCodec {
  pub namespace: &'static str,
  pub short_form: Option<(char, &'static str)>,
}

impl ArtResolver for ImageAssetCodec {
  fn asset_id(&self, source: &str, max_edge: u32) -> Option<String> {
    ImageAssetCodec::asset_id(self, source, max_edge)
  }

  fn url(&self, source: &str) -> String {
    source.to_owned()
  }
}

fn percent_encode(raw: &str) -> String {
  let mut out = String::with_capacity(raw.len());
  for byte in raw.bytes() {
    match byte {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
      other => out.push_str(&format!("%{other:02X}")),
    }
  }
  out
}

fn percent_decode(encoded: &str) -> Option<String> {
  let mut out = Vec::with_capacity(encoded.len());
  let bytes = encoded.as_bytes();
  let mut at = 0;
  while at < bytes.len() {
    if bytes[at] == b'%' {
      let hex = bytes.get(at + 1..at + 3)?;
      out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
      at += 3;
    } else {
      out.push(bytes[at]);
      at += 1;
    }
  }
  String::from_utf8(out).ok()
}

impl ImageAssetCodec {
  pub fn asset_id(&self, url: &str, max_edge: u32) -> Option<String> {
    if url.is_empty() {
      return None;
    }
    if let Some((tag, prefix)) = self.short_form
      && let Some(rest) = url.strip_prefix(prefix)
    {
      return Some(format!("{}{max_edge}/{tag}{rest}", self.namespace));
    }
    Some(format!("{}{max_edge}/u{}", self.namespace, percent_encode(url)))
  }

  pub fn parse(&self, id: &str) -> Option<(String, u32)> {
    let rest = id.strip_prefix(self.namespace)?;
    let (edge, tagged) = rest.split_once('/')?;
    let max_edge: u32 = edge.parse().ok()?;
    let mut chars = tagged.chars();
    let tag = chars.next()?;
    let body = chars.as_str();
    let url = match self.short_form {
      Some((short, prefix)) if tag == short => format!("{prefix}{body}"),
      _ if tag == 'u' => percent_decode(body)?,
      _ => return None,
    };
    Some((url, max_edge))
  }
}

#[cfg(test)]
mod tests {
  use std::sync::atomic::{AtomicUsize, Ordering};

  use bridgething_io::{HttpDownloadSink, HttpResponse, HttpSink, HttpTransport};

  use super::*;
  use crate::{backend::ScaledImage, media::MediaSniff};

  const CODEC: ImageAssetCodec = ImageAssetCodec {
    namespace: "spotify/img/",
    short_form: Some(('i', "https://i.scdn.co/image/")),
  };

  #[derive(Default)]
  struct Origin {
    fetches: AtomicUsize,
    status: Mutex<u16>,
  }

  impl Origin {
    fn serving() -> Arc<Self> {
      Arc::new(Self {
        fetches: AtomicUsize::new(0),
        status: Mutex::new(200),
      })
    }
  }

  impl HttpTransport for Origin {
    fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
      self.fetches.fetch_add(1, Ordering::SeqCst);
      sink.complete(HttpResponse {
        status: *self.status.lock().unwrap(),
        headers: Vec::new(),
        body: b"master".to_vec(),
      });
    }

    fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      sink.on_failed("the art suite never takes the streaming arm".into());
    }
  }

  #[derive(Default)]
  struct Scaler {
    scales: AtomicUsize,
  }

  impl ImageScaler for Scaler {
    fn scale(&self, bytes: Vec<u8>, _sniff: MediaSniff, target: ImageTarget) -> Option<ScaledImage> {
      let ImageTarget::Thumbnail { max_edge, quality } = target else {
        panic!("art asks for a thumbnail, got {target:?}");
      };
      assert_eq!(quality, JPEG_QUALITY);
      self.scales.fetch_add(1, Ordering::SeqCst);
      Some(ScaledImage {
        bytes: format!("{}@{max_edge}", String::from_utf8_lossy(&bytes)).into_bytes(),
        mime: "image/jpeg".into(),
        width: max_edge,
        height: max_edge,
      })
    }
  }

  struct Rig {
    dir: tempfile::TempDir,
    origin: Arc<Origin>,
    exec: HttpExecutor,
    scaler: Arc<Scaler>,
    cache: ArtCache,
  }

  impl Rig {
    fn new() -> Self {
      Self::with_budget(1 << 20)
    }

    fn with_budget(budget: u64) -> Self {
      let dir = tempfile::tempdir().unwrap();
      let origin = Origin::serving();
      let scaler = Arc::new(Scaler::default());
      let cache = ArtCache::new(DiskCache::open(dir.path().join("art"), budget), Some(scaler.clone()));
      Self {
        exec: HttpExecutor::new(origin.clone()),
        dir,
        origin,
        scaler,
        cache,
      }
    }

    fn reopen(&self) -> ArtCache {
      ArtCache::new(
        DiskCache::open(self.dir.path().join("art"), 1 << 20),
        Some(self.scaler.clone()),
      )
    }

    fn fetches(&self) -> usize {
      self.origin.fetches.load(Ordering::SeqCst)
    }

    fn scales(&self) -> usize {
      self.scaler.scales.load(Ordering::SeqCst)
    }
  }

  #[tokio::test]
  async fn a_repeated_scale_reuses_the_fetch_and_the_downsample() {
    let rig = Rig::new();

    let first = rig.cache.scaled(&rig.exec, "https://art.test/one", 96).await.unwrap();
    let second = rig.cache.scaled(&rig.exec, "https://art.test/one", 96).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(rig.fetches(), 1);
    assert_eq!(rig.scales(), 1);
  }

  #[tokio::test]
  async fn a_second_edge_reuses_the_master_and_earns_its_own_downsample() {
    let rig = Rig::new();

    let thumb = rig.cache.scaled(&rig.exec, "https://art.test/one", 96).await.unwrap();
    let hero = rig.cache.scaled(&rig.exec, "https://art.test/one", 248).await.unwrap();

    assert_ne!(thumb, hero);
    assert_eq!(rig.fetches(), 1);
    assert_eq!(rig.scales(), 2);
  }

  #[tokio::test]
  async fn a_warmed_master_spares_the_scaler_a_second_fetch() {
    let rig = Rig::new();

    assert_eq!(
      rig.cache.master(&rig.exec, "https://art.test/one").await.unwrap(),
      b"master"
    );
    rig.cache.scaled(&rig.exec, "https://art.test/one", 96).await.unwrap();

    assert_eq!(rig.fetches(), 1);
  }

  #[tokio::test]
  async fn a_non_success_status_yields_nothing_and_is_not_cached() {
    let rig = Rig::new();
    *rig.origin.status.lock().unwrap() = 404;

    assert!(rig.cache.scaled(&rig.exec, "https://art.test/gone", 96).await.is_none());
    assert!(rig.cache.master(&rig.exec, "https://art.test/gone").await.is_none());

    assert_eq!(rig.fetches(), 2);
    assert_eq!(rig.scales(), 0);
  }

  #[tokio::test]
  async fn adopted_bytes_scale_without_ever_touching_the_origin() {
    let rig = Rig::new();

    let key = rig.cache.adopt(b"embedded".to_vec());
    assert_eq!(key, rig.cache.adopt(b"embedded".to_vec()), "the same bytes share a key");
    assert_ne!(key, rig.cache.adopt(b"other".to_vec()));
    assert_eq!(rig.cache.scaled(&rig.exec, &key, 96).await.unwrap(), b"embedded@96");

    assert_eq!(rig.fetches(), 0);
    assert_eq!(rig.scales(), 1);
  }

  #[tokio::test]
  async fn art_survives_a_restart_without_a_refetch_or_a_rescale() {
    let rig = Rig::new();
    rig
      .cache
      .scaled(&rig.exec, "https://i.scdn.co/image/ab", 96)
      .await
      .unwrap();
    let embedded = rig.cache.adopt(b"embedded".to_vec());

    let reopened = rig.reopen();

    assert_eq!(
      reopened
        .scaled(&rig.exec, "https://i.scdn.co/image/ab", 96)
        .await
        .unwrap(),
      b"master@96"
    );
    assert_eq!(
      reopened.master(&rig.exec, "https://i.scdn.co/image/ab").await.unwrap(),
      b"master"
    );
    assert_eq!(
      reopened.adopt(b"embedded".to_vec()),
      embedded,
      "embedded keys are stable across runs"
    );
    assert_eq!(rig.fetches(), 1);
    assert_eq!(rig.scales(), 1);
  }

  #[tokio::test]
  async fn content_addressed_art_never_expires_and_only_leaves_by_size() {
    let entry = 4 + b"master".len() as u64;
    let rig = Rig::with_budget(2 * entry);
    rig.cache.master(&rig.exec, "https://art.test/a").await.unwrap();
    rig.cache.master(&rig.exec, "https://art.test/b").await.unwrap();
    for _ in 0..5 {
      rig.cache.master(&rig.exec, "https://art.test/a").await.unwrap();
    }
    assert_eq!(rig.fetches(), 2);

    rig.cache.master(&rig.exec, "https://art.test/c").await.unwrap();
    rig.cache.master(&rig.exec, "https://art.test/a").await.unwrap();
    assert_eq!(rig.fetches(), 3, "a was hot, so b paid for c");

    rig.cache.master(&rig.exec, "https://art.test/b").await.unwrap();
    assert_eq!(rig.fetches(), 4);
  }

  #[tokio::test]
  async fn art_the_os_purged_is_refetched_not_an_error() {
    let rig = Rig::new();
    rig.cache.scaled(&rig.exec, "https://art.test/one", 96).await.unwrap();
    std::fs::remove_dir_all(rig.dir.path().join("art")).unwrap();

    assert_eq!(
      rig.cache.scaled(&rig.exec, "https://art.test/one", 96).await.unwrap(),
      b"master@96"
    );
    assert_eq!(rig.fetches(), 2);
    assert_eq!(
      rig.cache.scaled(&rig.exec, "https://art.test/one", 96).await.unwrap(),
      b"master@96"
    );
    assert_eq!(rig.fetches(), 2);
  }

  #[tokio::test]
  async fn concurrent_asks_for_one_image_share_a_fetch_and_a_downsample() {
    let rig = Rig::new();

    let asks = (0..8).map(|_| rig.cache.scaled(&rig.exec, "https://art.test/one", 96));
    let served = futures::future::join_all(asks).await;

    assert!(
      served
        .iter()
        .all(|bytes| bytes.as_deref() == Some(b"master@96".as_slice()))
    );
    assert_eq!(rig.fetches(), 1);
    assert_eq!(rig.scales(), 1);
    assert!(rig.cache.turns.lock().unwrap().is_empty());
  }

  #[test]
  fn scdn_urls_take_the_short_form_and_round_trip() {
    let id = CODEC.asset_id("https://i.scdn.co/image/deadbeef", 248).unwrap();
    assert_eq!(id, "spotify/img/248/ideadbeef");
    assert_eq!(
      CODEC.parse(&id).unwrap(),
      ("https://i.scdn.co/image/deadbeef".into(), 248)
    );
  }

  #[test]
  fn foreign_urls_encode_whole_and_round_trip() {
    let url = "https://example.com/a b/c?x=1&y=Ü";
    let id = CODEC.asset_id(url, 96).unwrap();
    assert!(id.starts_with("spotify/img/96/u"));
    assert_eq!(CODEC.parse(&id).unwrap(), (url.to_string(), 96));
  }

  #[test]
  fn junk_ids_parse_to_nothing() {
    assert!(CODEC.parse("spotify/img/").is_none());
    assert!(CODEC.parse("spotify/img/abc/ideadbeef").is_none());
    assert!(CODEC.parse("spotify/img/248/xdeadbeef").is_none());
    assert!(CODEC.parse("other/img/248/ideadbeef").is_none());
    assert!(CODEC.parse("").is_none());
  }
}
