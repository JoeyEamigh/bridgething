use std::{
  collections::{HashMap, HashSet},
  net::SocketAddr,
  time::Duration,
};

use libbridgething::{
  Device, GatewayInfo, Peer, PeerCompanionStatus, PeerIap2Status,
  client::{
    BluetoothPairingResult, BluetoothStatus, BridgeToClientBluetoothMsg, BridgeToClientBluetoothMsgEvent,
    BridgeToClientPeerMsg, BridgeToClientPeerMsgEvent, ConnectedDevice as WireConnectedDevice, PairedDevicesMap,
    PeerSnapshotMap,
  },
  wire::MsgMeta,
};
use tokio::sync::{mpsc, watch};

use crate::{
  bluetooth::Address,
  capabilities::CapabilitiesRegistry,
  net::{WSError, WireEventBus},
  player::Player,
  state::{AudioManager, LogTap, PlaybackTargetStore, RouteTable, TelephonyManager, TunnelRoutes, log_tap::LogOwner},
  stock::{broadcast_stock_connection, broadcast_stock_disconnection},
};

const PEER_CMD_CAPACITY: usize = 64;
const USEFUL_LINK_DOWN_GRACE: Duration = Duration::from_secs(12);

#[derive(Debug, Default, Clone)]
pub struct PeerSnapshot {
  pub peers: HashMap<Address, Peer>,
}

#[derive(Debug)]
enum PeerCommand {
  Upsert {
    mac: Address,
    device: Device,
  },
  EnsureExists {
    mac: Address,
    device: Device,
  },
  SetPaired {
    mac: Address,
    paired: bool,
  },
  SetIap2 {
    mac: Address,
    iap2: PeerIap2Status,
  },
  SetCompanion {
    mac: Address,
    companion: PeerCompanionStatus,
  },
  SetDisplayName {
    mac: Address,
    name: String,
  },
  SetLanguage {
    mac: Address,
    language: String,
  },
  SetUuid {
    mac: Address,
    uuid: String,
  },
  Remove {
    mac: Address,
  },
  RemoveBluez {
    mac: Address,
  },
  NotePinShown {
    mac: Address,
  },
  ConfirmPairing {
    mac: Address,
  },
  SeedTo {
    addr: SocketAddr,
  },
  ResyncStockConnection,
  FlushPendingDisconnect {
    epoch: u64,
  },
}

#[derive(Debug, Clone)]
pub struct PeerTracker {
  cmd_tx: mpsc::Sender<PeerCommand>,
  snapshot_rx: watch::Receiver<PeerSnapshot>,
}

impl PeerTracker {
  #[allow(clippy::too_many_arguments)]
  pub fn new(
    bus: WireEventBus,
    player: Player,
    audio: AudioManager,
    capabilities: CapabilitiesRegistry,
    playback_targets: PlaybackTargetStore,
    telephony: TelephonyManager,
    ws_routes: RouteTable,
    stream_routes: RouteTable,
    tunnel_routes: TunnelRoutes,
    log_tap: LogTap,
  ) -> Self {
    let (cmd_tx, cmd_rx) = mpsc::channel(PEER_CMD_CAPACITY);
    let (snapshot_tx, snapshot_rx) = watch::channel(PeerSnapshot::default());
    tokio::spawn(run_actor(
      cmd_rx,
      cmd_tx.clone(),
      snapshot_tx,
      bus,
      player,
      audio,
      capabilities,
      playback_targets,
      telephony,
      ws_routes,
      stream_routes,
      tunnel_routes,
      log_tap,
    ));
    Self { cmd_tx, snapshot_rx }
  }

  pub fn snapshot(&self) -> PeerSnapshot {
    self.snapshot_rx.borrow().clone()
  }

  pub fn watch_snapshot(&self) -> watch::Receiver<PeerSnapshot> {
    self.snapshot_rx.clone()
  }

