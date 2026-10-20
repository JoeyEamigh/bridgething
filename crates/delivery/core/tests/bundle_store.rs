use std::{
  collections::HashMap,
  io::Write,
  path::{Path, PathBuf},
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
};

use bridgething_delivery::{
  bundle::{
    ASR_MODEL_NAME, BundleConfig, BundleKind, BundleManifest, BundlePlatform, BundleState, BundleStore,
    fetch::{ArtifactFetch, DownloadRequest, FetchError},
  },
  seam::{ArtifactKind, ArtifactValidator, TransferPolicy},
};
use tempfile::TempDir;
use tokio::sync::{Semaphore, oneshot};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const NLU_ANDROID: [&str; 3] = ["manifest.json", "tokenizer.json", "model.tflite"];
const NLU_IOS: [&str; 3] = ["manifest.json", "tokenizer.json", "model.mlpackage/Manifest.json"];
const NLU_DESKTOP: [&str; 3] = ["manifest.json", "tokenizer.json", "model.onnx"];

// ---------------------------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------------------------

fn nlu_manifest(version: &str, sha: &str) -> String {
  format!(
    r#"{{
      "version": "{version}",
      "updated_at": "2026-08-02T00:00:00Z",
      "ios": {{
        "url": "https://ota.bridgething.com/nlu/stable/bundle/{version}/bundle-ios.zip",
        "size": 1024,
        "sha256": "ios-{sha}"
      }},
      "android": {{
        "url": "https://ota.bridgething.com/nlu/stable/bundle/{version}/bundle-android.zip",
        "size": 512,
        "sha256": "{sha}"
      }},
      "macos": {{
        "url": "https://ota.bridgething.com/nlu/stable/bundle/{version}/bundle-ios.zip",
        "size": 1024,
        "sha256": "ios-{sha}"
      }},
      "linux": {{
        "url": "https://ota.bridgething.com/nlu/stable/bundle/{version}/bundle-desktop.zip",
        "size": 2048,
        "sha256": "desktop-{sha}"
      }},
      "windows": {{
        "url": "https://ota.bridgething.com/nlu/stable/bundle/{version}/bundle-desktop.zip",
        "size": 2048,
        "sha256": "desktop-{sha}"
      }}
    }}"#
  )
}

fn asr_manifest(version: &str, sha: &str) -> String {
  format!(
    r#"{{
      "version": "{version}",
      "model": "tiny.en",
      "updated_at": "2026-08-02T00:00:00Z",
      "android": {{
        "url": "https://ota.bridgething.com/asr/stable/model/{version}/ggml-tiny.en.bin",
        "size": 512,
        "sha256": "{sha}"
      }},
      "macos": {{
        "url": "https://ota.bridgething.com/asr/stable/model/{version}/ggml-tiny.en.bin",
        "size": 512,
        "sha256": "{sha}"
      }},
      "linux": {{
        "url": "https://ota.bridgething.com/asr/stable/model/{version}/ggml-tiny.en.bin",
        "size": 512,
        "sha256": "{sha}"
      }},
      "windows": {{
        "url": "https://ota.bridgething.com/asr/stable/model/{version}/ggml-tiny.en.bin",
        "size": 512,
        "sha256": "{sha}"
      }}
    }}"#
  )
}

fn make_bundle_zip(dir: &Path, name: &str, entries: &[&str]) -> PathBuf {
  let path = dir.join(format!("{name}.zip"));
  let mut zip = ZipWriter::new(std::fs::File::create(&path).expect("the scratch dir is writable"));
  let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
  for entry in entries {
    zip.start_file(*entry, options).expect("an entry starts");
    zip.write_all(b"{}").expect("an entry writes");
  }
  zip.finish().expect("the archive closes");
  path
}

fn make_weights(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
  let path = dir.join(format!("{name}.bin"));
  std::fs::write(&path, bytes).expect("the scratch dir is writable");
  path
}

struct FakeFetch {
  manifest: Mutex<String>,
  artifacts: Mutex<HashMap<String, PathBuf>>,
  texts: AtomicUsize,
  downloads: AtomicUsize,
  urls: Mutex<Vec<String>>,
  text_error: Mutex<Option<String>>,
  gate: Mutex<Option<Arc<Semaphore>>>,
  started: Mutex<Option<oneshot::Sender<()>>>,
}

impl FakeFetch {
  fn new(manifest: String) -> Arc<Self> {
    Arc::new(FakeFetch {
      manifest: Mutex::new(manifest),
      artifacts: Mutex::new(HashMap::new()),
      texts: AtomicUsize::new(0),
      downloads: AtomicUsize::new(0),
      urls: Mutex::new(Vec::new()),
      text_error: Mutex::new(None),
      gate: Mutex::new(None),
      started: Mutex::new(None),
    })
  }

  fn with_artifact(self: &Arc<Self>, sha: &str, source: PathBuf) -> Arc<Self> {
    self.artifacts.lock().unwrap().insert(sha.to_string(), source);
    self.clone()
  }

