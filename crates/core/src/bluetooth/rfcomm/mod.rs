use std::collections::HashMap;

#[cfg(target_os = "linux")]
use bluer::{
  Session,
  rfcomm::{self, Profile, ProfileHandle},
};
use bridgething_sdk_runtime::{LaneFeed, OutboundLanes, lanes};
use futures::StreamExt;
#[cfg(target_os = "linux")]
use libbridgething::{BRIDGETHING_PROFILE_UUID, BRIDGETHING_RFCOMM_CHANNEL};
use libbridgething::{
  Device, DeviceType, LinkKind, PeerCompanionStatus, Priority,
  gateway::{BridgeToGatewayMsg, GatewayToBridgeMsg},
  protocol::{BridgeEndec, Compress, DecodedFrame, EnvelopeProbe, encode_frame},
  wire::MsgMeta,
};
use tokio::{
  io::{AsyncRead, AsyncWrite, AsyncWriteExt},
  sync::mpsc,
  task::JoinHandle,
};
use tokio_util::{
  bytes::{Bytes, BytesMut},
  codec::FramedRead,
};

use super::{Address, BluetoothResult, GatewayRecvTx, GatewaySendRx, peer_owners::PeerOwners};
use crate::{
  bluetooth::{GatewayType, InboundGatewayMessage, OutboundGatewayMessage, auto_nack_for_failed_decode},
  peer::PeerTracker,
  state::meta::DeviceMeta,
};

const RFCOMM_BATCH_BYTES: usize = 4 * 1024;
const RFCOMM_LANE_BYTES: usize = 256 * 1024;

#[derive(Debug)]
enum ConnectionMessage {
  Msg(Box<GatewayToBridgeMsg>),
  DecodeFailed(EnvelopeProbe),
  Close,
}

impl From<GatewayToBridgeMsg> for ConnectionMessage {
  fn from(msg: GatewayToBridgeMsg) -> Self {
    Self::Msg(Box::new(msg))
  }
}

type ConnectionTx = mpsc::Sender<(Address, ConnectionMessage)>;
type ConnectionRx = mpsc::Receiver<(Address, ConnectionMessage)>;

pub type InjectConnectionTx = mpsc::Sender<(Address, tokio::io::DuplexStream)>;
pub(crate) type InjectConnectionRx = mpsc::Receiver<(Address, tokio::io::DuplexStream)>;

#[derive(Debug)]
pub enum ConnectionSource {
  #[cfg(target_os = "linux")]
  Bluez(ProfileHandle),
  Injected(InjectConnectionRx),
}

enum Incoming {
  #[cfg(target_os = "linux")]
  Bluez(Address, rfcomm::Stream),
  Injected(Address, tokio::io::DuplexStream),
}

impl ConnectionSource {
  async fn accept(&mut self) -> Option<Incoming> {
    match self {
      #[cfg(target_os = "linux")]
      Self::Bluez(handle) => loop {
        let request = handle.next().await?;
        let address: Address = request.device().into();
        tracing::debug!("rfcomm connect request from: {address}");
        match request.accept() {
          Ok(stream) => {
            tracing::debug!("rfcomm accepted connection from: {address}");
            return Some(Incoming::Bluez(address, stream));
          }
          Err(err) => tracing::warn!("({address}) rfcomm accept failed: {err:?}"),
        }
      },
      Self::Injected(rx) => {
        let (address, stream) = rx.recv().await?;
        Some(Incoming::Injected(address, stream))
      }
    }
  }
}

pub(crate) fn inject_channel() -> (InjectConnectionTx, InjectConnectionRx) {
  mpsc::channel(16)
}

#[derive(Debug)]
struct Connection {
  address: Address,
  lanes: OutboundLanes<Bytes>,
  _writer_handle: JoinHandle<()>,
  _reader_handle: JoinHandle<()>,
}

impl Connection {
  fn new<S>(address: Address, stream: S, tx: ConnectionTx) -> Self
  where
    S: AsyncRead + AsyncWrite + Send + 'static,
  {
    let (read_half, write_half) = tokio::io::split(stream);
    let reader = FramedRead::new(read_half, BridgeEndec::default());
    let _reader_handle = tokio::spawn(reader_task(address, reader, tx));

    let (lanes, feed) = lanes(RFCOMM_LANE_BYTES, RFCOMM_BATCH_BYTES);
    let _writer_handle = tokio::spawn(writer_task(address, write_half, feed));

    Self {
      address,
      lanes,
      _writer_handle,
      _reader_handle,
    }
  }