  pub fn connected_companion(&self) -> Option<GatewayInfo> {
    self
      .snapshot_rx
      .borrow()
      .peers
      .values()
      .find_map(|peer| match &peer.companion {
        PeerCompanionStatus::Connected(info) => Some(info.clone()),
        _ => None,
      })
  }

  pub async fn upsert(&self, mac: Address, device: Device) {
    self.send(PeerCommand::Upsert { mac, device }).await;
  }

  pub async fn ensure_exists(&self, mac: Address, device: Device) {
    self.send(PeerCommand::EnsureExists { mac, device }).await;
  }

  pub async fn set_paired(&self, mac: Address, paired: bool) {
    self.send(PeerCommand::SetPaired { mac, paired }).await;
  }

  pub async fn set_iap2(&self, mac: Address, iap2: PeerIap2Status) {
    self.send(PeerCommand::SetIap2 { mac, iap2 }).await;
  }

  pub async fn set_companion(&self, mac: Address, companion: PeerCompanionStatus) {
    self.send(PeerCommand::SetCompanion { mac, companion }).await;
  }

  pub async fn set_display_name(&self, mac: Address, name: String) {
    self.send(PeerCommand::SetDisplayName { mac, name }).await;
  }

  pub async fn set_language(&self, mac: Address, language: String) {
    self.send(PeerCommand::SetLanguage { mac, language }).await;
  }

  pub async fn set_uuid(&self, mac: Address, uuid: String) {
    self.send(PeerCommand::SetUuid { mac, uuid }).await;
  }

  pub async fn remove(&self, mac: Address) {
    self.send(PeerCommand::Remove { mac }).await;
  }

  pub async fn remove_bluez(&self, mac: Address) {
    self.send(PeerCommand::RemoveBluez { mac }).await;
  }

  pub async fn note_pin_shown(&self, mac: Address) {
    self.send(PeerCommand::NotePinShown { mac }).await;
  }

  pub async fn confirm_pairing(&self, mac: Address) {
    self.send(PeerCommand::ConfirmPairing { mac }).await;
  }

  pub async fn seed_to(&self, addr: SocketAddr) {
    self.send(PeerCommand::SeedTo { addr }).await;
  }

  pub async fn resync_stock_connection(&self) {
    self.send(PeerCommand::ResyncStockConnection).await;
  }

  async fn send(&self, cmd: PeerCommand) {
    if self.cmd_tx.send(cmd).await.is_err() {
      tracing::warn!("peer tracker: command channel closed; command dropped");
    }
  }

  #[cfg(test)]
  pub fn noop() -> Self {
    let (cmd_tx, _cmd_rx) = mpsc::channel(1);
    let (_snapshot_tx, snapshot_rx) = watch::channel(PeerSnapshot::default());
    Self { cmd_tx, snapshot_rx }
  }

  #[cfg(test)]
  pub fn scripted() -> (Self, watch::Sender<PeerSnapshot>) {
    let (cmd_tx, _cmd_rx) = mpsc::channel(1);
    let (snapshot_tx, snapshot_rx) = watch::channel(PeerSnapshot::default());
    (Self { cmd_tx, snapshot_rx }, snapshot_tx)
  }
}

struct PeerActor {
  peers: HashMap<Address, Peer>,
  pin_pending: HashSet<Address>,
  bus: WireEventBus,
  player: Player,
  audio: AudioManager,
  capabilities: CapabilitiesRegistry,
  playback_targets: PlaybackTargetStore,
  telephony: TelephonyManager,
  ws_routes: RouteTable,
  stream_routes: RouteTable,
  tunnel_routes: TunnelRoutes,
  log_tap: LogTap,
  snapshot_tx: watch::Sender<PeerSnapshot>,
  cmd_tx: mpsc::Sender<PeerCommand>,
  disconnect_gen: u64,
  pending_bluez_removals: HashSet<Address>,
  presented_connected: bool,
  presented_device: Option<Device>,
}

