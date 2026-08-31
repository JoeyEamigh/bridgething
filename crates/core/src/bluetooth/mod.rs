use std::{
  collections::HashMap,
  net::SocketAddr,
  sync::{Arc, Mutex},
  time::Duration,
};

#[cfg(target_os = "linux")]
use iap2::Iap2Manager;
use iap2::{Iap2EaGateway, Iap2EaGatewayHandle, Iap2EventsRx, Iap2Handles};
use libbridgething::{
  Priority,
  gateway::{BridgeToGatewayMsg, BridgeToGatewayMsgData, GatewayToBridgeMsg, GatewayToBridgeMsgData},
  protocol::{Compress, EnvelopeProbe},
  wire::{MsgMeta, RequestError, ResponseMeta, WireCommand, WireError, WireEvent, WireRequest},
};
#[cfg(target_os = "linux")]
use profiles::ProfileManager;
use tokio::{sync::oneshot, task::JoinHandle};
use uuid::Uuid;

mod address;
pub mod iap2;
pub mod le;
mod network;
pub mod profiles;
mod rfcomm;

#[cfg(target_os = "linux")]
mod adapter;
#[cfg(target_os = "linux")]
mod auth;
#[cfg(all(target_os = "linux", debug_assertions))]
mod debug;
#[cfg(target_os = "linux")]
mod hci;
mod peer_owners;
#[cfg(target_os = "linux")]
mod scan;

pub use address::Address;
pub use iap2::{Iap2Event, Iap2InjectTx, Iap2OutboundTapTx, Iap2TransportCommand};
use le::{LeBootstrap, LeManager};
use network::NetworkGateway;
use peer_owners::PeerOwners;
use profiles::{ProfileCommand, ProfileCommandRx, ProfileCommandTx};
pub use rfcomm::InjectConnectionTx;
pub(crate) use rfcomm::inject_channel;
use rfcomm::{ConnectionSource, RfcommGateway};

use crate::{
  handler::Iap2EventRouter,
  net::WSError,
  peer::PeerTracker,
  player::PlayerError,
  state::{State, StateError, meta::DeviceMeta},
};
#[cfg(target_os = "linux")]
use crate::{net::WireEventBus, state::DeviceStore};

pub type BluetoothMan = Arc<BluetoothManager>;
pub type BluetoothTx = tokio::sync::mpsc::Sender<BluetoothEvent>;

const GATEWAY_OUTBOUND_CAPACITY: usize = 64;

#[derive(Debug)]
pub enum BluetoothEvent {
  Gateway(InboundGatewayMessage),
}

#[derive(Debug, Clone)]
pub struct BluetoothDeps {
  #[cfg(target_os = "linux")]
  pub bus: WireEventBus,
  pub meta: DeviceMeta,
  #[cfg(target_os = "linux")]
  pub devices: DeviceStore,
  pub peers: PeerTracker,
}

pub(crate) enum BluetoothBringup {
  #[cfg(target_os = "linux")]
  Real,
  Headless(rfcomm::InjectConnectionRx),
}

#[derive(Debug)]
pub struct BluetoothManager {
  pub gateway_man: GatewayMan,
  pub iap2: Iap2Handles,
  pub le: LeManager,
  pub profile_man: ProfileManAccess,
}

pub(crate) struct BluetoothBootstrap {
  gateway: GatewayBootstrap,
  iap2_events_rx: Iap2EventsRx,
  iap2_bootstrap: iap2::Iap2Bootstrap,
  le: LeBootstrap,
  profile_command_rx: ProfileCommandRx,
}

impl BluetoothBootstrap {
  pub(crate) fn iap2_inject_tx(&self) -> Iap2InjectTx {
    self.iap2_bootstrap.events_tx()
  }

  pub(crate) fn iap2_outbound_tap(&self) -> Iap2OutboundTapTx {
    self.iap2_bootstrap.outbound_tap_tx()
  }
}

