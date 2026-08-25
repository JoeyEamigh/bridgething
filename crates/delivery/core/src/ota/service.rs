use std::{
  collections::{BTreeMap, BTreeSet},
  path::PathBuf,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  time::{Duration, SystemTime},
};

use bridgething_gateway::{Gateway, HandlerError, OutboundLink, Reply, RequestFailure};
use bridgething_sdk_runtime::rt;
use futures::{future::Either, pin_mut};
use libbridgething::{
  BridgeThingMeta, OtaError, OtaFinished, OtaKind, OtaPhase, OtaProgress, WebappInfo,
  gateway::{
    OtaActivate, OtaAssetRange, OtaAssetRangeAbandon, OtaAssetRangeRejected, OtaAssetRangeReply, OtaBegin, OtaPatch,
    OtaPatchAlgorithm, TransferAbandon, TransferRef,
  },
};
use tokio::sync::{Notify, broadcast};
use uuid::Uuid;

use crate::{
  bundle::{
    ArtifactDigest,
    fetch::{ArtifactFetch, DownloadRequest, FetchError, fetch_json},
  },
  ota::{
    autopush::{AutoPushSchedule, MIN_POLL_INTERVAL_SECONDS, MIN_RESUME_DELAY_MS},
    event::{OtaPhaseSnapshot, OtaPlanStep, OtaPollEvent, OtaStepKind},
    manifest::{
      OtaArtifactUrls, OtaCompositeVersion, OtaDiscoverManifest, OtaManifestRelease, OtaReleaseArtifacts,
      WAKEWORD_MODEL_FILE,
    },
    poll::{OtaPollConfig, drift, pushable_release, wakeword_drift},
    progress::{OtaRunProgress, ota_progress},
    range::RangeServer,
    rate::RateTracker,
    run_store::{OtaAvailable, OtaPollStatus, OtaRun, OtaRunStore, OtaStoreChange},
    stream::{Artifact, ArtifactStreamer, FileSource},
    watchdog::{IDLE_DEADLINE_MS, IDLE_POLL_MS, ProgressClock, stalled_reason},
  },
  seam::Clock,
  transfer::{AckWindow, SourceRange},
  webapp::BUILTIN_WEBAPPS,
};

pub const IMAGE_SWU_ASSET: &str = "update.swu";
pub const SYSTEM_ZCK_ASSET: &str = "system.img.zck";
pub const BOOT_ZCK_ASSET: &str = "boot.vfat.zck";

pub const ABANDONED_REASON: &str = "update ended without reporting a result";
pub const LINK_LOST_REASON: &str = "the link to the device dropped mid-update";

const RESUME_READY_DEADLINE_MS: u64 = 60_000;
const RESUME_READY_POLL_MS: u64 = 250;

pub const ACK_TIMEOUT_MS: u64 = 15_000;

pub const CACHE_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

type ProgressSink = Arc<dyn Fn(OtaPhaseSnapshot) + Send + Sync>;

fn direct_sink(feed: Arc<Feed>, device_id: &str, kind: OtaKind) -> ProgressSink {
  let device_id = device_id.to_owned();
  Arc::new(move |snapshot| {
    feed.emit(OtaPollEvent::Progress {
      device_id: device_id.clone(),
      kind,
      step_id: 0,
      snapshot,
    });
  })
}

fn failed(reason: impl Into<String>) -> OtaPhaseSnapshot {
  OtaPhaseSnapshot::Failed { reason: reason.into() }
}

pub fn image_plan(artifacts: Option<&OtaReleaseArtifacts>) -> Vec<OtaPlanStep> {
  let sized = |digest: Option<&ArtifactDigest>| digest.map_or(0, |digest| digest.size);
  let swu = sized(artifacts.and_then(|artifacts| artifacts.image_swu.as_ref()));
  let zck = sized(artifacts.and_then(|artifacts| artifacts.image_zck.as_ref()));
  let boot = sized(artifacts.and_then(|artifacts| artifacts.image_boot_zck.as_ref()));

  vec![
    step(0, OtaStepKind::Download, IMAGE_SWU_ASSET, swu),
    step(1, OtaStepKind::Download, SYSTEM_ZCK_ASSET, zck),
    step(2, OtaStepKind::Download, BOOT_ZCK_ASSET, boot),
    step(3, OtaStepKind::Stream, IMAGE_SWU_ASSET, swu),
    step(4, OtaStepKind::Apply, "installing image", zck),
    step(5, OtaStepKind::Reboot, "reboot", 0),
  ]
}

pub fn bandaid_plan(pieces: &[(String, u64)]) -> Vec<OtaPlanStep> {
  let mut steps = Vec::with_capacity(pieces.len() * 2 + 2);
  for (label, bytes) in pieces {
    steps.push(step(steps.len() as u32, OtaStepKind::Download, label, *bytes));
  }
  for (label, bytes) in pieces {
    steps.push(step(steps.len() as u32, OtaStepKind::Stream, label, *bytes));
  }
  steps.push(step(steps.len() as u32, OtaStepKind::Apply, "installing", 0));
  steps.push(step(steps.len() as u32, OtaStepKind::Reboot, "reboot", 0));
  steps
}

fn step(id: u32, kind: OtaStepKind, label: &str, bytes: u64) -> OtaPlanStep {
  OtaPlanStep {
    id,
    kind,
    label: label.to_owned(),
    bytes,
  }
}

pub fn route_step(plan: &[OtaPlanStep], cursor: usize, snapshot: &OtaPhaseSnapshot) -> usize {
  let held = cursor.min(plan.len().saturating_sub(1));
  let mut ahead = plan.iter().skip(cursor);

  let found = match snapshot {
    OtaPhaseSnapshot::Downloading { asset, .. } => {
      ahead.position(|step| step.kind == OtaStepKind::Download && &step.label == asset)
    }
    OtaPhaseSnapshot::Streaming { asset, .. } => {
      ahead.position(|step| step.kind == OtaStepKind::Stream && &step.label == asset)
    }
    OtaPhaseSnapshot::Applying { phase, .. } => {
      let want = match phase {
        OtaPhase::Reboot => OtaStepKind::Reboot,
        _ => OtaStepKind::Apply,
      };
      ahead.position(|step| step.kind == want)
    }
    _ => return held,
  };

  found.map_or(held, |at| cursor + at)
}

pub struct OtaServiceDeps {
  pub clock: Arc<dyn Clock>,
  pub fetch: Arc<dyn ArtifactFetch>,
  pub cache_dir: PathBuf,
  pub data_dir: Option<PathBuf>,
}

struct Feed {
  store: Mutex<OtaRunStore>,
  events: broadcast::Sender<OtaPollEvent>,
  store_changes: broadcast::Sender<OtaStoreChange>,
  identities: Mutex<BTreeMap<String, String>>,
}

impl Feed {
  fn emit(&self, event: OtaPollEvent) {
    let identity = event
      .device_id()
      .and_then(|device_id| self.identities.lock().unwrap().get(device_id).cloned());
    for change in self.store.lock().unwrap().ingest(event.clone(), identity.as_deref()) {
      let _ = self.store_changes.send(change);
    }
    let _ = self.events.send(event);
  }

  fn retract_available(&self, device_id: &str) {
    let cleared = self.store.lock().unwrap().clear_available(device_id);
    if let Some(change) = cleared {
      let _ = self.store_changes.send(change);
    }
  }
}

#[derive(Debug, Clone)]
pub enum OtaSignal {
  LinkLost,
  Progress(OtaProgress),
  Error(Box<OtaError>),
  Finished(OtaFinished),
  WebappInstalled(Box<WebappInfo>),
}

struct Link {
  gateway: Gateway,
  range: Arc<RangeServer>,
  streamer: ArtifactStreamer,
  signals: broadcast::Sender<OtaSignal>,
  meta: Option<BridgeThingMeta>,
}

struct LinkHandle {
  gateway: Gateway,
  streamer: ArtifactStreamer,
  signals: broadcast::Sender<OtaSignal>,
}

#[derive(Default)]
struct PollState {
  config: Option<OtaPollConfig>,
  generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriveMode {
  Full,
  Stage,
  Install,
  Apply(OtaKind),
}

fn stages_for_activate(kind: OtaKind) -> bool {
  match kind {
    OtaKind::Image | OtaKind::Daemon | OtaKind::BuiltinWebapp => true,
    OtaKind::WakewordModel | OtaKind::InstalledWebapp => false,
  }
}

struct DriveOutcome {
  terminal: OtaPhaseSnapshot,
  update_id: String,
  installed: Option<WebappInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WebappInstallResult {
  Installed(Box<WebappInfo>),
  Failed { reason: String },
}

pub const IN_FLIGHT_REASON: &str = "another update is already in flight for this device";

#[derive(Debug, Clone)]
struct DaemonPatchPlan {
  url: String,
  filename: String,
  digest: ArtifactDigest,
  source_sha256: Option<String>,
  result_sha256: String,
  result_size: u64,
  algorithm: OtaPatchAlgorithm,
}

#[derive(Debug, Clone)]
struct BandaidPiece {
  kind: OtaKind,
  url: String,
  filename: String,
  version: String,
  asset_label: String,
  expected: Option<ArtifactDigest>,
  patch: Option<DaemonPatchPlan>,
}

pub struct BandaidArtifact {
  pub kind: OtaKind,
  pub artifact: Arc<dyn Artifact>,
  pub label: String,
  pub patch: Option<OtaPatch>,
  pub version: Option<String>,
}

fn wakeword_piece(
  root_url: &str,
  channel: &str,
  version: &str,
  artifacts: Option<&OtaReleaseArtifacts>,
) -> BandaidPiece {
  BandaidPiece {
    kind: OtaKind::WakewordModel,
    url: OtaArtifactUrls::wakeword_model(root_url, channel, version),
    filename: format!("wakeword-{channel}-{version}-{WAKEWORD_MODEL_FILE}"),
    version: version.to_owned(),
    asset_label: "wake word model".into(),
    expected: artifacts
      .and_then(|artifacts| artifacts.wakeword.as_ref())
      .and_then(|wakeword| wakeword.model.clone()),
    patch: None,
  }
}

fn daemon_piece(
  urls: &OtaArtifactUrls,
  root_url: &str,
  channel: &str,
  to_version: &str,
  from_version: &str,
  from_sha256: Option<&str>,
  artifacts: Option<&OtaReleaseArtifacts>,
) -> BandaidPiece {
  let daemon = artifacts.and_then(|artifacts| artifacts.daemon.as_ref());
  let patch = artifacts.and_then(|artifacts| artifacts.daemon_patches.get(from_version));
  let compressed = artifacts.and_then(|artifacts| artifacts.daemon_zst.as_ref());

  let delta = daemon.and_then(|daemon| match patch {
    Some(patch) if crate::ota::manifest::patch_source_matches(patch.source_sha256.as_deref(), from_sha256) => {
      Some(DaemonPatchPlan {
        url: OtaArtifactUrls::daemon_patch(root_url, channel, to_version, from_version),
        filename: format!("daemon-{channel}-{to_version}-from-{from_version}.patch"),
        digest: patch.digest(),
        source_sha256: patch.source_sha256.clone(),
        result_sha256: daemon.sha256.clone(),
        result_size: daemon.size,
        algorithm: OtaPatchAlgorithm::ZstdPatchFrom,
      })
    }
    _ => compressed.map(|compressed| DaemonPatchPlan {
      url: urls.daemon_binary_zst.clone(),
      filename: format!("daemon-{channel}-{to_version}.zst"),
      digest: compressed.clone(),
      source_sha256: None,
      result_sha256: daemon.sha256.clone(),
      result_size: daemon.size,
      algorithm: OtaPatchAlgorithm::Zstd,
    }),
  });

  BandaidPiece {
    kind: OtaKind::Daemon,
    url: urls.daemon_binary.clone(),
    filename: format!("daemon-{channel}-{to_version}"),
    version: to_version.to_owned(),
    asset_label: "daemon".into(),
    expected: daemon.cloned(),
    patch: delta,
  }
}

struct StepRouter {
  feed: Arc<Feed>,
  device_id: String,
  kind: OtaKind,
  plan: Vec<OtaPlanStep>,
  cursor: Mutex<usize>,
}

impl StepRouter {
  fn sink(feed: Arc<Feed>, device_id: &str, kind: OtaKind, plan: Vec<OtaPlanStep>) -> ProgressSink {
    let router = StepRouter {
      feed,
      device_id: device_id.to_owned(),
      kind,
      plan,
      cursor: Mutex::new(0),
    };
    Arc::new(move |snapshot| router.route(snapshot))
  }