#[allow(clippy::too_many_arguments)]
async fn run_actor(
  mut cmd_rx: mpsc::Receiver<PeerCommand>,
  cmd_tx: mpsc::Sender<PeerCommand>,
  snapshot_tx: watch::Sender<PeerSnapshot>,
  bus: WireEventBus,
  player: Player,
  audio: AudioManager,
  capabilities: CapabilitiesRegistry,
  playback_targets: PlaybackTargetStore,
  telephony: TelephonyManager,
  ws_routes: RouteTable,
  stream_routes: RouteTable,
  tunnel_routes: TunnelRoutes,
  log_tap: LogTap,
) {
  let mut actor = PeerActor {
    peers: HashMap::new(),
    pin_pending: HashSet::new(),
    bus,
    player,
    audio,
    capabilities,
    playback_targets,
    telephony,
    ws_routes,
    stream_routes,
    tunnel_routes,
    log_tap,
    snapshot_tx,
    cmd_tx,
    disconnect_gen: 0,
    pending_bluez_removals: HashSet::new(),
    presented_connected: false,
    presented_device: None,
  };

  while let Some(cmd) = cmd_rx.recv().await {
    actor.handle(cmd).await;
  }
  tracing::debug!("peer actor: command channel closed; exiting");
}

impl PeerActor {
  async fn handle(&mut self, cmd: PeerCommand) {
    match cmd {
      PeerCommand::Upsert { mac, device } => self.upsert(mac, device).await,
      PeerCommand::EnsureExists { mac, device } => self.ensure_exists(mac, device).await,
      PeerCommand::SetPaired { mac, paired } => self.set_paired(mac, paired).await,
      PeerCommand::SetIap2 { mac, iap2 } => self.set_iap2(mac, iap2).await,
      PeerCommand::SetCompanion { mac, companion } => self.set_companion(mac, companion).await,
      PeerCommand::SetDisplayName { mac, name } => self.set_display_name(mac, name).await,
      PeerCommand::SetLanguage { mac, language } => self.set_language(mac, language).await,
      PeerCommand::SetUuid { mac, uuid } => self.set_uuid(mac, uuid).await,
      PeerCommand::Remove { mac } => self.remove(mac).await,
      PeerCommand::RemoveBluez { mac } => self.remove_bluez(mac).await,
      PeerCommand::NotePinShown { mac } => {
        self.pin_pending.insert(mac);
      }
      PeerCommand::ConfirmPairing { mac } => self.confirm_pairing(mac).await,
      PeerCommand::SeedTo { addr } => self.seed_to(addr).await,
      PeerCommand::ResyncStockConnection => self.resync_stock_connection().await,
      PeerCommand::FlushPendingDisconnect { epoch } => self.flush_pending_disconnect(epoch).await,
    }
  }

  fn publish_snapshot(&self) {
    let _ = self.snapshot_tx.send(PeerSnapshot {
      peers: self.peers.clone(),
    });
  }

  async fn upsert(&mut self, mac: Address, device: Device) {
    self.pending_bluez_removals.remove(&mac);
    let prior = self.peers.get(&mac).cloned();
    let entry = self.peers.entry(mac).or_insert_with(|| Peer::new(device.clone()));
    entry.device = device;
    let diff = Diff::compute(mac, prior, Some(entry.clone()), &self.peers);
    self.broadcast_diff(diff).await;
  }

  async fn ensure_exists(&mut self, mac: Address, device: Device) {
    self.pending_bluez_removals.remove(&mac);
    if self.peers.contains_key(&mac) {
      return;
    }
    let entry = Peer::new(device);
    self.peers.insert(mac, entry.clone());
    let diff = Diff::compute(mac, None, Some(entry), &self.peers);
    self.broadcast_diff(diff).await;
  }

