#[path = "rig/art.rs"]
mod art;
#[path = "dispatch/support.rs"]
mod support;

use std::{
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::{
  api::{CapabilityFlags, HostInfo},
  backend::{
    AmActionSink, AmAuthSink, AmAuthStatus, AmCatalogSink, AmEntry, AmFavoritesSink, AmFlagSink, AmItem, AmItemSink,
    AmKind, AmLibraryScope, AmPage, AmPageSink, AmPlayerCommand, AmPlayerInbox, AmPlayerSnapshot, AmRepeatMode,
    AmSearchResults, AmSearchSink, AmShelf, AmShelvesSink, AmSnapshotSink, AppleMusicBackend,
  },
  hub::Hub,
  provider::{Provider, ProviderAuthState, ProviderError, apple_music::AppleMusicProvider},
};
use bridgething_io::{HttpDownloadSink, HttpRequest, HttpResponse, HttpSink, HttpTransport};
use libbridgething::{
  BrowseEntry, ItemKind, ItemRef, LibraryItem, PlaybackState,
  gateway::{
    GatewayToBridgeMsg, GatewayToBridgeMsgData, GatewayToBridgePlayerMsg, LibraryBrowseRequest, LibrarySearchRequest,
  },
};
use support::Peer;

use crate::art::{ArtProbe, TagScaler};

struct NoHttp;

impl HttpTransport for NoHttp {
  fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
    sink.complete(HttpResponse {
      status: 404,
      headers: Vec::new(),
      body: Vec::new(),
    });
  }

  fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    sink.on_failed("no art in these cases".into());
  }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OtherAudio {
  Silent,
  Playing,
  Fails,
  Drops,
}

struct FakeAm {
  auth: Mutex<AmAuthStatus>,
  can_play: Mutex<Option<bool>>,
  snapshot: Mutex<AmPlayerSnapshot>,
  other_audio: Mutex<OtherAudio>,
  favorites: Mutex<Vec<String>>,
  commands: Mutex<Vec<AmPlayerCommand>>,
  plays: Mutex<Vec<(String, Option<String>)>>,
  inbox: Mutex<Option<Arc<AmPlayerInbox>>>,
  shelves: Mutex<Vec<AmShelf>>,
  pages: Mutex<std::collections::HashMap<String, AmPage>>,
  mute: Mutex<bool>,
  held: Mutex<Vec<Arc<AmItemSink>>>,
}

fn empty_page() -> AmPage {
  AmPage {
    items: Vec::new(),
    total: None,
    has_more: false,
  }
}

impl FakeAm {
  fn new() -> Arc<Self> {
    Arc::new(Self {
      auth: Mutex::new(AmAuthStatus::Authorized),
      can_play: Mutex::new(Some(true)),
      snapshot: Mutex::new(AmPlayerSnapshot {
        entry: None,
        playing: false,
        position_ms: 0,
        shuffle: false,
        repeat: AmRepeatMode::Off,
        can_seek: true,
      }),
      other_audio: Mutex::new(OtherAudio::Silent),
      favorites: Mutex::new(Vec::new()),
      commands: Mutex::new(Vec::new()),
      plays: Mutex::new(Vec::new()),
      inbox: Mutex::new(None),
      shelves: Mutex::new(Vec::new()),
      pages: Mutex::new(std::collections::HashMap::new()),
      mute: Mutex::new(false),
      held: Mutex::new(Vec::new()),
    })
  }

  fn set_now_playing(&self, entry: AmEntry, playing: bool) {
    self.snapshot.lock().unwrap().entry = Some(entry);
    self.snapshot.lock().unwrap().playing = playing;
    if let Some(inbox) = self.inbox.lock().unwrap().clone() {
      inbox.on_changed();
    }
  }
}

fn scope_key(scope: &AmLibraryScope) -> String {
  match scope {
    AmLibraryScope::Playlists => "playlists".into(),
    AmLibraryScope::Albums => "albums".into(),
    AmLibraryScope::Artists => "artists".into(),
    AmLibraryScope::Songs => "songs".into(),
    AmLibraryScope::RecentlyPlayed => "recently-played".into(),
    AmLibraryScope::Children { uri } => format!("children:{uri}"),
  }
}