impl BluetoothManager {
  pub(crate) fn create(serial_suffix: [char; 4]) -> (BluetoothMan, BluetoothBootstrap) {
    let (gateway_man, gateway_bootstrap) = GatewayMan::allocate();
    let (iap2_handles, iap2_events_rx, iap2_bootstrap) = iap2::allocate_iap2();
    let (le_handle, le_bootstrap) = LeManager::allocate(serial_suffix);
    let (profile_command_tx, profile_command_rx) = tokio::sync::mpsc::channel(profiles::PROFILE_COMMAND_CAPACITY);

    let manager = Arc::new(Self {
      gateway_man,
      iap2: iap2_handles,
      le: le_handle,
      profile_man: ProfileManAccess { tx: profile_command_tx },
    });

    let bootstrap = BluetoothBootstrap {
      gateway: gateway_bootstrap,
      iap2_events_rx,
      iap2_bootstrap,
      le: le_bootstrap,
      profile_command_rx,
    };

    (manager, bootstrap)
  }

  #[cfg(test)]
  pub(crate) fn capturing() -> (BluetoothMan, GatewaySendRx) {
    let (manager, bootstrap) = Self::create(['0'; 4]);
    (manager, bootstrap.gateway.outbound_rx)
  }

  pub(crate) fn spawn(
    self: &BluetoothMan,
    bootstrap: BluetoothBootstrap,
    deps: BluetoothDeps,
    state: State,
    bluetooth_tx: BluetoothTx,
    network_bind: SocketAddr,
    bringup: BluetoothBringup,
  ) -> JoinHandle<()> {
    let manager = self.clone();
    tokio::spawn(async move {
      if let Err(err) = manager
        .run(bootstrap, deps, state, bluetooth_tx, network_bind, bringup)
        .await
      {
        tracing::error!(?err, "FATAL: bluetooth coordinator failed");
      }
    })
  }

