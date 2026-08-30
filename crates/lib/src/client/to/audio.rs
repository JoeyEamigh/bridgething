use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::AudioError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct AudioErrorReply {
  pub error: AudioError,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct TtsStarted {
  #[ts(type = "string")]
  pub id: Uuid,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct TtsEnded {
  #[ts(type = "string")]
  pub id: Uuid,
  /// `false` when the speech was cancelled or interrupted.
  pub completed: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct VolumeChanged {
  /// Output level, `0.0` (silent) to `1.0` (max).
  pub level: f32,
  pub muted: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Volume, mute, speech, and short sounds on the device. `setVolume` and `setMute` set the output
/// level, `onVolumeChanged` reports every change, and `tts` speaks text.
pub enum BridgeToClientAudioMsg {
  #[bridge_event]
  TtsStarted(TtsStarted),
  #[bridge_event]
  TtsEnded(TtsEnded),
  #[bridge_event]
  VolumeChanged(VolumeChanged),
  #[bridge_event]
  ErrorEvent(AudioErrorReply),
}
