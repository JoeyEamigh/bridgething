use bridgething_macros::BridgeOuterEnum;
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

mod from;
mod to;

pub use from::*;
pub use to::*;

use crate::{
  BridgeThingMeta,
  wire::{MsgMeta, WireError},
};

/// gateway -> bridgething
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export, export_to = "gateway.ts")]
pub struct GatewayToBridgeMsg {
  #[ts(type = "string")]
  pub id: Uuid,
  pub meta: MsgMeta,
  pub data: GatewayToBridgeMsgData,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeOuterEnum)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub enum GatewayToBridgeMsgData {
  #[from]
  Asset(GatewayToBridgeAssetMsg),
  #[from]
  Audio(GatewayToBridgeAudioMsg),
  #[from]
  Authority(GatewayToBridgeAuthorityMsg),
  #[from]
  Capabilities(GatewayToBridgeCapabilitiesMsg),
  #[from]
  Chrome(GatewayToBridgeChromeMsg),
  #[from]
  Forward(GatewayToBridgeForwardMsg),
  #[from]
  Geo(GatewayToBridgeGeoMsg),
  #[from]
  Library(GatewayToBridgeLibraryMsg),
  #[from]
  Lyrics(GatewayToBridgeLyricsMsg),
  #[from]
  Net(GatewayToBridgeNetMsg),
  #[from]
  Notifications(GatewayToBridgeNotificationsMsg),
  #[from]
  Phone(GatewayToBridgePhoneMsg),
  #[from]
  Player(GatewayToBridgePlayerMsg),
  #[from]
  System(GatewayToBridgeSystemMsg),
  #[from]
  Time(GatewayToBridgeTimeMsg),
  #[from]
  Transfer(GatewayToBridgeTransferMsg),
  #[from]
  Tunnel(GatewayToBridgeTunnelMsg),
  #[from]
  Voice(GatewayToBridgeVoiceMsg),
  #[from]
  Webapp(GatewayToBridgeWebappMsg),
  #[from]
  Error(WireError),
}

/// bridgething -> gateway
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export, export_to = "gateway.ts")]
pub struct BridgeToGatewayMsg {
  #[ts(type = "string")]
  pub id: Uuid,
  pub meta: MsgMeta,
  pub data: BridgeToGatewayMsgData,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeOuterEnum)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub enum BridgeToGatewayMsgData {
  #[from]
  Version(Box<BridgeThingMeta>),
  #[from]
  Asset(BridgeToGatewayAssetMsg),
  #[from]
  Audio(BridgeToGatewayAudioMsg),
  #[from]
  Geo(BridgeToGatewayGeoMsg),
  #[from]
  Library(BridgeToGatewayLibraryMsg),
  #[from]
  Lyrics(BridgeToGatewayLyricsMsg),
  #[from]
  Net(BridgeToGatewayNetMsg),
  #[from]
  Notifications(BridgeToGatewayNotificationsMsg),
  #[from]
  Phone(BridgeToGatewayPhoneMsg),
  #[from]
  Player(BridgeToGatewayPlayerMsg),
  #[from]
  System(BridgeToGatewaySystemMsg),
  #[from]
  Transfer(BridgeToGatewayTransferMsg),
  #[from]
  Tunnel(BridgeToGatewayTunnelMsg),
  #[from]
  Voice(BridgeToGatewayVoiceMsg),
  #[from]
  Webapp(BridgeToGatewayWebappMsg),
  #[from]
  Forward(BridgeToGatewayForwardMsg),
  #[from]
  Error(WireError),
  Ack,
  Done,
}
