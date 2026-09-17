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
  ForwardMessage,
  wire::{MsgMeta, WireError},
};

/// One message from a webapp to the daemon, sent over the local websocket on port 8891.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export, export_to = "client.ts")]
pub struct ClientToBridgeMsg {
  #[ts(type = "string")]
  pub id: Uuid,
  pub meta: MsgMeta,
  pub data: ClientToBridgeMsgData,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeOuterEnum)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub enum ClientToBridgeMsgData {
  #[from]
  Asset(ClientToBridgeAssetMsg),
  #[from]
  Audio(ClientToBridgeAudioMsg),
  #[from]
  Bluetooth(ClientToBridgeBluetoothMsg),
  #[from]
  Capabilities(ClientToBridgeCapabilitiesMsg),
  #[from]
  Config(ClientToBridgeConfigMsg),
  #[from]
  Doc(ClientToBridgeDocMsg),
  #[from]
  Geo(ClientToBridgeGeoMsg),
  #[from]
  Hardware(ClientToBridgeHardwareMsg),
  #[from]
  Library(ClientToBridgeLibraryMsg),
  #[from]
  Lyrics(ClientToBridgeLyricsMsg),
  #[from]
  Net(ClientToBridgeNetMsg),
  #[from]
  Notifications(ClientToBridgeNotificationsMsg),
  #[from]
  Phone(ClientToBridgePhoneMsg),
  #[from]
  Player(ClientToBridgePlayerMsg),
  #[from]
  Store(ClientToBridgeStoreMsg),
  #[from]
  System(ClientToBridgeSystemMsg),
  #[from]
  Time(ClientToBridgeTimeMsg),
  #[from]
  Voice(ClientToBridgeVoiceMsg),
  #[from]
  Webapp(ClientToBridgeWebappMsg),
  #[from]
  Forward(ForwardMessage),

  // legacy and stock app stuffs
  #[from]
  #[ts(skip)]
  LegacyStock(ClientLegacyStockCommand),
}

/// One message from the daemon to a webapp, sent over the local websocket on port 8891.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export, export_to = "client.ts")]
pub struct BridgeToClientMsg {
  #[ts(type = "string")]
  pub id: Uuid,
  pub meta: MsgMeta,
  pub data: BridgeToClientMsgData,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeOuterEnum)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub enum BridgeToClientMsgData {
  #[from]
  Asset(BridgeToClientAssetMsg),
  #[from]
  Audio(BridgeToClientAudioMsg),
  #[from]
  Bluetooth(BridgeToClientBluetoothMsg),
  #[from]
  Capabilities(BridgeToClientCapabilitiesMsg),
  #[from]
  Config(BridgeToClientConfigMsg),
  #[from]
  Doc(BridgeToClientDocMsg),
  #[from]
  Geo(BridgeToClientGeoMsg),
  #[from]
  Hardware(BridgeToClientHardwareMsg),
  #[from]
  Library(BridgeToClientLibraryMsg),
  #[from]
  Lyrics(BridgeToClientLyricsMsg),
  #[from]
  Net(BridgeToClientNetMsg),
  #[from]
  Notifications(BridgeToClientNotificationsMsg),
  #[from]
  Peer(BridgeToClientPeerMsg),
  #[from]
  Phone(BridgeToClientPhoneMsg),
  #[from]
  Player(BridgeToClientPlayerMsg),
  #[from]
  Store(BridgeToClientStoreMsg),
  #[from]
  System(BridgeToClientSystemMsg),
  #[from]
  Time(BridgeToClientTimeMsg),
  #[from]
  Voice(BridgeToClientVoiceMsg),
  #[from]
  Webapp(BridgeToClientWebappMsg),
  #[from]
  Forward(ForwardMessage),
  #[from]
  Error(WireError),
  /// The daemon accepted the command. No further reply follows.
  Ack,
  Done,
}
