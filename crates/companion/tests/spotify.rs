#[path = "rig/art.rs"]
mod art;
#[path = "rig/backends.rs"]
mod backends;
#[path = "dispatch/flow.rs"]
mod flow;
#[path = "rig/log_sink.rs"]
mod log_sink;
#[path = "support/poll.rs"]
mod poll;
#[path = "dispatch/quiet.rs"]
mod quiet;
#[path = "dispatch/support.rs"]
mod support;

use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
  time::Duration,
};

use base64::Engine;
use bridgething_companion::{
  api::{CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo, ModelPlatform, SpotifyProviderConfig},
  backend::{ModelArtifactKind, ModelArtifactValidator, ModelValidationError, SecretStore, TransferPolicy},
  hub::Hub,
  provider::{
    Provider, ProviderAuthState, ProviderRegistry,
    spotify::{SpotifyConfig, SpotifyProvider},
  },
  session::Session,
};
use bridgething_io::{
  HttpDownloadSink, HttpRequest, HttpResponse, HttpSink, HttpTransport, WsConnect, WsFrame, WsInbox, WsTransport,
};
use libbridgething::{
  CompanionAuthorityScope, ItemKind, ItemRef, PlaybackState, PlaybackTargetKind,
  gateway::{
    AuthorityClaim, GatewayToBridgeAuthorityMsg, GatewayToBridgeLibraryMsg, GatewayToBridgeMsg, GatewayToBridgeMsgData,
    GatewayToBridgePlayerMsg, LibraryFavoritesContainsRequest, PlaybackTargets, QueueSnapshot,
  },
};
use librespot_protocol::{
  connect::{Cluster, ClusterUpdate, DeviceInfo},
  devices::DeviceType,
  player::{PlayerState as PbPlayerState, ProvidedTrack},
};
use poll::eventually;
use protobuf::{Message, MessageField};
use support::Peer;
use uuid::Uuid;

use crate::art::{ArtProbe, TagScaler};

const IMAGE_HEX: &str = "ab67616d00001e02deadbeef";

// ---- fixtures ----------------------------------------------------------------

#[derive(Default)]
struct MemorySecrets {
  entries: Mutex<std::collections::HashMap<String, String>>,
  device_id_reads: AtomicUsize,
  device_id_stall: Mutex<Duration>,
}

impl MemorySecrets {
  fn paired() -> Arc<Self> {
    let store = Self::default();
    store
      .entries
      .lock()
      .unwrap()
      .insert("spotify.refresh_token".into(), "rt".into());
    store
      .entries
      .lock()
      .unwrap()
      .insert("spotify.username".into(), "user".into());
    Arc::new(store)
  }

  fn builds(&self) -> usize {
    self.device_id_reads.load(Ordering::SeqCst)
  }

  fn stall_device_id(&self, how_long: Duration) {
    *self.device_id_stall.lock().unwrap() = how_long;
  }
}

impl SecretStore for MemorySecrets {
  fn get(&self, key: String) -> Option<String> {
    if key == "spotify.device_id" {
      self.device_id_reads.fetch_add(1, Ordering::SeqCst);
      let stall = *self.device_id_stall.lock().unwrap();
      if !stall.is_zero() {
        std::thread::sleep(stall);
      }
    }
    self.entries.lock().unwrap().get(&key).cloned()
  }

  fn set(&self, key: String, value: String) {
    self.entries.lock().unwrap().insert(key, value);
  }

  fn remove(&self, key: String) {
    self.entries.lock().unwrap().remove(&key);
  }

  fn get_blob(&self, _key: String) -> Option<Vec<u8>> {
    None
  }
}

struct FakeHttp {
  hits: Mutex<Vec<(String, Vec<u8>)>>,
  cluster: Mutex<Option<Vec<u8>>>,
  product: Mutex<&'static str>,
  paired: bool,
}

