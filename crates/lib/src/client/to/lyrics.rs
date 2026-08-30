use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Lyrics;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// `lyrics` is null when the lookup worked and no lyrics exist for the track.
pub struct LyricsReply {
  /// Compare with your now-playing state to drop a late reply.
  pub track_uri: Option<String>,
  /// Use this when the item has no uri.
  pub track_persistent_id: Option<String>,
  pub lyrics: Option<Lyrics>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// Retry only `lookupFailed`.
pub enum LyricsError {
  /// No companion app is connected.
  NoGateway,
  /// The connected companion app has no lyrics source.
  NotSupported,
  NothingPlaying,
  /// The playing item carries no artist or title to look up.
  TrackUnidentifiable,
  LookupFailed {
    reason: String,
  },
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LyricsErrorReply {
  pub error: LyricsError,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Lyrics for the playing track. Check `capabilities.available.lyrics` before you offer a lyrics view.
pub enum BridgeToClientLyricsMsg {
  #[bridge_response]
  LyricsReply(LyricsReply),
  #[bridge_response]
  LyricsErrorReply(LyricsErrorReply),
}
