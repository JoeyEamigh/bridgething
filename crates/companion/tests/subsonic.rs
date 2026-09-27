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
#[path = "rig/subsonic.rs"]
mod subsonic_stub;
#[path = "dispatch/support.rs"]
mod support;

use std::sync::{Arc, Mutex};

use backends::{Heard, Offline, RigHost};
use bridgething_companion::{
  api::{AuthKind, CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo, ProviderCredentials, SignInMethod},
  backend::{ForeignHttp, ImageScaler, NativeHttp, SecretStore, StreamMetadata, StreamStatus},
  hub::Hub,
  provider::{
    PlayerTransport, Provider, ProviderAuthState, ProviderError, ProviderRegistry,
    subsonic::{KEY_PASSWORD, KEY_SERVER_URL, KEY_USERNAME, SubsonicConfig, SubsonicProvider},
  },
  session::Session,
};
use libbridgething::{
  BrowseEntry, ItemKind, ItemRef, LibraryItem, PlayContext, PlaybackState, PlayerState, QueuePosition,
  gateway::{
    GatewayToBridgeMsg, GatewayToBridgeMsgData, GatewayToBridgePlayerMsg, LibraryBrowseRequest,
    LibraryFavoritesContainsRequest, LibraryFavoritesListRequest, LibrarySearchRequest, PlayUri, QueueSnapshot,
    QueueUri, TrackIdentity,
  },
};
use log_sink::Quiet;
use poll::eventually;
use secrets::MemorySecrets;
use stream_fake::{APP_BUNDLE, FakeStreamBackend, StreamCall};
use subsonic_stub::{PASSWORD, SubsonicStub, USERNAME};
use support::Peer;

const ALBUM_ONE: &str = "subsonic:album:al1";
const ALBUM_TWO: &str = "subsonic:album:al2";
const TRACK_ONE: &str = "subsonic:track:s1";
const TRACK_TWO: &str = "subsonic:track:s2";
const TRACK_THREE: &str = "subsonic:track:s3";
const TRACK_FOUR: &str = "subsonic:track:s4";
const TRACK_FIVE: &str = "subsonic:track:s5";

struct TagScaler;

impl ImageScaler for TagScaler {
  fn downsample_jpeg(&self, bytes: Vec<u8>, max_edge: u32, _quality: f32) -> Option<Vec<u8>> {
    Some(format!("{}@{max_edge}", String::from_utf8_lossy(&bytes)).into_bytes())
  }
}