impl FakeHttp {
  fn new() -> Arc<Self> {
    Arc::new(Self {
      hits: Mutex::new(Vec::new()),
      cluster: Mutex::new(None),
      product: Mutex::new(r#"{"product":"premium","catalogue":"premium","country":"US"}"#),
      paired: true,
    })
  }

  fn set_cluster(&self, cluster: &Cluster) {
    *self.cluster.lock().unwrap() = Some(cluster.write_to_bytes().unwrap());
  }

  fn hits_matching(&self, needle: &str) -> Vec<(String, Vec<u8>)> {
    self
      .hits
      .lock()
      .unwrap()
      .iter()
      .filter(|(url, _)| url.contains(needle))
      .cloned()
      .collect()
  }

  async fn wait_hit(&self, needle: &str) -> (String, Vec<u8>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
      if let Some(hit) = self.hits_matching(needle).into_iter().next() {
        return hit;
      }
      assert!(tokio::time::Instant::now() < deadline, "no {needle} request arrived");
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  }
}

impl HttpTransport for FakeHttp {
  fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>) {
    let url = request.url.clone();
    self.hits.lock().unwrap().push((url.clone(), request.body.clone()));
    let ok = |body: Vec<u8>| HttpResponse {
      status: 200,
      headers: Vec::new(),
      body,
    };
    if url.contains("clienttoken") {
      sink.fail("no client token in tests".into());
    } else if url.contains("/api/device/code") {
      sink.complete(ok(
        br#"{"device_code":"dc","user_code":"ABCD","verification_url":"https://spotify.com/pair","interval":1,"expires_in":60}"#.to_vec(),
      ));
    } else if url.contains("/api/token") {
      if self.paired {
        sink.complete(ok(br#"{"access_token":"bearer","expires_in":3600}"#.to_vec()));
      } else {
        sink.complete(ok(br#"{"error":"authorization_pending"}"#.to_vec()));
      }
    } else if url.contains("apresolve.spotify.com") {
      sink.complete(ok(br#"{"dealer":["dealer.test:443"]}"#.to_vec()));
    } else if url.contains("melody/v1/product_state") {
      sink.complete(ok(self.product.lock().unwrap().as_bytes().to_vec()));
    } else if url.contains("/connect-state/v1/devices/") {
      sink.complete(ok(self.cluster.lock().unwrap().clone().unwrap_or_default()));
    } else {
      sink.complete(ok(Vec::new()));
    }
  }

  fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    sink.on_failed("the fixture transport has no streaming arm".into());
  }
}

#[derive(Default)]
struct FakeWs {
  socket: Mutex<Option<(Uuid, Arc<WsInbox>)>>,
  connects: Mutex<usize>,
  sent: Mutex<Vec<String>>,
}

impl FakeWs {
  fn new() -> Arc<Self> {
    Arc::new(Self::default())
  }

  async fn wait_connected(&self, at_least: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while *self.connects.lock().unwrap() < at_least {
      assert!(tokio::time::Instant::now() < deadline, "the dealer never connected");
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  }

  fn push(&self, text: &str) {
    let (id, inbox) = self.socket.lock().unwrap().clone().expect("a live dealer socket");
    inbox.on_text(id, text.to_owned());
  }

  fn open_session(&self) {
    self.push(r#"{"headers":{"Spotify-Connection-Id":"test-connection"}}"#);
  }

  fn push_cluster(&self, cluster: &Cluster) {
    let mut update = ClusterUpdate::new();
    update.cluster = MessageField::some(cluster.clone());
    let payload = base64::engine::general_purpose::STANDARD.encode(update.write_to_bytes().unwrap());
    self.push(&format!(
      r#"{{"type":"message","uri":"hm://connect-state/v1/cluster","payloads":["{payload}"]}}"#
    ));
  }

  fn push_library_changed(&self) {
    self.push(r#"{"type":"message","uri":"hm://playlist/v2/user/x","payloads":[]}"#);
  }
}

impl WsTransport for FakeWs {
  fn connect(&self, connect: WsConnect, inbox: Arc<WsInbox>) {
    *self.socket.lock().unwrap() = Some((connect.id, inbox.clone()));
    *self.connects.lock().unwrap() += 1;
    inbox.on_open(connect.id, None);
  }

  fn send(&self, _id: Uuid, frame: WsFrame) {
    if let WsFrame::Text(text) = frame {
      self.sent.lock().unwrap().push(text);
    }
  }

  fn disconnect(&self, _id: Uuid, _code: Option<u16>, _reason: Option<String>) {}
}

fn provided_track(uri: &str, title: &str, saved: bool) -> ProvidedTrack {
  let mut track = ProvidedTrack::new();
  track.uri = uri.to_owned();
  let md = &mut track.metadata;
  md.insert("title".into(), title.to_owned());
  md.insert("artist_name".into(), "Artist".into());
  md.insert("artist_uri".into(), "spotify:artist:1".into());
  md.insert("album_title".into(), "Album".into());
  md.insert("album_uri".into(), "spotify:album:1".into());
  md.insert("duration".into(), "1000".into());
  md.insert(
    "image_xlarge_url".into(),
    format!("https://i.scdn.co/image/{IMAGE_HEX}"),
  );
  if saved {
    md.insert("collection.in_collection".into(), "true".into());
  }
  track
}

struct ClusterSpec {
  track: Option<ProvidedTrack>,
  next: Vec<ProvidedTrack>,
  active: &'static str,
  playing: bool,
  position_ms: i64,
  devices: Vec<(&'static str, &'static str, DeviceType, u32)>,
  context: &'static str,
}

impl Default for ClusterSpec {
  fn default() -> Self {
    Self {
      track: None,
      next: Vec::new(),
      active: "",
      playing: false,
      position_ms: 0,
      devices: Vec::new(),
      context: "spotify:playlist:1",
    }
  }
}

fn cluster(spec: ClusterSpec) -> Cluster {
  let mut cluster = Cluster::new();
  cluster.active_device_id = spec.active.to_owned();
  let mut ps = PbPlayerState::new();
  if let Some(track) = spec.track {
    ps.track = MessageField::some(track);
    ps.is_playing = spec.playing;
    ps.is_paused = !spec.playing;
    ps.position_as_of_timestamp = spec.position_ms;
    ps.duration = 1000;
    ps.context_uri = spec.context.to_owned();
    ps.context_metadata.insert("context_description".into(), "Ctx".into());
  }
  ps.next_tracks = spec.next;
  cluster.player_state = MessageField::some(ps);
  for (id, name, kind, volume) in spec.devices {
    let mut info = DeviceInfo::new();
    info.name = name.to_owned();
    info.device_type = kind.into();
    info.volume = volume;
    cluster.device.insert(id.to_owned(), info);
  }
  cluster
}

struct Rig {
  hub: Arc<Hub>,
  peer: Peer,
  provider: Arc<SpotifyProvider>,
  http: Arc<FakeHttp>,
  ws: Arc<FakeWs>,
  auth_states: Arc<Mutex<Vec<ProviderAuthState>>>,
}

async fn boot(initial: Cluster) -> Rig {
  boot_full(initial, MemorySecrets::paired(), None).await
}

async fn boot_with(initial: Cluster, secrets: Arc<MemorySecrets>) -> Rig {
  boot_full(initial, secrets, None).await
}

async fn boot_full(initial: Cluster, secrets: Arc<MemorySecrets>, product: Option<&'static str>) -> Rig {
  let (gateway, peer) = Peer::link();
  let hub = Hub::new(
    Arc::new(gateway),
    HostInfo {
      app_name: "spotify-test".into(),
      app_version: "0.0.1".into(),
      os_name: "test".into(),
      os_version: String::new(),
      host_identifier: String::new(),
    },
    CapabilityFlags {
      geo: false,
      notifications: false,
      net_fetch: true,
      net_ws: true,
      audio_tts: false,
      voice_model: false,
    },
  );
  hub.start();
  let http = FakeHttp::new();
  http.set_cluster(&initial);
  if let Some(product) = product {
    *http.product.lock().unwrap() = product;
  }
  let ws = FakeWs::new();
  let provider = SpotifyProvider::new(
    SpotifyConfig {
      worker_base: "https://worker.test/auth".into(),
      psk: "psk".into(),
      device_id: "me-device".into(),
    },
    http.clone(),
    ws.clone(),
    secrets,
    None,
    None,
  );
  let auth_states: Arc<Mutex<Vec<ProviderAuthState>>> = Arc::new(Mutex::new(Vec::new()));
  let sink = auth_states.clone();
  provider.set_auth_observer(Some(Arc::new(move |state| {
    sink.lock().unwrap().push(state);
  })));
  hub.attach(provider.clone()).await.expect("the provider attached");
  Rig {
    hub,
    peer,
    provider,
    http,
    ws,
    auth_states,
  }
}

async fn boot_connected(initial: Cluster) -> Rig {
  let rig = boot(initial).await;
  rig.ws.wait_connected(1).await;
  rig.ws.open_session();
  rig
}

fn snapshot_of(msg: &GatewayToBridgeMsg) -> Option<libbridgething::PlayerState> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::Snapshot(state)) => Some(state.as_ref().clone()),
    _ => None,
  }
}

fn queue_of(msg: &GatewayToBridgeMsg) -> Option<QueueSnapshot> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::QueueChanged(queue)) => Some(queue.clone()),
    _ => None,
  }
}

fn targets_of(msg: &GatewayToBridgeMsg) -> Option<PlaybackTargets> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::TargetsChanged(targets)) => Some(targets.clone()),
    _ => None,
  }
}