  fn route(&self, snapshot: OtaPhaseSnapshot) {
    let step_id = {
      let mut cursor = self.cursor.lock().unwrap();
      *cursor = route_step(&self.plan, *cursor, &snapshot);
      self.plan.get(*cursor).map_or(0, |step| step.id)
    };

    self.feed.emit(OtaPollEvent::Progress {
      device_id: self.device_id.clone(),
      kind: self.kind,
      step_id,
      snapshot,
    });
  }
}

pub struct OtaService {
  deps: OtaServiceDeps,
  feed: Arc<Feed>,
  links: Mutex<BTreeMap<String, Link>>,
  schedules: Mutex<BTreeMap<String, AutoPushSchedule>>,
  recheck_soon: AtomicBool,
  image_targets: Mutex<BTreeMap<String, String>>,
  in_flight: Mutex<BTreeSet<String>>,
  poll: Mutex<PollState>,
  wake: Notify,
}

impl OtaService {
  pub fn new(deps: OtaServiceDeps) -> Arc<Self> {
    let feed = Feed {
      store: Mutex::new(OtaRunStore::new(deps.clock.clone(), deps.data_dir.clone())),
      events: broadcast::channel(256).0,
      store_changes: broadcast::channel(256).0,
      identities: Mutex::new(BTreeMap::new()),
    };

    Arc::new(Self {
      deps,
      feed: Arc::new(feed),
      links: Mutex::new(BTreeMap::new()),
      schedules: Mutex::new(BTreeMap::new()),
      recheck_soon: AtomicBool::new(false),
      image_targets: Mutex::new(BTreeMap::new()),
      in_flight: Mutex::new(BTreeSet::new()),
      poll: Mutex::new(PollState::default()),
      wake: Notify::new(),
    })
  }

  pub fn events(&self) -> broadcast::Receiver<OtaPollEvent> {
    self.feed.events.subscribe()
  }

  pub fn store_changes(&self) -> broadcast::Receiver<OtaStoreChange> {
    self.feed.store_changes.subscribe()
  }

  pub async fn adopt(self: &Arc<Self>, device_id: &str, gateway: Gateway) {
    let acks = Arc::new(AckWindow::new());
    let outbound: Arc<dyn OutboundLink> = Arc::new(gateway.clone());
    let range = RangeServer::new(outbound.clone(), acks.clone(), self.deps.clock.clone());

    let link = Link {
      streamer: ArtifactStreamer::new(outbound, acks, self.deps.clock.clone()),
      gateway,
      range,
      signals: broadcast::channel(256).0,
      meta: None,
    };
    self.links.lock().unwrap().insert(device_id.to_owned(), link);
    self.feed.identities.lock().unwrap().remove(device_id);
    self
      .schedules
      .lock()
      .unwrap()
      .entry(device_id.to_owned())
      .or_default()
      .link_opened(self.deps.clock.unix_millis());

    self.wake.notify_waiters();

    let armed = self
      .feed
      .store
      .lock()
      .unwrap()
      .run(device_id)
      .is_some_and(|run| run.resumable);
    if armed {
      let service = self.clone();
      let device_id = device_id.to_owned();
      rt::spawn(async move { service.resume_interrupted(&device_id).await });
    }
  }

  async fn resume_interrupted(self: Arc<Self>, device_id: &str) {
    rt::sleep(Duration::from_millis(MIN_RESUME_DELAY_MS)).await;
    if !self.await_drivable(device_id).await {
      return;
    }
    let held = self
      .feed
      .store
      .lock()
      .unwrap()
      .run(device_id)
      .and_then(|run| run.identity.clone());
    let live = self.meta(device_id).await.map(|meta| meta.serial_number);
    if let (Some(held), Some(live)) = (held.as_deref(), live.as_deref())
      && held != live
    {
      tracing::info!(
        %device_id,
        "a different device answers at this address than the one the interrupted update was driving; not resuming"
      );
      let _ = self.feed.store.lock().unwrap().take_resume(device_id);
      return;
    }
    let resume = self.feed.store.lock().unwrap().take_resume(device_id);
    let Some(resume) = resume else {
      return;
    };
    tracing::info!(%device_id, version = %resume.version, "resuming the update the link interrupted");
    self
      .apply_version(device_id, &resume.channel, &resume.version, &resume.root_url)
      .await;
  }

  async fn await_drivable(&self, device_id: &str) -> bool {
    let deadline = self.deps.clock.unix_millis() + RESUME_READY_DEADLINE_MS;
    loop {
      if self.link(device_id).is_none() {
        return false;
      }
      let idle = !self.in_flight.lock().unwrap().contains(device_id);
      if idle && self.meta(device_id).await.is_some() {
        return true;
      }
      if self.deps.clock.unix_millis() >= deadline {
        return false;
      }
      rt::sleep(Duration::from_millis(RESUME_READY_POLL_MS)).await;
    }
  }

  pub fn device_meta(&self, device_id: &str, meta: BridgeThingMeta) {
    self.record_meta(device_id, meta);
  }

  pub fn nickname_changed(&self, device_id: &str, nickname: Option<String>) -> Option<BridgeThingMeta> {
    self.record_nickname(device_id, nickname)
  }

  pub fn progress(&self, device_id: &str, tick: OtaProgress) {
    self.signal(device_id, OtaSignal::Progress(tick));
  }

  pub fn error(&self, device_id: &str, error: OtaError) {
    self.signal(device_id, OtaSignal::Error(Box::new(error)));
  }

  pub fn finished(&self, device_id: &str, finished: OtaFinished) {
    tracing::debug!(%device_id, ?finished, "the device reported an update result");
    self.signal(device_id, OtaSignal::Finished(finished));
  }

  pub fn webapp_installed(&self, device_id: &str, info: WebappInfo) {
    self.signal(device_id, OtaSignal::WebappInstalled(Box::new(info)));
  }

  pub fn transfer_ack(&self, device_id: &str, transfer_id: Uuid, received: u64) {
    if let Some(link) = self.links.lock().unwrap().get(device_id) {
      link.streamer.acks().note(transfer_id, received);
    }
  }

  pub async fn asset_range(
    &self,
    device_id: &str,
    id: Uuid,
    request: OtaAssetRange,
  ) -> Result<Reply<OtaAssetRangeReply>, HandlerError<OtaAssetRangeRejected>> {
    let range = self.links.lock().unwrap().get(device_id).map(|link| link.range.clone());
    match range {
      Some(range) => range.answer(id, request).await,
      None => Err(HandlerError::Domain(OtaAssetRangeRejected {
        reason: format!("no link to {device_id}"),
      })),
    }
  }

  pub fn asset_range_abandon(&self, device_id: &str, payload: OtaAssetRangeAbandon) {
    tracing::debug!(%device_id, ?payload, "the device abandoned a range it asked for");
  }

  fn signal(&self, device_id: &str, signal: OtaSignal) {
    if let Some(link) = self.links.lock().unwrap().get(device_id) {
      let _ = link.signals.send(signal);
    }
  }

  pub async fn release(&self, device_id: &str) {
    let signals = {
      let mut links = self.links.lock().unwrap();
      match links.remove(device_id) {
        Some(link) => link.signals,
        None => return,
      }
    };
    {
      let mut schedules = self.schedules.lock().unwrap();
      match schedules.get_mut(device_id) {
        Some(schedule) if schedule.failures() == 0 && schedule.next_at_ms().is_none() => {
          schedules.remove(device_id);
        }
        Some(schedule) => schedule.link_closed(),
        None => {}
      }
    }
    let interrupted = self.feed.store.lock().unwrap().interrupt(device_id);
    if let Some(run) = interrupted {
      let _ = self.feed.store_changes.send(OtaStoreChange::Run(Box::new(run)));
    }
    let _ = signals.send(OtaSignal::LinkLost);
  }

  pub async fn meta(&self, device_id: &str) -> Option<BridgeThingMeta> {
    self
      .links
      .lock()
      .unwrap()
      .get(device_id)
      .and_then(|link| link.meta.clone())
  }

  pub async fn set_poll_config(self: &Arc<Self>, config: Option<OtaPollConfig>) {
    let generation = {
      let mut poll = self.poll.lock().unwrap();
      poll.generation += 1;
      poll.config = config.clone();
      poll.generation
    };
    self.wake.notify_waiters();

    if config.is_some() {
      let service = self.clone();
      rt::spawn(async move { service.run_poll_loop(generation).await });
    }
  }

  pub async fn poll_now(&self) {
    let config = self.poll.lock().unwrap().config.clone();
    if let Some(config) = config {
      self.poll(&config).await;
    }
  }

  pub async fn check_now(&self, root_url: &str) {
    self
      .poll(&OtaPollConfig {
        root_url: root_url.to_owned(),
        auto_push: false,
        ..Default::default()
      })
      .await;
  }

  pub async fn apply_version(&self, device_id: &str, channel: &str, version: &str, root_url: &str) {
    if self.link(device_id).is_none() {
      self.fail(device_id, OtaKind::Image, "no link to the device");
      return;
    }
    let Some(composite) = OtaCompositeVersion::parse(version) else {
      self.fail(
        device_id,
        OtaKind::Image,
        format!("'{version}' is not a composite version"),
      );
      return;
    };
    let Some(meta) = self.meta(device_id).await else {
      self.fail(device_id, OtaKind::Image, "device meta not yet known");
      return;
    };
    if self.in_flight.lock().unwrap().contains(device_id) {
      self.recheck_soon.store(true, Ordering::SeqCst);
      return;
    }

    let config = OtaPollConfig {
      root_url: root_url.to_owned(),
      ..Default::default()
    };
    let release = self
      .discover_manifest(root_url)
      .await
      .ok()
      .and_then(|manifest| manifest.releases.get(version).cloned());
    let artifacts = release.as_ref().and_then(|release| release.artifacts.clone());
    let urls = OtaArtifactUrls::build(
      root_url,
      channel,
      &composite.daemon,
      &composite.image,
      &meta.image_variant,
    );

    if meta.image_version != composite.image {
      self
        .run_image_auto(device_id, channel, &composite, &urls, artifacts.as_ref(), &config)
        .await;
      return;
    }
    let mut batch = Vec::new();
    if meta.app_version != composite.daemon {
      batch.push(daemon_piece(
        &urls,
        root_url,
        channel,
        &composite.daemon,
        &meta.app_version,
        meta.daemon_sha256.as_deref(),
        artifacts.as_ref(),
      ));
    }
    if let Some(wanted) = wakeword_drift(&meta, release.as_ref(), &composite.daemon) {
      batch.push(wakeword_piece(root_url, channel, &wanted, artifacts.as_ref()));
    }
    self
      .run_bandaid_batch_auto(device_id, batch, &composite, channel, root_url)
      .await;
  }

  pub async fn retained_runs(&self) -> Vec<OtaRun> {
    self.feed.store.lock().unwrap().runs().into_iter().cloned().collect()
  }

  pub fn run_progress(&self, device_id: &str, now_ms: u64) -> Option<OtaRunProgress> {
    self
      .feed
      .store
      .lock()
      .unwrap()
      .run(device_id)
      .map(|run| ota_progress(run, now_ms))
  }

  pub async fn retained_available(&self) -> Vec<OtaAvailable> {
    self
      .feed
      .store
      .lock()
      .unwrap()
      .available()
      .into_iter()
      .cloned()
      .collect()
  }

  pub async fn retained_poll_status(&self) -> OtaPollStatus {
    self.feed.store.lock().unwrap().poll_status().clone()
  }

  pub async fn dismiss_run(&self, device_id: &str) {
    let cleared = self.feed.store.lock().unwrap().dismiss(device_id);
    if let Some(run) = cleared {
      let _ = self.feed.store_changes.send(OtaStoreChange::Run(Box::new(run)));
    }
  }

