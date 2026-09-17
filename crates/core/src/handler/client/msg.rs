use std::net::SocketAddr;

use libbridgething::{
  ForwardMessage,
  client::{
    BridgeToClientMsg, ClientLegacyStockCommand, ClientToBridgeAssetMsg, ClientToBridgeAudioMsgCommand,
    ClientToBridgeBluetoothMsg, ClientToBridgeCapabilitiesMsgRequest, ClientToBridgeConfigMsgRequest,
    ClientToBridgeDocMsgRequest, ClientToBridgeGeoMsg, ClientToBridgeHardwareMsg, ClientToBridgeLibraryMsg,
    ClientToBridgeLyricsMsg, ClientToBridgeMsg, ClientToBridgeMsgData, ClientToBridgeNetMsg,
    ClientToBridgeNotificationsMsg, ClientToBridgePhoneMsg, ClientToBridgePlayerMsg, ClientToBridgeStoreMsgRequest,
    ClientToBridgeSystemMsg, ClientToBridgeTimeMsgRequest, ClientToBridgeVoiceMsg, ClientToBridgeWebappMsg,
  },
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::stock::{StockInterAppRecv, StockRecvMsg, StockSendMsg};

pub type RecvTx = tokio::sync::mpsc::Sender<RecvMsg>;
pub type RecvRx = tokio::sync::mpsc::Receiver<RecvMsg>;
pub type SendTx = tokio::sync::mpsc::Sender<PossibleSendMsg>;
pub type SendRx = tokio::sync::mpsc::Receiver<PossibleSendMsg>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMode {
  Modern,
  Stock,
}

// --- receives ---
#[derive(Debug)]
pub struct RecvMsg {
  pub id: Uuid,
  pub from: SocketAddr,
  pub data: RecvMsgData,

  pub stock_msg_id: Option<usize>,
}

#[derive(Debug)]
pub enum RecvMsgData {
  Asset(ClientToBridgeAssetMsg),
  Audio(ClientToBridgeAudioMsgCommand),
  Bluetooth(ClientToBridgeBluetoothMsg),
  Capabilities(ClientToBridgeCapabilitiesMsgRequest),
  Config(ClientToBridgeConfigMsgRequest),
  Doc(ClientToBridgeDocMsgRequest),
  Geo(ClientToBridgeGeoMsg),
  Hardware(ClientToBridgeHardwareMsg),
  Library(ClientToBridgeLibraryMsg),
  Lyrics(ClientToBridgeLyricsMsg),
  Net(ClientToBridgeNetMsg),
  Notifications(ClientToBridgeNotificationsMsg),
  Phone(ClientToBridgePhoneMsg),
  Player(ClientToBridgePlayerMsg),
  Store(ClientToBridgeStoreMsgRequest),
  System(ClientToBridgeSystemMsg),
  Time(ClientToBridgeTimeMsgRequest),
  Voice(ClientToBridgeVoiceMsg),
  Webapp(ClientToBridgeWebappMsg),
  Forward(ForwardMessage),

  // stock compatibility
  LegacyStock(ClientLegacyStockCommand),

  // typed-request response
  Response {
    request_id: Uuid,
    data: ClientToBridgeMsgData,
  },

  // ignored and unsupported
  Hole,
  Unsupported(PossibleRecvMsg),

  // metadata
  ChangeMode(ClientMode),

  // errors
  ConnectionClosed(u16, String),
  Error(axum::Error),
}

impl From<ClientToBridgeMsg> for RecvMsgData {
  fn from(msg: ClientToBridgeMsg) -> Self {
    match msg.data {
      ClientToBridgeMsgData::Asset(inner) => Self::Asset(inner),
      ClientToBridgeMsgData::Audio(inner) => match inner.into_command() {
        Some(cmd) => Self::Audio(cmd),
        None => Self::Hole,
      },
      ClientToBridgeMsgData::Bluetooth(inner) => Self::Bluetooth(inner),
      ClientToBridgeMsgData::Capabilities(inner) => match inner.into_request() {
        Some(req) => Self::Capabilities(req),
        None => Self::Hole,
      },
      ClientToBridgeMsgData::Config(inner) => match inner.into_request() {
        Some(req) => Self::Config(req),
        None => Self::Hole,
      },
      ClientToBridgeMsgData::Doc(inner) => match inner.into_request() {
        Some(req) => Self::Doc(req),
        None => Self::Hole,
      },
      ClientToBridgeMsgData::Geo(inner) => Self::Geo(inner),
      ClientToBridgeMsgData::Hardware(inner) => Self::Hardware(inner),
      ClientToBridgeMsgData::Library(inner) => Self::Library(inner),
      ClientToBridgeMsgData::Lyrics(inner) => Self::Lyrics(inner),
      ClientToBridgeMsgData::Net(inner) => Self::Net(inner),
      ClientToBridgeMsgData::Notifications(inner) => Self::Notifications(inner),
      ClientToBridgeMsgData::Phone(inner) => Self::Phone(inner),
      ClientToBridgeMsgData::Player(inner) => Self::Player(inner),
      ClientToBridgeMsgData::Store(inner) => match inner.into_request() {
        Some(req) => Self::Store(req),
        None => Self::Hole,
      },
      ClientToBridgeMsgData::System(inner) => Self::System(inner),
      ClientToBridgeMsgData::Time(inner) => match inner.into_request() {
        Some(req) => Self::Time(req),
        None => Self::Hole,
      },
      ClientToBridgeMsgData::Voice(inner) => Self::Voice(inner),
      ClientToBridgeMsgData::Webapp(inner) => Self::Webapp(inner),
      ClientToBridgeMsgData::Forward(data) => Self::Forward(data),
      ClientToBridgeMsgData::LegacyStock(msg) => Self::LegacyStock(msg),
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PossibleRecvMsg {
  Modern(ClientToBridgeMsg),
  Stock(StockRecvMsg),
  #[serde(rename_all = "camelCase")]
  StockInterApp {
    msg_id: usize,
    #[serde(flatten)]
    data: StockInterAppRecv,
    user_action: bool,
  },
}

impl From<PossibleRecvMsg> for RecvMsgData {
  fn from(recv: PossibleRecvMsg) -> Self {
    match recv {
      PossibleRecvMsg::Modern(msg) => msg.into(),
      PossibleRecvMsg::Stock(msg) => msg.into(),
      PossibleRecvMsg::StockInterApp { .. } => RecvMsgData::from_stock_inter_app_possible_recv(recv),
    }
  }
}

impl PossibleRecvMsg {
  pub fn uuid(&self) -> uuid::Uuid {
    match self {
      PossibleRecvMsg::Modern(msg) => msg.id,
      _ => uuid::Uuid::now_v7(),
    }
  }
}

// --- sends ---
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(untagged)]
pub enum PossibleSendMsg {
  Modern(Box<BridgeToClientMsg>),
  Stock(Box<StockSendMsg>),
}

impl From<BridgeToClientMsg> for PossibleSendMsg {
  fn from(msg: BridgeToClientMsg) -> Self {
    Self::Modern(Box::new(msg))
  }
}

impl From<StockSendMsg> for PossibleSendMsg {
  fn from(msg: StockSendMsg) -> Self {
    Self::Stock(Box::new(msg))
  }
}

impl PossibleSendMsg {
  pub fn from_send_msg(
    msg: BridgeToClientMsg,
    mode: &ClientMode,
    stock_msg_id: Option<usize>,
    call_slot: &crate::stock::StockCallSlot,
    phone: crate::stock::StockDeviceType,
  ) -> Self {
    match mode {
      ClientMode::Modern => msg.into(),
      ClientMode::Stock => crate::stock::server_event_to_stock(msg, stock_msg_id, call_slot, phone).into(),
    }
  }

  pub fn is_noop(&self) -> bool {
    matches!(self, Self::Stock(msg) if matches!(**msg, StockSendMsg::Unsupported))
  }
}
