use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::BrightnessMode;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct DisplaySetMode {
  pub mode: BrightnessMode,
}

/// Call `displaySetMode` with `manual` first, or the level is ignored.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct DisplaySetLevel {
  /// Backlight level, `0.0` (dimmest) to `1.0` (brightest).
  pub level: f32,
}

#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Hardware,
  request_variant = StateGet,
  response = crate::client::HardwareStateReply,
  response_variant = StateReply,
)]
pub struct HardwareStateGet;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Controls the display backlight.
pub enum ClientToBridgeHardwareMsg {
  #[bridge_command]
  DisplaySetMode(DisplaySetMode),
  #[bridge_command]
  DisplaySetLevel(DisplaySetLevel),
  #[bridge_request]
  StateGet,
}
