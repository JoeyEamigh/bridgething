use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PhoneCallStatus {
  Disconnected,
  Sending,
  Ringing,
  Connecting,
  Active,
  Held,
  Disconnecting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PhoneCallDirection {
  Incoming,
  Outgoing,
}

/// How the call is carried. A phone that does not distinguish bearers reports `telephony`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PhoneCallService {
  Unknown,
  Telephony,
  FaceTimeAudio,
  FaceTimeVideo,
}

/// `callId` stays the same for the life of the call. Pass it to `answer`, `decline`, `end`, `hold`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PhoneCall {
  pub call_id: String,
  /// E.164 form when the phone provides one.
  pub remote_id: String,
  pub display_name: String,
  pub status: PhoneCallStatus,
  pub direction: PhoneCallDirection,
  pub started_at_unix_s: Option<u32>,
  pub label: Option<String>,
  pub address_book_id: Option<String>,
  pub service: Option<PhoneCallService>,
  pub is_conferenced: Option<bool>,
  pub conference_group: Option<u8>,
}

/// Call waiting and conference calls produce more than one entry.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PhoneState {
  pub active_calls: Vec<PhoneCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum CallEndReason {
  Local,
  Remote,
  Missed,
  Declined,
  Failed { reason: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum RegistrationStatus {
  Unknown,
  NotRegistered,
  Searching,
  Denied,
  RegisteredHome,
  RegisteredRoaming,
  EmergencyCallsOnly,
}

/// Enable a call-control button only while its `*Available` flag is true. Treat a null flag as
/// unavailable.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct CommunicationsState {
  pub signal_strength: Option<u8>,
  pub registration_status: Option<RegistrationStatus>,
  pub airplane_mode: Option<bool>,
  pub carrier_name: Option<String>,
  pub cellular_supported: Option<bool>,
  pub telephony_enabled: Option<bool>,
  pub face_time_audio_enabled: Option<bool>,
  pub face_time_video_enabled: Option<bool>,
  pub mute_status: Option<bool>,
  pub current_call_count: Option<u8>,
  pub new_voicemail_count: Option<u8>,
  pub initiate_call_available: Option<bool>,
  pub end_and_accept_available: Option<bool>,
  pub hold_and_accept_available: Option<bool>,
  pub swap_available: Option<bool>,
  pub merge_available: Option<bool>,
  pub hold_available: Option<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PhoneError {
  /// The phone reports no call with this id.
  CallNotFound {
    call_id: String,
  },
  ActionRejected {
    reason: String,
  },
  /// No phone is connected.
  NoTarget,
  /// The verb's `*Available` flag was false when the action ran.
  Unavailable {
    verb: String,
  },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum DtmfTone {
  D0,
  D1,
  D2,
  D3,
  D4,
  D5,
  D6,
  D7,
  D8,
  D9,
  Star,
  Hash,
}

/// What to do with an existing call when answering a new one.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum AcceptCallAction {
  /// Answer the new call and hold the existing one.
  #[default]
  Accept,
  /// End the existing call and answer the new one.
  EndAndAccept,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum EndCallAction {
  #[default]
  End,
  EndAll,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum InitiateCallType {
  #[default]
  Destination,
  Voicemail,
  Redial,
}