  fn set_manifest(&self, manifest: String) {
    *self.manifest.lock().unwrap() = manifest;
  }

  fn fail_manifest(&self, reason: &str) {
    *self.text_error.lock().unwrap() = Some(reason.to_string());
  }

  fn downloads(&self) -> usize {
    self.downloads.load(Ordering::SeqCst)
  }

  fn texts(&self) -> usize {
    self.texts.load(Ordering::SeqCst)
  }

  fn urls(&self) -> Vec<String> {
    self.urls.lock().unwrap().clone()
  }
}

#[async_trait::async_trait]
impl ArtifactFetch for FakeFetch {
  async fn bytes(&self, url: &str, _limit: usize) -> Result<Vec<u8>, FetchError> {
    self.texts.fetch_add(1, Ordering::SeqCst);
    self.urls.lock().unwrap().push(url.to_string());
    if let Some(reason) = self.text_error.lock().unwrap().clone() {
      return Err(FetchError::Transport(reason));
    }
    Ok(self.manifest.lock().unwrap().clone().into_bytes())
  }

  async fn download(&self, request: DownloadRequest) -> Result<PathBuf, FetchError> {
    self.downloads.fetch_add(1, Ordering::SeqCst);
    if let Some(started) = self.started.lock().unwrap().take() {
      let _ = started.send(());
    }
    let gate = self.gate.lock().unwrap().clone();
    if let Some(gate) = gate {
      gate.acquire().await.expect("the gate is never closed").forget();
    }

    let expected = request
      .expected
      .clone()
      .expect("a bundle artifact always carries a digest");
    let source = self
      .artifacts
      .lock()
      .unwrap()
      .get(&expected.sha256)
      .cloned()
      .ok_or_else(|| FetchError::Transport(format!("no artifact staged for {}", expected.sha256)))?;

    std::fs::create_dir_all(&request.dir).map_err(|e| FetchError::Io(e.to_string()))?;
    let dest = request.dir.join(format!("{}-{}", request.filename, expected.sha256));
    std::fs::copy(&source, &dest).map_err(|e| FetchError::Io(e.to_string()))?;
    if let Some(progress) = request.progress.clone() {
      progress(expected.size, expected.size);
    }
    Ok(dest)
  }
}

struct Policy(bool);

impl TransferPolicy for Policy {
  fn allows_large_transfer(&self) -> bool {
    self.0
  }
}

struct Watcher {
  root: PathBuf,
  version: Mutex<String>,
  seen: Mutex<Vec<(ArtifactKind, PathBuf)>>,
  live_at_validate: Mutex<Vec<bool>>,
  file_at_validate: Mutex<Vec<bool>>,
  reject: Mutex<Option<String>>,
}

impl Watcher {
  fn new(root: PathBuf, version: &str) -> Arc<Self> {
    Arc::new(Watcher {
      root,
      version: Mutex::new(version.to_string()),
      seen: Mutex::new(Vec::new()),
      live_at_validate: Mutex::new(Vec::new()),
      file_at_validate: Mutex::new(Vec::new()),
      reject: Mutex::new(None),
    })
  }

  fn reject_with(&self, reason: &str) {
    *self.reject.lock().unwrap() = Some(reason.to_string());
  }

  fn accept(&self) {
    *self.reject.lock().unwrap() = None;
  }

  fn expect_version(&self, version: &str) {
    *self.version.lock().unwrap() = version.to_string();
  }

  fn staged(&self) -> Vec<PathBuf> {
    self.seen.lock().unwrap().iter().map(|(_, p)| p.clone()).collect()
  }

  fn kinds(&self) -> Vec<ArtifactKind> {
    self.seen.lock().unwrap().iter().map(|(k, _)| *k).collect()
  }

  fn live_when_validated(&self) -> Vec<bool> {
    self.live_at_validate.lock().unwrap().clone()
  }

  fn file_when_validated(&self) -> Vec<bool> {
    self.file_at_validate.lock().unwrap().clone()
  }
}

impl ArtifactValidator for Watcher {
  fn validate(&self, kind: ArtifactKind, staged: &Path) -> Result<(), String> {
    self.seen.lock().unwrap().push((kind, staged.to_path_buf()));
    self.file_at_validate.lock().unwrap().push(staged.is_file());
    let live = self.root.join(self.version.lock().unwrap().clone());
    self.live_at_validate.lock().unwrap().push(live.exists());
    match self.reject.lock().unwrap().clone() {
      Some(reason) => Err(reason),
      None => Ok(()),
    }
  }
}

struct Fixture {
  dir: PathBuf,
  kind: BundleKind,
  platform: BundlePlatform,
  enabled: bool,
  policy: Arc<dyn TransferPolicy>,
  validator: Arc<dyn ArtifactValidator>,
}

impl Fixture {
  fn nlu(dir: &Path) -> Self {
    Fixture {
      dir: dir.to_path_buf(),
      kind: BundleKind::Nlu,
      platform: BundlePlatform::Android,
      enabled: true,
      policy: Arc::new(Policy(true)),
      validator: Watcher::new(dir.to_path_buf(), "unused"),
    }
  }