  async fn set_paired(&mut self, mac: Address, paired: bool) {
    let Some(peer) = self.peers.get_mut(&mac) else {
      return;
    };
    let prior = peer.clone();
    peer.paired = paired;
    let snapshot = peer.clone();
    let diff = Diff::compute(mac, Some(prior), Some(snapshot), &self.peers);
    self.broadcast_diff(diff).await;
  }

  async fn set_iap2(&mut self, mac: Address, iap2: PeerIap2Status) {
    let Some(peer) = self.peers.get_mut(&mac) else {
      return;
    };
    let prior = peer.clone();
    peer.iap2 = iap2;
    let snapshot = peer.clone();
    let diff = Diff::compute(mac, Some(prior), Some(snapshot), &self.peers);
    self.broadcast_diff(diff).await;
    self.maybe_complete_bluez_removal(mac).await;
  }

  async fn set_companion(&mut self, mac: Address, companion: PeerCompanionStatus) {
    let Some(peer) = self.peers.get_mut(&mac) else {
      return;
    };
    let prior = peer.clone();
    peer.companion = companion;
    let snapshot = peer.clone();
    let diff = Diff::compute(mac, Some(prior), Some(snapshot), &self.peers);
    self.broadcast_diff(diff).await;
    self.maybe_complete_bluez_removal(mac).await;
  }

  async fn set_display_name(&mut self, mac: Address, display_name: String) {
    let Some(peer) = self.peers.get_mut(&mac) else {
      return;
    };
    let prior = peer.clone();
    peer.display_name = Some(display_name);
    let snapshot = peer.clone();
    let diff = Diff::compute(mac, Some(prior), Some(snapshot), &self.peers);
    self.broadcast_diff(diff).await;
  }

  async fn set_language(&mut self, mac: Address, language: String) {
    let Some(peer) = self.peers.get_mut(&mac) else {
      return;
    };
    let prior = peer.clone();
    peer.language = Some(language);
    let snapshot = peer.clone();
    let diff = Diff::compute(mac, Some(prior), Some(snapshot), &self.peers);
    self.broadcast_diff(diff).await;
  }

  async fn set_uuid(&mut self, mac: Address, uuid: String) {
    let Some(peer) = self.peers.get_mut(&mac) else {
      return;
    };
    let prior = peer.clone();
    peer.uuid = Some(uuid);
    let snapshot = peer.clone();
    let diff = Diff::compute(mac, Some(prior), Some(snapshot), &self.peers);
    self.broadcast_diff(diff).await;
  }

  async fn remove(&mut self, mac: Address) {
    self.pin_pending.remove(&mac);
    self.pending_bluez_removals.remove(&mac);
    let prior = self.peers.remove(&mac);
    if prior.is_none() {
      return;
    }
    let diff = Diff::compute(mac, prior, None, &self.peers);
    self.broadcast_diff(diff).await;
  }

  async fn remove_bluez(&mut self, mac: Address) {
    if self.peers.get(&mac).is_some_and(Peer::has_useful_link) {
      self.pin_pending.remove(&mac);
      self.pending_bluez_removals.insert(mac);
      return;
    }
    self.remove(mac).await;
  }

  async fn maybe_complete_bluez_removal(&mut self, mac: Address) {
    if self.pending_bluez_removals.contains(&mac) && self.peers.get(&mac).is_some_and(|p| !p.has_useful_link()) {
      self.remove(mac).await;
    }
  }

  async fn confirm_pairing(&mut self, mac: Address) {
    let was_pending = self.pin_pending.remove(&mac);
    if !was_pending {
      return;
    }
    if let Err(errs) = self
      .bus
      .broadcast(
        BridgeToClientBluetoothMsg::PairingResult(BluetoothPairingResult { success: true }),
        MsgMeta::Event,
      )
      .await
    {
      log_broadcast_errors("confirm_pairing", errs);
    }
  }