fn claim_of(scope: CompanionAuthorityScope) -> impl Fn(&GatewayToBridgeMsg) -> Option<AuthorityClaim> {
  move |msg| match &msg.data {
    GatewayToBridgeMsgData::Authority(GatewayToBridgeAuthorityMsg::Claim(claim)) if claim.scope == scope => {
      Some(claim.clone())
    }
    _ => None,
  }
}

async fn await_auth(rig: &Rig, wanted: impl Fn(&ProviderAuthState) -> bool) -> ProviderAuthState {
  let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
  loop {
    if let Some(state) = rig.auth_states.lock().unwrap().iter().find(|state| wanted(state)) {
      return state.clone();
    }
    assert!(
      tokio::time::Instant::now() < deadline,
      "the auth state never arrived; saw {:?}",
      rig.auth_states.lock().unwrap()
    );
    tokio::time::sleep(Duration::from_millis(5)).await;
  }
}

// ---- now-playing + authority -------------------------------------------------

#[tokio::test]
async fn a_dealer_player_push_snapshots_and_claims_authority() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: true,
    ..ClusterSpec::default()
  }))
  .await;

  let state = rig.peer.wait("the player snapshot", snapshot_of).await;
  assert_eq!(state.track.as_ref().unwrap().title.as_deref(), Some("Song"));
  assert_eq!(state.playback.state, PlaybackState::Playing);
  assert!(
    state
      .track
      .as_ref()
      .unwrap()
      .artwork_id
      .as_deref()
      .unwrap()
      .starts_with("spotify/img/248/i"),
    "track art rides a spotify asset id"
  );

  let claim = rig
    .peer
    .wait(
      "the playback claim",
      claim_of(CompanionAuthorityScope::NowPlayingPlayback),
    )
    .await;
  assert_eq!(claim.app_bundle.as_deref(), Some("com.spotify.client"));
}