impl AppleMusicBackend for FakeAm {
  fn start(&self, inbox: Arc<AmPlayerInbox>) {
    *self.inbox.lock().unwrap() = Some(inbox);
  }

  fn stop(&self) {
    *self.inbox.lock().unwrap() = None;
  }

  fn snapshot(&self, sink: Arc<AmSnapshotSink>) {
    sink.complete(self.snapshot.lock().unwrap().clone());
  }

  fn auth_status(&self, sink: Arc<AmAuthSink>) {
    sink.complete(*self.auth.lock().unwrap());
  }

  fn request_authorization(&self, sink: Arc<AmAuthSink>) {
    sink.complete(*self.auth.lock().unwrap());
  }

  fn can_play_catalog_content(&self, sink: Arc<AmCatalogSink>) {
    sink.complete(*self.can_play.lock().unwrap());
  }

  fn is_other_audio_playing(&self, sink: Arc<AmFlagSink>) {
    match *self.other_audio.lock().unwrap() {
      OtherAudio::Silent => sink.complete(false),
      OtherAudio::Playing => sink.complete(true),
      OtherAudio::Fails => sink.fail("the media bridge gave up".into()),
      OtherAudio::Drops => drop(sink),
    }
  }

  fn play_context(&self, context_uri: String, start_at_uri: Option<String>, sink: Arc<AmActionSink>) {
    self.plays.lock().unwrap().push((context_uri, start_at_uri));
    sink.ok();
  }

  fn queue_insert(&self, _uri: String, _next: bool, sink: Arc<AmActionSink>) {
    sink.ok();
  }

  fn command(&self, cmd: AmPlayerCommand, sink: Arc<AmActionSink>) {
    self.commands.lock().unwrap().push(cmd);
    sink.ok();
  }

  fn library(&self, scope: AmLibraryScope, _limit: u32, _offset: u32, sink: Arc<AmPageSink>) {
    let page = self
      .pages
      .lock()
      .unwrap()
      .get(&scope_key(&scope))
      .cloned()
      .unwrap_or_else(empty_page);
    sink.complete(page);
  }

  fn recommendations(&self, sink: Arc<AmShelvesSink>) {
    sink.complete(self.shelves.lock().unwrap().clone());
  }

  fn resolve(&self, uri: String, sink: Arc<AmItemSink>) {
    if *self.mute.lock().unwrap() {
      self.held.lock().unwrap().push(sink);
      return;
    }
    sink.complete(AmItem {
      uri,
      kind: AmKind::Playlist,
      title: "Ctx".into(),
      subtitle: Some("Curator".into()),
      artist_name: None,
      artist_uri: None,
      album_name: None,
      album_uri: None,
      artwork_url: Some("https://art.test/{w}x{h}.jpg".into()),
      duration_ms: None,
      track_count: None,
    })
  }

  fn search(&self, _query: String, _limit: u32, sink: Arc<AmSearchSink>) {
    sink.complete(AmSearchResults {
      songs: vec![song("applemusic:song:1", "T")],
      albums: vec![item("applemusic:album:1", AmKind::Album, "A")],
      artists: Vec::new(),
      playlists: Vec::new(),
    });
  }

  fn is_favorite(&self, uris: Vec<String>, sink: Arc<AmFavoritesSink>) {
    let favorites = self.favorites.lock().unwrap();
    sink.complete(uris.iter().map(|uri| favorites.contains(uri)).collect());
  }

  fn add_favorite(&self, uri: String, sink: Arc<AmActionSink>) {
    self.favorites.lock().unwrap().push(uri);
    sink.ok();
  }
}

fn item(uri: &str, kind: AmKind, title: &str) -> AmItem {
  AmItem {
    uri: uri.into(),
    kind,
    title: title.into(),
    subtitle: None,
    artist_name: None,
    artist_uri: None,
    album_name: None,
    album_uri: None,
    artwork_url: Some("https://art.test/{w}x{h}.jpg".into()),
    duration_ms: None,
    track_count: None,
  }
}

fn song(uri: &str, title: &str) -> AmItem {
  AmItem {
    artist_name: Some("Artist".into()),
    artist_uri: Some("applemusic:artist:1".into()),
    album_name: Some("Album".into()),
    album_uri: Some("applemusic:album:1".into()),
    duration_ms: Some(1000),
    ..item(uri, AmKind::Song, title)
  }
}

