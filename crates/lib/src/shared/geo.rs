use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Position {
  pub lat: f64,
  pub lon: f64,
  pub alt_m: Option<f32>,
  /// Uncertainty radius in meters.
  pub accuracy_m: f32,
  pub speed_mps: Option<f32>,
  pub heading_deg: Option<f32>,
  /// Fix time, not arrival time.
  pub ts_unix_s: u32,
}

/// `coarse` asks for a lower-power, less precise fix. Any open `fine` subscription raises it for all.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum GeoAccuracy {
  Coarse,
  #[default]
  Fine,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum GeoError {
  /// The phone denies location to the companion app.
  PermissionDenied,
  /// The active webapp must list `geo` in its manifest permissions.
  NotDeclared,
  /// The phone is connected but produced no fix.
  Unavailable,
  /// The token does not match an open subscription.
  UnknownToken,
}
