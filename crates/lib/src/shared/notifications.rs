use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NotificationCategory {
  #[default]
  Other,
  IncomingCall,
  MissedCall,
  Voicemail,
  Social,
  Schedule,
  Email,
  News,
  HealthAndFitness,
  BusinessAndFinance,
  Location,
  Entertainment,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NotificationApp {
  /// For example `com.apple.MobileSMS`.
  pub bundle_id: String,
  pub display_name: Option<String>,
  pub icon_asset_id: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NotificationFlags {
  pub silent: bool,
  pub important: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NotificationAction {
  /// Button text, in the phone's language.
  pub label: String,
}

/// `id` stays the same while the notification exists. Pass it to `invokePositive` and
/// `invokeNegative`, and match it against `onRemoved`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Notification {
  pub id: String,
  pub app: NotificationApp,
  pub category: NotificationCategory,
  pub title: Option<String>,
  pub subtitle: Option<String>,
  pub message: Option<String>,
  pub timestamp_unix_s: Option<u32>,
  pub flags: NotificationFlags,
  pub positive_action: Option<NotificationAction>,
  pub negative_action: Option<NotificationAction>,
}

/// `acted` covers both the positive and the negative action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum DismissReason {
  UserDismissed,
  Acted,
  RemoteDismissed,
}

/// Whether the device can read notifications from a connected iPhone.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum AncsAuthState {
  /// No iPhone has attached since the device booted.
  #[default]
  Unknown,
  /// The device is still working out whether it has access.
  Probing,
  Authorized,
  Unauthorized,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NotificationsError {
  /// The notification is gone from the phone.
  NotFound { id: String },
  /// The notification has no action in that slot, or the phone refused it.
  ActionRejected { reason: String },
  /// No phone is connected.
  NoTarget,
}
