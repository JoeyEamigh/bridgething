#[path = "support/poll.rs"]
mod poll;
#[path = "rig/stream.rs"]
mod stream_fake;
#[path = "dispatch/support.rs"]
mod support;

use std::sync::{Arc, Mutex};

use bridgething_companion::{
  api::{CapabilityFlags, HostInfo},
  backend::{ForeignHttp, ImageScaler, NativeHttp, StreamStatus},
  hub::Hub,
  provider::{
    PlayerTransport, Provider, ProviderAuthState,
    subsonic::{SubsonicConfig, SubsonicProvider},
  },
};
use libbridgething::{
  BrowseEntry, ItemKind, LibraryItem, PlaybackState,
  gateway::{GatewayToBridgeMsgData, GatewayToBridgePlayerMsg, LibraryBrowseRequest, LibrarySearchRequest, PlayUri},
};
use poll::eventually;
use stream_fake::{FakeStreamBackend, StreamCall};
use support::Peer;

const URL_VAR: &str = "BRIDGETHING_SUBSONIC_LIVE_URL";
const USERNAME_VAR: &str = "BRIDGETHING_SUBSONIC_LIVE_USERNAME";
const PASSWORD_VAR: &str = "BRIDGETHING_SUBSONIC_LIVE_PASSWORD";

struct KeepBytes;

impl ImageScaler for KeepBytes {
  fn downsample_jpeg(&self, bytes: Vec<u8>, _max_edge: u32, _quality: f32) -> Option<Vec<u8>> {
    Some(bytes)
  }
}

async fn patiently(mut holds: impl FnMut() -> bool) -> bool {
  for _ in 0..4 {
    if eventually(&mut holds).await {
      return true;
    }
  }
  false
}

fn live_config() -> Option<SubsonicConfig> {
  Some(SubsonicConfig {
    server_url: std::env::var(URL_VAR).ok()?,
    username: std::env::var(USERNAME_VAR).ok()?,
    password: std::env::var(PASSWORD_VAR).ok()?,
  })
}

