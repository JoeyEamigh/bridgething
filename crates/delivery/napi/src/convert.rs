use bridgething_delivery::{
  bundle::BundleState,
  discovery::{Endpoint as CoreEndpoint, EndpointChange as CoreEndpointChange},
  ota::{
    event::{OtaPhaseSnapshot, OtaPlanStep, OtaPollEvent, ota_kind_slug},
    service::WebappInstallResult,
  },
};
use libbridgething::WebappInfo;
use napi_derive::napi;

#[napi(object)]
pub struct Phase {
  pub kind: String,
  pub asset: Option<String>,
  pub received: Option<f64>,
  pub sent: Option<f64>,
  pub total: Option<f64>,
  pub rate_per_sec: Option<f64>,
  pub eta_seconds: Option<f64>,
  pub write_percent: Option<u32>,
  pub reason: Option<String>,
}

impl Phase {
  fn named(kind: &str) -> Self {
    Phase {
      kind: kind.to_owned(),
      asset: None,
      received: None,
      sent: None,
      total: None,
      rate_per_sec: None,
      eta_seconds: None,
      write_percent: None,
      reason: None,
    }
  }
}

impl From<OtaPhaseSnapshot> for Phase {
  fn from(snapshot: OtaPhaseSnapshot) -> Self {
    let base = Phase::named(snapshot.kind_name());
    match snapshot {
      OtaPhaseSnapshot::Idle | OtaPhaseSnapshot::Staged | OtaPhaseSnapshot::Completed => base,
      OtaPhaseSnapshot::Downloading {
        asset,
        received,
        total,
        rate_per_sec,
      } => Phase {
        asset: Some(asset),
        received: Some(received as f64),
        total: Some(total as f64),
        rate_per_sec,
        ..base
      },
      OtaPhaseSnapshot::Streaming {
        asset,
        sent,
        total,
        rate_per_sec,
        eta_seconds,
      } => Phase {
        asset: Some(asset),
        sent: Some(sent as f64),
        total: Some(total as f64),
        rate_per_sec,
        eta_seconds,
        ..base
      },
      OtaPhaseSnapshot::Applying { phase, write_percent } => Phase {
        asset: Some(format!("{phase:?}")),
        write_percent: Some(write_percent),
        ..base
      },
      OtaPhaseSnapshot::Failed { reason } => Phase {
        reason: Some(reason),
        ..base
      },
    }
  }
}

#[napi(object)]
pub struct PlanStep {
  pub id: u32,
  pub kind: String,
  pub label: String,
  pub bytes: f64,
}

impl From<&OtaPlanStep> for PlanStep {
  fn from(step: &OtaPlanStep) -> Self {
    PlanStep {
      id: step.id,
      kind: step.kind.slug().to_owned(),
      label: step.label.clone(),
      bytes: step.bytes as f64,
    }
  }
}

#[napi(object)]
pub struct UpdateEvent {
  pub kind: String,
  pub device_id: Option<String>,
  pub update_kind: Option<String>,
  pub release: Option<String>,
  pub daemon_version: Option<String>,
  pub image_version: Option<String>,
  pub version: Option<String>,
  pub checked_at: Option<String>,
  pub reason: Option<String>,
  pub step_id: Option<u32>,
  pub steps: Option<Vec<PlanStep>>,
  pub phase: Option<Phase>,
}

impl UpdateEvent {
  fn named(kind: &str) -> Self {
    UpdateEvent {
      kind: kind.to_owned(),
      device_id: None,
      update_kind: None,
      release: None,
      daemon_version: None,
      image_version: None,
      version: None,
      checked_at: None,
      reason: None,
      step_id: None,
      steps: None,
      phase: None,
    }
  }
}

