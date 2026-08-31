use std::{
  path::{Path, PathBuf},
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
};

use bridgething_delivery::bundle::{
  ArtifactDigest, BundleManifest,
  fetch::{
    ArtifactFetch, DigestField, DownloadRequest, FetchError, HttpArtifactFetch, MEMORY_BODY_CAP, fetch_json,
    sha256_file, sha256_hex,
  },
};
use bridgething_io::{HttpDownloadSink, HttpExecutor, HttpRequest, HttpSink, HttpTransport};
use tempfile::TempDir;

const URL: &str = "https://ota.example/artifact.bin";
const CHUNK: usize = 64 * 1024;

struct StubServer {
  status: u16,
  payload: Vec<u8>,
  calls: AtomicUsize,
  urls: Mutex<Vec<String>>,
}

impl StubServer {
  fn serving(payload: Vec<u8>) -> Arc<Self> {
    Arc::new(StubServer {
      status: 200,
      payload,
      calls: AtomicUsize::new(0),
      urls: Mutex::new(Vec::new()),
    })
  }

  fn refusing(status: u16) -> Arc<Self> {
    Arc::new(StubServer {
      status,
      payload: b"<html>nope</html>".to_vec(),
      calls: AtomicUsize::new(0),
      urls: Mutex::new(Vec::new()),
    })
  }

  fn calls(&self) -> usize {
    self.calls.load(Ordering::SeqCst)
  }
}

impl HttpTransport for StubServer {
  fn execute(&self, _request: HttpRequest, _sink: Arc<HttpSink>) {
    unreachable!("the fetch seam streams every body so nothing large is ever resident");
  }

  fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    self.calls.fetch_add(1, Ordering::SeqCst);
    self.urls.lock().unwrap().push(request.url);
    sink.on_response(self.status, Vec::new(), Some(self.payload.len() as u64));
    for chunk in self.payload.chunks(CHUNK) {
      sink.on_chunk(chunk.to_vec());
    }
    sink.on_finished();
  }
}

struct Unreachable;

impl HttpTransport for Unreachable {
  fn execute(&self, _request: HttpRequest, _sink: Arc<HttpSink>) {
    unreachable!("the fetch seam streams every body so nothing large is ever resident");
  }

  fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    sink.on_failed("no route to host".to_string());
  }
}

fn fetcher(transport: Arc<dyn HttpTransport>) -> HttpArtifactFetch {
  HttpArtifactFetch::new(HttpExecutor::new(transport))
}

fn payload(len: usize) -> Vec<u8> {
  (0..len).map(|i| (i % 251) as u8).collect()
}

fn digest(bytes: &[u8]) -> ArtifactDigest {
  ArtifactDigest {
    size: bytes.len() as u64,
    sha256: sha256_hex(bytes),
  }
}

fn download_of(dir: &Path, expected: Option<ArtifactDigest>) -> DownloadRequest {
  DownloadRequest {
    url: URL.to_string(),
    dir: dir.to_path_buf(),
    filename: "artifact".to_string(),
    asset: "test".to_string(),
    expected,
    progress: None,
  }
}

fn watching(sink: impl Fn(u64, u64) + Send + Sync + 'static) -> Arc<dyn Fn(u64, u64) + Send + Sync> {
  Arc::new(sink)
}

fn entries(dir: &Path) -> Vec<String> {
  let mut names: Vec<String> = std::fs::read_dir(dir)
    .expect("the download directory exists")
    .map(|e| e.expect("a readable entry").file_name().to_string_lossy().into_owned())
    .collect();
  names.sort();
  names
}

#[tokio::test]
async fn a_verified_artifact_lands_under_its_content_address() {
  let scratch = TempDir::new().unwrap();
  let bytes = payload(4096);
  let expected = digest(&bytes);

  let landed = fetcher(StubServer::serving(bytes.clone()))
    .download(download_of(scratch.path(), Some(expected.clone())))
    .await
    .unwrap();

  assert_eq!(
    landed.file_name().unwrap(),
    format!("artifact-{}", expected.sha256).as_str()
  );
  assert_eq!(std::fs::read(&landed).unwrap(), bytes);
}

#[tokio::test]
async fn a_body_that_runs_past_the_declared_size_is_cut_off_mid_stream() {
  let scratch = TempDir::new().unwrap();
  let flood = payload(4 * CHUNK);
  let budget = (CHUNK + CHUNK / 2) as u64;
  let ticks: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
  let sink = ticks.clone();

  let mut request = download_of(
    scratch.path(),
    Some(ArtifactDigest {
      size: budget,
      sha256: sha256_hex(&flood),
    }),
  );
  request.progress = Some(watching(move |received, total| {
    sink.lock().unwrap().push((received, total));
  }));

  let err = fetcher(StubServer::serving(flood)).download(request).await.unwrap_err();

  assert!(
    matches!(&err, FetchError::Io(reason) if reason.contains("ran past the declared")),
    "the write is refused as the body streams, not after it lands: {err:?}"
  );
  let seen = ticks.lock().unwrap();
  assert_eq!(
    seen.iter().map(|(received, _)| *received).max(),
    Some(CHUNK as u64),
    "one chunk landed and the next was refused, so the disk never held more than the budget"
  );
  assert!(
    entries(scratch.path()).is_empty(),
    "and the oversized staging file is not left on disk"
  );
}

