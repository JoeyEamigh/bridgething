mod bluetooth;
mod net;

mod als;
mod mic;
mod systemd;

mod asset;
mod authority;
mod capabilities;
mod chrome;
mod db;

mod handler;
mod input;
mod install;
mod ota;
mod paths;
mod peer;
mod player;
mod proxy;
mod state;
mod transfer;
mod transport;

mod stock;

mod monitoring;
mod overlay;

use std::{future::Future, net::SocketAddr, path::PathBuf, pin::Pin};

use als::{AlsConfig, AlsManager};
use asset::AssetCache;
use authority::AuthorityRegistry;
pub use bluetooth::{Address, Iap2Event, Iap2InjectTx, Iap2OutboundTapTx, Iap2TransportCommand};
use bluetooth::{BluetoothBringup, BluetoothDeps, BluetoothManager};
use capabilities::CapabilitiesRegistry;
#[cfg(feature = "test-tap")]
pub use handler::client::{ClientMode, PossibleSendMsg};
use handler::{ClientHandler, GatewayHandler};
use libbridgething::{
  BRIDGETHING_NETWORK_GATEWAY_PORT, BRIDGETHING_OTA_RANGE_PROXY_PORT, BRIDGETHING_SOCKS_PROXY_PORT,
  BRIDGETHING_STOCK_WS_PORT, BRIDGETHING_WS_MODERN_PORT,
};
use mic::{MicConfig, MicManager};
#[cfg(feature = "test-tap")]
pub use net::TappedFrame;
use ota::{InstalledWebappApply, OtaOrchestrator, OtaTerminators, RangeProxy, WakewordReload};
use peer::PeerTracker;
use player::Player;
// don't pub anything else from core so that dead code lints still work
pub use state::{AppState, State};
use state::{
  AudioManager, DeviceStore, KvStore, MetaStore, RouteTable, StateAssembly, TelephonyManager, TimeManager,
  WebappRegistry,
};
use systemd::Notify;
use transfer::ChunkedTransfer;
use transport::TransportController;
pub struct Daemon {
  pub state: State,
  pub inject: Option<HeadlessInject>,
  #[cfg(feature = "test-tap")]
  pub server_addrs: ServerAddrs,
  loop_fut: Pin<Box<dyn Future<Output = ()> + Send>>,
}

#[cfg(feature = "test-tap")]
pub const FRAME_TAP_PORT: u16 = 8894;

#[cfg(feature = "test-tap")]
#[derive(Clone, Copy, Debug)]
pub struct ServerAddrs {
  pub stock: SocketAddr,
  pub modern: SocketAddr,
  pub frame_tap: SocketAddr,
  pub proxy: Option<SocketAddr>,
  pub range_proxy: Option<SocketAddr>,
}

impl Daemon {
  pub async fn run(self) {
    self.loop_fut.await
  }
}

pub async fn run_daemon(config: DaemonConfig) {
  init(config).await.run().await;
}

