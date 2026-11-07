#[path = "support/art_store.rs"]
mod art_store;
#[path = "support/poll.rs"]
mod poll;
#[path = "rig/secrets.rs"]
mod secrets;
#[path = "rig/stream.rs"]
mod stream_fake;
#[path = "dispatch/support.rs"]
mod support;

use std::sync::{Arc, Mutex};

use bridgething_companion::{
  api::{CapabilityFlags, HostInfo},
  backend::{ForeignHttp, ImageScaler, ImageTarget, NativeHttp, ScaledImage, SecretStore, StreamStatus},
  hub::Hub,
  media::MediaSniff,
  provider::{
    PlayerTransport, Provider, ProviderAuthState,
    plex::{KEY_TOKEN, PLEX_TV, PlexConfig, PlexProvider},
  },
};
use bridgething_io::{HttpExecutor, HttpHeader, HttpMethod, HttpRequest};
use libbridgething::{
  BrowseEntry, BrowseResult, ItemKind, ItemRef, LibraryItem, NluPopularityFilter, NluResolvedIntent, NluSlots,
  NluTargetType, PlaybackState,
  gateway::{
    GatewayToBridgeMsgData, GatewayToBridgePlayerMsg, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibrarySearchRequest, PlayUri, TrackIdentity,
  },
};
use poll::eventually;
use secrets::MemorySecrets;
use stream_fake::{FakeStreamBackend, StreamCall};
use support::Peer;

const TOKEN_VAR: &str = "BRIDGETHING_PLEX_LIVE_TOKEN";

struct KeepBytes;

impl ImageScaler for KeepBytes {
  fn scale(&self, bytes: Vec<u8>, _sniff: MediaSniff, _target: ImageTarget) -> Option<ScaledImage> {
    Some(ScaledImage {
      bytes,
      mime: "image/jpeg".into(),
      width: 0,
      height: 0,
    })
  }
}

struct Live {
  _hub: Arc<Hub>,
  peer: Peer,
  provider: Arc<PlexProvider>,
  backend: Arc<FakeStreamBackend>,
  _art: art_store::Art,
}