  fn asr(dir: &Path) -> Self {
    Fixture {
      kind: BundleKind::Asr,
      ..Fixture::nlu(dir)
    }
  }

  fn platform(mut self, platform: BundlePlatform) -> Self {
    self.platform = platform;
    self
  }

  fn disabled(mut self) -> Self {
    self.enabled = false;
    self
  }

  fn policy(mut self, allows: bool) -> Self {
    self.policy = Arc::new(Policy(allows));
    self
  }

  fn validator(mut self, validator: Arc<dyn ArtifactValidator>) -> Self {
    self.validator = validator;
    self
  }

  fn build(self, fetch: Arc<dyn ArtifactFetch>) -> BundleStore {
    BundleStore::new(
      self.kind,
      BundleConfig::new(self.dir, self.platform),
      self.enabled,
      fetch,
      self.policy,
      self.validator,
    )
  }
}

fn root_of(dir: &Path, kind: BundleKind) -> PathBuf {
  dir.join(format!("bridgething-{}", kind.slug()))
}

fn listing(dir: &Path) -> Vec<String> {
  let mut names: Vec<String> = std::fs::read_dir(dir)
    .expect("the root exists")
    .map(|e| e.expect("a readable entry").file_name().to_string_lossy().into_owned())
    .collect();
  names.sort();
  names
}

fn ready(version: &str) -> BundleState {
  BundleState::Ready {
    version: version.to_string(),
  }
}

fn failure(state: &BundleState) -> &str {
  match state {
    BundleState::Failed { reason } => reason,
    other => panic!("expected a failed state, got {other:?}"),
  }
}

// ---------------------------------------------------------------------------------------------
// manifest
// ---------------------------------------------------------------------------------------------

#[test]
fn the_manifest_decodes_the_published_shape_and_carries_every_platform_arm() {
  let manifest: BundleManifest = serde_json::from_str(&nlu_manifest("1.0.0", "aaa")).unwrap();

  assert_eq!(manifest.version, "1.0.0");
  assert_eq!(manifest.updated_at, "2026-08-02T00:00:00Z");
  assert_eq!(manifest.android.as_ref().unwrap().size, 512);
  assert_eq!(manifest.android.as_ref().unwrap().sha256, "aaa");
  assert_eq!(manifest.ios.as_ref().unwrap().size, 1024);
  assert_eq!(manifest.ios.as_ref().unwrap().sha256, "ios-aaa");
  assert_eq!(
    manifest.macos, manifest.ios,
    "the desktop and the phone run the same coreml package"
  );
}

#[test]
fn the_asr_manifest_decodes_without_the_fields_only_the_nlu_one_carries() {
  let manifest: BundleManifest = serde_json::from_str(&asr_manifest("1.0.0", "aaa")).unwrap();

  assert_eq!(manifest.version, "1.0.0");
  assert!(manifest.ios.is_none());
  assert_eq!(manifest.android.as_ref().unwrap().digest().sha256, "aaa");
}

#[test]
fn a_manifest_arm_projects_to_the_digest_the_fetcher_verifies() {
  let manifest: BundleManifest = serde_json::from_str(&nlu_manifest("1.0.0", "aaa")).unwrap();

  let digest = manifest.artifact_for(BundlePlatform::Android).unwrap().digest();

  assert_eq!(digest.size, 512);
  assert_eq!(digest.sha256, "aaa");
}

#[tokio::test]
async fn the_manifest_url_is_built_from_the_root_the_kind_and_the_channel() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch.clone());

  store.ensure().await;

  assert_eq!(fetch.urls(), ["https://ota.bridgething.com/nlu/stable/manifest.json"]);
}

// ---------------------------------------------------------------------------------------------
// nlu bundles
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_fresh_bundle_validates_and_rotates_into_place() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch);

  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  let live = store.live().expect("a ready store serves a path");
  assert!(live.join("manifest.json").exists());
  assert!(live.join("model.tflite").exists());
}

#[tokio::test]
async fn the_state_stream_carries_the_download_and_the_version_it_lands_on() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch);
  let mut changes = store.subscribe();

  store.ensure().await;

  let mut seen = Vec::new();
  while let Ok(state) = changes.try_recv() {
    seen.push(state);
  }
  assert_eq!(
    seen.first(),
    Some(&BundleState::Downloading {
      received: 0,
      total: 512
    })
  );
  assert_eq!(seen.last(), Some(&ready("1.0.0")));
}

#[tokio::test]
async fn an_already_installed_version_is_not_downloaded_again() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch.clone());

  store.ensure().await;
  store.ensure().await;

  assert_eq!(fetch.downloads(), 1);
  assert_eq!(fetch.texts(), 2, "the manifest is still consulted on every pass");
}

