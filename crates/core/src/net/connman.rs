use std::{
  collections::HashMap,
  net::SocketAddr,
  sync::{Arc, Mutex},
  time::Duration,
};

use axum::extract::ws::WebSocket;
use dashmap::DashMap;
use libbridgething::{
  client::{BridgeToClientMsg, BridgeToClientMsgData, ClientToBridgeMsgData},
  wire::{MsgMeta, RequestError, ResponseMeta, WireCommand, WireError, WireEvent, WireRequest},
};
use tokio::{
  sync::{mpsc::error::TrySendError, oneshot},
  task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{WSError, WSResult, connection::Connection};
use crate::{
  handler::client::{ClientMode, PossibleSendMsg, RecvMsg, RecvMsgData, RecvRx, RecvTx, SendTx},
  state::State,
  stock::{StockCallSlot, StockDeviceType, StockPeerPhone, StockSendMsg},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(feature = "test-tap")]
const FRAME_TAP_CAPACITY: usize = 256;

#[cfg(feature = "test-tap")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TappedFrame {
  pub to: SocketAddr,
  pub mode: ClientMode,
  pub json: String,
}

#[cfg(feature = "test-tap")]
impl TappedFrame {
  pub fn json(&self) -> &str {
    &self.json
  }
}

#[derive(Debug)]
struct ClientData {
  tx: SendTx,
  mode: ClientMode,
  stock_call: StockCallSlot,

  _handle: JoinHandle<()>,
  cancel_token: CancellationToken,
}

pub fn create_client_manager() -> (ClientMan, ClientListener) {
  let (tx, rx) = tokio::sync::mpsc::channel(64);

  let client_man = Arc::new(ClientManager::new(tx));
  let listener = ClientListener::new(rx, client_man.clone());

  (client_man, listener)
}

#[derive(Debug)]
pub struct ClientListener {
  rx: RecvRx,
  client_man: ClientMan,
}

impl ClientListener {
  fn new(rx: RecvRx, client_man: ClientMan) -> Self {
    Self { rx, client_man }
  }

  /// cancel-safe
  pub async fn recv(&mut self) -> WSResult<RecvMsg> {
    loop {
      let msg = self.rx.recv().await.ok_or(WSError::ChannelClosed)?;
      tracing::trace!("new parsed message from {:?}", msg.from);

      if let RecvMsgData::ChangeMode(mode) = &msg.data {
        self.client_man.change_mode(&msg.from, mode);
      };

      if let RecvMsgData::ConnectionClosed(_, _) = msg.data {
        self.client_man.handle_disconnect(msg.from);
      };

      if let RecvMsgData::Response { request_id, data } = msg.data {
        if !self.client_man.complete_pending(&request_id, data) {
          tracing::warn!(
            "({:?}) stray response-meta message with no matching pending request {request_id}; dropping",
            msg.from
          );
        }
        continue;
      }

      return Ok(msg);
    }
  }
}

pub type ClientMan = Arc<ClientManager>;

#[derive(Debug)]
pub struct ClientManager {
  connections: DashMap<SocketAddr, ClientData>,
  cancel_token: CancellationToken,

  tx: RecvTx,
  pending: Mutex<HashMap<Uuid, oneshot::Sender<ClientToBridgeMsgData>>>,
  stock_phone: StockPeerPhone,

  #[cfg(feature = "test-tap")]
  frame_tap: tokio::sync::broadcast::Sender<TappedFrame>,
}

impl ClientManager {
  fn new(tx: RecvTx) -> Self {
    tracing::info!("creating connection manager");

    Self {
      connections: DashMap::new(),
      cancel_token: CancellationToken::new(),

      tx,
      pending: Mutex::new(HashMap::new()),
      stock_phone: StockPeerPhone::default(),

      #[cfg(feature = "test-tap")]
      frame_tap: tokio::sync::broadcast::channel(FRAME_TAP_CAPACITY).0,
    }
  }

  pub fn set_stock_phone(&self, phone: StockDeviceType) {
    self.stock_phone.set(phone);
  }

  #[cfg(feature = "test-tap")]
  pub fn subscribe_frames(&self) -> tokio::sync::broadcast::Receiver<TappedFrame> {
    self.frame_tap.subscribe()
  }