#[tokio::test]
async fn the_provided_saved_flag_drives_liked_without_a_contains_round_trip() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", true)),
    playing: true,
    ..ClusterSpec::default()
  }))
  .await;

  let state = rig.peer.wait("the player snapshot", snapshot_of).await;
  assert_eq!(state.track.as_ref().unwrap().liked, Some(true));
}

#[tokio::test]
async fn a_dealer_queue_push_sends_queue_changed() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    next: vec![provided_track("spotify:track:2", "Next", false)],
    playing: true,
    ..ClusterSpec::default()
  }))
  .await;

  let queue = rig.peer.wait("the queue push", queue_of).await;
  assert_eq!(queue.order, vec!["spotify:track:2".to_string()]);
  assert_eq!(queue.items[0].title.as_deref(), Some("Next"));
}

#[tokio::test]
async fn a_device_push_sends_targets_changed() {
  let rig = boot_connected(cluster(ClusterSpec {
    devices: vec![
      ("speaker", "Kitchen", DeviceType::SPEAKER, 26214),
      ("laptop", "Desk", DeviceType::COMPUTER, 0),
    ],
    active: "speaker",
    ..ClusterSpec::default()
  }))
  .await;

  let targets = rig.peer.wait("the targets push", targets_of).await;
  let mut ids: Vec<&str> = targets.targets.iter().map(|t| t.id.as_str()).collect();
  ids.sort();
  assert_eq!(ids, vec!["laptop", "speaker"]);
  let speaker = targets.targets.iter().find(|t| t.id == "speaker").unwrap();
  assert_eq!(speaker.kind, PlaybackTargetKind::Speaker);
  assert_eq!(speaker.volume_percent, Some(40));
  assert!(speaker.is_active);
  let laptop = targets.targets.iter().find(|t| t.id == "laptop").unwrap();
  assert_eq!(
    laptop.volume_percent, None,
    "an endpoint reporting no volume stays null"
  );
}