pub async fn init(config: DaemonConfig) -> Daemon {
  bridgething_io::install_crypto_provider();
  let (log_tap, log_tap_layer) = state::LogTap::new();
  if config.install_logger {
    monitoring::init_logger(log_tap_layer);
  }

  let notifier = systemd::init_notifier();

  notifier.status("initializing bridgething...");

  let listeners = net::Listeners::bind(
    config.stock_bind,
    config.modern_bind,
    #[cfg(feature = "test-tap")]
    config.frame_tap_bind,
  )
  .await
  .expect("failed to open client listeners");
  let modern_port = listeners.modern_addr().port();

  let static_meta = state::meta::SuperbirdMeta::read_or_default().await;
  let serial_number = static_meta.serial_number.clone();
  tracing::debug!("metadata: {:?}", &static_meta);

  let (client_man, mut client_listener) = net::create_client_manager();
  let bus = net::WireEventBus::new(client_man.clone());

  let (db, assets_blobs_dir, transfers_dir) = match config.state_dir.as_deref() {
    Some(dir) => {
      let assets = dir.join("assets");
      let transfers = dir.join("transfers");
      tokio::fs::create_dir_all(&assets)
        .await
        .expect("failed to create harness asset dir");
      tokio::fs::create_dir_all(&transfers)
        .await
        .expect("failed to create harness transfer dir");
      let db = db::open(None).await.expect("failed to open in-memory state database");
      (db, assets, transfers)
    }
    None => {
      let db = state::open_state_db().await.expect("failed to open state database");
      (db, paths::assets_blobs_dir(), paths::transfers_dir())
    }
  };

  let devices = DeviceStore::new(db.clone(), bus.clone());
  let kv = KvStore::new(db.clone());
  let meta_store = MetaStore::new(db.clone());

  ota::retire_superseded_wakeword_model().await;

  let meta = state::meta::DeviceMeta::init(static_meta, kv.clone()).await;

  let installed_webapps_root = config.webapps_dir.clone().unwrap_or_else(paths::webapps_dir);
  let builtin_webapps_root = config.ro_webapps_dir.clone().unwrap_or_else(paths::ro_webapps_dir);
  let seed_marker = installed_webapps_root
    .parent()
    .map(|p| p.join(".seeded"))
    .unwrap_or_else(|| installed_webapps_root.join(".seeded"));
  let webapps = WebappRegistry::init(
    installed_webapps_root,
    builtin_webapps_root,
    state::storage::WebappProvenanceStore::new(db.clone()),
  )
  .await
  .expect("failed to initialize webapp registry");
  meta_store
    .enforce_active_webapp_exists(&webapps)
    .await
    .expect("failed to enforce active webapp invariant");

  let asset_pending = AssetCache::init(db.clone(), assets_blobs_dir)
    .await
    .expect("failed to initialize asset cache");
  let (assets, asset_cache_handle) = asset_pending.spawn();

  let bandaid_sweep = if paths::is_on_device() {
    vec![paths::bandaid_transfers_dir()]
  } else {
    Vec::new()
  };
  let transfer_pending = ChunkedTransfer::init(transfers_dir, bandaid_sweep)
    .await
    .expect("failed to initialize chunked transfer manager");
  let (transfers, transfer_handle) = transfer_pending.spawn();

  let ws_routes = RouteTable::new();
  let stream_routes = RouteTable::new();
  let geo_watchers = state::GeoWatchers::new();
  let geo_last_fix = state::GeoLastFix::new();
  let tunnel_routes = state::TunnelRoutes::new();

  let authority = AuthorityRegistry::new();
  let capabilities = CapabilitiesRegistry::new(bus.clone(), authority.clone());
  let player = Player::new(bus.clone(), authority.clone(), assets.clone());
  let audio = AudioManager::new(authority.clone(), bus.clone());
  let playback_targets = state::PlaybackTargetStore::new(bus.clone());
  let asset_wait = asset::wait::AssetWaitTracker::new();
  let _asset_invalidator = asset::wait::spawn_invalidator(assets.clone(), asset_wait.clone());
  let iap2_pending_art = handler::iap2::Iap2PendingArt::new();
  let spotify_wake_gate = handler::gateway::SpotifyWakeGate::new();
  let (bluetooth, bluetooth_bootstrap) = BluetoothManager::create(
    serial_number
      .chars()
      .rev()
      .take(4)
      .collect::<Vec<_>>()
      .into_iter()
      .rev()
      .collect::<Vec<_>>()
      .try_into()
      .unwrap_or(['D', 'E', 'A', 'D']),
  );

  let telephony = TelephonyManager::new(bus.clone(), bluetooth.iap2.telephony.clone());

  let peers = PeerTracker::new(
    bus.clone(),
    player.clone(),
    audio.clone(),
    capabilities.clone(),
    playback_targets.clone(),
    telephony.clone(),
    ws_routes.clone(),
    stream_routes.clone(),
    tunnel_routes.clone(),
    log_tap.clone(),
  );

  let chrome = chrome::Chrome::init(format!("http://127.0.0.1:{modern_port}/"))
    .await
    .expect("failed to initialize chrome");

  let (bluetooth_tx, mut bluetooth_rx) = tokio::sync::mpsc::channel(16);
  let bluetooth_deps = BluetoothDeps {
    #[cfg(target_os = "linux")]
    bus: bus.clone(),
    meta: meta.clone(),
    #[cfg(target_os = "linux")]
    devices: devices.clone(),
    peers: peers.clone(),
  };
  let time = TimeManager::new(bus.clone());

  let (als, als_handle) = AlsManager::init(bus.clone(), AlsConfig::default())
    .await
    .expect("failed to initialize ALS manager")
    .spawn();
  let (mic, mic_handle) = MicManager::init(bus.clone(), bluetooth.clone(), MicConfig::default())
    .await
    .spawn();

  let transfer_sinks = transfer::sinks::TransferSinks::default();

  let range_proxy_handle = RangeProxy::spawn(
    bluetooth.clone(),
    transfer_sinks.clone(),
    assets.clone(),
    config
      .state_dir
      .clone()
      .unwrap_or_else(paths::state_dir)
      .join("range-cache"),
    config.range_proxy_bind,
  )
  .await;

  let installed_apply: InstalledWebappApply = {
    let webapps = webapps.clone();
    let kv = kv.clone();
    let bus = bus.clone();
    let bluetooth = bluetooth.clone();
    std::sync::Arc::new(move |path, provenance| {
      let webapps = webapps.clone();
      let kv = kv.clone();
      let bus = bus.clone();
      let bluetooth = bluetooth.clone();
      Box::pin(async move { install::apply_and_announce(&webapps, &kv, &bus, &bluetooth, path, provenance).await })
    })
  };

  let wakeword_reload: WakewordReload = {
    let mic = mic.clone();
    let meta = meta.clone();
    std::sync::Arc::new(move || {
      let mic = mic.clone();
      let meta = meta.clone();
      Box::pin(async move {
        if let Err(err) = mic.reload_wakeword().await {
          tracing::warn!("could not reload the wake word after an update: {err}");
        }
        meta.refresh_wakeword_model_version().await;
      })
    })
  };

  let (ota_events_tx, ota_events_rx) = tokio::sync::mpsc::channel(64);
  let (ota, _ota_handle) = OtaOrchestrator::spawn(
    transfers.clone(),
    ota_events_tx,
    bluetooth.gateway_man.clone(),
    OtaTerminators {
      reboot: std::sync::Arc::new(trigger_reboot),
      restart_self: std::sync::Arc::new(trigger_restart_self),
    },
    range_proxy_handle.proxy.clone(),
    peers.clone(),
    installed_apply,
    wakeword_reload,
    transfer_sinks.clone(),
    assets.clone(),
  );

  let state = AppState::assemble(StateAssembly {
    client_man: client_man.clone(),
    bus,
    modern_port,
    meta,
    player,
    chrome,
    webapps,
    assets,
    asset_wait,
    iap2_pending_art,
    spotify_wake_gate,
    authority,
    capabilities,
    playback_targets,
    peers,
    telephony,
    time,
    audio,
    als,
    mic,
    devices,
    kv,
    ws_routes,
    stream_routes,
    geo_watchers,
    geo_last_fix,
    log_tap,
    tunnel_routes,
    transfer_sinks,
    db,
    meta_store,
    asset_cache_handle,
    transfer_handle,
    als_handle,
    mic_handle,
  });

  spawn_ota_event_forwarder(bluetooth.clone(), state.client_man.clone(), ota_events_rx);
  spawn_nickname_observer(
    state.meta.subscribe(),
    bluetooth.clone(),
    state.bus.clone(),
    serial_number.clone(),
  );
  spawn_next_art_warmer(state.clone(), bluetooth.clone());
  spawn_primary_companion_resync(state.authority.clone(), bluetooth.clone());
  spawn_asset_event_forwarder(state.assets.subscribe(), state.bus.clone());

  let transport = TransportController::new(
    state.authority.clone(),
    state.player.clone(),
    bluetooth.clone(),
    bluetooth.iap2.transport.clone(),
  );

  notifier.status("starting servers...");
  let proxy_listener = proxy::bind(config.proxy_bind).await;
  #[cfg(feature = "test-tap")]
  let server_addrs = ServerAddrs {
    stock: listeners.stock_addr(),
    modern: listeners.modern_addr(),
    frame_tap: listeners.frame_tap_addr(),
    proxy: proxy_listener.as_ref().and_then(|l| l.local_addr().ok()),
    range_proxy: range_proxy_handle.bound_addr,
  };
  if let Some(listener) = proxy_listener {
    proxy::spawn(listener, state.clone(), bluetooth.clone());
  }
  let server = net::Server::serve(state.clone(), listeners);

  if let Err(err) = state.chrome.send(chrome::ChromeCommand::NoteServing).await {
    tracing::warn!("failed to tell the chrome worker we are serving: {err:?}");
  }

  state.sync_overlay(true).await;

  if let Some(examples_dir) = config.examples_dir.clone() {
    install::seed_examples(&state.webapps, &examples_dir, &seed_marker).await;
  }

  let client_handler = ClientHandler::new(state.clone(), bluetooth.clone(), transport.clone());
  let gateway_handler = GatewayHandler::new(state.clone(), bluetooth.clone(), ota, transport);

  let _input = input::InputManager::spawn(state.clone());

  let (bringup, headless_inject) = match config.bluetooth {
    #[cfg(target_os = "linux")]
    BluetoothMode::Real => {
      notifier.status("initializing bluetooth stack...");
      (BluetoothBringup::Real, None)
    }
    #[cfg(not(target_os = "linux"))]
    BluetoothMode::Real => panic!("this build has no bluez; only BluetoothMode::Headless can be brought up"),
    BluetoothMode::Headless => {
      notifier.status("initializing gateway transports (no radio)...");
      let (inject_tx, inject_rx) = bluetooth::inject_channel();
      let inject = HeadlessInject {
        rfcomm: inject_tx,
        iap2: bluetooth_bootstrap.iap2_inject_tx(),
        iap2_outbound: bluetooth_bootstrap.iap2_outbound_tap(),
      };
      (BluetoothBringup::Headless(inject_rx), Some(inject))
    }
  };
  let bluetooth_handle = bluetooth.spawn(
    bluetooth_bootstrap,
    bluetooth_deps,
    state.clone(),
    bluetooth_tx.clone(),
    config.network_bind,
    bringup,
  );

  notifier.ready(true, Some("ready to accept connections..."));

  let state_out = state.clone();
  let handle_signals = config.handle_signals;

  let iap2_shutdown = bluetooth.iap2.shutdown.clone();

  let loop_fut = Box::pin(async move {
    let _asset_invalidator = _asset_invalidator;
    let _ota_handle = _ota_handle;
    let _bluetooth_handle = bluetooth_handle;
    let _input = _input;

    let mut server = server;
    let range_proxy_handle = range_proxy_handle;

    loop {
      tokio::select! {
        client_conn = server.listen() => {
          if let Ok((stream, address, mode)) = client_conn
            && let Err(err) = client_man.handle_connection(address, stream, mode, &state).await {
              tracing::error!("failed to accept tcp stream: {:?}", err);
            }
        },
        Ok(msg) = client_listener.recv() => {
          if let Err(err) = client_handler.handle(msg).await {
            tracing::error!("failed to handle websocket message: {:?}", err);
          }
        },
        Some(msg) = bluetooth_rx.recv() => {
          match msg {
            bluetooth::BluetoothEvent::Gateway(data) => {
              if let Err(err) = gateway_handler.handle(data).await {
                tracing::error!("failed to handle bluetooth message: {:?}", err);
              }
            }
          }
        },
        _ = monitoring::wait_for_signal(), if handle_signals => {
          break;
        }
      }
    }

    tracing::info!("shutting down...");
    iap2_shutdown.shutdown().await;
    state.chrome.shutdown().await;
    server.shutdown().await;
    range_proxy_handle.cancel.cancel();

    tracing::info!("thank you for using bridgething!");
  });

  Daemon {
    state: state_out,
    inject: headless_inject,
    #[cfg(feature = "test-tap")]
    server_addrs,
    loop_fut,
  }
}

