//! Wire primitives. The companion app speaks msgpack over Bluetooth, a webapp JSON over a WebSocket.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "wire.ts")]
pub struct ResponseMeta {
  #[ts(type = "string")]
  pub request_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "wire.ts")]
pub enum MsgMeta {
  Command,
  Event,
  Request,
  Response(ResponseMeta),
}

/// A request failed before a handler could answer it.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "wire.ts")]
pub enum WireError {
  /// The receiver serves no operation for this request.
  Unsupported,
  /// The receiver knows this request but has no backend behind it.
  Unimplemented,
  Malformed {
    reason: String,
  },
  HandlerFailed {
    reason: String,
  },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError<E> {
  Domain(E),
  Protocol(WireError),
  ResponseMismatch,
}

/// A one-way message the sender emits.
pub trait WireEvent<W>: Into<W> {}

/// A one-way message that asks the receiver to act.
pub trait WireCommand<W>: Into<W> {}

pub trait WireRequest: Sized + Into<Self::Outbound> {
  type Outbound;
  type Inbound;
  type Response;
  type DomainError;

  fn extract(data: Self::Inbound) -> Result<Self::Response, RequestError<Self::DomainError>>;
  fn encode_response(response: Self::Response) -> Self::Inbound;
  fn encode_domain_error(err: Self::DomainError) -> Self::Inbound;
}
