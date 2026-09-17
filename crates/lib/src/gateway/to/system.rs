use bridgething_macros::{BridgeEnum, WireRequest};
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{LauncherGesture, LogEntry, OtaError, OtaFinished, OtaProgress, RangeSpec};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct OtaBeginAck {
  pub resume_from_offset: u32,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct OtaBeginRejected {
  pub reason: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct DeviceNicknameReply {
  pub nickname: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct DeviceNicknameRejected {
  pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct LauncherGestureReply {
  pub gesture: LauncherGesture,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[wire_request(
  direction = BridgeToGateway,
  surface = System,
  request_variant = OtaAssetRange,
  response = crate::gateway::OtaAssetRangeReply,
  response_variant = OtaAssetRangeReply,
  error = crate::gateway::OtaAssetRangeRejected,
  error_variant = OtaAssetRangeRejected,
)]
pub struct OtaAssetRange {
  pub update_id: String,
  pub asset: String,
  pub ranges: Vec<RangeSpec>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct OtaAssetRangeAbandon {
  #[ts(type = "string")]
  pub request_id: Uuid,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct LogsTailReply {
  pub entries: Vec<LogEntry>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct LogsSubscribeReply {
  pub token: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[wire_request(
  direction = BridgeToGateway,
  surface = System,
  request_variant = Keepalive,
  response = crate::gateway::KeepaliveAck,
  response_variant = KeepaliveAck,
)]
pub struct KeepalivePing {
  pub seq: u32,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[bridge_enum(into = crate::gateway::BridgeToGatewayMsgData)]
pub enum BridgeToGatewaySystemMsg {
  #[bridge_event]
  OtaProgress(OtaProgress),
  #[bridge_event]
  OtaError(OtaError),
  #[bridge_event]
  OtaFinished(OtaFinished),
  #[bridge_response]
  OtaBeginAck(OtaBeginAck),
  #[bridge_response]
  OtaBeginRejected(OtaBeginRejected),
  #[bridge_request]
  OtaAssetRange(OtaAssetRange),
  #[bridge_command]
  OtaAssetRangeAbandon(OtaAssetRangeAbandon),
  #[bridge_response]
  DeviceNickname(DeviceNicknameReply),
  #[bridge_response]
  DeviceNicknameRejected(DeviceNicknameRejected),
  #[bridge_event]
  DeviceNicknameChanged(DeviceNicknameReply),
  #[bridge_response]
  LauncherGestureReply(LauncherGestureReply),
  #[bridge_event]
  LauncherGestureChanged(LauncherGestureReply),
  #[bridge_response]
  LogsTailReply(LogsTailReply),
  #[bridge_response]
  LogsSubscribeReply(LogsSubscribeReply),
  #[bridge_event]
  LogEntry(LogEntry),
  #[bridge_request]
  Keepalive(KeepalivePing),
}
