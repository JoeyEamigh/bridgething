#[path = "support/art_store.rs"]
mod art_store;
#[path = "rig/backends.rs"]
mod backends;
#[path = "rig/log_sink.rs"]
mod log_sink;
#[path = "support/poll.rs"]
mod poll;
#[path = "rig/secrets.rs"]
mod secrets;
#[path = "rig/stage.rs"]
mod stage;
#[path = "rig/stream.rs"]
mod stream_fake;
#[path = "rig/subsonic.rs"]
mod subsonic_stub;
#[path = "dispatch/support.rs"]
mod support;

use std::{ops::Deref, sync::Arc};

use bridgething_companion::{
  api::{AuthKind, ProviderCredentials, SignInMethod},
  backend::{ForeignHttp, NativeHttp, SecretStore, StreamMetadata, StreamStatus, StreamTiming},
  provider::{
    PlayerTransport, Provider, ProviderAuthState, ProviderError, ProviderRegistry,
    subsonic::{KEY_PASSWORD, KEY_SERVER_URL, KEY_USERNAME, SubsonicConfig, SubsonicProvider},
  },
  session::Session,
};
use libbridgething::{
  BrowseEntry, ItemKind, ItemRef, LibraryItem, NluPopularityFilter, NluResolvedIntent, NluSlots, NluTargetType,
  PlayContext, PlaybackState, QueuePosition,
  gateway::{
    LibraryBrowseRequest, LibraryFavoritesContainsRequest, LibraryFavoritesListRequest, LibrarySearchRequest, PlayUri,
    QueueUri, TrackIdentity,
  },
};
use poll::eventually;
use secrets::MemorySecrets;
use stage::{Stage, boot_session, snapshot_of, stream_session};
use stream_fake::{APP_BUNDLE, FakeStreamBackend, StreamCall};
use subsonic_stub::{PASSWORD, SubsonicStub, USERNAME, voice_library};

const SESSION_APP: &str = "subsonic-session-test";
const ALBUM_ONE: &str = "subsonic:album:al1";
const ALBUM_TWO: &str = "subsonic:album:al2";
const TRACK_ONE: &str = "subsonic:track:s1";
const TRACK_TWO: &str = "subsonic:track:s2";
const TRACK_THREE: &str = "subsonic:track:s3";
const TRACK_FOUR: &str = "subsonic:track:s4";
const TRACK_FIVE: &str = "subsonic:track:s5";

fn track_ref(uri: &str) -> ItemRef {
  ItemRef {
    uri: uri.into(),
    kind: ItemKind::Track,
    persistent_id: None,
  }
}

struct Rig {
  stage: Stage,
  provider: Arc<SubsonicProvider>,
  stub: SubsonicStub,
}

impl Deref for Rig {
  type Target = Stage;

  fn deref(&self) -> &Stage {
    &self.stage
  }
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
}

async fn boot_with(stub: SubsonicStub, password: &str) -> Rig {
  let stage = Stage::new("subsonic-test");
  let provider = SubsonicProvider::new(
    SubsonicConfig {
      server_url: stub.url.clone(),
      username: USERNAME.into(),
      password: password.into(),
    },
    stage.backend.clone(),
    Arc::new(ForeignHttp::new(Arc::new(NativeHttp::default()))),
    stage.art(),
  );
  stage.attach(provider.clone()).await;
  Rig { stage, provider, stub }
}

async fn boot() -> Rig {
  let rig = boot_with(SubsonicStub::serve(), PASSWORD).await;
  rig.authenticated().await;
  rig
}

