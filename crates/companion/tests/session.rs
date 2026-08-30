#[path = "rig/backends.rs"]
mod backends;
#[path = "rig/fakes.rs"]
mod fakes;
#[path = "rig/fakes_probe.rs"]
mod fakes_probe;
#[path = "rig/heard.rs"]
mod heard;
#[path = "rig/log_sink.rs"]
mod log_sink;
#[path = "rig/media.rs"]
mod media;
#[path = "support/poll.rs"]
mod poll;
#[path = "rig/secrets.rs"]
mod secrets;
#[path = "voicekit/mod.rs"]
mod voicekit;

use std::{
  collections::HashMap,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use backends::{Heard, Offline, RigHost};
use bridgething_companion::{
  api::{
    AncsAuthStatus, AuthKind, CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo, LogOrigin, PeerLinkStatus,
    ProviderTokens, SessionEvent, SessionPeer, SpotifyProviderConfig,
  },
  backend::{
    AmActionSink, AmAuthSink, AmAuthStatus, AmCatalogSink, AmFavoritesSink, AmFlagSink, AmItemSink, AmLibraryScope,
    AmPage, AmPageSink, AmPlayerCommand, AmPlayerInbox, AmPlayerSnapshot, AmRepeatMode, AmSearchResults, AmSearchSink,
    AmShelvesSink, AmSnapshotSink, AppleMusicBackend, ConnectivityInbox, ConnectivityMonitor, LinkDevice, LinkInbox,
    LinkTransport, MediaSessionBackend, PrepareSink, SecretStore, SpeechRecognizer, Transcription, TranscriptionSink,
  },
  provider::{Provider, ProviderAuthState},
  session::Session,
  voice::{
    controller::ArmedModel,
    inference::{InferError, InferenceOutput, NluInference},
    intent_catalog,
  },
};
use bridgething_gateway::SystemHandler;
use fakes::FakeSource;
use libbridgething::{
  AncsAuthState, LogEntry, NluSlots, NluTargetType, VoiceCaptureReason,
  gateway::{
    BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewaySystemMsg, BridgeToGatewayVoiceMsg, GatewayToBridgeMsg,
    GatewayToBridgeMsgData, GatewayToBridgeSystemMsg, GatewayToBridgeVoiceMsg, LogsSubscribeReply, LogsTailReply,
    VoiceCloseReason, VoiceDispatch, VoiceFrame, VoiceStreamClose, VoiceStreamOpen,
  },
  protocol::{BridgeEndec, DecodedFrame, PrioritizedFrame},
  wire::{MsgMeta, ResponseMeta, WireError},
};
use log_sink::Quiet;
use media::{FakeMediaBackend, playing};
use poll::eventually;
use secrets::MemorySecrets;
use tokio_util::{
  bytes::BytesMut,
  codec::{Decoder, Encoder},
};
use uuid::Uuid;

const DEVICE: &str = "session-device";
const OTHER: &str = "session-device-two";
const DIRECT: &str = "ws://session-device-direct/";

#[derive(Default)]
struct HandLink {
  inbox: Mutex<Option<Arc<LinkInbox>>>,
  wrote: Mutex<HashMap<String, BytesMut>>,
  drops_batches: AtomicBool,
}

impl HandLink {
  fn fail_sends(&self) {
    self.drops_batches.store(true, Ordering::SeqCst);
  }

  async fn connect(&self, device_id: &str) {
    assert!(
      eventually(|| self.inbox.lock().unwrap().is_some()).await,
      "the session started the transport"
    );
    let held = self.inbox.lock().unwrap().clone();
    if let Some(inbox) = held {
      inbox.on_connected(LinkDevice {
        id: device_id.to_owned(),
        name: "hand".into(),
      });
    }
  }

  fn drop_link(&self, device_id: &str) {
    let held = self.inbox.lock().unwrap().clone();
    if let Some(inbox) = held {
      inbox.on_disconnected(device_id.to_owned());
    }
  }

  fn say(&self, device_id: &str, msg: BridgeToGatewayMsg) {
    let mut bytes = BytesMut::new();
    BridgeEndec::default()
      .encode(PrioritizedFrame::normal(msg), &mut bytes)
      .expect("the device frame encodes");
    let held = self.inbox.lock().unwrap().clone();
    if let Some(inbox) = held {
      inbox.on_bytes(device_id.to_owned(), bytes.to_vec());
    }
  }

  fn heard_by(&self, device_id: &str) -> Vec<GatewayToBridgeMsg> {
    let mut buffer = self.wrote.lock().unwrap().get(device_id).cloned().unwrap_or_default();
    let mut decoder = BridgeEndec::default();
    let mut msgs = Vec::new();
    while let Ok(Some(DecodedFrame::Frame(frame))) = decoder.decode(&mut buffer) {
      msgs.push(frame.msg);
    }
    msgs
  }
}

impl LinkTransport for HandLink {
  fn max_batch_bytes(&self) -> u32 {
    16 * 1024
  }

  fn start(&self, inbox: Arc<LinkInbox>) {
    *self.inbox.lock().unwrap() = Some(inbox);
  }

  fn stop(&self) {}

  fn send(&self, device_id: String, batch: Vec<u8>) {
    if self.drops_batches.load(Ordering::SeqCst) {
      let held = self.inbox.lock().unwrap().clone();
      if let Some(inbox) = held {
        inbox.on_send_failed(device_id);
      }
      return;
    }
    self
      .wrote
      .lock()
      .unwrap()
      .entry(device_id.clone())
      .or_default()
      .extend_from_slice(&batch);
    let held = self.inbox.lock().unwrap().clone();
    if let Some(inbox) = held {
      inbox.on_write_complete(device_id);
    }
  }

  fn disconnect(&self, _device_id: String) {}
  fn reconnect(&self, _device_id: String) {}
}

fn keepalive(seq: u32) -> BridgeToGatewayMsg {
  BridgeToGatewayMsg {
    id: Uuid::now_v7(),
    meta: MsgMeta::Request,
    data: libbridgething::gateway::BridgeToGatewaySystemMsg::Keepalive(libbridgething::gateway::KeepalivePing { seq })
      .into(),
  }
}

fn keepalive_acks(msgs: &[GatewayToBridgeMsg]) -> Vec<u32> {
  msgs
    .iter()
    .filter_map(|msg| match &msg.data {
      libbridgething::gateway::GatewayToBridgeMsgData::System(
        libbridgething::gateway::GatewayToBridgeSystemMsg::KeepaliveAck(ack),
      ) => Some(ack.seq),
      _ => None,
    })
    .collect()
}

fn player_snapshots(msgs: &[GatewayToBridgeMsg]) -> usize {
  msgs
    .iter()
    .filter(|msg| {
      matches!(
        &msg.data,
        libbridgething::gateway::GatewayToBridgeMsgData::Player(
          libbridgething::gateway::GatewayToBridgePlayerMsg::Snapshot(_)
        )
      )
    })
    .count()
}

fn session(link: Arc<HandLink>) -> (Arc<Session>, Arc<Heard>, tempfile::TempDir) {
  session_full(link, Arc::new(MemorySecrets::default()), false, None, None, None)
}

fn session_hearing(
  link: Arc<HandLink>,
  speech: Arc<dyn SpeechRecognizer>,
) -> (Arc<Session>, Arc<Heard>, tempfile::TempDir) {
  session_full(
    link,
    Arc::new(MemorySecrets::default()),
    false,
    None,
    Some(speech),
    None,
  )
}

fn session_full(
  link: Arc<HandLink>,
  secrets: Arc<MemorySecrets>,
  spotify: bool,
  apple_music: Option<Arc<dyn AppleMusicBackend>>,
  speech: Option<Arc<dyn SpeechRecognizer>>,
  media_sessions: Option<Arc<dyn MediaSessionBackend>>,
) -> (Arc<Session>, Arc<Heard>, tempfile::TempDir) {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let heard = Arc::new(Heard::default());
  let backends = CompanionBackends {
    link: Some(link),
    host: Arc::new(RigHost),
    http: Arc::new(Offline),
    ws: Arc::new(Offline),
    secrets,
    log: Arc::new(Quiet),
    audio: None,
    volume: None,
    geo: None,
    notifications: None,
    phone: None,
    media_sessions,
    speech,
    nlu: None,
    apple_music,
    image: None,
    model_validator: None,
    transfer_policy: None,
    connectivity: None,
    device_waker: None,
    extensions: None,
  };
  let session = Session::new(
    CompanionConfig {
      host: HostInfo {
        app_name: "session-test".into(),
        app_version: "0.0.0".into(),
        os_name: "linux".into(),
        os_version: "0".into(),
        host_identifier: "session-test".into(),
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
      spotify: spotify.then(|| SpotifyProviderConfig {
        worker_base: "https://worker.invalid/auth".into(),
        psk: "session-test-psk".into(),
      }),
    },
    backends,
    heard.clone(),
    Arc::new(Offline),
  );
  (session, heard, spool)
}

struct SaidAloud(String);

impl SpeechRecognizer for SaidAloud {
  fn prepare(&self, sink: Arc<PrepareSink>) {
    sink.on_ready();
  }

  fn transcribe(&self, _pcm: Vec<f32>, _sample_rate_hz: u32, sink: Arc<TranscriptionSink>) {
    sink.complete(Transcription {
      text: self.0.clone(),
      alternatives: Vec::new(),
      segments: Vec::new(),
      confidence: None,
    });
  }
}

struct PlayNaming(String);

#[async_trait::async_trait]
impl NluInference for PlayNaming {
  async fn prewarm(&self) {}

  async fn infer(&self, _transcript: &str) -> Result<InferenceOutput, InferError> {
    let mut intent_logits = vec![0.0; intent_catalog::SURFACE_NAMES.len()];
    let at = intent_catalog::SURFACE_NAMES
      .iter()
      .position(|name| *name == "PLAY")
      .expect("the catalog carries PLAY");
    intent_logits[at] = 9.0;
    Ok(InferenceOutput {
      intent_logits,
      in_domain_logit: 8.0,
      slots: NluSlots {
        target: Some(self.0.clone()),
        target_type: Some(NluTargetType::Album),
        ..NluSlots::default()
      },
    })
  }
}

fn voice_turn(stream_id: Uuid) -> Vec<BridgeToGatewayMsg> {
  let framed = |data: BridgeToGatewayMsgData| BridgeToGatewayMsg {
    id: Uuid::now_v7(),
    meta: MsgMeta::Event,
    data,
  };
  let mut said = vec![framed(BridgeToGatewayMsgData::Voice(
    BridgeToGatewayVoiceMsg::StreamOpen(VoiceStreamOpen {
      stream_id,
      format: voicekit::format(),
      reason: VoiceCaptureReason::WakeWord,
    }),
  ))];
  for (seq, packet) in voicekit::packets().into_iter().enumerate() {
    said.push(framed(BridgeToGatewayMsgData::Voice(BridgeToGatewayVoiceMsg::Frame(
      VoiceFrame {
        stream_id,
        seq: seq as u32,
        packet,
      },
    ))));
  }
  said.push(framed(BridgeToGatewayMsgData::Voice(
    BridgeToGatewayVoiceMsg::StreamClose(VoiceStreamClose {
      stream_id,
      reason: VoiceCloseReason::EndOfSpeech,
    }),
  )));
  said
}

fn dispatched(msgs: &[GatewayToBridgeMsg]) -> Option<VoiceDispatch> {
  msgs.iter().find_map(|msg| match &msg.data {
    GatewayToBridgeMsgData::Voice(GatewayToBridgeVoiceMsg::Dispatch(dispatch)) => Some(dispatch.clone()),
    _ => None,
  })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_turn_through_a_live_session_carries_the_uri_the_provider_resolved() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) = session_hearing(link.clone(), Arc::new(SaidAloud("play the strokes".into())));
  session.voice_controller().set_model(Some(ArmedModel {
    client: Arc::new(PlayNaming("the strokes".into())),
    bundle: None,
    rejection: None,
  }));
  session.start();
  link.connect(DEVICE).await;
  assert!(eventually(|| session.gateway_for(DEVICE).is_some()).await);

  let source = FakeSource::resolving("hand", "hand:album:strokes");
  session
    .add_provider(source.clone() as Arc<dyn Provider>)
    .await
    .expect("the provider attaches");
  assert!(eventually(|| session.hub().attached_ids().contains(&"hand".to_owned())).await);

  for msg in voice_turn(Uuid::now_v7()) {
    link.say(DEVICE, msg);
  }

  assert!(
    eventually(|| dispatched(&link.heard_by(DEVICE)).is_some()).await,
    "the session answered the turn"
  );
  let dispatch = dispatched(&link.heard_by(DEVICE)).expect("a dispatch");
  assert_eq!(dispatch.resolved.intent, "PLAY");
  assert_eq!(
    dispatch.resolved.slots.uri.as_deref(),
    Some("hand:album:strokes"),
    "the session has to install the attached provider's resolver, not leave the dispatcher bare"
  );
  assert_eq!(source.catalog().searched(), ["the strokes"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_providers_auth_still_reaches_the_host_after_the_link_comes_back() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) = session(link.clone());
  session.start();
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_some()).await,
    "the session adopted the first link"
  );

  let source: Arc<FakeSource> = FakeSource::new("hand");
  session
    .add_provider(source.clone() as Arc<dyn Provider>)
    .await
    .expect("the provider attaches");
  assert!(
    eventually(|| session.hub().attached_ids().contains(&"hand".to_owned())).await,
    "and the hub took it"
  );

  source.report_auth(ProviderAuthState::Authenticated);
  let signed_in = session.snapshot().await;
  assert_eq!(
    signed_in.providers[0].auth_state.kind,
    AuthKind::Authenticated,
    "the session heard the first auth move"
  );

  link.drop_link(DEVICE);
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_none()).await,
    "the link going away releases that peer"
  );
  assert!(
    session.hub().attached_ids().contains(&"hand".to_owned()),
    "but the hub is session-scoped, so the provider stays attached with no peer at all"
  );
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_some()).await,
    "and the next link comes up under the same hub"
  );

  source.report_auth(ProviderAuthState::Failed {
    reason: "the token expired".into(),
  });
  let after = session.snapshot().await;
  assert_eq!(
    after.providers[0].auth_state.kind,
    AuthKind::Failed,
    "a provider that lost its account after a reconnect still says so: the session owns the auth \
     feed, so the observer it installed has to come back with the provider"
  );
  assert_eq!(
    after.providers[0].auth_state.message.as_deref(),
    Some("the token expired")
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_direct_link_that_dies_on_its_own_still_says_the_peer_went_away() {
  let (session, heard, _spool) = session(Arc::new(HandLink::default()));
  let (companion_io, daemon_io) = tokio::io::duplex(64 * 1024);

  session
    .connect_direct(
      LinkDevice {
        id: DIRECT.to_owned(),
        name: "direct".into(),
      },
      bridgething_gateway::transport::FramedConnector::new(companion_io),
    )
    .await;
  assert!(
    eventually(|| session.gateway_for(DIRECT).is_some()).await,
    "the direct link came up"
  );

  drop(daemon_io);

  assert!(
    eventually(|| session.gateway_for(DIRECT).is_none()).await,
    "a websocket that went away releases the peer without anybody asking it to"
  );
  assert!(
    heard
      .events()
      .iter()
      .any(|event| matches!(event, SessionEvent::PeerDisconnected { device_id } if device_id == DIRECT)),
    "a direct link has no transport to report the drop, so the routing task ending is the only thing that can, \
     and the host is left thinking it is still connected if it stays quiet"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_direct_link_the_host_dropped_says_the_peer_went_away_exactly_once() {
  let (session, heard, _spool) = session(Arc::new(HandLink::default()));
  let (companion_io, daemon_io) = tokio::io::duplex(64 * 1024);

  session
    .connect_direct(
      LinkDevice {
        id: DIRECT.to_owned(),
        name: "direct".into(),
      },
      bridgething_gateway::transport::FramedConnector::new(companion_io),
    )
    .await;
  assert!(eventually(|| session.gateway_for(DIRECT).is_some()).await);

  session.direct_disconnected(DIRECT).await;
  drop(daemon_io);
  tokio::time::sleep(Duration::from_millis(200)).await;

  let said = heard
    .events()
    .iter()
    .filter(|event| matches!(event, SessionEvent::PeerDisconnected { device_id } if device_id == DIRECT))
    .count();
  assert_eq!(
    said, 1,
    "the routing task ends behind every teardown, so the watcher has to keep quiet about a link the host already released"
  );
}

#[derive(Default)]
struct HandConnectivity {
  inbox: Mutex<Option<Arc<ConnectivityInbox>>>,
  stopped: Mutex<bool>,
}

impl ConnectivityMonitor for HandConnectivity {
  fn start(&self, inbox: Arc<ConnectivityInbox>) {
    *self.inbox.lock().unwrap() = Some(inbox);
  }

  fn stop(&self) {
    *self.stopped.lock().unwrap() = true;
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reachability_edge_reaches_every_provider_and_stop_releases_the_monitor() {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let monitor = Arc::new(HandConnectivity::default());
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
    speech: None,
    nlu: None,
    apple_music: None,
    image: None,
    model_validator: None,
    transfer_policy: None,
    connectivity: Some(monitor.clone()),
    device_waker: None,
    extensions: None,
  };
  let session = Session::new(
    CompanionConfig {
      host: HostInfo {
        app_name: "session-test".into(),
        app_version: "0.0.0".into(),
        os_name: "linux".into(),
        os_version: "0".into(),
        host_identifier: "session-test".into(),
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
    Arc::new(Heard::default()),
    Arc::new(Offline),
  );

  let source: Arc<FakeSource> = FakeSource::new("hand");
  session
    .add_provider(source.clone() as Arc<dyn Provider>)
    .await
    .expect("the provider registers");
  session.start();

  assert!(
    eventually(|| monitor.inbox.lock().unwrap().is_some()).await,
    "the session started the platform monitor"
  );
  let inbox = monitor.inbox.lock().unwrap().clone().expect("an inbox");
  inbox.on_changed(false);
  inbox.on_changed(true);

  assert!(
    eventually(|| source.connectivity_heard() == vec![false, true]).await,
    "every edge reaches the provider, in the order the platform reported it"
  );

  session.stop().await;
  assert!(
    *monitor.stopped.lock().unwrap(),
    "stopping the session releases the platform monitor"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn host_settings_are_session_state_and_project_into_the_snapshot() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) = session(link.clone());
  session.start();

  session
    .set_capability_flags(CapabilityFlags {
      geo: true,
      notifications: true,
      net_fetch: true,
      net_ws: true,
      audio_tts: true,
      voice_model: false,
    })
    .await;
  session
    .set_ota_poll_config(Some(bridgething_companion::api::OtaPollConfig {
      interval_seconds: 30,
      auto_push: false,
      root_url: None,
    }))
    .await;
  session.set_device_auto_resume(DEVICE, false);

  let snap = session.snapshot().await;
  assert!(snap.capability_flags.geo, "flags moved off the construction-time copy");
  assert!(snap.capability_flags.notifications);
  let poll = snap.ota_poll_config.expect("the stored poll config projects");
  assert_eq!(
    poll.interval_seconds, 30,
    "the snapshot keeps the host's value; the poller floors it"
  );
  assert!(!poll.auto_push);

  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_some()).await,
    "the session adopted the link"
  );
  link.drop_link(DEVICE);
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_none()).await,
    "the link went away"
  );
  let snap = session.snapshot().await;
  assert!(snap.capability_flags.geo);
  assert!(snap.ota_poll_config.is_some());
}

// ---- more than one peer -------------------------------------------------------

fn meta(serial: &str) -> libbridgething::BridgeThingMeta {
  libbridgething::BridgeThingMeta {
    bridgething_version: "0.0.0".into(),
    libbridgething_version: "0.0.0".into(),
    app_name: "bridgething".into(),
    nickname: None,
    app_version: "0.0.0".into(),
    daemon_sha256: None,
    wakeword_model_version: None,
    os_name: "linux".into(),
    os_version: "0".into(),
    os_description: String::new(),
    bt_mac: String::new(),
    serial_number: serial.into(),
    fcc_id: String::new(),
    ic_id: String::new(),
    model_name: "superbird".into(),
    channel: "dev".into(),
    image_variant: String::new(),
    image_version: String::new(),
    image_build_id: String::new(),
    image_build_date: String::new(),
    image_distro: String::new(),
    image_machine: String::new(),
    discord: String::new(),
    credits: String::new(),
  }
}

async fn two_peers(link: &Arc<HandLink>, session: &Arc<Session>) {
  link.connect(DEVICE).await;
  link.connect(OTHER).await;
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_some() && session.gateway_for(OTHER).is_some()).await,
    "both peers came up, saw {:?}",
    session.device_ids()
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_peer_does_not_displace_the_first() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) = session(link.clone());
  session.start();
  two_peers(&link, &session).await;

  let mut ids = session.device_ids();
  ids.sort();
  assert_eq!(
    ids,
    vec![DEVICE.to_owned(), OTHER.to_owned()],
    "adopting a second link keeps the first: a desktop is exactly where two Car Things get plugged in"
  );

  assert!(
    eventually(|| session.observer().peers().len() == 2).await,
    "and the host is told about both, saw {:?}",
    session.observer().peers()
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_from_one_peer_is_answered_only_on_that_peers_link() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) = session(link.clone());
  session.start();
  two_peers(&link, &session).await;

  link.say(DEVICE, keepalive(11));
  link.say(OTHER, keepalive(22));

  assert!(
    eventually(|| {
      keepalive_acks(&link.heard_by(DEVICE)) == vec![11] && keepalive_acks(&link.heard_by(OTHER)) == vec![22]
    })
    .await,
    "each peer's request is answered on its own link and nowhere else, first heard {:?}, second \
     heard {:?}",
    keepalive_acks(&link.heard_by(DEVICE)),
    keepalive_acks(&link.heard_by(OTHER))
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_one_peer_leaves_the_other_serving() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) = session(link.clone());
  session.start();
  two_peers(&link, &session).await;

  link.drop_link(DEVICE);
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_none()).await,
    "the first peer went away"
  );
  assert!(
    session.gateway_for(OTHER).is_some(),
    "and the second is untouched by it"
  );

  link.say(OTHER, keepalive(7));
  assert!(
    eventually(|| keepalive_acks(&link.heard_by(OTHER)).contains(&7)).await,
    "the surviving peer still gets answers, heard {:?}",
    keepalive_acks(&link.heard_by(OTHER))
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn now_playing_reaches_every_peer() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) = session(link.clone());
  session.start();
  two_peers(&link, &session).await;

  let source: Arc<FakeSource> = FakeSource::new("hand");
  session
    .add_provider(source.clone() as Arc<dyn Provider>)
    .await
    .expect("the provider attaches");

  let before = (
    player_snapshots(&link.heard_by(DEVICE)),
    player_snapshots(&link.heard_by(OTHER)),
  );
  source.submit(libbridgething::PlayerState {
    track: Some(libbridgething::MediaItem {
      uri: Some("hand:track:one".into()),
      title: Some("one".into()),
      ..libbridgething::MediaItem::default()
    }),
    playback: libbridgething::Playback {
      state: libbridgething::PlaybackState::Playing,
      ..libbridgething::Playback::default()
    },
    queue: Vec::new(),
    options: libbridgething::PlayerOptions::default(),
    context: None,
    target: None,
  });

  assert!(
    eventually(|| {
      player_snapshots(&link.heard_by(DEVICE)) > before.0 && player_snapshots(&link.heard_by(OTHER)) > before.1
    })
    .await,
    "one companion has one now-playing, so the snapshot fans out to every peer; first saw {}, \
     second saw {}",
    player_snapshots(&link.heard_by(DEVICE)),
    player_snapshots(&link.heard_by(OTHER))
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn device_meta_is_attributed_to_the_peer_that_announced_it() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) = session(link.clone());
  session.start();
  two_peers(&link, &session).await;

  for (device_id, serial) in [(DEVICE, "serial-one"), (OTHER, "serial-two")] {
    link.say(
      device_id,
      BridgeToGatewayMsg {
        id: Uuid::now_v7(),
        meta: MsgMeta::Event,
        data: BridgeToGatewayMsgData::Version(Box::new(meta(serial))),
      },
    );
  }

  let filed = |device_id: &'static str, serial: &'static str| {
    session
      .observer()
      .device_metas()
      .iter()
      .any(|entry| entry.device_id == device_id && entry.meta.serial_number == serial)
  };
  assert!(
    eventually(|| filed(DEVICE, "serial-one") && filed(OTHER, "serial-two")).await,
    "each announce is filed under the peer that sent it, saw {:?}",
    session.observer().device_metas()
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ancs_seed_lands_on_a_session_with_no_notification_backend() {
  let link = Arc::new(HandLink::default());
  let (session, heard, _spool) = session(link.clone());
  session.start();
  two_peers(&link, &session).await;

  for (device_id, state) in [
    (DEVICE, AncsAuthState::Authorized),
    (OTHER, AncsAuthState::Unauthorized),
  ] {
    link.say(
      device_id,
      BridgeToGatewayMsg {
        id: Uuid::now_v7(),
        meta: MsgMeta::Event,
        data: libbridgething::gateway::BridgeToGatewayNotificationsMsg::AncsAuthStateChanged(state).into(),
      },
    );
  }

  let filed = |device_id: &'static str, status: AncsAuthStatus| {
    session
      .observer()
      .ancs_statuses()
      .iter()
      .any(|entry| entry.device_id == device_id && entry.status == status)
  };
  assert!(
    eventually(|| filed(DEVICE, AncsAuthStatus::Authorized) && filed(OTHER, AncsAuthStatus::Unauthorized)).await,
    "the daemon seeds the pairing state on every connect and it is peer state, not a notification-backend \
     concern: this session has no notification backend at all, saw {:?}",
    session.observer().ancs_statuses()
  );

  let snap = session.snapshot().await;
  assert!(
    snap
      .ancs_auth_statuses
      .iter()
      .any(|entry| entry.device_id == DEVICE && entry.status == AncsAuthStatus::Authorized),
    "and it projects into the snapshot the host reconciles from, saw {:?}",
    snap.ancs_auth_statuses
  );
  assert!(
    heard.events().iter().any(|event| matches!(
      event,
      SessionEvent::AncsAuthStatusChanged { device_id, status }
        if device_id == DEVICE && *status == AncsAuthStatus::Authorized
    )),
    "the host also gets the live edge, not only the reconcile"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_goes_away_takes_every_cache_keyed_on_it_along() {
  let (session, _heard, _spool) = session(Arc::new(HandLink::default()));
  let observer = session.observer();

  for (device_id, serial, status) in [
    (DEVICE, "serial-one", AncsAuthState::Authorized),
    (OTHER, "serial-two", AncsAuthState::Unauthorized),
  ] {
    observer.peer_connected(SessionPeer {
      id: device_id.to_owned(),
      name: "hand".into(),
      status: PeerLinkStatus::Connected,
      link_error: None,
    });
    observer.device_meta(device_id, meta(serial));
    observer.ancs(device_id, status);
    observer.webapps_listed(device_id, Vec::new(), None);
  }

  observer.peer_disconnected(DEVICE);

  assert!(
    observer.peers().iter().all(|peer| peer.id != DEVICE)
      && observer.device_metas().iter().all(|entry| entry.device_id != DEVICE)
      && observer.ancs_statuses().iter().all(|entry| entry.device_id != DEVICE)
      && observer.webapps().iter().all(|entry| entry.device_id != DEVICE),
    "a device that left keeps no cached state behind: the host reads the head of these lists, so a stale \
     entry shows one device's channel and webapp shelf under another, saw meta {:?} ancs {:?} webapps {:?}",
    observer.device_metas(),
    observer.ancs_statuses(),
    observer.webapps()
  );
  assert!(
    observer.peers().iter().any(|peer| peer.id == OTHER)
      && observer.device_metas().iter().any(|entry| entry.device_id == OTHER)
      && observer.ancs_statuses().iter().any(|entry| entry.device_id == OTHER)
      && observer.webapps().iter().any(|entry| entry.device_id == OTHER),
    "and the peer that is still linked keeps all of its own"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_the_transport_could_not_send_closes_the_link_rather_than_wedging_it() {
  let link = Arc::new(HandLink::default());
  let (session, heard, _spool) = session(link.clone());
  session.start();
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.peer_for(DEVICE).is_some()).await,
    "the link came up"
  );

  link.fail_sends();
  link.say(DEVICE, keepalive(1));

  assert!(
    eventually(|| session.peer_for(DEVICE).is_none()).await,
    "a transport that drops a batch never completes the write, so without a failure channel the \
     outbound half waits on a credit that is never coming and every later command dies in the queue"
  );
  assert!(
    heard.events().iter().any(|event| matches!(
      event,
      SessionEvent::PeerDisconnected { device_id } if device_id == DEVICE
    )),
    "and the host is told the peer is gone rather than left showing it linked"
  );
}

// ---- provider catalog ---------------------------------------------------------

const REFRESH_KEY: &str = "spotify.refresh_token";

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_provider_id_is_refused() {
  let (session, _heard, _spool) = session_full(
    Arc::new(HandLink::default()),
    Arc::new(MemorySecrets::default()),
    true,
    None,
    None,
    None,
  );
  assert!(session.connect_provider("tidal").await.is_err());
  assert!(
    session
      .complete_provider_auth(
        "tidal",
        ProviderTokens {
          access_token: "a".into(),
          refresh_token: "r".into(),
        },
      )
      .await
      .is_err()
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_catalog_sign_in_runs_before_any_link_exists() {
  let (session, heard, _spool) = session_full(
    Arc::new(HandLink::default()),
    Arc::new(MemorySecrets::default()),
    true,
    None,
    None,
    None,
  );

  let listed = session.provider_infos();
  assert_eq!(listed.len(), 1, "the catalog lists spotify before it is live");
  assert_eq!(listed[0].id, "spotify");
  assert!(listed[0].available);
  assert!(!listed[0].connected);

  session.connect_provider("spotify").await.expect("the provider came up");
  assert!(
    eventually(|| {
      session
        .provider_infos()
        .first()
        .is_some_and(|info| info.auth_state.kind == AuthKind::Failed)
    })
    .await,
    "the offline sign-in failed through the auth feed with no device link up"
  );
  assert!(
    heard
      .events()
      .iter()
      .any(|event| matches!(event, SessionEvent::ProvidersChanged { .. })),
    "every auth move re-announced the provider list"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn complete_provider_auth_persists_the_refresh_token_and_connects() {
  let secrets = Arc::new(MemorySecrets::default());
  let (session, _heard, _spool) = session_full(Arc::new(HandLink::default()), secrets.clone(), true, None, None, None);

  session
    .complete_provider_auth(
      "spotify",
      ProviderTokens {
        access_token: "bearer-from-pkce".into(),
        refresh_token: "refresh-from-pkce".into(),
      },
    )
    .await
    .expect("completion registered the provider");

  assert_eq!(
    secrets.get(REFRESH_KEY.into()).as_deref(),
    Some("refresh-from-pkce"),
    "the refresh token landed in the secret store"
  );
  let info = &session.provider_infos()[0];
  assert!(info.connected, "credentials in the store read as connected");

  session.cancel_auth("spotify").await;
  assert!(
    secrets.get(REFRESH_KEY.into()).is_some(),
    "cancel keeps the stored credentials"
  );
  assert!(!session.provider_infos()[0].connected);

  session.connect_provider("spotify").await.expect("reconnects");
  session.disconnect_provider("spotify").await;
  assert!(
    secrets.get(REFRESH_KEY.into()).is_none(),
    "disconnect is a sign-out and clears them"
  );
  assert!(!session.provider_infos()[0].connected);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_catalog_provider_sits_on_the_hub_before_and_after_a_link_arrives() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) =
    session_full(link.clone(), Arc::new(MemorySecrets::default()), true, None, None, None);
  session.connect_provider("spotify").await.expect("came up");
  assert!(
    session.hub().attached_ids().contains(&"spotify".to_owned()),
    "a sign-in with no peer at all still lands on the session's hub: there is no second detached hub \
     for it to be parked on"
  );

  session.start();
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_some()).await,
    "the link came up"
  );
  assert!(
    session.hub().attached_ids().contains(&"spotify".to_owned()),
    "and the link arriving neither detaches nor re-attaches it"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn stored_credentials_restore_the_sign_in_on_start() {
  let secrets = Arc::new(MemorySecrets::default());
  secrets.set(REFRESH_KEY.into(), "stored-refresh".into());
  let (session, _heard, _spool) = session_full(Arc::new(HandLink::default()), secrets, true, None, None, None);

  session.start();
  assert!(
    eventually(|| session.provider_infos().first().is_some_and(|info| info.connected)).await,
    "an install holding a refresh token comes back signed in without being asked"
  );
}

// ---- device log ring ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_log_entry_lands_in_the_ring_and_the_event_stream() {
  let link = Arc::new(HandLink::default());
  let (session, heard, _spool) = session(link.clone());
  session.start();
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.peer_for(DEVICE).is_some()).await,
    "the link came up"
  );

  let peer = session.peer_for(DEVICE).expect("a live peer");
  peer
    .log_entry(LogEntry {
      ts_unix_s: 1_700_000_000,
      level: libbridgething::LogLevel::Warn,
      target: "daemon::ota".into(),
      message: "the spool filled".into(),
    })
    .await
    .expect("the system surface took the entry");

  let tail = session.log_ring().tail(10);
  assert!(
    tail.iter().any(
      |record| record.message == "the spool filled" && record.origin == bridgething_delivery::log::LogOrigin::Device
    ),
    "the forwarded line is retained with a device origin, got {tail:?}"
  );
  assert!(
    heard.events().iter().any(|event| matches!(
      event,
      SessionEvent::Log {
        origin: LogOrigin::Device,
        ..
      }
    )),
    "and the live event said which side produced it"
  );
}

fn log_line(message: &str) -> LogEntry {
  LogEntry {
    ts_unix_s: 1_700_000_000,
    level: libbridgething::LogLevel::Info,
    target: "daemon::net".into(),
    message: message.into(),
  }
}

fn said(entry: LogEntry) -> BridgeToGatewayMsg {
  BridgeToGatewayMsg {
    id: Uuid::now_v7(),
    meta: MsgMeta::Event,
    data: BridgeToGatewaySystemMsg::LogEntry(entry).into(),
  }
}

fn answering(request_id: Uuid, data: BridgeToGatewayMsgData) -> BridgeToGatewayMsg {
  BridgeToGatewayMsg {
    id: Uuid::now_v7(),
    meta: MsgMeta::Response(ResponseMeta { request_id }),
    data,
  }
}

fn asked(msgs: &[GatewayToBridgeMsg], want: fn(&GatewayToBridgeSystemMsg) -> bool) -> Option<Uuid> {
  msgs.iter().find_map(|msg| match &msg.data {
    GatewayToBridgeMsgData::System(system) if matches!(msg.meta, MsgMeta::Request) && want(system) => Some(msg.id),
    _ => None,
  })
}

async fn awaited(link: &HandLink, want: fn(&GatewayToBridgeSystemMsg) -> bool) -> Uuid {
  assert!(
    eventually(|| asked(&link.heard_by(DEVICE), want).is_some()).await,
    "the request reached the device"
  );
  asked(&link.heard_by(DEVICE), want).expect("the request reached the device")
}

async fn drained(link: &HandLink, seq: u32) {
  link.say(DEVICE, keepalive(seq));
  assert!(
    eventually(|| keepalive_acks(&link.heard_by(DEVICE)).contains(&seq)).await,
    "the inbound path drained past every frame queued before the keepalive"
  );
}

fn unsubscribed(msgs: &[GatewayToBridgeMsg]) -> Vec<String> {
  msgs
    .iter()
    .filter_map(|msg| match &msg.data {
      GatewayToBridgeMsgData::System(GatewayToBridgeSystemMsg::LogsUnsubscribe(payload)) => Some(payload.token.clone()),
      _ => None,
    })
    .collect()
}

fn logged(heard: &Heard) -> Vec<String> {
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

async fn streaming(session: &Arc<Session>, link: &HandLink) -> tokio::task::JoinHandle<()> {
  let toggling = tokio::spawn({
    let session = session.clone();
    async move { session.set_device_log_streaming(true).await }
  });
  let subscribe = awaited(link, |msg| matches!(msg, GatewayToBridgeSystemMsg::LogsSubscribe(_))).await;
  link.say(
    DEVICE,
    answering(
      subscribe,
      BridgeToGatewaySystemMsg::LogsSubscribeReply(LogsSubscribeReply { token: "tap-1".into() }).into(),
    ),
  );
  toggling
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_the_log_stream_seeds_the_session_so_far_ahead_of_the_lines_it_overlaps() {
  let link = Arc::new(HandLink::default());
  let (session, heard, _spool) = session(link.clone());
  session.start();
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.peer_for(DEVICE).is_some()).await,
    "the link came up"
  );

  let toggling = streaming(&session, &link).await;
  link.say(DEVICE, said(log_line("spoken while the history was in flight")));
  let tail = awaited(&link, |msg| matches!(msg, GatewayToBridgeSystemMsg::LogsTail(_))).await;
  drained(&link, 7).await;
  assert!(
    logged(&heard).is_empty(),
    "a line seen before the history lands is held, not shown ahead of it"
  );

  link.say(
    DEVICE,
    answering(
      tail,
      BridgeToGatewaySystemMsg::LogsTailReply(LogsTailReply {
        entries: vec![
          log_line("spoken before the app asked"),
          log_line("spoken while the history was in flight"),
        ],
      })
      .into(),
    ),
  );
  toggling.await.expect("the toggle settled");
  link.say(DEVICE, said(log_line("spoken once the stream was open")));

  let want = vec![
    "spoken before the app asked".to_owned(),
    "spoken while the history was in flight".to_owned(),
    "spoken once the stream was open".to_owned(),
  ];
  assert!(
    eventually(|| logged(&heard) == want).await,
    "history, then the overlap exactly once, then live, got {:?}",
    logged(&heard)
  );
  assert_eq!(
    session
      .log_ring()
      .tail(10)
      .into_iter()
      .map(|record| record.message)
      .collect::<Vec<_>>(),
    want,
    "and the snapshot the app reads back carries the same order"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_switched_off_while_its_subscribe_was_in_flight_hands_the_tap_back() {
  let link = Arc::new(HandLink::default());
  let (session, heard, _spool) = session(link.clone());
  session.start();
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.peer_for(DEVICE).is_some()).await,
    "the link came up"
  );

  let toggling = tokio::spawn({
    let session = session.clone();
    async move { session.set_device_log_streaming(true).await }
  });
  let subscribe = awaited(&link, |msg| matches!(msg, GatewayToBridgeSystemMsg::LogsSubscribe(_))).await;
  link.say(DEVICE, said(log_line("spoken while the subscribe was in flight")));

  session.set_device_log_streaming(false).await;
  link.say(
    DEVICE,
    answering(
      subscribe,
      BridgeToGatewaySystemMsg::LogsSubscribeReply(LogsSubscribeReply { token: "tap-1".into() }).into(),
    ),
  );
  toggling.await.expect("the toggle settled");

  assert!(
    eventually(|| unsubscribed(&link.heard_by(DEVICE)) == ["tap-1"]).await,
    "the late token is handed back rather than left streaming, got {:?}",
    unsubscribed(&link.heard_by(DEVICE))
  );
  assert!(
    asked(&link.heard_by(DEVICE), |msg| matches!(
      msg,
      GatewayToBridgeSystemMsg::LogsTail(_)
    ))
    .is_none(),
    "and a stream nobody wants never asks for history"
  );
  assert_eq!(
    logged(&heard),
    vec!["spoken while the subscribe was in flight".to_owned()],
    "the line held for a backfill that will not come is released, not stranded"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_will_not_serve_the_history_still_gets_a_live_stream() {
  let link = Arc::new(HandLink::default());
  let (session, heard, _spool) = session(link.clone());
  session.start();
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.peer_for(DEVICE).is_some()).await,
    "the link came up"
  );

  let toggling = streaming(&session, &link).await;
  link.say(DEVICE, said(log_line("spoken while the history was in flight")));
  let tail = awaited(&link, |msg| matches!(msg, GatewayToBridgeSystemMsg::LogsTail(_))).await;
  link.say(
    DEVICE,
    answering(tail, BridgeToGatewayMsgData::Error(WireError::Unsupported)),
  );
  toggling.await.expect("the toggle settled");
  link.say(DEVICE, said(log_line("spoken once the stream was open")));

  let want = vec![
    "spoken while the history was in flight".to_owned(),
    "spoken once the stream was open".to_owned(),
  ];
  assert!(
    eventually(|| logged(&heard) == want).await,
    "a refused tail releases the held line and keeps streaming, got {:?}",
    logged(&heard)
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_catalog_sign_in_keeps_its_auth_feed_across_link_churn() {
  let link = Arc::new(HandLink::default());
  let (session, heard, _spool) = session_full(link.clone(), Arc::new(MemorySecrets::default()), true, None, None, None);
  session.connect_provider("spotify").await.expect("came up");
  assert!(
    eventually(|| session
      .provider_infos()
      .first()
      .is_some_and(|info| info.auth_state.kind == AuthKind::Failed))
    .await,
    "the offline sign-in settled"
  );

  session.start();
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_some()).await,
    "a peer arrived"
  );
  link.drop_link(DEVICE);
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_none()).await,
    "and left again"
  );
  link.connect(DEVICE).await;
  assert!(
    eventually(|| session.gateway_for(DEVICE).is_some()).await,
    "and came back"
  );
  assert!(
    session.hub().attached_ids().contains(&"spotify".to_owned()),
    "none of that churn detached the provider"
  );

  let before = heard.events().len();
  session.connect_provider("spotify").await.expect("signs in again");
  assert!(
    eventually(|| {
      heard.events()[before..].iter().any(|event| {
        matches!(
          event,
          SessionEvent::ProvidersChanged { providers }
            if providers.first().is_some_and(|p| p.auth_state.kind == AuthKind::Pending)
        )
      })
    })
    .await,
    "signing in again re-opens the auth conversation through the session's observer: re-attaching \
     a provider must not tear off the feed the session installed on it, events after the churn: \
     {:?}",
    &heard.events()[before..]
  );
}

