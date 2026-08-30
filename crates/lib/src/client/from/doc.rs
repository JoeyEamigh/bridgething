use bridgething_macros::{BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Returns one doc value for the active webapp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Doc,
  request_variant = Get,
  response = crate::client::DocGetReply,
  response_variant = Get,
)]
pub struct DocGet {
  pub key: String,
}

/// Returns every doc value for the active webapp.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Doc,
  request_variant = List,
  response = crate::client::DocListReply,
  response_variant = List,
)]
pub struct DocList;

/// Writes a doc value. Keep it at or under 256 KiB.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Doc,
  request_variant = Set,
  response = crate::client::DocAck,
  response_variant = Ack,
  error = crate::WebappError,
  error_variant = Error,
)]
pub struct DocSet {
  pub key: String,
  pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Doc,
  request_variant = Delete,
  response = crate::client::DocAck,
  response_variant = Ack,
)]
pub struct DocDelete {
  pub key: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Reads and writes the key/value state the active webapp shares with the companion app.
pub enum ClientToBridgeDocMsg {
  #[bridge_request]
  Get(DocGet),
  #[bridge_request]
  List,
  #[bridge_request]
  Set(DocSet),
  #[bridge_request]
  Delete(DocDelete),
}
