use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::Instant,
};

use bridgething_iap2::session::EaStreamSender;
use libbridgething::{
  PeerCompanionStatus, Priority,
  gateway::BridgeToGatewayMsg,
  protocol::{BridgeEndec, Compress, DecodedFrame, encode_frame},
  wire::MsgMeta,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::{
  bytes::{Bytes, BytesMut},
  codec::Decoder,
};

use super::super::BluetoothResult;
use crate::{
  bluetooth::{
    Address, BluetoothEvent, GatewayType, InboundGatewayMessage, OutboundGatewayMessage, auto_nack_for_failed_decode,
    peer_owners::PeerOwners,
  },
  peer::PeerTracker,
  state::meta::DeviceMeta,
};

const STREAM_INPUT_CAPACITY: usize = 16;

pub struct StreamOpened {
  pub address: Address,
  pub stream_id: u16,
  pub protocol_id: u8,
  pub inbound_rx: mpsc::Receiver<Bytes>,
  pub outbound: EaStreamSender,
}

impl std::fmt::Debug for StreamOpened {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("StreamOpened")
      .field("address", &self.address)
      .field("stream_id", &self.stream_id)
      .field("protocol_id", &self.protocol_id)
      .finish()
  }
}

#[derive(Debug)]
pub struct StreamClosed {
  pub address: Address,
  pub stream_id: u16,
}

#[derive(Clone, Debug, Default)]
pub struct EaActivity {
  inner: Arc<Mutex<HashMap<Address, Instant>>>,
}

impl EaActivity {
  fn stamp(&self, address: Address) {
    self.inner.lock().unwrap().insert(address, Instant::now());
  }

  fn clear(&self, address: Address) {
    self.inner.lock().unwrap().remove(&address);
  }

  pub fn last_activity(&self, address: Address) -> Option<Instant> {
    self.inner.lock().unwrap().get(&address).copied()
  }
}

#[derive(Clone, Debug)]
pub struct Iap2EaGatewayHandle {
  open_tx: mpsc::Sender<StreamOpened>,
  closed_tx: mpsc::Sender<StreamClosed>,
  activity: EaActivity,
}

impl Iap2EaGatewayHandle {
  pub async fn notify_open(&self, opened: StreamOpened) {
    if let Err(err) = self.open_tx.send(opened).await {
      tracing::warn!(?err, "iap2 ea gateway: open notification dropped");
    }
  }

  pub async fn notify_closed(&self, closed: StreamClosed) {
    if let Err(err) = self.closed_tx.send(closed).await {
      tracing::warn!(?err, "iap2 ea gateway: close notification dropped");
    }
  }

  pub fn activity(&self) -> EaActivity {
    self.activity.clone()
  }
}

type Key = (Address, u16);

#[derive(Debug)]
struct StreamConn {
  outbound: EaStreamSender,
  _reader_handle: JoinHandle<()>,
}

#[derive(Debug)]
pub struct Iap2EaGateway {
  meta: DeviceMeta,
  peers: PeerTracker,
  bluetooth_tx: mpsc::Sender<BluetoothEvent>,
  send_tx: mpsc::Sender<OutboundGatewayMessage>,
  send_rx: mpsc::Receiver<OutboundGatewayMessage>,
  open_rx: mpsc::Receiver<StreamOpened>,
  closed_rx: mpsc::Receiver<StreamClosed>,
  conn_close_tx: mpsc::Sender<Key>,
  conn_close_rx: mpsc::Receiver<Key>,
  conns: HashMap<Key, StreamConn>,
  peer_owners: PeerOwners,
  activity: EaActivity,
}

impl Iap2EaGateway {
  pub fn init(
    meta: DeviceMeta,
    peers: PeerTracker,
    bluetooth_tx: mpsc::Sender<BluetoothEvent>,
    peer_owners: PeerOwners,
  ) -> (Self, Iap2EaGatewayHandle) {
    let (send_tx, send_rx) = mpsc::channel(STREAM_INPUT_CAPACITY);
    let (open_tx, open_rx) = mpsc::channel(STREAM_INPUT_CAPACITY);
    let (closed_tx, closed_rx) = mpsc::channel(STREAM_INPUT_CAPACITY);
    let (conn_close_tx, conn_close_rx) = mpsc::channel(STREAM_INPUT_CAPACITY);
    let activity = EaActivity::default();
    let handle = Iap2EaGatewayHandle {
      open_tx,
      closed_tx,
      activity: activity.clone(),
    };
    let gateway = Self {
      meta,
      peers,
      bluetooth_tx,
      send_tx,
      send_rx,
      open_rx,
      closed_rx,
      conn_close_tx,
      conn_close_rx,
      conns: HashMap::new(),
      peer_owners,
      activity,
    };
    (gateway, handle)
  }

  pub fn send_tx(&self) -> mpsc::Sender<OutboundGatewayMessage> {
    self.send_tx.clone()
  }

  pub fn spawn(mut self) -> JoinHandle<()> {
    tokio::spawn(async move { self.run().await })
  }

  async fn run(&mut self) {
    tracing::info!("iap2 ea gateway: running");
    loop {
      tokio::select! {
        Some(opened) = self.open_rx.recv() => {
          if let Err(err) = self.handle_open(opened).await {
            tracing::warn!(?err, "iap2 ea gateway: failed to open stream");
          }
        }
        Some(closed) = self.closed_rx.recv() => {
          self.tear_down((closed.address, closed.stream_id)).await;
        }
        Some(key) = self.conn_close_rx.recv() => {
          self.tear_down(key).await;
        }
        Some(msg) = self.send_rx.recv() => {
          self.dispatch_outbound(msg).await;
        }
        else => {
          tracing::error!("iap2 ea gateway: all input channels closed");
          return;
        }
      }
    }
  }

