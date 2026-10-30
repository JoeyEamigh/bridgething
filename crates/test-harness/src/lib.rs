use std::{
  path::Path,
  sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
  },
  time::Duration,
};

#[cfg(feature = "chrome")]
mod chrome;
#[cfg(target_os = "linux")]
mod device;
pub mod model;
mod reset;
mod seam;
use anyhow::Result;
use bridgething::{
  Address, DaemonConfig, HeadlessInject, Iap2Event, Iap2TransportCommand, LeCommand, ServerAddrs, State, TappedFrame,
};
use bridgething_client::Client as CommandClient;
use bridgething_gateway::Gateway;
use bridgething_iap2::SessionEvent;
#[cfg(feature = "chrome")]
pub use chrome::ChromeView;
#[cfg(target_os = "linux")]
pub use device::DeviceHarness;
use futures::StreamExt;
use libbridgething::{
  Device, DeviceType, LinkKind, PeerIap2Status,
  gateway::{BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewayVoiceMsg},
};
pub use reset::FakeResetRig;
pub use seam::{
  CommandDriver, FrameObserve, GatewayDriver, HarnessIap2Source, Iap2OutboundObserve, Iap2Source, Iap2SourceDriver,
  ModernClientDriver, OverAirTransport, WebappProvision,
};
#[cfg(target_os = "linux")]
pub use seam::{DeviceIap2Source, DeviceTier};
use tokio::{net::TcpStream, sync::broadcast, task::JoinHandle};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DUPLEX_BUF: usize = 256 * 1024;
const FRAME_OBSERVER_CAPACITY: usize = 256;
const IDENTIFY_DEADLINE: Duration = Duration::from_secs(5);
const VOICE_TURN_DEADLINE: Duration = Duration::from_secs(5);

pub struct Harness {
  state: State,
  inject: HeadlessInject,
  server_addrs: ServerAddrs,
  next_peer: AtomicU8,
  reset_rig: Arc<FakeResetRig>,
  stop: CancellationToken,
  daemon: JoinHandle<()>,
  _state_dir: tempfile::TempDir,
}

impl Harness {
  pub async fn start() -> Result<Self> {
    Self::start_inner(StockBundle::None).await
  }

  pub async fn start_with_stock_webapp() -> Result<Self> {
    Self::start_inner(StockBundle::Dist).await
  }

  pub async fn start_with_stub_stock_webapp() -> Result<Self> {
    Self::start_inner(StockBundle::Stub).await
  }

  async fn start_inner(stock: StockBundle) -> Result<Self> {
    let state_dir = tempfile::tempdir()?;
    match stock {
      StockBundle::None => {}
      StockBundle::Dist => provision_stock_bundle(state_dir.path())?,
      StockBundle::Stub => provision_stub_stock_bundle(state_dir.path())?,
    }
    let reset_rig = Arc::new(FakeResetRig::default());
    let assembled = bridgething::init(headless_config(state_dir.path(), &reset_rig)).await;

    let state = assembled.state.clone();
    let inject = assembled
      .inject
      .clone()
      .expect("headless assembly must expose inject handles");
    let server_addrs = assembled.server_addrs;
    let stop = assembled.stop.clone();
    let daemon = tokio::spawn(assembled.run());

    Ok(Self {
      state,
      inject,
      server_addrs,
      next_peer: AtomicU8::new(1),
      reset_rig,
      stop,
      daemon,
      _state_dir: state_dir,
    })
  }

  pub fn state(&self) -> &State {
    &self.state
  }

  pub fn state_dir(&self) -> &Path {
    self._state_dir.path()
  }

  pub fn reset_rig(&self) -> &FakeResetRig {
    &self.reset_rig
  }

  pub fn stop_daemon(&self) {
    self.stop.cancel();
  }

  pub fn stop_daemon_on_reboot(&self) {
    self.reset_rig.stop_on_reboot(self.stop.clone());
  }

  pub fn daemon_exited(&self) -> bool {
    self.daemon.is_finished()
  }

  pub async fn activate_webapp_declaring(&self, permissions: &[&str]) -> Result<Uuid> {
    let id = Uuid::now_v7();
    let dir = self._state_dir.path().join("webapps").join(id.simple().to_string());
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("index.html"), b"<!doctype html><title>harness</title>")?;
    let declared = permissions
      .iter()
      .map(|p| format!("\"{p}\""))
      .collect::<Vec<_>>()
      .join(",");
    std::fs::write(
      dir.join("manifest.json"),
      format!(r#"{{"id":"{id}","name":"harness","version":"0.1.0","config":[],"permissions":[{declared}]}}"#),
    )?;
    self.state.webapps.rescan().await;
    self.state.set_active_webapp(id).await?;
    Ok(id)
  }