  async fn send(&self, msg: &BridgeToGatewayMsg, priority: Priority, compress: Compress) -> BluetoothResult<()> {
    tracing::trace!(target: "bridgething::rfcomm::frame", "({}) sending rfcomm message ({:?}): {:?}", self.address, priority, msg);
    let mut buf = BytesMut::new();
    encode_frame(priority, compress, msg, &mut buf)?;
    if !self.lanes.send(priority, buf.freeze()).await {
      tracing::debug!("({}) rfcomm writer lane closed; dropping frame", self.address);
    }
    Ok(())
  }
}

async fn reader_task<R>(address: Address, mut reader: FramedRead<R, BridgeEndec>, tx: ConnectionTx)
where
  R: AsyncRead + Unpin + Send + 'static,
{
  while let Some(frame) = reader.next().await {
    match frame {
      Ok(DecodedFrame::Frame(frame)) => {
        if let Err(e) = tx.send((address, frame.msg.into())).await {
          tracing::error!("({address}) failed to forward gateway message: {:?}", e);
        }
      }
      Ok(DecodedFrame::Failed(e)) => {
        if let libbridgething::protocol::EndecError::TypedDecode { error, probe } = e {
          tracing::warn!(
            target: "bridgething::rfcomm::decode",
            "({address}) typed decode failed: surface={:?} event={:?} kind={:?} id={:?}: {error}",
            probe.data_type, probe.data_event, probe.meta_kind, probe.id,
          );
          if tx
            .send((address, ConnectionMessage::DecodeFailed(*probe)))
            .await
            .is_err()
          {
            tracing::debug!("({address}) rfcomm dispatcher gone; dropping decode-failed");
          }
        }
      }
      Err(e) => {
        tracing::debug!("({address}) error decoding rfcomm frame: {:?}", e);
        break;
      }
    }
  }

  tracing::info!("({address}) bluetooth connection closed");
  if let Err(e) = tx.send((address, ConnectionMessage::Close)).await {
    tracing::error!("({address}) failed to send close message: {:?}", e);
  }
}

async fn writer_task<W>(address: Address, mut writer: W, mut feed: LaneFeed<Bytes>)
where
  W: AsyncWrite + Unpin + Send + 'static,
{
  while let Some(batch) = feed.next_batch().await {
    if let Err(err) = writer.write_all(&batch.into_bytes()).await {
      tracing::debug!("({address}) rfcomm write error: {:?}", err);
      break;
    }
    if let Err(err) = writer.flush().await {
      tracing::debug!("({address}) rfcomm flush error: {:?}", err);
      break;
    }
  }
  tracing::debug!("({address}) rfcomm writer task exiting");
}

#[derive(Debug)]
pub struct RfcommGateway {
  meta: DeviceMeta,
  peers: PeerTracker,
  source: ConnectionSource,

  conn_tx: ConnectionTx,
  conn_rx: ConnectionRx,
  connections: HashMap<Address, Connection>,

  recv_tx: GatewayRecvTx,
  send_rx: GatewaySendRx,
  peer_owners: PeerOwners,
}

impl RfcommGateway {
  pub fn init(
    source: ConnectionSource,
    meta: DeviceMeta,
    peers: PeerTracker,
    recv_tx: GatewayRecvTx,
    send_rx: GatewaySendRx,
    peer_owners: PeerOwners,
  ) -> Self {
    let (conn_tx, conn_rx) = mpsc::channel(16);

    Self {
      meta,
      peers,
      source,

      conn_tx,
      conn_rx,
      connections: HashMap::new(),

      recv_tx,
      send_rx,
      peer_owners,
    }
  }

  pub fn spawn(mut self) -> JoinHandle<()> {
    tokio::spawn(async move { self.recv().await })
  }