#[tokio::test]
async fn remote_playback_names_the_active_target_and_claims_volume() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: true,
    active: "speaker",
    devices: vec![("speaker", "Kitchen", DeviceType::SPEAKER, 32767)],
    ..ClusterSpec::default()
  }))
  .await;

  let state = rig
    .peer
    .wait("a snapshot naming the target", |msg| {
      snapshot_of(msg).filter(|state| state.target.is_some())
    })
    .await;
  assert_eq!(state.target.as_ref().unwrap().id, "speaker");
  assert_eq!(state.target.as_ref().unwrap().name, "Kitchen");
  rig
    .peer
    .wait("the volume claim", claim_of(CompanionAuthorityScope::Volume))
    .await;
  assert!(rig.provider.owns_volume().await, "remote playback owns volume");
}

#[tokio::test]
async fn local_playback_leaves_the_target_unset() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: true,
    active: "phone-1",
    devices: vec![
      ("phone-1", "Phone", DeviceType::SMARTPHONE, 0),
      ("speaker", "Kitchen", DeviceType::SPEAKER, 26214),
    ],
    ..ClusterSpec::default()
  }))
  .await;

  let state = rig.peer.wait("the snapshot", snapshot_of).await;
  assert_eq!(
    state.target.as_ref().map(|target| target.id.as_str()),
    Some("phone-1"),
    "the cluster names the phone as the active endpoint"
  );
  assert!(!rig.provider.owns_volume().await, "the phone is not a remote speaker");
  rig
    .peer
    .quiet(
      "a volume claim for phone-local playback",
      claim_of(CompanionAuthorityScope::Volume),
    )
    .await;
}

// ---- verbs -------------------------------------------------------------------

#[tokio::test]
async fn volume_verbs_step_the_remote_connect_device() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: true,
    active: "speaker",
    devices: vec![("speaker", "Kitchen", DeviceType::SPEAKER, 32767)],
    ..ClusterSpec::default()
  }))
  .await;
  rig
    .peer
    .wait("the volume claim", claim_of(CompanionAuthorityScope::Volume))
    .await;

  let level = rig.provider.volume_up().await.expect("the step landed");
  assert!((level - 0.5625).abs() < 0.01, "6.25 percent above half, got {level}");
  rig
    .http
    .wait_hit("/connect-state/v1/connect/volume/from/me-device/to/speaker")
    .await;
}

#[tokio::test]
async fn transfer_to_forwards_to_the_connect_api() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: true,
    ..ClusterSpec::default()
  }))
  .await;
  rig.peer.wait("the snapshot", snapshot_of).await;

  rig.provider.transfer_to("speaker").await.expect("the transfer landed");
  rig
    .http
    .wait_hit("/connect-state/v1/connect/transfer/from/me-device/to/speaker")
    .await;
}

#[tokio::test]
async fn queue_forwards_every_wire_position_to_the_client() {
  use bridgething_companion::provider::PlayerTransport;
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: true,
    active: "phone-1",
    devices: vec![("phone-1", "Phone", DeviceType::SMARTPHONE, 0)],
    ..ClusterSpec::default()
  }))
  .await;
  rig.peer.wait("the snapshot", snapshot_of).await;

  rig
    .provider
    .queue(libbridgething::gateway::QueueUri {
      uri: "spotify:track:a".into(),
      position: libbridgething::QueuePosition::Append,
    })
    .await
    .expect("the append landed");
  let (_, body) = rig.http.wait_hit("/player/command/from/me-device/to/phone-1").await;
  let body = String::from_utf8(body).unwrap();
  assert!(
    body.contains(r#""endpoint":"add_to_queue""#),
    "append is add_to_queue: {body}"
  );
  assert!(body.contains("spotify:track:a"));
}