  async fn run(
    self: BluetoothMan,
    bootstrap: BluetoothBootstrap,
    deps: BluetoothDeps,
    state: State,
    bluetooth_tx: BluetoothTx,
    network_bind: SocketAddr,
    bringup: BluetoothBringup,
  ) -> BluetoothResult<()> {
    match bringup {
      BluetoothBringup::Headless(inject_rx) => {
        let BluetoothBootstrap {
          gateway,
          mut iap2_events_rx,
          iap2_bootstrap,
          profile_command_rx,
          le: le_bootstrap,
          ..
        } = bootstrap;
        tracing::debug!("bringing up gateway transports + iap2 router with no radio (headless)");

        let _profile_actor = profiles::spawn_no_radio_actor(profile_command_rx);
        let _le_actor = le_bootstrap.spawn_no_radio();

        let (mut outbound_rx, outbound_tap_tx) = iap2_bootstrap.into_headless_outbound();
        tokio::spawn(async move {
          while let Some(cmd) = outbound_rx.recv().await {
            let _ = outbound_tap_tx.send(cmd);
          }
        });
        let gateway_runtime = self
          .gateway_man
          .start(
            gateway,
            ConnectionSource::Injected(inject_rx),
            network_bind,
            &deps,
            bluetooth_tx,
          )
          .await?;

        let pending_art = state.iap2_pending_art.clone();
        let router = Arc::new(Iap2EventRouter::new(
          state,
          self.clone(),
          gateway_runtime.iap2_ea_handle.clone(),
          self.iap2.reconnect.clone(),
          pending_art,
        ));

        loop {
          match iap2_events_rx.recv().await {
            Some(event) => router.route(event).await,
            None => {
              tracing::warn!("headless coordinator: iap2 event stream ended; parking");
              std::future::pending::<()>().await;
            }
          }
        }
      }
      #[cfg(target_os = "linux")]
      BluetoothBringup::Real => {
        let BluetoothBootstrap {
          gateway,
          mut iap2_events_rx,
          iap2_bootstrap,
          le: le_bootstrap,
          profile_command_rx,
        } = bootstrap;

        tracing::debug!("initializing bluetooth manager");
        let (session, adapter) = retry_bluez("bluetooth bring-up", Self::bring_up_adapter).await;

        tracing::info!("initialized bluetooth adapter {}", adapter.name());

        if let Err(err) = scan::apply_fast_inquiry_scan(&adapter) {
          tracing::warn!(?err, "failed to apply fast inquiry scan params");
        }

        #[cfg(debug_assertions)]
        if let Err(err) = debug::query_adapter(&adapter).await {
          tracing::warn!(?err, "adapter debug query failed");
        }

        tracing::debug!("setting up bluetooth profile manager");
        let profile_man = Arc::new(ProfileManager::init(
          adapter.clone(),
          deps.bus.clone(),
          deps.devices.clone(),
          deps.peers.clone(),
          self.iap2.reconnect.clone(),
        ));
        let _profile_actor = profiles::spawn_command_actor(profile_man.clone(), profile_command_rx);

        let _agent_handle = retry_bluez("agent registration", || {
          auth::build_agent(&session, profile_man.clone())
        })
        .await;

        let _adapter_event_handle = adapter::AdapterEventStream {
          stream: Box::new(retry_bluez("adapter event stream", || adapter.events()).await),
          adapter: adapter.clone(),
        }
        .spawn(profile_man.clone());

        tracing::debug!("setting up bluetooth gateway transports");
        let source = retry_bluez("rfcomm profile registration", || rfcomm::bluez_source(&session)).await;
        let gateway_runtime = self
          .gateway_man
          .start(gateway, source, network_bind, &deps, bluetooth_tx)
          .await?;

        tracing::debug!("setting up iap2 manager");
        let _iap2_handle = Iap2Manager::start(
          iap2_bootstrap,
          &session,
          adapter.clone(),
          deps.meta.static_meta(),
          &state.authority,
        )
        .await?;

        tracing::debug!("setting up le dispatcher");
        let _le_handle = le_bootstrap
          .start(adapter.clone(), deps.bus.clone(), self.clone(), state.audio.clone())
          .await;

        let pending_art = state.iap2_pending_art.clone();
        let router = Arc::new(Iap2EventRouter::new(
          state,
          self.clone(),
          gateway_runtime.iap2_ea_handle.clone(),
          self.iap2.reconnect.clone(),
          pending_art,
        ));

        loop {
          match iap2_events_rx.recv().await {
            Some(event) => router.route(event).await,
            None => {
              tracing::warn!("bluetooth coordinator: iap2 event stream ended; coordinator parking");
              std::future::pending::<()>().await;
            }
          }
        }
      }
    }
  }

  pub async fn connect(&self, mac: &str) -> BluetoothResult<()> {
    let address: Address = mac.parse()?;
    tracing::debug!(%address, "kicking iAP2 reconnect from connect command");
    self.iap2.reconnect.kick(address).await;
    Ok(())
  }

  #[cfg(target_os = "linux")]
  async fn bring_up_adapter() -> BluetoothResult<(bluer::Session, bluer::Adapter)> {
    let session = bluer::Session::new().await?;
    let adapter = adapter::get_adapter(&session).await?;

    tracing::debug!("attempting to power on adapter");
    adapter.set_powered(true).await?;

    tracing::debug!("configuring adapter");
    adapter.set_pairable_timeout(0).await?;
    adapter.set_pairable(true).await?;
    adapter.set_discoverable_timeout(0).await?;
    adapter.set_discoverable(true).await?;
    Ok((session, adapter))
  }
}

#[derive(Debug, Clone)]
pub struct ProfileManAccess {
  tx: ProfileCommandTx,
}

impl ProfileManAccess {
  pub async fn set_alias(&self, alias: String) -> BluetoothResult<()> {
    self.request(|reply| ProfileCommand::SetAlias { alias, reply }).await
  }

  pub async fn set_discoverable(&self, discoverable: bool) -> BluetoothResult<()> {
    self
      .request(|reply| ProfileCommand::SetDiscoverable { discoverable, reply })
      .await
  }

  pub async fn forget(&self, mac: &str) -> BluetoothResult<()> {
    let mac = mac.to_string();
    self.request(|reply| ProfileCommand::Forget { mac, reply }).await
  }

