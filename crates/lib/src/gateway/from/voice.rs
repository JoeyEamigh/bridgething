use bridgething_macros::BridgeEnum;
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{NluResolvedIntent, NluStage, VoiceCaptureReason};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct VoiceMicOpen {
  pub reason: VoiceCaptureReason,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct VoiceDispatch {
  #[ts(type = "string")]
  pub stream_id: Uuid,
  pub resolved: Box<NluResolvedIntent>,
  pub stage: Option<NluStage>,
  pub asr_confidence: Option<f32>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[bridge_enum(into = crate::gateway::GatewayToBridgeMsgData)]
pub enum GatewayToBridgeVoiceMsg {
  #[bridge_command]
  MicOpen(VoiceMicOpen),
  #[bridge_command]
  MicClose,
  #[bridge_command]
  Dispatch(VoiceDispatch),
}