#[tokio::test]
async fn a_bundle_that_fails_validation_leaves_the_previous_one_serving() {
  let scratch = TempDir::new().unwrap();
  let first = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let second = make_bundle_zip(scratch.path(), "v2", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "1.0.0"))
    .with_artifact("1.0.0", first)
    .with_artifact("2.0.0", second);
  let watcher = Watcher::new(root_of(scratch.path(), BundleKind::Nlu), "1.0.0");
  let store = Fixture::nlu(scratch.path())
    .validator(watcher.clone())
    .build(fetch.clone());

  store.ensure().await;
  assert_eq!(store.state(), ready("1.0.0"));

  watcher.reject_with("the model refused to load");
  fetch.set_manifest(nlu_manifest("2.0.0", "2.0.0"));
  store.ensure().await;

  assert_eq!(
    store.state(),
    ready("1.0.0"),
    "a rejected bundle never displaces a good one"
  );
  assert_eq!(store.live().unwrap().file_name().unwrap(), "1.0.0");
  assert!(!root_of(scratch.path(), BundleKind::Nlu).join("2.0.0").exists());
}

#[tokio::test]
async fn the_validator_runs_on_the_staged_copy_before_anything_rotates() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let root = root_of(scratch.path(), BundleKind::Nlu);
  let watcher = Watcher::new(root.clone(), "1.0.0");
  let store = Fixture::nlu(scratch.path()).validator(watcher.clone()).build(fetch);

  store.ensure().await;

  assert_eq!(watcher.kinds(), [ArtifactKind::NluModel]);
  let staged = watcher.staged();
  assert_eq!(staged.len(), 1);
  assert_ne!(
    staged[0],
    root.join("1.0.0"),
    "the validator sees staging, not the live path"
  );
  assert!(staged[0].starts_with(&root));
  assert_eq!(
    watcher.live_when_validated(),
    [false],
    "nothing is live until the validator has passed"
  );
}

#[tokio::test]
async fn an_archive_missing_a_required_entry_never_rotates_in() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &["manifest.json", "tokenizer.json"]);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch);

  store.ensure().await;

  assert!(store.live().is_none());
  assert_eq!(failure(&store.state()), "nlu archive is missing model.tflite");
  assert!(!root_of(scratch.path(), BundleKind::Nlu).join("1.0.0").exists());
}

#[tokio::test]
async fn an_ios_bundle_wants_the_coreml_package_where_android_wants_the_tflite_graph() {
  let scratch = TempDir::new().unwrap();
  let android_shaped = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let ios_shaped = make_bundle_zip(scratch.path(), "v2", &NLU_IOS);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa"))
    .with_artifact("ios-aaa", android_shaped)
    .with_artifact("ios-1.0.0", ios_shaped);

  let store = Fixture::nlu(scratch.path())
    .platform(BundlePlatform::Ios)
    .build(fetch.clone());
  store.ensure().await;
  assert_eq!(failure(&store.state()), "nlu archive is missing model.mlpackage");

  fetch.set_manifest(nlu_manifest("1.0.0", "1.0.0"));
  let store = Fixture::nlu(scratch.path()).platform(BundlePlatform::Ios).build(fetch);
  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert!(store.live().unwrap().join("model.mlpackage").is_dir());
}

#[tokio::test]
async fn a_macos_store_reads_the_arm_that_carries_the_same_coreml_package_ios_gets() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_IOS);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("ios-aaa", zip);
  let store = Fixture::nlu(scratch.path())
    .platform(BundlePlatform::Macos)
    .build(fetch.clone());

  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert!(store.live().unwrap().join("model.mlpackage").is_dir());
  assert!(
    fetch.urls().iter().all(|url| !url.ends_with("bundle-android.zip")),
    "the desktop never reaches for the tflite graph it cannot run"
  );
}

#[tokio::test]
async fn a_desktop_bundle_wants_the_onnx_graph_that_neither_phone_arm_carries() {
  let scratch = TempDir::new().unwrap();
  let ios_shaped = make_bundle_zip(scratch.path(), "v1", &NLU_IOS);
  let onnx_shaped = make_bundle_zip(scratch.path(), "v2", &NLU_DESKTOP);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa"))
    .with_artifact("desktop-aaa", ios_shaped)
    .with_artifact("desktop-1.0.0", onnx_shaped);

  let store = Fixture::nlu(scratch.path())
    .platform(BundlePlatform::Linux)
    .build(fetch.clone());
  store.ensure().await;
  assert_eq!(failure(&store.state()), "nlu archive is missing model.onnx");

  fetch.set_manifest(nlu_manifest("1.0.0", "1.0.0"));
  let store = Fixture::nlu(scratch.path())
    .platform(BundlePlatform::Linux)
    .build(fetch);
  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert!(store.live().unwrap().join("model.onnx").is_file());
}

#[tokio::test]
async fn a_windows_store_reads_the_same_desktop_arm_linux_does() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_DESKTOP);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("desktop-aaa", zip);
  let store = Fixture::nlu(scratch.path())
    .platform(BundlePlatform::Windows)
    .build(fetch.clone());

  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert!(store.live().unwrap().join("model.onnx").is_file());
  assert!(
    fetch
      .urls()
      .iter()
      .all(|url| !url.ends_with("bundle-ios.zip") && !url.ends_with("bundle-android.zip")),
    "a desktop never reaches for a phone arm it cannot run"
  );
}

