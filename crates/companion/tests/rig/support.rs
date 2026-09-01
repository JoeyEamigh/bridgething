use std::{
  path::PathBuf,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use bridgething_companion::{
  api::{CapabilityFlags, CompanionBackends, CompanionConfig, CompanionSession, HostInfo, SessionEvent},
  backend::{ExtensionHost, HttpTransport, LinkDevice, LinkInbox, LinkTransport, VolumeBackend},
  session::Session,
};
use bridgething_gateway::Gateway;
use bridgething_test_harness::Harness;
use libbridgething::{
  GeoAccuracy, HttpHeader, HttpMethod, ItemKind, ItemRef, NetFetchRequest, OtaPhase, Priority, RangeSpec,
  RedirectPolicy,
  gateway::{
    AssetRequest, BridgeToGatewayAssetMsg, BridgeToGatewayGeoMsg, BridgeToGatewayLibraryMsg, BridgeToGatewayLyricsMsg,
    BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewayNetMsg, BridgeToGatewayPhoneMsg,
    BridgeToGatewaySystemMsg, BridgeToGatewayTransferMsg, BridgeToGatewayTunnelMsg, GeoGetOnce, KeepalivePing,
    LibraryBrowseRequest, LibraryFavoritesContainsRequest, LibraryFavoritesListRequest, LibraryRecommendationsRequest,
    LibraryResolveContextRequest, LibrarySearchRequest, LyricsRequest, NetFetchRequestMsg, NetWsOpen, OtaAssetRange,
    TrackIdentity, TunnelOpen,
  },
  protocol::{BridgeEndec, DecodedFrame, GatewayEndec},
  wire::MsgMeta,
};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::{bytes::BytesMut, codec::Decoder};
use uuid::Uuid;

use crate::{
  backends::{Heard, Offline, RigHost},
  log_sink::Quiet,
  secrets::MemorySecrets,
};

pub const DEVICE: &str = "rig-device";
pub const TO_DEVICE: &str = "toDevice";
pub const TO_HOST: &str = "toHost";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireEntry {
  #[serde(skip_serializing)]
  pub dir: String,
  pub lane: String,
  pub msg: String,
  pub meta: String,
  pub id: String,
}

fn surface_variant<T: Serialize>(data: &T) -> String {
  let value = serde_json::to_value(data).expect("wire data serializes");
  let surface = value
    .get("type")
    .and_then(serde_json::Value::as_str)
    .unwrap_or("unknown");
  match value
    .get("data")
    .and_then(|inner| inner.get("event"))
    .and_then(serde_json::Value::as_str)
  {
    Some(variant) => format!("{surface}.{variant}"),
    None => surface.to_owned(),
  }
}

fn lane_name(priority: Priority) -> &'static str {
  match priority {
    Priority::Normal => "normal",
    Priority::Bulk => "bulk",
    Priority::Background => "background",
  }
}

fn sequence(ids: &mut Vec<Uuid>, id: Uuid) -> String {
  let at = ids.iter().position(|seen| *seen == id).unwrap_or_else(|| {
    ids.push(id);
    ids.len() - 1
  });
  format!("#{at}")
}

fn note<T: Serialize>(
  ids: &mut Vec<Uuid>,
  dir: &str,
  priority: Priority,
  meta: &MsgMeta,
  id: Uuid,
  data: &T,
) -> WireEntry {
  let (kind, identity) = match meta {
    MsgMeta::Command => ("command", None),
    MsgMeta::Event => ("event", None),
    MsgMeta::Request => ("request", Some(id)),
    MsgMeta::Response(response) => ("response", Some(response.request_id)),
  };
  WireEntry {
    dir: dir.to_owned(),
    lane: lane_name(priority).to_owned(),
    msg: surface_variant(data),
    meta: kind.to_owned(),
    id: identity.map_or_else(|| "-".to_owned(), |id| sequence(ids, id)),
  }
}

#[derive(Default)]
struct TapState {
  from_host: BridgeEndec,
  from_host_buf: BytesMut,
  from_device: GatewayEndec,
  from_device_buf: BytesMut,
  ids: Vec<Uuid>,
  entries: Vec<WireEntry>,
  said: DeviceSaid,
}

#[derive(Default)]
struct Tap {
  recording: AtomicBool,
  state: Mutex<TapState>,
}

