use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Capabilities;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct CapabilitiesSnapshot {
  pub capabilities: Capabilities,
}

/// What the connected companion app can do. `get` reads it, `onUpdate` delivers it as it changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
pub enum BridgeToClientCapabilitiesMsg {
  #[bridge_event]
  Update(CapabilitiesSnapshot),
  #[bridge_response]
  Snapshot(CapabilitiesSnapshot),
}
