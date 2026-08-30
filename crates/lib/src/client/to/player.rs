use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{CurrentlyActiveApplication, NowPlayingUpdate, PlaybackTarget, PlayerError, PlayerState, QueueItem};

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PlayerStateReply {
  pub state: PlayerState,
  /// The app driving playback, when the phone reports it.
  pub active_app: Option<CurrentlyActiveApplication>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PlayerQueueReply {
  /// The now-playing track, when one is loaded.
  pub current: Option<QueueItem>,
  /// Upcoming tracks in queue order.
  pub items: Vec<QueueItem>,
  /// Recently-played history.
  pub previous: Vec<QueueItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PlayerErrorReply {
  pub error: PlayerError,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PlayerTargetsReply {
  pub targets: Vec<PlaybackTarget>,
}

/// Playback state and transport for a webapp. `onSnapshot` delivers the full `PlayerState` after
/// every change. `stateGet`, `queueGet`, and `targetsGet` return the same shapes on demand.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
pub enum BridgeToClientPlayerMsg {
  #[bridge_event]
  Snapshot(PlayerStateReply),
  #[bridge_event]
  Delta(NowPlayingUpdate),
  #[bridge_event]
  QueueChanged(PlayerQueueReply),
  #[bridge_event]
  TargetsChanged(PlayerTargetsReply),
  #[bridge_response]
  StateReply(PlayerStateReply),
  #[bridge_response]
  QueueReply(PlayerQueueReply),
  #[bridge_event]
  ErrorEvent(PlayerErrorReply),
  #[bridge_response]
  TargetsReply(PlayerTargetsReply),
  #[bridge_response]
  ErrorReply(PlayerErrorReply),
}