impl Tap {
  fn to_device(&self, batch: &[u8]) {
    if !self.recording.load(Ordering::Relaxed) {
      return;
    }
    let mut held = self.state.lock().unwrap();
    let state = &mut *held;
    state.from_host_buf.extend_from_slice(batch);
    loop {
      match state.from_host.decode(&mut state.from_host_buf) {
        Ok(Some(DecodedFrame::Frame(frame))) => {
          let entry = note(
            &mut state.ids,
            TO_DEVICE,
            frame.priority,
            &frame.msg.meta,
            frame.msg.id,
            &frame.msg.data,
          );
          state.entries.push(entry);
        }
        Ok(None) => return,
        Ok(Some(DecodedFrame::Failed(error))) | Err(error) => {
          state.entries.push(undecodable(TO_DEVICE, &error));
          return;
        }
      }
    }
  }

  fn to_host(&self, bytes: &[u8]) {
    let recording = self.recording.load(Ordering::Relaxed);
    let mut held = self.state.lock().unwrap();
    let state = &mut *held;
    state.from_device_buf.extend_from_slice(bytes);
    loop {
      match state.from_device.decode(&mut state.from_device_buf) {
        Ok(Some(DecodedFrame::Frame(frame))) => {
          match &frame.msg.data {
            BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Ack(ack)) => {
              state.said.acked.push(ack.received)
            }
            BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaProgress(tick)) => {
              state.said.phases.push(tick.phase)
            }
            _ => {}
          }
          if recording {
            let entry = note(
              &mut state.ids,
              TO_HOST,
              frame.priority,
              &frame.msg.meta,
              frame.msg.id,
              &frame.msg.data,
            );
            state.entries.push(entry);
          }
        }
        Ok(None) => return,
        Ok(Some(DecodedFrame::Failed(error))) | Err(error) => {
          if recording {
            state.entries.push(undecodable(TO_HOST, &error));
          }
          return;
        }
      }
    }
  }
}

fn undecodable(dir: &str, error: &impl std::fmt::Display) -> WireEntry {
  WireEntry {
    dir: dir.to_owned(),
    lane: "-".to_owned(),
    msg: format!("undecodable: {error}"),
    meta: "-".to_owned(),
    id: "-".to_owned(),
  }
}

struct HarnessLink {
  writes: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>>,
  io: Mutex<Option<tokio::io::DuplexStream>>,
  tap: Arc<Tap>,
}

impl HarnessLink {
  fn new(io: tokio::io::DuplexStream, tap: Arc<Tap>) -> Arc<Self> {
    Arc::new(Self {
      writes: Mutex::new(None),
      io: Mutex::new(Some(io)),
      tap,
    })
  }
}

impl LinkTransport for HarnessLink {
  fn max_batch_bytes(&self) -> u32 {
    16 * 1024
  }

  fn start(&self, inbox: Arc<LinkInbox>) {
    let Some(io) = self.io.lock().unwrap().take() else {
      return;
    };
    let (mut read, mut write) = tokio::io::split(io);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    *self.writes.lock().unwrap() = Some(tx);

    let reporting = inbox.clone();
    tokio::spawn(async move {
      while let Some(batch) = rx.recv().await {
        if write.write_all(&batch).await.is_err() {
          return;
        }
        let _ = write.flush().await;
        reporting.on_write_complete(DEVICE.into());
      }
    });

    let reading = inbox.clone();
    let tap = self.tap.clone();
    tokio::spawn(async move {
      let mut buf = vec![0u8; 64 * 1024];
      loop {
        match read.read(&mut buf).await {
          Ok(0) | Err(_) => {
            reading.on_disconnected(DEVICE.into());
            return;
          }
          Ok(read) => {
            tap.to_host(&buf[..read]);
            reading.on_bytes(DEVICE.into(), buf[..read].to_vec());
          }
        }
      }
    });

    inbox.on_connected(LinkDevice {
      id: DEVICE.into(),
      name: "harness".into(),
    });
  }

  fn stop(&self) {
    *self.writes.lock().unwrap() = None;
  }

  fn send(&self, _device_id: String, batch: Vec<u8>) {
    self.tap.to_device(&batch);
    if let Some(writes) = self.writes.lock().unwrap().as_ref() {
      let _ = writes.send(batch);
    }
  }

  fn disconnect(&self, _device_id: String) {}
  fn reconnect(&self, _device_id: String) {}
}

#[derive(Clone, Default)]
pub struct DeviceSaid {
  pub acked: Vec<u32>,
  pub phases: Vec<OtaPhase>,
}