  async fn handle_open(&mut self, opened: StreamOpened) -> BluetoothResult<()> {
    let StreamOpened {
      address,
      stream_id,
      protocol_id,
      inbound_rx,
      outbound,
    } = opened;
    let key = (address, stream_id);
    tracing::info!(%address, stream_id, protocol_id, "iap2 ea gateway: stream opened");

    let _reader_handle = tokio::spawn(reader_task(
      address,
      inbound_rx,
      self.bluetooth_tx.clone(),
      self.conn_close_tx.clone(),
      key,
      outbound.clone(),
      self.activity.clone(),
    ));
    self.conns.insert(
      key,
      StreamConn {
        outbound,
        _reader_handle,
      },
    );

    let version = BridgeToGatewayMsg {
      id: uuid::Uuid::now_v7(),
      meta: MsgMeta::Event,
      data: self.meta.snapshot().into(),
    };
    self
      .send_to_stream(key, &version, Priority::Normal, Compress::Auto)
      .await;

    self.peer_owners.register(address, GatewayType::Iap2Ea);
    let _ = self.peers.set_companion(address, PeerCompanionStatus::Pending).await;
    Ok(())
  }

  async fn dispatch_outbound(&mut self, message: OutboundGatewayMessage) {
    let OutboundGatewayMessage {
      address,
      priority,
      compress,
      msg,
    } = message;
    if let Some(address) = address {
      let keys: Vec<Key> = self.conns.keys().copied().filter(|(a, _)| *a == address).collect();
      if keys.is_empty() {
        tracing::trace!(%address, "iap2 ea gateway: no stream for {address}; addressed send dropped");
        return;
      }
      for key in keys {
        self.send_to_stream(key, &msg, priority, compress).await;
      }
    } else {
      let keys: Vec<Key> = self.conns.keys().copied().collect();
      for key in keys {
        self.send_to_stream(key, &msg, priority, compress).await;
      }
    }
  }

  async fn send_to_stream(&mut self, key: Key, msg: &BridgeToGatewayMsg, priority: Priority, compress: Compress) {
    let Some(conn) = self.conns.get(&key) else { return };
    let mut buf = BytesMut::new();
    if let Err(err) = encode_frame(priority, compress, msg, &mut buf) {
      tracing::error!(stream_id = key.1, ?err, "iap2 ea gateway: encode failed");
      return;
    }
    if let Err(err) = conn.outbound.send(priority, buf.freeze()).await {
      tracing::warn!(stream_id = key.1, ?err, "iap2 ea gateway: chunker channel closed");
      self.tear_down(key).await;
      return;
    }
    self.activity.stamp(key.0);
  }

  async fn tear_down(&mut self, key: Key) {
    if self.conns.remove(&key).is_none() {
      return;
    }
    tracing::info!(address = %key.0, stream_id = key.1, "iap2 ea gateway: stream torn down");
    let still_open_for_address = self.conns.keys().any(|(a, _)| *a == key.0);
    if !still_open_for_address {
      self.activity.clear(key.0);
      self.peer_owners.unregister(key.0, GatewayType::Iap2Ea);
      let _ = self.peers.set_companion(key.0, PeerCompanionStatus::None).await;
    }
  }
}

async fn reader_task(
  address: Address,
  mut inbound_rx: mpsc::Receiver<Bytes>,
  bluetooth_tx: mpsc::Sender<BluetoothEvent>,
  conn_close_tx: mpsc::Sender<Key>,
  key: Key,
  outbound: EaStreamSender,
  activity: EaActivity,
) {
  let mut buf = BytesMut::new();
  let mut codec = BridgeEndec::default();
  loop {
    loop {
      match codec.decode(&mut buf) {
        Ok(Some(DecodedFrame::Frame(frame))) => {
          let event = BluetoothEvent::Gateway(InboundGatewayMessage::new(
            Some(address),
            GatewayType::Iap2Ea,
            frame.msg,
          ));
          if bluetooth_tx.send(event).await.is_err() {
            tracing::error!(%address, "iap2 ea gateway: bluetooth bus closed");
            let _ = conn_close_tx.send(key).await;
            return;
          }
        }
        Ok(None) => break,
        Ok(Some(DecodedFrame::Failed(err))) => {
          if let libbridgething::protocol::EndecError::TypedDecode { error, probe } = err {
            tracing::warn!(
              target: "bridgething::iap2_ea::decode",
              %address, stream_id = key.1,
              "typed decode failed: surface={:?} event={:?} kind={:?} id={:?}: {error}",
              probe.data_type, probe.data_event, probe.meta_kind, probe.id,
            );
            if let Some(nack) = auto_nack_for_failed_decode(&probe) {
              let mut nack_buf = BytesMut::new();
              if let Err(e) = encode_frame(Priority::Normal, Compress::Auto, &nack, &mut nack_buf) {
                tracing::error!(%address, ?e, "iap2 ea gateway: encode auto-nack failed");
              } else if let Err(e) = outbound.send(Priority::Normal, nack_buf.freeze()).await {
                tracing::warn!(%address, ?e, "iap2 ea gateway: auto-nack send failed");
              }
            }
          }
        }
        Err(err) => {
          tracing::debug!(%address, ?err, "iap2 ea gateway: decode error; tearing down stream");
          let _ = conn_close_tx.send(key).await;
          return;
        }
      }
    }

    match inbound_rx.recv().await {
      Some(chunk) => {
        activity.stamp(address);
        buf.extend_from_slice(&chunk);
      }
      None => {
        tracing::debug!(%address, "iap2 ea gateway: inbound channel closed");
        let _ = conn_close_tx.send(key).await;
        return;
      }
    }
  }
}
