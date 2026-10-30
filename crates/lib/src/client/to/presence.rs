use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{PresenceInfo, PresenceSettings, PresenceSettingsError};

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PresenceSnapshot {
  pub presence: PresenceInfo,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PresenceSettingsReply {
  pub settings: PresenceSettings,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PresenceSettingsErrorReply {
  pub error: PresenceSettingsError,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Whether the screen should be lit, as decided from the host display state. A webapp or native
/// renderer that draws every frame should stop drawing while `display` is `off`. `get` returns the
/// state on demand and `onChanged` reports every later change. `settingsGet` and `settingsSet` read
/// and change when the screen dims and sleeps, and `onSettingsChanged` reports every change.
pub enum BridgeToClientPresenceMsg {
  #[bridge_event]
  Changed(PresenceSnapshot),
  #[bridge_response]
  Snapshot(PresenceSnapshot),
  #[bridge_event]
  SettingsChanged(PresenceSettingsReply),
  #[bridge_response]
  SettingsReply(PresenceSettingsReply),
  #[bridge_response]
  SettingsErrorReply(PresenceSettingsErrorReply),
}
