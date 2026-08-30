use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{HttpHeader, NetFetchRequest, WsFrame};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Net,
  request_variant = Fetch,
  response = crate::client::NetFetchReply,
  response_variant = FetchReply,
  error = crate::client::NetFetchErrorReply,
  error_variant = FetchErrorReply,
)]
/// Sends one HTTP request through the connected companion app and returns the response.
pub struct NetFetch {
  pub request: NetFetchRequest,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Net,
  request_variant = WsOpen,
  response = crate::client::NetWsOpenReply,
  response_variant = WsOpenReply,
  error = crate::client::NetWsErrorReply,
  error_variant = WsErrorReply,
)]
/// Opens a WebSocket through the connected companion app. Set `connectionId` yourself.
pub struct NetWsOpen {
  #[ts(type = "string")]
  pub connection_id: Uuid,
  pub url: String,
  /// Subprotocols offered to the server, in preference order.
  pub protocols: Option<Vec<String>>,
  pub headers: Option<Vec<HttpHeader>>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct NetWsClose {
  #[ts(type = "string")]
  pub connection_id: Uuid,
  pub code: Option<u16>,
  pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct NetWsSend {
  #[ts(type = "string")]
  pub connection_id: Uuid,
  pub frame: WsFrame,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct NetStreamOpen {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  pub request: NetFetchRequest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct NetStreamCancel {
  #[ts(type = "string")]
  pub stream_id: Uuid,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// HTTP requests, WebSockets, and byte streams, proxied through the connected companion app.
pub enum ClientToBridgeNetMsg {
  #[bridge_request]
  Fetch(NetFetch),
  #[bridge_request]
  WsOpen(NetWsOpen),
  #[bridge_command]
  WsClose(NetWsClose),
  #[bridge_command]
  WsSend(NetWsSend),
  #[bridge_command]
  StreamOpen(NetStreamOpen),
  #[bridge_command]
  StreamCancel(NetStreamCancel),
}
