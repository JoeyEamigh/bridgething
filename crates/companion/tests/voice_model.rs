#[path = "rig/backends.rs"]
mod backends;
#[path = "rig/heard.rs"]
mod heard;
#[path = "rig/log_sink.rs"]
mod log_sink;
#[path = "rig/secrets.rs"]
mod secrets;

use std::{
  collections::HashMap,
  io::Write,
  path::{Path, PathBuf},
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
  },
  time::Duration,
};

use bridgething_companion::{
  api::{CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo, ModelPlatform, SessionEvent, VoiceModelStatus},
  backend::{
    ModelArtifactKind, ModelArtifactValidator, ModelValidationError, NluModelOutputs, NluModelRunner, NluRunnerError,
    PrepareSink, SpeechRecognizer, Transcription, TranscriptionSink, TransferPolicy,
  },
  session::Session,
  voice::intent_catalog,
};
use bridgething_delivery::bundle::fetch::{ArtifactFetch, DownloadRequest, FetchError};
use libbridgething::NluStage;
use serde_json::json;
use tokio::sync::Semaphore;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{
  backends::{Heard, Offline, RigHost},
  log_sink::Quiet,
  secrets::MemorySecrets,
};

const DEADLINE: Duration = Duration::from_secs(5);
const CONTENT_UTTERANCE: &str = "play the new mitski album";
const MAX_LEN: usize = 8;
const BIO_TAGS: [&str; 3] = ["O", "B-target", "I-target"];

// MARK: bundle fixture

fn bundle_manifest(names: &[&str]) -> String {
  let intents: Vec<_> = names
    .iter()
    .map(|name| json!({ "name": name, "slots": ["target"] }))
    .collect();
  json!({
    "schemaVersion": "0.3.1",
    "maxLen": MAX_LEN,
    "intents": intents,
    "bioTags": BIO_TAGS,
    "closedHeads": [],
    "rejection": { "inDomainThreshold": 0.5, "clarifyMargin": 0.4 },
  })
  .to_string()
}

fn tokenizer_json() -> String {
  let vocab: serde_json::Map<String, serde_json::Value> = ["[PAD]", "[UNK]", "play", "the", "new", "mitski", "album"]
    .iter()
    .enumerate()
    .map(|(id, token)| ((*token).to_owned(), json!(id)))
    .collect();
  json!({
    "version": "1.0",
    "truncation": null,
    "padding": null,
    "added_tokens": [],
    "normalizer": { "type": "Lowercase" },
    "pre_tokenizer": { "type": "Whitespace" },
    "post_processor": null,
    "decoder": null,
    "model": { "type": "WordLevel", "vocab": vocab, "unk_token": "[UNK]" },
  })
  .to_string()
}

fn write_bundle(dir: &Path, file: &str, names: &[&str]) -> PathBuf {
  let path = dir.join(file);
  let mut zip = ZipWriter::new(std::fs::File::create(&path).expect("the scratch dir is writable"));
  let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
  for (name, body) in [
    ("manifest.json", bundle_manifest(names)),
    ("tokenizer.json", tokenizer_json()),
    ("model.tflite", "TFL3 shaped bytes".to_owned()),
  ] {
    zip.start_file(name, options).expect("an entry starts");
    zip.write_all(body.as_bytes()).expect("an entry writes");
  }
  zip.finish().expect("the archive closes");
  path
}

fn staged_bundle(dir: &Path) -> PathBuf {
  write_bundle(dir, "bundle-android.zip", &intent_catalog::SURFACE_NAMES)
}

fn mismatched_bundle(dir: &Path) -> PathBuf {
  write_bundle(dir, "bundle-android.zip", &["AAA_NOT_AN_INTENT", "ZZZ_ALSO_NOT"])
}

fn staged_weights(dir: &Path) -> PathBuf {
  let path = dir.join("ggml-tiny.en.bin");
  std::fs::write(&path, b"ggml").expect("the scratch dir is writable");
  path
}

// MARK: doubles