  async fn seed_to(&mut self, addr: SocketAddr) {
    let snapshot: HashMap<String, Peer> = self
      .peers
      .iter()
      .map(|(mac, peer)| (mac.to_string(), peer.clone()))
      .collect();
    if let Err(err) = self
      .bus
      .send_event(addr, BridgeToClientPeerMsgEvent::Snapshot(PeerSnapshotMap(snapshot)))
      .await
    {
      tracing::debug!(?err, %addr, "peer seed snapshot send failed");
    }

    let connected = self.presented_connected;
    if let Some(device) = self.presented_device.clone()
      && let Err(err) = self
        .bus
        .send_event(
          addr,
          BridgeToClientBluetoothMsgEvent::ConnectedDevice(WireConnectedDevice {
            name: device.name.clone(),
            mac: device.id.clone(),
          }),
        )
        .await
    {
      tracing::debug!(?err, %addr, "peer seed connected-device send failed");
    }
    if let Err(err) = self
      .bus
      .send_event(
        addr,
        BridgeToClientBluetoothMsgEvent::Status(BluetoothStatus { connected }),
      )
      .await
    {
      tracing::debug!(?err, %addr, "peer seed bluetooth-status send failed");
    }
  }

  async fn resync_stock_connection(&mut self) {
    let Some(device) = self
      .presented_connected
      .then(|| self.presented_device.clone())
      .flatten()
    else {
      if let Err(errs) = broadcast_stock_disconnection(&self.bus).await {
        log_broadcast_errors("resync stock disconnection", errs);
      }
      return;
    };
    if let Err(errs) = broadcast_stock_connection(&self.bus, &device, &self.capabilities).await {
      log_broadcast_errors("resync_stock_connection", errs);
    }
    if let Err(errs) = self
      .bus
      .broadcast(
        BridgeToClientBluetoothMsg::ConnectedDevice(WireConnectedDevice {
          name: device.name.clone(),
          mac: device.id.clone(),
        }),
        MsgMeta::Event,
      )
      .await
    {
      log_broadcast_errors("resync connected device", errs);
    }
    if let Err(errs) = self
      .bus
      .broadcast(
        BridgeToClientBluetoothMsg::Status(BluetoothStatus { connected: true }),
        MsgMeta::Event,
      )
      .await
    {
      log_broadcast_errors("resync bluetooth status", errs);
    }
    if let Err(err) = self.player.send_state().await {
      tracing::warn!(?err, "failed to send player state during stock resync");
    }
    if let Err(err) = self.audio.broadcast_current().await {
      tracing::warn!(?err, "failed to seed volume on stock activate");
    }
  }

