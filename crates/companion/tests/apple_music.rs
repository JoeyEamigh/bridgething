#[path = "rig/art.rs"]
mod art;
#[path = "support/art_store.rs"]
mod art_store;
#[path = "dispatch/support.rs"]
mod support;

use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::{
  api::{CapabilityFlags, HostInfo},
  backend::{
    AmActionSink, AmArtworkSink, AmAuthSink, AmAuthStatus, AmCatalogSink, AmCountSink, AmDate, AmEntry,
    AmFavoritesSink, AmFlagSink, AmInbox, AmItem, AmItemSink, AmItemsSink, AmKind, AmLibraryScope, AmPage, AmPageSink,
    AmPlayerCommand, AmPlayerSnapshot, AmRef, AmRelations, AmRelationsSink, AmRepeatMode, AmSearchResults,
    AmSearchSink, AmShelf, AmShelvesSink, AmSnapshotSink, AppleMusicBackend,
  },
  hub::Hub,
  provider::{Provider, ProviderAuthState, ProviderError, apple_music::AppleMusicProvider},
};
use bridgething_io::{HttpDownloadSink, HttpRequest, HttpResponse, HttpSink, HttpTransport};
use libbridgething::{
  BrowseEntry, BrowseResult, ItemKind, ItemRef, LibraryItem, NluPopularityFilter, NluResolvedIntent, NluSlots,
  NluTargetType, PlayContext, PlaybackState, QueuePosition,
  gateway::{
    GatewayToBridgeMsg, GatewayToBridgeMsgData, GatewayToBridgePlayerMsg, LibraryBrowseRequest,
    LibraryFavoritesContainsRequest, LibraryRecommendationsRequest, LibrarySearchRequest, PlayUri, QueueUri,
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

#[derive(Default)]
struct FakeAm {
  auth: Mutex<Option<AmAuthStatus>>,
  can_play: Mutex<Option<Option<bool>>>,
  snapshot: Mutex<Option<AmPlayerSnapshot>>,
  other_audio: Mutex<Option<OtherAudio>>,
  favorites: Mutex<Vec<AmRef>>,
  favorite_asks: Mutex<Vec<Vec<AmRef>>>,
  commands: Mutex<Vec<AmPlayerCommand>>,
  plays: Mutex<Vec<(AmRef, Option<AmRef>)>>,
  queued: Mutex<Vec<(AmRef, bool)>>,
  inbox: Mutex<Option<Arc<AmInbox>>>,
  shelves: Mutex<Vec<AmShelf>>,
  pages: Mutex<HashMap<AmLibraryScope, AmPage>>,
  counts: Mutex<HashMap<AmLibraryScope, u32>>,
  count_asks: Mutex<Vec<AmLibraryScope>>,
  children: Mutex<HashMap<AmRef, Vec<AmItem>>>,
  children_asks: Mutex<Vec<AmRef>>,
  resolved: Mutex<HashMap<AmRef, AmItem>>,
  relations: Mutex<HashMap<AmRef, AmRelations>>,
  resolve_asks: Mutex<Vec<AmRef>>,
  searches: Mutex<Vec<(String, u32, u32)>>,
  search_results: Mutex<Option<AmSearchResults>>,
  search_fails: Mutex<bool>,
  artwork: Mutex<Vec<String>>,
  mute: Mutex<bool>,
  held: Mutex<Vec<Arc<AmItemSink>>>,
}

fn empty_page() -> AmPage {
  AmPage {
    items: Vec::new(),
    has_more: false,
  }
}

fn idle_snapshot() -> AmPlayerSnapshot {
  AmPlayerSnapshot {
    entry: None,
    playing: false,
    position_ms: 0,
    shuffle: false,
    repeat: AmRepeatMode::Off,
    can_seek: true,
  }
}

impl FakeAm {
  fn new() -> Arc<Self> {
    Arc::new(Self::default())
  }

  fn set_now_playing(&self, entry: AmEntry, playing: bool) {
    let mut snapshot = self.snapshot.lock().unwrap().clone().unwrap_or_else(idle_snapshot);
    snapshot.entry = Some(entry);
    snapshot.playing = playing;
    *self.snapshot.lock().unwrap() = Some(snapshot);
    if let Some(inbox) = self.inbox.lock().unwrap().clone() {
      inbox.on_player_changed();
    }
  }

  fn library_changed(&self) {
    self
      .inbox
      .lock()
      .unwrap()
      .clone()
      .expect("observation started")
      .on_library_changed();
  }
}

impl AppleMusicBackend for FakeAm {
  fn start(&self, inbox: Arc<AmInbox>) {
    *self.inbox.lock().unwrap() = Some(inbox);
  }

  fn stop(&self) {
    *self.inbox.lock().unwrap() = None;
  }

  fn snapshot(&self, sink: Arc<AmSnapshotSink>) {
    sink.complete(self.snapshot.lock().unwrap().clone().unwrap_or_else(idle_snapshot));
  }

  fn auth_status(&self, sink: Arc<AmAuthSink>) {
    sink.complete(self.auth.lock().unwrap().unwrap_or(AmAuthStatus::Authorized));
  }

  fn request_authorization(&self, sink: Arc<AmAuthSink>) {
    sink.complete(self.auth.lock().unwrap().unwrap_or(AmAuthStatus::Authorized));
  }

  fn can_play_catalog_content(&self, sink: Arc<AmCatalogSink>) {
    sink.complete(self.can_play.lock().unwrap().unwrap_or(Some(true)));
  }

  fn is_other_audio_playing(&self, sink: Arc<AmFlagSink>) {
    match self.other_audio.lock().unwrap().unwrap_or(OtherAudio::Silent) {
      OtherAudio::Silent => sink.complete(false),
      OtherAudio::Playing => sink.complete(true),
      OtherAudio::Fails => sink.fail("the media bridge gave up".into()),
      OtherAudio::Drops => drop(sink),
    }
  }

  fn play_context(&self, context: AmRef, start_at: Option<AmRef>, sink: Arc<AmActionSink>) {
    self.plays.lock().unwrap().push((context, start_at));
    sink.ok();
  }

  fn queue_insert(&self, item: AmRef, next: bool, sink: Arc<AmActionSink>) {
    self.queued.lock().unwrap().push((item, next));
    sink.ok();
  }

  fn command(&self, cmd: AmPlayerCommand, sink: Arc<AmActionSink>) {
    self.commands.lock().unwrap().push(cmd);
    sink.ok();
  }

  fn library(&self, scope: AmLibraryScope, limit: u32, offset: u32, sink: Arc<AmPageSink>) {
    let page = self
      .pages
      .lock()
      .unwrap()
      .get(&scope)
      .cloned()
      .unwrap_or_else(empty_page);
    let start = (offset as usize).min(page.items.len());
    let end = start.saturating_add(limit as usize).min(page.items.len());
    sink.complete(AmPage {
      items: page.items[start..end].to_vec(),
      has_more: end < page.items.len() || page.has_more,
    });
  }

  fn library_count(&self, scope: AmLibraryScope, sink: Arc<AmCountSink>) {
    self.count_asks.lock().unwrap().push(scope);
    match self.counts.lock().unwrap().get(&scope) {
      Some(count) => sink.complete(*count),
      None => sink.fail("no count in this case".into()),
    }
  }

  fn recently_played(&self, _limit: u32, _offset: u32, sink: Arc<AmPageSink>) {
    sink.complete(empty_page());
  }

  fn children(&self, item: AmRef, sink: Arc<AmItemsSink>) {
    self.children_asks.lock().unwrap().push(item.clone());
    match self.children.lock().unwrap().get(&item) {
      Some(items) => sink.complete(items.clone()),
      None => sink.fail(format!("no children for {item:?}")),
    }
  }

  fn recommendations(&self, sink: Arc<AmShelvesSink>) {
    sink.complete(self.shelves.lock().unwrap().clone());
  }

  fn resolve(&self, item: AmRef, sink: Arc<AmItemSink>) {
    self.resolve_asks.lock().unwrap().push(item.clone());
    if *self.mute.lock().unwrap() {
      self.held.lock().unwrap().push(sink);
      return;
    }
    if let Some(resolved) = self.resolved.lock().unwrap().get(&item) {
      sink.complete(resolved.clone());
      return;
    }
    sink.complete(AmItem {
      subtitle: Some("Curator".into()),
      ..am(item.kind, &item.id, item.library, "Ctx")
    })
  }

  fn relations(&self, item: AmRef, sink: Arc<AmRelationsSink>) {
    sink.complete(self.relations.lock().unwrap().get(&item).cloned().unwrap_or_default());
  }

  fn search(&self, query: String, limit: u32, offset: u32, sink: Arc<AmSearchSink>) {
    self.searches.lock().unwrap().push((query, limit, offset));
    if *self.search_fails.lock().unwrap() {
      sink.fail("developer token request failed".into());
      return;
    }
    let results = self
      .search_results
      .lock()
      .unwrap()
      .clone()
      .unwrap_or_else(|| AmSearchResults {
        top: Vec::new(),
        songs: vec![song(catalog(AmKind::Song, "1"), "T")],
        albums: vec![am(AmKind::Album, "1", false, "A")],
        artists: Vec::new(),
        playlists: Vec::new(),
        stations: Vec::new(),
      });
    sink.complete(results);
  }

  fn is_favorite(&self, items: Vec<AmRef>, sink: Arc<AmFavoritesSink>) {
    self.favorite_asks.lock().unwrap().push(items.clone());
    let favorites = self.favorites.lock().unwrap();
    sink.complete(items.iter().map(|item| favorites.contains(item)).collect());
  }

  fn add_favorite(&self, item: AmRef, sink: Arc<AmActionSink>) {
    self.favorites.lock().unwrap().push(item);
    sink.ok();
  }

  fn artwork(&self, url: String, sink: Arc<AmArtworkSink>) {
    self.artwork.lock().unwrap().push(url.clone());
    if url.starts_with("musicKit://") {
      sink.complete(b"musickit".to_vec());
    } else {
      sink.fail(format!("the backend only serves musicKit art, not {url}"));
    }
  }
}

fn catalog(kind: AmKind, id: &str) -> AmRef {
  AmRef {
    kind,
    id: id.into(),
    library: false,
  }
}

fn library(kind: AmKind, id: &str) -> AmRef {
  AmRef {
    kind,
    id: id.into(),
    library: true,
  }
}

fn am(kind: AmKind, id: &str, in_library: bool, title: &str) -> AmItem {
  AmItem {
    item: AmRef {
      kind,
      id: id.into(),
      library: in_library,
    },
    title: title.into(),
    subtitle: None,
    artist_name: None,
    album_name: None,
    artwork_url: Some("https://art.test/{w}x{h}.jpg".into()),
    duration_ms: None,
    track_count: None,
    released: None,
    single: None,
  }
}

fn song(item: AmRef, title: &str) -> AmItem {
  AmItem {
    item,
    artist_name: Some("Artist".into()),
    album_name: Some("Album".into()),
    duration_ms: Some(1000),
    ..am(AmKind::Song, "", false, title)
  }
}

struct Rig {
  _hub: Arc<Hub>,
  peer: Peer,
  provider: Arc<AppleMusicProvider>,
  backend: Arc<FakeAm>,
  auth_states: Arc<Mutex<Vec<ProviderAuthState>>>,
  _art: art_store::Art,
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
  let art = art_store::Art::new(None);
  let provider = AppleMusicProvider::new(backend.clone(), Arc::new(NoHttp), art.cache());
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
    _art: art,
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
    item: Some(catalog(AmKind::Song, "1")),
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
  *backend.auth.lock().unwrap() = Some(AmAuthStatus::Denied);
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
  *backend.can_play.lock().unwrap() = Some(Some(false));
  let rig = boot(backend).await;
  let failed = await_auth(&rig, |state| matches!(state, ProviderAuthState::Failed { .. })).await;
  match failed {
    ProviderAuthState::Failed { reason } => assert_eq!(reason, "An Apple Music subscription is required"),
    other => panic!("expected failed, got {other:?}"),
  }
}

fn request(node: Option<&str>, limit: u32, offset: u32) -> LibraryBrowseRequest {
  LibraryBrowseRequest {
    node_id: node.map(str::to_owned),
    limit,
    offset,
    sections: None,
    preview: None,
  }
}

fn folder_totals(result: &BrowseResult) -> Vec<(&str, Option<u32>)> {
  result
    .entries
    .iter()
    .map(|entry| match entry {
      BrowseEntry::Folder(folder) => (folder.node_id.as_str(), folder.total),
      BrowseEntry::Item(_) => panic!("the root is folders only"),
    })
    .collect()
}

fn uris(result: &BrowseResult) -> Vec<String> {
  result
    .entries
    .iter()
    .map(|entry| match entry {
      BrowseEntry::Item(LibraryItem::Track(track)) => track.id.clone(),
      BrowseEntry::Item(LibraryItem::Album(album)) => album.id.clone(),
      BrowseEntry::Item(LibraryItem::Artist(artist)) => artist.id.clone(),
      BrowseEntry::Item(LibraryItem::Playlist(playlist)) => playlist.uri.clone(),
      BrowseEntry::Item(LibraryItem::Station(station)) => station.uri.clone(),
      other => panic!("expected items, got {other:?}"),
    })
    .collect()
}

#[tokio::test]
async fn the_root_browse_carries_the_staples_with_real_counts_then_the_rec_rails() {
  let backend = FakeAm::new();
  backend.pages.lock().unwrap().insert(
    AmLibraryScope::Playlists,
    AmPage {
      items: (0..8)
        .map(|n| am(AmKind::Playlist, &format!("p.{n}"), true, "Mix"))
        .collect(),
      has_more: true,
    },
  );
  backend.counts.lock().unwrap().extend([
    (AmLibraryScope::Playlists, 120),
    (AmLibraryScope::Albums, 0),
    (AmLibraryScope::Artists, 31),
  ]);
  backend.shelves.lock().unwrap().push(AmShelf {
    id: "made-for-you".into(),
    title: "Made For You".into(),
    items: vec![am(AmKind::Album, "9", false, "Alb")],
    total: Some(4),
  });
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let result = rig
    .provider
    .browse(request(None, 20, 0))
    .await
    .expect("the root browse resolved");

  assert_eq!(
    folder_totals(&result),
    vec![
      ("playlists", Some(120)),
      ("albums", Some(0)),
      ("artists", Some(31)),
      ("songs", None),
      ("rec:made-for-you", Some(4)),
    ],
    "a shelf total is the library count, not the preview length"
  );
  let BrowseEntry::Folder(playlists) = &result.entries[0] else {
    panic!()
  };
  assert_eq!(playlists.preview_children.as_ref().unwrap().len(), 8);
}

#[tokio::test]
async fn library_counts_are_fetched_once_and_refetched_after_a_library_change() {
  let backend = FakeAm::new();
  backend.counts.lock().unwrap().insert(AmLibraryScope::Albums, 12);
  let rig = boot(backend.clone()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let first = rig.provider.browse(request(Some("albums"), 10, 0)).await.unwrap();
  let second = rig.provider.browse(request(Some("albums"), 10, 10)).await.unwrap();
  assert_eq!((first.total, second.total), (Some(12), Some(12)));
  let asked = |backend: &FakeAm| {
    backend
      .count_asks
      .lock()
      .unwrap()
      .iter()
      .filter(|scope| **scope == AmLibraryScope::Albums)
      .count()
  };
  assert_eq!(asked(&backend), 1, "the count is cached across pages");

  backend.counts.lock().unwrap().insert(AmLibraryScope::Albums, 13);
  backend.library_changed();
  let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
  loop {
    let after = rig.provider.browse(request(Some("albums"), 10, 0)).await.unwrap();
    if after.total == Some(13) {
      break;
    }
    assert!(
      tokio::time::Instant::now() < deadline,
      "the library change never refreshed the count"
    );
    tokio::time::sleep(Duration::from_millis(5)).await;
  }
  assert_eq!(asked(&backend), 2, "a library change refetches the count once");
}

#[tokio::test]
async fn a_staple_page_derives_has_more_from_the_count() {
  let backend = FakeAm::new();
  backend.counts.lock().unwrap().insert(AmLibraryScope::Songs, 10);
  backend.pages.lock().unwrap().insert(
    AmLibraryScope::Songs,
    AmPage {
      items: (0..10)
        .map(|n| song(library(AmKind::Song, &n.to_string()), "S"))
        .collect(),
      has_more: true,
    },
  );
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let page = rig.provider.browse(request(Some("songs"), 10, 0)).await.unwrap();
  assert_eq!(page.total, Some(10));
  assert!(!page.has_more, "a full last page is the end when the count says so");
}

#[tokio::test]
async fn library_items_carry_their_provenance_in_the_uri() {
  let backend = FakeAm::new();
  backend.pages.lock().unwrap().insert(
    AmLibraryScope::Albums,
    AmPage {
      items: vec![am(AmKind::Album, "67114851", true, "Discovery")],
      has_more: false,
    },
  );
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let albums = rig.provider.browse(request(Some("albums"), 10, 0)).await.unwrap();
  assert_eq!(uris(&albums), vec!["applemusic:library:album:67114851".to_string()]);

  let found = rig
    .provider
    .search(LibrarySearchRequest {
      query: "x".into(),
      kinds: Some(vec![ItemKind::Album]),
      limit: 10,
      offset: 0,
    })
    .await
    .unwrap();
  let LibraryItem::Album(album) = &found.items[0] else {
    panic!("expected an album");
  };
  assert_eq!(album.id, "applemusic:album:1", "catalog results stay catalog uris");
}

#[tokio::test]
async fn drilldowns_route_by_provenance_not_id_shape() {
  let backend = FakeAm::new();
  let numeric_library_album = library(AmKind::Album, "67114851");
  let numeric_catalog_album = catalog(AmKind::Album, "67114851");
  backend.children.lock().unwrap().extend([
    (
      numeric_library_album.clone(),
      vec![song(library(AmKind::Song, "1"), "Owned")],
    ),
    (
      numeric_catalog_album.clone(),
      vec![song(catalog(AmKind::Song, "2"), "Store")],
    ),
  ]);
  let rig = boot(backend.clone()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let owned = rig
    .provider
    .browse(request(Some("applemusic:library:album:67114851"), 25, 0))
    .await
    .unwrap();
  let store = rig
    .provider
    .browse(request(Some("applemusic:album:67114851"), 25, 0))
    .await
    .unwrap();

  assert_eq!(uris(&owned), vec!["applemusic:library:song:1".to_string()]);
  assert_eq!(uris(&store), vec!["applemusic:song:2".to_string()]);
  assert_eq!(
    *backend.children_asks.lock().unwrap(),
    vec![numeric_library_album, numeric_catalog_album]
  );
}

#[tokio::test]
async fn a_long_playlist_pages_from_one_children_fetch() {
  let backend = FakeAm::new();
  let playlist = library(AmKind::Playlist, "p.long");
  backend.children.lock().unwrap().insert(
    playlist.clone(),
    (0..250)
      .map(|n| song(library(AmKind::Song, &n.to_string()), "S"))
      .collect(),
  );
  let rig = boot(backend.clone()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let mut seen = Vec::new();
  for offset in [0, 100, 200] {
    let page = rig
      .provider
      .browse(request(Some("applemusic:library:playlist:p.long"), 100, offset))
      .await
      .unwrap();
    seen.push((page.entries.len(), page.total, page.has_more));
  }

  assert_eq!(
    seen,
    vec![(100, Some(250), true), (100, Some(250), true), (50, Some(250), false)]
  );
  assert_eq!(
    backend.children_asks.lock().unwrap().len(),
    1,
    "every page slices one fetch"
  );
}

fn daft_punk(backend: &FakeAm, catalog_albums: &[&str]) -> AmRef {
  let artist = library(AmKind::Artist, "117443263");
  backend.children.lock().unwrap().insert(
    artist.clone(),
    vec![
      am(AmKind::Album, "l.1", true, "Discovery"),
      am(AmKind::Album, "l.2", true, "Homework"),
    ],
  );
  backend
    .resolved
    .lock()
    .unwrap()
    .insert(artist.clone(), am(AmKind::Artist, "117443263", true, "Daft Punk"));
  *backend.search_results.lock().unwrap() = Some(AmSearchResults {
    top: Vec::new(),
    songs: Vec::new(),
    albums: Vec::new(),
    artists: vec![
      am(AmKind::Artist, "900", false, "Daft Punk Tribute"),
      am(AmKind::Artist, "5", false, "daft punk"),
    ],
    playlists: Vec::new(),
    stations: Vec::new(),
  });
  let mut store = vec![
    song(catalog(AmKind::Song, "top1"), "One More Time"),
    song(catalog(AmKind::Song, "top2"), "Around the World"),
  ];
  store.extend(
    catalog_albums
      .iter()
      .map(|title| am(AmKind::Album, title, false, title)),
  );
  backend
    .children
    .lock()
    .unwrap()
    .insert(catalog(AmKind::Artist, "5"), store);
  artist
}

#[tokio::test]
async fn a_library_artist_leads_with_the_matching_catalog_artists_top_songs() {
  let backend = FakeAm::new();
  daft_punk(&backend, &["Random Access Memories", "Discovery"]);
  let rig = boot(backend.clone()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let page = rig
    .provider
    .browse(request(Some("applemusic:library:artist:117443263"), 25, 0))
    .await
    .unwrap();

  assert_eq!(
    uris(&page),
    vec![
      "applemusic:song:top1".to_string(),
      "applemusic:song:top2".to_string(),
      "applemusic:library:album:l.1".to_string(),
      "applemusic:library:album:l.2".to_string(),
    ]
  );
  assert_eq!(page.total, Some(4));
  assert_eq!(
    backend
      .searches
      .lock()
      .unwrap()
      .first()
      .map(|(query, _, _)| query.clone()),
    Some("Daft Punk".to_string())
  );
  assert!(
    !backend
      .children_asks
      .lock()
      .unwrap()
      .contains(&catalog(AmKind::Artist, "900")),
    "a near-miss name is never a candidate"
  );
}

#[tokio::test]
async fn a_top_song_on_a_library_artist_plays_within_its_catalog_artist() {
  use bridgething_companion::provider::PlayerTransport;
  let backend = FakeAm::new();
  daft_punk(&backend, &["Discovery"]);
  let rig = boot(backend.clone()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;
  rig
    .provider
    .browse(request(Some("applemusic:library:artist:117443263"), 25, 0))
    .await
    .unwrap();

  let artist_context = || {
    Some(PlayContext {
      context_uri: "applemusic:library:artist:117443263".into(),
    })
  };
  rig
    .provider
    .play(PlayUri {
      uri: "applemusic:song:top2".into(),
      context: artist_context(),
    })
    .await
    .unwrap();
  rig
    .provider
    .play(PlayUri {
      uri: "applemusic:library:album:l.1".into(),
      context: artist_context(),
    })
    .await
    .unwrap();

  assert_eq!(
    *backend.plays.lock().unwrap(),
    vec![
      (catalog(AmKind::Artist, "5"), Some(catalog(AmKind::Song, "top2"))),
      (
        library(AmKind::Artist, "117443263"),
        Some(library(AmKind::Album, "l.1"))
      ),
    ]
  );
}

#[tokio::test]
async fn a_library_artist_without_a_shared_album_shows_albums_only() {
  let backend = FakeAm::new();
  daft_punk(&backend, &["Some Other Record"]);
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let page = rig
    .provider
    .browse(request(Some("applemusic:library:artist:117443263"), 25, 0))
    .await
    .unwrap();

  assert_eq!(
    uris(&page),
    vec![
      "applemusic:library:album:l.1".to_string(),
      "applemusic:library:album:l.2".to_string(),
    ]
  );
}

#[tokio::test]
async fn a_library_artist_falls_back_to_albums_when_the_catalog_is_unreachable() {
  let backend = FakeAm::new();
  daft_punk(&backend, &["Discovery"]);
  *backend.search_fails.lock().unwrap() = true;
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let page = rig
    .provider
    .browse(request(Some("applemusic:library:artist:117443263"), 25, 0))
    .await
    .unwrap();

  assert_eq!(page.entries.len(), 2);
}

#[tokio::test]
async fn artist_recommendations_share_the_library_artist_page() {
  let backend = FakeAm::new();
  daft_punk(&backend, &["Discovery"]);
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let recs = rig
    .provider
    .recommendations(LibraryRecommendationsRequest {
      seeds: vec![ItemRef {
        uri: "applemusic:library:artist:117443263".into(),
        kind: ItemKind::Artist,
        persistent_id: None,
      }],
      kind: None,
      limit: 3,
      offset: 0,
    })
    .await
    .unwrap();

  assert_eq!(recs.items.len(), 3);
  assert_eq!(recs.total, Some(4));
  assert!(recs.has_more);
  assert!(matches!(&recs.items[0], LibraryItem::Track(track) if track.id == "applemusic:song:top1"));
}

#[tokio::test]
async fn playback_verbs_carry_provenance_to_the_backend() {
  use bridgething_companion::provider::PlayerTransport;
  let rig = boot(FakeAm::new()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  rig
    .provider
    .play(PlayUri {
      uri: "applemusic:library:song:9".into(),
      context: Some(PlayContext {
        context_uri: "applemusic:library:playlist:p.mix".into(),
      }),
    })
    .await
    .unwrap();
  rig
    .provider
    .queue(QueueUri {
      uri: "applemusic:song:44".into(),
      position: QueuePosition::Next,
    })
    .await
    .unwrap();
  rig
    .provider
    .resolve_context("applemusic:library:album:3")
    .await
    .unwrap();
  let refused = rig
    .provider
    .play(PlayUri {
      uri: "spotify:track:1".into(),
      context: None,
    })
    .await;

  assert_eq!(
    *rig.backend.plays.lock().unwrap(),
    vec![(library(AmKind::Playlist, "p.mix"), Some(library(AmKind::Song, "9")))]
  );
  assert_eq!(
    *rig.backend.queued.lock().unwrap(),
    vec![(catalog(AmKind::Song, "44"), true)]
  );
  assert_eq!(
    *rig.backend.resolve_asks.lock().unwrap(),
    vec![library(AmKind::Album, "3")]
  );
  assert!(matches!(refused, Err(ProviderError::Failed(_))), "{refused:?}");
}

#[tokio::test]
async fn favorites_lookups_carry_provenance_and_skip_foreign_uris() {
  let backend = FakeAm::new();
  backend.favorites.lock().unwrap().push(library(AmKind::Song, "7"));
  let rig = boot(backend.clone()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let flags = rig
    .provider
    .favorites_contains(LibraryFavoritesContainsRequest {
      uris: vec![
        "applemusic:library:song:7".into(),
        "nonsense".into(),
        "applemusic:song:7".into(),
      ],
    })
    .await
    .unwrap();

  assert_eq!(flags, vec![true, false, false]);
  assert_eq!(
    backend.favorite_asks.lock().unwrap().last().cloned(),
    Some(vec![library(AmKind::Song, "7"), catalog(AmKind::Song, "7")])
  );
}

#[tokio::test]
async fn search_clamps_to_the_apple_limit_and_passes_the_offset() {
  let backend = FakeAm::new();
  *backend.search_results.lock().unwrap() = Some(AmSearchResults {
    top: Vec::new(),
    songs: (0..25)
      .map(|n| song(catalog(AmKind::Song, &n.to_string()), "S"))
      .collect(),
    albums: Vec::new(),
    artists: Vec::new(),
    playlists: Vec::new(),
    stations: Vec::new(),
  });
  let rig = boot(backend.clone()).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;

  let result = rig
    .provider
    .search(LibrarySearchRequest {
      query: "daft punk".into(),
      kinds: None,
      limit: 100,
      offset: 30,
    })
    .await
    .unwrap();

  assert_eq!(
    *backend.searches.lock().unwrap(),
    vec![("daft punk".to_string(), 25, 30)]
  );
  assert!(result.has_more, "a bucket filled to the clamp has more behind it");
}

#[tokio::test]
async fn library_artwork_loads_through_the_backend_and_never_touches_http() {
  let backend = FakeAm::new();
  let probe = ArtProbe::new();
  let art_store = art_store::Art::new(Some(TagScaler::new()));
  let provider = AppleMusicProvider::new(backend.clone(), probe.clone(), art_store.cache());
  backend.resolved.lock().unwrap().insert(
    library(AmKind::Playlist, "p.1"),
    AmItem {
      artwork_url: Some("musicKit://artwork/library/ABC/{w}x{h}?at=1".into()),
      ..am(AmKind::Playlist, "p.1", true, "Mine")
    },
  );

  let id = provider
    .resolve_context("applemusic:library:playlist:p.1")
    .await
    .unwrap()
    .artwork_id
    .expect("the playlist carries art");
  let art = provider.asset(&id).await.unwrap().expect("library art came back");
  let again = provider.asset(&id).await.unwrap().expect("library art came back again");

  assert_eq!(art.bytes, b"musickit@248");
  assert_eq!(again.bytes, art.bytes);
  assert_eq!(
    *backend.artwork.lock().unwrap(),
    vec!["musicKit://artwork/library/ABC/248x248?at=1".to_string()]
  );
  assert_eq!(probe.fetches(), 0, "musicKit urls never reach the http transport");
}

#[tokio::test]
async fn catalog_artwork_stays_on_http() {
  let backend = FakeAm::new();
  let probe = ArtProbe::new();
  let art_store = art_store::Art::new(Some(TagScaler::new()));
  let provider = AppleMusicProvider::new(backend.clone(), probe.clone(), art_store.cache());

  let id = provider
    .resolve_context("applemusic:album:1")
    .await
    .unwrap()
    .artwork_id
    .expect("the album carries art");
  provider.asset(&id).await.unwrap().expect("catalog art came back");

  assert_eq!(probe.fetches(), 1);
  assert!(backend.artwork.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_rec_rail_drilldown_pages_the_shelf() {
  let backend = FakeAm::new();
  backend.shelves.lock().unwrap().push(AmShelf {
    id: "made-for-you".into(),
    title: "Made For You".into(),
    items: vec![
      am(AmKind::Album, "1", false, "A"),
      am(AmKind::Album, "2", false, "B"),
      am(AmKind::Album, "3", false, "C"),
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
  assert_eq!(*rig.backend.favorites.lock().unwrap(), vec![catalog(AmKind::Song, "1")]);

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

  *backend.other_audio.lock().unwrap() = Some(OtherAudio::Playing);
  rig.provider.handle_peer_connected(true).await;
  assert!(
    !backend.commands.lock().unwrap().contains(&AmPlayerCommand::Play),
    "other audio vetoes the resume"
  );

  *backend.other_audio.lock().unwrap() = Some(OtherAudio::Silent);
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

  *backend.other_audio.lock().unwrap() = Some(OtherAudio::Fails);
  rig.provider.handle_peer_connected(true).await;
  assert!(
    !backend.commands.lock().unwrap().contains(&AmPlayerCommand::Play),
    "a failed audio query is unknown, not a licence to play"
  );

  *backend.other_audio.lock().unwrap() = Some(OtherAudio::Drops);
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
  let art = art_store::Art::new(None);
  let provider = AppleMusicProvider::new(backend, Arc::new(NoHttp), art.cache());

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
  let art = art_store::Art::new(Some(scaler.clone()));
  let provider = AppleMusicProvider::new(FakeAm::new(), probe.clone(), art.cache());

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
    .play(PlayUri {
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
    vec![(catalog(AmKind::Album, "1"), None)]
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

fn dated(item: AmItem, year: i32, month: u32, day: u32) -> AmItem {
  AmItem {
    released: Some(AmDate { year, month, day }),
    ..item
  }
}

fn by(item: AmItem, artist: &str) -> AmItem {
  AmItem {
    artist_name: Some(artist.into()),
    ..item
  }
}

fn radiohead_catalog(backend: &FakeAm) {
  let artist = am(AmKind::Artist, "r1", false, "Radiohead");
  let okc = by(
    dated(am(AmKind::Album, "okc", false, "OK Computer"), 1997, 5, 21),
    "Radiohead",
  );
  *backend.search_results.lock().unwrap() = Some(AmSearchResults {
    top: vec![artist.clone(), okc.clone()],
    songs: vec![
      by(song(catalog(AmKind::Song, "stp"), "Creep"), "Stone Temple Pilots"),
      by(song(catalog(AmKind::Song, "creep"), "Creep"), "Radiohead"),
    ],
    albums: vec![okc.clone()],
    artists: vec![artist],
    playlists: vec![
      am(AmKind::Playlist, "pl.stranger", false, "Favorite Songs"),
      am(AmKind::Playlist, "pl.road", false, "Road Trip"),
    ],
    stations: Vec::new(),
  });
  backend.children.lock().unwrap().extend([
    (
      catalog(AmKind::Artist, "r1"),
      vec![
        by(song(catalog(AmKind::Song, "creep"), "Creep"), "Radiohead"),
        by(song(catalog(AmKind::Song, "karma"), "Karma Police"), "Radiohead"),
        okc.clone(),
        dated(am(AmKind::Album, "pablo", false, "Pablo Honey"), 1993, 2, 22),
        AmItem {
          single: Some(true),
          ..dated(am(AmKind::Album, "creep-single", false, "Creep - Single"), 1992, 9, 21)
        },
        dated(am(AmKind::Album, "astoria", false, "Live at the Astoria"), 1991, 5, 27),
      ],
    ),
    (
      catalog(AmKind::Album, "okc"),
      (1..=12)
        .map(|n| song(catalog(AmKind::Song, &format!("okc-{n}")), &format!("Track {n}")))
        .collect(),
    ),
  ]);
  backend.relations.lock().unwrap().extend([
    (
      catalog(AmKind::Artist, "r1"),
      AmRelations {
        album: None,
        artist: None,
        station: Some(catalog(AmKind::Station, "ra.r1")),
      },
    ),
    (
      catalog(AmKind::Song, "karma"),
      AmRelations {
        album: Some(catalog(AmKind::Album, "okc")),
        artist: Some(catalog(AmKind::Artist, "r1")),
        station: Some(catalog(AmKind::Station, "ra.karma")),
      },
    ),
  ]);
}

fn own_library(backend: &FakeAm, favorites: bool) {
  let mut playlists = vec![
    am(AmKind::Playlist, "p.road", true, "Road Trip"),
    AmItem {
      subtitle: Some("Apple Music".into()),
      ..am(AmKind::Playlist, "p.road-added", true, "Road Trip")
    },
  ];
  if favorites {
    playlists.push(am(AmKind::Playlist, "p.favorites", true, "Favorite Songs"));
  }
  let mut pages = backend.pages.lock().unwrap();
  pages.insert(
    AmLibraryScope::Playlists,
    AmPage {
      items: playlists,
      has_more: false,
    },
  );
  pages.insert(
    AmLibraryScope::Songs,
    AmPage {
      items: (1..=6)
        .map(|n| song(library(AmKind::Song, &format!("i.{n}")), &format!("Owned {n}")))
        .collect(),
      has_more: false,
    },
  );
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
    .expect("an authorized provider resolves voice");
  let out = resolver
    .decorate(NluResolvedIntent {
      intent: "PLAY".into(),
      slots,
      transcript: String::new(),
      alternates: None,
    })
    .await
    .expect("the catalog answered");
  (out.slots.uri, out.slots.context_uri)
}

async fn voice_rig(favorites: bool) -> Rig {
  let backend = FakeAm::new();
  radiohead_catalog(&backend);
  own_library(&backend, favorites);
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Authenticated)).await;
  rig
}

#[tokio::test]
async fn voice_is_unavailable_until_apple_music_authorizes() {
  let backend = FakeAm::new();
  *backend.auth.lock().unwrap() = Some(AmAuthStatus::Denied);
  let rig = boot(backend).await;
  await_auth(&rig, |state| matches!(state, ProviderAuthState::Failed { .. })).await;
  assert!(rig.provider.voice_resolver().is_none());
}

#[tokio::test]
async fn voice_names_an_artist_an_album_and_a_track_by_its_artist() {
  let rig = voice_rig(false).await;
  let artist = voice_pick(&rig, spoken(Some("radiohead"), Some(NluTargetType::Artist))).await;
  assert_eq!(artist.0.as_deref(), Some("applemusic:artist:r1"));
  let album = voice_pick(&rig, spoken(Some("ok computer"), Some(NluTargetType::Album))).await;
  assert_eq!(album.0.as_deref(), Some("applemusic:album:okc"));
  let track = voice_pick(&rig, spoken(Some("creep by radiohead"), Some(NluTargetType::Track))).await;
  assert_eq!(
    track.0.as_deref(),
    Some("applemusic:song:creep"),
    "the by-phrase names the artist, so the cover by someone else loses"
  );
}

#[tokio::test]
async fn voice_reads_the_first_album_as_the_earliest_full_studio_release() {
  let rig = voice_rig(false).await;
  let first = voice_pick(
    &rig,
    NluSlots {
      popularity_filter: Some(NluPopularityFilter::First),
      ..spoken(Some("radiohead"), Some(NluTargetType::Album))
    },
  )
  .await;
  assert_eq!(
    first,
    (
      Some("applemusic:album:pablo".into()),
      Some("applemusic:artist:r1".into())
    ),
    "the earlier single and live tape are not the debut album"
  );
}

#[tokio::test]
async fn voice_counts_a_position_into_the_named_album() {
  let rig = voice_rig(false).await;
  let third = voice_pick(
    &rig,
    NluSlots {
      position: Some(3),
      ..spoken(Some("ok computer"), Some(NluTargetType::Album))
    },
  )
  .await;
  assert_eq!(
    third,
    (
      Some("applemusic:song:okc-3".into()),
      Some("applemusic:album:okc".into())
    )
  );
}

#[tokio::test]
async fn liked_songs_are_the_favorite_songs_playlist_when_the_library_has_one() {
  let rig = voice_rig(true).await;
  let searches_before = rig.backend.searches.lock().unwrap().len();
  for target in ["my liked songs", "favorite songs", "my favorites", "my library"] {
    let liked = voice_pick(&rig, spoken(Some(target), None)).await;
    assert_eq!(
      liked.0.as_deref(),
      Some("applemusic:library:playlist:p.favorites"),
      "{target:?}"
    );
  }
  assert_eq!(
    rig.backend.searches.lock().unwrap().len(),
    searches_before,
    "the catalog is never asked, so a stranger's \"Favorite Songs\" cannot win"
  );
}

#[tokio::test]
async fn liked_songs_fall_back_to_the_library_songs_without_a_favorites_playlist() {
  use bridgething_companion::provider::PlayerTransport;
  let rig = voice_rig(false).await;
  let liked = voice_pick(&rig, spoken(Some("my liked songs"), None)).await;
  assert_eq!(liked.0.as_deref(), Some("applemusic:library:songs:all"));
  let second = voice_pick(
    &rig,
    NluSlots {
      position: Some(2),
      ..spoken(Some("my liked songs"), None)
    },
  )
  .await;
  assert_eq!(
    second,
    (
      Some("applemusic:library:song:i.2".into()),
      Some("applemusic:library:songs:all".into())
    )
  );

  rig
    .provider
    .play(PlayUri {
      uri: "applemusic:library:songs:all".into(),
      context: None,
    })
    .await
    .expect("the library songs collection plays");
  assert_eq!(
    rig.backend.plays.lock().unwrap().last().cloned(),
    Some((library(AmKind::Songs, "all"), None))
  );
}

#[tokio::test]
async fn the_users_own_playlist_beats_a_strangers_catalog_playlist_of_the_same_name() {
  let rig = voice_rig(false).await;
  for kind in [None, Some(NluTargetType::Playlist)] {
    let road = voice_pick(&rig, spoken(Some("road trip"), kind)).await;
    assert_eq!(
      road.0.as_deref(),
      Some("applemusic:library:playlist:p.road"),
      "{kind:?}: the user's own playlist, ahead of one they added and the catalog's"
    );
  }
}

#[tokio::test]
async fn a_station_request_plays_the_artists_station() {
  let rig = voice_rig(false).await;
  let station = voice_pick(&rig, spoken(Some("radiohead"), Some(NluTargetType::Station))).await;
  assert_eq!(station.0.as_deref(), Some("applemusic:station:ra.r1"));
}

#[tokio::test]
async fn a_bare_kind_reads_the_relations_of_the_song_that_is_on() {
  let rig = voice_rig(false).await;
  rig.backend.set_now_playing(
    AmEntry {
      item: Some(catalog(AmKind::Song, "karma")),
      ..entry("Karma Police")
    },
    true,
  );
  let album = voice_pick(&rig, spoken(None, Some(NluTargetType::Album))).await;
  assert_eq!(album.0.as_deref(), Some("applemusic:album:okc"));
  let artist = voice_pick(&rig, spoken(None, Some(NluTargetType::Artist))).await;
  assert_eq!(artist.0.as_deref(), Some("applemusic:artist:r1"));
}