fn arm_manifest(slug: &str, version: &str, name: &str, path: &Path) -> String {
  json!({
    "version": version,
    "updated_at": "2026-08-09T00:00:00Z",
    "android": {
      "url": format!("https://ota.bridgething.com/{slug}/stable/{version}/{name}"),
      "size": std::fs::metadata(path).expect("the artifact exists").len(),
      "sha256": format!("{slug}-{version}"),
    },
  })
  .to_string()
}

struct StagedFetch {
  artifacts: Mutex<HashMap<String, PathBuf>>,
  manifests: Mutex<HashMap<String, String>>,
  manifest_reads: AtomicUsize,
  downloads: AtomicUsize,
  gate: Mutex<Option<Arc<Semaphore>>>,
}

impl StagedFetch {
  fn new(bundle: PathBuf, weights: PathBuf) -> Arc<Self> {
    Arc::new(StagedFetch {
      manifests: Mutex::new(HashMap::from([
        (
          "nlu".to_owned(),
          arm_manifest("nlu", "1.0.0", "bundle-android.zip", &bundle),
        ),
        (
          "asr".to_owned(),
          arm_manifest("asr", "1.0.0", "ggml-tiny.en.bin", &weights),
        ),
      ])),
      artifacts: Mutex::new(HashMap::from([
        ("nlu-1.0.0".to_owned(), bundle),
        ("asr-1.0.0".to_owned(), weights),
      ])),
      manifest_reads: AtomicUsize::new(0),
      downloads: AtomicUsize::new(0),
      gate: Mutex::new(None),
    })
  }

  fn downloads(&self) -> usize {
    self.downloads.load(Ordering::SeqCst)
  }

  fn manifest_reads(&self) -> usize {
    self.manifest_reads.load(Ordering::SeqCst)
  }

  fn publish(&self, slug: &str, version: &str, name: &str, artifact: PathBuf) {
    self
      .manifests
      .lock()
      .unwrap()
      .insert(slug.to_owned(), arm_manifest(slug, version, name, &artifact));
    self
      .artifacts
      .lock()
      .unwrap()
      .insert(format!("{slug}-{version}"), artifact);
  }

  fn hold(&self) -> Arc<Semaphore> {
    let gate = Arc::new(Semaphore::new(0));
    *self.gate.lock().unwrap() = Some(gate.clone());
    gate
  }

  fn release(&self, gate: &Semaphore) {
    *self.gate.lock().unwrap() = None;
    gate.add_permits(16);
  }
}

#[async_trait::async_trait]
impl ArtifactFetch for StagedFetch {
  async fn text(&self, url: &str) -> Result<String, FetchError> {
    self.manifest_reads.fetch_add(1, Ordering::SeqCst);
    let held = self.manifests.lock().unwrap();
    held
      .iter()
      .find(|(slug, _)| url.contains(&format!("/{slug}/")))
      .map(|(_, body)| body.clone())
      .ok_or_else(|| FetchError::Transport(format!("nothing staged for {url}")))
  }

  async fn download(&self, request: DownloadRequest) -> Result<PathBuf, FetchError> {
    self.downloads.fetch_add(1, Ordering::SeqCst);
    let gate = self.gate.lock().unwrap().clone();
    if let Some(gate) = gate {
      gate.acquire().await.expect("the gate is never closed").forget();
    }
    let expected = request.expected.clone().expect("a bundle carries a digest");
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
    if let Some(progress) = request.progress {
      progress(expected.size, expected.size);
    }
    Ok(dest)
  }
}

#[derive(Default)]
struct RecordingValidator(AtomicUsize);

impl ModelArtifactValidator for RecordingValidator {
  fn validate(&self, _kind: ModelArtifactKind, _path: String) -> Result<(), ModelValidationError> {
    self.0.fetch_add(1, Ordering::SeqCst);
    Ok(())
  }
}

struct SwitchedPolicy(AtomicBool);

impl SwitchedPolicy {
  fn allowing() -> Arc<Self> {
    Arc::new(SwitchedPolicy(AtomicBool::new(true)))
  }

  fn metered() -> Arc<Self> {
    Arc::new(SwitchedPolicy(AtomicBool::new(false)))
  }