  pub async fn push_update(
    &self,
    device_id: &str,
    swu: Arc<dyn Artifact>,
    zcks: BTreeMap<String, Arc<dyn Artifact>>,
    update_url_base: Option<&str>,
  ) -> OtaPhaseSnapshot {
    let progress = direct_sink(self.feed.clone(), device_id, OtaKind::Image);
    self.push_image(device_id, swu, zcks, update_url_base, &progress).await
  }

  pub async fn push_daemon(
    &self,
    device_id: &str,
    artifact: Arc<dyn Artifact>,
    patch: Option<OtaPatch>,
  ) -> OtaPhaseSnapshot {
    self
      .push_bandaid_batch(
        device_id,
        vec![BandaidArtifact {
          kind: OtaKind::Daemon,
          artifact,
          label: "daemon".into(),
          patch,
          version: None,
        }],
      )
      .await
  }

  pub async fn push_builtin_webapp(&self, device_id: &str, bundle: Arc<dyn Artifact>) -> OtaPhaseSnapshot {
    self
      .push_bandaid_batch(
        device_id,
        vec![BandaidArtifact {
          kind: OtaKind::BuiltinWebapp,
          artifact: bundle,
          label: "webapp".into(),
          patch: None,
          version: None,
        }],
      )
      .await
  }

  pub async fn push_bandaid_batch(&self, device_id: &str, artifacts: Vec<BandaidArtifact>) -> OtaPhaseSnapshot {
    let kind = artifacts.first().map_or(OtaKind::Daemon, |artifact| artifact.kind);
    let progress = direct_sink(self.feed.clone(), device_id, kind);
    self.apply_bandaid_batch(device_id, &artifacts, &progress).await
  }

  pub async fn install_webapp(
    &self,
    device_id: &str,
    bundle: Arc<dyn Artifact>,
    provenance: Option<&str>,
  ) -> WebappInstallResult {
    if !self.try_begin_in_flight(device_id) {
      return WebappInstallResult::Failed {
        reason: IN_FLIGHT_REASON.to_owned(),
      };
    }
    let outcome = self
      .drive(
        device_id,
        OtaKind::InstalledWebapp,
        bundle,
        "webapp",
        None,
        DriveMode::Install,
        None,
        provenance,
        None,
        &direct_sink(self.feed.clone(), device_id, OtaKind::InstalledWebapp),
      )
      .await;
    self.end_in_flight(device_id);

    match (outcome.terminal, outcome.installed) {
      (_, Some(info)) => WebappInstallResult::Installed(Box::new(info)),
      (OtaPhaseSnapshot::Failed { reason }, None) => WebappInstallResult::Failed { reason },
      (other, None) => WebappInstallResult::Failed {
        reason: format!("install ended without a verdict (last phase: {other:?})"),
      },
    }
  }

  fn link(&self, device_id: &str) -> Option<LinkHandle> {
    self.links.lock().unwrap().get(device_id).map(|link| LinkHandle {
      gateway: link.gateway.clone(),
      streamer: link.streamer.clone(),
      signals: link.signals.clone(),
    })
  }

  fn record_meta(&self, device_id: &str, meta: BridgeThingMeta) {
    if !meta.serial_number.is_empty() {
      self
        .feed
        .identities
        .lock()
        .unwrap()
        .insert(device_id.to_owned(), meta.serial_number.clone());
    }
    let reached = self
      .image_targets
      .lock()
      .unwrap()
      .get(device_id)
      .is_some_and(|target| *target == meta.image_version);

    let first_announce = {
      let mut links = self.links.lock().unwrap();
      let Some(link) = links.get_mut(device_id) else { return };
      let first_announce = link.meta.is_none();
      link.meta = Some(meta.clone());
      first_announce
    };

    if reached {
      self.image_targets.lock().unwrap().remove(device_id);
      self.note_auto_push_result(device_id, false);
      self.feed.emit(OtaPollEvent::Updated {
        device_id: device_id.to_owned(),
        kind: OtaKind::Image,
        version: meta.image_version,
      });
    }
    if first_announce {
      self.wake.notify_waiters();
    }
  }

  fn record_nickname(&self, device_id: &str, nickname: Option<String>) -> Option<BridgeThingMeta> {
    let mut links = self.links.lock().unwrap();
    let meta = links.get_mut(device_id).and_then(|link| link.meta.as_mut())?;
    meta.nickname = nickname;
    Some(meta.clone())
  }

  fn arm_range_server(&self, device_id: &str, zcks: BTreeMap<String, Arc<dyn Artifact>>) {
    if let Some(link) = self.links.lock().unwrap().get(device_id) {
      link.range.set_assets(zcks);
    }
  }

  fn set_image_target(&self, device_id: &str, version: &str) {
    self
      .image_targets
      .lock()
      .unwrap()
      .insert(device_id.to_owned(), version.to_owned());
  }

  fn try_begin_in_flight(&self, device_id: &str) -> bool {
    self.in_flight.lock().unwrap().insert(device_id.to_owned())
  }

  fn end_in_flight(&self, device_id: &str) {
    self.in_flight.lock().unwrap().remove(device_id);
    let open = self.feed.store.lock().unwrap().open_run_kind(device_id);
    if let Some(kind) = open {
      self.fail(device_id, kind, ABANDONED_REASON);
    }
    self.sweep_cache();
  }

  fn sweep_cache(&self) {
    if !self.in_flight.lock().unwrap().is_empty() {
      return;
    }
    let Ok(entries) = std::fs::read_dir(&self.deps.cache_dir) else {
      return;
    };

    let mut spooled: Vec<(Option<SystemTime>, u64, PathBuf)> = entries
      .flatten()
      .filter_map(|entry| {
        let meta = entry.metadata().ok()?;
        meta.is_file().then(|| (meta.modified().ok(), meta.len(), entry.path()))
      })
      .collect();
    let mut held: u64 = spooled.iter().map(|(_, size, _)| size).sum();
    if held <= CACHE_BUDGET_BYTES {
      return;
    }

    spooled.sort_by(|left, right| left.0.cmp(&right.0));
    for (_, size, path) in spooled {
      if held <= CACHE_BUDGET_BYTES {
        break;
      }
      match std::fs::remove_file(&path) {
        Ok(()) => held -= size,
        Err(e) => tracing::warn!(path = %path.display(), %e, "could not evict a spooled artifact"),
      }
    }
  }

  fn fail(&self, device_id: &str, kind: OtaKind, reason: impl Into<String>) {
    self.feed.emit(OtaPollEvent::Failed {
      device_id: device_id.to_owned(),
      kind,
      reason: reason.into(),
    });
  }

  fn auto_push_ready(&self, device_id: &str) -> bool {
    let now = self.deps.clock.unix_millis();
    self
      .schedules
      .lock()
      .unwrap()
      .get(device_id)
      .is_some_and(|schedule| schedule.ready(now))
  }

  fn note_auto_push_result(&self, device_id: &str, push_failed: bool) {
    let now = self.deps.clock.unix_millis();
    let mut schedules = self.schedules.lock().unwrap();
    let Some(schedule) = schedules.get_mut(device_id) else {
      return;
    };
    if push_failed {
      schedule.record_failure(now);
    } else {
      schedule.record_success();
    }
  }

  async fn run_poll_loop(self: Arc<Self>, generation: u64) {
    while let Some(config) = self.current_config(generation) {
      self.poll(&config).await;
      if self.current_config(generation).is_none() {
        return;
      }

      let now = self.deps.clock.unix_millis();
      let deadline = self.wake_deadline(now, config.interval_seconds);
      let sleeping = rt::sleep(Duration::from_millis(deadline.saturating_sub(now)));
      let woken = self.wake.notified();
      pin_mut!(sleeping, woken);
      futures::future::select(sleeping, woken).await;
    }
  }

  fn current_config(&self, generation: u64) -> Option<OtaPollConfig> {
    let poll = self.poll.lock().unwrap();
    (poll.generation == generation).then(|| poll.config.clone()).flatten()
  }

  fn wake_deadline(&self, now: u64, interval_seconds: u64) -> u64 {
    if self.recheck_soon.swap(false, Ordering::SeqCst) {
      return now + MIN_POLL_INTERVAL_SECONDS * 1_000;
    }
    self
      .schedules
      .lock()
      .unwrap()
      .values()
      .map(|schedule| schedule.wake_deadline_ms(now, interval_seconds))
      .min()
      .unwrap_or_else(|| AutoPushSchedule::new().wake_deadline_ms(now, interval_seconds))
  }

  pub async fn discover_manifest(&self, root_url: &str) -> Result<OtaDiscoverManifest, FetchError> {
    let url = format!("{}/manifest.json", root_url.trim_end_matches('/'));
    fetch_json(self.deps.fetch.as_ref(), &url).await
  }

  async fn poll(&self, config: &OtaPollConfig) {
    let manifest = match self.discover_manifest(&config.root_url).await {
      Ok(manifest) => manifest,
      Err(e) => {
        self
          .feed
          .emit(OtaPollEvent::ManifestPollFailed { reason: e.to_string() });
        self.recheck_soon.store(true, Ordering::SeqCst);
        return;
      }
    };
    self.feed.emit(OtaPollEvent::ManifestPolled {
      updated_at: manifest.updated_at.clone(),
    });

    let announced: Vec<(String, BridgeThingMeta)> = self
      .links
      .lock()
      .unwrap()
      .iter()
      .filter_map(|(device_id, link)| link.meta.clone().map(|meta| (device_id.clone(), meta)))
      .collect();

    for (device_id, meta) in announced {
      let Some((latest, release)) = pushable_release(&manifest, &meta.channel) else {
        continue;
      };
      self.reconcile(&device_id, &meta, &latest, release, config).await;
    }
  }

  async fn reconcile(
    &self,
    device_id: &str,
    meta: &BridgeThingMeta,
    latest: &OtaCompositeVersion,
    release: Option<&OtaManifestRelease>,
    config: &OtaPollConfig,
  ) {
    if self.in_flight.lock().unwrap().contains(device_id) {
      self.recheck_soon.store(true, Ordering::SeqCst);
      return;
    }

    let artifacts = release.and_then(|release| release.artifacts.as_ref());
    let urls = OtaArtifactUrls::build(
      &config.root_url,
      &meta.channel,
      &latest.daemon,
      &latest.image,
      &meta.image_variant,
    );

    let webapps = self
      .builtin_webapp_drift(device_id, release, &meta.channel, config)
      .await;
    let webapps_known = webapps.is_some();
    let webapps = webapps.unwrap_or_default();
    let wakeword = wakeword_drift(meta, release, &latest.daemon);
    let drifted = drift(meta, latest);
    if !drifted.any() && webapps.is_empty() && wakeword.is_none() {
      if webapps_known {
        self.feed.retract_available(device_id);
      } else {
        self.recheck_soon.store(true, Ordering::SeqCst);
      }
      return;
    }

    self.feed.emit(OtaPollEvent::UpdateAvailable {
      device_id: device_id.to_owned(),
      release: latest.composite(),
      daemon_version: latest.daemon.clone(),
      image_version: latest.image.clone(),
    });

    if drifted.image {
      if config.auto_push && self.auto_push_ready(device_id) {
        self
          .run_image_auto(device_id, &meta.channel, latest, &urls, artifacts, config)
          .await;
      }
      return;
    }

    let mut batch = Vec::new();
    if drifted.daemon {
      batch.push(daemon_piece(
        &urls,
        &config.root_url,
        &meta.channel,
        &latest.daemon,
        &meta.app_version,
        meta.daemon_sha256.as_deref(),
        artifacts,
      ));
    }
    batch.extend(webapps);
    if let Some(version) = &wakeword {
      batch.push(wakeword_piece(&config.root_url, &meta.channel, version, artifacts));
    }

    if !batch.is_empty() && config.auto_push && self.auto_push_ready(device_id) {
      self
        .run_bandaid_batch_auto(device_id, batch, latest, &meta.channel, &config.root_url)
        .await;
    }
  }