async fn signed_in() -> Option<Live> {
  let Ok(token) = std::env::var(TOKEN_VAR) else {
    eprintln!("set {TOKEN_VAR} to an account X-Plex-Token to drive real plex servers");
    return None;
  };
  let (gateway, peer) = Peer::link();
  let hub = Hub::new(
    Arc::new(gateway),
    HostInfo {
      app_name: "plex-live".into(),
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
    false,
  );
  hub.start();
  let secrets = Arc::new(MemorySecrets::default());
  secrets.set(KEY_TOKEN.into(), token);
  let backend = FakeStreamBackend::new();
  let art = art_store::Art::new(Some(Arc::new(KeepBytes)));
  let provider = PlexProvider::new(
    PlexConfig {
      tv_base: PLEX_TV.into(),
      client_id: "bridgething-plex-live".into(),
    },
    secrets,
    backend.clone(),
    Arc::new(ForeignHttp::new(Arc::new(NativeHttp::default()))),
    art.cache(),
  );
  let auth = Arc::new(Mutex::new(Vec::new()));
  let seen = auth.clone();
  provider.set_auth_observer(Some(Arc::new(move |state| seen.lock().unwrap().push(state))));
  hub.attach(provider.clone()).await.expect("attached");
  let signed = || {
    auth
      .lock()
      .unwrap()
      .iter()
      .any(|state| matches!(state, ProviderAuthState::Authenticated))
  };
  let mut authenticated = false;
  for _ in 0..6 {
    if eventually(signed).await {
      authenticated = true;
      break;
    }
  }
  assert!(
    authenticated,
    "the live sign-in did not authenticate: {:?}",
    auth.lock().unwrap()
  );
  Some(Live {
    _hub: hub,
    peer,
    provider,
    backend,
    _art: art,
  })
}

fn node(node_id: Option<&str>, limit: u32, offset: u32) -> LibraryBrowseRequest {
  LibraryBrowseRequest {
    node_id: node_id.map(str::to_owned),
    limit,
    offset,
    sections: None,
    preview: None,
  }
}

fn items(result: &BrowseResult) -> Vec<LibraryItem> {
  result
    .entries
    .iter()
    .filter_map(|entry| match entry {
      BrowseEntry::Item(item) => Some(item.clone()),
      BrowseEntry::Folder(_) => None,
    })
    .collect()
}

async fn first_bytes(url: &str) -> Option<u16> {
  let http = HttpExecutor::new(Arc::new(ForeignHttp::new(Arc::new(NativeHttp::default()))));
  http
    .execute(HttpRequest {
      method: HttpMethod::Get,
      url: url.into(),
      headers: vec![HttpHeader {
        name: "Range".into(),
        value: "bytes=0-1023".into(),
      }],
      body: Vec::new(),
      timeout_ms: 15_000,
    })
    .await
    .ok()
    .map(|response| response.status)
}

#[tokio::test]
async fn real_servers_sign_in_browse_search_and_hand_the_phone_a_stream() {
  let Some(Live {
    _hub,
    peer,
    provider,
    backend,
    _art,
  }) = signed_in().await
  else {
    return;
  };

  let root = provider
    .browse(LibraryBrowseRequest {
      preview: Some(3),
      ..node(None, 20, 0)
    })
    .await
    .expect("the root browse");
  let folders: Vec<String> = root
    .entries
    .iter()
    .filter_map(|entry| match entry {
      BrowseEntry::Folder(folder) => Some(format!(
        "{} total {:?} ({} previewed)",
        folder.node_id,
        folder.total,
        folder.preview_children.as_ref().map_or(0, Vec::len)
      )),
      BrowseEntry::Item(_) => None,
    })
    .collect();
  eprintln!("root: {folders:#?}");
  assert_eq!(folders.len(), 6);

  let first_page = provider.browse(node(Some("albums"), 5, 0)).await.expect("albums");
  let tail = first_page.total.expect("a known album total").saturating_sub(3);
  let deep_page = provider
    .browse(node(Some("albums"), 5, tail))
    .await
    .expect("the last page");
  eprintln!(
    "albums total {:?}, more {}; tail page has {} (more {})",
    first_page.total,
    first_page.has_more,
    deep_page.entries.len(),
    deep_page.has_more
  );
  assert!(first_page.has_more);
  assert_eq!(deep_page.entries.len(), 3);
  assert!(!deep_page.has_more);

  let recent = provider
    .browse(node(Some(libbridgething::RECENTS_NODE_ID), 5, 0))
    .await
    .expect("recents");
  let added = provider
    .browse(node(Some("recently-added"), 5, 0))
    .await
    .expect("added");
  eprintln!("recently played {:?}, recently added {:?}", recent.total, added.total);

  let album = items(&first_page)
    .into_iter()
    .find_map(|item| match item {
      LibraryItem::Album(album) => Some(album),
      _ => None,
    })
    .expect("an album");
  eprintln!("first album: {} {} ({:?})", album.id, album.name, album.artwork_id);
  assert!(album.id.starts_with("plex:album:"));

  let tracks = provider.browse(node(Some(&album.id), 50, 0)).await.expect("its tracks");
  let track = items(&tracks)
    .into_iter()
    .find_map(|item| match item {
      LibraryItem::Track(track) => Some(track),
      _ => None,
    })
    .expect("the album has tracks");
  eprintln!(
    "first track: {} by {} ({} ms)",
    track.name, track.artist.name, track.duration_ms
  );

  let hits = provider
    .search(LibrarySearchRequest {
      query: track.name.clone(),
      kinds: Some(vec![
        ItemKind::Track,
        ItemKind::Album,
        ItemKind::Artist,
        ItemKind::Playlist,
      ]),
      limit: 5,
      offset: 0,
    })
    .await
    .expect("the search");
  eprintln!("search kinds: {:?}, items: {}", hits.kinds, hits.items.len());
  assert!(
    hits
      .items
      .iter()
      .any(|item| matches!(item, LibraryItem::Track(hit) if hit.name == track.name))
  );

  if let Some(artwork_id) = &album.artwork_id {
    let art = provider
      .asset(artwork_id)
      .await
      .expect("the asset call")
      .expect("cover art");
    eprintln!("cover art: {} bytes", art.bytes.len());
    assert!(art.bytes.len() > 100);
  }

  PlayerTransport::play(
    provider.as_ref(),
    PlayUri {
      uri: album.id.clone(),
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
    "playing: {:?} / {:?} / {:?} queue {:?}/{:?}",
    now.title, now.artist, now.album, state.playback.queue_index, state.playback.queue_count
  );
  assert_eq!(now.uri.as_deref(), Some(track.id.as_str()));
  let source = backend.last_source().expect("the phone got a stream");
  eprintln!(
    "stream source: {}...",
    source.url.split("X-Plex-Token").next().unwrap_or_default()
  );
  assert!(source.url.contains("X-Plex-Token="));
  assert!(!source.live);
  assert_eq!(backend.transport_calls(), vec![StreamCall::Play(source.url.clone())]);
  let title = now.title.clone().unwrap_or_default();
  assert!(
    eventually(|| backend.presentations().last().is_some_and(|shown| shown.title == title)).await,
    "the lock screen shows the catalog title: {:?}",
    backend.presentations()
  );
  let answered = first_bytes(&source.url).await;
  eprintln!("the stream answers {answered:?}");
  assert!(answered.is_some_and(|status| status < 300));

  let lyrics = provider
    .lyrics(&TrackIdentity {
      artist: track.artist.name.clone(),
      track: track.name.clone(),
      album: None,
      duration_ms: None,
      isrc: None,
    })
    .await
    .expect("the lyrics call");
  eprintln!(
    "lyrics: {:?}",
    lyrics.as_ref().map(|lyrics| (
      lyrics.synced.as_ref().map(Vec::len),
      lyrics.plain.as_ref().map(String::len)
    ))
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
  }

  provider.detach().await;
}

#[tokio::test]
async fn the_playing_track_s_synced_lyrics_come_from_its_lyric_stream() {
  let Some(live) = signed_in().await else { return };
  let hits = live
    .provider
    .search(LibrarySearchRequest {
      query: "Love Me Do".into(),
      kinds: Some(vec![ItemKind::Track]),
      limit: 10,
      offset: 0,
    })
    .await
    .expect("the search");
  let mut synced = None;
  for hit in &hits.items {
    let LibraryItem::Track(track) = hit else { continue };
    PlayerTransport::play(
      live.provider.as_ref(),
      PlayUri {
        uri: track.id.clone(),
        context: None,
      },
    )
    .await
    .expect("the track plays");
    let identity = TrackIdentity {
      artist: track.artist.name.clone(),
      track: track.name.clone(),
      album: None,
      duration_ms: None,
      isrc: None,
    };
    let lyrics = live.provider.lyrics(&identity).await.expect("the lyrics call");
    eprintln!(
      "{} on {}: {:?}",
      track.name,
      track.album.name,
      lyrics.as_ref().map(|lyrics| lyrics.synced.as_ref().map(Vec::len))
    );
    if let Some(lines) = lyrics.and_then(|lyrics| lyrics.synced) {
      synced = Some(lines);
      break;
    }
  }
  let lines = synced.expect("one of the matches carries timed lyrics");
  assert!(lines.iter().any(|line| line.text.contains("love me do")));
  live.provider.detach().await;
}

#[tokio::test]
async fn a_like_round_trips_through_the_server_and_back() {
  let Some(live) = signed_in().await else { return };
  let tracks = live.provider.browse(node(Some("random"), 1, 0)).await.expect("a track");
  let Some(LibraryItem::Track(track)) = items(&tracks).into_iter().next() else {
    panic!("the library has a track");
  };
  assert!(!track.saved, "pick an unrated track: {}", track.id);
  let item = ItemRef {
    uri: track.id.clone(),
    kind: ItemKind::Track,
    persistent_id: None,
  };
  let request = || LibraryFavoritesContainsRequest {
    uris: vec![track.id.clone()],
  };
  live.provider.favorites_set(item.clone(), true).await.expect("liked");
  let liked = live.provider.favorites_contains(request()).await.expect("contains");
  live.provider.favorites_set(item, false).await.expect("unliked");
  let after = live.provider.favorites_contains(request()).await.expect("contains");
  eprintln!("{} liked {liked:?} then {after:?}", track.name);
  assert_eq!((liked, after), (vec![true], vec![false]));
  live.provider.detach().await;
}

fn spoken(target: &str, kind: NluTargetType) -> NluResolvedIntent {
  NluResolvedIntent {
    intent: "PLAY".into(),
    slots: NluSlots {
      target: Some(target.into()),
      target_type: Some(kind),
      ..NluSlots::default()
    },
    transcript: String::new(),
    alternates: None,
  }
}

#[tokio::test]
async fn real_servers_answer_voice_picks_and_build_stations() {
  let Some(live) = signed_in().await else { return };
  let artists = live
    .provider
    .browse(node(Some("artists"), 5, 0))
    .await
    .expect("artists");
  let artist = items(&artists)
    .into_iter()
    .find_map(|item| match item {
      LibraryItem::Artist(artist) => Some(artist),
      _ => None,
    })
    .expect("an artist");
  let resolver = live
    .provider
    .voice_resolver()
    .expect("a signed-in account resolves voice");

  let named = resolver
    .decorate(spoken(&artist.name, NluTargetType::Artist))
    .await
    .expect("the artist resolves");
  eprintln!("artist {:?} -> {:?}", artist.name, named.slots.uri);
  assert!(named.slots.uri.is_some_and(|uri| uri.starts_with("plex:artist:")));

  let first = resolver
    .decorate(NluResolvedIntent {
      slots: NluSlots {
        popularity_filter: Some(NluPopularityFilter::First),
        ..spoken(&artist.name, NluTargetType::Album).slots
      },
      ..spoken(&artist.name, NluTargetType::Album)
    })
    .await
    .expect("the first album resolves");
  eprintln!("first album -> {:?}", first.slots.uri);
  assert!(first.slots.uri.is_some_and(|uri| uri.starts_with("plex:album:")));

  let station = resolver
    .decorate(spoken(&artist.name, NluTargetType::Station))
    .await
    .expect("the station resolves")
    .slots
    .uri
    .expect("a station uri");
  eprintln!("station -> {station}");
  PlayerTransport::play(
    live.provider.as_ref(),
    PlayUri {
      uri: station,
      context: None,
    },
  )
  .await
  .expect("the station plays");
  let state = live
    .peer
    .wait("the station snapshot", |msg| match &msg.data {
      GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::Snapshot(state))
        if state.playback.state == PlaybackState::Playing =>
      {
        Some(state.as_ref().clone())
      }
      _ => None,
    })
    .await;
  eprintln!(
    "station playing {:?} by {:?}, {:?} queued",
    state.track.as_ref().and_then(|track| track.title.clone()),
    state.track.as_ref().and_then(|track| track.artist.clone()),
    state.playback.queue_count
  );
  assert!(state.playback.queue_count.is_some_and(|count| count > 1));

  let liked = resolver
    .decorate(spoken("my liked songs", NluTargetType::Playlist))
    .await
    .expect("liked songs resolve");
  assert_eq!(liked.slots.uri.as_deref(), Some("plex:liked:songs"));
  live.provider.detach().await;
}
