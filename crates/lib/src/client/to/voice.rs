use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{NluSlots, NluStage, VoiceCaptureReason, VoiceDispatchErrorCode, VoiceDispatchTarget};

/// Intents that ask the webapp to change what it shows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub enum VoiceDisplayIntent {
  Search,
  ShowView,
  MoreLikeThis,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct VoiceIntent {
  pub intent: VoiceDisplayIntent,
  #[serde(default)]
  pub slots: NluSlots,
  pub transcript: String,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub enum VoicePhase {
  #[default]
  Idle,
  Listening,
  Thinking,
  Done,
  Failed,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct VoiceState {
  pub muted: bool,
  pub capturing: bool,
  #[serde(default)]
  pub phase: VoicePhase,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// Why a voice turn ended early.
pub struct VoiceActivityError {
  pub code: VoiceDispatchErrorCode,
  pub msg: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// One step of a voice turn, reported as it happens.
pub struct VoiceActivity {
  pub phase: VoicePhase,
  #[ts(type = "string | null")]
  pub stream_id: Option<Uuid>,
  pub reason: Option<VoiceCaptureReason>,
  pub score: Option<f32>,
  pub transcript: Option<String>,
  pub intent: Option<String>,
  #[serde(default)]
  pub slots: NluSlots,
  pub stage: Option<NluStage>,
  pub target: Option<VoiceDispatchTarget>,
  pub error: Option<VoiceActivityError>,
}

impl VoiceActivity {
  pub fn new(phase: VoicePhase) -> Self {
    Self {
      phase,
      stream_id: None,
      reason: None,
      score: None,
      transcript: None,
      intent: None,
      slots: NluSlots::default(),
      stage: None,
      target: None,
      error: None,
    }
  }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct VoiceStateReply {
  pub state: VoiceState,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Voice capture and intent results for a webapp. `onStateChanged` reports mic mute and capture
/// state, `onActivity` reports each step of a voice turn, and `onIntent` delivers a resolved intent.
pub enum BridgeToClientVoiceMsg {
  #[bridge_event]
  StateChanged(VoiceState),
  #[bridge_event]
  Activity(VoiceActivity),
  #[bridge_event]
  Intent(VoiceIntent),
  #[bridge_response]
  StateReply(VoiceStateReply),
}
