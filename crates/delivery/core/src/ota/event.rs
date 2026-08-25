use libbridgething::{OtaKind, OtaPhase};

pub const CANCELLED_REASON: &str = "cancelled";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OtaStepKind {
  Download,
  Stream,
  Apply,
  Reboot,
}

impl OtaStepKind {
  pub fn slug(self) -> &'static str {
    match self {
      OtaStepKind::Download => "download",
      OtaStepKind::Stream => "stream",
      OtaStepKind::Apply => "apply",
      OtaStepKind::Reboot => "reboot",
    }
  }
}

pub fn ota_kind_slug(kind: OtaKind) -> &'static str {
  match kind {
    OtaKind::Image => "image",
    OtaKind::Daemon => "daemon",
    OtaKind::BuiltinWebapp => "builtinWebapp",
    OtaKind::InstalledWebapp => "installedWebapp",
    OtaKind::WakewordModel => "wakewordModel",
  }
}

pub fn parse_ota_kind(slug: &str) -> Option<OtaKind> {
  [
    OtaKind::Image,
    OtaKind::Daemon,
    OtaKind::BuiltinWebapp,
    OtaKind::InstalledWebapp,
    OtaKind::WakewordModel,
  ]
  .into_iter()
  .find(|kind| ota_kind_slug(*kind) == slug)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtaPlanStep {
  pub id: u32,
  pub kind: OtaStepKind,
  pub label: String,
  pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
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
    phase: OtaPhase,
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

impl OtaPhaseSnapshot {
  pub fn kind_name(&self) -> &'static str {
    match self {
      OtaPhaseSnapshot::Idle => "idle",
      OtaPhaseSnapshot::Downloading { .. } => "downloading",
      OtaPhaseSnapshot::Streaming { .. } => "streaming",
      OtaPhaseSnapshot::Applying { .. } => "applying",
      OtaPhaseSnapshot::Staged => "staged",
      OtaPhaseSnapshot::Completed => "completed",
      OtaPhaseSnapshot::Failed { .. } => "failed",
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
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

impl OtaPollEvent {
  pub fn device_id(&self) -> Option<&str> {
    match self {
      OtaPollEvent::ManifestPolled { .. } | OtaPollEvent::ManifestPollFailed { .. } => None,
      OtaPollEvent::UpdateAvailable { device_id, .. }
      | OtaPollEvent::Planned { device_id, .. }
      | OtaPollEvent::Progress { device_id, .. }
      | OtaPollEvent::Updated { device_id, .. }
      | OtaPollEvent::Failed { device_id, .. } => Some(device_id),
    }
  }

  pub fn kind_name(&self) -> &'static str {
    match self {
      OtaPollEvent::ManifestPolled { .. } => "manifestPolled",
      OtaPollEvent::ManifestPollFailed { .. } => "manifestPollFailed",
      OtaPollEvent::UpdateAvailable { .. } => "updateAvailable",
      OtaPollEvent::Planned { .. } => "planned",
      OtaPollEvent::Progress { .. } => "progress",
      OtaPollEvent::Updated { .. } => "updated",
      OtaPollEvent::Failed { .. } => "failed",
    }
  }
}
