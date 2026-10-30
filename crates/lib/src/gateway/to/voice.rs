use bridgething_macros::BridgeEnum;
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{VoiceCaptureReason, VoiceDispatchErrorCode, VoiceDispatchTarget};

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub enum VoiceCodec {
  #[default]
  Opus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct VoiceFormat {
  pub codec: VoiceCodec,
  pub sample_rate_hz: u32,
  pub channels: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct VoiceStreamOpen {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  pub format: VoiceFormat,
  #[serde(default, skip_serializing_if = "reason_is_default")]
  pub reason: VoiceCaptureReason,
}

fn reason_is_default(reason: &VoiceCaptureReason) -> bool {
  *reason == VoiceCaptureReason::default()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct VoiceFrame {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  pub seq: u32,
  #[debug(skip)]
  #[ts(type = "Uint8Array")]
  pub packet: bytes::Bytes,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub enum VoiceCloseReason {
  #[default]
  EndOfSpeech,
  Cancelled,
  Muted,
  Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct VoiceStreamClose {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  pub reason: VoiceCloseReason,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct VoiceDispatched {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  pub target: VoiceDispatchTarget,
  pub intent: String,
  pub webapp_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct VoiceDispatchFailed {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  pub code: VoiceDispatchErrorCode,
  pub intent: String,
  pub msg: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[bridge_enum(into = crate::gateway::BridgeToGatewayMsgData)]
pub enum BridgeToGatewayVoiceMsg {
  #[bridge_event]
  StreamOpen(VoiceStreamOpen),
  #[bridge_event]
  Frame(VoiceFrame),
  #[bridge_event]
  StreamClose(VoiceStreamClose),
  #[bridge_event]
  Dispatched(VoiceDispatched),
  #[bridge_event]
  DispatchFailed(VoiceDispatchFailed),
}
