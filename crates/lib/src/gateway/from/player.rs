use bridgething_macros::BridgeEnum;
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{PlaybackTarget, PlayerError, PlayerState, QueueItem};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct PlayerErrorReply {
  pub error: PlayerError,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct QueueSnapshot {
  pub order: Vec<String>,
  pub items: Vec<QueueItem>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct PlaybackTargets {
  pub targets: Vec<PlaybackTarget>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct PlayerSnapshotAck {}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct SpotifyWakeRequest {
  pub allow_play_tap: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[bridge_enum(into = crate::gateway::GatewayToBridgeMsgData)]
pub enum GatewayToBridgePlayerMsg {
  #[bridge_event]
  Snapshot(Box<PlayerState>),
  #[bridge_event]
  QueueChanged(QueueSnapshot),
  #[bridge_event]
  TargetsChanged(PlaybackTargets),
  #[bridge_event]
  ErrorEvent(PlayerErrorReply),
  #[bridge_command]
  RequestSpotifyWake(SpotifyWakeRequest),
  #[bridge_response]
  SnapshotAck(PlayerSnapshotAck),
  #[bridge_response]
  ErrorReply(PlayerErrorReply),
}
