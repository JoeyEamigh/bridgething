use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{DocEntry, WebappError};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct DocGetReply {
  pub key: String,
  /// Null when the key is unset.
  pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct DocListReply {
  pub entries: Vec<DocEntry>,
}

/// The stored value after a `set` or `delete`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct DocAck {
  pub key: String,
  pub value: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct DocChanged {
  pub key: String,
  /// Null when the entry was deleted.
  pub value: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Key/value state the active webapp shares with the companion app. `get`, `list`, `set`, and
/// `delete` read and write it, and `onChanged` reports the writes the companion app makes.
pub enum BridgeToClientDocMsg {
  #[bridge_response]
  Get(DocGetReply),
  #[bridge_response]
  List(DocListReply),
  #[bridge_response]
  Ack(DocAck),
  #[bridge_response]
  Error(WebappError),
  #[bridge_event]
  Changed(DocChanged),
}