fn snapshot_of(msg: &GatewayToBridgeMsg) -> Option<PlayerState> {
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

fn track_ref(uri: &str) -> ItemRef {
  ItemRef {
    uri: uri.into(),
    kind: ItemKind::Track,
    persistent_id: None,
  }
}

struct Rig {
  hub: Arc<Hub>,
  peer: Peer,
  provider: Arc<SubsonicProvider>,
  backend: Arc<FakeStreamBackend>,
  stub: SubsonicStub,
  auth: Arc<Mutex<Vec<ProviderAuthState>>>,
}

impl Rig {
  async fn play(&self, uri: &str, context: Option<&str>) -> Result<(), ProviderError> {
    PlayerTransport::play(
      self.provider.as_ref(),
      PlayUri {
        uri: uri.into(),
        context: context.map(|context_uri| PlayContext {
          context_uri: context_uri.into(),
        }),
      },
    )
    .await
  }

  async fn snapshot_for(&self, uri: &str) -> PlayerState {
    let wanted = uri.to_owned();
    self
      .peer
      .wait(&format!("a snapshot for {uri}"), |msg| {
        snapshot_of(msg).filter(|state| {
          state.playback.state == PlaybackState::Playing
            && state.track.as_ref().and_then(|track| track.uri.as_deref()) == Some(wanted.as_str())
        })
      })
      .await
  }

  async fn queue_with(&self, order: &[&str]) -> QueueSnapshot {
    let wanted: Vec<String> = order.iter().map(|uri| (*uri).to_owned()).collect();
    self
      .peer
      .wait(&format!("a queue of {order:?}"), |msg| {
        queue_of(msg).filter(|queue| queue.order == wanted)
      })
      .await
  }

  async fn played_urls(&self) -> Vec<String> {
    self
      .backend
      .transport_calls()
      .into_iter()
      .filter_map(|call| match call {
        StreamCall::Play(url) => Some(url),
        _ => None,
      })
      .collect()
  }

  async fn authenticated(&self) {
    assert!(
      eventually(|| {
        self
          .auth
          .lock()
          .unwrap()
          .iter()
          .any(|state| matches!(state, ProviderAuthState::Authenticated))
      })
      .await,
      "the sign-in never authenticated: {:?}",
      self.auth.lock().unwrap()
    );
  }
}

async fn boot_with(stub: SubsonicStub, password: &str) -> Rig {
  let (gateway, peer) = Peer::link();
  let hub = Hub::new(
    Arc::new(gateway),
    HostInfo {
      app_name: "subsonic-test".into(),
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
    SubsonicConfig {
      server_url: stub.url.clone(),
      username: USERNAME.into(),
      password: password.into(),
    },
    backend.clone(),
    Arc::new(ForeignHttp::new(Arc::new(NativeHttp::default()))),
    Some(Arc::new(TagScaler)),
  );
  let auth = Arc::new(Mutex::new(Vec::new()));
  let seen = auth.clone();
  provider.set_auth_observer(Some(Arc::new(move |state| seen.lock().unwrap().push(state))));
  hub
    .attach(provider.clone())
    .await
    .expect("the subsonic provider attached");
  Rig {
    hub,
    peer,
    provider,
    backend,
    stub,
    auth,
  }
}

async fn boot() -> Rig {
  let rig = boot_with(SubsonicStub::serve(), PASSWORD).await;
  rig.authenticated().await;
  rig
}

#[tokio::test]
async fn signing_in_pings_the_server_with_a_salted_token() {
  let rig = boot().await;
  let states = rig.auth.lock().unwrap().clone();
  assert!(
    matches!(states.first(), Some(ProviderAuthState::Pending { user_code: None, .. })),
    "the sign-in opens pending with nothing for the user to do: {states:?}"
  );
  assert_eq!(rig.stub.endpoints(), vec!["ping"]);
  assert_eq!(
    rig.hub.for_uri(TRACK_ONE).map(|provider| provider.name().to_owned()),
    Some("subsonic".to_owned())
  );
  assert!(rig.hub.for_uri("https://radio.example/live").is_none());
  assert_eq!(rig.hub.provider_app_bundles(), vec![APP_BUNDLE.to_owned()]);
}

#[tokio::test]
async fn a_wrong_password_fails_the_sign_in_with_a_reason() {
  let rig = boot_with(SubsonicStub::serve(), "not-the-password").await;
  assert!(
    eventually(|| {
      rig
        .auth
        .lock()
        .unwrap()
        .iter()
        .any(|state| matches!(state, ProviderAuthState::Failed { .. }))
    })
    .await
  );
  let states = rig.auth.lock().unwrap().clone();
  let Some(ProviderAuthState::Failed { reason }) = states.last() else {
    panic!("expected a failure, got {states:?}");
  };
  assert_eq!(reason, "the server rejected the username or password");
}

#[tokio::test]
async fn the_root_browse_offers_the_staple_folders_with_previews() {
  let rig = boot().await;
  let root = rig
    .provider
    .browse(LibraryBrowseRequest {
      node_id: None,
      limit: 20,
      offset: 0,
      sections: None,
      preview: Some(2),
    })
    .await
    .expect("the root browse");
  let folders: Vec<(String, usize)> = root
    .entries
    .iter()
    .map(|entry| match entry {
      BrowseEntry::Folder(folder) => (
        folder.node_id.clone(),
        folder.preview_children.as_ref().map_or(0, Vec::len),
      ),
      BrowseEntry::Item(_) => panic!("the root is folders only"),
    })
    .collect();
  assert_eq!(
    folders,
    vec![
      ("playlists".to_owned(), 1),
      ("albums".to_owned(), 2),
      ("artists".to_owned(), 2),
      ("recently-played".to_owned(), 2),
      ("recently-added".to_owned(), 2),
      ("random".to_owned(), 2),
    ]
  );
}

#[tokio::test]
async fn an_album_node_lists_its_tracks_as_subsonic_uris() {
  let rig = boot().await;
  let page = rig
    .provider
    .browse(LibraryBrowseRequest {
      node_id: Some(ALBUM_ONE.into()),
      limit: 2,
      offset: 0,
      sections: None,
      preview: None,
    })
    .await
    .expect("the album page");
  assert_eq!(page.total, Some(3));
  assert!(page.has_more);
  let tracks: Vec<(String, String, String)> = page
    .entries
    .iter()
    .map(|entry| match entry {
      BrowseEntry::Item(LibraryItem::Track(track)) => (track.id.clone(), track.name.clone(), track.image_id.clone()),
      other => panic!("an album lists tracks, got {other:?}"),
    })
    .collect();
  assert_eq!(
    tracks,
    vec![
      (
        TRACK_ONE.to_owned(),
        "Sad Robot".to_owned(),
        "subsonic/img/96/cal-1".to_owned()
      ),
      (
        TRACK_TWO.to_owned(),
        "Space Invaders".to_owned(),
        "subsonic/img/96/cal-1".to_owned()
      ),
    ]
  );
  let albums = rig
    .provider
    .browse(LibraryBrowseRequest {
      node_id: Some("albums".into()),
      limit: 10,
      offset: 0,
      sections: None,
      preview: None,
    })
    .await
    .expect("the album list");
  let names: Vec<String> = albums
    .entries
    .iter()
    .filter_map(|entry| match entry {
      BrowseEntry::Item(LibraryItem::Album(album)) => Some(album.name.clone()),
      _ => None,
    })
    .collect();
  assert_eq!(names, vec!["8-bit lagerfeuer", "Between two worlds", "Second Album"]);
}

#[tokio::test]
async fn search_honors_the_requested_kinds() {
  let rig = boot().await;
  let result = rig
    .provider
    .search(LibrarySearchRequest {
      query: "robot".into(),
      kinds: Some(vec![ItemKind::Track, ItemKind::Playlist]),
      limit: 5,
      offset: 0,
    })
    .await
    .expect("the search");
  assert_eq!(result.kinds, vec![ItemKind::Track]);
  assert!(matches!(result.items.as_slice(), [LibraryItem::Track(track)] if track.id == TRACK_ONE));
  let request = rig
    .stub
    .requests()
    .into_iter()
    .find(|line| line.contains("search3"))
    .expect("the search reached the server");
  assert!(request.contains("songCount=5") && request.contains("albumCount=0") && request.contains("artistCount=0"));

  let playlists = rig
    .provider
    .search(LibrarySearchRequest {
      query: "trip".into(),
      kinds: None,
      limit: 5,
      offset: 0,
    })
    .await
    .expect("the playlist search");
  assert_eq!(playlists.kinds, vec![ItemKind::Playlist]);
}

#[tokio::test]
async fn playing_an_album_streams_the_first_track_and_publishes_the_catalog_metadata() {
  let rig = boot().await;
  rig.play(ALBUM_ONE, None).await.expect("the album plays");

  let played = rig.played_urls().await;
  assert_eq!(played.len(), 1, "one stream url reached the phone: {played:?}");
  assert!(played[0].starts_with(&format!("{}/rest/stream?", rig.stub.url)));
  assert!(played[0].contains("&id=s1") && played[0].contains("u=demo&t="));
  let source = rig.backend.last_source().expect("a source");
  assert!(!source.live);

  let state = rig.snapshot_for(TRACK_ONE).await;
  let track = state.track.expect("the catalog track");
  assert_eq!(track.title.as_deref(), Some("Sad Robot"));
  assert_eq!(track.artist.as_deref(), Some("Pornophonique"));
  assert_eq!(track.album.as_deref(), Some("8-bit lagerfeuer"));
  assert_eq!(track.album_uri.as_deref(), Some(ALBUM_ONE));
  assert_eq!(track.artwork_id.as_deref(), Some("subsonic/img/248/cal-1"));
  assert_eq!(track.duration_ms, Some(212_000));
  assert_eq!(track.liked, Some(false));
  assert_eq!(track.track_number, Some(1));
  assert_eq!(state.playback.queue_index, Some(0));
  assert_eq!(state.playback.queue_count, Some(3));
  let context = state.context.expect("the album is the context");
  assert_eq!(context.uri, ALBUM_ONE);
  assert_eq!(context.name.as_deref(), Some("8-bit lagerfeuer"));

  let queue = rig.queue_with(&[TRACK_TWO, TRACK_THREE]).await;
  assert_eq!(queue.items[0].title.as_deref(), Some("Space Invaders"));
  assert_eq!(queue.items[0].artwork_id.as_deref(), Some("subsonic/img/96/cal-1"));
  assert!(!queue.items[0].queued);
  assert_eq!(rig.hub.now_playing().current_source().as_deref(), Some("subsonic"));
}

#[tokio::test]
async fn a_track_played_inside_its_album_starts_the_album_at_that_track() {
  let rig = boot().await;
  rig.play(TRACK_TWO, Some(ALBUM_ONE)).await.expect("the track plays");
  let state = rig.snapshot_for(TRACK_TWO).await;
  assert_eq!(state.playback.queue_index, Some(1));
  assert_eq!(state.playback.queue_count, Some(3));
  rig.queue_with(&[TRACK_THREE]).await;
}

#[tokio::test]
async fn a_lone_track_plays_without_a_queue() {
  let rig = boot().await;
  rig.play(TRACK_THREE, None).await.expect("the track plays");
  let state = rig.snapshot_for(TRACK_THREE).await;
  assert_eq!(state.playback.queue_index, None);
  assert_eq!(state.playback.queue_count, None);
  assert!(state.context.is_none());
  rig.queue_with(&[]).await;
}

#[tokio::test]
async fn the_phone_ending_a_track_advances_to_the_next_and_the_last_one_clears() {
  let rig = boot().await;
  rig.play(ALBUM_TWO, None).await.expect("the album plays");
  rig.snapshot_for(TRACK_FOUR).await;
  let first_sink = rig.backend.last_sink().expect("the first sink");
  first_sink.on_status(StreamStatus::Ended);

  assert!(eventually(|| rig.backend.sources.lock().unwrap().len() == 2).await);
  assert!(
    !rig.backend.transport_calls().contains(&StreamCall::Stop),
    "an ended track is not stopped again: {:?}",
    rig.backend.transport_calls()
  );
  rig.snapshot_for(TRACK_FIVE).await;
  rig.queue_with(&[]).await;

  rig
    .backend
    .last_sink()
    .expect("the second sink")
    .on_status(StreamStatus::Ended);
  assert!(eventually(|| rig.hub.now_playing().current_source().is_none()).await);
  assert_eq!(rig.backend.sources.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn skip_verbs_walk_the_queue() {
  let rig = boot().await;
  rig.play(ALBUM_ONE, None).await.expect("the album plays");
  rig.snapshot_for(TRACK_ONE).await;

  rig.provider.skip_next().await.expect("skip next");
  rig.snapshot_for(TRACK_TWO).await;
  assert!(rig.backend.transport_calls().contains(&StreamCall::Stop));

  rig.provider.skip_to_index(0).await.expect("skip to the next upcoming");
  rig.snapshot_for(TRACK_THREE).await;
  assert!(
    rig.provider.skip_to_index(0).await.is_err(),
    "nothing is upcoming after the last track"
  );

  rig.provider.skip_prev().await.expect("skip prev");
  let played = rig.played_urls().await;
  assert_eq!(played.len(), 4);
  assert!(played[3].contains("&id=s2"));

  rig.provider.skip_prev().await.expect("skip prev again");
  rig
    .provider
    .skip_prev()
    .await
    .expect("skip prev at the head replays it");
  let played = rig.played_urls().await;
  assert_eq!(played.len(), 6);
  assert!(played[4].contains("&id=s1") && played[5].contains("&id=s1"));

  rig.provider.skip_next().await.expect("skip next");
  rig.provider.skip_next().await.expect("skip next");
  rig.provider.skip_next().await.expect("skip next past the end stops");
  assert!(eventually(|| rig.hub.now_playing().current_source().is_none()).await);
}

#[tokio::test]
async fn queueing_adds_after_the_current_program_or_right_after_the_track() {
  let rig = boot().await;
  rig.play(ALBUM_ONE, None).await.expect("the album plays");
  rig.queue_with(&[TRACK_TWO, TRACK_THREE]).await;

  rig
    .provider
    .queue(QueueUri {
      uri: ALBUM_TWO.into(),
      position: QueuePosition::Append,
    })
    .await
    .expect("append");
  let queue = rig.queue_with(&[TRACK_TWO, TRACK_THREE, TRACK_FOUR, TRACK_FIVE]).await;
  assert!(queue.items[2].queued && queue.items[3].queued);
  assert!(!queue.items[0].queued);

  rig
    .provider
    .queue(QueueUri {
      uri: "subsonic:track:s6".into(),
      position: QueuePosition::Next,
    })
    .await
    .expect("play next");
  rig
    .queue_with(&["subsonic:track:s6", TRACK_TWO, TRACK_THREE, TRACK_FOUR, TRACK_FIVE])
    .await;

  rig
    .provider
    .queue(QueueUri {
      uri: TRACK_ONE.into(),
      position: QueuePosition::Index(1),
    })
    .await
    .expect("queue at a slot");
  rig
    .queue_with(&[
      "subsonic:track:s6",
      TRACK_ONE,
      TRACK_TWO,
      TRACK_THREE,
      TRACK_FOUR,
      TRACK_FIVE,
    ])
    .await;
}

#[tokio::test]
async fn queueing_with_nothing_playing_starts_playback() {
  let rig = boot().await;
  rig
    .provider
    .queue(QueueUri {
      uri: ALBUM_TWO.into(),
      position: QueuePosition::Append,
    })
    .await
    .expect("queue on an idle player");
  rig.snapshot_for(TRACK_FOUR).await;
  rig.queue_with(&[TRACK_FIVE]).await;
}

#[tokio::test]
async fn the_phone_s_empty_metadata_never_blanks_the_catalog_and_its_tags_win_when_present() {
  let rig = boot().await;
  rig.play(TRACK_ONE, None).await.expect("the track plays");
  rig.snapshot_for(TRACK_ONE).await;
  let sink = rig.backend.last_sink().expect("the sink");

  sink.on_metadata(StreamMetadata::default());
  sink.on_status(StreamStatus::Playing);
  let state = rig
    .peer
    .wait("the snapshot after the empty metadata", |msg| {
      snapshot_of(msg).filter(|state| state.playback.state == PlaybackState::Playing)
    })
    .await;
  assert_eq!(
    state.track.as_ref().and_then(|track| track.title.as_deref()),
    Some("Sad Robot")
  );

  sink.on_metadata(StreamMetadata {
    title: Some("Sad Robot (2009 Remaster)".into()),
    ..StreamMetadata::default()
  });
  rig
    .peer
    .wait("the phone's own title on the wire", |msg| {
      snapshot_of(msg).filter(|state| {
        state.track.as_ref().and_then(|track| track.title.as_deref()) == Some("Sad Robot (2009 Remaster)")
          && state.track.as_ref().and_then(|track| track.artist.as_deref()) == Some("Pornophonique")
      })
    })
    .await;

  let presentations = rig.backend.presentations();
  assert!(
    eventually(|| {
      rig
        .backend
        .presentations()
        .last()
        .is_some_and(|shown| shown.title == "Sad Robot (2009 Remaster)")
    })
    .await,
    "the lock screen follows the wire: {presentations:?}"
  );
  let shown = rig.backend.presentations().pop().expect("a presentation");
  assert_eq!(shown.artist.as_deref(), Some("Pornophonique"));
  assert_eq!(shown.album.as_deref(), Some("8-bit lagerfeuer"));
  assert_eq!(
    shown.artwork.as_deref(),
    Some(b"cover-jpeg:al-1".as_slice()),
    "the cover art rides to the lock screen through the authenticated cover url"
  );
}

#[tokio::test]
async fn favorites_star_on_the_server_and_flip_liked_on_the_wire() {
  let rig = boot().await;
  rig.play(TRACK_ONE, None).await.expect("the track plays");
  rig.snapshot_for(TRACK_ONE).await;

  rig
    .provider
    .favorites_toggle(track_ref(TRACK_ONE))
    .await
    .expect("toggle on");
  assert_eq!(rig.stub.starred(), vec!["song:s1"]);
  rig
    .peer
    .wait("the liked flag on the wire", |msg| {
      snapshot_of(msg).filter(|state| state.track.as_ref().and_then(|track| track.liked) == Some(true))
    })
    .await;

  let contains = rig
    .provider
    .favorites_contains(LibraryFavoritesContainsRequest {
      uris: vec![TRACK_ONE.into(), TRACK_TWO.into(), ALBUM_ONE.into()],
    })
    .await
    .expect("contains");
  assert_eq!(contains, vec![true, false, false]);

  rig
    .provider
    .favorites_set(
      ItemRef {
        uri: ALBUM_ONE.into(),
        kind: ItemKind::Album,
        persistent_id: None,
      },
      true,
    )
    .await
    .expect("star an album");
  assert_eq!(rig.stub.starred(), vec!["album:al1", "song:s1"]);

  let page = rig
    .provider
    .favorites_list(LibraryFavoritesListRequest { limit: 10, offset: 0 })
    .await
    .expect("the starred page");
  assert_eq!(page.total, Some(2));
  assert!(matches!(&page.items[0], LibraryItem::Track(track) if track.saved));
  assert!(matches!(&page.items[1], LibraryItem::Album(album) if album.id == ALBUM_ONE));

  rig
    .provider
    .favorites_toggle(track_ref(TRACK_ONE))
    .await
    .expect("toggle off");
  assert_eq!(rig.stub.starred(), vec!["album:al1"]);
  assert!(
    rig
      .provider
      .favorites_toggle(ItemRef {
        uri: "subsonic:playlist:pl1".into(),
        kind: ItemKind::Playlist,
        persistent_id: None,
      })
      .await
      .is_err(),
    "playlists cannot be starred"
  );
}

#[tokio::test]
async fn assets_resolve_cover_ids_through_the_authenticated_cover_url() {
  let rig = boot().await;
  let asset = rig
    .provider
    .asset("subsonic/img/96/cal-1")
    .await
    .expect("the asset call")
    .expect("bytes");
  assert_eq!(asset.bytes, b"cover-jpeg:al-1@96");
  assert_eq!(asset.mime.as_deref(), Some("image/jpeg"));
  let cover = rig
    .stub
    .requests()
    .into_iter()
    .find(|line| line.contains("getCoverArt"))
    .expect("the cover was fetched from the server");
  assert!(cover.contains("id=al-1") && cover.contains("size=600") && cover.contains("u=demo"));
  assert_eq!(rig.provider.asset("spotify/img/96/iabc").await.unwrap(), None);
}

#[tokio::test]
async fn lyrics_come_from_the_server_synced_for_the_playing_track() {
  let rig = boot().await;
  let identity = TrackIdentity {
    artist: "Pornophonique".into(),
    track: "Sad Robot".into(),
    album: None,
    duration_ms: None,
    isrc: None,
  };
  let plain = rig
    .provider
    .lyrics(&identity)
    .await
    .expect("lyrics")
    .expect("plain lyrics by artist and title");
  assert_eq!(plain.synced, None);
  assert_eq!(plain.plain.as_deref(), Some("first line\nsecond line"));
  assert_eq!(plain.source, "subsonic");

  rig.play(TRACK_ONE, None).await.expect("the track plays");
  rig.snapshot_for(TRACK_ONE).await;
  let synced = rig
    .provider
    .lyrics(&identity)
    .await
    .expect("lyrics")
    .expect("synced lyrics by song id");
  let lines = synced.synced.expect("synced lines");
  assert_eq!(lines.len(), 2);
  assert_eq!(lines[1].start_ms, 4_200);
  assert_eq!(lines[1].text, "second line");

  let none = rig
    .provider
    .lyrics(&TrackIdentity {
      artist: "Nobody".into(),
      track: "Nothing".into(),
      album: None,
      duration_ms: None,
      isrc: None,
    })
    .await
    .expect("lyrics");
  assert!(none.is_some(), "the playing track's lyrics are served while it plays");
}

#[tokio::test]
async fn the_last_peer_leaving_leaves_the_album_playing() {
  let rig = boot().await;
  rig.hub.peer_connected("car-1").await;
  rig.play(ALBUM_ONE, None).await.expect("the album plays");
  rig.snapshot_for(TRACK_ONE).await;
  rig.hub.peer_disconnected("car-1");
  tokio::time::sleep(std::time::Duration::from_millis(100)).await;
  assert!(!rig.backend.transport_calls().contains(&StreamCall::Stop));
  assert_eq!(rig.hub.now_playing().current_source().as_deref(), Some("subsonic"));
}

#[tokio::test]
async fn the_probe_refuses_the_audio_body_and_the_phone_plays_the_origin() {
  let rig = boot().await;
  rig.play(TRACK_ONE, None).await.expect("the track plays");
  rig.snapshot_for(TRACK_ONE).await;
  let source = rig.backend.last_source().expect("a source");
  assert!(
    source.url.contains("/rest/stream?"),
    "no relay for a finite file: {}",
    source.url
  );
  assert!(!source.live);
  assert!(
    rig.stub.endpoints().iter().any(|endpoint| endpoint == "stream"),
    "the probe touched the stream url: {:?}",
    rig.stub.endpoints()
  );
}

fn boot_session(secrets: Arc<MemorySecrets>, stream: Option<Arc<FakeStreamBackend>>) -> (Arc<Session>, Arc<Heard>) {
  let spool = std::env::temp_dir().join(format!("subsonic-session-{}", uuid::Uuid::now_v7().simple()));
  std::fs::create_dir_all(&spool).expect("a scratch dir");
  let heard = Arc::new(Heard::default());
  let backends = CompanionBackends {
    link: None,
    host: Arc::new(RigHost),
    http: Arc::new(NativeHttp::default()),
    ws: Arc::new(Offline),
    secrets,
    log: Arc::new(Quiet),
    audio: None,
    volume: None,
    geo: None,
    notifications: None,
    phone: None,
    media_sessions: None,
    stream: stream.map(|backend| backend as Arc<dyn bridgething_companion::backend::StreamBackend>),
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
        app_name: "subsonic-session-test".into(),
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
      state_dir: spool.to_string_lossy().into_owned(),
      cache_dir: spool.to_string_lossy().into_owned(),
      model_platform: None,
      spotify: None,
    },
    backends,
    heard.clone(),
    Arc::new(Offline),
  );
  (session, heard)
}

fn auth_kind(session: &Session) -> Option<AuthKind> {
  session
    .provider_infos()
    .iter()
    .find(|info| info.id == "subsonic")
    .map(|info| info.auth_state.kind)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_catalog_offers_subsonic_only_when_the_host_can_play() {
  let (session, _) = boot_session(Arc::new(MemorySecrets::default()), None);
  assert!(session.provider_infos().iter().all(|info| info.id != "subsonic"));

  let (session, _) = session_with_stream();
  let info = session
    .provider_infos()
    .into_iter()
    .find(|info| info.id == "subsonic")
    .expect("subsonic is offered");
  assert_eq!(info.display_name, "Subsonic");
  assert_eq!(info.sign_in, SignInMethod::ServerLogin);
  assert!(info.available && !info.connected);
}

fn session_with_stream() -> (Arc<Session>, Arc<MemorySecrets>) {
  let secrets = Arc::new(MemorySecrets::default());
  let (session, _) = boot_session(secrets.clone(), Some(FakeStreamBackend::new()));
  (session, secrets)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_login_is_normalized_stored_and_restores_on_the_next_start() {
  let stub = SubsonicStub::serve();
  let (session, secrets) = session_with_stream();
  session
    .complete_provider_auth(
      "subsonic",
      ProviderCredentials::ServerLogin {
        server_url: format!("{}/", stub.url),
        username: format!(" {USERNAME} "),
        password: PASSWORD.into(),
      },
    )
    .await
    .expect("the login is accepted");
  assert_eq!(secrets.get(KEY_SERVER_URL.into()).as_deref(), Some(stub.url.as_str()));
  assert_eq!(secrets.get(KEY_USERNAME.into()).as_deref(), Some(USERNAME));
  assert_eq!(secrets.get(KEY_PASSWORD.into()).as_deref(), Some(PASSWORD));
  assert!(eventually(|| auth_kind(&session) == Some(AuthKind::Authenticated)).await);
  assert!(
    session
      .provider_infos()
      .iter()
      .any(|info| info.id == "subsonic" && info.connected)
  );
  session.stop().await;

  let (restored, _) = boot_session(secrets.clone(), Some(FakeStreamBackend::new()));
  restored.start();
  assert!(
    eventually(|| auth_kind(&restored) == Some(AuthKind::Authenticated)).await,
    "the stored login signs in again on start"
  );
  restored.disconnect_provider("subsonic").await;
  assert_eq!(secrets.get(KEY_PASSWORD.into()), None);
  assert_eq!(secrets.get(KEY_SERVER_URL.into()), None);
  restored.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn junk_logins_and_oauth_tokens_are_refused_before_anything_connects() {
  let (session, secrets) = session_with_stream();
  assert!(
    session
      .complete_provider_auth(
        "subsonic",
        ProviderCredentials::ServerLogin {
          server_url: "ftp://music.example".into(),
          username: USERNAME.into(),
          password: PASSWORD.into(),
        },
      )
      .await
      .is_err()
  );
  assert!(
    session
      .complete_provider_auth(
        "subsonic",
        ProviderCredentials::ServerLogin {
          server_url: "music.example".into(),
          username: "  ".into(),
          password: PASSWORD.into(),
        },
      )
      .await
      .is_err()
  );
  assert!(
    session
      .complete_provider_auth(
        "subsonic",
        ProviderCredentials::OauthTokens {
          access_token: "a".into(),
          refresh_token: "r".into(),
        },
      )
      .await
      .is_err()
  );
  assert_eq!(secrets.get(KEY_SERVER_URL.into()), None);
  assert!(session.provider_infos().iter().all(|info| !info.connected));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_login_surfaces_as_a_failed_sign_in() {
  let stub = SubsonicStub::serve();
  let (session, _) = session_with_stream();
  session
    .complete_provider_auth(
      "subsonic",
      ProviderCredentials::ServerLogin {
        server_url: stub.url.clone(),
        username: USERNAME.into(),
        password: "wrong".into(),
      },
    )
    .await
    .expect("the login is stored and tried");
  assert!(eventually(|| auth_kind(&session) == Some(AuthKind::Failed)).await);
  let info = session
    .provider_infos()
    .into_iter()
    .find(|info| info.id == "subsonic")
    .expect("subsonic");
  assert_eq!(
    info.auth_state.message.as_deref(),
    Some("the server rejected the username or password")
  );
  session.stop().await;
}
