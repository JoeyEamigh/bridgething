#[path = "rig/backends.rs"]
mod backends;
#[path = "rig/icecast.rs"]
mod icecast;
#[path = "rig/log_sink.rs"]
mod log_sink;
#[path = "support/poll.rs"]
mod poll;
#[path = "rig/secrets.rs"]
mod secrets;
#[path = "rig/stream.rs"]
mod stream_fake;
#[path = "dispatch/support.rs"]
mod support;

use std::{sync::Arc, time::Duration};

use backends::{Heard, Offline, RigHost};
use bridgething_companion::{
  api::{CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo},
  backend::{
    ForeignHttp, ImageScaler, NativeHttp, StreamMetadata, StreamPresentation, StreamStatus, StreamTiming,
    net::{HttpDownloadSink, HttpHeader, HttpRequest, HttpSink, HttpTransport},
  },
  hub::Hub,
  provider::{PlayerTransport, Provider, ProviderError, ProviderRegistry, stream::StreamProvider},
  session::Session,
};
use icecast::{Icecast, Station, authority, fetch};
use libbridgething::{
  PlayContext, PlaybackState, PlayerError,
  gateway::{GatewayToBridgeMsg, GatewayToBridgeMsgData, GatewayToBridgePlayerMsg, PlayUri},
};
use log_sink::Quiet;
use poll::eventually;
use secrets::MemorySecrets;
use stream_fake::{APP_BUNDLE, FakeStreamBackend, StreamCall};
use support::Peer;

const URL: &str = "https://radio.example/live";
const STATION: &str = "Groove Salad";

struct IcyOrigin {
  delay: Duration,
}

impl IcyOrigin {
  fn new(delay: Duration) -> Arc<Self> {
    Arc::new(Self { delay })
  }
}

impl HttpTransport for IcyOrigin {
  fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
    sink.fail("the icy origin only streams".into());
  }

  fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    assert!(
      request
        .headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("icy-metadata") && header.value == "1"),
      "the probe asks the origin for icy metadata"
    );
    let delay = self.delay;
    tokio::spawn(async move {
      tokio::time::sleep(delay).await;
      let headers = vec![
        HttpHeader {
          name: "icy-name".into(),
          value: STATION.into(),
        },
        HttpHeader {
          name: "Content-Length".into(),
          value: "1073741824".into(),
        },
      ];
      let wanted = sink.on_response(200, headers, Some(1_073_741_824));
      assert!(
        !wanted,
        "an origin without icy-metaint is not relayed, so the body is refused"
      );
      sink.on_finished();
    });
  }
}

fn snapshot_of(msg: &GatewayToBridgeMsg) -> Option<libbridgething::PlayerState> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::Snapshot(state)) => Some(state.as_ref().clone()),
    _ => None,
  }
}

fn error_of(msg: &GatewayToBridgeMsg) -> Option<PlayerError> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::ErrorEvent(reply)) => Some(reply.error.clone()),
    _ => None,
  }
}

struct Rig {
  hub: Arc<Hub>,
  peer: Peer,
  provider: Arc<StreamProvider>,
  backend: Arc<FakeStreamBackend>,
}

impl Rig {
  async fn play(&self, url: &str) {
    PlayerTransport::play(
      self.provider.as_ref(),
      PlayUri {
        uri: url.into(),
        context: None,
      },
    )
    .await
    .expect("play routes to the stream provider");
  }

  async fn playing_snapshot(&self) -> libbridgething::PlayerState {
    self
      .peer
      .wait("the playing snapshot", |msg| {
        snapshot_of(msg).filter(|state| state.playback.state == PlaybackState::Playing)
      })
      .await
  }
}

struct TagScaler;

impl ImageScaler for TagScaler {
  fn downsample_jpeg(&self, bytes: Vec<u8>, max_edge: u32, _quality: f32) -> Option<Vec<u8>> {
    Some(format!("{}@{max_edge}", String::from_utf8_lossy(&bytes)).into_bytes())
  }
}

fn title_of(msg: &GatewayToBridgeMsg) -> Option<String> {
  snapshot_of(msg)?.track?.title
}

async fn boot() -> Rig {
  boot_with(Arc::new(Offline)).await
}

async fn boot_with(http: Arc<dyn HttpTransport>) -> Rig {
  boot_with_scaler(http, None).await
}