#[tokio::test]
async fn a_manifest_that_predates_the_desktop_arms_still_serves_the_phones() {
  let scratch = TempDir::new().unwrap();
  let older = r#"{
      "version": "1.0.0",
      "updated_at": "2026-08-02T00:00:00Z",
      "ios": {
        "url": "https://ota.bridgething.com/nlu/stable/bundle/1.0.0/bundle-ios.zip",
        "size": 1024,
        "sha256": "ios-aaa"
      }
    }"#;
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_IOS);
  let fetch = FakeFetch::new(older.to_owned()).with_artifact("ios-aaa", zip);
  let store = Fixture::nlu(scratch.path()).platform(BundlePlatform::Ios).build(fetch);

  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
}

#[tokio::test]
async fn a_macos_asr_store_takes_the_platform_agnostic_weights_android_takes() {
  let scratch = TempDir::new().unwrap();
  let weights = make_weights(scratch.path(), "v1", b"ggml-shaped");
  let fetch = FakeFetch::new(asr_manifest("1.0.0", "aaa")).with_artifact("aaa", weights);
  let store = Fixture::asr(scratch.path())
    .platform(BundlePlatform::Macos)
    .build(fetch);

  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert_eq!(store.live().unwrap().file_name().unwrap(), ASR_MODEL_NAME);
}

#[tokio::test]
async fn an_ios_store_downloads_the_ios_arm_of_the_manifest() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_IOS);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("ios-aaa", zip);
  let store = Fixture::nlu(scratch.path()).platform(BundlePlatform::Ios).build(fetch);
  let mut changes = store.subscribe();

  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert_eq!(
    changes.try_recv().unwrap(),
    BundleState::Downloading {
      received: 0,
      total: 1024
    },
    "the ios arm declares its own size"
  );
}

#[tokio::test]
async fn a_manifest_without_an_artifact_for_this_platform_fails() {
  let scratch = TempDir::new().unwrap();
  let fetch = FakeFetch::new(asr_manifest("1.0.0", "aaa"));
  let store = Fixture::nlu(scratch.path()).platform(BundlePlatform::Ios).build(fetch);

  store.ensure().await;

  assert_eq!(
    failure(&store.state()),
    "the nlu manifest carries nothing for this platform"
  );
  assert!(store.live().is_none());
}

#[tokio::test]
async fn an_archive_entry_escaping_the_staging_root_is_rejected() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(
    scratch.path(),
    "evil",
    &["manifest.json", "tokenizer.json", "model.tflite", "../escaped.txt"],
  );
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch);

  store.ensure().await;

  assert_eq!(
    failure(&store.state()),
    "nlu archive entry escapes the staging root: ../escaped.txt"
  );
  assert!(store.live().is_none());
  assert!(!root_of(scratch.path(), BundleKind::Nlu).join("escaped.txt").exists());
  assert!(!scratch.path().join("escaped.txt").exists());
}

#[tokio::test]
async fn a_manifest_fetch_failure_leaves_the_installed_bundle_serving() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch.clone());
  store.ensure().await;

  fetch.fail_manifest("no route to host");
  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert!(store.live().is_some());
}

#[tokio::test]
async fn a_manifest_fetch_failure_with_nothing_installed_reports_the_reason() {
  let scratch = TempDir::new().unwrap();
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa"));
  fetch.fail_manifest("no route to host");
  let store = Fixture::nlu(scratch.path()).build(fetch);

  store.ensure().await;

  assert_eq!(failure(&store.state()), "no route to host");
}

#[tokio::test]
async fn turning_the_capability_off_stops_serving_the_bundle_without_deleting_it() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch);
  store.ensure().await;

  store.set_enabled(false).await;

  assert_eq!(store.state(), BundleState::Absent);
  assert!(store.live().is_none(), "a gated capability serves no model");
  assert!(
    root_of(scratch.path(), BundleKind::Nlu)
      .join("1.0.0/model.tflite")
      .exists(),
    "the capability switch gates the model, it does not uninstall it"
  );
}

#[tokio::test]
async fn cycling_the_capability_off_and_on_never_downloads_a_second_time() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch.clone());
  store.ensure().await;

  for _ in 0..4 {
    store.set_enabled(false).await;
    store.set_enabled(true).await;
    store.ensure().await;
  }

  assert_eq!(fetch.downloads(), 1, "toggle churn must not re-fetch the artifact");
  assert_eq!(store.state(), ready("1.0.0"));
  assert!(store.live().is_some());
}

