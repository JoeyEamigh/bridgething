use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ConfigEntry;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct ConfigGetReply {
  pub key: String,
  /// Null when the key is unset.
  pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct ConfigListReply {
  pub entries: Vec<ConfigEntry>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct ConfigChanged {
  pub key: String,
  /// Null when the entry was deleted.
  pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Read-only settings the companion app holds for the active webapp. `get` and `list` read them,
/// `onChanged` reports every write.
pub enum BridgeToClientConfigMsg {
  #[bridge_response]
  Get(ConfigGetReply),
  #[bridge_response]
  List(ConfigListReply),
  #[bridge_event]
  Changed(ConfigChanged),
}