#[derive(Default)]
pub struct Setup {
  pub recording: bool,
  pub extensions: Option<Arc<dyn ExtensionHost>>,
  pub http: Option<Arc<dyn HttpTransport>>,
  pub volume: Option<Arc<dyn VolumeBackend>>,
}

pub struct Rig {
  pub harness: Harness,
  pub companion: Arc<CompanionSession>,
  pub session: Arc<Session>,
  pub heard: Arc<Heard>,
  tap: Arc<Tap>,
  spool: tempfile::TempDir,
}

impl Rig {
  pub async fn start() -> Self {
    Self::launch(Setup::default()).await
  }

  pub async fn recording() -> Self {
    Self::launch(Setup {
      recording: true,
      ..Setup::default()
    })
    .await
  }

  pub async fn with_extension_host(host: Arc<dyn ExtensionHost>) -> Self {
    Self::launch(Setup {
      extensions: Some(host),
      ..Setup::default()
    })
    .await
  }

  // a host that can move its own output volume, recording so the emitted frames are inspectable
  pub async fn with_volume(volume: Arc<dyn VolumeBackend>) -> Self {
    Self::launch(Setup {
      recording: true,
      volume: Some(volume),
      ..Setup::default()
    })
    .await
  }

  pub async fn with_http(http: Arc<dyn HttpTransport>) -> Self {
    Self::launch(Setup {
      http: Some(http),
      ..Setup::default()
    })
    .await
  }