#[tokio::test]
async fn re_enabling_publishes_the_installed_version_without_waiting_on_the_network() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch.clone());
  store.ensure().await;
  store.set_enabled(false).await;

  fetch.fail_manifest("no route to host");
  store.set_enabled(true).await;

  assert_eq!(
    store.state(),
    ready("1.0.0"),
    "an offline re-enable still serves what is already on disk"
  );
  assert!(store.live().is_some());
}

#[tokio::test]
async fn a_disabled_store_never_checks() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).disabled().build(fetch.clone());

  store.ensure().await;

  assert_eq!(fetch.texts(), 0);
  assert_eq!(fetch.downloads(), 0);
  assert_eq!(store.state(), BundleState::Absent);
}

#[tokio::test]
async fn turning_the_capability_back_on_installs_again() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).disabled().build(fetch.clone());

  store.set_enabled(true).await;
  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert_eq!(fetch.downloads(), 1);
}

#[tokio::test]
async fn toggling_to_the_state_it_is_already_in_changes_nothing() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch.clone());
  store.ensure().await;

  store.set_enabled(true).await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert_eq!(fetch.downloads(), 1);
}

#[tokio::test]
async fn rotating_a_new_version_prunes_the_one_it_replaced() {
  let scratch = TempDir::new().unwrap();
  let first = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let second = make_bundle_zip(scratch.path(), "v2", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "1.0.0"))
    .with_artifact("1.0.0", first)
    .with_artifact("2.0.0", second);
  let store = Fixture::nlu(scratch.path()).build(fetch.clone());

  store.ensure().await;
  fetch.set_manifest(nlu_manifest("2.0.0", "2.0.0"));
  store.ensure().await;

  assert_eq!(store.state(), ready("2.0.0"));
  assert_eq!(listing(&root_of(scratch.path(), BundleKind::Nlu)), ["2.0.0", "current"]);
}

#[tokio::test]
async fn a_new_store_adopts_a_bundle_already_on_disk() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  Fixture::nlu(scratch.path()).build(fetch.clone()).ensure().await;

  let second = Fixture::nlu(scratch.path()).build(fetch);

  assert_eq!(second.state(), ready("1.0.0"));
  assert!(second.live().is_some());
}

#[tokio::test]
async fn a_current_marker_pointing_at_an_incomplete_tree_is_not_adopted() {
  let scratch = TempDir::new().unwrap();
  let root = root_of(scratch.path(), BundleKind::Nlu);
  std::fs::create_dir_all(root.join("1.0.0")).unwrap();
  std::fs::write(root.join("1.0.0/manifest.json"), b"{}").unwrap();
  std::fs::write(root.join("current"), b"1.0.0").unwrap();
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa"));

  let store = Fixture::nlu(scratch.path()).build(fetch);

  assert_eq!(store.state(), BundleState::Absent);
  assert!(store.live().is_none());
}

#[tokio::test]
async fn overlapping_ensure_calls_share_a_single_run() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let gate = Arc::new(Semaphore::new(0));
  let (started_tx, started_rx) = oneshot::channel();
  *fetch.gate.lock().unwrap() = Some(gate.clone());
  *fetch.started.lock().unwrap() = Some(started_tx);
  let store = Arc::new(Fixture::nlu(scratch.path()).build(fetch.clone()));

  let first = tokio::spawn({
    let store = store.clone();
    async move { store.ensure().await }
  });
  started_rx.await.expect("the download begins");
  let second = tokio::spawn({
    let store = store.clone();
    async move { store.ensure().await }
  });
  for _ in 0..8 {
    tokio::task::yield_now().await;
  }
  gate.add_permits(1);
  first.await.unwrap();
  second.await.unwrap();

  assert_eq!(fetch.downloads(), 1);
  assert_eq!(
    fetch.texts(),
    1,
    "the second caller joins the run rather than starting one"
  );
  assert_eq!(store.state(), ready("1.0.0"));
}

#[tokio::test]
async fn a_metered_network_defers_the_download() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).policy(false).build(fetch.clone());

  store.ensure().await;

  assert_eq!(fetch.downloads(), 0);
  assert_eq!(store.state(), BundleState::Absent);
}

#[tokio::test]
async fn a_metered_network_keeps_the_installed_bundle_serving() {
  let scratch = TempDir::new().unwrap();
  let first = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let second = make_bundle_zip(scratch.path(), "v2", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "1.0.0"))
    .with_artifact("1.0.0", first)
    .with_artifact("2.0.0", second);
  Fixture::nlu(scratch.path()).build(fetch.clone()).ensure().await;

  fetch.set_manifest(nlu_manifest("2.0.0", "2.0.0"));
  let metered = Fixture::nlu(scratch.path()).policy(false).build(fetch.clone());
  metered.ensure().await;

  assert_eq!(fetch.downloads(), 1, "the upgrade waits for an unmetered link");
  assert_eq!(metered.state(), ready("1.0.0"));
}

#[tokio::test]
async fn an_explicit_download_runs_on_a_metered_link() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).policy(false).build(fetch.clone());

  store.ensure().await;
  assert_eq!(fetch.downloads(), 0, "nothing starts on its own over cellular");

  store.download_now().await;

  assert_eq!(fetch.downloads(), 1);
  assert_eq!(store.state(), ready("1.0.0"));
  assert!(store.live().is_some());
}