  async fn builtin_webapp_drift(
    &self,
    device_id: &str,
    release: Option<&OtaManifestRelease>,
    channel: &str,
    config: &OtaPollConfig,
  ) -> Option<Vec<BandaidPiece>> {
    let Some(release) = release.filter(|release| !release.builtin_webapps.is_empty()) else {
      return Some(Vec::new());
    };
    let installed = self.installed_webapps(device_id).await?;

    let mut drifted = Vec::new();
    for (slug, id) in BUILTIN_WEBAPPS {
      let Some(available) = release.builtin_webapps.get(slug) else {
        continue;
      };
      let Some(current) = installed.get(&id) else { continue };
      if current == available {
        continue;
      }
      drifted.push(BandaidPiece {
        kind: OtaKind::BuiltinWebapp,
        url: OtaArtifactUrls::builtin_webapp(&config.root_url, channel, slug, available),
        filename: format!("webapp-{channel}-{slug}-{available}"),
        version: available.clone(),
        asset_label: format!("webapp: {slug}"),
        expected: release
          .artifacts
          .as_ref()
          .and_then(|artifacts| artifacts.webapps.get(slug).cloned()),
        patch: None,
      });
    }
    Some(drifted)
  }

  async fn installed_webapps(&self, device_id: &str) -> Option<BTreeMap<Uuid, String>> {
    let link = self.link(device_id)?;
    let list = match link.gateway.webapp().list().await {
      Ok(list) => list,
      Err(err) => {
        tracing::warn!(device_id, ?err, "webapp list failed; holding the ota verdict");
        return None;
      }
    };
    if list.webapps.is_empty() {
      tracing::debug!(device_id, "webapp list came back empty; holding the ota verdict");
      return None;
    }
    Some(
      list
        .webapps
        .into_iter()
        .map(|webapp| (webapp.id, webapp.version))
        .collect(),
    )
  }

  async fn run_image_auto(
    &self,
    device_id: &str,
    channel: &str,
    latest: &OtaCompositeVersion,
    urls: &OtaArtifactUrls,
    artifacts: Option<&OtaReleaseArtifacts>,
    config: &OtaPollConfig,
  ) {
    if !self.try_begin_in_flight(device_id) {
      return;
    }
    self.set_image_target(device_id, &latest.image);

    let plan = image_plan(artifacts);
    self.feed.emit(OtaPollEvent::Planned {
      device_id: device_id.to_owned(),
      kind: OtaKind::Image,
      release: latest.composite(),
      daemon_version: latest.daemon.clone(),
      image_version: latest.image.clone(),
      channel: channel.to_owned(),
      root_url: config.root_url.clone(),
      steps: plan.clone(),
    });
    let progress = StepRouter::sink(self.feed.clone(), device_id, OtaKind::Image, plan);

    let target = &latest.image;
    let pulled = async {
      let swu = self
        .download(
          &urls.image_swu,
          &format!("image-{channel}-{target}.swu"),
          IMAGE_SWU_ASSET,
          artifacts.and_then(|artifacts| artifacts.image_swu.clone()),
          &progress,
        )
        .await?;
      let zck = self
        .download(
          &urls.image_zck,
          &format!("image-{channel}-{target}.zck"),
          SYSTEM_ZCK_ASSET,
          artifacts.and_then(|artifacts| artifacts.image_zck.clone()),
          &progress,
        )
        .await?;
      let boot = self
        .download(
          &urls.image_boot_zck,
          &format!("image-{channel}-{target}-boot.zck"),
          BOOT_ZCK_ASSET,
          artifacts.and_then(|artifacts| artifacts.image_boot_zck.clone()),
          &progress,
        )
        .await?;
      Ok::<_, FetchError>((swu, zck, boot))
    }
    .await;

    let (swu, zck, boot) = match pulled {
      Ok(pulled) => pulled,
      Err(e) => {
        let reason = format!("image download failed: {e}");
        progress(failed(reason.clone()));
        self.fail(device_id, OtaKind::Image, reason);
        self.note_auto_push_result(device_id, true);
        self.end_in_flight(device_id);
        return;
      }
    };

    let zcks: BTreeMap<String, Arc<dyn Artifact>> = BTreeMap::from([
      (
        SYSTEM_ZCK_ASSET.to_owned(),
        Arc::new(FileSource::open(zck)) as Arc<dyn Artifact>,
      ),
      (
        BOOT_ZCK_ASSET.to_owned(),
        Arc::new(FileSource::open(boot)) as Arc<dyn Artifact>,
      ),
    ]);
    let terminal = self
      .push_image(
        device_id,
        Arc::new(FileSource::open(swu)),
        zcks,
        Some(&config.root_url),
        &progress,
      )
      .await;

    self.emit_terminal(device_id, OtaKind::Image, target, &terminal);
    self.note_auto_push_result(device_id, matches!(terminal, OtaPhaseSnapshot::Failed { .. }));
    self.end_in_flight(device_id);
  }

  async fn run_bandaid_batch_auto(
    &self,
    device_id: &str,
    pieces: Vec<BandaidPiece>,
    latest: &OtaCompositeVersion,
    channel: &str,
    root_url: &str,
  ) {
    if pieces.is_empty() || !self.try_begin_in_flight(device_id) {
      return;
    }

    let kind = if pieces.iter().any(|piece| piece.kind == OtaKind::Daemon) {
      OtaKind::Daemon
    } else {
      pieces.first().map_or(OtaKind::BuiltinWebapp, |piece| piece.kind)
    };
    let weights: Vec<(String, u64)> = pieces
      .iter()
      .map(|piece| {
        (
          piece.asset_label.clone(),
          piece.expected.as_ref().map_or(0, |digest| digest.size),
        )
      })
      .collect();
    let plan = bandaid_plan(&weights);

    self.feed.emit(OtaPollEvent::Planned {
      device_id: device_id.to_owned(),
      kind,
      release: latest.composite(),
      daemon_version: latest.daemon.clone(),
      image_version: latest.image.clone(),
      channel: channel.to_owned(),
      root_url: root_url.to_owned(),
      steps: plan.clone(),
    });
    let progress = StepRouter::sink(self.feed.clone(), device_id, kind, plan);

    let mut terminal = self.bandaid_attempt(device_id, &pieces, true, &progress).await;
    if matches!(terminal, OtaPhaseSnapshot::Failed { .. }) && pieces.iter().any(|piece| piece.patch.is_some()) {
      terminal = self.bandaid_attempt(device_id, &pieces, false, &progress).await;
    }

    match terminal {
      OtaPhaseSnapshot::Failed { reason } => {
        self.fail(device_id, kind, reason);
        self.note_auto_push_result(device_id, true);
      }
      _ => {
        for piece in &pieces {
          self.feed.emit(OtaPollEvent::Updated {
            device_id: device_id.to_owned(),
            kind: piece.kind,
            version: piece.version.clone(),
          });
        }
        self.note_auto_push_result(device_id, false);
      }
    }
    self.end_in_flight(device_id);
  }

  async fn bandaid_attempt(
    &self,
    device_id: &str,
    pieces: &[BandaidPiece],
    use_patch: bool,
    progress: &ProgressSink,
  ) -> OtaPhaseSnapshot {
    let mut artifacts = Vec::with_capacity(pieces.len());
    for piece in pieces {
      let delta = piece.patch.as_ref().filter(|_| use_patch);
      let (url, filename, expected) = match delta {
        Some(delta) => (delta.url.clone(), delta.filename.clone(), Some(delta.digest.clone())),
        None => (piece.url.clone(), piece.filename.clone(), piece.expected.clone()),
      };

      match self
        .download(&url, &filename, &piece.asset_label, expected, progress)
        .await
      {
        Ok(path) => artifacts.push(BandaidArtifact {
          kind: piece.kind,
          artifact: Arc::new(FileSource::open(path)),
          label: piece.asset_label.clone(),
          version: Some(piece.version.clone()),
          patch: delta.map(|delta| OtaPatch {
            algorithm: delta.algorithm,
            result_sha256: delta.result_sha256.clone(),
            result_size: delta.result_size as u32,
            source_sha256: delta.source_sha256.clone(),
          }),
        }),
        Err(e) => {
          let reason = format!("bandaid download failed: {e}");
          progress(failed(reason.clone()));
          return failed(reason);
        }
      }
    }

    self.apply_bandaid_batch(device_id, &artifacts, progress).await
  }

  async fn download(
    &self,
    url: &str,
    filename: &str,
    asset: &str,
    expected: Option<ArtifactDigest>,
    progress: &ProgressSink,
  ) -> Result<PathBuf, FetchError> {
    let known_total = expected.as_ref().map_or(0, |digest| digest.size);
    progress(OtaPhaseSnapshot::Downloading {
      asset: asset.to_owned(),
      received: 0,
      total: known_total,
      rate_per_sec: None,
    });

    let tracker = Mutex::new(RateTracker::new(self.deps.clock.clone()));
    let sink = progress.clone();
    let label = asset.to_owned();
    let ticking: Arc<dyn Fn(u64, u64) + Send + Sync> = Arc::new(move |received, reported| {
      let rate_per_sec = {
        let mut tracker = tracker.lock().unwrap();
        tracker.record(received);
        tracker.rate_per_sec()
      };
      sink(OtaPhaseSnapshot::Downloading {
        asset: label.clone(),
        received,
        total: if known_total > 0 { known_total } else { reported },
        rate_per_sec,
      });
    });

    self
      .deps
      .fetch
      .download(DownloadRequest {
        url: url.to_owned(),
        dir: self.deps.cache_dir.clone(),
        filename: filename.to_owned(),
        asset: asset.to_owned(),
        expected,
        progress: Some(ticking),
      })
      .await
  }

  fn emit_terminal(&self, device_id: &str, kind: OtaKind, version: &str, terminal: &OtaPhaseSnapshot) {
    if kind == OtaKind::Image {
      let claimed = self.image_targets.lock().unwrap().remove(device_id).is_some();
      if !claimed {
        return;
      }
    }

    match terminal {
      OtaPhaseSnapshot::Completed | OtaPhaseSnapshot::Staged => self.feed.emit(OtaPollEvent::Updated {
        device_id: device_id.to_owned(),
        kind,
        version: version.to_owned(),
      }),
      OtaPhaseSnapshot::Failed { reason } => self.fail(device_id, kind, reason.clone()),
      other => self.fail(
        device_id,
        kind,
        format!("update ended before completing (last phase: {other:?})"),
      ),
    }
  }

  async fn push_image(
    &self,
    device_id: &str,
    swu: Arc<dyn Artifact>,
    zcks: BTreeMap<String, Arc<dyn Artifact>>,
    update_url_base: Option<&str>,
    progress: &ProgressSink,
  ) -> OtaPhaseSnapshot {
    self.arm_range_server(device_id, zcks);
    self
      .drive(
        device_id,
        OtaKind::Image,
        swu,
        IMAGE_SWU_ASSET,
        update_url_base,
        DriveMode::Full,
        None,
        None,
        None,
        progress,
      )
      .await
      .terminal
  }

  async fn apply_bandaid_batch(
    &self,
    device_id: &str,
    artifacts: &[BandaidArtifact],
    progress: &ProgressSink,
  ) -> OtaPhaseSnapshot {
    let mut staged = Vec::with_capacity(artifacts.len());
    let mut applied = false;
    for artifact in artifacts {
      let stages = stages_for_activate(artifact.kind);
      let mode = if stages {
        DriveMode::Stage
      } else {
        DriveMode::Apply(artifact.kind)
      };
      let outcome = self
        .drive(
          device_id,
          artifact.kind,
          artifact.artifact.clone(),
          &artifact.label,
          None,
          mode,
          artifact.patch.clone(),
          None,
          artifact.version.as_deref(),
          progress,
        )
        .await;
      match (stages, &outcome.terminal) {
        (true, OtaPhaseSnapshot::Staged) => staged.push(outcome.update_id),
        (false, OtaPhaseSnapshot::Completed) => applied = true,
        _ => return outcome.terminal,
      }
    }

    if staged.is_empty() && applied {
      return OtaPhaseSnapshot::Completed;
    }
    self.commit_bandaid(device_id, staged, progress).await
  }

  async fn commit_bandaid(&self, device_id: &str, expected: Vec<String>, progress: &ProgressSink) -> OtaPhaseSnapshot {
    let Some(link) = self.link(device_id) else {
      return failed("no link to the device");
    };

    let watching = self.watch_terminal(&link, DriveMode::Full, progress);
    if let Err(e) = link.gateway.system().ota_activate(OtaActivate { expected }).await {
      return failed(format!("OtaActivate send failed: {e}"));
    }
    watching.await.0
  }