#[tokio::test]
async fn a_real_server_signs_in_browses_searches_and_hands_the_phone_a_stream() {
  let Some(config) = live_config() else {
    eprintln!("set {URL_VAR}, {USERNAME_VAR} and {PASSWORD_VAR} to drive a real subsonic server");
    return;
  };
  let (gateway, peer) = Peer::link();
  let hub = Hub::new(
    Arc::new(gateway),
    HostInfo {
      app_name: "subsonic-live".into(),
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
  let provider = SubsonicProvider::new(
    config.clone(),
    backend.clone(),
    Arc::new(ForeignHttp::new(Arc::new(NativeHttp::default()))),
    Some(Arc::new(KeepBytes)),
  );
  let auth = Arc::new(Mutex::new(Vec::new()));
  let seen = auth.clone();
  provider.set_auth_observer(Some(Arc::new(move |state| seen.lock().unwrap().push(state))));
  hub.attach(provider.clone()).await.expect("attached");
  assert!(
    eventually(|| {
      auth
        .lock()
        .unwrap()
        .iter()
        .any(|state| matches!(state, ProviderAuthState::Authenticated))
    })
    .await,
    "the live sign-in did not authenticate: {:?}",
    auth.lock().unwrap()
  );

  let root = provider
    .browse(LibraryBrowseRequest {
      node_id: None,
      limit: 20,
      offset: 0,
      sections: None,
      preview: Some(3),
    })
    .await
    .expect("the root browse");
  let folders: Vec<String> = root
    .entries
    .iter()
    .filter_map(|entry| match entry {
      BrowseEntry::Folder(folder) => Some(format!(
        "{} ({} previewed)",
        folder.node_id,
        folder.preview_children.as_ref().map_or(0, Vec::len)
      )),
      BrowseEntry::Item(_) => None,
    })
    .collect();
  eprintln!("root: {folders:?}");
  assert_eq!(folders.len(), 6);

  let albums = provider
    .browse(LibraryBrowseRequest {
      node_id: Some("albums".into()),
      limit: 5,
      offset: 0,
      sections: None,
      preview: None,
    })
    .await
    .expect("the album list");
  let first = albums
    .entries
    .iter()
    .find_map(|entry| match entry {
      BrowseEntry::Item(LibraryItem::Album(album)) => Some(album.clone()),
      _ => None,
    })
    .expect("at least one album");
  eprintln!("first album: {} ({:?})", first.name, first.artwork_id);

  let tracks = provider
    .browse(LibraryBrowseRequest {
      node_id: Some(first.id.clone()),
      limit: 50,
      offset: 0,
      sections: None,
      preview: None,
    })
    .await
    .expect("the album's tracks");
  let track = tracks
    .entries
    .iter()
    .find_map(|entry| match entry {
      BrowseEntry::Item(LibraryItem::Track(track)) => Some(track.clone()),
      _ => None,
    })
    .expect("the album has tracks");
  eprintln!(
    "first track: {} by {} ({} ms)",
    track.name, track.artist.name, track.duration_ms
  );

  let hits = provider
    .search(LibrarySearchRequest {
      query: track.name.split(' ').next().unwrap_or(&track.name).to_owned(),
      kinds: Some(vec![ItemKind::Track, ItemKind::Album, ItemKind::Artist]),
      limit: 5,
      offset: 0,
    })
    .await
    .expect("the search");
  eprintln!("search kinds: {:?}, items: {}", hits.kinds, hits.items.len());
  assert!(!hits.items.is_empty());

  if let Some(artwork_id) = &first.artwork_id {
    match provider.asset(artwork_id).await.expect("the asset call") {
      Some(art) => {
        eprintln!("cover art: {} bytes", art.bytes.len());
        assert!(art.bytes.len() > 100);
      }
      None => eprintln!("the server did not serve {artwork_id}; public demos rate-limit cover art"),
    }
  }

  PlayerTransport::play(
    provider.as_ref(),
    PlayUri {
      uri: first.id.clone(),
      context: None,
    },
  )
  .await
  .expect("the album plays");
  let state = peer
    .wait("the playing snapshot", |msg| match &msg.data {
      GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::Snapshot(state))
        if state.playback.state == PlaybackState::Playing =>
      {
        Some(state.as_ref().clone())
      }
      _ => None,
    })
    .await;
  let now = state.track.expect("a track");
  eprintln!(
    "playing: {:?} / {:?} / {:?} art {:?} queue {:?}/{:?}",
    now.title, now.artist, now.album, now.artwork_id, state.playback.queue_index, state.playback.queue_count
  );
  assert_eq!(now.uri.as_deref(), Some(track.id.as_str()));
  assert_eq!(now.title.as_deref(), Some(track.name.as_str()));
  let source = backend.last_source().expect("the phone got a stream");
  eprintln!("stream source: {} (live {})", source.url, source.live);
  assert!(
    source
      .url
      .starts_with(&format!("{}/rest/stream?", config.server_url.trim_end_matches('/')))
  );
  assert!(!source.live);
  assert_eq!(backend.transport_calls(), vec![StreamCall::Play(source.url.clone())]);
  assert!(
    patiently(|| {
      backend
        .presentations()
        .last()
        .is_some_and(|shown| shown.title == track.name)
    })
    .await,
    "the lock screen shows the catalog title: {:?}",
    backend.presentations()
  );
  let shown = backend.presentations().pop().expect("a presentation");
  eprintln!(
    "presented: {} / {:?} / {:?} with {} art bytes",
    shown.title,
    shown.artist,
    shown.album,
    shown.artwork.as_ref().map_or(0, Vec::len)
  );

  if state.playback.queue_count.is_some_and(|count| count > 1) {
    backend.last_sink().expect("the sink").on_status(StreamStatus::Ended);
    assert!(
      eventually(|| backend
        .calls()
        .iter()
        .filter(|call| matches!(call, StreamCall::Play(_)))
        .count()
        == 2)
      .await,
      "the end of the first track starts the second: {:?}",
      backend.calls()
    );
    eprintln!("advanced to: {}", backend.last_source().expect("the second source").url);
  }

  provider.detach().await;
}