  async fn launch(setup: Setup) -> Self {
    let harness = Harness::start().await.expect("the headless daemon boots");
    let io = harness.connect_android_io().await.expect("a link to the daemon");
    let spool = tempfile::tempdir().expect("a scratch directory");
    let heard = Arc::new(Heard::default());
    let tap = Arc::new(Tap::default());
    tap.recording.store(setup.recording, Ordering::Relaxed);

    let backends = CompanionBackends {
      link: Some(HarnessLink::new(io, tap.clone())),
      host: Arc::new(RigHost),
      http: setup.http.unwrap_or_else(|| Arc::new(Offline)),
      ws: Arc::new(Offline),
      secrets: Arc::new(MemorySecrets::default()),
      log: Arc::new(Quiet),
      audio: None,
      volume: setup.volume,
      geo: None,
      notifications: None,
      phone: None,
      media_sessions: None,
      speech: None,
      nlu: None,
      apple_music: None,
      image: None,
      model_validator: None,
      transfer_policy: None,
      connectivity: None,
      device_waker: None,
      extensions: setup.extensions,
    };

    let companion = CompanionSession::create(
      CompanionConfig {
        host: HostInfo {
          app_name: "rig".into(),
          app_version: "0.0.0".into(),
          os_name: "linux".into(),
          os_version: "0".into(),
          host_identifier: "rig".into(),
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
    );
    let session = companion.session().clone();
    session.start();

    let rig = Rig {
      harness,
      companion,
      session,
      heard,
      tap,
      spool,
    };
    rig.settle().await;
    rig
  }

  pub fn device_id(&self) -> &str {
    DEVICE
  }

  pub fn gateway(&self) -> Gateway {
    self.session.gateway_for(DEVICE).expect("the link is up")
  }

  pub async fn settle(&self) {
    for _ in 0..400 {
      if self.session.gateway_for(DEVICE).is_some() {
        tokio::time::sleep(Duration::from_millis(20)).await;
        return;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the session never adopted the harness link");
  }

  pub fn said(&self) -> DeviceSaid {
    self.tap.state.lock().unwrap().said.clone()
  }

  pub fn transcript(&self) -> Vec<WireEntry> {
    self.tap.state.lock().unwrap().entries.clone()
  }

  pub async fn await_event(&self, within: Duration, mut matches: impl FnMut(&SessionEvent) -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
      if self.heard.0.lock().unwrap().iter().any(&mut matches) {
        return true;
      }
      if tokio::time::Instant::now() >= deadline {
        return false;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
  }

  pub fn replies(&self) -> usize {
    self
      .tap
      .state
      .lock()
      .unwrap()
      .entries
      .iter()
      .filter(|entry| entry.dir == TO_DEVICE && entry.meta == "response")
      .count()
  }

  pub async fn await_replies(&self, within: Duration, count: usize) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
      if self.replies() >= count {
        return true;
      }
      if tokio::time::Instant::now() >= deadline {
        return false;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  }

  pub fn write_artifact(&self, name: &str, len: usize) -> PathBuf {
    let path = self.spool.path().join(name);
    let body: Vec<u8> = (0..len).map(|at| (at % 251) as u8).collect();
    std::fs::write(&path, body).expect("the artifact spools");
    path
  }
}

pub struct Probe {
  pub name: &'static str,
  pub msg: BridgeToGatewayMsg,
}

fn probe(name: &'static str, data: BridgeToGatewayMsgData) -> Probe {
  Probe {
    name,
    msg: BridgeToGatewayMsg {
      id: Uuid::now_v7(),
      meta: MsgMeta::Request,
      data,
    },
  }
}

pub fn probes() -> Vec<Probe> {
  let track = TrackIdentity {
    track: "a".into(),
    artist: "b".into(),
    album: None,
    duration_ms: None,
    isrc: None,
  };
  vec![
    probe(
      "asset.request",
      BridgeToGatewayAssetMsg::Request(AssetRequest {
        id: "rig:asset".into(),
        request_id: Uuid::now_v7(),
      })
      .into(),
    ),
    probe(
      "system.keepalive",
      BridgeToGatewaySystemMsg::Keepalive(KeepalivePing { seq: 1 }).into(),
    ),
    probe(
      "system.otaAssetRange",
      BridgeToGatewaySystemMsg::OtaAssetRange(OtaAssetRange {
        update_id: "rig".into(),
        asset: "system.img.zck".into(),
        ranges: vec![RangeSpec { start: 0, length: 16 }],
      })
      .into(),
    ),
    probe(
      "library.browse",
      BridgeToGatewayLibraryMsg::Browse(LibraryBrowseRequest {
        node_id: None,
        limit: 10,
        offset: 0,
        sections: None,
        preview: None,
      })
      .into(),
    ),
    probe(
      "library.search",
      BridgeToGatewayLibraryMsg::Search(LibrarySearchRequest {
        query: "anything".into(),
        kinds: None,
        limit: 10,
        offset: 0,
      })
      .into(),
    ),
    probe(
      "library.resolveContext",
      BridgeToGatewayLibraryMsg::ResolveContext(LibraryResolveContextRequest {
        uri: "rig:context:anything".into(),
      })
      .into(),
    ),
    probe(
      "library.recommendations",
      BridgeToGatewayLibraryMsg::Recommendations(LibraryRecommendationsRequest {
        seeds: vec![ItemRef {
          uri: "rig:track:anything".into(),
          kind: ItemKind::Track,
          persistent_id: None,
        }],
        kind: None,
        limit: 10,
        offset: 0,
      })
      .into(),
    ),
    probe(
      "library.favoritesList",
      BridgeToGatewayLibraryMsg::FavoritesList(LibraryFavoritesListRequest { limit: 10, offset: 0 }).into(),
    ),
    probe(
      "library.favoritesContains",
      BridgeToGatewayLibraryMsg::FavoritesContains(LibraryFavoritesContainsRequest {
        uris: vec!["rig:track:anything".into()],
      })
      .into(),
    ),
    probe(
      "lyrics.get",
      BridgeToGatewayLyricsMsg::Get(LyricsRequest { track }).into(),
    ),
    probe(
      "geo.getOnce",
      BridgeToGatewayGeoMsg::GetOnce(GeoGetOnce {
        accuracy: GeoAccuracy::Coarse,
      })
      .into(),
    ),
    probe("phone.stateGet", BridgeToGatewayPhoneMsg::StateGet.into()),
    probe(
      "net.fetch",
      BridgeToGatewayNetMsg::Fetch(NetFetchRequestMsg {
        request: NetFetchRequest {
          url: "http://127.0.0.1:1/".into(),
          method: HttpMethod::Get,
          headers: vec![],
          body: None,
          timeout_ms: Some(500),
          redirect: RedirectPolicy::Follow,
        },
      })
      .into(),
    ),
    probe(
      "net.wsOpen",
      BridgeToGatewayNetMsg::WsOpen(NetWsOpen {
        connection_id: Uuid::now_v7(),
        url: "ws://127.0.0.1:1/".into(),
        protocols: None,
        headers: Some(Vec::<HttpHeader>::new()),
      })
      .into(),
    ),
    probe(
      "tunnel.open",
      BridgeToGatewayTunnelMsg::Open(TunnelOpen {
        tunnel_id: Uuid::now_v7(),
        host: "127.0.0.1".into(),
        port: 1,
      })
      .into(),
    ),
  ]
}
