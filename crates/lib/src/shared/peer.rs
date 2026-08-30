use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{Device, GatewayInfo};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Peer {
  pub device: Device,
  pub paired: bool,
  pub iap2: PeerIap2Status,
  pub companion: PeerCompanionStatus,
  pub display_name: Option<String>,
  pub language: Option<String>,
  pub uuid: Option<String>,
}

impl Peer {
  pub fn new(device: Device) -> Self {
    Self {
      device,
      paired: false,
      iap2: PeerIap2Status::None,
      companion: PeerCompanionStatus::None,
      display_name: None,
      language: None,
      uuid: None,
    }
  }

  /// True when the peer's iAP2 session is identified or its companion app is connected.
  pub fn has_useful_link(&self) -> bool {
    matches!(self.iap2, PeerIap2Status::Identified) || matches!(self.companion, PeerCompanionStatus::Connected { .. })
  }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PeerIap2Status {
  #[default]
  None,
  LinkUp,
  Authenticated,
  Identified,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PeerCompanionStatus {
  #[default]
  None,
  Pending,
  Connected(GatewayInfo),
}