  #[allow(clippy::too_many_arguments)]
  async fn drive(
    &self,
    device_id: &str,
    kind: OtaKind,
    artifact: Arc<dyn Artifact>,
    label: &str,
    update_url_base: Option<&str>,
    mode: DriveMode,
    patch: Option<OtaPatch>,
    provenance: Option<&str>,
    version: Option<&str>,
    progress: &ProgressSink,
  ) -> DriveOutcome {
    let abandoned = |terminal, update_id: &str| DriveOutcome {
      terminal,
      update_id: update_id.to_owned(),
      installed: None,
    };

    let Some(link) = self.link(device_id) else {
      return abandoned(failed("no link to the device"), "");
    };

    let total = artifact.size().unwrap_or(0);
    if total == 0 {
      return abandoned(failed("could not size artifact"), "");
    }
    let Ok(total_size) = u32::try_from(total) else {
      return abandoned(failed("artifact larger than 4 GiB"), "");
    };
    let hashed = {
      let artifact = artifact.clone();
      rt::spawn_blocking(move || artifact.sha256()).await
    };
    let update_id = match hashed {
      Ok(digest) => digest,
      Err(e) => return abandoned(failed(format!("sha256 failed: {e}")), ""),
    };

    let transfer_id = Uuid::now_v7();
    let begin = OtaBegin {
      kind,
      update_id: update_id.clone(),
      update_url_base: update_url_base.map(str::to_owned),
      transfer: TransferRef {
        id: transfer_id,
        total_size,
        sha256: Some(update_id.clone()),
      },
      patch,
      provenance: provenance.map(str::to_owned),
      version: version.map(str::to_owned),
    };

    let watching = self.watch_terminal(&link, mode, progress);
    let resume = match link.gateway.system().ota_begin(begin).await {
      Ok(ack) => u64::from(ack.resume_from_offset),
      Err(RequestFailure::Domain(rejected)) => {
        return abandoned(
          failed(format!("daemon rejected OtaBegin: {}", rejected.reason)),
          &update_id,
        );
      }
      Err(other) => return abandoned(failed(format!("OtaBegin protocol error: {other:?}")), &update_id),
    };

    progress(OtaPhaseSnapshot::Streaming {
      asset: label.to_owned(),
      sent: resume,
      total,
      rate_per_sec: None,
      eta_seconds: None,
    });

    let ranges = [SourceRange {
      start: 0,
      length: total,
    }];
    let streaming = async {
      let pushed = link
        .streamer
        .stream(
          transfer_id,
          artifact.as_ref(),
          label,
          &ranges,
          resume,
          Duration::from_millis(ACK_TIMEOUT_MS),
          progress.as_ref(),
        )
        .await;
      pushed.err().map(|e| failed(format!("chunk stream failed: {e}")))
    };

    let (terminal, installed) = {
      pin_mut!(watching, streaming);
      match futures::future::select(watching, streaming).await {
        Either::Left((terminal, _)) => terminal,
        Either::Right((Some(broken), _)) => (broken, None),
        Either::Right((None, watching)) => watching.await,
      }
    };

    link.streamer.acks().finish(transfer_id);
    if matches!(terminal, OtaPhaseSnapshot::Failed { .. }) {
      let _ = link
        .gateway
        .transfer()
        .abandon(TransferAbandon {
          transfer_id,
          reason: "attempt ended".into(),
        })
        .await;
    }

    DriveOutcome {
      terminal,
      update_id,
      installed,
    }
  }

  fn watch_terminal<'a>(
    &self,
    link: &LinkHandle,
    mode: DriveMode,
    progress: &'a ProgressSink,
  ) -> impl Future<Output = (OtaPhaseSnapshot, Option<WebappInfo>)> + 'a {
    let mut inbound = link.signals.subscribe();
    let clock = self.deps.clock.clone();

    async move {
      let quiet_since = ProgressClock::new(clock);
      let success = match mode {
        DriveMode::Full | DriveMode::Install | DriveMode::Apply(_) => OtaPhaseSnapshot::Completed,
        DriveMode::Stage => OtaPhaseSnapshot::Staged,
      };

      let reading = async {
        loop {
          let signal = match inbound.recv().await {
            Ok(signal) => signal,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return (failed("ota ended without a terminal"), None),
          };

          match signal {
            OtaSignal::LinkLost => return (failed(LINK_LOST_REASON), None),
            OtaSignal::WebappInstalled(info) if mode == DriveMode::Install => return (success, Some(*info)),
            OtaSignal::WebappInstalled(_) => continue,
            OtaSignal::Finished(done) if mode == DriveMode::Apply(done.kind) => return (success, None),
            OtaSignal::Finished(_) => continue,
            OtaSignal::Progress(tick) => {
              quiet_since.touch();
              progress(OtaPhaseSnapshot::Applying {
                phase: tick.phase,
                write_percent: u32::from(tick.percent.min(100)),
                dwl_percent: u32::from(tick.dwl_percent.min(100)),
                dwl_bytes: u64::from(tick.dwl_bytes),
              });

              let done = match mode {
                DriveMode::Full => tick.phase == OtaPhase::Reboot,
                DriveMode::Stage => tick.phase == OtaPhase::Writing && tick.percent >= 100,
                DriveMode::Install | DriveMode::Apply(_) => false,
              };
              if done {
                return (success, None);
              }
            }
            OtaSignal::Error(err) if err.replayed => continue,
            OtaSignal::Error(err) => {
              return (failed(format!("[{:?}] {}", err.code, err.msg)), None);
            }
          }
        }
      };

      let stalling = async {
        loop {
          rt::sleep(Duration::from_millis(IDLE_POLL_MS)).await;
          if quiet_since.idle_ms() > IDLE_DEADLINE_MS {
            return (failed(stalled_reason()), None);
          }
        }
      };

      pin_mut!(reading, stalling);
      match futures::future::select(reading, stalling).await {
        Either::Left((terminal, _)) | Either::Right((terminal, _)) => terminal,
      }
    }
  }
}
#[cfg(test)]
mod tests {
  use std::{collections::BTreeMap, sync::Arc, time::Duration};

  use libbridgething::{
    OtaErrorCode, OtaKind, OtaPhase, RangeSpec, WebappInfo, WebappRole, WebappSource, gateway::OtaPatchAlgorithm,
  };
  use tokio::sync::broadcast;
  use uuid::Uuid;

  use super::{
    BOOT_ZCK_ASSET, BandaidArtifact, CACHE_BUDGET_BYTES, IMAGE_SWU_ASSET, IN_FLIGHT_REASON, OtaService, OtaServiceDeps,
    SYSTEM_ZCK_ASSET, WebappInstallResult, bandaid_plan, daemon_piece, image_plan, route_step,
  };
  use crate::{
    bundle::ArtifactDigest,
    ota::{
      event::{OtaPhaseSnapshot, OtaPlanStep, OtaPollEvent, OtaStepKind},
      harness::{
        DEVICE, FakeDevice, FakeFetch, ManifestFixture, Spool, TestClock, digest_of, linked_gateway, meta, pattern,
        route_into, sha256_hex,
      },
      manifest::{OtaArtifactUrls, OtaPatchDigest, OtaReleaseArtifacts},
      run_store::RUNS_FILE,
      stream::FileSource,
      watchdog::stalled_reason,
    },
  };

  const CHANNEL: &str = "stable";
  const ROOT: &str = "https://ota.example";
  const FROM_VERSION: &str = "0.9.0";
  const TO_RELEASE: &str = "0.9.1+image.1.0.0";

  struct Rig {
    service: Arc<OtaService>,
    spool: Spool,
    _data: Spool,
    device: FakeDevice,
    fetch: Arc<FakeFetch>,
    events: broadcast::Receiver<OtaPollEvent>,
  }

  fn launch(cache_dir: &std::path::Path, data_dir: &std::path::Path, fetch: Arc<FakeFetch>) -> Arc<OtaService> {
    OtaService::new(OtaServiceDeps {
      clock: TestClock::new(),
      fetch,
      cache_dir: cache_dir.to_path_buf(),
      data_dir: Some(data_dir.to_path_buf()),
    })
  }

  async fn rig() -> Rig {
    let spool = Spool::new();
    let data = Spool::new();
    let fetch = FakeFetch::new();
    let service = launch(spool.path(), data.path(), fetch.clone());
    let events = service.events();
    let (gateway, device) = linked_gateway();
    service.adopt(DEVICE, gateway.clone()).await;
    route_into(&gateway, &service, DEVICE);
    Rig {
      service,
      spool,
      _data: data,
      device,
      fetch,
      events,
    }
  }

