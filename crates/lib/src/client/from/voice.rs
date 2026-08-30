use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// `preserve` keeps a voice capture that is already running.
pub struct MicMute {
  pub preserve: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct MicUnmute {
  pub preserve: bool,
}

/// Returns the current `VoiceState`.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Voice,
  request_variant = StateGet,
  response = crate::client::VoiceStateReply,
  response_variant = StateReply,
)]
pub struct VoiceStateGet;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Mic control and manual voice capture for a webapp.
pub enum ClientToBridgeVoiceMsg {
  #[bridge_command]
  Cancel,
  #[bridge_command]
  PushToTalk,
  #[bridge_command]
  Release,
  #[bridge_command]
  MuteMic(MicMute),
  #[bridge_command]
  UnmuteMic(MicUnmute),
  #[bridge_request]
  StateGet,
}
