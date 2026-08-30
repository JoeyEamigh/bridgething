use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::TimeInfo;

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct TimeSnapshot {
  pub time: TimeInfo,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Wall clock, locale, and timezone for a webapp. The connected phone supplies all three. `get`
/// returns them on demand and `onChanged` reports every later update.
pub enum BridgeToClientTimeMsg {
  #[bridge_event]
  Changed(TimeSnapshot),
  #[bridge_response]
  Snapshot(TimeSnapshot),
}
