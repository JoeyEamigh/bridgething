use bridgething_macros::{BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Capabilities,
  request_variant = Get,
  response = crate::client::CapabilitiesSnapshot,
  response_variant = Snapshot,
)]
pub struct CapabilitiesGet;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Reads what the connected companion app can do.
pub enum ClientToBridgeCapabilitiesMsg {
  #[bridge_request]
  Get,
}
