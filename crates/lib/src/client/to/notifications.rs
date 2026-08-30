use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{DismissReason, Notification, NotificationsError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct NotificationsErrorReply {
  pub error: NotificationsError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct NotificationRemoved {
  pub id: String,
  pub reason: DismissReason,
}

/// Phone notifications mirrored to a webapp. `onPosted`, `onUpdated`, and `onRemoved` track the
/// list. `invokePositive` and `invokeNegative` run the actions a notification offers.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
pub enum BridgeToClientNotificationsMsg {
  #[bridge_event]
  Posted(Notification),
  #[bridge_event]
  Updated(Notification),
  #[bridge_event]
  Removed(NotificationRemoved),
  #[bridge_event]
  ErrorEvent(NotificationsErrorReply),
}