  pub async fn reset(&self) -> BluetoothResult<()> {
    self.request(|reply| ProfileCommand::Reset { reply }).await
  }

  pub async fn upsert_paired_device(
    &self,
    mac: Address,
    device_type: libbridgething::DeviceType,
  ) -> BluetoothResult<libbridgething::Device> {
    self
      .request(|reply| ProfileCommand::UpsertPairedDevice {
        mac,
        device_type,
        reply,
      })
      .await
  }

  async fn request<T>(&self, command: impl FnOnce(profiles::Reply<T>) -> ProfileCommand) -> BluetoothResult<T> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .tx
      .send(command(reply_tx))
      .await
      .map_err(|_| BluetoothError::NoRadio)?;
    reply_rx.await.map_err(|_| BluetoothError::NoRadio)?
  }
}

#[cfg(target_os = "linux")]
pub(crate) async fn retry_bluez<T, E, F, Fut>(what: &str, mut op: F) -> T
where
  E: std::fmt::Debug,
  F: FnMut() -> Fut,
  Fut: std::future::Future<Output = Result<T, E>>,
{
  let mut delay = Duration::from_millis(250);
  loop {
    match op().await {
      Ok(value) => return value,
      Err(err) => {
        tracing::warn!(?err, "{what} failed; retrying in {}ms", delay.as_millis());
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
      }
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GatewayType {
  Rfcomm,
  Iap2Ea,
  Network,
}

#[derive(Debug, Clone)]
pub struct InboundGatewayMessage {
  pub address: Option<Address>,
  pub protocol: GatewayType,
  pub msg: GatewayToBridgeMsg,
}

impl InboundGatewayMessage {
  pub fn new(address: Option<Address>, protocol: GatewayType, msg: GatewayToBridgeMsg) -> Self {
    Self { address, protocol, msg }
  }
}

#[derive(Debug, Clone)]
pub struct OutboundGatewayMessage {
  pub address: Option<Address>,
  pub priority: Priority,
  pub compress: Compress,
  pub msg: Arc<BridgeToGatewayMsg>,
}

impl OutboundGatewayMessage {
  pub fn new(address: Option<Address>, msg: BridgeToGatewayMsg) -> Self {
    Self {
      address,
      priority: Priority::Normal,
      compress: Compress::Auto,
      msg: Arc::new(msg),
    }
  }

  pub fn to(address: Address, msg: BridgeToGatewayMsg) -> Self {
    Self::new(Some(address), msg)
  }

  pub fn all(msg: BridgeToGatewayMsg) -> Self {
    Self::new(None, msg)
  }

  pub fn with_priority(mut self, priority: Priority) -> Self {
    self.priority = priority;
    self
  }

  pub fn bulk(self) -> Self {
    self.with_priority(Priority::Bulk)
  }

  pub fn with_compress(mut self, compress: Compress) -> Self {
    self.compress = compress;
    self
  }
}

pub type GatewayRecvTx = tokio::sync::mpsc::Sender<InboundGatewayMessage>;
pub type GatewayRecvRx = tokio::sync::mpsc::Receiver<InboundGatewayMessage>;
pub type GatewaySendTx = tokio::sync::mpsc::Sender<OutboundGatewayMessage>;
pub type GatewaySendRx = tokio::sync::mpsc::Receiver<OutboundGatewayMessage>;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Pending {
  target: Option<Address>,
  tx: oneshot::Sender<GatewayToBridgeMsgData>,
}

type PendingRequests = Arc<Mutex<HashMap<Uuid, Pending>>>;

fn fail_pending_on_disconnect(pending: &PendingRequests, addr: Address, no_peers_left: bool) {
  pending.lock().expect("pending-request map poisoned").retain(|_id, p| {
    let drop_it = match p.target {
      Some(target) => target == addr,
      None => no_peers_left,
    };
    !drop_it
  });
}

#[derive(Debug, Clone)]
pub struct GatewayMan {
  outbound_tx: GatewaySendTx,
  peer_owners: PeerOwners,
  pending: PendingRequests,
}

pub(crate) struct GatewayBootstrap {
  outbound_rx: GatewaySendRx,
}

#[derive(Debug)]
struct GatewayRuntime {
  iap2_ea_handle: Iap2EaGatewayHandle,
  _rfcomm_handle: JoinHandle<()>,
  _rfcomm_listener: JoinHandle<()>,
  _iap2_ea_handle: JoinHandle<()>,
  _network_handle: JoinHandle<()>,
  _router_handle: JoinHandle<()>,
}

impl GatewayMan {
  fn allocate() -> (Self, GatewayBootstrap) {
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(GATEWAY_OUTBOUND_CAPACITY);
    let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
    let peer_owners = PeerOwners::new();
    peer_owners.set_disconnect_hook({
      let pending = pending.clone();
      Arc::new(move |addr, no_peers_left| fail_pending_on_disconnect(&pending, addr, no_peers_left))
    });
    let me = Self {
      outbound_tx,
      peer_owners,
      pending,
    };
    (me, GatewayBootstrap { outbound_rx })
  }

  #[cfg(test)]
  pub(crate) fn capturing() -> (Self, GatewaySendRx) {
    let (me, bootstrap) = Self::allocate();
    (me, bootstrap.outbound_rx)
  }

  async fn start(
    &self,
    bootstrap: GatewayBootstrap,
    source: ConnectionSource,
    network_bind: SocketAddr,
    deps: &BluetoothDeps,
    bluetooth_tx: BluetoothTx,
  ) -> BluetoothResult<GatewayRuntime> {
    tracing::debug!("initializing bluetooth gateway manager");

    let (rfcomm_recv_tx, rfcomm_recv_rx) = tokio::sync::mpsc::channel(16);
    let (rfcomm_send_tx, rfcomm_send_rx) = tokio::sync::mpsc::channel(16);

    let _rfcomm_handle = RfcommGateway::init(
      source,
      deps.meta.clone(),
      deps.peers.clone(),
      rfcomm_recv_tx,
      rfcomm_send_rx,
      self.peer_owners.clone(),
    )
    .spawn();

    let _rfcomm_listener = spawn_gateway_listener(GatewayType::Rfcomm, rfcomm_recv_rx, bluetooth_tx.clone());

    let (iap2_ea, iap2_ea_handle) = Iap2EaGateway::init(
      deps.meta.clone(),
      deps.peers.clone(),
      bluetooth_tx.clone(),
      self.peer_owners.clone(),
    );
    let iap2_ea_send_tx = iap2_ea.send_tx();
    let _iap2_ea_handle_join = iap2_ea.spawn();

    let network = NetworkGateway::init(
      network_bind,
      deps.meta.clone(),
      deps.peers.clone(),
      bluetooth_tx.clone(),
      self.peer_owners.clone(),
    )
    .await?;
    let network_send_tx = network.send_tx();
    let _network_handle = network.spawn();

    let _router_handle = spawn_outbound_router(
      bootstrap.outbound_rx,
      self.peer_owners.clone(),
      rfcomm_send_tx,
      iap2_ea_send_tx,
      network_send_tx,
    );

    Ok(GatewayRuntime {
      iap2_ea_handle,
      _rfcomm_handle,
      _rfcomm_listener,
      _iap2_ea_handle: _iap2_ea_handle_join,
      _network_handle,
      _router_handle,
    })
  }

  pub async fn send_all(&self, data: OutboundGatewayMessage) {
    tracing::trace!("queueing outbound gateway message: {:?}", &data);
    if let Err(err) = self.outbound_tx.send(data).await {
      tracing::error!(?err, "gateway outbound queue closed; drop");
    }
  }

  pub async fn broadcast<E: WireEvent<BridgeToGatewayMsgData>>(&self, event: E) {
    self.broadcast_event_with_priority(event, Priority::Normal).await;
  }

  pub async fn broadcast_event_bulk<E: WireEvent<BridgeToGatewayMsgData>>(&self, event: E) {
    self.broadcast_event_with_priority(event, Priority::Bulk).await;
  }

  pub async fn broadcast_event_background<E: WireEvent<BridgeToGatewayMsgData>>(&self, event: E) {
    self.broadcast_event_with_priority(event, Priority::Background).await;
  }

  async fn broadcast_event_with_priority<E: WireEvent<BridgeToGatewayMsgData>>(&self, event: E, priority: Priority) {
    self
      .send_all(
        OutboundGatewayMessage::all(BridgeToGatewayMsg {
          id: uuid::Uuid::now_v7(),
          meta: MsgMeta::Event,
          data: event.into(),
        })
        .with_priority(priority),
      )
      .await;
  }

  pub async fn send_event<E: WireEvent<BridgeToGatewayMsgData>>(&self, address: Address, event: E) {
    self
      .send_all(OutboundGatewayMessage::to(
        address,
        BridgeToGatewayMsg {
          id: uuid::Uuid::now_v7(),
          meta: MsgMeta::Event,
          data: event.into(),
        },
      ))
      .await;
  }

  pub async fn send_event_bulk<E: WireEvent<BridgeToGatewayMsgData>>(
    &self,
    address: Address,
    event: E,
    compress: Compress,
  ) {
    self
      .send_all(
        OutboundGatewayMessage::to(
          address,
          BridgeToGatewayMsg {
            id: uuid::Uuid::now_v7(),
            meta: MsgMeta::Event,
            data: event.into(),
          },
        )
        .bulk()
        .with_compress(compress),
      )
      .await;
  }

  pub async fn broadcast_command<C: WireCommand<BridgeToGatewayMsgData>>(&self, cmd: C) {
    self.command_with_priority(None, cmd, Priority::Normal).await;
  }

  pub async fn broadcast_command_bulk<C: WireCommand<BridgeToGatewayMsgData>>(&self, cmd: C) {
    self.command_with_priority(None, cmd, Priority::Bulk).await;
  }

  pub async fn command_bulk<C: WireCommand<BridgeToGatewayMsgData>>(&self, address: Option<Address>, cmd: C) {
    self.command_with_priority(address, cmd, Priority::Bulk).await;
  }

  pub async fn send_command<C: WireCommand<BridgeToGatewayMsgData>>(&self, address: Address, cmd: C) {
    self.command_with_priority(Some(address), cmd, Priority::Normal).await;
  }

  async fn command_with_priority<C: WireCommand<BridgeToGatewayMsgData>>(
    &self,
    address: Option<Address>,
    cmd: C,
    priority: Priority,
  ) {
    let msg = BridgeToGatewayMsg {
      id: uuid::Uuid::now_v7(),
      meta: MsgMeta::Command,
      data: cmd.into(),
    };
    let outbound = match address {
      Some(addr) => OutboundGatewayMessage::to(addr, msg),
      None => OutboundGatewayMessage::all(msg),
    };
    self.send_all(outbound.with_priority(priority)).await;
  }

  pub async fn request<R: WireRequest<Outbound = BridgeToGatewayMsgData, Inbound = GatewayToBridgeMsgData>>(
    &self,
    address: Option<Address>,
    req: R,
  ) -> Result<R::Response, RequestError<R::DomainError>> {
    self.request_with_priority(address, req, Priority::Normal).await
  }

  async fn request_with_priority<
    R: WireRequest<Outbound = BridgeToGatewayMsgData, Inbound = GatewayToBridgeMsgData>,
  >(
    &self,
    address: Option<Address>,
    req: R,
    priority: Priority,
  ) -> Result<R::Response, RequestError<R::DomainError>> {
    self
      .request_with_id_priority(Uuid::now_v7(), address, req, priority, REQUEST_TIMEOUT)
      .await
  }

  pub async fn request_with_timeout<
    R: WireRequest<Outbound = BridgeToGatewayMsgData, Inbound = GatewayToBridgeMsgData>,
  >(
    &self,
    address: Option<Address>,
    req: R,
    timeout: Duration,
  ) -> Result<R::Response, RequestError<R::DomainError>> {
    self
      .request_with_id_priority(Uuid::now_v7(), address, req, Priority::Normal, timeout)
      .await
  }

  pub async fn request_with_id<R: WireRequest<Outbound = BridgeToGatewayMsgData, Inbound = GatewayToBridgeMsgData>>(
    &self,
    id: Uuid,
    address: Option<Address>,
    req: R,
  ) -> Result<R::Response, RequestError<R::DomainError>> {
    self
      .request_with_id_priority(id, address, req, Priority::Normal, REQUEST_TIMEOUT)
      .await
  }

  async fn request_with_id_priority<
    R: WireRequest<Outbound = BridgeToGatewayMsgData, Inbound = GatewayToBridgeMsgData>,
  >(
    &self,
    id: Uuid,
    address: Option<Address>,
    req: R,
    priority: Priority,
    timeout: Duration,
  ) -> Result<R::Response, RequestError<R::DomainError>> {
    let (tx, rx) = oneshot::channel();
    self
      .pending
      .lock()
      .expect("pending-request map poisoned")
      .insert(id, Pending { target: address, tx });

    let msg = BridgeToGatewayMsg {
      id,
      meta: MsgMeta::Request,
      data: req.into(),
    };
    self
      .send_all(OutboundGatewayMessage::new(address, msg).with_priority(priority))
      .await;

    match tokio::time::timeout(timeout, rx).await {
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

  pub fn complete_pending(&self, request_id: &Uuid, data: GatewayToBridgeMsgData) -> bool {
    let pending = self
      .pending
      .lock()
      .expect("pending-request map poisoned")
      .remove(request_id);
    if let Some(pending) = pending {
      let _ = pending.tx.send(data);
      true
    } else {
      false
    }
  }
}

fn spawn_gateway_listener(gateway_type: GatewayType, mut rx: GatewayRecvRx, tx: BluetoothTx) -> JoinHandle<()> {
  tracing::debug!("spawning gateway listener for {gateway_type:?}");
  tokio::spawn(async move {
    while let Some(msg) = rx.recv().await {
      if let Err(err) = tx.send(BluetoothEvent::Gateway(msg)).await {
        tracing::error!("failed to send message to bluetooth manager: {:?}", err);
      }
    }
    tracing::debug!("gateway listener: all senders dropped; exiting");
  })
}

fn spawn_outbound_router(
  mut outbound_rx: GatewaySendRx,
  peer_owners: PeerOwners,
  rfcomm_send_tx: GatewaySendTx,
  iap2_ea_send_tx: GatewaySendTx,
  network_send_tx: GatewaySendTx,
) -> JoinHandle<()> {
  tokio::spawn(async move {
    while let Some(msg) = outbound_rx.recv().await {
      let targets = resolve_targets(&peer_owners, msg.address);
      match targets.len() {
        0 => tracing::trace!("outbound router: no targets for {:?}; dropping", msg.address),
        1 => dispatch_to(targets[0], msg, &rfcomm_send_tx, &iap2_ea_send_tx, &network_send_tx).await,
        _ => {
          let last = *targets.last().expect("non-empty targets");
          for kind in &targets[..targets.len() - 1] {
            dispatch_to(*kind, msg.clone(), &rfcomm_send_tx, &iap2_ea_send_tx, &network_send_tx).await;
          }
          dispatch_to(last, msg, &rfcomm_send_tx, &iap2_ea_send_tx, &network_send_tx).await;
        }
      }
    }
    tracing::debug!("outbound router: outbound channel closed; exiting");
  })
}

fn resolve_targets(peer_owners: &PeerOwners, address: Option<Address>) -> Vec<GatewayType> {
  match address {
    Some(addr) => match peer_owners.owner(&addr) {
      Some(kind) => vec![kind],
      None => {
        tracing::trace!(%addr, "outbound router: no transport owns address; dropping");
        Vec::new()
      }
    },
    None => {
      let active = peer_owners.active_kinds();
      let mut targets = Vec::with_capacity(3);
      for kind in [GatewayType::Rfcomm, GatewayType::Iap2Ea, GatewayType::Network] {
        if active.contains(&kind) {
          targets.push(kind);
        }
      }
      targets
    }
  }
}

async fn dispatch_to(
  kind: GatewayType,
  msg: OutboundGatewayMessage,
  rfcomm_send_tx: &GatewaySendTx,
  iap2_ea_send_tx: &GatewaySendTx,
  network_send_tx: &GatewaySendTx,
) {
  let tx = match kind {
    GatewayType::Rfcomm => rfcomm_send_tx,
    GatewayType::Iap2Ea => iap2_ea_send_tx,
    GatewayType::Network => network_send_tx,
  };
  if let Err(err) = tx.send(msg).await {
    tracing::error!(?err, ?kind, "outbound router: transport queue closed");
  }
}

pub fn auto_nack_for_failed_decode(probe: &EnvelopeProbe) -> Option<BridgeToGatewayMsg> {
  if !probe.is_request() {
    return None;
  }
  let request_id = probe.id?;
  Some(BridgeToGatewayMsg {
    id: Uuid::now_v7(),
    meta: MsgMeta::Response(ResponseMeta { request_id }),
    data: BridgeToGatewayMsgData::Error(WireError::Unsupported),
  })
}

pub type BluetoothResult<T> = Result<T, BluetoothError>;
#[derive(Debug, thiserror::Error)]
pub enum BluetoothError {
  #[cfg(target_os = "linux")]
  #[error("bluez error: {0}")]
  Bluez(#[from] bluer::Error),
  #[error("no bluetooth radio is attached to this daemon")]
  NoRadio,
  #[error(transparent)]
  InvalidAddress(#[from] address::InvalidAddress),
  #[error(transparent)]
  WS(#[from] WSError),
  #[error("state error: {0}")]
  State(#[from] StateError),
  #[error("connection to bluetooth daemon timed out")]
  Timeout,
  #[error(transparent)]
  Player(#[from] PlayerError),
  #[error(transparent)]
  MessagePackEnc(#[from] rmp_serde::encode::Error),
  #[error(transparent)]
  MessagePackDec(#[from] rmp_serde::decode::Error),
  #[error(transparent)]
  Endec(#[from] libbridgething::protocol::EndecError),
  #[error(transparent)]
  Io(#[from] std::io::Error),
}

crate::impl_broadcast_failure_from!(BluetoothError);

#[cfg(test)]
mod outbound_router_tests {
  use super::*;

  fn addr(last: u8) -> Address {
    Address::new([0xFE, 0xFE, 0x00, 0x00, 0x00, last])
  }

  #[test]
  fn a_broadcast_targets_the_network_transport_when_a_network_peer_is_registered() {
    let owners = PeerOwners::new();
    owners.register(addr(0x01), GatewayType::Network);

    let targets = resolve_targets(&owners, None);

    assert!(
      targets.contains(&GatewayType::Network),
      "a broadcast must reach the network gateway, or a cli pusher never learns its run ended"
    );
  }

  #[test]
  fn a_bluetooth_peer_does_not_displace_the_network_peer_from_a_broadcast() {
    let owners = PeerOwners::new();
    owners.register(addr(0x01), GatewayType::Network);
    owners.register(addr(0x02), GatewayType::Iap2Ea);

    let targets = resolve_targets(&owners, None);

    assert!(targets.contains(&GatewayType::Network), "network still targeted");
    assert!(targets.contains(&GatewayType::Iap2Ea), "phone still targeted");
    assert_eq!(targets.len(), 2, "exactly the two live transports");
  }

  #[test]
  fn a_broadcast_drops_the_network_transport_once_its_only_peer_goes_away() {
    let owners = PeerOwners::new();
    owners.register(addr(0x01), GatewayType::Network);
    owners.unregister(addr(0x01), GatewayType::Network);

    assert!(
      !resolve_targets(&owners, None).contains(&GatewayType::Network),
      "a closed connection must stop being a target"
    );
  }
}
