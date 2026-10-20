#[path = "rig/backends.rs"]
mod backends;
#[path = "rig/device_meta.rs"]
mod device_meta;
#[path = "rig/heard.rs"]
mod heard;
#[path = "rig/log_sink.rs"]
mod log_sink;
#[path = "support/poll.rs"]
mod poll;
#[path = "rig/secrets.rs"]
mod secrets;

use std::{
  sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
  },
  time::Duration,
};

use backends::{Heard, Offline, RigHost};
use bridgething_companion::{
  api::{CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo, LogOrigin, SessionEvent},
  backend::{
    ExtensionConfigEntry, ExtensionHost, ExtensionHostInbox, ExtensionMessage, LinkDevice, WsInbox, WsTransport,
    net::{WsConnect, WsFrame},
  },
  session::Session,
};
use device_meta::meta;
use libbridgething::{
  LogEntry, LogLevel,
  gateway::{
    BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewaySystemMsg, GatewayToBridgeMsg, GatewayToBridgeMsgData,
    GatewayToBridgeSystemMsg, GatewayToBridgeWebappMsg, LogsSubscribeReply,
  },
  protocol::{BridgeEndec, DecodedFrame, PrioritizedFrame},
  wire::{MsgMeta, ResponseMeta, WireError},
};
use log_sink::Quiet;
use poll::eventually;
use secrets::MemorySecrets;
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
  sync::mpsc,
};
use tokio_util::{
  bytes::BytesMut,
  codec::{Decoder, Encoder},
};
use uuid::Uuid;

const DEVICE: &str = "ws://link-lifecycle-device/";

fn session(
  extensions: Option<Arc<dyn ExtensionHost>>,
  ws: Arc<dyn WsTransport>,
) -> (Arc<Session>, Arc<Heard>, tempfile::TempDir) {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let heard = Arc::new(Heard::default());
  let backends = CompanionBackends {
    link: None,
    host: Arc::new(RigHost),
    http: Arc::new(Offline),
    ws,
    secrets: Arc::new(MemorySecrets::default()),
    log: Arc::new(Quiet),
    audio: None,
    volume: None,
    geo: None,
    notifications: None,
    phone: None,
    media_sessions: None,
    stream: None,
    speech: None,
    nlu: None,
    apple_music: None,
    image: None,
    transcoder: None,
    model_validator: None,
    transfer_policy: None,
    connectivity: None,
    device_waker: None,
    extensions,
  };
  let session = Session::new(
    CompanionConfig {
      host: HostInfo {
        app_name: "link-lifecycle".into(),
        app_version: "0.0.0".into(),
        os_name: "linux".into(),
        os_version: "0".into(),
        host_identifier: "link-lifecycle".into(),
      },
      capabilities: CapabilityFlags {
        geo: false,
        notifications: false,
        net_fetch: false,
        net_ws: false,
        audio_tts: false,
        voice_model: false,
      },
      state_dir: spool.path().to_string_lossy().into_owned(),
      cache_dir: spool.path().to_string_lossy().into_owned(),
      model_platform: None,
      spotify: None,
    },
    backends,
    heard.clone(),
    Arc::new(Offline),
  );
  (session, heard, spool)
}

fn framed(msg: BridgeToGatewayMsg) -> BytesMut {
  let mut frame = BytesMut::new();
  BridgeEndec::default()
    .encode(PrioritizedFrame::normal(msg), &mut frame)
    .expect("the device frame encodes");
  frame
}

struct DirectDaemon {
  heard: Arc<Mutex<Vec<GatewayToBridgeMsg>>>,
  say: mpsc::UnboundedSender<BridgeToGatewayMsg>,
  io: tokio::task::JoinHandle<()>,
}

impl Drop for DirectDaemon {
  fn drop(&mut self) {
    self.io.abort();
  }
}

impl DirectDaemon {
  fn spawn(io: DuplexStream, holds: fn(&GatewayToBridgeMsgData) -> bool) -> Self {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let (say, said) = mpsc::unbounded_channel();
    let io = tokio::spawn(serve(io, heard.clone(), said, holds));
    Self { heard, say, io }
  }

