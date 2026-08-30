use bridgething_macros::{BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Returns one config value for the active webapp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Config,
  request_variant = Get,
  response = crate::client::ConfigGetReply,
  response_variant = Get,
)]
pub struct ConfigGet {
  pub key: String,
}

/// Returns every config value set for the active webapp.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Config,
  request_variant = List,
  response = crate::client::ConfigListReply,
  response_variant = List,
)]
pub struct ConfigList;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Reads the settings the companion app holds for the active webapp.
pub enum ClientToBridgeConfigMsg {
  #[bridge_request]
  Get(ConfigGet),
  #[bridge_request]
  List,
}
