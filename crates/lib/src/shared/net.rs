//! HTTP, WebSocket, and stream access via the phone. TLS ends at the phone, so the link to the
//! device carries plaintext.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "UPPERCASE")]
#[ts(export, export_to = "shared.ts")]
pub enum HttpMethod {
  Get,
  Head,
  Post,
  Put,
  Patch,
  Delete,
  Options,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct HttpHeader {
  pub name: String,
  pub value: String,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum RedirectPolicy {
  #[default]
  Follow,
  /// Return the 3xx response to the caller.
  Manual,
  Error,
}

#[serde_with::serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NetFetchRequest {
  pub url: String,
  pub method: HttpMethod,
  pub headers: Vec<HttpHeader>,
  #[serde_as(as = "Option<serde_with::Bytes>")]
  #[ts(type = "Uint8Array | null")]
  pub body: Option<Vec<u8>>,
  pub timeout_ms: Option<u32>,
  pub redirect: RedirectPolicy,
}

#[serde_with::serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NetFetchResponse {
  pub status: u16,
  pub headers: Vec<HttpHeader>,
  #[serde_as(as = "serde_with::Bytes")]
  #[ts(type = "Uint8Array")]
  pub body: Vec<u8>,
}

/// First event of a stream. `StreamChunk` and `StreamEnd` events with the same `streamId` follow.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct StreamBegin {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  pub status: u16,
  pub headers: Vec<HttpHeader>,
  /// Null when the server declares no length.
  pub total_size: Option<u32>,
}

#[serde_with::serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct StreamChunk {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  /// Byte position of `bytes[0]` in the full body. Chunks arrive in order.
  pub offset: u32,
  #[serde_as(as = "serde_with::Bytes")]
  #[ts(type = "Uint8Array")]
  pub bytes: Vec<u8>,
}

/// The last event of a stream that finished.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct StreamEnd {
  #[ts(type = "string")]
  pub stream_id: Uuid,
}

/// The last event of a stream that failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct StreamError {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  pub error: NetError,
}

#[serde_with::serde_as]
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum WsFrame {
  Text(String),
  Binary(
    #[serde_as(as = "serde_with::Bytes")]
    #[ts(type = "Uint8Array")]
    Vec<u8>,
  ),
}

impl std::fmt::Debug for WsFrame {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    const HEAD: usize = 4096;
    match self {
      Self::Text(s) if s.len() > HEAD => {
        let head: String = s.chars().take(HEAD).collect();
        write!(f, "Text({head:?}… <{} bytes total>)", s.len())
      }
      Self::Text(s) => write!(f, "Text({s:?})"),
      Self::Binary(b) => write!(f, "Binary(<{} bytes>)", b.len()),
    }
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum WsError {
  ConnectFailed { reason: String },
  FrameTooLarge,
  GatewayDisconnected,
  ProtocolError { reason: String },
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NetError {
  RequestFailed { reason: String },
  Timeout,
  Unavailable,
  NoGateway,
}