#[derive(Clone)]
pub struct HeadlessInject {
  pub rfcomm: bluetooth::InjectConnectionTx,
  pub iap2: bluetooth::Iap2InjectTx,
  pub iap2_outbound: bluetooth::Iap2OutboundTapTx,
}

pub enum BluetoothMode {
  Real,
  Headless,
}

pub struct DaemonConfig {
  pub bluetooth: BluetoothMode,
  pub network_bind: SocketAddr,
  pub stock_bind: SocketAddr,
  pub modern_bind: SocketAddr,
  pub proxy_bind: SocketAddr,
  pub range_proxy_bind: SocketAddr,
  #[cfg(feature = "test-tap")]
  pub frame_tap_bind: SocketAddr,
  pub handle_signals: bool,
  pub install_logger: bool,
  pub state_dir: Option<PathBuf>,
  pub webapps_dir: Option<PathBuf>,
  pub ro_webapps_dir: Option<PathBuf>,
  pub examples_dir: Option<PathBuf>,
}

impl DaemonConfig {
  pub fn real() -> Self {
    Self {
      bluetooth: BluetoothMode::Real,
      network_bind: SocketAddr::from(([0, 0, 0, 0], BRIDGETHING_NETWORK_GATEWAY_PORT)),
      stock_bind: SocketAddr::from(([0, 0, 0, 0], BRIDGETHING_STOCK_WS_PORT)),
      modern_bind: SocketAddr::from(([0, 0, 0, 0], BRIDGETHING_WS_MODERN_PORT)),
      proxy_bind: SocketAddr::from(([127, 0, 0, 1], BRIDGETHING_SOCKS_PROXY_PORT)),
      range_proxy_bind: SocketAddr::from(([127, 0, 0, 1], BRIDGETHING_OTA_RANGE_PROXY_PORT)),
      #[cfg(feature = "test-tap")]
      frame_tap_bind: SocketAddr::from(([0, 0, 0, 0], FRAME_TAP_PORT)),
      handle_signals: true,
      install_logger: true,
      state_dir: None,
      webapps_dir: None,
      ro_webapps_dir: None,
      examples_dir: Some(paths::examples_dir()),
    }
  }

