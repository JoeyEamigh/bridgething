use std::{
  collections::HashMap,
  hash::{DefaultHasher, Hash, Hasher},
  sync::{Arc, Mutex},
};

use bridgething_io::{HttpExecutor, HttpMethod, HttpRequest};

use crate::backend::ImageScaler;

const JPEG_QUALITY: f32 = 0.6;
const FETCH_TIMEOUT_MS: u32 = 6_000;
const CACHE_BYTES: usize = 16 << 20;

async fn downsample(scaler: Option<Arc<dyn ImageScaler>>, data: Vec<u8>, max_edge: u32) -> Option<Vec<u8>> {
  let scaler = scaler?;
  tokio::task::spawn_blocking(move || scaler.downsample_jpeg(data, max_edge, JPEG_QUALITY))
    .await
    .ok()
    .flatten()
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ArtKey {
  url: String,
  max_edge: Option<u32>,
}

#[derive(Default)]
struct Store {
  entries: HashMap<ArtKey, Vec<u8>>,
  order: Vec<ArtKey>,
  held: usize,
}

impl Store {
  fn get(&self, key: &ArtKey) -> Option<Vec<u8>> {
    self.entries.get(key).cloned()
  }

  fn put(&mut self, key: ArtKey, bytes: Vec<u8>) {
    if self.entries.contains_key(&key) {
      return;
    }
    self.held += bytes.len();
    self.entries.insert(key.clone(), bytes);
    self.order.push(key);
    while self.held > CACHE_BYTES && !self.order.is_empty() {
      let evicted = self.order.remove(0);
      if let Some(bytes) = self.entries.remove(&evicted) {
        self.held -= bytes.len();
      }
    }
  }
}

pub struct ArtCache {
  exec: HttpExecutor,
  scaler: Option<Arc<dyn ImageScaler>>,
  store: Mutex<Store>,
}

impl ArtCache {
  pub fn new(exec: HttpExecutor, scaler: Option<Arc<dyn ImageScaler>>) -> Self {
    Self {
      exec,
      scaler,
      store: Mutex::new(Store::default()),
    }
  }

  pub fn adopt(&self, bytes: Vec<u8>) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let key = format!("embedded:{:016x}", hasher.finish());
    self.store.lock().unwrap().put(
      ArtKey {
        url: key.clone(),
        max_edge: None,
      },
      bytes,
    );
    key
  }

  pub async fn master(&self, url: &str) -> Option<Vec<u8>> {
    let key = ArtKey {
      url: url.to_owned(),
      max_edge: None,
    };
    if let Some(hit) = self.store.lock().unwrap().get(&key) {
      return Some(hit);
    }
    let response = self
      .exec
      .execute(HttpRequest {
        method: HttpMethod::Get,
        url: url.to_owned(),
        headers: Vec::new(),
        body: Vec::new(),
        timeout_ms: FETCH_TIMEOUT_MS,
      })
      .await
      .ok()?;
    if !response.ok() {
      return None;
    }
    self.store.lock().unwrap().put(key, response.body.clone());
    Some(response.body)
  }

  pub async fn scaled(&self, url: &str, max_edge: u32) -> Option<Vec<u8>> {
    let key = ArtKey {
      url: url.to_owned(),
      max_edge: Some(max_edge),
    };
    if let Some(hit) = self.store.lock().unwrap().get(&key) {
      return Some(hit);
    }
    let master = self.master(url).await?;
    let scaled = downsample(self.scaler.clone(), master, max_edge).await?;
    self.store.lock().unwrap().put(key, scaled.clone());
    Some(scaled)
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
    fn downsample_jpeg(&self, bytes: Vec<u8>, max_edge: u32, _quality: f32) -> Option<Vec<u8>> {
      self.scales.fetch_add(1, Ordering::SeqCst);
      Some(format!("{}@{max_edge}", String::from_utf8_lossy(&bytes)).into_bytes())
    }
  }

  fn cache(origin: Arc<Origin>, scaler: Arc<Scaler>) -> ArtCache {
    ArtCache::new(HttpExecutor::new(origin), Some(scaler))
  }

  #[tokio::test]
  async fn a_repeated_scale_reuses_the_fetch_and_the_downsample() {
    let origin = Origin::serving();
    let scaler = Arc::new(Scaler::default());
    let cache = cache(origin.clone(), scaler.clone());

    let first = cache.scaled("https://art.test/one", 96).await.unwrap();
    let second = cache.scaled("https://art.test/one", 96).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(origin.fetches.load(Ordering::SeqCst), 1);
    assert_eq!(scaler.scales.load(Ordering::SeqCst), 1);
  }

  #[tokio::test]
  async fn a_second_edge_reuses_the_master_and_earns_its_own_downsample() {
    let origin = Origin::serving();
    let scaler = Arc::new(Scaler::default());
    let cache = cache(origin.clone(), scaler.clone());

    let thumb = cache.scaled("https://art.test/one", 96).await.unwrap();
    let hero = cache.scaled("https://art.test/one", 248).await.unwrap();

    assert_ne!(thumb, hero);
    assert_eq!(origin.fetches.load(Ordering::SeqCst), 1);
    assert_eq!(scaler.scales.load(Ordering::SeqCst), 2);
  }

  #[tokio::test]
  async fn a_warmed_master_spares_the_scaler_a_second_fetch() {
    let origin = Origin::serving();
    let scaler = Arc::new(Scaler::default());
    let cache = cache(origin.clone(), scaler.clone());

    assert_eq!(cache.master("https://art.test/one").await.unwrap(), b"master");
    cache.scaled("https://art.test/one", 96).await.unwrap();

    assert_eq!(origin.fetches.load(Ordering::SeqCst), 1);
  }

  #[tokio::test]
  async fn a_non_success_status_yields_nothing_and_is_not_cached() {
    let origin = Origin::serving();
    *origin.status.lock().unwrap() = 404;
    let scaler = Arc::new(Scaler::default());
    let cache = cache(origin.clone(), scaler.clone());

    assert!(cache.scaled("https://art.test/gone", 96).await.is_none());
    assert!(cache.master("https://art.test/gone").await.is_none());

    assert_eq!(origin.fetches.load(Ordering::SeqCst), 2);
    assert_eq!(scaler.scales.load(Ordering::SeqCst), 0);
  }

  #[tokio::test]
  async fn adopted_bytes_scale_without_ever_touching_the_origin() {
    let origin = Origin::serving();
    let scaler = Arc::new(Scaler::default());
    let cache = cache(origin.clone(), scaler.clone());

    let key = cache.adopt(b"embedded".to_vec());
    assert_eq!(key, cache.adopt(b"embedded".to_vec()), "the same bytes share a key");
    assert_ne!(key, cache.adopt(b"other".to_vec()));
    assert_eq!(cache.scaled(&key, 96).await.unwrap(), b"embedded@96");

    assert_eq!(origin.fetches.load(Ordering::SeqCst), 0);
    assert_eq!(scaler.scales.load(Ordering::SeqCst), 1);
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
