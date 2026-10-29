use bridgething_macros::{BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Returns the current screen presence state.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Presence,
  request_variant = Get,
  response = crate::client::PresenceSnapshot,
  response_variant = Snapshot,
)]
pub struct PresenceGet;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
pub enum ClientToBridgePresenceMsg {
  #[bridge_request]
  Get,
}
