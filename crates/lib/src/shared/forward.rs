use bridgething_macros::WireEvent;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[serde_with::serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireEvent)]
#[wire(BridgeToClient, ClientToBridge)]
#[serde(
  tag = "encoding",
  content = "data",
  rename_all = "camelCase",
  rename_all_fields = "camelCase"
)]
#[ts(export, export_to = "shared.ts")]
pub enum ForwardMessage {
  Text(String),
  Json(#[ts(type = "unknown")] serde_json::Value),
  Binary(
    #[serde_as(as = "serde_with::Bytes")]
    #[ts(type = "Uint8Array")]
    Vec<u8>,
  ),
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct ForwardRouted {
  #[ts(type = "string")]
  pub webapp: Uuid,
  pub message: ForwardMessage,
}