  pub fn dev() -> Self {
    Self {
      bluetooth: BluetoothMode::Headless,
      network_bind: SocketAddr::from(([127, 0, 0, 1], BRIDGETHING_NETWORK_GATEWAY_PORT)),
      stock_bind: SocketAddr::from(([127, 0, 0, 1], BRIDGETHING_STOCK_WS_PORT)),
      modern_bind: SocketAddr::from(([127, 0, 0, 1], BRIDGETHING_WS_MODERN_PORT)),
      #[cfg(feature = "test-tap")]
      frame_tap_bind: SocketAddr::from(([127, 0, 0, 1], FRAME_TAP_PORT)),
      ..Self::real()
    }
  }

  #[cfg(feature = "test-tap")]
  pub fn headless(state_dir: PathBuf) -> Self {
    Self {
      bluetooth: BluetoothMode::Headless,
      network_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
      stock_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
      modern_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
      proxy_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
      range_proxy_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
      frame_tap_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
      handle_signals: false,
      install_logger: false,
      webapps_dir: Some(state_dir.join("webapps")),
      ro_webapps_dir: Some(state_dir.join("builtin")),
      examples_dir: None,
      state_dir: Some(state_dir),
    }
  }
}

fn spawn_ota_event_forwarder(
  bluetooth: bluetooth::BluetoothMan,
  client_man: net::ClientMan,
  mut rx: tokio::sync::mpsc::Receiver<libbridgething::gateway::BridgeToGatewaySystemMsgEvent>,
) {
  use libbridgething::{client::BridgeToClientSystemMsgEvent, gateway::BridgeToGatewaySystemMsgEvent};
  tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
      let client_mirror = match &event {
        BridgeToGatewaySystemMsgEvent::OtaProgress(p) => Some(BridgeToClientSystemMsgEvent::OtaProgress(*p)),
        BridgeToGatewaySystemMsgEvent::OtaError(e) => Some(BridgeToClientSystemMsgEvent::OtaError(e.clone())),
        BridgeToGatewaySystemMsgEvent::OtaFinished(f) => Some(BridgeToClientSystemMsgEvent::OtaFinished(f.clone())),
        BridgeToGatewaySystemMsgEvent::DeviceNicknameChanged(_) => None,
        BridgeToGatewaySystemMsgEvent::LogEntry(_) => None,
      };
      match event {
        BridgeToGatewaySystemMsgEvent::OtaProgress(_) => bluetooth.gateway_man.broadcast_event_background(event).await,
        _ => bluetooth.gateway_man.broadcast(event).await,
      }
      if let Some(mirror) = client_mirror {
        let _ = client_man.broadcast_event(mirror).await;
      }
    }
  });
}