#[tokio::test]
async fn skip_to_index_replays_the_context_and_skips_to_the_queued_uri() {
  use bridgething_companion::provider::PlayerTransport;
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    next: vec![
      provided_track("spotify:track:2", "Next", false),
      provided_track("spotify:track:3", "After", false),
    ],
    playing: true,
    active: "phone-1",
    devices: vec![("phone-1", "Phone", DeviceType::SMARTPHONE, 0)],
    ..ClusterSpec::default()
  }))
  .await;
  rig.peer.wait("the queue push", queue_of).await;

  rig.provider.skip_to_index(1).await.expect("the skip landed");
  let (_, body) = rig.http.wait_hit("/player/command/from/me-device/to/phone-1").await;
  let body = String::from_utf8(body).unwrap();
  assert!(
    body.contains(r#""endpoint":"play""#),
    "skipToIndex replays via play: {body}"
  );
  assert!(body.contains("spotify:playlist:1"), "the context rides along: {body}");
  assert!(
    body.contains("spotify:track:3"),
    "the queued uri is the skip target: {body}"
  );
}

#[tokio::test]
async fn skip_to_index_out_of_range_refuses_without_a_play() {
  use bridgething_companion::provider::PlayerTransport;
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    next: vec![provided_track("spotify:track:2", "Next", false)],
    playing: true,
    active: "phone-1",
    devices: vec![("phone-1", "Phone", DeviceType::SMARTPHONE, 0)],
    ..ClusterSpec::default()
  }))
  .await;
  rig.peer.wait("the queue push", queue_of).await;

  rig
    .provider
    .skip_to_index(9)
    .await
    .expect_err("an out-of-range index refuses");
  assert!(
    rig.http.hits_matching("player/command").is_empty(),
    "no play was issued"
  );
}

// ---- peer reconnect ----------------------------------------------------------

#[tokio::test]
async fn peer_reconnect_replays_the_fresh_position_not_the_stale_zero() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: true,
    position_ms: 0,
    ..ClusterSpec::default()
  }))
  .await;
  rig.peer.wait("the first snapshot", snapshot_of).await;

  rig.ws.push_cluster(&cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: false,
    position_ms: 90_000,
    ..ClusterSpec::default()
  }));
  rig
    .peer
    .wait("the paused snapshot", |msg| {
      snapshot_of(msg).filter(|state| state.playback.position_ms == 90_000)
    })
    .await;

  rig.provider.handle_peer_connected(false).await;
  rig
    .peer
    .wait_for("the replayed fresh position", 2, |msg| {
      snapshot_of(msg).filter(|state| state.playback.position_ms == 90_000)
    })
    .await;
}

#[tokio::test]
async fn peer_reconnect_resends_the_held_queue_with_no_track_change() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    next: vec![provided_track("spotify:track:2", "Next", false)],
    playing: true,
    ..ClusterSpec::default()
  }))
  .await;
  rig.peer.wait("the first queue push", queue_of).await;

  rig.provider.handle_peer_connected(false).await;
  rig.peer.wait_for("the re-synced queue", 2, queue_of).await;
}

#[tokio::test]
async fn an_aggressive_connect_runs_connect_resume() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: false,
    active: "phone-1",
    devices: vec![("phone-1", "Phone", DeviceType::SMARTPHONE, 0)],
    ..ClusterSpec::default()
  }))
  .await;
  rig.peer.wait("the snapshot", snapshot_of).await;

  rig.provider.handle_peer_connected(true).await;
  let (_, body) = rig.http.wait_hit("/player/command/from/me-device/to/phone-1").await;
  assert!(
    String::from_utf8(body).unwrap().contains(r#""endpoint":"resume""#),
    "connect resume resumes the parked phone session"
  );
}

#[tokio::test]
async fn a_non_aggressive_connect_never_resumes() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: false,
    active: "phone-1",
    devices: vec![("phone-1", "Phone", DeviceType::SMARTPHONE, 0)],
    ..ClusterSpec::default()
  }))
  .await;
  rig.peer.wait("the snapshot", snapshot_of).await;

  rig.provider.handle_peer_connected(false).await;
  tokio::time::sleep(Duration::from_millis(300)).await;
  assert!(
    rig.http.hits_matching("player/command").is_empty(),
    "non-aggressive connect must not reconcile playback"
  );
}

// ---- liked -------------------------------------------------------------------

