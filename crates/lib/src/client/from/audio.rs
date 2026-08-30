use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct SetVolume {
  /// Output level, `0.0` (silent) to `1.0` (max).
  pub level: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct SetMute {
  pub muted: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// `id` is any uuid you choose. `voice` comes from `capabilities.audio.voices`.
pub struct Tts {
  #[ts(type = "string")]
  pub id: Uuid,
  pub text: String,
  pub voice: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct TtsCancel {
  #[ts(type = "string")]
  pub id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct Earcon {
  /// One of `capabilities.audio.earcons`.
  pub name: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Controls output volume, speech, and short sounds on the device.
pub enum ClientToBridgeAudioMsg {
  #[bridge_command]
  VolumeUp,
  #[bridge_command]
  VolumeDown,
  #[bridge_command]
  SetVolume(SetVolume),
  #[bridge_command]
  MuteToggle,
  #[bridge_command]
  SetMute(SetMute),
  #[bridge_command]
  Tts(Tts),
  #[bridge_command]
  TtsCancel(TtsCancel),
  #[bridge_command]
  TtsCancelAll,
  #[bridge_command]
  Earcon(Earcon),
}
