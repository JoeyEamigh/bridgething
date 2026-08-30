use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{LogLevel, LogSource};

/// Returns the daemon's version and identity as `BridgeThingMeta`.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = System,
  request_variant = VersionRequest,
  response = crate::BridgeThingMeta,
  response_variant = Version,
  boxed_response,
)]
pub struct RequestVersion;

/// Returns disk and memory use, uptime, SoC temperature, load average, and versions.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = System,
  request_variant = DiagnosticsGet,
  response = crate::client::DiagnosticsReply,
  response_variant = DiagnosticsReply,
)]
pub struct DiagnosticsGet;

/// Returns a batch of recent log entries, narrowed by `levels` and `filter`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = System,
  request_variant = LogsTail,
  response = crate::client::LogsTailReply,
  response_variant = LogsTailReply,
)]
pub struct LogsTail {
  pub source: LogSource,
  /// The levels to include. An empty array matches every level.
  pub levels: Vec<LogLevel>,
  /// A case-sensitive substring matched against `target` and `message`. `null` matches every entry.
  pub filter: Option<String>,
  pub max_lines: u32,
}

/// Starts a live log stream and returns a token. Pass it to `logsUnsubscribe` to stop.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = System,
  request_variant = LogsSubscribe,
  response = crate::client::LogsSubscribeReply,
  response_variant = LogsSubscribeReply,
)]
pub struct LogsSubscribe {
  pub source: LogSource,
  pub levels: Vec<LogLevel>,
  pub filter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LogsUnsubscribe {
  pub token: String,
}

/// Returns the device nickname. Listen for `onDeviceNicknameChanged` to track later changes.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = System,
  request_variant = DeviceGetNickname,
  response = crate::client::DeviceNicknameReply,
  response_variant = DeviceNickname,
)]
pub struct DeviceGetNickname;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
pub enum ClientToBridgeSystemMsg {
  #[bridge_request]
  VersionRequest,
  #[bridge_request]
  DiagnosticsGet,
  #[bridge_request]
  LogsTail(LogsTail),
  #[bridge_request]
  LogsSubscribe(LogsSubscribe),
  #[bridge_command]
  LogsUnsubscribe(LogsUnsubscribe),
  #[bridge_command]
  Reboot,
  #[bridge_command]
  PowerOff,
  #[bridge_command]
  FactoryReset,
  #[bridge_request]
  DeviceGetNickname,
}
