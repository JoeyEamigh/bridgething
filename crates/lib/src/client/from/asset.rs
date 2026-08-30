use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// Returns the bytes for an asset id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Asset,
  request_variant = Get,
  response = crate::client::AssetGot,
  response_variant = Got,
  error = crate::client::AssetNotFound,
  error_variant = NotFound,
)]
pub struct AssetGet {
  pub id: String,
  /// Any uuid you choose.
  #[ts(type = "string")]
  pub request_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct AssetPreload {
  /// Only the first 64 are used.
  pub ids: Vec<String>,
}

/// Reads binary assets by id.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
pub enum ClientToBridgeAssetMsg {
  #[bridge_request]
  Get(AssetGet),
  #[bridge_command]
  Preload(AssetPreload),
}