  pub async fn restart(mut self) -> Result<Self> {
    self.daemon.abort();
    let _ = (&mut self.daemon).await;
    let assembled = bridgething::init(headless_config(self._state_dir.path(), &self.reset_rig)).await;
    self.state = assembled.state.clone();
    self.inject = assembled
      .inject
      .clone()
      .expect("headless assembly must expose inject handles");
    self.server_addrs = assembled.server_addrs;
    self.stop = assembled.stop.clone();
    self.daemon = tokio::spawn(assembled.run());
    Ok(self)
  }

  pub async fn connect_android(&self) -> Result<Gateway> {
    Ok(Gateway::from_io(self.connect_android_io().await?))
  }

  pub async fn connect_android_io(&self) -> Result<tokio::io::DuplexStream> {
    let (daemon_half, phone_half) = tokio::io::duplex(DUPLEX_BUF);
    let n = self.next_peer.fetch_add(1, Ordering::Relaxed);
    let addr = Address([0xFE, 0xED, 0x00, 0x00, 0x00, n]);
    self.inject.rfcomm.send((addr, daemon_half)).await?;
    Ok(phone_half)
  }

  pub async fn inject_iap2(&self, address: Address, event: SessionEvent) -> Result<()> {
    self.inject.iap2.send(Iap2Event { address, event }).await?;
    Ok(())
  }

  pub async fn iap2_artwork(&self, address: Address, transfer_id: u8, bytes: Vec<u8>) -> Result<()> {
    self
      .inject_iap2(
        address,
        SessionEvent::ArtworkBytes {
          transfer_id,
          bytes: bytes.into(),
        },
      )
      .await
  }

  pub async fn identify_iphone(&self, mac: Address) -> Result<()> {
    let peers = self.state.peers.clone();
    peers
      .ensure_exists(
        mac,
        Device {
          name: "test-iphone".into(),
          device_type: DeviceType::Ios,
          id: mac.to_string(),
          kind: LinkKind::Bluetooth,
          default: false,
        },
      )
      .await;
    peers.set_iap2(mac, PeerIap2Status::Identified).await;
    let up = self
      .wait_for(
        |s| s.peers.snapshot().peers.get(&mac).is_some_and(|p| p.has_useful_link()),
        IDENTIFY_DEADLINE,
      )
      .await;
    anyhow::ensure!(up, "iap2 peer {mac} never reached a useful link");
    Ok(())
  }

  pub async fn resolving_voice_turn(&self, listener: &Gateway) -> Result<Uuid> {
    let mut inbound = listener.events();
    let client = self.connect_command_client().await?;
    client.voice().push_to_talk().await?;
    let opened = next_voice(&mut inbound, |voice| match voice {
      BridgeToGatewayVoiceMsg::StreamOpen(open) => Some(open.stream_id),
      _ => None,
    })
    .await
    .ok_or_else(|| anyhow::anyhow!("the listener never saw the turn open"))?;
    client.voice().release().await?;
    next_voice(&mut inbound, |voice| match voice {
      BridgeToGatewayVoiceMsg::StreamClose(close) if close.stream_id == opened => Some(()),
      _ => None,
    })
    .await
    .ok_or_else(|| anyhow::anyhow!("the listener never saw turn {opened} close"))?;
    Ok(opened)
  }

  pub fn iap2_peer(&self) -> Address {
    let n = self.next_peer.fetch_add(1, Ordering::Relaxed);
    Address([0xA9, 0x00, 0x00, 0x00, 0x00, n])
  }