  async fn broadcast_diff(&mut self, diff: Diff) {
    self.publish_snapshot();

    if let Err(errs) = self
      .bus
      .broadcast(
        BridgeToClientPeerMsg::Snapshot(PeerSnapshotMap(diff.snapshot.clone())),
        MsgMeta::Event,
      )
      .await
    {
      log_broadcast_errors("peer snapshot", errs);
    }

    if diff.paired_set_changed {
      let paired_map: HashMap<String, Device> = diff
        .snapshot
        .values()
        .filter(|p| p.paired)
        .map(|p| (p.device.id.clone(), p.device.clone()))
        .collect();
      if let Err(errs) = self
        .bus
        .broadcast(
          BridgeToClientBluetoothMsg::PairedDevices(PairedDevicesMap(paired_map)),
          MsgMeta::Event,
        )
        .await
      {
        log_broadcast_errors("paired devices", errs);
      }
    }

    if diff.paired_transitioned_up {
      self.pin_pending.remove(&diff.mac);
      if let Err(errs) = self
        .bus
        .broadcast(
          BridgeToClientBluetoothMsg::PairingResult(BluetoothPairingResult { success: true }),
          MsgMeta::Event,
        )
        .await
      {
        log_broadcast_errors("pairing result", errs);
      }
    }

    if diff.useful_link_transitioned_up {
      self.disconnect_gen = self.disconnect_gen.wrapping_add(1);
      self.presented_connected = true;
      self.presented_device = diff.useful_device.clone();
      if let Some(device) = diff.useful_device.as_ref() {
        if let Err(errs) = self
          .bus
          .broadcast(
            BridgeToClientBluetoothMsg::ConnectedDevice(WireConnectedDevice {
              name: device.name.clone(),
              mac: device.id.clone(),
            }),
            MsgMeta::Event,
          )
          .await
        {
          log_broadcast_errors("connected device", errs);
        }
        if let Err(errs) = broadcast_stock_connection(&self.bus, device, &self.capabilities).await {
          log_broadcast_errors("stock connection", errs);
        }
        if let Err(err) = self.player.send_state().await {
          tracing::warn!(?err, "failed to send player state after useful link came up");
        }
      }
      if let Err(errs) = self
        .bus
        .broadcast(
          BridgeToClientBluetoothMsg::Status(BluetoothStatus { connected: true }),
          MsgMeta::Event,
        )
        .await
      {
        log_broadcast_errors("bluetooth status up", errs);
      }
    } else if self.presented_connected && !self.peers.values().any(|p| p.has_useful_link()) {
      if diff.removed || diff.companion_lost.is_some() {
        self.disconnect_gen = self.disconnect_gen.wrapping_add(1);
        self.presented_connected = false;
        self.presented_device = None;
        self.broadcast_useful_link_down().await;
      } else if diff.useful_link_transitioned_down {
        self.schedule_disconnect_flush();
      }
    }

    if let Some(addr) = diff.companion_lost {
      if let Err(err) = self.capabilities.clear_companion(addr).await {
        tracing::warn!(?err, "failed to clear companion capabilities on disconnect");
      }
      if let Err(err) = self.player.reset_companion(addr).await {
        tracing::warn!(?err, "failed to reset player queue state on companion disconnect");
      }
      if let Err(err) = self.playback_targets.clear_companion(addr).await {
        tracing::warn!(?err, "failed to clear playback targets on companion disconnect");
      }
      if let Err(err) = self.telephony.clear_companion(addr).await {
        tracing::warn!(?err, "failed to clear companion calls on disconnect");
      }
      let drained = self.log_tap.drain_for_owner(LogOwner::Gateway(Some(addr)));
      if !drained.is_empty() {
        tracing::debug!(count = drained.len(), %addr, "drained gateway log subscriptions on companion disconnect");
      }
      self.tear_down_net_routes(addr).await;
    }
  }

  async fn broadcast_useful_link_down(&self) {
    if let Err(errs) = self
      .bus
      .broadcast(
        BridgeToClientBluetoothMsg::Status(BluetoothStatus { connected: false }),
        MsgMeta::Event,
      )
      .await
    {
      log_broadcast_errors("bluetooth status down", errs);
    }
    if let Err(errs) = broadcast_stock_disconnection(&self.bus).await {
      log_broadcast_errors("stock disconnection", errs);
    }
  }

  fn schedule_disconnect_flush(&mut self) {
    self.disconnect_gen = self.disconnect_gen.wrapping_add(1);
    let epoch = self.disconnect_gen;
    let cmd_tx = self.cmd_tx.clone();
    tokio::spawn(async move {
      tokio::time::sleep(USEFUL_LINK_DOWN_GRACE).await;
      let _ = cmd_tx.send(PeerCommand::FlushPendingDisconnect { epoch }).await;
    });
  }

  async fn flush_pending_disconnect(&mut self, epoch: u64) {
    if epoch != self.disconnect_gen {
      return;
    }
    if self.peers.values().any(|p| p.has_useful_link()) {
      return;
    }
    self.presented_connected = false;
    self.presented_device = None;
    self.broadcast_useful_link_down().await;
  }

