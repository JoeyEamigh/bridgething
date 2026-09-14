use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::LauncherGesture;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Reports the M-button launcher gesture. `getGesture` reads the current
/// choice and `onGestureChanged` fires on every change.
pub enum BridgeToClientInputMsg {
  #[bridge_event]
  GestureChanged(LauncherGestureChanged),
  #[bridge_response]
  GetGestureReply(LauncherGestureReply),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LauncherGestureReply {
  pub gesture: LauncherGesture,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LauncherGestureChanged {
  pub gesture: LauncherGesture,
}