#[tokio::test]
async fn a_body_that_stops_short_of_the_declared_size_still_fails_on_size() {
  let scratch = TempDir::new().unwrap();
  let short = payload(2048);
  let mut claimed = digest(&short);
  claimed.size = 4096;

  let err = fetcher(StubServer::serving(short))
    .download(download_of(scratch.path(), Some(claimed)))
    .await
    .unwrap_err();

  assert!(matches!(
    err,
    FetchError::DigestMismatch {
      field: DigestField::Size,
      ..
    }
  ));
}

#[tokio::test]
async fn a_sha_mismatch_refuses_to_land_the_artifact() {
  let scratch = TempDir::new().unwrap();
  let bytes = payload(4096);
  let lying = ArtifactDigest {
    size: bytes.len() as u64,
    sha256: sha256_hex(&[0u8; 8]),
  };

  let err = fetcher(StubServer::serving(bytes))
    .download(download_of(scratch.path(), Some(lying)))
    .await
    .unwrap_err();

  assert!(matches!(
    err,
    FetchError::DigestMismatch {
      field: DigestField::Sha256,
      ..
    }
  ));
  assert_eq!(
    err.to_string(),
    "test sha256 does not match the manifest; refusing to install"
  );
  assert!(
    entries(scratch.path()).is_empty(),
    "a rejected download leaves nothing behind, not even the staging file"
  );
}

#[tokio::test]
async fn a_size_mismatch_refuses_to_land_the_artifact() {
  let scratch = TempDir::new().unwrap();
  let bytes = payload(4096);
  let lying = ArtifactDigest {
    size: bytes.len() as u64 + 1,
    sha256: sha256_hex(&bytes),
  };

  let err = fetcher(StubServer::serving(bytes))
    .download(download_of(scratch.path(), Some(lying)))
    .await
    .unwrap_err();

  assert!(matches!(
    err,
    FetchError::DigestMismatch {
      field: DigestField::Size,
      ..
    }
  ));
  assert_eq!(
    err.to_string(),
    "test size does not match the manifest; refusing to install"
  );
  assert!(entries(scratch.path()).is_empty());
}

#[tokio::test]
async fn a_cached_artifact_is_not_fetched_again() {
  let scratch = TempDir::new().unwrap();
  let bytes = payload(4096);
  let expected = digest(&bytes);
  let server = StubServer::serving(bytes);
  let fetch = fetcher(server.clone());

  fetch
    .download(download_of(scratch.path(), Some(expected.clone())))
    .await
    .unwrap();
  fetch
    .download(download_of(scratch.path(), Some(expected)))
    .await
    .unwrap();

  assert_eq!(server.calls(), 1);
}

#[tokio::test]
async fn a_cache_entry_of_the_wrong_size_is_discarded_and_refetched() {
  let scratch = TempDir::new().unwrap();
  let bytes = payload(4096);
  let expected = digest(&bytes);
  std::fs::write(
    scratch.path().join(format!("artifact-{}", expected.sha256)),
    b"truncated",
  )
  .unwrap();
  let server = StubServer::serving(bytes.clone());

  let landed = fetcher(server.clone())
    .download(download_of(scratch.path(), Some(expected)))
    .await
    .unwrap();

  assert_eq!(server.calls(), 1);
  assert_eq!(std::fs::read(&landed).unwrap(), bytes);
}

#[tokio::test]
async fn an_undigested_download_reuses_any_non_empty_cache_entry() {
  let scratch = TempDir::new().unwrap();
  std::fs::write(scratch.path().join("artifact"), b"already here").unwrap();
  let server = StubServer::serving(payload(64));

  let landed = fetcher(server.clone())
    .download(download_of(scratch.path(), None))
    .await
    .unwrap();

  assert_eq!(server.calls(), 0, "without a digest the cache key is the bare filename");
  assert_eq!(landed.file_name().unwrap(), "artifact");
  assert_eq!(std::fs::read(&landed).unwrap(), b"already here");
}

#[tokio::test]
async fn an_undigested_download_replaces_an_empty_cache_entry() {
  let scratch = TempDir::new().unwrap();
  std::fs::write(scratch.path().join("artifact"), b"").unwrap();
  let bytes = payload(64);
  let server = StubServer::serving(bytes.clone());

  let landed = fetcher(server.clone())
    .download(download_of(scratch.path(), None))
    .await
    .unwrap();

  assert_eq!(server.calls(), 1);
  assert_eq!(std::fs::read(&landed).unwrap(), bytes);
}

#[tokio::test]
async fn an_http_error_is_surfaced() {
  let scratch = TempDir::new().unwrap();

  let err = fetcher(StubServer::refusing(404))
    .download(download_of(scratch.path(), None))
    .await
    .unwrap_err();

  assert!(matches!(err, FetchError::HttpStatus(404)));
  assert!(err.to_string().contains("404"), "{err}");
  assert!(
    entries(scratch.path()).is_empty(),
    "an error page must never be mistaken for an artifact"
  );
}