  fn heard(&self, wants: impl Fn(&GatewayToBridgeMsgData) -> bool) -> Vec<Uuid> {
    self
      .heard
      .lock()
      .unwrap()
      .iter()
      .filter(|msg| wants(&msg.data))
      .map(|msg| msg.id)
      .collect()
  }

  async fn asked(&self, wants: impl Fn(&GatewayToBridgeMsgData) -> bool) -> Uuid {
    assert!(
      eventually(|| !self.heard(&wants).is_empty()).await,
      "the request reached the daemon"
    );
    self.heard(&wants)[0]
  }

  fn answer(&self, request_id: Uuid, data: BridgeToGatewayMsgData) {
    self.say(BridgeToGatewayMsg {
      id: Uuid::now_v7(),
      meta: MsgMeta::Response(ResponseMeta { request_id }),
      data,
    });
  }

  fn log(&self, message: &str) {
    self.say(BridgeToGatewayMsg {
      id: Uuid::now_v7(),
      meta: MsgMeta::Event,
      data: BridgeToGatewaySystemMsg::LogEntry(LogEntry {
        ts_unix_s: 1_700_000_000,
        level: LogLevel::Info,
        target: "daemon::net".into(),
        message: message.into(),
      })
      .into(),
    });
  }

  fn say(&self, msg: BridgeToGatewayMsg) {
    self.say.send(msg).expect("the daemon is still serving the link");
  }
}

fn holds_nothing(_: &GatewayToBridgeMsgData) -> bool {
  false
}

async fn serve(
  io: DuplexStream,
  heard: Arc<Mutex<Vec<GatewayToBridgeMsg>>>,
  mut said: mpsc::UnboundedReceiver<BridgeToGatewayMsg>,
  holds: fn(&GatewayToBridgeMsgData) -> bool,
) {
  let (mut reader, mut writer) = tokio::io::split(io);
  let mut decoder = BridgeEndec::default();
  let mut inbound = BytesMut::new();
  loop {
    tokio::select! {
      read = reader.read_buf(&mut inbound) => {
        if matches!(read, Ok(0) | Err(_)) {
          return;
        }
        while let Ok(Some(DecodedFrame::Frame(frame))) = decoder.decode(&mut inbound) {
          let refused = frame.msg.meta == MsgMeta::Request && !holds(&frame.msg.data);
          let request_id = frame.msg.id;
          heard.lock().unwrap().push(frame.msg);
          if !refused {
            continue;
          }
          let refusal = framed(BridgeToGatewayMsg {
            id: Uuid::now_v7(),
            meta: MsgMeta::Response(ResponseMeta { request_id }),
            data: BridgeToGatewayMsgData::Error(WireError::Unsupported),
          });
          if writer.write_all(&refusal).await.is_err() {
            return;
          }
        }
      }
      Some(msg) = said.recv() => {
        if writer.write_all(&framed(msg)).await.is_err() {
          return;
        }
      }
    }
  }
}

async fn direct_link(session: &Arc<Session>, serial: &str) -> DirectDaemon {
  direct_link_holding(session, serial, holds_nothing).await
}

async fn direct_link_holding(
  session: &Arc<Session>,
  serial: &str,
  holds: fn(&GatewayToBridgeMsgData) -> bool,
) -> DirectDaemon {
  let (companion_io, mut daemon_io) = tokio::io::duplex(64 * 1024);
  let announce = framed(BridgeToGatewayMsg {
    id: Uuid::now_v7(),
    meta: MsgMeta::Event,
    data: BridgeToGatewayMsgData::Version(Box::new(meta(serial))),
  });
  daemon_io
    .write_all(&announce)
    .await
    .expect("the daemon writes its version into the link before the companion reads it");
  let daemon = DirectDaemon::spawn(daemon_io, holds);
  session
    .connect_direct(
      LinkDevice {
        id: DEVICE.to_owned(),
        name: "direct".into(),
      },
      bridgething_gateway::transport::FramedConnector::new(companion_io),
    )
    .await;
  daemon
}

async fn planner_serial(session: &Arc<Session>) -> Option<String> {
  session.ota().meta(DEVICE).await.map(|meta| meta.serial_number)
}

