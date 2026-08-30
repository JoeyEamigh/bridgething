use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Returns the lyrics for the track that is playing.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Lyrics,
  request_variant = Get,
  response = crate::client::LyricsReply,
  response_variant = LyricsReply,
  error = crate::client::LyricsErrorReply,
  error_variant = LyricsErrorReply,
)]
pub struct LyricsGet;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
pub enum ClientToBridgeLyricsMsg {
  #[bridge_request]
  Get,
}