#[tokio::test]
async fn a_favorites_set_reemits_the_current_snapshot_with_a_position_age() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: false,
    ..ClusterSpec::default()
  }))
  .await;
  let first = rig.peer.wait("the first snapshot", snapshot_of).await;
  assert_eq!(
    first.playback.position_age_ms, None,
    "a live dealer emit carries no age"
  );
  assert_eq!(first.track.as_ref().unwrap().liked, Some(false));

  rig
    .provider
    .favorites_set(
      ItemRef {
        uri: "spotify:track:1".into(),
        kind: ItemKind::Track,
        persistent_id: None,
      },
      true,
    )
    .await
    .expect("the write landed");

  rig.http.wait_hit("collection/v2/write").await;
  let reemitted = rig
    .peer
    .wait("the liked re-emit", |msg| {
      snapshot_of(msg).filter(|state| state.track.as_ref().is_some_and(|track| track.liked == Some(true)))
    })
    .await;
  assert!(
    reemitted.playback.position_age_ms.is_some(),
    "a cached replay stamps its age"
  );
}

#[tokio::test]
async fn favorites_contains_reads_the_liked_cache() {
  let rig = boot_connected(cluster(ClusterSpec::default())).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;
  let liked = rig
    .provider
    .favorites_contains(LibraryFavoritesContainsRequest {
      uris: vec!["spotify:track:1".into()],
    })
    .await
    .expect("the contains resolved");
  assert_eq!(liked, vec![false]);
}

// ---- auth lifecycle ----------------------------------------------------------