fn spawn_asset_event_forwarder(
  mut rx: tokio::sync::broadcast::Receiver<asset::AssetCacheEvent>,
  bus: net::WireEventBus,
) {
  use libbridgething::client::{AssetCleared, AssetReady, BridgeToClientAssetMsgEvent};
  use tokio::sync::broadcast::error::RecvError;
  tokio::spawn(async move {
    loop {
      let event = match rx.recv().await {
        Ok(event) => event,
        Err(RecvError::Lagged(skipped)) => {
          tracing::debug!(skipped, "asset event forwarder lagged");
          continue;
        }
        Err(RecvError::Closed) => break,
      };
      let client_event = match event {
        asset::AssetCacheEvent::Ready { id } => BridgeToClientAssetMsgEvent::Ready(AssetReady { id }),
        asset::AssetCacheEvent::Cleared { id } => BridgeToClientAssetMsgEvent::Cleared(AssetCleared { id }),
      };
      if let Err(errs) = bus.broadcast_event(client_event).await {
        tracing::debug!(count = errs.len(), "asset-event client broadcast non-fatal errors");
      }
    }
  });
}

fn spawn_nickname_observer(
  mut rx: tokio::sync::watch::Receiver<Option<String>>,
  bluetooth: bluetooth::BluetoothMan,
  bus: net::WireEventBus,
  serial: String,
) {
  use libbridgething::{
    client::{BridgeToClientSystemMsgEvent, DeviceNicknameReply as ClientNicknameReply},
    gateway::{BridgeToGatewaySystemMsgEvent, DeviceNicknameReply as GatewayNicknameReply},
  };
  tokio::spawn(async move {
    loop {
      let value = rx.borrow_and_update().clone();

      systemd::avahi::publish_bridgething_service(value.as_deref(), &serial).await;

      bluetooth
        .gateway_man
        .broadcast(BridgeToGatewaySystemMsgEvent::DeviceNicknameChanged(
          GatewayNicknameReply {
            nickname: value.clone(),
          },
        ))
        .await;

      let client_event = BridgeToClientSystemMsgEvent::DeviceNicknameChanged(ClientNicknameReply { nickname: value });
      if let Err(errs) = bus.broadcast_event(client_event).await {
        tracing::debug!(count = errs.len(), "nickname-change client broadcast non-fatal errors");
      }

      if rx.changed().await.is_err() {
        break;
      }
    }
  });
}