  fn drain(events: &mut broadcast::Receiver<OtaPollEvent>) -> Vec<OtaPollEvent> {
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
      seen.push(event);
    }
    seen
  }

  async fn stream_through(device: &mut FakeDevice) -> String {
    let (request_id, begin) = device.await_ota_begin().await;
    device.ack_begin(request_id, 0);
    let mut sent = 0u32;
    while sent < begin.transfer.total_size {
      let fragment = device.next_fragment(begin.transfer.id).await;
      sent = fragment.offset + fragment.bytes.len() as u32;
      device.ack(begin.transfer.id, sent);
    }
    begin.update_id
  }

  #[test]
  fn an_image_plan_downloads_three_artifacts_streams_one_and_reboots() {
    let artifacts = OtaReleaseArtifacts {
      image_swu: Some(ArtifactDigest {
        size: 1_000,
        sha256: "swu".into(),
      }),
      image_zck: Some(ArtifactDigest {
        size: 2_000,
        sha256: "zck".into(),
      }),
      image_boot_zck: Some(ArtifactDigest {
        size: 3_000,
        sha256: "boot".into(),
      }),
      ..Default::default()
    };

    let plan = image_plan(Some(&artifacts));

    assert_eq!(
      plan,
      vec![
        OtaPlanStep {
          id: 0,
          kind: OtaStepKind::Download,
          label: IMAGE_SWU_ASSET.into(),
          bytes: 1_000
        },
        OtaPlanStep {
          id: 1,
          kind: OtaStepKind::Download,
          label: SYSTEM_ZCK_ASSET.into(),
          bytes: 2_000
        },
        OtaPlanStep {
          id: 2,
          kind: OtaStepKind::Download,
          label: BOOT_ZCK_ASSET.into(),
          bytes: 3_000
        },
        OtaPlanStep {
          id: 3,
          kind: OtaStepKind::Stream,
          label: IMAGE_SWU_ASSET.into(),
          bytes: 1_000
        },
        OtaPlanStep {
          id: 4,
          kind: OtaStepKind::Apply,
          label: "installing image".into(),
          bytes: 2_000
        },
        OtaPlanStep {
          id: 5,
          kind: OtaStepKind::Reboot,
          label: "reboot".into(),
          bytes: 0
        },
      ]
    );
  }

  #[test]
  fn an_image_plan_with_no_manifest_sizes_still_has_every_step() {
    let plan = image_plan(None);

    assert_eq!(plan.len(), 6);
    assert!(
      plan.iter().take(5).all(|step| step.bytes == 0),
      "an unsized plan weighs nothing but still describes the work"
    );
  }

  #[test]
  fn a_bandaid_plan_downloads_everything_before_it_streams_anything() {
    let plan = bandaid_plan(&[("daemon".into(), 512), ("webapp: hub".into(), 1_024)]);

    assert_eq!(
      plan,
      vec![
        OtaPlanStep {
          id: 0,
          kind: OtaStepKind::Download,
          label: "daemon".into(),
          bytes: 512
        },
        OtaPlanStep {
          id: 1,
          kind: OtaStepKind::Download,
          label: "webapp: hub".into(),
          bytes: 1_024
        },
        OtaPlanStep {
          id: 2,
          kind: OtaStepKind::Stream,
          label: "daemon".into(),
          bytes: 512
        },
        OtaPlanStep {
          id: 3,
          kind: OtaStepKind::Stream,
          label: "webapp: hub".into(),
          bytes: 1_024
        },
        OtaPlanStep {
          id: 4,
          kind: OtaStepKind::Apply,
          label: "installing".into(),
          bytes: 0
        },
        OtaPlanStep {
          id: 5,
          kind: OtaStepKind::Reboot,
          label: "reboot".into(),
          bytes: 0
        },
      ]
    );
  }

  fn two_piece_plan() -> Vec<OtaPlanStep> {
    bandaid_plan(&[("daemon".into(), 512), ("webapp: hub".into(), 1_024)])
  }

  #[test]
  fn a_download_routes_to_the_step_that_names_its_asset() {
    let plan = two_piece_plan();

    let at = route_step(
      &plan,
      0,
      &OtaPhaseSnapshot::Downloading {
        asset: "webapp: hub".into(),
        received: 1,
        total: 1_024,
        rate_per_sec: None,
      },
    );

    assert_eq!(at, 1);
  }

  #[test]
  fn a_stream_routes_past_the_downloads() {
    let plan = two_piece_plan();

    let at = route_step(
      &plan,
      1,
      &OtaPhaseSnapshot::Streaming {
        asset: "daemon".into(),
        sent: 0,
        total: 512,
        rate_per_sec: None,
        eta_seconds: None,
      },
    );

    assert_eq!(at, 2);
  }

  #[test]
  fn an_apply_routes_by_kind_and_a_reboot_routes_to_the_reboot_step() {
    let plan = two_piece_plan();

    let applying = route_step(
      &plan,
      2,
      &OtaPhaseSnapshot::Applying {
        phase: OtaPhase::Writing,
        write_percent: 10,
        dwl_percent: 0,
        dwl_bytes: 0,
      },
    );
    let rebooting = route_step(
      &plan,
      4,
      &OtaPhaseSnapshot::Applying {
        phase: OtaPhase::Reboot,
        write_percent: 100,
        dwl_percent: 100,
        dwl_bytes: 0,
      },
    );

    assert_eq!(applying, 4);
    assert_eq!(rebooting, 5);
  }

  #[test]
  fn routing_never_rewinds_the_cursor() {
    let plan = two_piece_plan();

    let at = route_step(
      &plan,
      3,
      &OtaPhaseSnapshot::Downloading {
        asset: "daemon".into(),
        received: 1,
        total: 512,
        rate_per_sec: None,
      },
    );

    assert_eq!(at, 3, "a late download tick must not drag the bar backwards");
  }

  #[test]
  fn a_terminal_snapshot_holds_the_cursor_where_it_is() {
    let plan = two_piece_plan();

    assert_eq!(route_step(&plan, 3, &OtaPhaseSnapshot::Staged), 3);
    assert_eq!(route_step(&plan, 3, &OtaPhaseSnapshot::Completed), 3);
    assert_eq!(
      route_step(&plan, 3, &OtaPhaseSnapshot::Failed { reason: "nope".into() }),
      3
    );
  }

  fn daemon_urls() -> OtaArtifactUrls {
    OtaArtifactUrls::build(ROOT, CHANNEL, "0.9.1", "1.0.0", "prod")
  }

  #[test]
  fn a_compressed_daemon_rides_as_a_patch_over_the_plain_binary() {
    let plain = pattern(2_048);
    let compressed = pattern(512);
    let artifacts = OtaReleaseArtifacts {
      daemon: Some(digest_of(&plain)),
      daemon_zst: Some(digest_of(&compressed)),
      ..Default::default()
    };
    let urls = daemon_urls();

    let piece = daemon_piece(&urls, ROOT, CHANNEL, "0.9.1", FROM_VERSION, None, Some(&artifacts));

    assert_eq!(
      (piece.url.as_str(), piece.expected.as_ref()),
      (urls.daemon_binary.as_str(), Some(&digest_of(&plain))),
      "the retry that drops the patch installs the piece as it landed, so the piece must name the runnable binary"
    );
    let patch = piece.patch.expect("the compressed artifact is offered as a patch");
    assert_eq!(patch.url, urls.daemon_binary_zst);
    assert_eq!(patch.digest, digest_of(&compressed));
    assert_eq!(patch.algorithm, OtaPatchAlgorithm::Zstd);
    assert_eq!(patch.result_sha256, digest_of(&plain).sha256);
  }

  #[test]
  fn a_usable_delta_is_preferred_over_the_compressed_binary() {
    let plain = pattern(2_048);
    let delta = pattern(64);
    let artifacts = OtaReleaseArtifacts {
      daemon: Some(digest_of(&plain)),
      daemon_zst: Some(digest_of(&pattern(512))),
      daemon_patches: BTreeMap::from([(
        FROM_VERSION.to_string(),
        OtaPatchDigest {
          size: delta.len() as u64,
          sha256: digest_of(&delta).sha256,
          source_sha256: None,
        },
      )]),
      ..Default::default()
    };
    let urls = daemon_urls();

    let piece = daemon_piece(&urls, ROOT, CHANNEL, "0.9.1", FROM_VERSION, None, Some(&artifacts));

    assert_eq!(piece.url, urls.daemon_binary);
    let patch = piece.patch.expect("a matching delta is offered");
    assert_eq!(
      patch.url,
      OtaArtifactUrls::daemon_patch(ROOT, CHANNEL, "0.9.1", FROM_VERSION)
    );
    assert_eq!(patch.algorithm, OtaPatchAlgorithm::ZstdPatchFrom);
  }

  #[test]
  fn a_release_with_neither_a_delta_nor_a_compressed_binary_carries_no_patch() {
    let plain = pattern(2_048);
    let artifacts = OtaReleaseArtifacts {
      daemon: Some(digest_of(&plain)),
      ..Default::default()
    };
    let urls = daemon_urls();

    let piece = daemon_piece(&urls, ROOT, CHANNEL, "0.9.1", FROM_VERSION, None, Some(&artifacts));

    assert_eq!(piece.url, urls.daemon_binary);
    assert!(piece.patch.is_none());
  }

  fn spool_sparse(spool: &Spool, name: &str, size: u64) {
    let file = std::fs::File::create(spool.path().join(name)).expect("the scratch directory is writable");
    file.set_len(size).expect("a stand-in the size of a release artifact");
    std::thread::sleep(Duration::from_millis(20));
  }

  fn spooled(spool: &Spool) -> Vec<String> {
    let mut held: Vec<String> = std::fs::read_dir(spool.path())
      .expect("the scratch directory is readable")
      .flatten()
      .map(|entry| entry.file_name().to_string_lossy().into_owned())
      .collect();
    held.sort();
    held
  }

  #[tokio::test]
  async fn the_spool_drops_its_oldest_artifacts_once_it_outgrows_its_budget() {
    let rig = rig().await;
    for name in ["oldest", "middle", "newest"] {
      spool_sparse(&rig.spool, name, CACHE_BUDGET_BYTES / 2);
    }

    rig.service.sweep_cache();

    assert_eq!(
      spooled(&rig.spool),
      vec!["middle".to_string(), "newest".to_string()],
      "every release spools a fresh set, so without eviction the cache grows until the disk does not"
    );
  }

  #[tokio::test]
  async fn the_spool_is_left_alone_while_an_update_is_still_reading_it() {
    let rig = rig().await;
    for name in ["oldest", "middle", "newest"] {
      spool_sparse(&rig.spool, name, CACHE_BUDGET_BYTES / 2);
    }
    assert!(rig.service.try_begin_in_flight(DEVICE));

    rig.service.sweep_cache();

    assert_eq!(
      spooled(&rig.spool).len(),
      3,
      "a live run streams from the artifacts it downloaded; evicting one mid-update breaks it"
    );
  }

  #[tokio::test]
  async fn a_daemon_bandaid_stages_activates_and_completes_on_reboot() {
    let mut rig = rig().await;
    let body = pattern(40 * 1024);
    let artifact = rig.spool.write("daemon", &body);

    let driving = {
      let service = rig.service.clone();
      let artifact = artifact.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    let update_id = stream_through(&mut rig.device).await;
    assert_eq!(update_id, sha256_hex(&body), "an artifact is announced by its digest");

    rig.device.progress(OtaPhase::Writing, 100);
    let expected = rig.device.await_activate().await;
    assert_eq!(
      expected,
      vec![sha256_hex(&body)],
      "activate names exactly what was staged"
    );
    rig.device.progress(OtaPhase::Reboot, 100);

    let terminal = driving.await.expect("the drive task");
    assert_eq!(terminal, OtaPhaseSnapshot::Completed);
  }

  #[tokio::test]
  async fn a_batch_stages_every_piece_before_it_activates_any() {
    let mut rig = rig().await;
    let first_body = pattern(8 * 1024);
    let second_body = pattern(12 * 1024);
    let first = rig.spool.write("daemon", &first_body);
    let second = rig.spool.write("hub.zip", &second_body);

    let driving = {
      let service = rig.service.clone();
      let pieces = vec![
        BandaidArtifact {
          kind: OtaKind::Daemon,
          artifact: Arc::new(FileSource::open(first)),
          label: "daemon".into(),
          patch: None,
          version: None,
        },
        BandaidArtifact {
          kind: OtaKind::BuiltinWebapp,
          artifact: Arc::new(FileSource::open(second)),
          label: "webapp: hub".into(),
          patch: None,
          version: None,
        },
      ];
      tokio::spawn(async move { service.push_bandaid_batch(DEVICE, pieces).await })
    };

    stream_through(&mut rig.device).await;
    rig.device.progress(OtaPhase::Writing, 100);
    assert!(
      rig.device.no_activate(Duration::from_millis(300)).await,
      "the batch must not activate while a piece is still unstaged"
    );

    stream_through(&mut rig.device).await;
    rig.device.progress(OtaPhase::Writing, 100);
    let expected = rig.device.await_activate().await;
    assert_eq!(
      expected,
      vec![sha256_hex(&first_body), sha256_hex(&second_body)],
      "activate lists the staged ids in order"
    );
    rig.device.progress(OtaPhase::Reboot, 100);

    assert_eq!(driving.await.expect("the drive task"), OtaPhaseSnapshot::Completed);
  }

  #[tokio::test]
  async fn a_piece_that_fails_to_stage_ends_the_batch_without_activating() {
    let mut rig = rig().await;
    let first = rig.spool.write("daemon", &pattern(8 * 1024));
    let second = rig.spool.write("hub.zip", &pattern(12 * 1024));

    let driving = {
      let service = rig.service.clone();
      let pieces = vec![
        BandaidArtifact {
          kind: OtaKind::Daemon,
          artifact: Arc::new(FileSource::open(first)),
          label: "daemon".into(),
          patch: None,
          version: None,
        },
        BandaidArtifact {
          kind: OtaKind::BuiltinWebapp,
          artifact: Arc::new(FileSource::open(second)),
          label: "webapp: hub".into(),
          patch: None,
          version: None,
        },
      ];
      tokio::spawn(async move { service.push_bandaid_batch(DEVICE, pieces).await })
    };

    stream_through(&mut rig.device).await;
    rig.device.ota_error(OtaErrorCode::WriteFailed, "no room");

    let terminal = driving.await.expect("the drive task");
    assert!(
      matches!(terminal, OtaPhaseSnapshot::Failed { ref reason } if reason.contains("no room")),
      "got {terminal:?}"
    );
    assert!(
      rig.device.no_activate(Duration::from_millis(300)).await,
      "a batch that never fully staged must not be committed"
    );
  }

  #[tokio::test]
  async fn a_wakeword_push_completes_on_the_finish_the_device_reports() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("wakeword.btww", &pattern(6 * 1024));

    let driving = {
      let service = rig.service.clone();
      let pieces = vec![BandaidArtifact {
        kind: OtaKind::WakewordModel,
        artifact: Arc::new(FileSource::open(artifact)),
        label: "wakeword".into(),
        patch: None,
        version: Some("1.1.0".into()),
      }];
      tokio::spawn(async move { service.push_bandaid_batch(DEVICE, pieces).await })
    };

    let update_id = stream_through(&mut rig.device).await;
    rig.device.ota_finished(OtaKind::WakewordModel, &update_id);

    assert_eq!(driving.await.expect("the drive task"), OtaPhaseSnapshot::Completed);
    assert!(
      rig.device.no_activate(Duration::from_millis(300)).await,
      "a kind the device applies inline has nothing to activate"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_wakeword_push_the_device_finishes_instantly_never_stalls() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("wakeword.btww", &pattern(4_096));

    let driving = {
      let service = rig.service.clone();
      let pieces = vec![BandaidArtifact {
        kind: OtaKind::WakewordModel,
        artifact: Arc::new(FileSource::open(artifact)),
        label: "wakeword".into(),
        patch: None,
        version: Some("1.1.0".into()),
      }];
      tokio::spawn(async move { service.push_bandaid_batch(DEVICE, pieces).await })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, begin.transfer.total_size);
    rig.device.ota_finished(OtaKind::WakewordModel, &begin.update_id);

    let terminal = tokio::time::timeout(Duration::from_secs(600), driving)
      .await
      .expect("a finish that lands on the heels of the ack must not park the caller")
      .expect("the drive task");

    assert_eq!(terminal, OtaPhaseSnapshot::Completed);
  }

  #[tokio::test]
  async fn a_batch_activates_only_the_pieces_the_device_staged() {
    let mut rig = rig().await;
    let daemon_body = pattern(8 * 1024);
    let daemon = rig.spool.write("daemon", &daemon_body);
    let model = rig.spool.write("wakeword.btww", &pattern(6 * 1024));

    let driving = {
      let service = rig.service.clone();
      let pieces = vec![
        BandaidArtifact {
          kind: OtaKind::WakewordModel,
          artifact: Arc::new(FileSource::open(model)),
          label: "wakeword".into(),
          patch: None,
          version: Some("1.1.0".into()),
        },
        BandaidArtifact {
          kind: OtaKind::Daemon,
          artifact: Arc::new(FileSource::open(daemon)),
          label: "daemon".into(),
          patch: None,
          version: None,
        },
      ];
      tokio::spawn(async move { service.push_bandaid_batch(DEVICE, pieces).await })
    };

    let model_id = stream_through(&mut rig.device).await;
    rig.device.ota_finished(OtaKind::WakewordModel, &model_id);

    stream_through(&mut rig.device).await;
    rig.device.progress(OtaPhase::Writing, 100);

    let expected = rig.device.await_activate().await;
    assert_eq!(
      expected,
      vec![sha256_hex(&daemon_body)],
      "activate names only what the device staged"
    );
    rig.device.progress(OtaPhase::Reboot, 100);

    assert_eq!(driving.await.expect("the drive task"), OtaPhaseSnapshot::Completed);
  }

  #[tokio::test]
  async fn an_ota_error_ends_the_stream_and_abandons_the_transfer() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("daemon", &pattern(512 * 1024));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, 0);
    let fragment = rig.device.next_fragment(begin.transfer.id).await;
    rig
      .device
      .ack(begin.transfer.id, fragment.offset + fragment.bytes.len() as u32);
    rig.device.ota_error(OtaErrorCode::OffsetMismatch, "synthetic");

    let abandon = rig.device.await_abandon(begin.transfer.id).await;
    assert_eq!(abandon.reason, "attempt ended");
    let terminal = driving.await.expect("the drive task");
    assert!(
      matches!(terminal, OtaPhaseSnapshot::Failed { ref reason } if reason.contains("synthetic")),
      "got {terminal:?}"
    );
  }

  #[tokio::test]
  async fn a_rejected_begin_fails_without_moving_a_byte() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("daemon", &pattern(16 * 1024));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.reject_begin(request_id, "slot busy");

    let terminal = driving.await.expect("the drive task");
    assert_eq!(
      terminal,
      OtaPhaseSnapshot::Failed {
        reason: "daemon rejected OtaBegin: slot busy".into()
      }
    );
    assert!(
      rig
        .device
        .no_fragment(begin.transfer.id, Duration::from_millis(200))
        .await,
      "a rejected update streams nothing"
    );
  }

  fn installed_info(version: &str) -> WebappInfo {
    WebappInfo {
      id: Uuid::now_v7(),
      name: "harness".into(),
      source: WebappSource::Installed,
      role: WebappRole::Standard,
      version: version.to_owned(),
      description: None,
      icon_hash: None,
      settings_hash: None,
      overlay_hash: None,
      config: Vec::new(),
      permissions: Vec::new(),
      renders_voice_display: false,
      art: None,
      provenance: None,
    }
  }

  #[tokio::test]
  async fn an_install_carries_its_provenance_and_ends_on_what_the_device_reports() {
    let mut rig = rig().await;
    let bundle = rig.spool.write("app.zip", &pattern(24 * 1024));
    let info = installed_info("1.2.3");

    let driving = {
      let service = rig.service.clone();
      let expected = info.clone();
      tokio::spawn(async move {
        let result = service
          .install_webapp(
            DEVICE,
            Arc::new(FileSource::open(bundle)),
            Some("https://catalog.test/app.zip"),
          )
          .await;
        assert_eq!(result, WebappInstallResult::Installed(Box::new(expected)));
      })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    assert_eq!(begin.kind, OtaKind::InstalledWebapp);
    assert_eq!(
      begin.provenance.as_deref(),
      Some("https://catalog.test/app.zip"),
      "an install records where the bundle came from"
    );
    rig.device.ack_begin(request_id, 0);
    let mut sent = 0u32;
    while sent < begin.transfer.total_size {
      let fragment = rig.device.next_fragment(begin.transfer.id).await;
      sent = fragment.offset + fragment.bytes.len() as u32;
      rig.device.ack(begin.transfer.id, sent);
    }

    rig.device.progress(OtaPhase::Writing, 0);
    rig.device.webapp_installed(info);
    driving.await.expect("the install task");
  }

  #[tokio::test]
  async fn a_streamed_bundle_the_device_rejects_is_not_an_install() {
    let mut rig = rig().await;
    let bundle = rig.spool.write("app.zip", &pattern(16 * 1024));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .install_webapp(DEVICE, Arc::new(FileSource::open(bundle)), None)
          .await
      })
    };

    stream_through(&mut rig.device).await;
    rig.device.ota_error(OtaErrorCode::WriteFailed, "bad manifest");

    assert_eq!(
      driving.await.expect("the install task"),
      WebappInstallResult::Failed {
        reason: "[WriteFailed] bad manifest".into()
      },
      "a bundle that reached the device but would not install is a failure"
    );
  }

  #[tokio::test]
  async fn a_writing_tick_at_full_is_not_an_install() {
    let mut rig = rig().await;
    let bundle = rig.spool.write("app.zip", &pattern(16 * 1024));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .install_webapp(DEVICE, Arc::new(FileSource::open(bundle)), None)
          .await
      })
    };

    stream_through(&mut rig.device).await;
    rig.device.progress(OtaPhase::Writing, 100);
    assert!(!driving.is_finished(), "an install waits for the device's verdict");

    rig.device.webapp_installed(installed_info("0.1.0"));
    assert!(matches!(
      driving.await.expect("the install task"),
      WebappInstallResult::Installed(_)
    ));
  }

  #[tokio::test]
  async fn an_install_on_a_device_with_an_update_running_is_refused() {
    let mut rig = rig().await;
    let bundle = rig.spool.write("app.zip", &pattern(16 * 1024));
    let artifact = rig.spool.write("daemon", &pattern(8 * 1024));

    let pushing = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .install_webapp(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };
    rig.device.await_ota_begin().await;

    assert_eq!(
      rig
        .service
        .install_webapp(DEVICE, Arc::new(FileSource::open(bundle)), None)
        .await,
      WebappInstallResult::Failed {
        reason: IN_FLIGHT_REASON.into()
      }
    );
    pushing.abort();
  }

  #[tokio::test]
  async fn an_artifact_that_is_not_spooled_fails_before_the_wire() {
    let mut rig = rig().await;
    let missing = rig.spool.path().join("never-written");

    let terminal = rig
      .service
      .push_daemon(DEVICE, Arc::new(FileSource::open(missing)), None)
      .await;

    assert_eq!(
      terminal,
      OtaPhaseSnapshot::Failed {
        reason: "could not size artifact".into()
      }
    );
    assert!(rig.device.no_ota_begin(Duration::from_millis(200)).await);
  }

  #[tokio::test]
  async fn a_drive_with_no_adopted_link_fails_rather_than_waiting_for_one() {
    let spool = Spool::new();
    let data = Spool::new();
    let service = launch(spool.path(), data.path(), FakeFetch::new());
    let artifact = spool.write("daemon", &pattern(4 * 1024));

    let terminal = tokio::time::timeout(
      Duration::from_secs(3),
      service.push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None),
    )
    .await
    .expect("an unlinked device fails fast");

    assert!(
      matches!(terminal, OtaPhaseSnapshot::Failed { .. }),
      "got {terminal:?}, an update to a device that is not here cannot succeed"
    );
  }

  #[tokio::test]
  async fn a_resume_offset_from_the_daemon_is_honored_by_the_drive() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("daemon", &pattern(160 * 1024));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    let resume = 64 * 1024u32;
    rig.device.ack_begin(request_id, resume);

    let first = rig.device.next_fragment(begin.transfer.id).await;
    assert_eq!(first.offset, resume, "the drive resumes where the daemon left off");

    let mut sent = first.offset + first.bytes.len() as u32;
    rig.device.ack(begin.transfer.id, sent);
    while sent < begin.transfer.total_size {
      let fragment = rig.device.next_fragment(begin.transfer.id).await;
      sent = fragment.offset + fragment.bytes.len() as u32;
      rig.device.ack(begin.transfer.id, sent);
    }

    rig.device.progress(OtaPhase::Writing, 100);
    rig.device.await_activate().await;
    rig.device.progress(OtaPhase::Reboot, 100);
    assert_eq!(driving.await.expect("the drive task"), OtaPhaseSnapshot::Completed);
  }

  #[tokio::test]
  async fn an_image_drive_completes_on_reboot_without_an_activate() {
    let mut rig = rig().await;
    let swu = rig.spool.write("update.swu", &pattern(32 * 1024));
    let zck = rig.spool.asset("system.zck", &pattern(1_024));
    let boot = rig.spool.asset("boot.zck", &pattern(2_048));

    let driving = {
      let service = rig.service.clone();
      let zcks = BTreeMap::from([(SYSTEM_ZCK_ASSET.to_string(), zck), (BOOT_ZCK_ASSET.to_string(), boot)]);
      tokio::spawn(async move {
        service
          .push_update(DEVICE, Arc::new(FileSource::open(swu)), zcks, Some("https://ota.test"))
          .await
      })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    assert_eq!(begin.kind, OtaKind::Image);
    assert_eq!(begin.update_url_base.as_deref(), Some("https://ota.test"));
    rig.device.ack_begin(request_id, 0);
    let mut sent = 0u32;
    while sent < begin.transfer.total_size {
      let fragment = rig.device.next_fragment(begin.transfer.id).await;
      sent = fragment.offset + fragment.bytes.len() as u32;
      rig.device.ack(begin.transfer.id, sent);
    }

    rig.device.progress(OtaPhase::Writing, 100);
    assert!(
      rig.device.no_activate(Duration::from_millis(300)).await,
      "an image is applied by the daemon, not committed by the companion"
    );
    rig.device.progress(OtaPhase::Reboot, 100);

    assert_eq!(driving.await.expect("the drive task"), OtaPhaseSnapshot::Completed);
  }

  #[tokio::test]
  async fn an_image_push_arms_the_range_server_with_its_zcks() {
    let mut rig = rig().await;
    let swu = rig.spool.write("update.swu", &pattern(32 * 1024));
    let zck = rig.spool.asset("system.zck", &[0xAA; 256]);
    let boot = rig.spool.asset("boot.zck", &[0xBB; 256]);

    let service = rig.service.clone();
    let zcks = BTreeMap::from([(SYSTEM_ZCK_ASSET.to_string(), zck), (BOOT_ZCK_ASSET.to_string(), boot)]);
    tokio::spawn(async move {
      service
        .push_update(DEVICE, Arc::new(FileSource::open(swu)), zcks, None)
        .await
    });
    rig.device.await_ota_begin().await;

    let id = rig
      .device
      .ask_range(SYSTEM_ZCK_ASSET, vec![RangeSpec { start: 0, length: 16 }]);
    let reply = rig
      .device
      .await_range_reply(id)
      .await
      .expect("the zcks handed to the push are servable");

    assert_eq!(reply.total_size, 256);
  }

  #[tokio::test(start_paused = true)]
  async fn a_daemon_that_goes_silent_mid_apply_fails_the_run_instead_of_hanging() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("daemon", &pattern(4_096));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, begin.transfer.total_size);

    let terminal = tokio::time::timeout(Duration::from_secs(600), driving)
      .await
      .expect("a silent daemon must not park the caller forever")
      .expect("the drive task");

    assert_eq!(
      terminal,
      OtaPhaseSnapshot::Failed {
        reason: stalled_reason()
      }
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_daemon_that_goes_silent_after_activate_fails_the_commit() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("daemon", &pattern(4_096));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, begin.transfer.total_size);
    rig.device.progress(OtaPhase::Writing, 100);
    rig.device.await_activate().await;

    let terminal = tokio::time::timeout(Duration::from_secs(600), driving)
      .await
      .expect("a silent commit must not park the caller forever")
      .expect("the drive task");

    assert_eq!(
      terminal,
      OtaPhaseSnapshot::Failed {
        reason: stalled_reason()
      }
    );
  }

  #[tokio::test(start_paused = true)]
  async fn progress_keeps_the_watchdog_quiet() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("daemon", &pattern(4_096));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, begin.transfer.total_size);

    for _ in 0..6 {
      rig.device.progress(OtaPhase::Writing, 10);
      tokio::time::sleep(Duration::from_millis(50_000)).await;
    }
    rig.device.progress(OtaPhase::Writing, 100);
    rig.device.await_activate().await;
    rig.device.progress(OtaPhase::Reboot, 100);

    let terminal = tokio::time::timeout(Duration::from_secs(600), driving)
      .await
      .expect("a reporting daemon runs to completion")
      .expect("the drive task");

    assert_eq!(terminal, OtaPhaseSnapshot::Completed);
  }

  #[tokio::test]
  async fn a_replayed_failure_does_not_kill_the_run_that_is_re_driving_it() {
    let mut rig = rig().await;
    let body = pattern(40 * 1024);
    let artifact = rig.spool.write("daemon", &body);

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    let update_id = stream_through(&mut rig.device).await;
    rig
      .device
      .ota_error_replayed(OtaErrorCode::Internal, "the attempt before this one died", &update_id);

    rig.device.progress(OtaPhase::Writing, 100);
    rig.device.await_activate().await;
    rig.device.progress(OtaPhase::Reboot, 100);

    let terminal = driving.await.expect("the drive task");
    assert_eq!(
      terminal,
      OtaPhaseSnapshot::Completed,
      "a resume re-drives the same update_id, so the replay of its predecessor's failure must not land on it"
    );
  }

  #[tokio::test]
  async fn a_live_failure_still_ends_the_run() {
    let mut rig = rig().await;
    let body = pattern(40 * 1024);
    let artifact = rig.spool.write("daemon", &body);

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    stream_through(&mut rig.device).await;
    rig.device.ota_error(OtaErrorCode::WriteFailed, "emmc said no");

    let terminal = driving.await.expect("the drive task");
    assert!(
      matches!(terminal, OtaPhaseSnapshot::Failed { .. }),
      "the replay guard must not have been widened into swallowing real failures, got {terminal:?}"
    );
  }

  fn publish_daemon_release(fetch: &FakeFetch, body: &[u8]) {
    let mut fixture = ManifestFixture::new(CHANNEL, TO_RELEASE);
    fixture.daemon = Some(digest_of(body));
    fetch.serve_text(&format!("{ROOT}/manifest.json"), fixture.json());
    fetch.serve_artifact(&format!("{ROOT}/daemon/{CHANNEL}/0.9.1/bridgething"), body.to_vec());
  }

  fn drive_apply(service: &Arc<OtaService>) {
    let service = service.clone();
    tokio::spawn(async move { service.apply_version(DEVICE, CHANNEL, TO_RELEASE, ROOT).await });
  }

  async fn run_of(service: &Arc<OtaService>) -> crate::ota::run_store::OtaRun {
    service
      .retained_runs()
      .await
      .into_iter()
      .find(|run| run.device_id == DEVICE)
      .expect("a run for the device")
  }

  #[tokio::test(start_paused = true)]
  async fn a_link_that_drops_mid_transfer_keeps_what_it_takes_to_re_drive() {
    let mut rig = rig().await;
    let body = pattern(64 * 1024);
    publish_daemon_release(&rig.fetch, &body);
    rig.service.device_meta(DEVICE, meta(FROM_VERSION, "1.0.0", CHANNEL));

    drive_apply(&rig.service);
    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, 0);
    rig.device.next_fragment(begin.transfer.id).await;

    rig.service.release(DEVICE).await;
    drop(rig.device);

    let run = run_of(&rig.service).await;
    assert!(run.resumable, "a flap mid-transfer is not a real failure");
    assert_eq!(run.channel.as_deref(), Some(CHANNEL));
    assert_eq!(run.root_url.as_deref(), Some(ROOT));
    assert_eq!(run.release_version.as_deref(), Some(TO_RELEASE));
  }

  #[tokio::test(start_paused = true)]
  async fn a_reconnect_re_drives_the_interrupted_run_with_the_same_parameters() {
    let mut rig = rig().await;
    let body = pattern(64 * 1024);
    publish_daemon_release(&rig.fetch, &body);
    rig.service.device_meta(DEVICE, meta(FROM_VERSION, "1.0.0", CHANNEL));

    drive_apply(&rig.service);
    let (request_id, first) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, 0);
    rig.device.next_fragment(first.transfer.id).await;
    rig.service.release(DEVICE).await;
    drop(rig.device);

    let (gateway, mut device) = linked_gateway();
    rig.service.adopt(DEVICE, gateway.clone()).await;
    route_into(&gateway, &rig.service, DEVICE);
    rig.service.device_meta(DEVICE, meta(FROM_VERSION, "1.0.0", CHANNEL));

    let (request_id, second) = device.await_ota_begin_within(Duration::from_secs(60)).await;

    assert_eq!(
      second.update_id, first.update_id,
      "a resume re-drives the same artifact so the device's partial is worth keeping"
    );

    device.ack_begin(request_id, second.transfer.total_size);
    device.progress(OtaPhase::Writing, 100);
    device.await_activate().await;
    device.progress(OtaPhase::Reboot, 100);

    let settling = async {
      loop {
        let run = run_of(&rig.service).await;
        if run.outcome.is_some() {
          return run;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
      }
    };
    let resumed = tokio::time::timeout(Duration::from_secs(120), settling)
      .await
      .expect("the resumed run reaches a result");

    assert!(!resumed.resumable, "a run that got picked back up is no longer waiting");
  }

  #[tokio::test(start_paused = true)]
  async fn a_flap_while_the_device_is_writing_is_still_resumable() {
    let mut rig = rig().await;
    let body = pattern(64 * 1024);
    publish_daemon_release(&rig.fetch, &body);
    rig.service.device_meta(DEVICE, meta(FROM_VERSION, "1.0.0", CHANNEL));

    drive_apply(&rig.service);
    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, begin.transfer.total_size);
    rig.device.progress(OtaPhase::Writing, 50);

    let writing = tokio::time::timeout(Duration::from_secs(30), async {
      loop {
        if run_of(&rig.service).await.phase == crate::ota::run_store::OtaRunPhase::Writing {
          return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
      }
    })
    .await;
    assert!(writing.is_ok(), "the device reported it was writing");

    rig.service.release(DEVICE).await;
    drop(rig.device);

    let run = run_of(&rig.service).await;
    assert!(
      run.resumable,
      "a flap mid-write is interrupted, not failed; only reboot and confirming are the run asking the device to go"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_run_the_device_finished_is_left_alone_by_a_later_flap() {
    let mut rig = rig().await;
    let body = pattern(64 * 1024);
    publish_daemon_release(&rig.fetch, &body);
    rig.service.device_meta(DEVICE, meta(FROM_VERSION, "1.0.0", CHANNEL));

    drive_apply(&rig.service);
    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, begin.transfer.total_size);
    rig.device.progress(OtaPhase::Writing, 100);
    rig.device.await_activate().await;
    rig.device.progress(OtaPhase::Reboot, 100);

    tokio::time::sleep(Duration::from_secs(1)).await;
    rig.service.release(DEVICE).await;

    let run = run_of(&rig.service).await;
    assert!(
      !run.resumable,
      "the reboot is the run asking the device to go away, not the link dropping under it"
    );
  }

  fn snapshot_runs(from: &std::path::Path, to: &std::path::Path) {
    let dest = to.join(RUNS_FILE);
    std::fs::create_dir_all(dest.parent().expect("the runs file is under a directory"))
      .expect("the scratch directory is writable");
    std::fs::copy(from.join(RUNS_FILE), dest).expect("an open run was persisted before the process died");
  }

  #[tokio::test(start_paused = true)]
  async fn an_update_the_app_died_under_re_drives_on_the_next_launch() {
    let cache = Spool::new();
    let killed = Spool::new();
    let relaunched = Spool::new();
    let fetch = FakeFetch::new();
    let body = pattern(64 * 1024);
    publish_daemon_release(&fetch, &body);

    let first = {
      let service = launch(cache.path(), killed.path(), fetch.clone());
      let (gateway, mut device) = linked_gateway();
      service.adopt(DEVICE, gateway.clone()).await;
      route_into(&gateway, &service, DEVICE);
      service.device_meta(DEVICE, meta(FROM_VERSION, "1.0.0", CHANNEL));

      drive_apply(&service);
      let (request_id, begin) = device.await_ota_begin().await;
      device.ack_begin(request_id, 0);
      device.next_fragment(begin.transfer.id).await;
      snapshot_runs(killed.path(), relaunched.path());
      begin.update_id
    };

    let service = launch(cache.path(), relaunched.path(), fetch.clone());
    let (gateway, mut device) = linked_gateway();
    service.adopt(DEVICE, gateway.clone()).await;
    route_into(&gateway, &service, DEVICE);
    service.device_meta(DEVICE, meta(FROM_VERSION, "1.0.0", CHANNEL));

    let (_, second) = device.await_ota_begin_within(Duration::from_secs(60)).await;

    assert_eq!(
      second.update_id, first,
      "a relaunch re-drives the same artifact, so the daemon's partial is still worth keeping"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_launch_with_nothing_interrupted_drives_nothing_on_its_own() {
    let mut rig = rig().await;
    publish_daemon_release(&rig.fetch, &pattern(64 * 1024));
    rig.service.device_meta(DEVICE, meta(FROM_VERSION, "1.0.0", CHANNEL));

    assert!(
      rig.device.no_ota_begin(Duration::from_secs(30)).await,
      "an empty store must not manufacture a run out of a link arriving"
    );
  }

  #[tokio::test]
  async fn a_full_offset_ack_streams_nothing_and_still_completes_on_the_device() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("daemon", &pattern(64 * 1024));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };

    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, begin.transfer.total_size);
    assert!(
      rig
        .device
        .no_fragment(begin.transfer.id, Duration::from_millis(300))
        .await,
      "an artifact the daemon already holds costs zero bytes on the link"
    );

    rig.device.progress(OtaPhase::Writing, 100);
    rig.device.await_activate().await;
    rig.device.progress(OtaPhase::Reboot, 100);

    assert_eq!(
      driving.await.expect("the drive task"),
      OtaPhaseSnapshot::Completed,
      "the drive still rides the device's own signals to a terminal"
    );
  }

  #[tokio::test]
  async fn a_completed_drive_reports_a_terminal_the_reducer_can_close_a_run_with() {
    let mut rig = rig().await;
    let artifact = rig.spool.write("daemon", &pattern(8 * 1024));

    let driving = {
      let service = rig.service.clone();
      tokio::spawn(async move {
        service
          .push_daemon(DEVICE, Arc::new(FileSource::open(artifact)), None)
          .await
      })
    };
    stream_through(&mut rig.device).await;
    rig.device.progress(OtaPhase::Writing, 100);
    rig.device.await_activate().await;
    rig.device.progress(OtaPhase::Reboot, 100);
    driving.await.expect("the drive task");

    let seen = drain(&mut rig.events);
    assert!(
      !seen.iter().any(|event| matches!(event, OtaPollEvent::Failed { .. })),
      "a clean run reports no failure, got {seen:?}"
    );
  }
}