struct Rig {
  _hub: Arc<Hub>,
  peer: Peer,
  provider: Arc<AppleMusicProvider>,
  backend: Arc<FakeAm>,
  auth_states: Arc<Mutex<Vec<ProviderAuthState>>>,
}

async fn boot(backend: Arc<FakeAm>) -> Rig {
  let (gateway, peer) = Peer::link();
  let hub = Hub::new(
    Arc::new(gateway),
    HostInfo {
      app_name: "am-test".into(),
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
  let provider = AppleMusicProvider::new(backend.clone(), Arc::new(NoHttp), None);
  let auth_states: Arc<Mutex<Vec<ProviderAuthState>>> = Arc::new(Mutex::new(Vec::new()));
  let sink = auth_states.clone();
  provider.set_auth_observer(Some(Arc::new(move |state| {
    sink.lock().unwrap().push(state);
  })));
  hub.attach(provider.clone()).await.expect("the provider attached");
  Rig {
    _hub: hub,
    peer,
    provider,
    backend,
    auth_states,
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

fn snapshot_of(msg: &GatewayToBridgeMsg) -> Option<libbridgething::PlayerState> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::Snapshot(state)) => Some(state.as_ref().clone()),
    _ => None,
  }
}

fn entry(title: &str) -> AmEntry {
  AmEntry {
    uri: Some("applemusic:song:1".into()),
    title: title.into(),
    artist_name: Some("Artist".into()),
    album_name: Some("Album".into()),
    artwork_url: Some("https://art.test/{w}x{h}.jpg".into()),
    duration_ms: Some(1000),
  }
}

#[tokio::test]
async fn an_authorized_subscriber_authenticates_and_snapshots_the_player() {
  let rig = boot(FakeAm::new()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;
  rig.backend.set_now_playing(entry("Song"), true);

  let state = rig.peer.wait("the snapshot", snapshot_of).await;
  let track = state.track.as_ref().unwrap();
  assert_eq!(track.title.as_deref(), Some("Song"));
  assert_eq!(track.artist.as_deref(), Some("Artist"));
  assert_eq!(state.playback.state, PlaybackState::Playing);
  assert!(
    track.artwork_id.as_deref().unwrap().starts_with("applemusic/img/248/u"),
    "the sized template rides an applemusic asset id: {:?}",
    track.artwork_id
  );
  assert_eq!(track.is_like_supported, Some(true));
}

#[tokio::test]
async fn a_denied_authorization_fails_with_the_settings_pointer() {
  let backend = FakeAm::new();
  *backend.auth.lock().unwrap() = AmAuthStatus::Denied;
  let rig = boot(backend).await;
  let failed = await_auth(&rig, |state| matches!(state, ProviderAuthState::Failed { .. })).await;
  match failed {
    ProviderAuthState::Failed { reason } => assert!(reason.contains("Settings > Privacy"), "{reason}"),
    other => panic!("expected failed, got {other:?}"),
  }
}

#[tokio::test]
async fn a_missing_subscription_fails_with_the_subscription_message() {
  let backend = FakeAm::new();
  *backend.can_play.lock().unwrap() = Some(false);
  let rig = boot(backend).await;
  let failed = await_auth(&rig, |state| matches!(state, ProviderAuthState::Failed { .. })).await;
  match failed {
    ProviderAuthState::Failed { reason } => assert_eq!(reason, "An Apple Music subscription is required"),
    other => panic!("expected failed, got {other:?}"),
  }
}

#[tokio::test]
async fn the_root_browse_carries_the_staples_then_the_rec_rails() {
  let backend = FakeAm::new();
  backend.pages.lock().unwrap().insert(
    "playlists".into(),
    AmPage {
      items: vec![item("applemusic:playlist:1", AmKind::Playlist, "Mix")],
      total: Some(12),
      has_more: true,
    },
  );
  backend.shelves.lock().unwrap().push(AmShelf {
    id: "made-for-you".into(),
    title: "Made For You".into(),
    items: vec![item("applemusic:album:9", AmKind::Album, "Alb")],
    total: Some(4),
  });
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let result = rig
    .provider
    .browse(LibraryBrowseRequest {
      node_id: None,
      limit: 20,
      offset: 0,
      sections: None,
      preview: None,
    })
    .await
    .expect("the root browse resolved");

  let folders: Vec<(&str, Option<u32>)> = result
    .entries
    .iter()
    .map(|entry| match entry {
      BrowseEntry::Folder(folder) => (folder.node_id.as_str(), folder.total),
      BrowseEntry::Item(_) => panic!("the root is folders only"),
    })
    .collect();
  assert_eq!(
    folders,
    vec![
      ("playlists", Some(12)),
      ("albums", None),
      ("artists", None),
      ("songs", None),
      ("rec:made-for-you", Some(4)),
    ]
  );
  let BrowseEntry::Folder(playlists) = &result.entries[0] else {
    panic!()
  };
  assert_eq!(playlists.preview_children.as_ref().unwrap().len(), 1);
}

#[tokio::test]
async fn a_rec_rail_drilldown_pages_the_shelf() {
  let backend = FakeAm::new();
  backend.shelves.lock().unwrap().push(AmShelf {
    id: "made-for-you".into(),
    title: "Made For You".into(),
    items: vec![
      item("applemusic:album:1", AmKind::Album, "A"),
      item("applemusic:album:2", AmKind::Album, "B"),
      item("applemusic:album:3", AmKind::Album, "C"),
    ],
    total: None,
  });
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let result = rig
    .provider
    .browse(LibraryBrowseRequest {
      node_id: Some("rec:made-for-you".into()),
      limit: 2,
      offset: 1,
      sections: None,
      preview: None,
    })
    .await
    .expect("the drilldown resolved");
  let names: Vec<&str> = result
    .entries
    .iter()
    .map(|entry| match entry {
      BrowseEntry::Item(LibraryItem::Album(album)) => album.name.as_str(),
      other => panic!("expected albums, got {other:?}"),
    })
    .collect();
  assert_eq!(names, vec!["B", "C"]);
  assert!(!result.has_more);
}

#[tokio::test]
async fn search_maps_by_requested_kinds() {
  let rig = boot(FakeAm::new()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let result = rig
    .provider
    .search(LibrarySearchRequest {
      query: "x".into(),
      kinds: Some(vec![ItemKind::Track, ItemKind::Album]),
      limit: 10,
      offset: 0,
    })
    .await
    .expect("the search resolved");
  assert_eq!(result.items.len(), 2);
  assert_eq!(result.kinds, vec![ItemKind::Track, ItemKind::Album]);
  let LibraryItem::Track(track) = &result.items[0] else {
    panic!("tracks come first");
  };
  assert_eq!(track.name, "T");
  assert_eq!(track.artist.name, "Artist");
}

#[tokio::test]
async fn favorites_are_add_only() {
  let rig = boot(FakeAm::new()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;
  let track = ItemRef {
    uri: "applemusic:song:1".into(),
    kind: ItemKind::Track,
    persistent_id: None,
  };

  rig
    .provider
    .favorites_set(track.clone(), true)
    .await
    .expect("a like lands");
  assert_eq!(
    *rig.backend.favorites.lock().unwrap(),
    vec!["applemusic:song:1".to_string()]
  );

  let refused = rig.provider.favorites_set(track.clone(), false).await;
  assert_eq!(refused, Err(ProviderError::NotImplemented), "an unlike cannot land");

  let refused = rig.provider.favorites_toggle(track).await;
  assert_eq!(
    refused,
    Err(ProviderError::NotImplemented),
    "toggling an already-liked item cannot unlike it"
  );
}

#[tokio::test]
async fn peer_connect_resumes_only_when_nothing_else_is_audible() {
  let backend = FakeAm::new();
  let rig = boot(backend.clone()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;
  backend.set_now_playing(entry("Song"), false);
  rig.peer.wait("the paused snapshot", snapshot_of).await;

  *backend.other_audio.lock().unwrap() = OtherAudio::Playing;
  rig.provider.handle_peer_connected(true).await;
  assert!(
    !backend.commands.lock().unwrap().contains(&AmPlayerCommand::Play),
    "other audio vetoes the resume"
  );

  *backend.other_audio.lock().unwrap() = OtherAudio::Silent;
  rig.provider.handle_peer_connected(true).await;
  assert!(
    backend.commands.lock().unwrap().contains(&AmPlayerCommand::Play),
    "a silent phone resumes the music app"
  );
}

#[tokio::test]
async fn peer_connect_holds_off_when_the_audio_query_does_not_answer() {
  let backend = FakeAm::new();
  let rig = boot(backend.clone()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;
  backend.set_now_playing(entry("Song"), false);
  rig.peer.wait("the paused snapshot", snapshot_of).await;

  *backend.other_audio.lock().unwrap() = OtherAudio::Fails;
  rig.provider.handle_peer_connected(true).await;
  assert!(
    !backend.commands.lock().unwrap().contains(&AmPlayerCommand::Play),
    "a failed audio query is unknown, not a licence to play"
  );

  *backend.other_audio.lock().unwrap() = OtherAudio::Drops;
  rig.provider.handle_peer_connected(true).await;
  assert!(
    !backend.commands.lock().unwrap().contains(&AmPlayerCommand::Play),
    "an abandoned audio query is unknown, not a licence to play"
  );
}

#[tokio::test(start_paused = true)]
async fn a_backend_that_never_answers_fails_the_request_instead_of_hanging() {
  let backend = FakeAm::new();
  *backend.mute.lock().unwrap() = true;
  let provider = AppleMusicProvider::new(backend, Arc::new(NoHttp), None);

  let outcome = tokio::time::timeout(
    Duration::from_secs(300),
    provider.resolve_context("applemusic:playlist:1"),
  )
  .await;

  match outcome {
    Ok(Err(ProviderError::Failed(reason))) => assert!(reason.contains("answer"), "{reason}"),
    Ok(other) => panic!("expected a failed resolve, got {other:?}"),
    Err(_) => panic!("the resolve hung past its deadline"),
  }
}

#[tokio::test]
async fn repeat_artwork_requests_fetch_and_scale_once() {
  let probe = ArtProbe::new();
  let scaler = TagScaler::new();
  let provider = AppleMusicProvider::new(FakeAm::new(), probe.clone(), Some(scaler.clone()));

  let id = provider
    .resolve_context("applemusic:playlist:1")
    .await
    .expect("the resolve landed")
    .artwork_id
    .expect("the context carries art");

  let first = provider
    .asset(&id)
    .await
    .expect("the asset resolved")
    .expect("art came back");
  let second = provider
    .asset(&id)
    .await
    .expect("the asset resolved")
    .expect("art came back");

  assert_eq!(first, second);
  assert_eq!(probe.fetches(), 1, "the master is fetched once");
  assert_eq!(scaler.scales(), 1, "the downsample runs once");
}

#[tokio::test]
async fn verbs_fold_into_backend_commands() {
  use bridgething_companion::provider::PlayerTransport;
  let rig = boot(FakeAm::new()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  rig.provider.pause().await.unwrap();
  rig.provider.seek_to(5000).await.unwrap();
  rig.provider.set_repeat(libbridgething::RepeatMode::One).await.unwrap();
  rig
    .provider
    .play(libbridgething::gateway::PlayUri {
      uri: "applemusic:album:1".into(),
      context: None,
    })
    .await
    .unwrap();

  assert_eq!(
    *rig.backend.commands.lock().unwrap(),
    vec![
      AmPlayerCommand::Pause,
      AmPlayerCommand::SeekTo { position_ms: 5000 },
      AmPlayerCommand::SetRepeat {
        mode: AmRepeatMode::One
      },
    ]
  );
  assert_eq!(
    *rig.backend.plays.lock().unwrap(),
    vec![("applemusic:album:1".to_string(), None)]
  );
}

#[tokio::test]
async fn resolve_context_carries_name_art_and_subtitle() {
  let rig = boot(FakeAm::new()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let reply = rig
    .provider
    .resolve_context("applemusic:playlist:1")
    .await
    .expect("the resolve landed");
  assert_eq!(reply.name.as_deref(), Some("Ctx"));
  assert_eq!(reply.subtitle.as_deref(), Some("Curator"));
  assert!(reply.artwork_id.as_deref().unwrap().starts_with("applemusic/img/248/u"));
}
