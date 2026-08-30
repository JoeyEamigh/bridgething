use bridgething_macros::{BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Store,
  request_variant = Get,
  response = crate::client::StorageResponse,
  response_variant = Response,
)]
pub struct KVGet {
  pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Store,
  request_variant = Put,
  response = crate::client::StorageResponse,
  response_variant = Response,
)]
pub struct KVPut {
  pub key: String,
  pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Store,
  request_variant = Delete,
  response = crate::client::StorageResponse,
  response_variant = Response,
)]
pub struct KVDelete {
  pub key: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
pub enum ClientToBridgeStoreMsg {
  #[bridge_request]
  Get(KVGet),
  #[bridge_request]
  Put(KVPut),
  #[bridge_request]
  Delete(KVDelete),
}