async fn planner_holds(session: &Arc<Session>, serial: &str, within: Duration) -> bool {
  let deadline = tokio::time::Instant::now() + within;
  loop {
    if planner_serial(session).await.as_deref() == Some(serial) {
      return true;
    }
    if tokio::time::Instant::now() >= deadline {
      return false;
    }
    tokio::time::sleep(Duration::from_millis(1)).await;
  }
}

struct BusyCores {
  stop: Arc<AtomicBool>,
}

impl BusyCores {
  fn all() -> Self {
    let stop = Arc::new(AtomicBool::new(false));
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    for _ in 0..cores {
      let stop = stop.clone();
      std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
          std::hint::spin_loop();
        }
      });
    }
    Self { stop }
  }
}

impl Drop for BusyCores {
  fn drop(&mut self) {
    self.stop.store(true, Ordering::Relaxed);
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_the_device_sends_as_a_direct_link_opens_reaches_the_update_planner() {
  let _contended = BusyCores::all();
  let (session, _heard, _spool) = session(None, Arc::new(Offline));

  for link in 0..2_000 {
    let serial = format!("link-{link}");
    let daemon = direct_link(&session, &serial).await;
    assert!(
      planner_holds(&session, &serial, Duration::from_secs(5)).await,
      "link {link}: the daemon announces its version the instant the link opens, so the update planner has to hold \
       the link before routing starts or the device is never offered an update, nor told it is current, on that link"
    );
    session.direct_disconnected(DEVICE).await;
    drop(daemon);
  }
}

#[derive(Default)]
struct HoldingExtensionHost {
  holding: Mutex<bool>,
  released: Condvar,
  disconnects: Mutex<Vec<String>>,
}

impl HoldingExtensionHost {
  fn hold_disconnects(&self) {
    *self.holding.lock().unwrap() = true;
  }

  fn release_disconnects(&self) {
    *self.holding.lock().unwrap() = false;
    self.released.notify_all();
  }

  fn told_disconnected(&self, device_id: &str) -> bool {
    self
      .disconnects
      .lock()
      .unwrap()
      .iter()
      .any(|device| device == device_id)
  }
}

impl ExtensionHost for HoldingExtensionHost {
  fn start(&self, _inbox: Arc<ExtensionHostInbox>) {}
  fn stop(&self) {}
  fn deliver(&self, _device: String, _webapp: String, _message: ExtensionMessage) {}
  fn device_connected(
    &self,
    _device: String,
    _name: String,
    _config: Vec<ExtensionConfigEntry>,
    _webapps: Vec<String>,
  ) {
  }

  fn device_disconnected(&self, device: String) {
    self.disconnects.lock().unwrap().push(device);
    let mut holding = self.holding.lock().unwrap();
    while *holding {
      holding = self.released.wait(holding).unwrap();
    }
  }

  fn device_active(&self, _device: String, _webapp: String, _active: bool) {}
  fn config_changed(&self, _device: String, _webapp: String, _key: String, _value: Option<String>) {}
}

fn last_word_on(heard: &Heard, device_id: &str) -> Option<&'static str> {
  heard.events().iter().rev().find_map(|event| match event {
    SessionEvent::PeerConnected { peer } if peer.id == device_id => Some("connected"),
    SessionEvent::PeerDisconnected { device_id: gone } if gone == device_id => Some("disconnected"),
    _ => None,
  })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_dies_while_its_replacement_comes_up_leaves_the_replacement_whole() {
  let host = Arc::new(HoldingExtensionHost::default());
  let (session, heard, _spool) = session(Some(host.clone() as Arc<dyn ExtensionHost>), Arc::new(Offline));

  let first = direct_link(&session, "first").await;
  assert!(planner_holds(&session, "first", Duration::from_secs(5)).await);

  host.hold_disconnects();
  drop(first);
  assert!(
    eventually(|| host.told_disconnected(DEVICE)).await,
    "the dying link is released, and the host hears about it partway through"
  );

  let replacing = {
    let session = session.clone();
    tokio::spawn(async move { direct_link(&session, "second").await })
  };
  planner_holds(&session, "second", Duration::from_millis(500)).await;
  host.release_disconnects();
  let _second = replacing.await.expect("the replacement link comes up");

  assert!(
    eventually(|| heard
      .events()
      .iter()
      .any(|event| matches!(event, SessionEvent::PeerDisconnected { device_id } if device_id == DEVICE)))
    .await,
    "the first link's death is reported"
  );
  assert!(
    planner_holds(&session, "second", Duration::from_secs(5)).await,
    "releasing the dead link must not take the replacement's place with the update planner, saw {:?}",
    planner_serial(&session).await
  );
  assert!(
    eventually(|| last_word_on(&heard, DEVICE) == Some("connected")).await,
    "the host is left believing the device is connected, because it is, saw {:?}",
    last_word_on(&heard, DEVICE)
  );
}

fn is_get_active(data: &GatewayToBridgeMsgData) -> bool {
  matches!(
    data,
    GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::GetActive)
  )
}

fn is_webapp_list(data: &GatewayToBridgeMsgData) -> bool {
  matches!(data, GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::List))
}

