use bridgething_delivery::ota::{
  event::{OtaPhaseSnapshot, OtaPlanStep, OtaPollEvent, ota_kind_slug},
  service::WebappInstallResult,
};
use libbridgething::{WebappInfo, gateway::WebappDocChanged};
use serde::Serialize;
use wasm_bindgen::JsValue;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Phase {
  pub kind: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub asset: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub received: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub sent: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub total: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rate_per_sec: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub eta_seconds: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub write_percent: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub reason: Option<String>,
}

impl Phase {
  fn named(kind: &str) -> Self {
    Phase {
      kind: kind.to_owned(),
      ..Phase::default()
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
        received: Some(received),
        total: Some(total),
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
        sent: Some(sent),
        total: Some(total),
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
  pub id: u32,
  pub kind: String,
  pub label: String,
  pub bytes: u64,
}

impl From<&OtaPlanStep> for PlanStep {
  fn from(step: &OtaPlanStep) -> Self {
    PlanStep {
      id: step.id,
      kind: step.kind.slug().to_owned(),
      label: step.label.clone(),
      bytes: step.bytes,
    }
  }
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEvent {
  pub kind: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub device_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub update_kind: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub release: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub daemon_version: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub image_version: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub channel: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub root_url: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub version: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub checked_at: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub reason: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub step_id: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub steps: Option<Vec<PlanStep>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub phase: Option<Phase>,
}

impl UpdateEvent {
  fn named(kind: &str) -> Self {
    UpdateEvent {
      kind: kind.to_owned(),
      ..UpdateEvent::default()
    }
  }
}

pub fn lagged(dropped: u64) -> UpdateEvent {
  UpdateEvent {
    reason: Some(format!("{dropped} events were dropped")),
    ..UpdateEvent::named("lagged")
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
        channel,
        root_url,
        steps,
      } => UpdateEvent {
        device_id: Some(device_id),
        update_kind: Some(ota_kind_slug(kind).to_owned()),
        release: Some(release),
        daemon_version: Some(daemon_version),
        image_version: Some(image_version),
        channel: Some(channel),
        root_url: Some(root_url),
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
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

pub fn install_result(result: WebappInstallResult) -> Result<InstalledWebapp, JsValue> {
  match result {
    WebappInstallResult::Installed(info) => Ok(InstalledWebapp::from(info.as_ref())),
    WebappInstallResult::Failed { reason } => Err(JsValue::from_str(&format!("install failed: {reason}"))),
  }
}

#[derive(Serialize)]
pub struct DocChange {
  pub id: String,
  pub key: String,
  pub value: Option<String>,
}

impl From<WebappDocChanged> for DocChange {
  fn from(change: WebappDocChanged) -> Self {
    DocChange {
      id: change.id.to_string(),
      key: change.key,
      value: change.value,
    }
  }
}

pub fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
  value
    .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
    .map_err(|e| JsValue::from_str(&e.to_string()))
}
