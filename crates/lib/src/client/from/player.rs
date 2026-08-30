use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{PlayContext, QueuePosition, RepeatMode};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PlayUri {
  /// The uri to play, such as `spotify:track:...`.
  pub uri: String,
  /// The album, playlist, or show to play `uri` within, so `skipNext` and `skipPrev` follow it.
  pub context: Option<PlayContext>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct QueueUri {
  pub uri: String,
  pub position: QueuePosition,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct SeekTo {
  /// Target playhead in milliseconds from track start.
  pub position_ms: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct SkipToIndex {
  /// 0-based index into the queue.
  pub index: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct SkipPrev {
  /// `true` restarts the current track once it is past the restart threshold.
  pub allow_seeking: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct SetShuffle {
  pub on: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct SetRepeat {
  pub mode: RepeatMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct SetSpeed {
  /// Playback rate; 1.0 is normal speed.
  pub speed: f32,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct SetCrossfade {
  /// Crossfade duration in milliseconds. `null` turns crossfade off.
  pub duration_ms: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Player,
  request_variant = StateGet,
  response = crate::client::PlayerStateReply,
  response_variant = StateReply,
)]
pub struct PlayerStateGet;

#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Player,
  request_variant = QueueGet,
  response = crate::client::PlayerQueueReply,
  response_variant = QueueReply,
)]
pub struct PlayerQueueGet;

/// Returns the playback targets the connected companion app can reach.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Player,
  request_variant = TargetsGet,
  response = crate::client::PlayerTargetsReply,
  response_variant = TargetsReply,
)]
pub struct PlayerTargetsGet;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct TransferTo {
  /// The `id` of a `PlaybackTarget` from `targetsGet`.
  pub target_id: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Playback control for a webapp.
pub enum ClientToBridgePlayerMsg {
  /// Starts playback of a uri, optionally within a context.
  #[bridge_command]
  Play(PlayUri),
  #[bridge_command]
  Queue(QueueUri),
  #[bridge_command]
  Pause,
  #[bridge_command]
  Resume,
  #[bridge_command]
  SkipNext,
  #[bridge_command]
  SkipPrev(SkipPrev),
  #[bridge_command]
  SkipToIndex(SkipToIndex),
  #[bridge_command]
  SeekTo(SeekTo),
  #[bridge_command]
  SetShuffle(SetShuffle),
  #[bridge_command]
  SetRepeat(SetRepeat),
  /// Changes playback speed. The connected companion app must support rate control.
  #[bridge_command]
  SetSpeed(SetSpeed),
  #[bridge_command]
  SetCrossfade(SetCrossfade),
  #[bridge_command]
  TransferTo(TransferTo),
  #[bridge_request]
  StateGet,
  #[bridge_request]
  QueueGet,
  #[bridge_request]
  TargetsGet,
}