  async fn tear_down_net_routes(&self, gateway: Address) {
    use libbridgething::{
      NetError, StreamError, WsError,
      client::{BridgeToClientNetMsgEvent, NetWsClosed, NetWsErrorEvent},
    };

    for (connection_id, owner) in self.ws_routes.drain_for_gateway(gateway) {
      let event = BridgeToClientNetMsgEvent::WsErrorEvent(NetWsErrorEvent {
        connection_id,
        error: WsError::GatewayDisconnected,
      });
      if let Err(err) = self.bus.send_event(owner, event).await {
        tracing::trace!(?err, "ws cleanup send failed");
      }
      let closed = BridgeToClientNetMsgEvent::WsClosed(NetWsClosed {
        connection_id,
        code: 1006,
        reason: "gateway disconnected".into(),
      });
      if let Err(err) = self.bus.send_event(owner, closed).await {
        tracing::trace!(?err, "ws cleanup send failed");
      }
    }

    for (stream_id, owner) in self.stream_routes.drain_for_gateway(gateway) {
      let event = BridgeToClientNetMsgEvent::StreamError(StreamError {
        stream_id,
        error: NetError::NoGateway,
      });
      if let Err(err) = self.bus.send_event(owner, event).await {
        tracing::trace!(?err, "stream cleanup send failed");
      }
    }

    let tunnels = self.tunnel_routes.kill_for_gateway(gateway);
    if tunnels > 0 {
      tracing::debug!(count = tunnels, "closing SOCKS tunnels on companion disconnect");
    }
  }
}

fn log_broadcast_errors(label: &str, errs: Vec<WSError>) {
  tracing::debug!(count = errs.len(), "{label}: peer broadcast errors");
}

struct Diff {
  mac: Address,
  snapshot: HashMap<String, Peer>,
  paired_transitioned_up: bool,
  paired_set_changed: bool,
  useful_link_transitioned_up: bool,
  useful_link_transitioned_down: bool,
  useful_device: Option<Device>,
  companion_lost: Option<Address>,
  removed: bool,
}

impl Diff {
  fn compute(mac: Address, prior: Option<Peer>, current: Option<Peer>, peers: &HashMap<Address, Peer>) -> Self {
    let identity = current.as_ref().or(prior.as_ref()).map(|p| p.device.id.clone());

    let was_paired = prior.as_ref().is_some_and(|p| p.paired);
    let is_paired = current.as_ref().is_some_and(|p| p.paired);
    let was_useful_self = prior.as_ref().is_some_and(|p| p.has_useful_link());
    let is_useful_self = current.as_ref().is_some_and(|p| p.has_useful_link());

    let other_useful = peers
      .values()
      .filter(|p| identity.as_deref() != Some(&p.device.id))
      .any(|p| p.has_useful_link());
    let any_useful_before = other_useful || was_useful_self;
    let any_useful_now = other_useful || is_useful_self;

    let was_companion_connected = matches!(
      prior.as_ref().map(|p| &p.companion),
      Some(PeerCompanionStatus::Connected(_))
    );
    let is_companion_connected = matches!(
      current.as_ref().map(|p| &p.companion),
      Some(PeerCompanionStatus::Connected(_))
    );
    let companion_lost = if was_companion_connected && !is_companion_connected {
      Some(mac)
    } else {
      None
    };

    let snapshot = peers
      .iter()
      .map(|(addr, peer)| (addr.to_string(), peer.clone()))
      .collect();

    Self {
      mac,
      snapshot,
      paired_transitioned_up: !was_paired && is_paired,
      paired_set_changed: was_paired != is_paired,
      useful_link_transitioned_up: !any_useful_before && any_useful_now,
      useful_link_transitioned_down: any_useful_before && !any_useful_now,
      useful_device: if !was_useful_self && is_useful_self {
        current.as_ref().map(|p| p.device.clone())
      } else {
        None
      },
      companion_lost,
      removed: prior.is_some() && current.is_none(),
    }
  }
}
