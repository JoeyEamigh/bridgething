#[path = "rig/backends.rs"]
mod backends;
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
    ForeignHttp, StreamMetadata, StreamStatus, StreamTiming,
    net::{HttpDownloadSink, HttpHeader, HttpRequest, HttpSink, HttpTransport},
  },
  hub::Hub,
  provider::{PlayerTransport, Provider, ProviderError, ProviderRegistry, stream::StreamProvider},
  session::Session,
};
use libbridgething::{
  PlaybackState, PlayerError,
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
          name: "icy-metaint".into(),
          value: "16000".into(),
        },
        HttpHeader {
          name: "Content-Length".into(),
          value: "1073741824".into(),
        },
      ];
      let wanted = sink.on_response(200, headers, Some(1_073_741_824));
      assert!(!wanted, "the probe refuses the body");
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

async fn boot() -> Rig {
  boot_with(Arc::new(Offline)).await
}

async fn boot_with(http: Arc<dyn HttpTransport>) -> Rig {
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
  let provider = StreamProvider::new(
    backend.clone(),
    APP_BUNDLE.into(),
    Arc::new(ForeignHttp::new(http)),
    None,
  );
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
  assert_eq!(state.playback.set_elapsed_time_available, Some(false));
  assert_eq!(rig.hub.now_playing().current_source().as_deref(), Some("stream"));
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
    rig.backend.calls(),
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
    rig.backend.calls(),
    vec![
      StreamCall::Play(URL.into()),
      StreamCall::Stop,
      StreamCall::Play("https://other.example/pop".into()),
    ]
  );
}

#[tokio::test]
async fn the_last_peer_leaving_stops_the_stream() {
  let rig = boot().await;
  rig.hub.peer_connected("car-1").await;
  rig.hub.peer_connected("car-2").await;
  rig.play(URL).await;
  rig.playing_snapshot().await;

  rig.hub.peer_disconnected("car-1").await;
  assert!(!rig.backend.calls().contains(&StreamCall::Stop));
  assert_eq!(rig.hub.now_playing().current_source().as_deref(), Some("stream"));

  rig.hub.peer_disconnected("car-2").await;
  assert!(rig.backend.calls().contains(&StreamCall::Stop));
  assert!(eventually(|| rig.hub.now_playing().current_source().is_none()).await);
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
