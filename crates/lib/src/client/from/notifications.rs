use bridgething_macros::{BridgeDispatch, BridgeEnum};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct NotificationInvoke {
  pub id: String,
}

/// Runs the actions a phone notification offers.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
pub enum ClientToBridgeNotificationsMsg {
  #[bridge_command]
  InvokePositive(NotificationInvoke),
  #[bridge_command]
  InvokeNegative(NotificationInvoke),
}