fn is_log_subscribe(data: &GatewayToBridgeMsgData) -> bool {
  matches!(
    data,
    GatewayToBridgeMsgData::System(GatewayToBridgeSystemMsg::LogsSubscribe(_))
  )
}

fn is_log_tail(data: &GatewayToBridgeMsgData) -> bool {
  matches!(
    data,
    GatewayToBridgeMsgData::System(GatewayToBridgeSystemMsg::LogsTail(_))
  )
}

fn held_through_bring_up(data: &GatewayToBridgeMsgData) -> bool {
  is_get_active(data) || is_log_subscribe(data) || is_log_tail(data)
}

fn device_lines(heard: &Heard) -> Vec<String> {
  heard
    .events()
    .into_iter()
    .filter_map(|event| match event {
      SessionEvent::Log {
        origin: LogOrigin::Device,
        message,
        ..
      } => Some(message),
      _ => None,
    })
    .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn streaming_switched_on_while_a_link_is_still_coming_up_opens_one_tap_and_shows_every_line() {
  let host = Arc::new(HoldingExtensionHost::default());
  let (session, heard, _spool) = session(Some(host as Arc<dyn ExtensionHost>), Arc::new(Offline));
  let daemon = direct_link_holding(&session, "streaming", held_through_bring_up).await;
  let active = daemon.asked(is_get_active).await;

  let toggling = tokio::spawn({
    let session = session.clone();
    async move { session.set_device_log_streaming(true).await }
  });
  let subscribe = daemon.asked(is_log_subscribe).await;
  daemon.answer(active, BridgeToGatewayMsgData::Error(WireError::Unsupported));
  assert!(
    eventually(|| !daemon.heard(is_webapp_list).is_empty()).await,
    "the link finishes coming up beside the tap the toggle is opening instead of asking for a second one, saw {} \
     subscribes",
    daemon.heard(is_log_subscribe).len()
  );

  daemon.answer(
    subscribe,
    BridgeToGatewaySystemMsg::LogsSubscribeReply(LogsSubscribeReply { token: "tap-1".into() }).into(),
  );
  daemon.log("spoken while the history was in flight");
  let tail = daemon.asked(is_log_tail).await;
  daemon.answer(tail, BridgeToGatewayMsgData::Error(WireError::Unsupported));
  toggling.await.expect("the toggle settled");
  daemon.log("spoken once the stream was open");

  let want = vec![
    "spoken while the history was in flight".to_owned(),
    "spoken once the stream was open".to_owned(),
  ];
  assert!(
    eventually(|| device_lines(&heard) == want).await,
    "every line reaches the app once the one tap is open, got {:?}",
    device_lines(&heard)
  );
  assert_eq!(
    daemon.heard(is_log_subscribe).len(),
    1,
    "a device streams through one tap, or every line it speaks arrives once per tap"
  );
}

const NETWORK_URL: &str = "ws://link-lifecycle-daemon:8892/";

#[derive(Default)]
struct DaemonSockets {
  serving: Mutex<Option<String>>,
  open: Mutex<Option<(String, Arc<WsInbox>)>>,
  dials: AtomicUsize,
}

impl DaemonSockets {
  fn serve(&self, serial: &str) {
    *self.serving.lock().unwrap() = Some(serial.to_owned());
  }

  fn refuse(&self) {
    *self.serving.lock().unwrap() = None;
  }

  fn hang_up(&self) {
    let open = self.open.lock().unwrap().take();
    if let Some((id, inbox)) = open {
      inbox.on_closed(id, None, "the daemon went away".into());
    }
  }

  fn dials(&self) -> usize {
    self.dials.load(Ordering::SeqCst)
  }
}

impl WsTransport for DaemonSockets {
  fn connect(&self, connect: WsConnect, inbox: Arc<WsInbox>) {
    self.dials.fetch_add(1, Ordering::SeqCst);
    let serving = self.serving.lock().unwrap().clone();
    let Some(serial) = serving else {
      inbox.on_closed(connect.id, None, "connection refused".into());
      return;
    };
    inbox.on_open(connect.id.clone(), None);
    let announce = framed(BridgeToGatewayMsg {
      id: Uuid::now_v7(),
      meta: MsgMeta::Event,
      data: BridgeToGatewayMsgData::Version(Box::new(meta(&serial))),
    });
    inbox.on_binary(connect.id.clone(), announce.to_vec());
    *self.open.lock().unwrap() = Some((connect.id, inbox));
  }

  fn send(&self, _id: String, _frame: WsFrame) {}

  fn disconnect(&self, id: String, _code: Option<u16>, _reason: Option<String>) {
    let mut open = self.open.lock().unwrap();
    if open.as_ref().is_some_and(|(held, _)| *held == id) {
      *open = None;
    }
  }
}

fn network_device() -> LinkDevice {
  LinkDevice {
    id: DEVICE.to_owned(),
    name: "network".into(),
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_link_the_daemon_drops_comes_back_on_its_own_with_the_version_the_daemon_returns_with() {
  let sockets = Arc::new(DaemonSockets::default());
  sockets.serve("before");
  let (session, heard, _spool) = session(None, sockets.clone());
  session
    .connect_network(NETWORK_URL.into(), network_device())
    .await
    .expect("the daemon answers");
  assert!(planner_holds(&session, "before", Duration::from_secs(5)).await);

  sockets.refuse();
  sockets.hang_up();
  assert!(
    eventually(|| sockets.dials() >= 3).await,
    "a daemon that went away is dialed again until it answers, like a bluetooth link is, saw {} dials",
    sockets.dials()
  );

  sockets.serve("after");
  assert!(
    planner_holds(&session, "after", Duration::from_secs(10)).await,
    "the daemon that came back is linked again and its new version reaches the update planner, saw {:?}",
    planner_serial(&session).await
  );
  assert!(
    eventually(|| last_word_on(&heard, DEVICE) == Some("connected")).await,
    "and the host hears the device is connected again"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_link_the_host_lets_go_of_is_not_dialed_again() {
  let sockets = Arc::new(DaemonSockets::default());
  sockets.serve("only");
  let (session, _heard, _spool) = session(None, sockets.clone());
  session
    .connect_network(NETWORK_URL.into(), network_device())
    .await
    .expect("the daemon answers");
  assert!(planner_holds(&session, "only", Duration::from_secs(5)).await);

  sockets.refuse();
  sockets.hang_up();
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_none()).await,
    "the link died"
  );
  session.direct_disconnected(DEVICE).await;
  let dialed = sockets.dials();
  sockets.serve("only");
  tokio::time::sleep(Duration::from_secs(4)).await;

  assert_eq!(
    sockets.dials(),
    dialed,
    "a link the host disconnected stays disconnected"
  );
  assert!(session.gateway_for(DEVICE).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_link_whose_daemon_is_not_up_yet_comes_up_once_it_is() {
  let sockets = Arc::new(DaemonSockets::default());
  let (session, _heard, _spool) = session(None, sockets.clone());

  let first = session.connect_network(NETWORK_URL.into(), network_device()).await;
  assert!(first.is_err(), "the first dial reports that nothing answered");

  sockets.serve("late");
  assert!(
    planner_holds(&session, "late", Duration::from_secs(10)).await,
    "a daemon that starts after the app asked for it is linked as soon as it answers"
  );
}