  pub async fn wait_for<F>(&self, mut predicate: F, timeout: Duration) -> bool
  where
    F: FnMut(&State) -> bool,
  {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
      if predicate(&self.state) {
        return true;
      }
      if tokio::time::Instant::now() >= deadline {
        return false;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  }

  pub fn observe_frames(&self) -> FrameObserver {
    FrameObserver {
      rx: self.state.client_man.subscribe_frames(),
    }
  }

  pub async fn connect_modern_client(&self) -> Result<MockWsClient> {
    let (stream, _resp) = connect_async(format!("ws://{}/", self.server_addrs.modern)).await?;
    Ok(MockWsClient { stream })
  }

  pub async fn connect_command_client(&self) -> Result<CommandClient> {
    Ok(CommandClient::connect(&format!("ws://{}/", self.server_addrs.modern)).await?)
  }

  pub async fn connect_overlay_command_client(&self) -> Result<CommandClient> {
    Ok(CommandClient::connect(&format!("ws://{}/?scope=overlay", self.server_addrs.modern)).await?)
  }

  pub fn observe_iap2_outbound(&self) -> Iap2OutboundObserver {
    TapObserver {
      rx: self.inject.iap2_outbound.subscribe(),
    }
  }

  pub fn observe_le(&self) -> LeObserver {
    TapObserver {
      rx: self.inject.le.subscribe(),
    }
  }

  pub async fn connect_stock_client(&self) -> Result<MockWsClient> {
    let (stream, _resp) = connect_async(format!("ws://{}/", self.server_addrs.stock)).await?;
    Ok(MockWsClient { stream })
  }

  pub fn modern_addr(&self) -> std::net::SocketAddr {
    self.server_addrs.modern
  }

  pub fn range_proxy_addr(&self) -> Option<std::net::SocketAddr> {
    self.server_addrs.range_proxy
  }

  pub fn proxy_addr(&self) -> std::net::SocketAddr {
    self.server_addrs.proxy.expect("SOCKS proxy bound")
  }

  #[cfg(feature = "chrome")]
  pub async fn open_stock_chrome(&self) -> Result<ChromeView> {
    ChromeView::launch(self.server_addrs.modern, self.server_addrs.stock.port()).await
  }

  pub async fn connect_frame_tap_ws(&self) -> Result<FrameObserver> {
    frame_tap_ws_observer(&format!("ws://{}/", self.server_addrs.frame_tap)).await
  }
}

pub struct FrameObserver {
  rx: broadcast::Receiver<TappedFrame>,
}

impl FrameObserver {
  pub async fn wait_for<F>(&mut self, timeout: Duration, pred: F) -> Option<TappedFrame>
  where
    F: Fn(&TappedFrame) -> bool,
  {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
      let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
      if remaining.is_zero() {
        return None;
      }
      match tokio::time::timeout(remaining, self.rx.recv()).await {
        Ok(Ok(frame)) if pred(&frame) => return Some(frame),
        Ok(Ok(_)) => continue,
        Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
        Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return None,
      }
    }
  }

  pub async fn collect_for(&mut self, window: Duration) -> Vec<TappedFrame> {
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    loop {
      let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
      if remaining.is_zero() {
        return frames;
      }
      match tokio::time::timeout(remaining, self.rx.recv()).await {
        Ok(Ok(frame)) => frames.push(frame),
        Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
        Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return frames,
      }
    }
  }
}

async fn next_voice<T>(
  inbound: &mut broadcast::Receiver<BridgeToGatewayMsg>,
  pick: impl Fn(BridgeToGatewayVoiceMsg) -> Option<T>,
) -> Option<T> {
  tokio::time::timeout(VOICE_TURN_DEADLINE, async {
    loop {
      match inbound.recv().await {
        Ok(BridgeToGatewayMsg {
          data: BridgeToGatewayMsgData::Voice(voice),
          ..
        }) => {
          if let Some(hit) = pick(voice) {
            return Some(hit);
          }
        }
        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
        Err(broadcast::error::RecvError::Closed) => return None,
      }
    }
  })
  .await
  .ok()
  .flatten()
}

pub fn proptest_cases(default: u32) -> u32 {
  std::env::var("PROPTEST_CASES")
    .ok()
    .and_then(|raw| raw.trim().parse().ok())
    .unwrap_or(default)
}

pub fn extract_substring_starting_with(haystack: &str, prefix: &str) -> Option<String> {
  let start = haystack.find(prefix)?;
  let tail = &haystack[start..];
  let end = tail
    .find(|c: char| c == '"' || c == '\\' || c.is_whitespace())
    .unwrap_or(tail.len());
  Some(tail[..end].to_string())
}

pub type Iap2OutboundObserver = TapObserver<Iap2TransportCommand>;

pub type LeObserver = TapObserver<LeCommand>;

pub struct TapObserver<T> {
  rx: broadcast::Receiver<T>,
}

