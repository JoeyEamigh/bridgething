use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::LauncherGesture;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LauncherGestureSet {
  pub gesture: LauncherGesture,
}

#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Input,
  request_variant = GetGesture,
  response = crate::client::LauncherGestureReply,
  response_variant = GetGestureReply,
)]
pub struct LauncherGestureGet;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Which M-button gesture jumps to the launcher. `setGesture` persists the
/// choice on the daemon; `getGesture` reads it back.
pub enum ClientToBridgeInputMsg {
  #[bridge_command]
  SetGesture(LauncherGestureSet),
  #[bridge_request]
  GetGesture,
}
