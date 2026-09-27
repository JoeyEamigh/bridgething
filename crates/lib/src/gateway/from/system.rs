use bridgething_macros::{BridgeEnum, WireRequest};
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::transfer::{TransferBody, TransferRef};
use crate::{LauncherGesture, LogLevel, LogSource, OtaKind, RangePart};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct KeepaliveAck {
  pub seq: u32,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[wire_request(
  direction = GatewayToBridge,
  surface = System,
  request_variant = OtaBegin,
  response = crate::gateway::OtaBeginAck,
  response_variant = OtaBeginAck,
  error = crate::gateway::OtaBeginRejected,
  error_variant = OtaBeginRejected,
)]
pub struct OtaBegin {
  pub kind: OtaKind,
  pub update_id: String,
  pub update_url_base: Option<String>,
  pub transfer: TransferRef,
  pub patch: Option<OtaPatch>,
  pub provenance: Option<String>,
  pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub enum OtaPatchAlgorithm {
  ZstdPatchFrom,
  Zstd,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct OtaPatch {
  pub algorithm: OtaPatchAlgorithm,
  pub result_sha256: String,
  pub result_size: u32,
  pub source_sha256: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct OtaAbandon {
  pub update_id: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct OtaActivate {
  pub expected: Vec<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct OtaAssetRangeReply {
  pub total_size: u32,
  pub parts: Vec<RangePart>,
  pub body: TransferBody,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct OtaAssetRangeRejected {
  pub reason: String,
}

#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = GatewayToBridge,
  surface = System,
  request_variant = DeviceGetNickname,
  response = crate::gateway::DeviceNicknameReply,
  response_variant = DeviceNickname,
)]
pub struct DeviceGetNickname;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[wire_request(
  direction = GatewayToBridge,
  surface = System,
  request_variant = DeviceSetNickname,
  response = crate::gateway::DeviceNicknameReply,
  response_variant = DeviceNickname,
  error = crate::gateway::DeviceNicknameRejected,
  error_variant = DeviceNicknameRejected,
)]
pub struct DeviceSetNickname {
  pub nickname: String,
}

/// Returns the M-button gesture that jumps to the launcher.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = GatewayToBridge,
  surface = System,
  request_variant = LauncherGestureGet,
  response = crate::gateway::LauncherGestureReply,
  response_variant = LauncherGestureReply,
)]
pub struct LauncherGestureGet;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct LauncherGestureSet {
  pub gesture: LauncherGesture,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[wire_request(
  direction = GatewayToBridge,
  surface = System,
  request_variant = LogsTail,
  response = crate::gateway::LogsTailReply,
  response_variant = LogsTailReply,
)]
pub struct LogsTail {
  pub source: LogSource,
  pub levels: Vec<LogLevel>,
  pub filter: Option<String>,
  pub max_lines: u32,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[wire_request(
  direction = GatewayToBridge,
  surface = System,
  request_variant = LogsSubscribe,
  response = crate::gateway::LogsSubscribeReply,
  response_variant = LogsSubscribeReply,
)]
pub struct LogsSubscribe {
  pub source: LogSource,
  pub levels: Vec<LogLevel>,
  pub filter: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct LogsUnsubscribe {
  pub token: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[bridge_enum(into = crate::gateway::GatewayToBridgeMsgData)]
pub enum GatewayToBridgeSystemMsg {
  #[bridge_request]
  OtaBegin(OtaBegin),
  #[bridge_command]
  OtaAbandon(OtaAbandon),
  #[bridge_command]
  OtaActivate(OtaActivate),
  #[bridge_command]
  CancelUpdate,
  #[bridge_response]
  OtaAssetRangeReply(OtaAssetRangeReply),
  #[bridge_response]
  OtaAssetRangeRejected(OtaAssetRangeRejected),
  #[bridge_request]
  DeviceGetNickname,
  #[bridge_request]
  DeviceSetNickname(DeviceSetNickname),
  #[bridge_request]
  LauncherGestureGet,
  #[bridge_command]
  LauncherGestureSet(LauncherGestureSet),
  #[bridge_request]
  LogsTail(LogsTail),
  #[bridge_request]
  LogsSubscribe(LogsSubscribe),
  #[bridge_command]
  LogsUnsubscribe(LogsUnsubscribe),
  #[bridge_response]
  KeepaliveAck(KeepaliveAck),
}