impl From<OtaPollEvent> for UpdateEvent {
  fn from(event: OtaPollEvent) -> Self {
    let base = UpdateEvent::named(event.kind_name());
    match event {
      OtaPollEvent::ManifestPolled { checked_at } => UpdateEvent {
        checked_at: Some(checked_at),
        ..base
      },
      OtaPollEvent::ManifestPollFailed { reason } => UpdateEvent {
        reason: Some(reason),
        ..base
      },
      OtaPollEvent::UpdateAvailable {
        device_id,
        release,
        daemon_version,
        image_version,
      } => UpdateEvent {
        device_id: Some(device_id),
        release: Some(release),
        daemon_version: Some(daemon_version),
        image_version: Some(image_version),
        ..base
      },
      OtaPollEvent::Planned {
        device_id,
        kind,
        release,
        daemon_version,
        image_version,
        channel: _,
        root_url: _,
        steps,
      } => UpdateEvent {
        device_id: Some(device_id),
        update_kind: Some(ota_kind_slug(kind).to_owned()),
        release: Some(release),
        daemon_version: Some(daemon_version),
        image_version: Some(image_version),
        steps: Some(steps.iter().map(PlanStep::from).collect()),
        ..base
      },
      OtaPollEvent::Progress {
        device_id,
        kind,
        step_id,
        snapshot,
      } => UpdateEvent {
        device_id: Some(device_id),
        update_kind: Some(ota_kind_slug(kind).to_owned()),
        step_id: Some(step_id),
        phase: Some(snapshot.into()),
        ..base
      },
      OtaPollEvent::Updated {
        device_id,
        kind,
        version,
      } => UpdateEvent {
        device_id: Some(device_id),
        update_kind: Some(ota_kind_slug(kind).to_owned()),
        version: Some(version),
        ..base
      },
      OtaPollEvent::Failed {
        device_id,
        kind,
        reason,
      } => UpdateEvent {
        device_id: Some(device_id),
        update_kind: Some(ota_kind_slug(kind).to_owned()),
        reason: Some(reason),
        ..base
      },
    }
  }
}

pub fn lagged(dropped: u64) -> UpdateEvent {
  UpdateEvent {
    reason: Some(format!("{dropped} events were dropped")),
    ..UpdateEvent::named("lagged")
  }
}

#[napi(object)]
pub struct InstalledWebapp {
  pub id: String,
  pub name: String,
  pub version: String,
  pub provenance: Option<String>,
}

impl From<&WebappInfo> for InstalledWebapp {
  fn from(info: &WebappInfo) -> Self {
    InstalledWebapp {
      id: info.id.to_string(),
      name: info.name.clone(),
      version: info.version.clone(),
      provenance: info.provenance.clone(),
    }
  }
}

pub fn install_result(result: WebappInstallResult) -> napi::Result<InstalledWebapp> {
  match result {
    WebappInstallResult::Installed(info) => Ok(InstalledWebapp::from(info.as_ref())),
    WebappInstallResult::Failed { reason } => Err(napi::Error::from_reason(format!("install failed: {reason}"))),
  }
}

#[napi(object)]
pub struct BundleStatus {
  pub state: String,
  pub version: Option<String>,
  pub received: Option<f64>,
  pub total: Option<f64>,
  pub reason: Option<String>,
  pub path: Option<String>,
}

pub fn bundle_status(state: BundleState, path: Option<String>) -> BundleStatus {
  match state {
    BundleState::Absent => BundleStatus {
      state: "absent".to_owned(),
      version: None,
      received: None,
      total: None,
      reason: None,
      path,
    },
    BundleState::Downloading { received, total } => BundleStatus {
      state: "downloading".to_owned(),
      version: None,
      received: Some(received as f64),
      total: Some(total as f64),
      reason: None,
      path,
    },
    BundleState::Ready { version } => BundleStatus {
      state: "ready".to_owned(),
      version: Some(version),
      received: None,
      total: None,
      reason: None,
      path,
    },
    BundleState::Failed { reason } => BundleStatus {
      state: "failed".to_owned(),
      version: None,
      received: None,
      total: None,
      reason: Some(reason),
      path,
    },
  }
}

#[napi(object)]
pub struct Endpoint {
  pub id: String,
  pub url: String,
  pub host: String,
  pub nickname: Option<String>,
}

#[napi(object)]
pub struct EndpointChange {
  pub kind: String,
  pub endpoint: Endpoint,
}

impl From<CoreEndpoint> for Endpoint {
  fn from(endpoint: CoreEndpoint) -> Self {
    Endpoint {
      id: endpoint.id,
      url: endpoint.url,
      host: endpoint.host,
      nickname: endpoint.nickname,
    }
  }
}

impl From<CoreEndpointChange> for EndpointChange {
  fn from(change: CoreEndpointChange) -> Self {
    let (kind, endpoint) = match change {
      CoreEndpointChange::Found(endpoint) => ("found", endpoint),
      CoreEndpointChange::Lost(endpoint) => ("lost", endpoint),
    };
    EndpointChange {
      kind: kind.to_owned(),
      endpoint: endpoint.into(),
    }
  }
}
