use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{GeoError, Position};

/// Pass `token` to `unwatch` to stop the watch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct GeoWatchReply {
  pub token: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct GeoGetOnceReply {
  pub position: Position,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct GeoErrorReply {
  pub error: GeoError,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Position fixes from the connected phone. `watch` subscribes and `getOnce` returns a single fix.
/// Declare the `geo` permission in the webapp manifest, or both fail.
pub enum BridgeToClientGeoMsg {
  #[bridge_event]
  Position(Position),
  #[bridge_event]
  ErrorEvent(GeoErrorReply),
  #[bridge_response]
  WatchReply(GeoWatchReply),
  #[bridge_response]
  GetOnceReply(GeoGetOnceReply),
  #[bridge_response]
  ErrorReply(GeoErrorReply),
}
