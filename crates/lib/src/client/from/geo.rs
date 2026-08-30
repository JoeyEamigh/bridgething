use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::GeoAccuracy;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Geo,
  request_variant = Watch,
  response = crate::client::GeoWatchReply,
  response_variant = WatchReply,
  error = crate::client::GeoErrorReply,
  error_variant = ErrorReply,
)]
/// Starts a position subscription. Fixes arrive as `onPosition` until you `unwatch` the token.
pub struct GeoWatch {
  pub accuracy: GeoAccuracy,
  pub min_interval_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct GeoUnwatch {
  pub token: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Geo,
  request_variant = GetOnce,
  response = crate::client::GeoGetOnceReply,
  response_variant = GetOnceReply,
  error = crate::client::GeoErrorReply,
  error_variant = ErrorReply,
)]
/// Returns a single position fix from the phone.
pub struct GeoGetOnce {
  pub accuracy: GeoAccuracy,
  /// Accept a held fix this many seconds old. `0` or null forces a fresh fix.
  #[serde(default)]
  pub max_age_s: Option<u32>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Reads position fixes from the connected phone.
pub enum ClientToBridgeGeoMsg {
  #[bridge_request]
  Watch(GeoWatch),
  #[bridge_command]
  Unwatch(GeoUnwatch),
  #[bridge_request]
  GetOnce(GeoGetOnce),
}
