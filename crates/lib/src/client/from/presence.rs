use bridgething_macros::{BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::PresenceSettings;

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

/// Returns when the screen dims and sleeps.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Presence,
  request_variant = SettingsGet,
  response = crate::client::PresenceSettingsReply,
  response_variant = SettingsReply,
)]
pub struct PresenceSettingsGet;

/// Replaces when the screen dims and sleeps. A request the device cannot honor, such as dimming
/// at or after sleeping, is refused with the reason and changes nothing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Presence,
  request_variant = SettingsSet,
  response = crate::client::PresenceSettingsReply,
  response_variant = SettingsReply,
  error = crate::client::PresenceSettingsErrorReply,
  error_variant = SettingsErrorReply,
)]
pub struct PresenceSettingsSet {
  pub settings: PresenceSettings,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
pub enum ClientToBridgePresenceMsg {
  #[bridge_request]
  Get,
  #[bridge_request]
  SettingsGet,
  #[bridge_request]
  SettingsSet(PresenceSettingsSet),
}