#[tokio::test]
async fn a_transport_failure_is_surfaced_with_its_reason() {
  let scratch = TempDir::new().unwrap();

  let err = fetcher(Arc::new(Unreachable))
    .download(download_of(scratch.path(), None))
    .await
    .unwrap_err();

  assert_eq!(err.to_string(), "no route to host");
  assert!(entries(scratch.path()).is_empty());
}

#[tokio::test]
async fn progress_reports_the_received_byte_counts() {
  let scratch = TempDir::new().unwrap();
  let bytes = payload(4096);
  let expected = digest(&bytes);
  let ticks: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
  let sink = ticks.clone();

  let mut request = download_of(scratch.path(), Some(expected));
  request.progress = Some(watching(move |received, total| {
    sink.lock().unwrap().push((received, total));
  }));

  fetcher(StubServer::serving(bytes.clone()))
    .download(request)
    .await
    .unwrap();

  let ticks = ticks.lock().unwrap();
  assert_eq!(ticks.last().unwrap().0, bytes.len() as u64);
  assert!(
    ticks.windows(2).all(|w| w[1].0 > w[0].0),
    "progress must be monotonic: {ticks:?}"
  );
  assert!(ticks.iter().all(|(_, total)| *total == bytes.len() as u64));
}

#[tokio::test]
async fn an_artifact_past_the_memory_cap_is_spooled_to_disk_as_it_arrives() {
  let scratch = TempDir::new().unwrap();
  let bytes = payload(MEMORY_BODY_CAP * 4);
  let expected = digest(&bytes);
  let staged = scratch.path().join(format!("artifact-{}.download", expected.sha256));
  let observed: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
  let sink = observed.clone();

  let mut request = download_of(scratch.path(), Some(expected));
  request.progress = Some(watching(move |received, _total| {
    let on_disk = std::fs::metadata(&staged).map(|m| m.len()).unwrap_or(0);
    sink.lock().unwrap().push((received, on_disk));
  }));

  fetcher(StubServer::serving(bytes.clone()))
    .download(request)
    .await
    .unwrap();

  let observed = observed.lock().unwrap();
  assert!(observed.len() > 1, "a multi-chunk body reports more than once");
  for (received, on_disk) in observed.iter() {
    assert_eq!(
      received, on_disk,
      "every byte counted has already been written out: {observed:?}"
    );
  }
  assert!(observed.first().unwrap().0 < bytes.len() as u64);
}

#[tokio::test]
async fn a_manifest_body_past_the_memory_cap_is_refused() {
  let oversize = payload(MEMORY_BODY_CAP + 1);

  let err = fetcher(StubServer::serving(oversize))
    .text("https://ota.example/nlu/stable/manifest.json")
    .await
    .unwrap_err();

  assert!(matches!(err, FetchError::TooLarge { limit } if limit == MEMORY_BODY_CAP));
}

#[tokio::test]
async fn fetch_json_decodes_the_response_body() {
  let body = br#"{"version":"1.0.0","updated_at":"2026-08-02T00:00:00Z","android":{"url":"https://ota.example/b.zip","size":512,"sha256":"aaa"}}"#;
  let fetch = fetcher(StubServer::serving(body.to_vec()));

  let manifest: BundleManifest = fetch_json(&fetch, "https://ota.example/nlu/stable/manifest.json")
    .await
    .unwrap();

  assert_eq!(manifest.version, "1.0.0");
  assert_eq!(manifest.updated_at, "2026-08-02T00:00:00Z");
  assert_eq!(
    manifest.android.map(|a| a.digest()),
    Some(ArtifactDigest {
      size: 512,
      sha256: "aaa".to_string(),
    })
  );
}

#[tokio::test]
async fn a_body_that_is_not_the_expected_json_fails_to_decode() {
  let fetch = fetcher(StubServer::serving(b"<html>maintenance</html>".to_vec()));

  let err = fetch_json::<BundleManifest>(&fetch, "https://ota.example/nlu/stable/manifest.json")
    .await
    .unwrap_err();

  assert!(matches!(err, FetchError::Decode(_)), "{err}");
}

#[tokio::test]
async fn text_carries_the_body_through_verbatim() {
  let fetch = fetcher(StubServer::serving(b"  0.8.4+image.2026.05.0\n".to_vec()));

  let body = fetch.text("https://ota.example/version").await.unwrap();

  assert_eq!(body, "  0.8.4+image.2026.05.0\n");
}

#[test]
fn hashing_a_file_matches_the_digest_of_its_bytes() {
  let scratch = TempDir::new().unwrap();
  let bytes = payload(MEMORY_BODY_CAP + 7);
  let blob: PathBuf = scratch.path().join("blob.bin");
  std::fs::write(&blob, &bytes).unwrap();

  assert_eq!(sha256_file(&blob).unwrap(), sha256_hex(&bytes));
}

#[test]
fn a_digest_is_lowercase_hex() {
  assert_eq!(
    sha256_hex(b""),
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
  );
}
