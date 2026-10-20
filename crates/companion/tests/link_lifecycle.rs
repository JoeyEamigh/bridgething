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
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use backends::{Heard, Offline, RigHost};
use bridgething_companion::{
  api::{CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo, SessionEvent},
  backend::{ExtensionConfigEntry, ExtensionHost, ExtensionHostInbox, ExtensionMessage, LinkDevice},
  session::Session,
};
use device_meta::meta;
use libbridgething::{
  gateway::{BridgeToGatewayMsg, BridgeToGatewayMsgData},
  protocol::{BridgeEndec, DecodedFrame, PrioritizedFrame},
  wire::{MsgMeta, ResponseMeta, WireError},
};
use log_sink::Quiet;
use poll::eventually;
use secrets::MemorySecrets;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_util::{
  bytes::BytesMut,
  codec::{Decoder, Encoder},
};
use uuid::Uuid;

const DEVICE: &str = "ws://link-lifecycle-device/";

fn session(extensions: Option<Arc<dyn ExtensionHost>>) -> (Arc<Session>, Arc<Heard>, tempfile::TempDir) {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let heard = Arc::new(Heard::default());
  let backends = CompanionBackends {
    link: None,
    host: Arc::new(RigHost),
    http: Arc::new(Offline),
    ws: Arc::new(Offline),
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

struct DirectDaemon(tokio::task::JoinHandle<()>);

impl Drop for DirectDaemon {
  fn drop(&mut self) {
    self.0.abort();
  }
}

async fn refuse_requests(mut io: DuplexStream) {
  let mut decoder = BridgeEndec::default();
  let mut inbound = BytesMut::new();
  loop {
    match io.read_buf(&mut inbound).await {
      Ok(0) | Err(_) => return,
      Ok(_) => {}
    }
    while let Ok(Some(DecodedFrame::Frame(frame))) = decoder.decode(&mut inbound) {
      if frame.msg.meta != MsgMeta::Request {
        continue;
      }
      let refusal = framed(BridgeToGatewayMsg {
        id: Uuid::now_v7(),
        meta: MsgMeta::Response(ResponseMeta {
          request_id: frame.msg.id,
        }),
        data: BridgeToGatewayMsgData::Error(WireError::Unsupported),
      });
      if io.write_all(&refusal).await.is_err() {
        return;
      }
    }
  }
}

async fn direct_link(session: &Arc<Session>, serial: &str) -> DirectDaemon {
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
  let daemon = DirectDaemon(tokio::spawn(refuse_requests(daemon_io)));
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
  let (session, _heard, _spool) = session(None);

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
  let (session, heard, _spool) = session(Some(host.clone() as Arc<dyn ExtensionHost>));

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
