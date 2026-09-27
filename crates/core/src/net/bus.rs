use std::net::SocketAddr;

use libbridgething::{
  client::{BridgeToClientMsgData, ClientToBridgeMsgData},
  wire::{MsgMeta, RequestError, WireCommand, WireEvent, WireRequest},
};
use uuid::Uuid;

use super::{ClientMan, ClientScope, WSError, WSResult};
use crate::stock::StockSendMsg;

#[derive(Debug, Clone)]
pub struct WireEventBus {
  client_man: ClientMan,
}

impl WireEventBus {
  pub fn new(client_man: ClientMan) -> Self {
    Self { client_man }
  }

  pub fn set_stock_phone(&self, phone: crate::stock::StockDeviceType) {
    self.client_man.set_stock_phone(phone);
  }

  pub async fn broadcast(
    &self,
    data: impl Into<BridgeToClientMsgData> + Clone,
    meta: MsgMeta,
  ) -> Result<(), Vec<WSError>> {
    self.client_man.broadcast(data, meta).await
  }

  pub async fn broadcast_event<E: WireEvent<BridgeToClientMsgData> + Clone>(
    &self,
    event: E,
  ) -> Result<(), Vec<WSError>> {
    self.client_man.broadcast_event(event).await
  }

  pub async fn broadcast_event_to_scopes<E: WireEvent<BridgeToClientMsgData> + Clone>(
    &self,
    scopes: &[ClientScope],
    event: E,
  ) -> Result<(), Vec<WSError>> {
    self.client_man.broadcast_event_to_scopes(scopes, event).await
  }

  pub async fn broadcast_command<C: WireCommand<BridgeToClientMsgData> + Clone>(
    &self,
    cmd: C,
  ) -> Result<(), Vec<WSError>> {
    self.client_man.broadcast_command(cmd).await
  }

  pub async fn send(
    &self,
    id: Uuid,
    to: SocketAddr,
    data: impl Into<BridgeToClientMsgData>,
    meta: MsgMeta,
    stock_msg_id: Option<usize>,
  ) -> WSResult<()> {
    self.client_man.send(id, to, data, meta, stock_msg_id).await
  }

  pub async fn send_event<E: WireEvent<BridgeToClientMsgData>>(&self, to: SocketAddr, event: E) -> WSResult<()> {
    self.client_man.send_event(to, event).await
  }

  pub async fn send_command<C: WireCommand<BridgeToClientMsgData>>(&self, to: SocketAddr, cmd: C) -> WSResult<()> {
    self.client_man.send_command(to, cmd).await
  }

  pub async fn request<R>(&self, to: SocketAddr, req: R) -> Result<R::Response, RequestError<R::DomainError>>
  where
    R: WireRequest<Outbound = BridgeToClientMsgData, Inbound = ClientToBridgeMsgData>,
  {
    self.client_man.request(to, req).await
  }

  pub async fn send_stock(&self, to: SocketAddr, data: impl Into<StockSendMsg>) -> WSResult<()> {
    self.client_man.send_stock(to, data).await
  }

  pub async fn broadcast_stock(&self, data: impl Into<StockSendMsg> + Clone) -> Result<(), Vec<WSError>> {
    self.client_man.broadcast_stock(data).await
  }
}
