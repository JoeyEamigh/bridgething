use bridgething_macros::BridgeEnum;
use bytes::Bytes;
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct AssetGot {
  #[ts(type = "string")]
  pub request_id: Uuid,
  pub id: String,
  #[debug(skip)]
  #[ts(type = "Uint8Array")]
  pub bytes: Bytes,
  /// Null when the source gives none.
  pub mime: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct AssetNotFound {
  #[ts(type = "string")]
  pub request_id: Uuid,
  pub id: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct AssetReady {
  pub id: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct AssetCleared {
  pub id: String,
}

/// Binary assets such as cover art, addressed by an opaque id. `get` returns bytes, `preload` fetches
/// ahead of time, and `onReady` and `onCleared` track which ids are available.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
pub enum BridgeToClientAssetMsg {
  #[bridge_response]
  Got(AssetGot),
  #[bridge_response]
  NotFound(AssetNotFound),
  #[bridge_event]
  Ready(AssetReady),
  #[bridge_event]
  Cleared(AssetCleared),
}