impl<T: Clone> TapObserver<T> {
  pub async fn wait_for<F>(&mut self, timeout: Duration, pred: F) -> Option<T>
  where
    F: Fn(&T) -> bool,
  {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
      let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
      if remaining.is_zero() {
        return None;
      }
      match tokio::time::timeout(remaining, self.rx.recv()).await {
        Ok(Ok(cmd)) if pred(&cmd) => return Some(cmd),
        Ok(Ok(_)) => continue,
        Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
        Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return None,
      }
    }
  }

  pub async fn collect_for(&mut self, window: Duration) -> Vec<T> {
    let mut cmds = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    loop {
      let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
      if remaining.is_zero() {
        return cmds;
      }
      match tokio::time::timeout(remaining, self.rx.recv()).await {
        Ok(Ok(cmd)) => cmds.push(cmd),
        Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
        Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return cmds,
      }
    }
  }
}

pub async fn frame_tap_ws_observer(url: &str) -> Result<FrameObserver> {
  let (stream, _resp) = connect_async(url).await?;
  let (tx, _seed_rx) = broadcast::channel(FRAME_OBSERVER_CAPACITY);
  let rx = tx.subscribe();

  tokio::spawn(async move {
    let mut stream = stream;
    while let Some(msg) = stream.next().await {
      match msg {
        Ok(Message::Text(text)) => match serde_json::from_str::<TappedFrame>(text.as_str()) {
          Ok(frame) => {
            let _ = tx.send(frame);
          }
          Err(err) => eprintln!("frame-tap WS observer failed to decode a frame: {err}"),
        },
        Ok(Message::Close(_)) | Err(_) => break,
        Ok(_) => continue,
      }
    }
  });

  Ok(FrameObserver { rx })
}

pub struct MockWsClient {
  stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl MockWsClient {
  pub async fn recv(&mut self) -> Option<String> {
    while let Some(msg) = self.stream.next().await {
      match msg {
        Ok(Message::Text(text)) => return Some(text.to_string()),
        Ok(_) => continue,
        Err(_) => return None,
      }
    }
    None
  }

  pub async fn recv_until_closed(&mut self) -> (Vec<String>, Option<(u16, String)>) {
    let mut texts = Vec::new();
    while let Some(msg) = self.stream.next().await {
      match msg {
        Ok(Message::Text(text)) => texts.push(text.to_string()),
        Ok(Message::Close(frame)) => {
          return (
            texts,
            frame.map(|frame| (u16::from(frame.code), frame.reason.to_string())),
          );
        }
        Ok(_) => continue,
        Err(_) => break,
      }
    }
    (texts, None)
  }

  pub async fn send_text(&mut self, text: impl Into<String>) -> Result<()> {
    use futures::SinkExt;
    self.stream.send(Message::Text(text.into().into())).await?;
    Ok(())
  }
}

fn headless_config(state_dir: &Path, reset_rig: &Arc<FakeResetRig>) -> DaemonConfig {
  DaemonConfig {
    factory_reset: reset_rig.clone(),
    ..DaemonConfig::headless(state_dir.to_path_buf())
  }
}

enum StockBundle {
  None,
  Dist,
  Stub,
}

fn provision_stub_stock_bundle(state_dir: &Path) -> Result<()> {
  let stock = state_dir.join("builtin").join("stock");
  std::fs::create_dir_all(&stock)?;
  std::fs::write(stock.join("index.html"), "<!doctype html><title>stock</title>")?;
  let manifest = serde_json::json!({
    "id": bridgething_delivery::webapp::STOCK_WEBAPP_ID,
    "name": "Spotify",
    "version": "0.0.0",
    "role": "standard",
    "config": [],
    "permissions": [],
  });
  std::fs::write(stock.join("manifest.json"), manifest.to_string())?;
  Ok(())
}

fn provision_stock_bundle(state_dir: &Path) -> Result<()> {
  let dist = stock_dist_path()?;
  let builtin = state_dir.join("builtin");
  std::fs::create_dir_all(&builtin)?;
  std::os::unix::fs::symlink(&dist, builtin.join("stock"))?;
  Ok(())
}

fn stock_dist_path() -> Result<std::path::PathBuf> {
  let has_index = |p: &Path| p.join("index.html").is_file();
  if let Ok(env) = std::env::var("BRIDGETHING_STOCK_DIST") {
    let p = std::path::PathBuf::from(env);
    if has_index(&p) {
      return Ok(p);
    }
    anyhow::bail!("BRIDGETHING_STOCK_DIST set to {} but no index.html there", p.display());
  }
  let sibling = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../superbird-webapp/dist");
  let sibling = sibling.canonicalize().unwrap_or(sibling);
  if has_index(&sibling) {
    return Ok(sibling);
  }
  anyhow::bail!(
    "stock dist not found at {} (build superbird-webapp or set BRIDGETHING_STOCK_DIST)",
    sibling.display()
  )
}