  #[cfg(feature = "test-tap")]
  pub fn client_count(&self) -> usize {
    self.connections.len()
  }

  pub fn change_mode(&self, from: &SocketAddr, mode: &ClientMode) {
    if let Some(mut client) = self.connections.get_mut(from) {
      client.mode = *mode;
    }
  }

  pub async fn send(
    &self,
    id: Uuid,
    to: SocketAddr,
    data: impl Into<BridgeToClientMsgData>,
    meta: MsgMeta,
    stock_msg_id: Option<usize>,
  ) -> WSResult<()> {
    let client = self.connections.get(&to).ok_or(WSError::NotConnected)?;
    let data = data.into();
    tracing::trace!("sending message to {to} with data {:?}", data);

    let msg = BridgeToClientMsg { id, data, meta };
    let msg = PossibleSendMsg::from_send_msg(
      msg,
      &client.mode,
      stock_msg_id,
      &client.stock_call,
      self.stock_phone.get(),
    );

    Ok(client.tx.send(msg).await?)
  }

  pub async fn broadcast(&self, data: impl Into<BridgeToClientMsgData>, meta: MsgMeta) -> Result<(), Vec<WSError>> {
    let data = data.into();

    let msg = BridgeToClientMsg {
      id: uuid::Uuid::now_v7(),
      data,
      meta,
    };

    let mut errors: Vec<WSError> = Vec::new();
    let mut closed: Vec<SocketAddr> = Vec::new();
    let phone = self.stock_phone.get();
    for c in self.connections.iter() {
      let out = PossibleSendMsg::from_send_msg(msg.clone(), &c.mode, None, &c.stock_call, phone);
      if let Err(err) = c.tx.try_send(out) {
        if matches!(err, TrySendError::Closed(_)) {
          closed.push(*c.key());
        }
        errors.push(WSError::from(err));
      }
    }
    self.prune_closed(&closed);

    if errors.is_empty() { Ok(()) } else { Err(errors) }
  }

  pub async fn send_event<E: WireEvent<BridgeToClientMsgData>>(&self, to: SocketAddr, event: E) -> WSResult<()> {
    self.send(Uuid::now_v7(), to, event.into(), MsgMeta::Event, None).await
  }

  pub async fn broadcast_event<E: WireEvent<BridgeToClientMsgData>>(&self, event: E) -> Result<(), Vec<WSError>> {
    self.broadcast(event.into(), MsgMeta::Event).await
  }

  pub async fn send_command<C: WireCommand<BridgeToClientMsgData>>(&self, to: SocketAddr, cmd: C) -> WSResult<()> {
    self.send(Uuid::now_v7(), to, cmd.into(), MsgMeta::Command, None).await
  }

  pub async fn broadcast_command<C: WireCommand<BridgeToClientMsgData>>(&self, cmd: C) -> Result<(), Vec<WSError>> {
    self.broadcast(cmd.into(), MsgMeta::Command).await
  }

