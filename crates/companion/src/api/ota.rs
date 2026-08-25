use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
};

use bridgething_delivery::{ota, seam::Clock};
use bridgething_sdk_runtime::rt::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum OtaKind {
  Image,
  Daemon,
  BuiltinWebapp,
  InstalledWebapp,
  WakewordModel,
}

impl From<OtaKind> for libbridgething::OtaKind {
  fn from(kind: OtaKind) -> Self {
    match kind {
      OtaKind::Image => Self::Image,
      OtaKind::Daemon => Self::Daemon,
      OtaKind::BuiltinWebapp => Self::BuiltinWebapp,
      OtaKind::InstalledWebapp => Self::InstalledWebapp,
      OtaKind::WakewordModel => Self::WakewordModel,
    }
  }
}

impl From<libbridgething::OtaKind> for OtaKind {
  fn from(kind: libbridgething::OtaKind) -> Self {
    match kind {
      libbridgething::OtaKind::Image => Self::Image,
      libbridgething::OtaKind::Daemon => Self::Daemon,
      libbridgething::OtaKind::BuiltinWebapp => Self::BuiltinWebapp,
      libbridgething::OtaKind::InstalledWebapp => Self::InstalledWebapp,
      libbridgething::OtaKind::WakewordModel => Self::WakewordModel,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum OtaApplyPhase {
  Streaming,
  Verifying,
  Writing,
  Confirming,
  Reboot,
}

impl From<OtaApplyPhase> for libbridgething::OtaPhase {
  fn from(phase: OtaApplyPhase) -> Self {
    match phase {
      OtaApplyPhase::Streaming => Self::Streaming,
      OtaApplyPhase::Verifying => Self::Verifying,
      OtaApplyPhase::Writing => Self::Writing,
      OtaApplyPhase::Confirming => Self::Confirming,
      OtaApplyPhase::Reboot => Self::Reboot,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum OtaStepKind {
  Download,
  Stream,
  Apply,
  Reboot,
}

impl From<OtaStepKind> for ota::event::OtaStepKind {
  fn from(kind: OtaStepKind) -> Self {
    match kind {
      OtaStepKind::Download => Self::Download,
      OtaStepKind::Stream => Self::Stream,
      OtaStepKind::Apply => Self::Apply,
      OtaStepKind::Reboot => Self::Reboot,
    }
  }
}

impl From<ota::event::OtaStepKind> for OtaStepKind {
  fn from(kind: ota::event::OtaStepKind) -> Self {
    match kind {
      ota::event::OtaStepKind::Download => Self::Download,
      ota::event::OtaStepKind::Stream => Self::Stream,
      ota::event::OtaStepKind::Apply => Self::Apply,
      ota::event::OtaStepKind::Reboot => Self::Reboot,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaPlanStep {
  pub id: u32,
  pub kind: OtaStepKind,
  pub label: String,
  pub bytes: u64,
}

impl From<OtaPlanStep> for ota::event::OtaPlanStep {
  fn from(step: OtaPlanStep) -> Self {
    Self {
      id: step.id,
      kind: step.kind.into(),
      label: step.label,
      bytes: step.bytes,
    }
  }
}

impl From<ota::event::OtaPlanStep> for OtaPlanStep {
  fn from(step: ota::event::OtaPlanStep) -> Self {
    Self {
      id: step.id,
      kind: step.kind.into(),
      label: step.label,
      bytes: step.bytes,
    }
  }
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum OtaPhaseSnapshot {
  Idle,
  Downloading {
    asset: String,
    received: u64,
    total: u64,
    rate_per_sec: Option<f64>,
  },
  Streaming {
    asset: String,
    sent: u64,
    total: u64,
    rate_per_sec: Option<f64>,
    eta_seconds: Option<f64>,
  },
  Applying {
    phase: OtaApplyPhase,
    write_percent: u32,
    dwl_percent: u32,
    dwl_bytes: u64,
  },
  Staged,
  Completed,
  Failed {
    reason: String,
  },
}

impl From<OtaPhaseSnapshot> for ota::event::OtaPhaseSnapshot {
  fn from(snapshot: OtaPhaseSnapshot) -> Self {
    match snapshot {
      OtaPhaseSnapshot::Idle => Self::Idle,
      OtaPhaseSnapshot::Downloading {
        asset,
        received,
        total,
        rate_per_sec,
      } => Self::Downloading {
        asset,
        received,
        total,
        rate_per_sec,
      },
      OtaPhaseSnapshot::Streaming {
        asset,
        sent,
        total,
        rate_per_sec,
        eta_seconds,
      } => Self::Streaming {
        asset,
        sent,
        total,
        rate_per_sec,
        eta_seconds,
      },
      OtaPhaseSnapshot::Applying {
        phase,
        write_percent,
        dwl_percent,
        dwl_bytes,
      } => Self::Applying {
        phase: phase.into(),
        write_percent,
        dwl_percent,
        dwl_bytes,
      },
      OtaPhaseSnapshot::Staged => Self::Staged,
      OtaPhaseSnapshot::Completed => Self::Completed,
      OtaPhaseSnapshot::Failed { reason } => Self::Failed { reason },
    }
  }
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum OtaPollEvent {
  ManifestPolled {
    updated_at: String,
  },
  ManifestPollFailed {
    reason: String,
  },
  UpdateAvailable {
    device_id: String,
    release: String,
    daemon_version: String,
    image_version: String,
  },
  Planned {
    device_id: String,
    kind: OtaKind,
    release: String,
    daemon_version: String,
    image_version: String,
    channel: String,
    root_url: String,
    steps: Vec<OtaPlanStep>,
  },
  Progress {
    device_id: String,
    kind: OtaKind,
    step_id: u32,
    snapshot: OtaPhaseSnapshot,
  },
  Updated {
    device_id: String,
    kind: OtaKind,
    version: String,
  },
  Failed {
    device_id: String,
    kind: OtaKind,
    reason: String,
  },
}

impl From<OtaPollEvent> for ota::event::OtaPollEvent {
  fn from(event: OtaPollEvent) -> Self {
    match event {
      OtaPollEvent::ManifestPolled { updated_at } => Self::ManifestPolled { updated_at },
      OtaPollEvent::ManifestPollFailed { reason } => Self::ManifestPollFailed { reason },
      OtaPollEvent::UpdateAvailable {
        device_id,
        release,
        daemon_version,
        image_version,
      } => Self::UpdateAvailable {
        device_id,
        release,
        daemon_version,
        image_version,
      },
      OtaPollEvent::Planned {
        device_id,
        kind,
        release,
        daemon_version,
        image_version,
        channel,
        root_url,
        steps,
      } => Self::Planned {
        device_id,
        kind: kind.into(),
        release,
        daemon_version,
        image_version,
        channel,
        root_url,
        steps: steps.into_iter().map(Into::into).collect(),
      },
      OtaPollEvent::Progress {
        device_id,
        kind,
        step_id,
        snapshot,
      } => Self::Progress {
        device_id,
        kind: kind.into(),
        step_id,
        snapshot: snapshot.into(),
      },
      OtaPollEvent::Updated {
        device_id,
        kind,
        version,
      } => Self::Updated {
        device_id,
        kind: kind.into(),
        version,
      },
      OtaPollEvent::Failed {
        device_id,
        kind,
        reason,
      } => Self::Failed {
        device_id,
        kind: kind.into(),
        reason,
      },
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum OtaRunOutcome {
  Succeeded,
  Failed,
  Cancelled,
}

impl From<ota::run_store::OtaRunOutcome> for OtaRunOutcome {
  fn from(outcome: ota::run_store::OtaRunOutcome) -> Self {
    match outcome {
      ota::run_store::OtaRunOutcome::Succeeded => Self::Succeeded,
      ota::run_store::OtaRunOutcome::Failed => Self::Failed,
      ota::run_store::OtaRunOutcome::Cancelled => Self::Cancelled,
    }
  }
}

impl From<OtaRunOutcome> for ota::run_store::OtaRunOutcome {
  fn from(outcome: OtaRunOutcome) -> Self {
    match outcome {
      OtaRunOutcome::Succeeded => Self::Succeeded,
      OtaRunOutcome::Failed => Self::Failed,
      OtaRunOutcome::Cancelled => Self::Cancelled,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum OtaRunPhase {
  Idle,
  Downloading,
  Streaming,
  Verifying,
  Writing,
  Confirming,
  Reboot,
  Completed,
  Failed,
}

impl From<ota::run_store::OtaRunPhase> for OtaRunPhase {
  fn from(phase: ota::run_store::OtaRunPhase) -> Self {
    match phase {
      ota::run_store::OtaRunPhase::Idle => Self::Idle,
      ota::run_store::OtaRunPhase::Downloading => Self::Downloading,
      ota::run_store::OtaRunPhase::Streaming => Self::Streaming,
      ota::run_store::OtaRunPhase::Verifying => Self::Verifying,
      ota::run_store::OtaRunPhase::Writing => Self::Writing,
      ota::run_store::OtaRunPhase::Confirming => Self::Confirming,
      ota::run_store::OtaRunPhase::Reboot => Self::Reboot,
      ota::run_store::OtaRunPhase::Completed => Self::Completed,
      ota::run_store::OtaRunPhase::Failed => Self::Failed,
    }
  }
}

impl From<OtaRunPhase> for ota::run_store::OtaRunPhase {
  fn from(phase: OtaRunPhase) -> Self {
    match phase {
      OtaRunPhase::Idle => Self::Idle,
      OtaRunPhase::Downloading => Self::Downloading,
      OtaRunPhase::Streaming => Self::Streaming,
      OtaRunPhase::Verifying => Self::Verifying,
      OtaRunPhase::Writing => Self::Writing,
      OtaRunPhase::Confirming => Self::Confirming,
      OtaRunPhase::Reboot => Self::Reboot,
      OtaRunPhase::Completed => Self::Completed,
      OtaRunPhase::Failed => Self::Failed,
    }
  }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaRun {
  pub run_id: String,
  pub device_id: String,
  pub kind: OtaKind,
  pub phase: OtaRunPhase,
  pub steps: Vec<OtaPlanStep>,
  pub step_id: u32,
  pub started_at_ms: u64,
  pub phase_started_at_ms: u64,
  pub stage_received: Option<u64>,
  pub stage_total: Option<u64>,
  pub rate_per_sec: Option<f64>,
  pub dwl_percent: Option<u32>,
  pub outcome: Option<OtaRunOutcome>,
  pub error: Option<String>,
  pub release_version: Option<String>,
  pub daemon_version: Option<String>,
  pub image_version: Option<String>,
  pub channel: Option<String>,
  pub root_url: Option<String>,
  pub resumable: bool,
  pub webapp_id: Option<String>,
  pub webapp_name: Option<String>,
}

impl From<ota::run_store::OtaRun> for OtaRun {
  fn from(run: ota::run_store::OtaRun) -> Self {
    Self {
      run_id: run.run_id,
      device_id: run.device_id,
      kind: run.kind.into(),
      phase: run.phase.into(),
      steps: run.steps.into_iter().map(Into::into).collect(),
      step_id: run.step_id,
      started_at_ms: run.started_at_ms,
      phase_started_at_ms: run.phase_started_at_ms,
      stage_received: run.stage_received,
      stage_total: run.stage_total,
      rate_per_sec: run.rate_per_sec,
      dwl_percent: run.dwl_percent,
      outcome: run.outcome.map(Into::into),
      error: run.error,
      release_version: run.release_version,
      daemon_version: run.daemon_version,
      image_version: run.image_version,
      channel: run.channel,
      root_url: run.root_url,
      resumable: run.resumable,
      webapp_id: run.webapp_id,
      webapp_name: run.webapp_name,
    }
  }
}

impl From<OtaRun> for ota::run_store::OtaRun {
  fn from(run: OtaRun) -> Self {
    Self {
      run_id: run.run_id,
      device_id: run.device_id,
      identity: None,
      kind: run.kind.into(),
      phase: run.phase.into(),
      steps: run.steps.into_iter().map(Into::into).collect(),
      step_id: run.step_id,
      started_at_ms: run.started_at_ms,
      phase_started_at_ms: run.phase_started_at_ms,
      stage_received: run.stage_received,
      stage_total: run.stage_total,
      rate_per_sec: run.rate_per_sec,
      dwl_percent: run.dwl_percent,
      outcome: run.outcome.map(Into::into),
      error: run.error,
      release_version: run.release_version,
      daemon_version: run.daemon_version,
      image_version: run.image_version,
      channel: run.channel,
      root_url: run.root_url,
      resumable: run.resumable,
      webapp_id: run.webapp_id,
      webapp_name: run.webapp_name,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaAvailable {
  pub device_id: String,
  pub release_version: Option<String>,
  pub daemon_version: Option<String>,
  pub image_version: Option<String>,
}

impl From<ota::run_store::OtaAvailable> for OtaAvailable {
  fn from(available: ota::run_store::OtaAvailable) -> Self {
    Self {
      device_id: available.device_id,
      release_version: available.release_version,
      daemon_version: available.daemon_version,
      image_version: available.image_version,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaPollStatus {
  pub last_polled_at: Option<String>,
  pub error: Option<String>,
}

impl From<ota::run_store::OtaPollStatus> for OtaPollStatus {
  fn from(status: ota::run_store::OtaPollStatus) -> Self {
    Self {
      last_polled_at: status.last_polled_at,
      error: status.error,
    }
  }
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum OtaStoreChange {
  Run { run: Box<OtaRun> },
  Available { available: OtaAvailable },
  Poll { status: OtaPollStatus },
}

impl From<ota::run_store::OtaStoreChange> for OtaStoreChange {
  fn from(change: ota::run_store::OtaStoreChange) -> Self {
    match change {
      ota::run_store::OtaStoreChange::Run(run) => Self::Run {
        run: Box::new((*run).into()),
      },
      ota::run_store::OtaStoreChange::Available(available) => Self::Available {
        available: available.into(),
      },
      ota::run_store::OtaStoreChange::Poll(status) => Self::Poll { status: status.into() },
    }
  }
}

#[uniffi::export]
pub fn ota_cancelled_reason() -> String {
  ota::event::CANCELLED_REASON.to_owned()
}

struct SystemClock;

impl Clock for SystemClock {
  fn now(&self) -> Instant {
    Instant::now()
  }

  fn unix_millis(&self) -> u64 {
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|elapsed| elapsed.as_millis() as u64)
      .unwrap_or(0)
  }
}

#[derive(uniffi::Object)]
pub struct OtaRunStore {
  inner: Mutex<ota::run_store::OtaRunStore>,
}

#[uniffi::export]
impl OtaRunStore {
  #[uniffi::constructor]
  pub fn new() -> Arc<Self> {
    Arc::new(Self {
      inner: Mutex::new(ota::run_store::OtaRunStore::new(Arc::new(SystemClock), None)),
    })
  }

  pub fn runs(&self) -> Vec<OtaRun> {
    self
      .inner
      .lock()
      .unwrap()
      .runs()
      .into_iter()
      .cloned()
      .map(Into::into)
      .collect()
  }

  pub fn available(&self) -> Vec<OtaAvailable> {
    self
      .inner
      .lock()
      .unwrap()
      .available()
      .into_iter()
      .cloned()
      .map(Into::into)
      .collect()
  }

  pub fn poll_status(&self) -> OtaPollStatus {
    self.inner.lock().unwrap().poll_status().clone().into()
  }

  pub fn open_run_kind(&self, device_id: String) -> Option<OtaKind> {
    self.inner.lock().unwrap().open_run_kind(&device_id).map(Into::into)
  }

  pub fn dismiss(&self, device_id: String) -> Option<OtaRun> {
    self.inner.lock().unwrap().dismiss(&device_id).map(Into::into)
  }

  pub fn interrupt(&self, device_id: String) -> Option<OtaRun> {
    self.inner.lock().unwrap().interrupt(&device_id).map(Into::into)
  }

  pub fn note_meta(&self, device_id: String, daemon_version: String, image_version: String) -> Option<OtaRun> {
    self
      .inner
      .lock()
      .unwrap()
      .note_meta(&device_id, &daemon_version, &image_version)
      .map(Into::into)
  }

  pub fn annotate_webapp(
    &self,
    device_id: String,
    webapp_id: Option<String>,
    webapp_name: Option<String>,
  ) -> Option<OtaRun> {
    self
      .inner
      .lock()
      .unwrap()
      .annotate_webapp(&device_id, webapp_id.as_deref(), webapp_name.as_deref())
      .map(Into::into)
  }

  pub fn ingest(&self, event: OtaPollEvent) -> Vec<OtaStoreChange> {
    self
      .inner
      .lock()
      .unwrap()
      .ingest(event.into(), None)
      .into_iter()
      .map(Into::into)
      .collect()
  }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaRunProgress {
  pub percent: u32,
  pub step_index: u32,
  pub step_count: u32,
  pub step_label: Option<String>,
  pub eta_seconds: Option<u64>,
}

#[uniffi::export]
pub fn ota_run_progress(run: OtaRun, now_ms: u64) -> OtaRunProgress {
  let progress = ota::progress::ota_progress(&run.into(), now_ms);
  OtaRunProgress {
    percent: progress.percent,
    step_index: progress.step_index as u32,
    step_count: progress.step_count as u32,
    step_label: progress.step_label,
    eta_seconds: progress.eta_seconds,
  }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct ArtifactDigest {
  pub size: u64,
  pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaPatchDigest {
  pub size: u64,
  pub sha256: String,
  pub source_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaReleaseArtifacts {
  pub daemon: Option<ArtifactDigest>,
  pub daemon_zst: Option<ArtifactDigest>,
  pub image_swu: Option<ArtifactDigest>,
  pub image_zck: Option<ArtifactDigest>,
  pub image_boot_zck: Option<ArtifactDigest>,
  pub webapps: HashMap<String, ArtifactDigest>,
  pub daemon_patches: HashMap<String, OtaPatchDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaManifestRelease {
  pub version: String,
  pub channel: String,
  pub yanked: Option<String>,
  pub deprecated: bool,
  pub builtin_webapps: HashMap<String, String>,
  pub artifacts: Option<OtaReleaseArtifacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaManifestChannel {
  pub name: String,
  pub stability: String,
  pub is_default: bool,
  pub latest: String,
  pub releases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaDiscoverManifest {
  pub manifest_version: u32,
  pub updated_at: String,
  pub channels: HashMap<String, OtaManifestChannel>,
  pub releases: HashMap<String, OtaManifestRelease>,
}

impl From<bridgething_delivery::bundle::ArtifactDigest> for ArtifactDigest {
  fn from(digest: bridgething_delivery::bundle::ArtifactDigest) -> Self {
    Self {
      size: digest.size,
      sha256: digest.sha256,
    }
  }
}

impl From<ota::manifest::OtaPatchDigest> for OtaPatchDigest {
  fn from(digest: ota::manifest::OtaPatchDigest) -> Self {
    Self {
      size: digest.size,
      sha256: digest.sha256,
      source_sha256: digest.source_sha256,
    }
  }
}

impl From<ota::manifest::OtaReleaseArtifacts> for OtaReleaseArtifacts {
  fn from(artifacts: ota::manifest::OtaReleaseArtifacts) -> Self {
    Self {
      daemon: artifacts.daemon.map(Into::into),
      daemon_zst: artifacts.daemon_zst.map(Into::into),
      image_swu: artifacts.image_swu.map(Into::into),
      image_zck: artifacts.image_zck.map(Into::into),
      image_boot_zck: artifacts.image_boot_zck.map(Into::into),
      webapps: artifacts
        .webapps
        .into_iter()
        .map(|(name, digest)| (name, digest.into()))
        .collect(),
      daemon_patches: artifacts
        .daemon_patches
        .into_iter()
        .map(|(from, digest)| (from, digest.into()))
        .collect(),
    }
  }
}

impl From<ota::manifest::OtaManifestRelease> for OtaManifestRelease {
  fn from(release: ota::manifest::OtaManifestRelease) -> Self {
    Self {
      version: release.version,
      channel: release.channel,
      yanked: release.yanked,
      deprecated: release.deprecated,
      builtin_webapps: release.builtin_webapps.into_iter().collect(),
      artifacts: release.artifacts.map(Into::into),
    }
  }
}

impl From<ota::manifest::OtaManifestChannel> for OtaManifestChannel {
  fn from(channel: ota::manifest::OtaManifestChannel) -> Self {
    Self {
      name: channel.name,
      stability: channel.stability,
      is_default: channel.is_default,
      latest: channel.latest,
      releases: channel.releases,
    }
  }
}

impl From<ota::manifest::OtaDiscoverManifest> for OtaDiscoverManifest {
  fn from(manifest: ota::manifest::OtaDiscoverManifest) -> Self {
    Self {
      manifest_version: manifest.manifest_version,
      updated_at: manifest.updated_at,
      channels: manifest
        .channels
        .into_iter()
        .map(|(slug, channel)| (slug, channel.into()))
        .collect(),
      releases: manifest
        .releases
        .into_iter()
        .map(|(version, release)| (version, release.into()))
        .collect(),
    }
  }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum OtaManifestError {
  #[error("manifest parse: {0}")]
  Parse(String),
}

#[uniffi::export]
pub fn parse_ota_discover_manifest(json: String) -> Result<OtaDiscoverManifest, OtaManifestError> {
  serde_json::from_str::<ota::manifest::OtaDiscoverManifest>(&json)
    .map(Into::into)
    .map_err(|error| OtaManifestError::Parse(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaCompositeVersion {
  pub daemon: String,
  pub image: String,
}

#[uniffi::export]
pub fn parse_ota_composite_version(raw: String) -> Option<OtaCompositeVersion> {
  ota::manifest::OtaCompositeVersion::parse(&raw).map(|version| OtaCompositeVersion {
    daemon: version.daemon,
    image: version.image,
  })
}

#[uniffi::export]
pub fn ota_composite_version_string(version: OtaCompositeVersion) -> String {
  ota::manifest::OtaCompositeVersion {
    daemon: version.daemon,
    image: version.image,
  }
  .composite()
}

#[uniffi::export]
pub fn ota_patch_source_matches(declared: Option<String>, running: Option<String>) -> bool {
  ota::manifest::patch_source_matches(declared.as_deref(), running.as_deref())
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaArtifactUrls {
  pub daemon_binary: String,
  pub daemon_binary_zst: String,
  pub image_swu: String,
  pub image_zck: String,
  pub image_boot_zck: String,
}

#[uniffi::export]
pub fn ota_artifact_urls(
  root_url: String,
  channel: String,
  daemon_version: String,
  image_version: String,
  image_variant: String,
) -> OtaArtifactUrls {
  let urls =
    ota::manifest::OtaArtifactUrls::build(&root_url, &channel, &daemon_version, &image_version, &image_variant);
  OtaArtifactUrls {
    daemon_binary: urls.daemon_binary,
    daemon_binary_zst: urls.daemon_binary_zst,
    image_swu: urls.image_swu,
    image_zck: urls.image_zck,
    image_boot_zck: urls.image_boot_zck,
  }
}

#[uniffi::export]
pub fn ota_builtin_webapp_url(root_url: String, channel: String, name: String, version: String) -> String {
  ota::manifest::OtaArtifactUrls::builtin_webapp(&root_url, &channel, &name, &version)
}

#[uniffi::export]
pub fn ota_daemon_patch_url(root_url: String, channel: String, to_version: String, from_version: String) -> String {
  ota::manifest::OtaArtifactUrls::daemon_patch(&root_url, &channel, &to_version, &from_version)
}