#[tokio::test]
async fn signing_in_pings_the_server_with_a_salted_token() {
  let rig = boot().await;
  let states = rig.auth_states();
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
  let failed = rig
    .settled(|state| matches!(state, ProviderAuthState::Failed { .. }))
    .await;
  assert_eq!(
    failed,
    ProviderAuthState::Failed {
      reason: "the server rejected the username or password".into()
    }
  );
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

  let played = rig.played_urls();
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
  let played = rig.played_urls();
  assert_eq!(played.len(), 4);
  assert!(played[3].contains("&id=s2"));

  rig.provider.skip_prev().await.expect("skip prev again");
  rig
    .provider
    .skip_prev()
    .await
    .expect("skip prev at the head replays it");
  let played = rig.played_urls();
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

fn scrobbles_of(rig: &Rig, id: &str) -> Vec<bool> {
  rig
    .stub
    .scrobbles()
    .into_iter()
    .filter(|(song, _)| song == id)
    .map(|(_, submission)| submission)
    .collect()
}

fn at(position_ms: u32) -> StreamTiming {
  StreamTiming {
    position_ms,
    duration_ms: Some(212_000),
    seekable: true,
  }
}

async fn quiet() {
  tokio::time::sleep(std::time::Duration::from_millis(150)).await;
}

#[tokio::test]
async fn a_track_scrobbles_now_playing_once_and_counts_as_played_past_half_way() {
  let rig = boot().await;
  rig.play(TRACK_ONE, None).await.expect("the track plays");
  rig.snapshot_for(TRACK_ONE).await;
  let sink = rig.backend.last_sink().expect("the sink");
  assert!(
    rig.stub.scrobbles().is_empty(),
    "nothing scrobbles before the phone plays"
  );

  sink.on_status(StreamStatus::Playing);
  assert!(eventually(|| scrobbles_of(&rig, "s1") == [false]).await);
  sink.on_status(StreamStatus::Paused);
  sink.on_status(StreamStatus::Playing);
  sink.on_timing(at(60_000));
  quiet().await;
  assert_eq!(scrobbles_of(&rig, "s1"), [false], "a resume is not a second start");

  sink.on_timing(at(106_000));
  assert!(
    eventually(|| scrobbles_of(&rig, "s1") == [false, true]).await,
    "half of the track counts it: {:?}",
    rig.stub.scrobbles()
  );
  sink.on_timing(at(180_000));
  sink.on_status(StreamStatus::Ended);
  quiet().await;
  assert_eq!(scrobbles_of(&rig, "s1"), [false, true], "a play counts once");
}

#[tokio::test]
async fn a_skip_before_half_way_is_not_a_play() {
  let rig = boot().await;
  rig.play(ALBUM_ONE, None).await.expect("the album plays");
  rig.snapshot_for(TRACK_ONE).await;
  let sink = rig.backend.last_sink().expect("the sink");
  sink.on_status(StreamStatus::Playing);
  sink.on_timing(at(20_000));
  assert!(eventually(|| scrobbles_of(&rig, "s1") == [false]).await);

  rig.provider.skip_next().await.expect("skip next");
  rig.snapshot_for(TRACK_TWO).await;
  rig
    .backend
    .last_sink()
    .expect("the second sink")
    .on_status(StreamStatus::Playing);
  assert!(eventually(|| scrobbles_of(&rig, "s2") == [false]).await);
  quiet().await;
  assert_eq!(scrobbles_of(&rig, "s1"), [false]);
}

#[tokio::test]
async fn a_track_the_phone_finishes_counts_as_played() {
  let rig = boot().await;
  rig.play(ALBUM_TWO, None).await.expect("the album plays");
  rig.snapshot_for(TRACK_FOUR).await;
  let sink = rig.backend.last_sink().expect("the sink");
  sink.on_status(StreamStatus::Playing);
  assert!(eventually(|| scrobbles_of(&rig, "s4") == [false]).await);
  sink.on_status(StreamStatus::Ended);
  assert!(
    eventually(|| scrobbles_of(&rig, "s4") == [false, true]).await,
    "the end of the track counts it: {:?}",
    rig.stub.scrobbles()
  );
  rig.snapshot_for(TRACK_FIVE).await;
  quiet().await;
  assert!(
    scrobbles_of(&rig, "s5").is_empty(),
    "the next track waits for the phone to play it"
  );
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
  let session = boot_session(SESSION_APP, Arc::new(MemorySecrets::default()), None);
  assert!(session.provider_infos().iter().all(|info| info.id != "subsonic"));

  let (session, _) = stream_session(SESSION_APP);
  let info = session
    .provider_infos()
    .into_iter()
    .find(|info| info.id == "subsonic")
    .expect("subsonic is offered");
  assert_eq!(info.display_name, "Subsonic");
  assert_eq!(info.sign_in, SignInMethod::ServerLogin);
  assert!(info.available && !info.connected);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_login_is_normalized_stored_and_restores_on_the_next_start() {
  let stub = SubsonicStub::serve();
  let (session, secrets) = stream_session(SESSION_APP);
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

  let restored = boot_session(SESSION_APP, secrets.clone(), Some(FakeStreamBackend::new()));
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
  let (session, secrets) = stream_session(SESSION_APP);
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
  let (session, _) = stream_session(SESSION_APP);
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

async fn voice_boot() -> Rig {
  let rig = boot_with(SubsonicStub::serve_with(voice_library(), PASSWORD), PASSWORD).await;
  rig.authenticated().await;
  rig
}

fn spoken(target: Option<&str>, kind: Option<NluTargetType>) -> NluSlots {
  NluSlots {
    target: target.map(str::to_string),
    target_type: kind,
    ..NluSlots::default()
  }
}

async fn voice_pick(rig: &Rig, slots: NluSlots) -> (Option<String>, Option<String>) {
  let resolver = rig
    .provider
    .voice_resolver()
    .expect("a signed-in server resolves voice");
  let out = resolver
    .decorate(NluResolvedIntent {
      intent: "PLAY".into(),
      slots,
      transcript: String::new(),
      alternates: None,
    })
    .await
    .expect("the server answered");
  (out.slots.uri, out.slots.context_uri)
}

#[tokio::test]
async fn voice_waits_for_the_sign_in() {
  let rig = boot_with(SubsonicStub::serve_with(voice_library(), PASSWORD), "wrong").await;
  rig
    .settled(|state| matches!(state, ProviderAuthState::Failed { .. }))
    .await;
  assert!(rig.provider.voice_resolver().is_none());
}

#[tokio::test]
async fn voice_names_an_artist_an_album_and_a_track_by_its_artist() {
  let rig = voice_boot().await;
  let artist = voice_pick(&rig, spoken(Some("pornophonique"), Some(NluTargetType::Artist))).await;
  assert_eq!(artist.0.as_deref(), Some("subsonic:artist:ar1"));
  let album = voice_pick(&rig, spoken(Some("second album"), Some(NluTargetType::Album))).await;
  assert_eq!(album.0.as_deref(), Some("subsonic:album:al2"));
  let track = voice_pick(
    &rig,
    spoken(Some("sad robot by maya filipic"), Some(NluTargetType::Track)),
  )
  .await;
  assert_eq!(
    track.0.as_deref(),
    Some("subsonic:track:s7"),
    "the by-phrase picks the named artist's song over the same title by another"
  );
}

#[tokio::test]
async fn voice_reads_the_first_album_as_the_earliest_full_studio_release() {
  let rig = voice_boot().await;
  let first = voice_pick(
    &rig,
    NluSlots {
      popularity_filter: Some(NluPopularityFilter::First),
      ..spoken(Some("pornophonique"), Some(NluTargetType::Album))
    },
  )
  .await;
  assert_eq!(
    first,
    (Some(ALBUM_ONE.into()), Some("subsonic:artist:ar1".into())),
    "the earlier EP and live album are not the debut album"
  );
}

#[tokio::test]
async fn voice_counts_a_position_into_the_named_album() {
  let rig = voice_boot().await;
  let second = voice_pick(
    &rig,
    NluSlots {
      position: Some(2),
      ..spoken(Some("8-bit lagerfeuer"), Some(NluTargetType::Album))
    },
  )
  .await;
  assert_eq!(second, (Some(TRACK_TWO.into()), Some(ALBUM_ONE.into())));
}

#[tokio::test]
async fn liked_songs_are_the_starred_songs_and_play_as_a_collection() {
  let rig = voice_boot().await;
  rig
    .provider
    .favorites_set(track_ref(TRACK_FOUR), true)
    .await
    .expect("starred");
  for target in ["my liked songs", "liked songs", "my favorites"] {
    let liked = voice_pick(&rig, spoken(Some(target), None)).await;
    assert_eq!(
      liked.0.as_deref(),
      Some("subsonic:starred:songs"),
      "{target:?} is the user's stars, never a stranger's playlist titled Liked Songs"
    );
  }
  let first = voice_pick(
    &rig,
    NluSlots {
      position: Some(1),
      ..spoken(Some("my liked songs"), None)
    },
  )
  .await;
  assert_eq!(first, (Some(TRACK_FOUR.into()), Some("subsonic:starred:songs".into())));

  rig.play("subsonic:starred:songs", None).await.expect("the stars play");
  let state = rig.snapshot_for(TRACK_FOUR).await;
  assert_eq!(
    state.context.map(|context| context.uri),
    Some("subsonic:starred:songs".to_owned())
  );
}

#[tokio::test]
async fn the_users_own_playlist_beats_a_strangers_of_the_same_name() {
  let rig = voice_boot().await;
  for kind in [None, Some(NluTargetType::Playlist)] {
    let road = voice_pick(&rig, spoken(Some("road trip"), kind)).await;
    assert_eq!(road.0.as_deref(), Some("subsonic:playlist:pl1"), "{kind:?}");
  }
}

#[tokio::test]
async fn a_station_request_plays_similar_songs_seeded_by_the_artist() {
  let rig = voice_boot().await;
  let station = voice_pick(&rig, spoken(Some("pornophonique"), Some(NluTargetType::Station))).await;
  assert_eq!(station.0.as_deref(), Some("subsonic:station:artist:ar1"));
  rig
    .play("subsonic:station:artist:ar1", None)
    .await
    .expect("the station plays");
  rig.snapshot_for("subsonic:track:s6").await;
  assert!(rig.stub.endpoints().contains(&"getSimilarSongs2".to_owned()));
}

#[tokio::test]
async fn a_popular_request_reads_the_artists_top_songs() {
  let rig = voice_boot().await;
  let top = voice_pick(
    &rig,
    NluSlots {
      popularity_filter: Some(NluPopularityFilter::Popular),
      ..spoken(Some("pornophonique"), None)
    },
  )
  .await;
  assert_eq!(top, (Some(TRACK_THREE.into()), Some("subsonic:artist:ar1".into())));
}

#[tokio::test]
async fn a_bare_album_kind_reads_the_song_that_is_on() {
  let rig = voice_boot().await;
  rig.play(TRACK_FIVE, Some(ALBUM_TWO)).await.expect("playing");
  rig.snapshot_for(TRACK_FIVE).await;
  let album = voice_pick(&rig, spoken(None, Some(NluTargetType::Album))).await;
  assert_eq!(album.0.as_deref(), Some(ALBUM_TWO));
  let first = voice_pick(
    &rig,
    NluSlots {
      position: Some(1),
      ..NluSlots::default()
    },
  )
  .await;
  assert_eq!(first, (Some(TRACK_FOUR.into()), Some(ALBUM_TWO.into())));
}
