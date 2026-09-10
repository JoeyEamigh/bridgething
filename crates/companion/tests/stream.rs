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

use std::sync::Arc;

use backends::{Heard, Offline, RigHost};
use bridgething_companion::{
  api::{CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo},
  hub::Hub,
  provider::{PlayerTransport, ProviderRegistry, stream::StreamProvider},
  session::Session,
};
use libbridgething::{
  PlaybackState,
  gateway::{GatewayToBridgeMsg, GatewayToBridgeMsgData, GatewayToBridgePlayerMsg, PlayUri},
};
use log_sink::Quiet;
use poll::eventually;
use secrets::MemorySecrets;
use stream_fake::{FakeStreamBackend, StreamCall};
use support::Peer;

const URL: &str = "https://radio.example/live";

fn snapshot_of(msg: &GatewayToBridgeMsg) -> Option<libbridgething::PlayerState> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::Snapshot(state)) => Some(state.as_ref().clone()),
    _ => None,
  }
}

struct Rig {
  hub: Arc<Hub>,
  peer: Peer,
  provider: Arc<StreamProvider>,
  backend: Arc<FakeStreamBackend>,
}

async fn boot() -> Rig {
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
  let provider = StreamProvider::new(backend.clone());
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

fn play(uri: &str) -> PlayUri {
  PlayUri {
    uri: uri.into(),
    context: None,
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
  assert!(
    rig.hub.for_uri("spotify:track:abc").is_none(),
    "no provider is attached for other schemes"
  );
}

#[tokio::test]
async fn play_hands_the_url_to_the_phone_backend() {
  let rig = boot().await;
  PlayerTransport::play(rig.provider.as_ref(), play(URL))
    .await
    .expect("play routes to the stream provider");

  assert_eq!(rig.backend.calls(), vec![StreamCall::Play(URL.into())]);

  let state = rig.peer.wait("the now-playing snapshot", snapshot_of).await;
  let track = state.track.as_ref().expect("a synthetic track");
  assert_eq!(track.uri.as_deref(), Some(URL));
  assert_eq!(track.title.as_deref(), Some("radio.example"));
  assert_eq!(state.playback.state, PlaybackState::Playing);
  assert_eq!(state.playback.set_elapsed_time_available, Some(false));
  assert_eq!(
    rig.hub.now_playing().current_source().as_deref(),
    Some("stream"),
    "the stream became the audible source"
  );
}

#[tokio::test]
async fn pause_and_resume_delegate_and_update_now_playing() {
  let rig = boot().await;
  PlayerTransport::play(rig.provider.as_ref(), play(URL))
    .await
    .expect("play");
  rig.peer.wait("the playing snapshot", snapshot_of).await;

  rig.provider.pause().await.expect("pause");
  assert!(rig.backend.calls().contains(&StreamCall::Pause));
  let paused = rig
    .peer
    .wait("the paused snapshot", |msg| {
      snapshot_of(msg).filter(|state| state.playback.state == PlaybackState::Paused)
    })
    .await;
  assert_eq!(paused.playback.state, PlaybackState::Paused);

  rig.provider.resume().await.expect("resume");
  assert!(rig.backend.calls().contains(&StreamCall::Resume));
  rig
    .peer
    .wait("the resumed snapshot", |msg| {
      snapshot_of(msg).filter(|state| state.playback.state == PlaybackState::Playing)
    })
    .await;
}

#[tokio::test]
async fn a_backend_stop_clears_the_source() {
  let rig = boot().await;
  PlayerTransport::play(rig.provider.as_ref(), play(URL))
    .await
    .expect("play");
  rig.peer.wait("the playing snapshot", snapshot_of).await;

  rig
    .backend
    .last_sink()
    .expect("the backend kept its sink")
    .on_stopped(None);
  assert!(
    eventually(|| rig.hub.now_playing().current_source().is_none()).await,
    "the stopped stream drops the source"
  );
}

#[tokio::test]
async fn playing_a_second_url_stops_the_first() {
  let rig = boot().await;
  PlayerTransport::play(rig.provider.as_ref(), play(URL))
    .await
    .expect("first play");
  PlayerTransport::play(rig.provider.as_ref(), play("https://other.example/pop"))
    .await
    .expect("second play");
  assert_eq!(
    rig.backend.calls(),
    vec![
      StreamCall::Play(URL.into()),
      StreamCall::Stop,
      StreamCall::Play("https://other.example/pop".into()),
    ]
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn session_start_attaches_the_stream_provider_when_the_backend_exists() {
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
    })
    .await,
    "starting the session attached the stream provider"
  );
  session.stop().await;
}
