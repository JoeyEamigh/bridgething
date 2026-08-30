use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// Returns the installed and built-in webapps a user can switch to.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Webapp,
  request_variant = List,
  response = crate::client::WebappListReply,
  response_variant = ListReply,
)]
pub struct WebappList;

#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Webapp,
  request_variant = Current,
  response = crate::client::WebappCurrentReply,
  response_variant = CurrentReply,
)]
pub struct WebappCurrent;

/// Switches the device to another webapp. The device shows one webapp at a time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Webapp,
  request_variant = Activate,
  response = crate::client::WebappActiveReply,
  response_variant = ActiveReply,
  error = crate::WebappError,
  error_variant = WebappError,
)]
pub struct WebappActivate {
  /// An `id` from `list`.
  #[ts(type = "string")]
  pub id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Webapp,
  request_variant = Icon,
  response = crate::client::WebappIconReply,
  response_variant = IconReply,
  error = crate::WebappError,
  error_variant = WebappError,
)]
pub struct WebappIcon {
  #[ts(type = "string")]
  pub id: Uuid,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
pub enum ClientToBridgeWebappMsg {
  #[bridge_request]
  List,
  #[bridge_request]
  Current,
  #[bridge_request]
  Activate(WebappActivate),
  #[bridge_request]
  Icon(WebappIcon),
}