  fn reaches_wifi(&self) {
    self.0.store(true, Ordering::SeqCst);
  }
}

impl TransferPolicy for SwitchedPolicy {
  fn allows_large_transfer(&self) -> bool {
    self.0.load(Ordering::SeqCst)
  }
}

#[derive(Default)]
struct PlayRunner {
  predictions: AtomicUsize,
}

impl NluModelRunner for PlayRunner {
  fn prewarm(&self) {}

  fn predict(&self, input_ids: Vec<i32>, _attention_mask: Vec<i32>) -> Result<NluModelOutputs, NluRunnerError> {
    self.predictions.fetch_add(1, Ordering::SeqCst);
    let play = intent_catalog::SURFACE_NAMES
      .iter()
      .position(|name| *name == "PLAY")
      .expect("PLAY is in the catalog");
    let mut intent_logits = vec![0.0f32; intent_catalog::SURFACE_NAMES.len()];
    intent_logits[play] = 12.0;
    Ok(NluModelOutputs {
      intent_logits,
      ood_logit: -8.0,
      bio_logits: vec![0.0; input_ids.len() * BIO_TAGS.len()],
      closed_logits: Vec::new(),
    })
  }
}

struct CountingRecognizer {
  prepared: AtomicUsize,
}

impl SpeechRecognizer for CountingRecognizer {
  fn prepare(&self, sink: Arc<PrepareSink>) {
    self.prepared.fetch_add(1, Ordering::SeqCst);
    sink.on_ready();
  }

  fn transcribe(&self, _pcm: Vec<f32>, _sample_rate_hz: u32, sink: Arc<TranscriptionSink>) {
    sink.complete(Transcription {
      text: String::new(),
      alternatives: Vec::new(),
      segments: Vec::new(),
      confidence: None,
    });
  }
}

// MARK: rig

struct ModelRig {
  session: Arc<Session>,
  heard: Arc<Heard>,
  fetch: Arc<StagedFetch>,
  runner: Arc<PlayRunner>,
  validator: Arc<RecordingValidator>,
  recognizer: Arc<CountingRecognizer>,
  _state: tempfile::TempDir,
  _staging: tempfile::TempDir,
}

impl ModelRig {
  fn build(policy: Arc<SwitchedPolicy>, voice_model: bool) -> Self {
    Self::with_bundle(policy, voice_model, staged_bundle)
  }

  fn with_bundle(policy: Arc<SwitchedPolicy>, voice_model: bool, make: fn(&Path) -> PathBuf) -> Self {
    let staging = tempfile::tempdir().expect("a scratch directory");
    let state = tempfile::tempdir().expect("a scratch directory");
    let fetch = StagedFetch::new(make(staging.path()), staged_weights(staging.path()));
    let runner = Arc::new(PlayRunner::default());
    let validator = Arc::new(RecordingValidator::default());
    let recognizer = Arc::new(CountingRecognizer {
      prepared: AtomicUsize::new(0),
    });
    let heard = Arc::new(Heard::default());

    let backends = CompanionBackends {
      link: None,
      host: Arc::new(RigHost),
      http: Arc::new(Offline),
      ws: Arc::new(Offline),
      secrets: Arc::new(MemorySecrets::default()),
      log: Arc::new(Quiet),
      audio: None,
      volume: None,
      geo: None,
      notifications: None,
      phone: None,
      media_sessions: None,
      speech: Some(recognizer.clone()),
      nlu: Some(runner.clone()),
      apple_music: None,
      image: None,
      model_validator: Some(validator.clone()),
      transfer_policy: Some(policy),
      connectivity: None,
      device_waker: None,
      extensions: None,
    };

    let session = Session::new(
      CompanionConfig {
        host: HostInfo {
          app_name: "rig".into(),
          app_version: "0.0.0".into(),
          os_name: "linux".into(),
          os_version: "0".into(),
          host_identifier: "rig".into(),
        },
        capabilities: CapabilityFlags {
          geo: false,
          notifications: false,
          net_fetch: false,
          net_ws: false,
          audio_tts: false,
          voice_model,
        },
        state_dir: state.path().to_string_lossy().into_owned(),
        cache_dir: state.path().to_string_lossy().into_owned(),
        model_platform: Some(ModelPlatform::Android),
        spotify: None,
      },
      backends,
      heard.clone(),
      fetch.clone(),
    );

    ModelRig {
      session,
      heard,
      fetch,
      runner,
      validator,
      recognizer,
      _state: state,
      _staging: staging,
    }
  }