#[tokio::test]
async fn an_explicit_download_does_not_settle_for_an_automatic_run_that_deferred() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Arc::new(Fixture::nlu(scratch.path()).policy(false).build(fetch.clone()));

  let automatic = tokio::spawn({
    let store = store.clone();
    async move { store.ensure().await }
  });
  let explicit = tokio::spawn({
    let store = store.clone();
    async move { store.download_now().await }
  });
  automatic.await.unwrap();
  explicit.await.unwrap();

  assert_eq!(
    fetch.downloads(),
    1,
    "joining the deferring run would answer the user's tap with nothing"
  );
  assert_eq!(store.state(), ready("1.0.0"));
}

#[tokio::test]
async fn an_explicit_download_stays_gated_behind_the_capability() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path())
    .disabled()
    .policy(false)
    .build(fetch.clone());

  store.download_now().await;

  assert_eq!(fetch.downloads(), 0);
  assert_eq!(fetch.texts(), 0);
  assert_eq!(store.state(), BundleState::Absent);
}

// ---------------------------------------------------------------------------------------------
// asr models
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_bare_weights_file_installs_and_resolves_to_the_file_itself() {
  let scratch = TempDir::new().unwrap();
  let weights = make_weights(scratch.path(), "v1", b"ggml weights");
  let fetch = FakeFetch::new(asr_manifest("1.0.0", "aaa")).with_artifact("aaa", weights);
  let store = Fixture::asr(scratch.path()).build(fetch);

  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  let live = store.live().expect("a ready store serves a path");
  assert!(live.is_file());
  assert_eq!(live.file_name().unwrap(), ASR_MODEL_NAME);
  assert_eq!(live.parent().unwrap().file_name().unwrap(), "1.0.0");
  assert_eq!(std::fs::read(&live).unwrap(), b"ggml weights");
}

#[tokio::test]
async fn the_validator_sees_the_weights_file_not_its_directory() {
  let scratch = TempDir::new().unwrap();
  let weights = make_weights(scratch.path(), "v1", b"ggml weights");
  let fetch = FakeFetch::new(asr_manifest("1.0.0", "aaa")).with_artifact("aaa", weights);
  let watcher = Watcher::new(root_of(scratch.path(), BundleKind::Asr), "1.0.0");
  let store = Fixture::asr(scratch.path()).validator(watcher.clone()).build(fetch);

  store.ensure().await;

  let staged = watcher.staged();
  assert_eq!(staged.len(), 1);
  assert_eq!(watcher.file_when_validated(), [true]);
  assert_eq!(staged[0].file_name().unwrap(), ASR_MODEL_NAME);
  assert_eq!(watcher.kinds(), [ArtifactKind::AsrModel]);
}

#[tokio::test]
async fn a_model_the_validator_rejects_never_rotates_in() {
  let scratch = TempDir::new().unwrap();
  let weights = make_weights(scratch.path(), "v1", b"not a ggml file");
  let fetch = FakeFetch::new(asr_manifest("1.0.0", "aaa")).with_artifact("aaa", weights);
  let watcher = Watcher::new(root_of(scratch.path(), BundleKind::Asr), "1.0.0");
  watcher.reject_with("asr model header is not ggml");
  let store = Fixture::asr(scratch.path()).validator(watcher).build(fetch);

  store.ensure().await;

  assert!(store.live().is_none());
  assert_eq!(failure(&store.state()), "asr model header is not ggml");
  assert!(!root_of(scratch.path(), BundleKind::Asr).join("1.0.0").exists());
}

#[tokio::test]
async fn an_empty_download_never_rotates_in() {
  let scratch = TempDir::new().unwrap();
  let weights = make_weights(scratch.path(), "v1", b"");
  let fetch = FakeFetch::new(asr_manifest("1.0.0", "aaa")).with_artifact("aaa", weights);
  let store = Fixture::asr(scratch.path()).build(fetch);

  store.ensure().await;

  assert!(store.live().is_none());
  assert_eq!(failure(&store.state()), "asr model is missing model.bin");
}

#[tokio::test]
async fn rotating_a_new_model_prunes_the_one_it_replaced() {
  let scratch = TempDir::new().unwrap();
  let first = make_weights(scratch.path(), "v1", b"ggml weights");
  let second = make_weights(scratch.path(), "v2", b"ggml newer");
  let fetch = FakeFetch::new(asr_manifest("1.0.0", "1.0.0"))
    .with_artifact("1.0.0", first)
    .with_artifact("2.0.0", second);
  let store = Fixture::asr(scratch.path()).build(fetch.clone());

  store.ensure().await;
  fetch.set_manifest(asr_manifest("2.0.0", "2.0.0"));
  store.ensure().await;

  assert_eq!(store.state(), ready("2.0.0"));
  assert_eq!(std::fs::read(store.live().unwrap()).unwrap(), b"ggml newer");
  assert_eq!(listing(&root_of(scratch.path(), BundleKind::Asr)), ["2.0.0", "current"]);
}