struct QuietAm;

impl AppleMusicBackend for QuietAm {
  fn start(&self, _inbox: Arc<AmPlayerInbox>) {}
  fn stop(&self) {}
  fn snapshot(&self, sink: Arc<AmSnapshotSink>) {
    sink.complete(AmPlayerSnapshot {
      entry: None,
      playing: false,
      position_ms: 0,
      shuffle: false,
      repeat: AmRepeatMode::Off,
      can_seek: false,
    });
  }
  fn auth_status(&self, sink: Arc<AmAuthSink>) {
    sink.complete(AmAuthStatus::Authorized);
  }
  fn request_authorization(&self, sink: Arc<AmAuthSink>) {
    sink.complete(AmAuthStatus::Authorized);
  }
  fn can_play_catalog_content(&self, sink: Arc<AmCatalogSink>) {
    sink.complete(Some(true));
  }
  fn is_other_audio_playing(&self, sink: Arc<AmFlagSink>) {
    sink.complete(false);
  }
  fn play_context(&self, _context_uri: String, _start_at_uri: Option<String>, sink: Arc<AmActionSink>) {
    sink.ok();
  }
  fn queue_insert(&self, _uri: String, _next: bool, sink: Arc<AmActionSink>) {
    sink.ok();
  }
  fn command(&self, _cmd: AmPlayerCommand, sink: Arc<AmActionSink>) {
    sink.ok();
  }
  fn library(&self, _scope: AmLibraryScope, _limit: u32, _offset: u32, sink: Arc<AmPageSink>) {
    sink.complete(AmPage {
      items: Vec::new(),
      total: Some(0),
      has_more: false,
    });
  }
  fn recommendations(&self, sink: Arc<AmShelvesSink>) {
    sink.complete(Vec::new());
  }
  fn resolve(&self, _uri: String, sink: Arc<AmItemSink>) {
    sink.fail("nothing to resolve in these cases".into());
  }
  fn search(&self, _query: String, _limit: u32, sink: Arc<AmSearchSink>) {
    sink.complete(AmSearchResults {
      songs: Vec::new(),
      albums: Vec::new(),
      artists: Vec::new(),
      playlists: Vec::new(),
    });
  }
  fn is_favorite(&self, uris: Vec<String>, sink: Arc<AmFavoritesSink>) {
    sink.complete(vec![false; uris.len()]);
  }
  fn add_favorite(&self, _uri: String, sink: Arc<AmActionSink>) {
    sink.ok();
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_apple_music_backend_is_a_provider_the_settings_screen_can_offer() {
  let link = Arc::new(HandLink::default());
  let (session, _heard, _spool) = session_full(
    link.clone(),
    Arc::new(MemorySecrets::default()),
    false,
    Some(Arc::new(QuietAm)),
    None,
    None,
  );

  let infos = session.provider_infos();
  assert!(
    infos
      .iter()
      .any(|info| info.id == "apple-music" && info.available && info.display_name == "Apple Music"),
    "a host that supplies an apple music backend gets the provider in the catalog, saw {infos:?}"
  );

  session
    .connect_provider("apple-music")
    .await
    .expect("the catalog entry builds a provider");
  assert!(
    eventually(|| session
      .provider_infos()
      .iter()
      .any(|info| info.id == "apple-music" && info.auth_state.kind == AuthKind::Authenticated))
    .await,
    "the built provider runs against the backend and reports the system authorization"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_apple_music_sign_in_comes_back_on_the_next_launch_until_it_is_signed_out() {
  let secrets = Arc::new(MemorySecrets::default());
  let (session, _heard, _spool) = session_full(
    Arc::new(HandLink::default()),
    secrets.clone(),
    false,
    Some(Arc::new(QuietAm)),
    None,
    None,
  );
  session
    .connect_provider("apple-music")
    .await
    .expect("the catalog entry builds a provider");
  assert!(
    eventually(|| session
      .provider_infos()
      .iter()
      .any(|info| info.id == "apple-music" && info.auth_state.kind == AuthKind::Authenticated))
    .await,
    "the sign-in went through"
  );

  let (relaunched, _heard, _spool) = session_full(
    Arc::new(HandLink::default()),
    secrets.clone(),
    false,
    Some(Arc::new(QuietAm)),
    None,
    None,
  );
  relaunched.start();
  assert!(
    eventually(|| relaunched
      .provider_infos()
      .iter()
      .any(|info| info.id == "apple-music" && info.connected))
    .await,
    "apple music holds no token of its own, but the system authorization it runs on outlives the \
     process: a relaunch has to restore it rather than making the user tap connect every launch, \
     saw {:?}",
    relaunched.provider_infos()
  );

  relaunched.disconnect_provider("apple-music").await;
  let (signed_out, _heard, _spool) = session_full(
    Arc::new(HandLink::default()),
    secrets,
    false,
    Some(Arc::new(QuietAm)),
    None,
    None,
  );
  signed_out.start();
  assert!(
    !eventually(|| signed_out
      .provider_infos()
      .iter()
      .any(|info| info.id == "apple-music" && info.connected))
    .await,
    "and a sign-out stays signed out: the restore is keyed on the user asking for the provider, not \
     on the system grant still being there"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_media_session_backend_mirrors_the_audible_player_to_every_peer() {
  let link = Arc::new(HandLink::default());
  let media = FakeMediaBackend::new();
  let (session, _heard, _spool) = session_full(
    link.clone(),
    Arc::new(MemorySecrets::default()),
    false,
    None,
    None,
    Some(media.clone()),
  );
  session.start();
  two_peers(&link, &session).await;

  assert!(
    eventually(|| media.inbox.lock().unwrap().is_some()).await,
    "starting the session starts the media backend"
  );

  let before = (
    player_snapshots(&link.heard_by(DEVICE)),
    player_snapshots(&link.heard_by(OTHER)),
  );
  media.emit(vec![playing("Video", "Creator", "org.example.player")]);
  assert!(
    eventually(|| {
      player_snapshots(&link.heard_by(DEVICE)) > before.0 && player_snapshots(&link.heard_by(OTHER)) > before.1
    })
    .await,
    "whatever the host is playing reaches every peer without any provider being signed in"
  );

  session.stop().await;
  assert!(
    eventually(|| media.inbox.lock().unwrap().is_none()).await,
    "stopping the session stops the media backend"
  );
}
