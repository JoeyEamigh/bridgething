use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{AcceptCallAction, DtmfTone, EndCallAction, InitiateCallType, PhoneCallService};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PhoneCallAction {
  pub call_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// `endAndAccept` requires `endAndAcceptAvailable` on `CommunicationsState`.
pub struct PhoneAcceptAction {
  pub call_id: String,
  pub action: AcceptCallAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// `endAll` ends every active call.
pub struct PhoneEndAction {
  pub call_id: String,
  pub action: EndCallAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// Requires `initiateCallAvailable` on `CommunicationsState`.
pub struct PhoneInitiateAction {
  pub kind: InitiateCallType,
  /// The number or address to dial, for a `destination` call.
  pub destination_id: Option<String>,
  /// The bearer to place the call on. `null` lets the phone pick its default.
  pub service: Option<PhoneCallService>,
  pub address_book_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PhoneMuteAction {
  pub mute: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct PhoneDtmfAction {
  /// The `callId` to send the tone on. `null` targets the active call.
  pub call_id: Option<String>,
  pub tone: DtmfTone,
}

/// Returns the current `PhoneState`.
#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = ClientToBridge,
  surface = Phone,
  request_variant = StateGet,
  response = crate::client::PhoneStateReply,
  response_variant = StateReply,
)]
pub struct PhoneStateGet;

/// Call control for a webapp. The connected phone reports each outcome as an event.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
pub enum ClientToBridgePhoneMsg {
  #[bridge_command]
  Answer(PhoneCallAction),
  #[bridge_command]
  Accept(PhoneAcceptAction),
  #[bridge_command]
  Decline(PhoneCallAction),
  #[bridge_command]
  End(PhoneCallAction),
  #[bridge_command]
  EndTyped(PhoneEndAction),
  #[bridge_command]
  /// Requires `holdAvailable` on `CommunicationsState`.
  Hold(PhoneCallAction),
  #[bridge_command]
  Unhold(PhoneCallAction),
  #[bridge_command]
  Initiate(PhoneInitiateAction),
  #[bridge_command]
  /// Requires `swapAvailable` on `CommunicationsState`.
  Swap,
  #[bridge_command]
  /// Requires `mergeAvailable` on `CommunicationsState`.
  Merge,
  #[bridge_command]
  Mute(PhoneMuteAction),
  #[bridge_command]
  Dtmf(PhoneDtmfAction),
  #[bridge_request]
  StateGet,
}
