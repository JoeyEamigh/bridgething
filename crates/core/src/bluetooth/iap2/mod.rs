#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, IntoRawFd, OwnedFd};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::{collections::HashSet, sync::Arc};

#[cfg(target_os = "linux")]
use bluer::{
  Adapter, Session,
  rfcomm::{ConnectRequest, Profile, ProfileHandle, Role, Stream},
};
#[cfg(target_os = "linux")]
use bridgething_iap2::{
  AppLaunch, IAP2_ACCESSORY_UUID, IAP2_DEVICE_UUID, IAP2_RFCOMM_CHANNEL, Iap2Event as Iap2InternalEvent, Iap2Session,
  Link, LinkConfig, Lsp,
  csm::identification::{CarthingIdentification, EaProtocol, EaProtocolMatchAction, IdentificationConfig},
  session::WorkerMfiAccess,
};
use bridgething_iap2::{HidCommand, NowPlayingCommand, SessionEvent, session::TelephonyCommand};
#[cfg(target_os = "linux")]
use bridgething_iap2::{Iap2Command, NowPlayingAuthorityState};
#[cfg(target_os = "linux")]
use bridgething_mfi::MfiAuth;
pub use ea::{EaActivity, Iap2EaGateway, Iap2EaGatewayHandle, StreamClosed, StreamOpened};
#[cfg(target_os = "linux")]
use futures::StreamExt;
#[cfg(target_os = "linux")]
use tokio::sync::RwLock;
use tokio::sync::{mpsc, oneshot};
#[cfg(target_os = "linux")]
use tokio::{sync::watch, task::JoinHandle};

mod ea;

use super::Address;
#[cfg(target_os = "linux")]
use super::BluetoothResult;
#[cfg(target_os = "linux")]
use crate::{
  authority::AuthorityRegistry,
  state::meta::{DeviceMeta, SuperbirdMeta},
};

#[cfg(target_os = "linux")]
const IAP2_PROFILE_NAME: &str = "iAP2";
#[cfg(target_os = "linux")]
const IAP2_CLIENT_PROFILE_NAME: &str = "iAP2 (device dial-in)";
const IAP2_CHANNEL_CAPACITY: usize = 16;
const IAP2_EVENTS_CAPACITY: usize = 64;
const IAP2_OUTBOUND_TAP_CAPACITY: usize = 256;
#[cfg(target_os = "linux")]
const COMPANION_BUNDLE_ID: &str = "com.bridgething.gateway";
pub(crate) const SPOTIFY_BUNDLE_ID: &str = "com.spotify.client";
#[cfg(target_os = "linux")]
const COMPANION_EA_PROTOCOL_ID: u8 = 1;
#[cfg(target_os = "linux")]
const LINK_TEARDOWN_GRACE: Duration = Duration::from_millis(750);
#[cfg(target_os = "linux")]
const EA_CLOSE_GRACE: Duration = Duration::from_millis(500);
#[cfg(target_os = "linux")]
const INHERITED_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const IAP2_SHUTDOWN_BUDGET: Duration = Duration::from_secs(3);
#[cfg(target_os = "linux")]
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(60);
#[cfg(target_os = "linux")]
const RECONNECT_DIAL_SETTLE: Duration = Duration::from_secs(8);
const RECONNECT_KICK_CAPACITY: usize = 16;
const SESSION_DEAD_CAPACITY: usize = 16;

#[cfg(target_os = "linux")]
fn iap2_service_record() -> String {
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
    <attribute id="0x0100"><text value="Wireless iAP" /></attribute>
</record>
"#,
    uuid = IAP2_ACCESSORY_UUID,
    channel = IAP2_RFCOMM_CHANNEL,
  )
}

#[derive(Debug)]
pub struct Iap2Event {
  pub address: Address,
  pub event: SessionEvent,
}

pub type Iap2EventsRx = mpsc::Receiver<Iap2Event>;

pub type Iap2InjectTx = mpsc::Sender<Iap2Event>;

