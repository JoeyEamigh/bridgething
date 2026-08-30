use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{BridgeThingMeta, Diagnostics, LogEntry, OtaError, OtaFinished, OtaProgress};

/// `nickname` is `null` until someone sets one.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct DeviceNicknameReply {
  pub nickname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct DiagnosticsReply {
  pub diagnostics: Diagnostics,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LogsTailReply {
  /// Matching entries in chronological order (oldest first).
  pub entries: Vec<LogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LogsSubscribeReply {
  pub token: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Device identity, health, logs, and power control for a webapp. `versionRequest` and
/// `diagnosticsGet` return the daemon version and a health snapshot, `logsTail` returns a batch of
/// log entries, and `logsSubscribe` streams them as `onLogEntry`. `onOtaProgress` and
/// `onOtaFinished` track a software update.
pub enum BridgeToClientSystemMsg {
  #[bridge_response]
  Version(Box<BridgeThingMeta>),
  #[bridge_response]
  DiagnosticsReply(DiagnosticsReply),
  #[bridge_response]
  LogsTailReply(LogsTailReply),
  #[bridge_response]
  LogsSubscribeReply(LogsSubscribeReply),
  #[bridge_event]
  LogEntry(LogEntry),
  #[bridge_event]
  OtaProgress(OtaProgress),
  #[bridge_event]
  OtaError(OtaError),
  #[bridge_event]
  OtaFinished(OtaFinished),
  #[bridge_response]
  DeviceNickname(DeviceNicknameReply),
  #[bridge_event]
  DeviceNicknameChanged(DeviceNicknameReply),
}