  async fn await_model_status(&self, status: VoiceModelStatus) -> bool {
    self
      .settle(|rig| {
        rig.heard.events().iter().any(|event| match event {
          SessionEvent::VoiceModelStateChanged { state } => state.status == status,
          _ => false,
        })
      })
      .await
  }

  async fn await_armed(&self) -> bool {
    self.settle(|rig| rig.session.voice_controller().has_model()).await
  }

  fn installed_bundle(&self) -> Option<PathBuf> {
    self.session.voice_models().and_then(|models| models.nlu_bundle())
  }

  fn armed_bundle(&self) -> Option<PathBuf> {
    self.session.voice_controller().armed_bundle().map(PathBuf::from)
  }

  fn staging(&self) -> &Path {
    self._staging.path()
  }

  async fn settle(&self, mut done: impl FnMut(&Self) -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
      if done(self) {
        return true;
      }
      if tokio::time::Instant::now() >= deadline {
        return false;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
  }
}

// MARK: tests

#[tokio::test]
async fn a_started_session_downloads_the_bundle_with_nobody_toggling_anything() {
  let rig = ModelRig::build(SwitchedPolicy::allowing(), true);
  rig.session.start();

  assert!(
    rig.await_model_status(VoiceModelStatus::Ready).await,
    "starting the session is the whole trigger; the model must not wait on a settings switch"
  );
  assert_eq!(rig.fetch.downloads(), 2, "the nlu bundle and the asr weights both land");
  assert_eq!(rig.validator.0.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn the_recognizer_is_prepared_at_start_and_again_once_its_weights_land() {
  let rig = ModelRig::build(SwitchedPolicy::allowing(), true);
  rig.session.start();

  assert!(
    rig
      .settle(|rig| rig.recognizer.prepared.load(Ordering::SeqCst) >= 2)
      .await,
    "nothing called prepare, so the recognizer never installs its own model"
  );
}

#[tokio::test]
async fn a_rotated_bundle_arms_the_voice_controller() {
  let rig = ModelRig::build(SwitchedPolicy::allowing(), true);
  rig.session.start();

  assert!(
    rig.await_armed().await,
    "a bundle that rotated into place must reach the controller, not just the filesystem"
  );
  let resolution = rig
    .session
    .voice_controller()
    .resolve(CONTENT_UTTERANCE)
    .await
    .expect("the armed model answers");
  assert_eq!(resolution.stage, NluStage::Model);
  assert_eq!(resolution.resolved.intent, "PLAY");
  assert_eq!(rig.runner.predictions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_content_turn_answers_no_model_while_no_bundle_is_installed() {
  let rig = ModelRig::build(SwitchedPolicy::metered(), true);
  rig.session.start();

  let before = rig
    .session
    .voice_controller()
    .resolve(CONTENT_UTTERANCE)
    .await
    .expect("an unarmed controller still answers");
  assert_eq!(before.stage, NluStage::NoModel);
  assert_eq!(before.resolved.intent, intent_catalog::NO_INTENT);
  assert_eq!(rig.runner.predictions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn reaching_wifi_retries_a_download_the_policy_deferred() {
  let policy = SwitchedPolicy::metered();
  let rig = ModelRig::build(policy.clone(), true);
  rig.session.start();

  assert!(
    rig.settle(|rig| rig.fetch.manifest_reads() > 0).await,
    "a metered network still checks the manifest"
  );
  assert_eq!(rig.fetch.downloads(), 0, "a metered network defers the transfer");
  assert!(!rig.session.voice_controller().has_model());
  assert!(
    !rig.session.large_transfers_allowed(),
    "an inspector has to be able to tell a deferral apart from a model nobody asked for"
  );

  policy.reaches_wifi();
  rig.session.ensure_voice_models();

  assert!(rig.await_model_status(VoiceModelStatus::Ready).await);
  assert!(rig.await_armed().await);
  assert!(rig.session.large_transfers_allowed());
}

#[tokio::test]
async fn a_bundle_already_on_disk_arms_without_a_second_download() {
  let policy = SwitchedPolicy::allowing();
  let first = ModelRig::build(policy.clone(), true);
  first.session.start();
  assert!(first.await_armed().await);
  let state_dir = first._state.path().to_path_buf();
  first.session.stop().await;

  let second = ModelRig::build(policy, true);
  copy_tree(&state_dir, second._state.path());
  second.session.start();

  assert!(
    second.await_armed().await,
    "a bundle that survived a restart must arm at start, not only when a download completes"
  );
  assert_eq!(second.fetch.downloads(), 0, "the installed version is already current");
}

#[tokio::test]
async fn turning_voice_understanding_off_disarms_the_controller() {
  let rig = ModelRig::build(SwitchedPolicy::allowing(), true);
  rig.session.start();
  assert!(rig.await_armed().await);

  rig
    .session
    .set_capability_flags(CapabilityFlags {
      geo: false,
      notifications: false,
      net_fetch: false,
      net_ws: false,
      audio_tts: false,
      voice_model: false,
    })
    .await;

  assert!(
    rig.settle(|rig| !rig.session.voice_controller().has_model()).await,
    "the controller must let go of a bundle the capability no longer covers"
  );
  let resolution = rig
    .session
    .voice_controller()
    .resolve(CONTENT_UTTERANCE)
    .await
    .expect("a disarmed controller still answers");
  assert_eq!(resolution.stage, NluStage::NoModel);
}

#[tokio::test]
async fn an_upgrade_keeps_the_old_bundle_armed_until_the_new_one_rotates_in() {
  let rig = ModelRig::build(SwitchedPolicy::allowing(), true);
  rig.session.start();
  assert!(rig.await_armed().await);
  let first = rig.installed_bundle().expect("1.0.0 installed");

  let gate = rig.fetch.hold();
  rig.fetch.publish(
    "nlu",
    "2.0.0",
    "bundle-android.zip",
    write_bundle(rig.staging(), "bundle-android-2.zip", &intent_catalog::SURFACE_NAMES),
  );
  rig.session.ensure_voice_models();

  assert!(
    rig.await_model_status(VoiceModelStatus::Downloading).await,
    "the upgrade started"
  );
  assert!(
    rig.session.voice_controller().has_model(),
    "a download in flight must not disarm a bundle that is still on disk"
  );
  assert_eq!(rig.installed_bundle().as_ref(), Some(&first));
  let resolution = rig
    .session
    .voice_controller()
    .resolve(CONTENT_UTTERANCE)
    .await
    .expect("voice keeps working mid-upgrade");
  assert_eq!(resolution.stage, NluStage::Model);

  rig.fetch.release(&gate);
  assert!(rig.settle(|rig| rig.installed_bundle().as_ref() != Some(&first)).await);
  assert!(rig.await_armed().await);
}

#[tokio::test]
async fn rapid_disable_and_enable_cycles_leave_the_controller_and_the_store_agreeing() {
  let rig = ModelRig::build(SwitchedPolicy::allowing(), true);
  rig.session.start();
  assert!(rig.await_armed().await);

  for round in 0..8 {
    let flags = |voice_model| CapabilityFlags {
      geo: false,
      notifications: false,
      net_fetch: false,
      net_ws: false,
      audio_tts: false,
      voice_model,
    };
    rig.session.set_capability_flags(flags(false)).await;
    assert!(
      rig.settle(|rig| !rig.session.voice_controller().has_model()).await,
      "round {round}: a stale load armed a bundle that was already deleted"
    );
    assert!(rig.installed_bundle().is_none());

    rig.session.set_capability_flags(flags(true)).await;
    assert!(
      rig.await_armed().await,
      "round {round}: the model never came back after re-enabling"
    );
    assert_eq!(
      rig.armed_bundle(),
      rig.installed_bundle(),
      "round {round}: the controller is holding a bundle other than the installed one"
    );
  }

  assert_eq!(
    rig.fetch.downloads(),
    2,
    "the capability switch gates the model; it must never re-fetch 127 MB per toggle"
  );
}

#[tokio::test]
async fn an_explicit_download_installs_over_a_metered_link_the_automatic_one_declines() {
  let rig = ModelRig::build(SwitchedPolicy::metered(), true);
  rig.session.start();

  assert!(
    rig.settle(|rig| rig.fetch.manifest_reads() > 0).await,
    "a metered network still checks the manifest"
  );
  assert_eq!(rig.fetch.downloads(), 0);

  rig.session.download_voice_models();

  assert!(rig.await_model_status(VoiceModelStatus::Ready).await);
  assert!(rig.await_armed().await);
  assert!(
    !rig.session.large_transfers_allowed(),
    "the link is still metered; the user overrode the rule rather than the rule changing"
  );
}

#[tokio::test]
async fn an_install_that_failed_is_retried_the_next_time_the_app_opens() {
  let rig = ModelRig::with_bundle(SwitchedPolicy::allowing(), true, mismatched_bundle);
  rig.session.start();

  assert!(rig.await_model_status(VoiceModelStatus::Failed).await);
  assert!(rig.installed_bundle().is_none());

  rig.fetch.publish(
    "nlu",
    "2.0.0",
    "bundle-android.zip",
    write_bundle(rig.staging(), "bundle-android-2.zip", &intent_catalog::SURFACE_NAMES),
  );
  rig.session.ensure_voice_models();

  assert!(
    rig.await_model_status(VoiceModelStatus::Ready).await,
    "reopening the app must retry an install that never landed"
  );
  assert!(rig.await_armed().await);
}

#[tokio::test]
async fn armed_and_installed_agree_after_a_burst_of_published_versions() {
  let rig = ModelRig::build(SwitchedPolicy::allowing(), true);
  rig.session.start();
  assert!(rig.await_armed().await);

  for version in ["2.0.0", "3.0.0", "4.0.0"] {
    rig.fetch.publish(
      "nlu",
      version,
      "bundle-android.zip",
      write_bundle(
        rig.staging(),
        &format!("bundle-android-{version}.zip"),
        &intent_catalog::SURFACE_NAMES,
      ),
    );
    rig.session.ensure_voice_models();
  }

  assert!(
    rig
      .settle(|rig| rig.installed_bundle().is_some_and(|dir| dir.ends_with("4.0.0")))
      .await,
    "the newest version never rotated in"
  );
  assert!(
    rig.settle(|rig| rig.armed_bundle() == rig.installed_bundle()).await,
    "the controller settled on a bundle other than the installed one"
  );
  assert_eq!(
    rig.armed_bundle(),
    rig.installed_bundle(),
    "armed and installed must agree once the rotations stop"
  );
}

#[tokio::test]
async fn a_bundle_whose_head_is_not_the_companion_catalog_never_rotates_in() {
  let rig = ModelRig::with_bundle(SwitchedPolicy::allowing(), true, mismatched_bundle);
  rig.session.start();

  assert!(
    rig.await_model_status(VoiceModelStatus::Failed).await,
    "a bundle the companion cannot dispatch has to fail loudly, not install and quietly answer noModel"
  );
  assert!(rig.installed_bundle().is_none());
  assert!(!rig.session.voice_controller().has_model());
}

fn copy_tree(from: &Path, to: &Path) {
  std::fs::create_dir_all(to).expect("the scratch dir is writable");
  for entry in std::fs::read_dir(from)
    .expect("the installed tree is readable")
    .flatten()
  {
    let target = to.join(entry.file_name());
    if entry.path().is_dir() {
      copy_tree(&entry.path(), &target);
    } else {
      std::fs::copy(entry.path(), target).expect("a file copies");
    }
  }
}