#[tokio::test]
async fn a_new_store_adopts_a_model_already_on_disk() {
  let scratch = TempDir::new().unwrap();
  let weights = make_weights(scratch.path(), "v1", b"ggml weights");
  let fetch = FakeFetch::new(asr_manifest("1.0.0", "aaa")).with_artifact("aaa", weights);
  Fixture::asr(scratch.path()).build(fetch.clone()).ensure().await;

  let second = Fixture::asr(scratch.path()).build(fetch);

  assert_eq!(second.state(), ready("1.0.0"));
  assert!(second.live().is_some());
}

#[tokio::test]
async fn the_two_kinds_install_side_by_side_under_their_own_roots() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let weights = make_weights(scratch.path(), "v1", b"ggml weights");
  let nlu = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let asr = FakeFetch::new(asr_manifest("2.0.0", "bbb")).with_artifact("bbb", weights);

  Fixture::nlu(scratch.path()).build(nlu).ensure().await;
  Fixture::asr(scratch.path()).build(asr).ensure().await;

  assert!(scratch.path().join("bridgething-nlu/1.0.0/model.tflite").exists());
  assert!(scratch.path().join("bridgething-asr/2.0.0/model.bin").exists());
}

#[tokio::test]
async fn turning_the_capability_off_stops_serving_the_model_without_deleting_it() {
  let scratch = TempDir::new().unwrap();
  let weights = make_weights(scratch.path(), "v1", b"ggml weights");
  let fetch = FakeFetch::new(asr_manifest("1.0.0", "aaa")).with_artifact("aaa", weights);
  let store = Fixture::asr(scratch.path()).build(fetch);
  store.ensure().await;

  store.set_enabled(false).await;

  assert_eq!(store.state(), BundleState::Absent);
  assert!(store.live().is_none());
  assert!(
    root_of(scratch.path(), BundleKind::Asr)
      .join("1.0.0")
      .join(ASR_MODEL_NAME)
      .exists()
  );
}

// ---------------------------------------------------------------------------------------------
// staging hygiene
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_successful_install_leaves_no_download_or_staging_directory_behind() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let store = Fixture::nlu(scratch.path()).build(fetch);

  store.ensure().await;

  assert_eq!(listing(&root_of(scratch.path(), BundleKind::Nlu)), ["1.0.0", "current"]);
}

#[tokio::test]
async fn a_failed_install_leaves_no_staging_directory_behind() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let watcher = Watcher::new(root_of(scratch.path(), BundleKind::Nlu), "1.0.0");
  watcher.reject_with("the model refused to load");
  let store = Fixture::nlu(scratch.path())
    .validator(watcher.clone())
    .build(fetch.clone());

  store.ensure().await;
  assert!(matches!(store.state(), BundleState::Failed { .. }));

  watcher.accept();
  watcher.expect_version("1.0.0");
  store.ensure().await;

  assert_eq!(store.state(), ready("1.0.0"));
  assert_eq!(
    listing(&root_of(scratch.path(), BundleKind::Nlu)),
    ["1.0.0", "current"],
    "the abandoned staging tree from the first attempt is gone"
  );
}

// ---------------------------------------------------------------------------------------------
// staging off the executor
// ---------------------------------------------------------------------------------------------

struct Blocker {
  entered: tokio::sync::mpsc::UnboundedSender<()>,
  release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl ArtifactValidator for Blocker {
  fn validate(&self, _kind: ArtifactKind, _staged: &Path) -> Result<(), String> {
    let _ = self.entered.send(());
    self
      .release
      .lock()
      .unwrap()
      .recv_timeout(std::time::Duration::from_secs(5))
      .map_err(|_| "staging held the only executor thread, so nothing could release it".to_string())
  }
}

#[tokio::test(flavor = "current_thread")]
async fn staging_a_bundle_leaves_the_executor_free_to_run_other_tasks() {
  let scratch = TempDir::new().unwrap();
  let zip = make_bundle_zip(scratch.path(), "v1", &NLU_ANDROID);
  let fetch = FakeFetch::new(nlu_manifest("1.0.0", "aaa")).with_artifact("aaa", zip);
  let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
  let (release_tx, release_rx) = std::sync::mpsc::channel();
  let store = Fixture::nlu(scratch.path())
    .validator(Arc::new(Blocker {
      entered: entered_tx,
      release: Mutex::new(release_rx),
    }))
    .build(fetch);

  let installing = tokio::spawn(async move {
    store.download_now().await;
    store.state()
  });

  tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx.recv())
    .await
    .expect("staging held the only executor thread")
    .expect("the validator ran");
  let _ = release_tx.send(());

  assert_eq!(
    tokio::time::timeout(std::time::Duration::from_secs(5), installing)
      .await
      .expect("the install finishes once staging is released")
      .expect("the install task did not panic"),
    ready("1.0.0")
  );
}