#[tokio::test]
async fn the_premium_gate_surfaces_an_auth_failure() {
  let rig = boot_full(
    cluster(ClusterSpec::default()),
    MemorySecrets::paired(),
    Some(r#"{"product":"free","catalogue":"free","country":"US"}"#),
  )
  .await;
  await_auth(
    &rig,
    |state| matches!(state, ProviderAuthState::Failed { reason } if reason == "Spotify Premium is required"),
  )
  .await;
}

#[tokio::test]
async fn a_device_flow_pending_surfaces_the_user_code() {
  let store = Arc::new(MemorySecrets::default());
  let rig = boot_with(cluster(ClusterSpec::default()), store).await;
  let pending = await_auth(&rig, |state| {
    matches!(state, ProviderAuthState::Pending { user_code: Some(_), .. })
  })
  .await;
  match pending {
    ProviderAuthState::Pending {
      user_code,
      verification_url,
      ..
    } => {
      assert_eq!(user_code.as_deref(), Some("ABCD"));
      assert_eq!(verification_url.as_deref(), Some("https://spotify.com/pair"));
    }
    other => panic!("expected pending, got {other:?}"),
  }
}

#[tokio::test]
async fn a_library_change_relays_to_the_gateway_after_the_debounce() {
  let rig = boot_connected(cluster(ClusterSpec::default())).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;
  tokio::time::sleep(Duration::from_millis(50)).await;
  rig.ws.push_library_changed();
  rig
    .peer
    .wait("the library-changed relay", |msg| match &msg.data {
      GatewayToBridgeMsgData::Library(GatewayToBridgeLibraryMsg::LibraryChanged(changed)) => Some(changed.scope),
      _ => None,
    })
    .await;
}

// ---- connectivity ------------------------------------------------------------

#[tokio::test]
async fn only_the_connectivity_restored_edge_resyncs_and_only_once() {
  let rig = boot_connected(cluster(ClusterSpec::default())).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  rig.provider.connectivity_changed(true).await;
  tokio::time::sleep(Duration::from_millis(50)).await;
  assert_eq!(
    *rig.ws.connects.lock().unwrap(),
    1,
    "an initial already-available callback must not resync"
  );

  rig.provider.connectivity_changed(false).await;
  rig.provider.connectivity_changed(true).await;
  rig.ws.wait_connected(2).await;
  tokio::time::sleep(Duration::from_millis(50)).await;
  assert_eq!(
    *rig.ws.connects.lock().unwrap(),
    2,
    "the restored edge resyncs exactly once"
  );
}

// ---- attachment --------------------------------------------------------------

#[tokio::test]
async fn detach_clears_the_source_and_the_capabilities_shrink() {
  let rig = boot_connected(cluster(ClusterSpec {
    track: Some(provided_track("spotify:track:1", "Song", false)),
    playing: true,
    ..ClusterSpec::default()
  }))
  .await;
  rig.peer.wait("the snapshot", snapshot_of).await;
  assert_eq!(rig.hub.attached_schemes(), vec!["spotify".to_string()]);

  rig.hub.detach("spotify").await;
  assert!(rig.hub.attached_schemes().is_empty());
  assert!(rig.hub.library().is_none());
  assert!(
    eventually(|| rig.hub.now_playing().current_source().is_none()).await,
    "the audible source cleared"
  );
}

// ---- artwork -----------------------------------------------------------------

#[tokio::test]
async fn repeat_asset_requests_fetch_and_scale_once() {
  let probe = ArtProbe::new();
  let scaler = TagScaler::new();
  let provider = SpotifyProvider::new(
    SpotifyConfig {
      worker_base: "https://worker.test/auth".into(),
      psk: "psk".into(),
      device_id: "me-device".into(),
    },
    probe.clone(),
    FakeWs::new(),
    MemorySecrets::paired(),
    Some(scaler.clone()),
    None,
  );

  let thumb = provider
    .asset(&format!("spotify/img/96/i{IMAGE_HEX}"))
    .await
    .expect("the asset resolved")
    .expect("art came back");
  let again = provider
    .asset(&format!("spotify/img/96/i{IMAGE_HEX}"))
    .await
    .expect("the asset resolved")
    .expect("art came back");
  assert_eq!(thumb, again);
  assert_eq!(probe.fetches(), 1, "the master is fetched once");
  assert_eq!(scaler.scales(), 1, "the downsample runs once");

  let hero = provider
    .asset(&format!("spotify/img/248/i{IMAGE_HEX}"))
    .await
    .expect("the asset resolved")
    .expect("art came back");
  assert_ne!(hero, thumb);
  assert_eq!(probe.fetches(), 1, "a second edge reuses the cached master");
  assert_eq!(scaler.scales(), 2, "a second edge earns its own downsample");
}

// ---- session registration ----------------------------------------------------

fn catalog_session(secrets: Arc<MemorySecrets>) -> Arc<Session> {
  Session::new(
    CompanionConfig {
      host: HostInfo {
        app_name: "toctou".into(),
        app_version: "0.0.0".into(),
        os_name: "linux".into(),
        os_version: "0".into(),
        host_identifier: "toctou".into(),
      },
      capabilities: CapabilityFlags {
        geo: false,
        notifications: false,
        net_fetch: false,
        net_ws: false,
        audio_tts: false,
        voice_model: false,
      },
      state_dir: std::env::temp_dir().to_string_lossy().into_owned(),
      cache_dir: std::env::temp_dir().to_string_lossy().into_owned(),
      model_platform: Some(ModelPlatform::Ios),
      spotify: Some(SpotifyProviderConfig {
        worker_base: "https://worker.test/auth".into(),
        psk: "psk".into(),
      }),
    },
    CompanionBackends {
      link: None,
      host: Arc::new(backends::RigHost),
      http: Arc::new(backends::Offline),
      ws: Arc::new(backends::Offline),
      secrets,
      log: Arc::new(log_sink::Quiet),
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
      model_validator: Some(Arc::new(NoopValidator)),
      transfer_policy: Some(Arc::new(Unmetered)),
      connectivity: None,
      device_waker: None,
      extensions: None,
    },
    Arc::new(backends::Heard::default()),
    Arc::new(backends::Offline),
  )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_connects_register_one_provider() {
  let secrets = MemorySecrets::paired();
  secrets.stall_device_id(Duration::from_millis(120));
  let session = catalog_session(secrets.clone());

  let left = session.clone();
  let right = session.clone();
  let (first, second) = tokio::join!(
    tokio::spawn(async move { left.connect_provider("spotify").await }),
    tokio::spawn(async move { right.connect_provider("spotify").await })
  );
  first.expect("the first connect ran").expect("and attached");
  second.expect("the second connect ran").expect("and attached");

  assert_eq!(
    secrets.builds(),
    1,
    "a racing pair of connects must share one provider, not build two"
  );
  let providers = session.snapshot().await.providers;
  assert_eq!(providers.len(), 1);
  assert!(providers[0].connected, "and the survivor is the one that attached");
}

struct NoopValidator;

impl ModelArtifactValidator for NoopValidator {
  fn validate(&self, _kind: ModelArtifactKind, _path: String) -> Result<(), ModelValidationError> {
    Ok(())
  }
}

struct Unmetered;

impl TransferPolicy for Unmetered {
  fn allows_large_transfer(&self) -> bool {
    true
  }
}

#[test]
fn constructing_a_session_outside_a_runtime_does_not_panic() {
  let session = catalog_session(MemorySecrets::paired());
  assert_eq!(session.provider_infos().len(), 1);
}