  pub async fn request<R>(&self, to: SocketAddr, req: R) -> Result<R::Response, RequestError<R::DomainError>>
  where
    R: WireRequest<Outbound = BridgeToClientMsgData, Inbound = ClientToBridgeMsgData>,
  {
    let id = Uuid::now_v7();
    let (tx, rx) = oneshot::channel();
    self
      .pending
      .lock()
      .expect("pending-request map poisoned")
      .insert(id, tx);

    if let Err(err) = self.send(id, to, req.into(), MsgMeta::Request, None).await {
      self.pending.lock().expect("pending poisoned").remove(&id);
      return Err(RequestError::Protocol(WireError::HandlerFailed {
        reason: format!("send failed: {err:?}"),
      }));
    }

    match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
      Ok(Ok(data)) => R::extract(data),
      Ok(Err(_)) => {
        self.pending.lock().expect("pending poisoned").remove(&id);
        Err(RequestError::Protocol(WireError::HandlerFailed {
          reason: "response channel closed".into(),
        }))
      }
      Err(_) => {
        self.pending.lock().expect("pending poisoned").remove(&id);
        Err(RequestError::Protocol(WireError::HandlerFailed {
          reason: "request timed out".into(),
        }))
      }
    }
  }

  pub fn complete_pending(&self, request_id: &Uuid, data: ClientToBridgeMsgData) -> bool {
    let tx = self
      .pending
      .lock()
      .expect("pending-request map poisoned")
      .remove(request_id);
    if let Some(tx) = tx {
      let _ = tx.send(data);
      true
    } else {
      false
    }
  }

  pub fn complete_pending_meta(&self, meta: &ResponseMeta, data: ClientToBridgeMsgData) -> bool {
    self.complete_pending(&meta.request_id, data)
  }

  pub async fn send_stock(&self, to: SocketAddr, data: impl Into<StockSendMsg>) -> WSResult<()> {
    let client = self.connections.get(&to).ok_or(WSError::NotConnected)?;
    if client.mode != ClientMode::Stock {
      tracing::trace!("attempting to send stock message to non-stock device, ignoring...");
      return Ok(());
    }

    let msg = data.into();
    tracing::trace!("sending stock message to {to} with data {:?}", msg);

    Ok(client.tx.send(msg.into()).await?)
  }

  pub async fn broadcast_stock(&self, data: impl Into<StockSendMsg> + Clone) -> Result<(), Vec<WSError>> {
    let msg = data.into();

    let mut errors: Vec<WSError> = Vec::new();
    let mut closed: Vec<SocketAddr> = Vec::new();
    for c in self.connections.iter() {
      if c.mode != ClientMode::Stock {
        continue;
      }
      if let Err(err) = c.tx.try_send(msg.clone().into()) {
        if matches!(err, TrySendError::Closed(_)) {
          closed.push(*c.key());
        }
        errors.push(WSError::from(err));
      }
    }
    self.prune_closed(&closed);

    if errors.is_empty() { Ok(()) } else { Err(errors) }
  }

  fn prune_closed(&self, closed: &[SocketAddr]) {
    for addr in closed {
      if let Some((_addr, dead)) = self.connections.remove(addr) {
        dead.cancel_token.cancel();
        tracing::debug!("pruned closed client {addr} during broadcast");
      }
    }
  }

  /// NOT cancel-safe
  pub async fn handle_connection(
    &self,
    address: SocketAddr,
    ws: WebSocket,
    mode: ClientMode,
    state: &State,
  ) -> WSResult<()> {
    tracing::debug!("handling accepted websocket connection from {address}");

    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let cancel_token = self.cancel_token.child_token();

    let data = ClientData {
      tx,
      mode,
      stock_call: StockCallSlot::default(),

      _handle: Connection::spawn(
        address,
        ws,
        self.tx.clone(),
        rx,
        cancel_token.clone(),
        mode,
        #[cfg(feature = "test-tap")]
        self.frame_tap.clone(),
      ),
      cancel_token,
    };

    if data.mode == ClientMode::Stock {
      tracing::debug!("new stock connection from {address}");
    } else {
      tracing::debug!("new modern connection from {address}");
    }

    let synthesize_change_mode = data.mode == ClientMode::Stock;
    let send_capabilities_snapshot = data.mode == ClientMode::Modern;
    self.connections.insert(address, data);

    if synthesize_change_mode {
      let msg = RecvMsg {
        id: uuid::Uuid::now_v7(),
        from: address,
        data: RecvMsgData::ChangeMode(ClientMode::Stock),
        stock_msg_id: None,
      };
      if let Err(err) = self.tx.send(msg).await {
        tracing::error!("failed to fire synthetic ChangeMode for {address}: {:?}", err);
      }
    }

    if send_capabilities_snapshot {
      if let Err(err) = state.capabilities.send_snapshot_to(address).await {
        tracing::warn!(
          ?err,
          "failed to seed capabilities snapshot for new modern client {address}"
        );
      }
      state.peers.seed_to(address).await;
      if let Err(err) = state.audio.send_current_to(address).await {
        tracing::warn!(?err, "failed to seed volume for new modern client {address}");
      }
    }

    Ok(())
  }

  pub fn handle_disconnect(&self, address: SocketAddr) {
    if let Some((_addr, data)) = self.connections.remove(&address) {
      data.cancel_token.cancel();
      tracing::debug!("removed connection handle for {address}");
    }
  }

  pub async fn _handle_shutdown(self) {
    self.cancel_token.cancel();

    JoinSet::from_iter(self.connections.into_iter().map(|(_, c)| c._handle))
      .join_all()
      .await;
  }
}