  async fn recv(&mut self) {
    tracing::info!("rfcomm gateway listening for connections");

    loop {
      tokio::select! {
        incoming = self.source.accept() => match incoming {
          #[cfg(target_os = "linux")]
          Some(Incoming::Bluez(address, stream)) => {
            if let Err(err) = self.add_connection(address, stream).await {
              tracing::error!("({address}) failed to add rfcomm connection: {:?}", err);
            }
          }
          Some(Incoming::Injected(address, stream)) => {
            if let Err(err) = self.add_connection(address, stream).await {
              tracing::error!("({address}) failed to add injected connection: {:?}", err);
            }
          }
          None => {
            match self.source {
              ConnectionSource::Injected(_) => tracing::debug!("rfcomm injected connection source ended"),
              #[cfg(target_os = "linux")]
              ConnectionSource::Bluez(_) => tracing::error!("rfcomm connection source ended"),
            }
            return;
          }
        },
        Some(data) = self.send_rx.recv() => {
          tracing::trace!(target: "bridgething::rfcomm::frame", "rfcomm gateway received message: {:?}", data);
          let OutboundGatewayMessage { address, priority, compress, msg } = data;
          if let Some(address) = address {
            if let Some(conn) = self.connections.get(&address) {
              if let Err(e) = conn.send(&msg, priority, compress).await {
                tracing::error!("failed to send rfcomm frame: {:?}", e);
              }
            } else {
              tracing::trace!("rfcomm: no connection for {address}; addressed send dropped");
            }
          } else {
            for conn in self.connections.values() {
              if let Err(e) = conn.send(&msg, priority, compress).await {
                tracing::error!("failed to send rfcomm frame: {:?}", e);
              }
            }
          }
        },
        Some((address, msg)) = self.conn_rx.recv() => {
          tracing::trace!(target: "bridgething::rfcomm::frame", "rfcomm message from {}: {:?}", address, msg);
          match msg {
            ConnectionMessage::Close => {
              tracing::debug!("rfcomm connection closed: {:?}", address);
              self.connections.remove(&address);
              self.peer_owners.unregister(address, GatewayType::Rfcomm);
              let _ = self.peers.set_companion(address, PeerCompanionStatus::None).await;
            },
            ConnectionMessage::Msg(msg) => {
              if let Err(e) = self.recv_tx.send(InboundGatewayMessage::new(Some(address), GatewayType::Rfcomm, *msg)).await {
                tracing::error!("failed to send rfcomm message to gateway: {:?}", e);
              }
            }
            ConnectionMessage::DecodeFailed(probe) => {
              if let Some(nack) = auto_nack_for_failed_decode(&probe)
                && let Some(conn) = self.connections.get(&address)
                  && let Err(e) = conn.send(&nack, Priority::Normal, Compress::Auto).await {
                    tracing::error!("({address}) failed to send auto-nack: {:?}", e);
                  }
            }
          }
        },
      }
    }
  }

  async fn add_connection<S>(&mut self, address: Address, stream: S) -> BluetoothResult<()>
  where
    S: AsyncRead + AsyncWrite + Send + 'static,
  {
    let connection = Connection::new(address, stream, self.conn_tx.clone());
    let version = BridgeToGatewayMsg {
      id: uuid::Uuid::now_v7(),
      meta: MsgMeta::Event,
      data: self.meta.snapshot().into(),
    };
    connection.send(&version, Priority::Normal, Compress::Auto).await?;

    self.connections.insert(address, connection);
    self.peer_owners.register(address, GatewayType::Rfcomm);
    let placeholder = Device {
      name: address.to_string(),
      device_type: DeviceType::Unknown,
      id: address.to_string(),
      kind: LinkKind::Bluetooth,
      default: false,
    };
    self.peers.ensure_exists(address, placeholder).await;
    let _ = self.peers.set_companion(address, PeerCompanionStatus::Pending).await;

    Ok(())
  }
}

#[cfg(target_os = "linux")]
pub async fn bluez_source(session: &Session) -> BluetoothResult<ConnectionSource> {
  tracing::debug!("creating rfcomm gateway profile");
  let profile = Profile {
    uuid: BRIDGETHING_PROFILE_UUID,
    name: Some("bridgething".to_string()),
    role: Some(rfcomm::Role::Server),
    channel: Some(BRIDGETHING_RFCOMM_CHANNEL as u16),
    require_authentication: Some(false),
    require_authorization: Some(false),
    service_record: Some(bridgething_service_record()),
    ..Default::default()
  };

  let handle = session.register_profile(profile).await?;
  Ok(ConnectionSource::Bluez(handle))
}

#[cfg(target_os = "linux")]
fn bridgething_service_record() -> String {
  format!(
    r#"<?xml version="1.0" encoding="UTF-8" ?>
<record>
    <attribute id="0x0001"><sequence><uuid value="{uuid}" /></sequence></attribute>
    <attribute id="0x0004"><sequence>
        <sequence><uuid value="0x0100" /></sequence>
        <sequence><uuid value="0x0003" /><uint8 value="0x{channel:02x}" /></sequence>
    </sequence></attribute>
    <attribute id="0x0005"><sequence><uuid value="0x1002" /></sequence></attribute>
    <attribute id="0x0006"><sequence>
        <uint16 value="0x656e" />
        <uint16 value="0x006a" />
        <uint16 value="0x0100" />
    </sequence></attribute>
    <attribute id="0x0008"><uint8 value="0xff" /></attribute>
    <attribute id="0x0009"><sequence>
        <sequence><uuid value="0x1101" /><uint16 value="0x0100" /></sequence>
    </sequence></attribute>
    <attribute id="0x0100"><text value="bridgething" /></attribute>
</record>
"#,
    uuid = BRIDGETHING_PROFILE_UUID,
    channel = BRIDGETHING_RFCOMM_CHANNEL,
  )
}