async fn boot_with_scaler(http: Arc<dyn HttpTransport>, scaler: Option<Arc<dyn ImageScaler>>) -> Rig {
  let (gateway, peer) = Peer::link();
  let hub = Hub::new(
    Arc::new(gateway),
    HostInfo {
      app_name: "stream-test".into(),
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
    false,
  );
  hub.start();
  let backend = FakeStreamBackend::new();
  let provider = StreamProvider::new(backend.clone(), Arc::new(ForeignHttp::new(http)), scaler);
  hub
    .attach(provider.clone())
    .await
    .expect("the stream provider attached");
  Rig {
    hub,
    peer,
    provider,
    backend,
  }
}

#[tokio::test]
async fn claims_http_and_https_but_nothing_else() {
  let rig = boot().await;
  assert_eq!(
    rig
      .hub
      .for_uri("http://radio.example/live")
      .map(|p| p.name().to_owned()),
    Some("stream".to_owned())
  );
  assert_eq!(
    rig.hub.for_uri(URL).map(|p| p.name().to_owned()),
    Some("stream".to_owned())
  );
  assert!(rig.hub.for_uri("spotify:track:abc").is_none());
  assert_eq!(rig.hub.provider_app_bundles(), vec![APP_BUNDLE.to_owned()]);
}

#[tokio::test]
async fn play_hands_the_url_to_the_phone_and_publishes_a_buffering_track() {
  let rig = boot().await;
  rig.play(URL).await;

  assert_eq!(rig.backend.calls(), vec![StreamCall::Play(URL.into())]);

  let state = rig.playing_snapshot().await;
  let track = state.track.as_ref().expect("a synthetic track");
  assert_eq!(track.uri.as_deref(), Some(URL));
  assert_eq!(track.title.as_deref(), Some("radio.example"));
  assert_eq!(track.artwork_id, None);
  assert_eq!(state.context, None);
  assert_eq!(state.playback.set_elapsed_time_available, Some(false));
  assert_eq!(rig.hub.now_playing().current_source().as_deref(), Some("stream"));
}

#[tokio::test]
async fn the_play_context_is_reported_as_the_playback_context() {
  let rig = boot().await;
  PlayerTransport::play(
    rig.provider.as_ref(),
    PlayUri {
      uri: URL.into(),
      context: Some(PlayContext {
        context_uri: "radio-atlas:station:abc".into(),
      }),
    },
  )
  .await
  .expect("play routes to the stream provider");

  let state = rig.playing_snapshot().await;
  let context = state.context.as_ref().expect("the play context is reported");
  assert_eq!(context.uri, "radio-atlas:station:abc");
  assert_eq!(context.name, None);
}

#[tokio::test]
async fn an_offline_probe_hands_the_phone_a_plain_url() {
  let rig = boot().await;
  rig.play(URL).await;
  let source = rig.backend.last_source().expect("the phone got a source");
  assert!(!source.live);
  assert_eq!(source.station, None);
}

#[tokio::test]
async fn an_icy_origin_marks_the_stream_live_and_names_the_station() {
  let rig = boot_with(IcyOrigin::new(Duration::ZERO)).await;
  rig.play(URL).await;
  let source = rig.backend.last_source().expect("the phone got a source");
  assert!(source.live, "icy headers mean live radio");
  assert_eq!(source.station.as_deref(), Some(STATION));
  assert_eq!(
    source.url, URL,
    "without icy-metaint the phone plays the origin directly"
  );

  let named = rig
    .peer
    .wait("the station snapshot", |msg| {
      snapshot_of(msg).filter(|state| {
        state
          .track
          .as_ref()
          .is_some_and(|track| track.title.as_deref() == Some(STATION))
      })
    })
    .await;
  assert_eq!(named.playback.set_elapsed_time_available, Some(false));

  let sink = rig.backend.last_sink().expect("the backend kept its sink");
  sink.on_timing(StreamTiming {
    position_ms: 2_000,
    duration_ms: Some(67_102_302),
    seekable: true,
  });
  let timed = rig
    .peer
    .wait("the timed snapshot", |msg| {
      snapshot_of(msg).filter(|state| state.playback.position_ms == 2_000)
    })
    .await;
  assert_eq!(
    timed.track.as_ref().and_then(|track| track.duration_ms),
    None,
    "a fabricated duration never reaches the device for live radio"
  );
  assert_eq!(timed.playback.set_elapsed_time_available, Some(false));
  assert!(matches!(
    rig.provider.seek_to(5_000).await,
    Err(ProviderError::NotImplemented)
  ));

  sink.on_metadata(StreamMetadata {
    title: Some("Blue in Green".into()),
    ..Default::default()
  });
  rig
    .peer
    .wait("the titled snapshot", |msg| {
      snapshot_of(msg).filter(|state| {
        state
          .track
          .as_ref()
          .is_some_and(|track| track.title.as_deref() == Some("Blue in Green"))
      })
    })
    .await;
}

#[tokio::test]
async fn a_play_that_lands_during_the_probe_wins() {
  let rig = boot_with(IcyOrigin::new(Duration::from_millis(200))).await;
  let first = rig.play(URL);
  let second = rig.play("https://other.example/pop");
  tokio::join!(first, second);
  assert_eq!(
    rig
      .backend
      .calls()
      .into_iter()
      .filter(|call| matches!(call, StreamCall::Play(_)))
      .collect::<Vec<_>>(),
    vec![StreamCall::Play("https://other.example/pop".into())],
    "the superseded play never reaches the phone"
  );
}

#[tokio::test]
async fn the_backend_status_drives_playback_state_without_a_verb() {
  let rig = boot().await;
  rig.play(URL).await;
  rig.playing_snapshot().await;
  let sink = rig.backend.last_sink().expect("the backend kept its sink");

  sink.on_status(StreamStatus::Paused);
  rig
    .peer
    .wait("the paused snapshot", |msg| {
      snapshot_of(msg).filter(|state| state.playback.state == PlaybackState::Paused)
    })
    .await;
  assert!(!rig.backend.calls().contains(&StreamCall::Pause));

  sink.on_status(StreamStatus::Playing);
  assert!(
    eventually(|| {
      let playing = rig
        .peer
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(snapshot_of)
        .filter(|state| state.playback.state == PlaybackState::Playing)
        .count();
      playing >= 2 && rig.hub.now_playing().current_source().as_deref() == Some("stream")
    })
    .await
  );
}

#[tokio::test]
async fn pause_and_resume_only_ask_the_phone() {
  let rig = boot().await;
  rig.play(URL).await;
  rig.provider.pause().await.expect("pause");
  rig.provider.resume().await.expect("resume");
  assert_eq!(
    rig.backend.transport_calls(),
    vec![StreamCall::Play(URL.into()), StreamCall::Pause, StreamCall::Resume]
  );
}

#[tokio::test]
async fn seeking_is_refused_until_the_phone_reports_seekable_media() {
  let rig = boot().await;
  rig.play(URL).await;
  assert!(matches!(
    rig.provider.seek_to(5_000).await,
    Err(ProviderError::NotImplemented)
  ));
  rig
    .backend
    .last_sink()
    .expect("the backend kept its sink")
    .on_timing(StreamTiming {
      position_ms: 1_000,
      duration_ms: Some(90_000),
      seekable: true,
    });
  let seekable = rig
    .peer
    .wait("the seekable snapshot", |msg| {
      snapshot_of(msg).filter(|state| state.playback.set_elapsed_time_available == Some(true))
    })
    .await;
  assert_eq!(seekable.playback.position_ms, 1_000);
  assert_eq!(
    seekable.track.as_ref().and_then(|track| track.duration_ms),
    Some(90_000)
  );
  rig.provider.seek_to(5_000).await.expect("seek");
  assert!(rig.backend.calls().contains(&StreamCall::SeekTo(5_000)));
}

#[tokio::test]
async fn metadata_from_the_phone_becomes_the_track() {
  let rig = boot().await;
  rig.play(URL).await;
  rig
    .backend
    .last_sink()
    .expect("the backend kept its sink")
    .on_metadata(StreamMetadata {
      title: Some("Blue in Green".into()),
      artist: Some("Miles Davis".into()),
      album: None,
      artwork_url: Some("https://radio.example/art.jpg".into()),
      artwork: None,
    });
  let state = rig
    .peer
    .wait("the titled snapshot", |msg| {
      snapshot_of(msg).filter(|state| {
        state
          .track
          .as_ref()
          .is_some_and(|track| track.title.as_deref() == Some("Blue in Green"))
      })
    })
    .await;
  let track = state.track.expect("a track");
  assert_eq!(track.artist.as_deref(), Some("Miles Davis"));
  let artwork = track.artwork_id.expect("an artwork id");
  assert!(artwork.starts_with("stream/img/"), "{artwork}");
  assert!(
    rig
      .provider
      .asset(&artwork)
      .await
      .expect("the asset path answers")
      .is_none(),
    "an offline host has no bytes to serve"
  );
}

#[tokio::test]
async fn a_failed_stream_clears_the_source_and_reports_the_failure() {
  let rig = boot().await;
  rig.play(URL).await;
  rig.playing_snapshot().await;
  rig
    .backend
    .last_sink()
    .expect("the backend kept its sink")
    .on_status(StreamStatus::Failed { reason: "404".into() });
  assert!(eventually(|| rig.hub.now_playing().current_source().is_none()).await);
  let error = rig.peer.wait("the play failure", error_of).await;
  assert_eq!(error, PlayerError::PlayFailed { reason: "404".into() });
}

#[tokio::test]
async fn an_ended_stream_clears_the_source() {
  let rig = boot().await;
  rig.play(URL).await;
  rig.playing_snapshot().await;
  rig
    .backend
    .last_sink()
    .expect("the backend kept its sink")
    .on_status(StreamStatus::Ended);
  assert!(eventually(|| rig.hub.now_playing().current_source().is_none()).await);
}

#[tokio::test]
async fn playing_a_second_url_stops_the_first() {
  let rig = boot().await;
  rig.play(URL).await;
  rig.play("https://other.example/pop").await;
  assert_eq!(
    rig.backend.transport_calls(),
    vec![
      StreamCall::Play(URL.into()),
      StreamCall::Stop,
      StreamCall::Play("https://other.example/pop".into()),
    ]
  );
}

#[tokio::test]
async fn the_last_peer_leaving_leaves_the_stream_playing() {
  let rig = boot().await;
  rig.hub.peer_connected("car-1").await;
  rig.play(URL).await;
  rig.playing_snapshot().await;

  rig.hub.peer_disconnected("car-1");
  tokio::time::sleep(Duration::from_millis(100)).await;
  assert!(
    !rig.backend.calls().contains(&StreamCall::Stop),
    "the phone owns the stream; the car leaving is not a stop"
  );
  assert_eq!(rig.hub.now_playing().current_source().as_deref(), Some("stream"));

  rig.hub.peer_connected("car-1").await;
  assert_eq!(rig.hub.now_playing().current_source().as_deref(), Some("stream"));
}

#[tokio::test(flavor = "multi_thread")]
async fn session_start_attaches_the_stream_provider_with_the_host_bundle() {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let backend = FakeStreamBackend::new();
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
    stream: Some(backend),
    speech: None,
    nlu: None,
    apple_music: None,
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
        app_name: "stream-session-test".into(),
        app_version: "0.0.0".into(),
        os_name: "linux".into(),
        os_version: String::new(),
        host_identifier: String::new(),
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
  session.start();
  assert!(
    eventually(|| {
      session
        .hub()
        .for_uri(URL)
        .is_some_and(|provider| provider.name() == "stream")
        && session.hub().provider_app_bundles() == vec![APP_BUNDLE.to_owned()]
    })
    .await
  );
  session.stop().await;
}

fn tone(len: usize) -> Vec<u8> {
  (0..len).map(|index| (index * 7 % 251) as u8).collect()
}

fn groove(trickle: bool) -> Station {
  Station {
    name: "Test Radio",
    content_type: "audio/mpeg",
    metaint: 64,
    audio: tone(1_000),
    titles: vec![Some("first"), None, Some("second")],
    trickle,
    logo: None,
  }
}

async fn boot_native() -> Rig {
  boot_with(Arc::new(NativeHttp::default())).await
}

async fn relayed_source(rig: &Rig, url: &str) -> bridgething_companion::backend::StreamSource {
  rig.play(url).await;
  let source = rig.backend.last_source().expect("the phone got a source");
  assert!(
    source.url.starts_with("http://127.0.0.1:"),
    "a metaint origin is relayed through loopback, got {}",
    source.url
  );
  assert!(source.live);
  source
}

#[tokio::test]
async fn a_metaint_origin_is_relayed_clean_and_its_titles_reach_the_wire() {
  let origin = Icecast::serve(groove(false));
  let rig = boot_native().await;
  let source = relayed_source(&rig, &origin.url).await;
  assert_eq!(source.station.as_deref(), Some("Test Radio"));
  assert!(
    origin
      .requests()
      .iter()
      .all(|request| request.to_ascii_lowercase().contains("icy-metadata: 1")),
    "the relay asks the origin for icy metadata"
  );

  rig
    .peer
    .wait("the first icy title", |msg| {
      title_of(msg).filter(|title| title == "first")
    })
    .await;
  rig
    .peer
    .wait("the second icy title", |msg| {
      title_of(msg).filter(|title| title == "second")
    })
    .await;
  let titles: Vec<String> = rig.peer.seen.lock().unwrap().iter().filter_map(title_of).collect();
  let first = titles.iter().position(|title| title == "first").unwrap();
  let second = titles.iter().position(|title| title == "second").unwrap();
  assert!(first < second, "titles land in stream order: {titles:?}");

  let served = fetch(&source.url, &[]).await.expect("the relay answers a player");
  assert_eq!(served.status, 200);
  assert_eq!(served.header("content-type"), Some("audio/mpeg"));
  assert_eq!(served.header("transfer-encoding"), Some("chunked"));
  assert_eq!(served.header("accept-ranges"), Some("none"));
  assert_eq!(served.header("content-length"), None);
  assert_eq!(
    served.body, origin.audio,
    "the player hears the audio with every metadata block removed"
  );

  let before = rig.peer.seen.lock().unwrap().len();
  let sink = rig.backend.last_sink().expect("the backend kept its sink");
  sink.on_metadata(StreamMetadata {
    title: Some("Test Radio".into()),
    ..Default::default()
  });
  assert!(eventually(|| rig.peer.seen.lock().unwrap().len() > before).await);
  let latest = rig.peer.seen.lock().unwrap().iter().rev().find_map(title_of);
  assert_eq!(
    latest.as_deref(),
    Some("second"),
    "the icy title outranks whatever the phone reports"
  );
}

#[tokio::test]
async fn a_range_probe_and_a_second_player_both_get_the_whole_live_stream() {
  let origin = Icecast::serve(groove(false));
  let rig = boot_native().await;
  let source = relayed_source(&rig, &origin.url).await;

  let (probe, real) = tokio::join!(fetch(&source.url, &[("Range", "bytes=0-1")]), fetch(&source.url, &[]));
  let probe = probe.expect("the range probe is answered");
  assert_eq!(
    probe.status, 200,
    "a range request is answered with the live stream, never a partial"
  );
  assert_eq!(probe.body, origin.audio);
  assert_eq!(real.expect("the second connection is answered").body, origin.audio);
}

#[tokio::test]
async fn a_player_with_the_wrong_token_is_refused() {
  let origin = Icecast::serve(groove(false));
  let rig = boot_native().await;
  let source = relayed_source(&rig, &origin.url).await;
  let wrong = format!("http://{}/not-the-token", authority(&source.url));
  let refused = fetch(&wrong, &[]).await.expect("the relay answers");
  assert_eq!(refused.status, 404);
  assert!(refused.body.is_empty());
}

#[tokio::test]
async fn the_relay_outlives_the_peer_and_stop_hangs_up_on_the_origin() {
  let origin = Icecast::serve(groove(true));
  let rig = boot_native().await;
  rig.hub.peer_connected("car-1").await;
  let source = relayed_source(&rig, &origin.url).await;
  assert_eq!(origin.closed(), 0);
  let port = authority(&source.url).to_owned();

  rig.hub.peer_disconnected("car-1");
  tokio::time::sleep(Duration::from_millis(200)).await;
  assert!(!rig.backend.calls().contains(&StreamCall::Stop));
  assert!(
    std::net::TcpStream::connect(&port).is_ok(),
    "the relay keeps serving after the car leaves"
  );
  assert_eq!(origin.closed(), 0);

  rig.provider.detach().await;
  assert!(rig.backend.calls().contains(&StreamCall::Stop));
  assert!(
    eventually(|| std::net::TcpStream::connect(&port).is_err()).await,
    "the relay port closes with the stream"
  );
  assert!(
    eventually(|| origin.closed() == 1).await,
    "the origin sees its connection dropped once the relay stops"
  );
}

#[tokio::test]
async fn a_superseded_play_never_serves_the_old_stream() {
  let first = Icecast::serve(groove(true));
  let second = Icecast::serve(Station {
    name: "Other Radio",
    audio: tone(500),
    titles: vec![Some("elsewhere")],
    ..groove(false)
  });
  let rig = boot_native().await;
  let stale = relayed_source(&rig, &first.url).await;
  let fresh = relayed_source(&rig, &second.url).await;
  assert_ne!(stale.url, fresh.url, "every playback gets its own port and token");

  let stale_port = authority(&stale.url).to_owned();
  assert!(eventually(|| std::net::TcpStream::connect(&stale_port).is_err()).await);
  assert!(eventually(|| first.closed() == 1).await);
  assert_eq!(
    fetch(&fresh.url, &[]).await.expect("the new relay serves").body,
    second.audio
  );
  rig
    .peer
    .wait("the new station's title", |msg| {
      title_of(msg).filter(|title| title == "elsewhere")
    })
    .await;
}

#[tokio::test]
async fn artwork_bytes_from_the_phone_become_a_scaled_asset() {
  let rig = boot_with_scaler(Arc::new(Offline), Some(Arc::new(TagScaler))).await;
  rig.play(URL).await;
  rig
    .backend
    .last_sink()
    .expect("the backend kept its sink")
    .on_metadata(StreamMetadata {
      title: Some("Blue in Green".into()),
      artwork: Some(b"cover".to_vec()),
      ..Default::default()
    });
  let state = rig
    .peer
    .wait("the artful snapshot", |msg| {
      snapshot_of(msg).filter(|state| state.track.as_ref().is_some_and(|track| track.artwork_id.is_some()))
    })
    .await;
  let artwork = state.track.and_then(|track| track.artwork_id).expect("an artwork id");
  assert!(artwork.starts_with("stream/img/248/"), "{artwork}");
  let asset = rig
    .provider
    .asset(&artwork)
    .await
    .expect("the asset path answers")
    .expect("embedded bytes are served without any fetch");
  assert_eq!(asset.bytes, b"cover@248");
  assert_eq!(asset.mime.as_deref(), Some("image/jpeg"));
}

#[tokio::test]
async fn the_phone_is_shown_exactly_what_the_wire_shows() {
  let origin = Icecast::serve(Station {
    titles: vec![Some("first")],
    logo: Some(b"logo-bytes"),
    ..groove(true)
  });
  let rig = boot_with_scaler(Arc::new(NativeHttp::default()), Some(Arc::new(TagScaler))).await;
  relayed_source(&rig, &origin.url).await;

  assert!(
    eventually(|| {
      rig
        .backend
        .presentations()
        .iter()
        .any(|shown| shown.title == "first" && shown.artwork.as_deref() == Some(b"logo-bytes".as_slice()))
    })
    .await,
    "the phone never saw the icy title with the station logo: {:?}",
    rig.backend.presentations()
  );

  let state = rig
    .peer
    .wait("the artful snapshot", |msg| {
      snapshot_of(msg).filter(|state| state.track.as_ref().is_some_and(|track| track.artwork_id.is_some()))
    })
    .await;
  let artwork = state.track.and_then(|track| track.artwork_id).expect("an artwork id");
  let asset = rig
    .provider
    .asset(&artwork)
    .await
    .expect("the asset path answers")
    .expect("the logo scales for the wire");
  assert_eq!(asset.bytes, b"logo-bytes@248");
  assert_eq!(
    origin
      .requests()
      .iter()
      .filter(|head| head.starts_with("GET /logo"))
      .count(),
    1,
    "the phone and the wire share one logo fetch"
  );
}

#[tokio::test]
async fn the_phone_s_own_tags_are_shown_back_to_it() {
  let rig = boot().await;
  rig.play(URL).await;
  assert!(
    eventually(|| rig.backend.presentations().last().map(|shown| shown.title.as_str()) == Some("radio.example")).await,
    "before any metadata the phone shows the host, like the wire does: {:?}",
    rig.backend.presentations()
  );
  rig
    .backend
    .last_sink()
    .expect("the backend kept its sink")
    .on_metadata(StreamMetadata {
      title: Some("Blue in Green".into()),
      artist: Some("Miles Davis".into()),
      artwork: Some(b"cover".to_vec()),
      ..Default::default()
    });
  let expected = StreamPresentation {
    title: "Blue in Green".into(),
    artist: Some("Miles Davis".into()),
    album: None,
    artwork: Some(b"cover".to_vec()),
  };
  assert!(
    eventually(|| rig.backend.presentations().last() == Some(&expected)).await,
    "the phone's tags never came back as its presentation: {:?}",
    rig.backend.presentations()
  );

  rig.provider.pause().await.expect("pause");
  rig.provider.resume().await.expect("resume");
  assert_eq!(
    rig.backend.presentations().len(),
    2,
    "an unchanged presentation is not re-sent on every tick"
  );
}