fn spawn_next_art_warmer(state: State, bluetooth: bluetooth::BluetoothMan) {
  let mut rx = state.player.snapshot_watch();
  tokio::spawn(async move {
    let mut last: Option<String> = None;
    loop {
      let head_art = rx
        .borrow_and_update()
        .queue_reply
        .items
        .first()
        .and_then(|item| item.artwork_id.clone())
        .filter(|id| !id.is_empty());
      if let Some(id) = head_art
        && last.as_deref() != Some(id.as_str())
        && state.gateway_info().is_some()
      {
        last = Some(id.clone());
        handler::client::asset::preload_assets(state.clone(), bluetooth.clone(), vec![id]).await;
      }
      if rx.changed().await.is_err() {
        break;
      }
    }
  });
}

fn spawn_primary_companion_resync(authority: AuthorityRegistry, bluetooth: bluetooth::BluetoothMan) {
  let mut rx = authority.primary_subscription_rx();
  tokio::spawn(async move {
    let mut prev = *rx.borrow_and_update();
    loop {
      if rx.changed().await.is_err() {
        break;
      }
      let next = *rx.borrow_and_update();
      let handed_over = prev.is_some() && next.is_some() && prev != next;
      prev = next;
      let Some(addr) = next.filter(|_| handed_over) else {
        continue;
      };
      let gateway_man = bluetooth.gateway_man.clone();
      tokio::spawn(async move {
        match gateway_man
          .request(Some(addr), libbridgething::gateway::PlayerSnapshotRequest {})
          .await
        {
          Ok(_) => tracing::debug!(%addr, "new primary companion acked the snapshot request"),
          Err(err) => tracing::debug!(?err, %addr, "new primary companion did not answer the snapshot request"),
        }
      });
    }
  });
}

fn trigger_reboot() {
  tokio::spawn(async {
    if let Err(err) = systemd::power::reboot().await {
      tracing::error!("ota reboot failed: {err}");
    }
  });
}

fn trigger_restart_self() {
  tokio::spawn(async {
    if let Err(err) = systemd::power::restart_self().await {
      tracing::error!("ota daemon restart failed: {err}");
    }
  });
}