pub type Iap2OutboundTapTx = tokio::sync::broadcast::Sender<Iap2TransportCommand>;

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionOrigin {
  Dialed,
  Inherited,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct ActiveSession {
  generation: u64,
  hid_tx: mpsc::Sender<HidCommand>,
  np_tx: mpsc::Sender<NowPlayingCommand>,
  tel_tx: mpsc::Sender<TelephonyCommand>,
  app_launch_tx: mpsc::Sender<String>,
  session_shutdown_tx: mpsc::Sender<oneshot::Sender<()>>,
  rfcomm_fd: Option<OwnedFd>,
  link_command_tx: mpsc::Sender<Iap2Command>,
  link_handle: JoinHandle<bridgething_iap2::Result<()>>,
  session_handle: JoinHandle<bridgething_iap2::Result<()>>,
  shovel_handle: JoinHandle<()>,
}

#[cfg(target_os = "linux")]
impl ActiveSession {
  fn abort(&self) {
    self.link_handle.abort();
    self.session_handle.abort();
    self.shovel_handle.abort();
  }

  async fn close_ea_sessions(&self) {
    let (ack_tx, ack_rx) = oneshot::channel();
    if self.session_shutdown_tx.send(ack_tx).await.is_err() {
      return;
    }
    if tokio::time::timeout(EA_CLOSE_GRACE, ack_rx).await.is_err() {
      tracing::warn!("iAP2 external accessory close did not confirm within its budget");
    }
  }

  async fn shutdown(self) {
    self.close_ea_sessions().await;
    if self.link_command_tx.send(Iap2Command::Disconnect).await.is_ok() {
      let _ = tokio::time::timeout(LINK_TEARDOWN_GRACE, self.link_handle).await;
    } else {
      self.link_handle.abort();
    }
    self.session_handle.abort();
    self.shovel_handle.abort();

    if let Some(fd) = self.rfcomm_fd
      && crate::systemd::socket::stash_iap2_fd(fd.as_fd())
    {
      tracing::info!("handed the iAP2 RFCOMM socket to the service manager to outlive this process");
    }
  }
}

#[derive(Debug, Clone, Copy)]
pub enum Iap2TransportCommand {
  Hid(HidCommand),
  NowPlaying(NowPlayingCommand),
  RequestAppLaunch(&'static str),
}

#[derive(Debug, Clone)]
pub struct Iap2ReconnectHandle {
  tx: mpsc::Sender<Address>,
}

impl Iap2ReconnectHandle {
  pub async fn kick(&self, mac: Address) {
    if self.tx.send(mac).await.is_err() {
      tracing::debug!(%mac, "iap2 reconnect kick dropped; manager exited");
    }
  }
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("the iAP2 link is down")]
pub struct Iap2LinkDown;

#[derive(Debug, Clone)]
pub struct Iap2TransportHandle {
  tx: mpsc::Sender<Iap2TransportCommand>,
}

impl Iap2TransportHandle {
  pub async fn send_hid(&self, cmd: HidCommand) {
    if self.tx.send(Iap2TransportCommand::Hid(cmd)).await.is_err() {
      tracing::error!(?cmd, "iap2 link down; the transport press reached nothing");
    }
  }

  pub async fn send_now_playing(&self, cmd: NowPlayingCommand) {
    if self.tx.send(Iap2TransportCommand::NowPlaying(cmd)).await.is_err() {
      tracing::error!(?cmd, "iap2 link down; the now-playing command reached nothing");
    }
  }

  pub async fn wake_spotify(&self) {
    if self
      .tx
      .send(Iap2TransportCommand::RequestAppLaunch(SPOTIFY_BUNDLE_ID))
      .await
      .is_err()
    {
      tracing::error!("iap2 link down; the wake-spotify request reached nothing");
    }
  }
}

#[derive(Debug, Clone)]
pub struct Iap2TelephonyHandle {
  tx: mpsc::Sender<TelephonyCommand>,
}

impl Iap2TelephonyHandle {
  pub async fn send(&self, cmd: TelephonyCommand) -> Result<(), Iap2LinkDown> {
    self.tx.send(cmd).await.map_err(|_| Iap2LinkDown)
  }

  #[cfg(test)]
  pub(crate) fn for_test() -> (Self, mpsc::Receiver<TelephonyCommand>) {
    let (tx, rx) = mpsc::channel(IAP2_CHANNEL_CAPACITY);
    (Iap2TelephonyHandle { tx }, rx)
  }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Default)]
pub struct Iap2ActiveSessions {
  inner: Arc<RwLock<HashSet<Address>>>,
}

#[cfg(target_os = "linux")]
impl Iap2ActiveSessions {
  pub async fn insert(&self, mac: Address) {
    self.inner.write().await.insert(mac);
  }

  pub async fn remove(&self, mac: &Address) {
    self.inner.write().await.remove(mac);
  }

  pub async fn contains(&self, mac: &Address) -> bool {
    self.inner.read().await.contains(mac)
  }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct Iap2Manager {
  server_handle: ProfileHandle,
  client_handle: ProfileHandle,
  identification: IdentificationConfig,
  mfi_worker: WorkerMfiAccess,
  adapter: Adapter,
  sessions: HashMap<Address, ActiveSession>,
  active_sessions: Iap2ActiveSessions,
  reconnects: HashMap<Address, JoinHandle<()>>,
  reconnect_rx: mpsc::Receiver<Address>,
  transport_rx: mpsc::Receiver<Iap2TransportCommand>,
  telephony_rx: mpsc::Receiver<TelephonyCommand>,
  events_tx: mpsc::Sender<Iap2Event>,
  now_playing_authority: watch::Receiver<NowPlayingAuthorityState>,
  companion_launch: watch::Receiver<bool>,
  next_generation: u64,
  session_dead_tx: mpsc::Sender<(Address, u64)>,
  session_dead_rx: mpsc::Receiver<(Address, u64)>,
  shutdown_rx: mpsc::Receiver<oneshot::Sender<()>>,
}

#[derive(Debug, Clone)]
pub struct Iap2ShutdownHandle {
  tx: mpsc::Sender<oneshot::Sender<()>>,
}

impl Iap2ShutdownHandle {
  pub async fn shutdown(&self) {
    let (ack_tx, ack_rx) = oneshot::channel();
    if self.tx.send(ack_tx).await.is_err() {
      return;
    }
    if tokio::time::timeout(IAP2_SHUTDOWN_BUDGET, ack_rx).await.is_err() {
      tracing::warn!("iAP2 teardown did not finish within its budget; exiting anyway");
    }
  }
}

#[derive(Debug, Clone)]
pub struct Iap2Handles {
  pub reconnect: Iap2ReconnectHandle,
  pub transport: Iap2TransportHandle,
  pub telephony: Iap2TelephonyHandle,
  pub shutdown: Iap2ShutdownHandle,
}

pub(super) struct Iap2Bootstrap {
  #[cfg(target_os = "linux")]
  reconnect_rx: mpsc::Receiver<Address>,
  transport_rx: mpsc::Receiver<Iap2TransportCommand>,
  #[cfg(target_os = "linux")]
  telephony_rx: mpsc::Receiver<TelephonyCommand>,
  events_tx: mpsc::Sender<Iap2Event>,
  #[cfg(target_os = "linux")]
  session_dead_tx: mpsc::Sender<(Address, u64)>,
  #[cfg(target_os = "linux")]
  session_dead_rx: mpsc::Receiver<(Address, u64)>,
  #[cfg(target_os = "linux")]
  shutdown_rx: mpsc::Receiver<oneshot::Sender<()>>,
  outbound_tap_tx: Iap2OutboundTapTx,
}

impl Iap2Bootstrap {
  pub(super) fn events_tx(&self) -> Iap2InjectTx {
    self.events_tx.clone()
  }

  pub(super) fn outbound_tap_tx(&self) -> Iap2OutboundTapTx {
    self.outbound_tap_tx.clone()
  }

  pub(super) fn into_headless_outbound(self) -> (mpsc::Receiver<Iap2TransportCommand>, Iap2OutboundTapTx) {
    (self.transport_rx, self.outbound_tap_tx)
  }
}

pub(super) fn allocate_iap2() -> (Iap2Handles, Iap2EventsRx, Iap2Bootstrap) {
  let (reconnect_tx, _reconnect_rx) = mpsc::channel(RECONNECT_KICK_CAPACITY);
  let (transport_tx, transport_rx) = mpsc::channel::<Iap2TransportCommand>(IAP2_CHANNEL_CAPACITY);
  let (telephony_tx, _telephony_rx) = mpsc::channel::<TelephonyCommand>(IAP2_CHANNEL_CAPACITY);
  let (events_tx, events_rx) = mpsc::channel::<Iap2Event>(IAP2_EVENTS_CAPACITY);
  let (_session_dead_tx, _session_dead_rx) = mpsc::channel::<(Address, u64)>(SESSION_DEAD_CAPACITY);
  let (shutdown_tx, _shutdown_rx) = mpsc::channel::<oneshot::Sender<()>>(1);
  let (outbound_tap_tx, _) = tokio::sync::broadcast::channel::<Iap2TransportCommand>(IAP2_OUTBOUND_TAP_CAPACITY);

  let handles = Iap2Handles {
    reconnect: Iap2ReconnectHandle { tx: reconnect_tx },
    transport: Iap2TransportHandle { tx: transport_tx },
    telephony: Iap2TelephonyHandle { tx: telephony_tx },
    shutdown: Iap2ShutdownHandle { tx: shutdown_tx },
  };
  let bootstrap = Iap2Bootstrap {
    #[cfg(target_os = "linux")]
    reconnect_rx: _reconnect_rx,
    transport_rx,
    #[cfg(target_os = "linux")]
    telephony_rx: _telephony_rx,
    events_tx,
    #[cfg(target_os = "linux")]
    session_dead_tx: _session_dead_tx,
    #[cfg(target_os = "linux")]
    session_dead_rx: _session_dead_rx,
    #[cfg(target_os = "linux")]
    shutdown_rx: _shutdown_rx,
    outbound_tap_tx,
  };
  (handles, events_rx, bootstrap)
}

#[cfg(target_os = "linux")]
impl Iap2Manager {
  pub(super) async fn start(
    bootstrap: Iap2Bootstrap,
    session: &Session,
    adapter: Adapter,
    meta: &DeviceMeta,
    authority: &AuthorityRegistry,
  ) -> BluetoothResult<Option<JoinHandle<()>>> {
    let mfi_worker = match probe_and_spawn_worker().await {
      Ok(w) => w,
      Err(reason) => {
        tracing::warn!(%reason, "MFi probe failed; iAP2 disabled");
        drop(bootstrap);
        return Ok(None);
      }
    };

    let server_handle = super::retry_bluez("iap2 server profile registration", || {
      session.register_profile(Profile {
        uuid: IAP2_ACCESSORY_UUID,
        name: Some(IAP2_PROFILE_NAME.to_string()),
        role: Some(Role::Server),
        channel: Some(IAP2_RFCOMM_CHANNEL as u16),
        require_authentication: Some(false),
        require_authorization: Some(false),
        auto_connect: Some(true),
        service_record: Some(iap2_service_record()),
        ..Default::default()
      })
    })
    .await;
    tracing::info!(channel = IAP2_RFCOMM_CHANNEL, "registered iAP2 RFCOMM server profile");

    let client_handle = super::retry_bluez("iap2 client profile registration", || {
      session.register_profile(Profile {
        uuid: IAP2_DEVICE_UUID,
        name: Some(IAP2_CLIENT_PROFILE_NAME.to_string()),
        role: Some(Role::Client),
        require_authentication: Some(false),
        require_authorization: Some(false),
        auto_connect: Some(true),
        ..Default::default()
      })
    })
    .await;
    tracing::info!("registered iAP2 RFCOMM client profile (accessory-initiated reconnect)");

    let identification = build_identification(meta.static_meta());
    let Iap2Bootstrap {
      reconnect_rx,
      transport_rx,
      telephony_rx,
      events_tx,
      session_dead_rx,
      session_dead_tx,
      shutdown_rx,
      outbound_tap_tx: _,
    } = bootstrap;
    let active_sessions = Iap2ActiveSessions::default();

    let manager = Self {
      server_handle,
      client_handle,
      identification,
      mfi_worker,
      adapter,
      sessions: HashMap::new(),
      active_sessions,
      reconnects: HashMap::new(),
      reconnect_rx,
      transport_rx,
      telephony_rx,
      events_tx,
      now_playing_authority: authority.now_playing_subscription_rx(),
      companion_launch: meta.subscribe_companion_launch(),
      next_generation: 0,
      session_dead_tx,
      session_dead_rx,
      shutdown_rx,
    };

    Ok(Some(manager.spawn()))
  }

  fn spawn(mut self) -> JoinHandle<()> {
    tokio::spawn(async move { self.recv().await })
  }

  async fn recv(&mut self) {
    tracing::info!("iAP2 manager listening for iPhone connections");
    self.restore_inherited().await;
    self.kickoff_reconnects_for_paired_ios().await;

    loop {
      tokio::select! {
        Some(request) = self.server_handle.next() => {
          if let Err(err) = self.accept(request, ConnectDirection::Inbound).await {
            tracing::error!(?err, "iAP2 server accept failed");
          }
        }
        Some(request) = self.client_handle.next() => {
          if let Err(err) = self.accept(request, ConnectDirection::Outbound).await {
            tracing::error!(?err, "iAP2 client accept failed");
          }
        }
        Some(mac) = self.reconnect_rx.recv() => {
          self.spawn_reconnect(mac).await;
        }
        Some((mac, generation)) = self.session_dead_rx.recv() => {
          self.handle_session_dead(mac, generation).await;
        }
        Some(cmd) = self.transport_rx.recv() => {
          self.dispatch_transport(cmd).await;
        }
        Some(cmd) = self.telephony_rx.recv() => {
          self.dispatch_telephony(cmd).await;
        }
        Some(ack) = self.shutdown_rx.recv() => {
          self.shutdown_all().await;
          let _ = ack.send(());
          return;
        }
        else => {
          tracing::error!("iAP2 manager streams all ended - this should not happen");
          return;
        }
      }
    }
  }

  async fn shutdown_all(&mut self) {
    for (mac, task) in self.reconnects.drain() {
      task.abort();
      tracing::debug!(%mac, "cancelled iAP2 reconnect loop for shutdown");
    }

    let sessions = std::mem::take(&mut self.sessions);
    if sessions.is_empty() {
      return;
    }

    tracing::info!(count = sessions.len(), "tearing down iAP2 links for shutdown");
    for (mac, session) in sessions {
      self.active_sessions.remove(&mac).await;
      session.shutdown().await;
      tracing::debug!(%mac, "iAP2 link torn down");
    }
  }

  async fn accept(&mut self, request: ConnectRequest, direction: ConnectDirection) -> BluetoothResult<()> {
    let address = request.device();
    tracing::info!(%address, ?direction, "iAP2 connect request");

    let stream = request.accept()?;
    self.start_session(address.into(), stream, SessionOrigin::Dialed).await
  }

  async fn restore_inherited(&mut self) {
    let inherited = crate::systemd::socket::claim_inherited_iap2();
    if inherited.is_empty() {
      return;
    }
    crate::systemd::socket::clear_iap2_fds();

    for fd in inherited {
      let stream = match unsafe { Stream::from_raw_fd(fd.into_raw_fd()) } {
        Ok(stream) => stream,
        Err(err) => {
          tracing::warn!(?err, "inherited iAP2 RFCOMM socket could not be adopted");
          continue;
        }
      };
      let address: Address = match stream.peer_addr() {
        Ok(addr) => addr.addr.into(),
        Err(err) => {
          tracing::warn!(?err, "inherited iAP2 RFCOMM socket has no peer; dropping it");
          continue;
        }
      };
      tracing::info!(%address, "resuming iAP2 on the RFCOMM socket carried across the restart");
      if let Err(err) = self.start_session(address, stream, SessionOrigin::Inherited).await {
        tracing::warn!(%address, ?err, "could not resume the inherited iAP2 socket");
      }
    }
  }

  async fn start_session(&mut self, address: Address, stream: Stream, origin: SessionOrigin) -> BluetoothResult<()> {
    if let Some(stale) = self.sessions.remove(&address) {
      tracing::debug!(%address, "aborting stale iAP2 session stack before replacing it");
      stale.abort();
    }
    self.active_sessions.remove(&address).await;
    self.cancel_reconnect(&address);

    let generation = self.next_generation;
    self.next_generation = self.next_generation.wrapping_add(1);

    let (link_command_tx, link_command_rx) = mpsc::channel::<Iap2Command>(IAP2_CHANNEL_CAPACITY);
    let (link_events_tx, link_events_rx) = mpsc::channel::<Iap2InternalEvent>(IAP2_CHANNEL_CAPACITY);
    let (session_events_tx, session_events_rx) = mpsc::channel::<SessionEvent>(IAP2_CHANNEL_CAPACITY);
    let (hid_tx, hid_rx) = mpsc::channel::<HidCommand>(IAP2_CHANNEL_CAPACITY);
    let (np_tx, np_rx) = mpsc::channel::<NowPlayingCommand>(IAP2_CHANNEL_CAPACITY);
    let (tel_tx, tel_rx) = mpsc::channel::<TelephonyCommand>(IAP2_CHANNEL_CAPACITY);
    let (app_launch_tx, app_launch_rx) = mpsc::channel::<String>(IAP2_CHANNEL_CAPACITY);
    let (session_shutdown_tx, session_shutdown_rx) = mpsc::channel::<oneshot::Sender<()>>(1);

    let rfcomm_fd = match unsafe { BorrowedFd::borrow_raw(stream.as_raw_fd()) }.try_clone_to_owned() {
      Ok(fd) => Some(fd),
      Err(err) => {
        tracing::warn!(%address, ?err, "could not duplicate the iAP2 RFCOMM socket; restarts will drop it");
        None
      }
    };

    let mut link_config = LinkConfig::new(Lsp::accessory_default());
    if matches!(origin, SessionOrigin::Inherited) {
      link_config.handshake_timeout = INHERITED_HANDSHAKE_TIMEOUT;
    }
    let link_handle = tokio::spawn(Link::run(stream, link_config, link_events_tx, link_command_rx));

    let mfi = self.mfi_worker.handle();
    let session = Iap2Session::with_app_launch(
      self.identification.clone(),
      Some(AppLaunch {
        bundle_id: COMPANION_BUNDLE_ID.to_string(),
        relaunch: self.companion_launch.clone(),
      }),
      app_launch_rx,
      session_shutdown_rx,
      mfi,
      link_command_tx.clone(),
      link_events_rx,
      session_events_tx,
      hid_rx,
      np_rx,
      self.now_playing_authority.clone(),
      tel_rx,
    );
    let session_handle = tokio::spawn(session.run());

    let shovel_handle = tokio::spawn(shovel_session_events(
      address,
      generation,
      session_events_rx,
      self.events_tx.clone(),
      self.session_dead_tx.clone(),
    ));

    self.active_sessions.insert(address).await;
    self.sessions.insert(
      address,
      ActiveSession {
        generation,
        hid_tx,
        np_tx,
        tel_tx,
        app_launch_tx,
        session_shutdown_tx,
        rfcomm_fd,
        link_command_tx,
        link_handle,
        session_handle,
        shovel_handle,
      },
    );

    Ok(())
  }

  async fn dispatch_transport(&self, cmd: Iap2TransportCommand) {
    let Some(session) = self.sessions.values().next() else {
      tracing::trace!(?cmd, "iAP2 transport command with no active session; dropping");
      return;
    };
    match cmd {
      Iap2TransportCommand::Hid(hid) => {
        if session.hid_tx.send(hid).await.is_err() {
          tracing::debug!(?hid, "iAP2 session HID receiver closed; dropping command");
        }
      }
      Iap2TransportCommand::NowPlaying(np) => {
        if session.np_tx.send(np).await.is_err() {
          tracing::debug!(?np, "iAP2 session NowPlaying receiver closed; dropping command");
        }
      }
      Iap2TransportCommand::RequestAppLaunch(bundle) => {
        if session.app_launch_tx.send(bundle.to_string()).await.is_err() {
          tracing::debug!(bundle, "iAP2 session app-launch receiver closed; dropping command");
        }
      }
    }
  }

  async fn dispatch_telephony(&self, cmd: TelephonyCommand) {
    let Some(session) = self.sessions.values().next() else {
      tracing::trace!(?cmd, "iAP2 telephony command with no active session; dropping");
      return;
    };
    if session.tel_tx.send(cmd).await.is_err() {
      tracing::debug!("iAP2 session telephony receiver closed; dropping command");
    }
  }

  async fn kickoff_reconnects_for_paired_ios(&mut self) {
    let addresses = match self.adapter.device_addresses().await {
      Ok(a) => a,
      Err(err) => {
        tracing::warn!(?err, "failed to enumerate paired devices for iAP2 reconnect");
        return;
      }
    };
    for mac in addresses {
      self.spawn_reconnect(mac.into()).await;
    }
  }

  async fn peer_advertises_iap2(&self, mac: Address) -> bool {
    let Ok(device) = self.adapter.device(mac.into()) else {
      return false;
    };
    if !device.is_paired().await.unwrap_or(false) {
      return false;
    }
    device
      .uuids()
      .await
      .ok()
      .flatten()
      .is_some_and(|set| set.contains(&IAP2_DEVICE_UUID))
  }

  async fn spawn_reconnect(&mut self, mac: Address) {
    if !self.peer_advertises_iap2(mac).await {
      tracing::trace!(%mac, "iAP2 reconnect skipped: peer does not advertise iAP2");
      return;
    }
    if self.sessions.contains_key(&mac) {
      tracing::trace!(%mac, "iAP2 session already active; skipping reconnect kick");
      return;
    }
    if let Some(handle) = self.reconnects.get(&mac)
      && !handle.is_finished()
    {
      tracing::trace!(%mac, "iAP2 reconnect already in flight; skipping kick");
      return;
    }
    let adapter = self.adapter.clone();
    let active = self.active_sessions.clone();
    let task = tokio::spawn(reconnect_loop(adapter, active, mac));
    self.reconnects.insert(mac, task);
  }

  fn cancel_reconnect(&mut self, mac: &Address) {
    if let Some(handle) = self.reconnects.remove(mac) {
      handle.abort();
    }
  }

  async fn handle_session_dead(&mut self, mac: Address, generation: u64) {
    match self.sessions.get(&mac) {
      Some(active) if active.generation == generation => {}
      Some(_) => {
        tracing::trace!(%mac, generation, "ignoring stale iAP2 session-dead signal");
        return;
      }
      None => {
        tracing::trace!(%mac, generation, "ignoring iAP2 session-dead signal: no active session");
        return;
      }
    }

    tracing::info!(%mac, "iAP2 session ended; cleaning up and re-arming reconnect");
    self.sessions.remove(&mac);
    self.active_sessions.remove(&mac).await;
    self.spawn_reconnect(mac).await;
  }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
enum ConnectDirection {
  Inbound,
  Outbound,
}

#[cfg(target_os = "linux")]
async fn reconnect_loop(adapter: Adapter, active_sessions: Iap2ActiveSessions, mac: Address) {
  let device = match adapter.device(mac.into()) {
    Ok(d) => d,
    Err(err) => {
      tracing::debug!(%mac, ?err, "iAP2 reconnect aborted: no device handle");
      return;
    }
  };
  let mut delay = RECONNECT_INITIAL_DELAY;
  loop {
    tokio::time::sleep(delay).await;

    if !still_should_reconnect(&adapter, &active_sessions, mac).await {
      tracing::debug!(%mac, "iAP2 reconnect loop exiting (peer unbonded or already up)");
      return;
    }

    tracing::debug!(%mac, ?delay, "dialing iPhone iAP2-device channel");
    match device.connect_profile(&IAP2_DEVICE_UUID).await {
      Ok(()) => {
        tracing::info!(%mac, "iAP2 connect_profile dial succeeded; awaiting NewConnection");
        tokio::time::sleep(RECONNECT_DIAL_SETTLE).await;
        if !still_should_reconnect(&adapter, &active_sessions, mac).await {
          return;
        }
        tracing::warn!(%mac, "no iAP2 session after successful dial; disconnecting profile and retrying");
        if let Err(err) = device.disconnect_profile(&IAP2_DEVICE_UUID).await {
          tracing::debug!(%mac, ?err, "iAP2 disconnect_profile failed");
        }
        delay = (delay * 2).min(RECONNECT_MAX_DELAY);
      }
      Err(err) => {
        tracing::debug!(%mac, ?err, "iAP2 connect_profile dial failed; backing off");
        delay = (delay * 2).min(RECONNECT_MAX_DELAY);
      }
    }
  }
}

#[cfg(target_os = "linux")]
async fn still_should_reconnect(adapter: &Adapter, active_sessions: &Iap2ActiveSessions, mac: Address) -> bool {
  let Ok(device) = adapter.device(mac.into()) else {
    return false;
  };
  if !device.is_paired().await.unwrap_or(false) {
    return false;
  }
  if device.is_connected().await.unwrap_or(false) && active_sessions.contains(&mac).await {
    return false;
  }
  true
}

#[cfg(target_os = "linux")]
async fn shovel_session_events(
  address: Address,
  generation: u64,
  mut rx: mpsc::Receiver<SessionEvent>,
  tx: mpsc::Sender<Iap2Event>,
  dead_tx: mpsc::Sender<(Address, u64)>,
) {
  while let Some(event) = rx.recv().await {
    if tx.send(Iap2Event { address, event }).await.is_err() {
      tracing::debug!(%address, "iap2 events channel closed; dropping shovel");
      break;
    }
  }
  if dead_tx.send((address, generation)).await.is_err() {
    tracing::trace!(%address, generation, "iap2 session-dead channel closed; manager exited");
  }
}

#[cfg(target_os = "linux")]
fn build_identification(meta: &SuperbirdMeta) -> IdentificationConfig {
  let bt_mac = parse_bt_mac(&meta.bt_mac);
  let mut config = IdentificationConfig::for_carthing(CarthingIdentification {
    serial_number: meta.serial_number.clone(),
    firmware_version: format!("v{}", env!("CARGO_PKG_VERSION")),
    bt_mac,
  });
  config.supported_external_accessory_protocols = vec![EaProtocol {
    id: COMPANION_EA_PROTOCOL_ID,
    name: COMPANION_BUNDLE_ID.to_string(),
    match_action: EaProtocolMatchAction::NoAlertAction,
    native_transport_component_identifier: None,
  }];
  config
}

#[cfg(target_os = "linux")]
fn parse_bt_mac(s: &str) -> [u8; 6] {
  let parts: Vec<&str> = s.split(':').collect();
  if parts.len() != 6 {
    tracing::warn!(meta_bt_mac = %s, "unexpected bt_mac format; iAP2 transport component MAC will be all zeros");
    return [0; 6];
  }
  let mut out = [0u8; 6];
  for (i, part) in parts.iter().enumerate() {
    match u8::from_str_radix(part, 16) {
      Ok(b) => out[i] = b,
      Err(_) => {
        tracing::warn!(meta_bt_mac = %s, "non-hex byte in bt_mac; iAP2 transport component MAC will be all zeros");
        return [0; 6];
      }
    }
  }
  out
}

#[cfg(all(target_os = "linux", debug_assertions))]
async fn probe_and_spawn_worker() -> Result<WorkerMfiAccess, String> {
  use bridgething_mfi::RemoteI2c;

  let host = std::env::var("SUPERBIRD_HOST").map_err(|_| "SUPERBIRD_HOST env not set".to_string())?;
  let addr = format!("{host}:9090");

  let mfi = tokio::task::spawn_blocking(move || -> Result<MfiAuth<RemoteI2c>, String> {
    let transport = RemoteI2c::connect(addr.as_str()).map_err(|e| format!("RemoteI2c::connect({addr}): {e:?}"))?;
    let mut auth = MfiAuth::with_transport(transport);
    auth.cert().map_err(|e| format!("cert probe: {e:?}"))?;
    Ok(auth)
  })
  .await
  .map_err(|e| format!("MFi probe task panicked: {e:?}"))??;

  tracing::info!("MFi probe via RemoteI2c succeeded; spawning iap2-mfi-worker");
  Ok(WorkerMfiAccess::spawn(mfi))
}

#[cfg(all(target_os = "linux", not(debug_assertions)))]
async fn probe_and_spawn_worker() -> Result<WorkerMfiAccess, String> {
  let mfi = tokio::task::spawn_blocking(|| -> Result<MfiAuth<bridgething_mfi::LinuxI2c>, String> {
    let mut auth = MfiAuth::open_default().map_err(|e| format!("MfiAuth::open_default: {e:?}"))?;
    auth.cert().map_err(|e| format!("cert probe: {e:?}"))?;
    Ok(auth)
  })
  .await
  .map_err(|e| format!("MFi probe task panicked: {e:?}"))??;

  tracing::info!("MFi probe via /dev/i2c-3 succeeded; spawning iap2-mfi-worker");
  Ok(WorkerMfiAccess::spawn(mfi))
}
