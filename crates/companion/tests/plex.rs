#[path = "support/art_store.rs"]
mod art_store;
#[path = "rig/backends.rs"]
mod backends;
#[path = "rig/log_sink.rs"]
mod log_sink;
#[path = "rig/plex.rs"]
mod plex_stub;
#[path = "support/poll.rs"]
mod poll;
#[path = "rig/secrets.rs"]
mod secrets;
#[path = "rig/stage.rs"]
mod stage;
#[path = "rig/stream.rs"]
mod stream_fake;
#[path = "dispatch/support.rs"]
mod support;

use std::{ops::Deref, sync::Arc};

use bridgething_companion::{
  api::{ProviderCredentials, SignInMethod},
  backend::{ForeignHttp, NativeHttp, SecretStore, StreamStatus, StreamTiming},
  provider::{
    PlayerTransport, Provider, ProviderAuthState, ProviderError,
    plex::{KEY_TOKEN, PlexConfig, PlexProvider},
  },
};
use libbridgething::{
  BrowseEntry, BrowseResult, ItemKind, ItemRef, LibraryItem, NluResolvedIntent, NluSlots, NluTargetType, PlayContext,
  gateway::{
    LibraryBrowseRequest, LibraryFavoritesContainsRequest, LibraryFavoritesListRequest, LibrarySearchRequest, PlayUri,
    TrackIdentity,
  },
};
use plex_stub::{
  ACCOUNT_TOKEN, CLIENT_ID, EMONA_LYRICS, MACHINE_A, MACHINE_B, MACHINE_C, PIN_CODE, PLAIN_LYRICS, PlexStub, Script,
  Seen, TOKEN_A, TOKEN_B, TOKEN_C, TV,
};
use poll::eventually;
use secrets::MemorySecrets;
use stage::{Stage, boot_session, snapshot_of, stream_session};
use stream_fake::StreamCall;

const ALBUM_A: &str = "plex:album:machine-a:101";
const SECOND: &str = "plex:album:machine-a:102";
const EMONA_SESSIONS: &str = "plex:album:machine-a:103";
const SECOND_FLAC: &str = "plex:album:machine-a:104";
const ALBUM_B: &str = "plex:album:machine-b:202";
const SPECTRA: &str = "plex:album:machine-c:301";
const ROBOT: &str = "plex:track:machine-a:1001";
const INVADERS: &str = "plex:track:machine-a:1002";
const MACHINE: &str = "plex:track:machine-a:1003";
const GAME_OVER: &str = "plex:track:machine-a:1004";
const EMONA: &str = "plex:track:machine-a:1005";
const ATTIC_ROBOT: &str = "plex:track:machine-b:2001";
const LIVE_ROBOT: &str = "plex:track:machine-b:2002";
const IVY: &str = "plex:track:machine-b:2003";
const OUTRO: &str = "plex:track:machine-b:2005";
const FOCUS: &str = "plex:track:machine-b:2006";
const HUMAN: &str = "plex:track:machine-b:2007";
const ATTIC_GAME_OVER: &str = "plex:track:machine-b:2008";
const LIKED: &str = "plex:liked:songs";
const COVER_A: &str = "amachine-a:/library/metadata/101/thumb/1700000000";
const COVER_B: &str = "amachine-b:/library/metadata/202/thumb/1700000000";
const SESSION_APP: &str = "plex-session-test";

struct Rig {
  stage: Stage,
  provider: Arc<PlexProvider>,
  stub: PlexStub,
  secrets: Arc<MemorySecrets>,
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

  async fn browse(&self, node_id: Option<&str>, limit: u32, offset: u32) -> BrowseResult {
    self
      .provider
      .browse(LibraryBrowseRequest {
        node_id: node_id.map(str::to_owned),
        limit,
        offset,
        sections: None,
        preview: None,
      })
      .await
      .unwrap_or_else(|error| panic!("browsing {node_id:?} failed: {error:?}"))
  }

  async fn contains(&self, uris: &[&str]) -> Vec<bool> {
    self
      .provider
      .favorites_contains(LibraryFavoritesContainsRequest {
        uris: uris.iter().map(|uri| (*uri).to_owned()).collect(),
      })
      .await
      .expect("favorites contains")
  }

  async fn lyrics(&self) -> Option<libbridgething::Lyrics> {
    self
      .provider
      .lyrics(&TrackIdentity {
        artist: "Pornophonique".into(),
        track: "Sad Robot".into(),
        album: None,
        duration_ms: None,
        isrc: None,
      })
      .await
      .expect("the lyrics call")
  }

  fn album_fetches(&self) -> Vec<usize> {
    [(MACHINE_A, "1"), (MACHINE_B, "3"), (MACHINE_C, "5")]
      .into_iter()
      .map(|(host, section)| {
        self
          .stub
          .on(host, &format!("/library/sections/{section}/all"))
          .iter()
          .filter(|seen| seen.query.get("type").map(String::as_str) == Some("9"))
          .count()
      })
      .collect()
  }

  fn timeline(&self, host: &str, key: &str) -> Vec<(String, u32, Option<u32>)> {
    self
      .stub
      .on(host, "/:/timeline")
      .into_iter()
      .filter(|seen| seen.query.get("ratingKey").map(String::as_str) == Some(key))
      .map(|seen| {
        let number = |name: &str| seen.query.get(name).and_then(|value| value.parse::<u32>().ok());
        (
          seen.query["state"].clone(),
          number("time").unwrap_or(0),
          number("duration"),
        )
      })
      .collect()
  }
}

async fn boot_with(stub: PlexStub, token: Option<&str>) -> Rig {
  let stage = Stage::new("plex-test");
  let secrets = Arc::new(MemorySecrets::default());
  if let Some(token) = token {
    secrets.set(KEY_TOKEN.into(), token.into());
  }
  let provider = PlexProvider::new(
    PlexConfig {
      tv_base: stub.tv_url.clone(),
      client_id: CLIENT_ID.into(),
    },
    secrets.clone(),
    stage.backend.clone(),
    Arc::new(ForeignHttp::new(Arc::new(NativeHttp::default()))),
    stage.art(),
  );
  stage.attach(provider.clone()).await;
  Rig {
    stage,
    provider,
    stub,
    secrets,
  }
}

async fn boot() -> Rig {
  let rig = boot_with(PlexStub::serve(), Some(ACCOUNT_TOKEN)).await;
  rig.authenticated().await;
  rig
}

fn items(result: &BrowseResult) -> Vec<LibraryItem> {
  result
    .entries
    .iter()
    .map(|entry| match entry {
      BrowseEntry::Item(item) => item.clone(),
      BrowseEntry::Folder(folder) => panic!("a node lists items, got the folder {}", folder.node_id),
    })
    .collect()
}

fn names(result: &BrowseResult) -> Vec<(String, String)> {
  items(result)
    .into_iter()
    .map(|item| match item {
      LibraryItem::Album(album) => (album.id, album.name),
      LibraryItem::Artist(artist) => (artist.id, artist.name),
      LibraryItem::Track(track) => (track.id, track.name),
      LibraryItem::Playlist(playlist) => (playlist.uri, playlist.name),
      other => panic!("plex lists no {other:?}"),
    })
    .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
  expected
    .iter()
    .map(|(id, name)| ((*id).to_owned(), (*name).to_owned()))
    .collect()
}

fn signed(seen: &[Seen], token: &str) -> bool {
  !seen.is_empty() && seen.iter().all(|seen| seen.token.as_deref() == Some(token))
}

fn timing(position_ms: u32) -> StreamTiming {
  StreamTiming {
    position_ms,
    duration_ms: Some(200_000),
    seekable: true,
  }
}

#[tokio::test]
async fn a_pin_sign_in_hands_out_the_plex_auth_url_and_keeps_the_token_it_earns() {
  let rig = boot_with(PlexStub::serve(), None).await;
  let pending = rig
    .settled(|state| {
      matches!(
        state,
        ProviderAuthState::Pending {
          verification_url_complete: Some(_),
          ..
        }
      )
    })
    .await;
  let url = format!(
    "https://app.plex.tv/auth#?clientID={CLIENT_ID}&code={PIN_CODE}&context%5Bdevice%5D%5Bproduct%5D=bridgething"
  );
  assert_eq!(
    pending,
    ProviderAuthState::Pending {
      user_code: None,
      verification_url: Some(url.clone()),
      verification_url_complete: Some(url),
    }
  );

  let mut paired = false;
  for _ in 0..3 {
    if eventually(|| rig.secrets.get(KEY_TOKEN.into()).is_some()).await {
      paired = true;
      break;
    }
  }
  assert!(paired, "the pin never paid out a token: {:?}", rig.auth_states());
  rig.authenticated().await;
  assert_eq!(rig.secrets.get(KEY_TOKEN.into()).as_deref(), Some(ACCOUNT_TOKEN));
  assert_eq!(rig.stub.polls(), 2, "the pin is polled until it carries a token");
  let states = rig.auth_states();
  assert_eq!(
    states.first(),
    Some(&ProviderAuthState::Pending {
      user_code: None,
      verification_url: None,
      verification_url_complete: None,
    })
  );
  assert_eq!(states.last(), Some(&ProviderAuthState::Authenticated));
  assert!(
    !states
      .iter()
      .any(|state| matches!(state, ProviderAuthState::Failed { .. }))
  );

  let pins = rig.stub.on(TV, "/api/v2/pins");
  assert_eq!(pins.len(), 1);
  assert_eq!(pins[0].method, "POST");
  assert_eq!(pins[0].query.get("strong").map(String::as_str), Some("true"));
  assert_eq!(pins[0].token, None);
  assert!(signed(&rig.stub.on(TV, "/api/v2/resources"), ACCOUNT_TOKEN));
  assert!(rig.provider.voice_resolver().is_some());
}

#[tokio::test]
async fn a_stored_token_skips_the_pin_and_probes_every_server_with_its_own_token() {
  let rig = boot().await;
  assert!(rig.stub.on(TV, "/api/v2/pins").is_empty());
  assert_eq!(rig.stub.polls(), 0);
  assert!(signed(&rig.stub.on(TV, "/api/v2/resources"), ACCOUNT_TOKEN));
  assert!(signed(&rig.stub.on(MACHINE_A, "/identity"), TOKEN_A));
  assert!(signed(&rig.stub.on(MACHINE_A, "/library/sections"), TOKEN_A));
  assert!(signed(&rig.stub.on(MACHINE_B, "/identity"), TOKEN_B));
  assert!(signed(&rig.stub.on(MACHINE_B, "/library/sections"), TOKEN_B));
  assert!(signed(&rig.stub.on(MACHINE_C, "/identity"), TOKEN_C));
  assert!(signed(&rig.stub.on(MACHINE_C, "/library/sections"), TOKEN_C));
}

#[tokio::test]
async fn a_revoked_token_fails_the_sign_in_with_a_reason_and_is_forgotten() {
  let stub = PlexStub::serve_with(Script {
    revoked: true,
    ..Script::default()
  });
  let rig = boot_with(stub, Some(ACCOUNT_TOKEN)).await;
  let failed = rig
    .settled(|state| matches!(state, ProviderAuthState::Failed { .. }))
    .await;
  assert_eq!(
    failed,
    ProviderAuthState::Failed {
      reason: "plex signed this device out; sign in again".into()
    }
  );
  assert_eq!(rig.secrets.get(KEY_TOKEN.into()), None);
  assert!(rig.provider.voice_resolver().is_none());
  assert!(matches!(
    rig
      .provider
      .browse(LibraryBrowseRequest {
        node_id: Some("albums".into()),
        limit: 5,
        offset: 0,
        sections: None,
        preview: None,
      })
      .await,
    Err(ProviderError::NotAuthenticated)
  ));
}

#[tokio::test]
async fn the_best_answering_connection_wins_and_carries_every_request() {
  let rig = boot().await;
  assert!(
    signed(&rig.stub.on(TV, "/identity"), TOKEN_A),
    "the relay connection was probed"
  );
  assert!(
    rig
      .stub
      .seen()
      .iter()
      .filter(|seen| seen.host == TV)
      .all(|seen| seen.path.starts_with("/api/v2/") || seen.path == "/identity"),
    "the local connection beats the relay: {:?}",
    rig.stub.on(TV, "/library/sections")
  );

  rig.play(LIVE_ROBOT, None).await.expect("the track plays");
  rig.snapshot_for(LIVE_ROBOT).await;
  let played = rig.played_urls();
  assert_eq!(played.len(), 1);
  let remote = rig.stub.url(MACHINE_B);
  assert!(
    played[0].starts_with(&format!("{remote}/library/parts/2002/")),
    "the dead local connection lost to the remote one: {} (dead {})",
    played[0],
    rig.stub.dead_url
  );
  assert!(played[0].ends_with(&format!("X-Plex-Token={TOKEN_B}")));
  let probe = rig.stub.on(MACHINE_B, "/library/parts/2002/1700000000/file.mp3");
  assert!(signed(&probe, TOKEN_B), "the stream probe reached the remote server");
  assert!(signed(&rig.stub.on(MACHINE_B, "/library/metadata/2002"), TOKEN_B));
}

#[tokio::test]
async fn the_root_offers_the_staple_folders_with_deduped_totals_across_every_server() {
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
  let folders: Vec<(String, Option<u32>, usize)> = root
    .entries
    .iter()
    .map(|entry| match entry {
      BrowseEntry::Folder(folder) => (
        folder.node_id.clone(),
        folder.total,
        folder.preview_children.as_ref().map_or(0, Vec::len),
      ),
      BrowseEntry::Item(_) => panic!("the root is folders only"),
    })
    .collect();
  assert_eq!(
    folders,
    vec![
      ("playlists".to_owned(), Some(2), 2),
      ("albums".to_owned(), Some(6), 2),
      ("artists".to_owned(), Some(3), 2),
      ("recently-played".to_owned(), Some(3), 2),
      ("recently-added".to_owned(), Some(6), 2),
      ("random".to_owned(), None, 2),
    ]
  );
}

#[tokio::test]
async fn overlapping_albums_and_artists_appear_once_from_the_preferred_server() {
  let rig = boot().await;
  let albums = rig.browse(Some("albums"), 20, 0).await;
  assert_eq!(
    names(&albums),
    pairs(&[
      (ALBUM_A, "8-bit lagerfeuer"),
      (EMONA_SESSIONS, "The Emona Sessions"),
      (ALBUM_B, "Live at the Lagerfeuer"),
      (SECOND, "Second Album"),
      (SECOND_FLAC, "Second Album (FLAC)"),
      (SPECTRA, "Spectra"),
    ]),
    "one copy per album, the owned local server first, the shared local server before the remote one"
  );
  assert_eq!(albums.total, Some(6));
  assert!(!albums.has_more);

  let artists = rig.browse(Some("artists"), 20, 0).await;
  assert_eq!(
    names(&artists),
    pairs(&[
      ("plex:artist:machine-c:31", "Chipzel"),
      ("plex:artist:machine-a:12", "Maya Filipic"),
      ("plex:artist:machine-a:11", "Pornophonique"),
    ])
  );
  assert_eq!(artists.total, Some(3));

  let playlists = rig.browse(Some("playlists"), 20, 0).await;
  assert_eq!(
    names(&playlists),
    pairs(&[
      ("plex:playlist:machine-a:501", "Road trip"),
      ("plex:playlist:machine-b:601", "Night drive"),
    ])
  );
}

#[tokio::test]
async fn album_pages_come_from_the_deduped_index_with_exact_totals() {
  let rig = boot().await;
  let middle = rig.browse(Some("albums"), 2, 2).await;
  assert_eq!(
    names(&middle),
    pairs(&[(ALBUM_B, "Live at the Lagerfeuer"), (SECOND, "Second Album")])
  );
  assert_eq!(middle.total, Some(6));
  assert!(middle.has_more);

  let tail = rig.browse(Some("albums"), 2, 4).await;
  assert_eq!(
    names(&tail),
    pairs(&[(SECOND_FLAC, "Second Album (FLAC)"), (SPECTRA, "Spectra")])
  );
  assert_eq!(tail.total, Some(6));
  assert!(!tail.has_more);
  assert_eq!(
    rig.album_fetches(),
    [1, 1, 1],
    "every library is read into the index once"
  );
}

#[tokio::test]
async fn the_album_index_is_reused_until_the_servers_are_found_again() {
  let rig = boot().await;
  rig
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
  rig.browse(Some("albums"), 5, 0).await;
  rig.browse(Some("recently-played"), 5, 0).await;
  rig.browse(Some("recently-added"), 5, 0).await;
  assert_eq!(rig.album_fetches(), [1, 1, 1]);

  rig.provider.connectivity_changed(true).await;
  rig.browse(Some("albums"), 5, 0).await;
  assert_eq!(rig.album_fetches(), [2, 2, 2], "a fresh discovery rebuilds the index");
}

#[tokio::test]
async fn recent_folders_fold_every_copy_and_list_newest_first() {
  let rig = boot().await;
  let added = rig.browse(Some("recently-added"), 4, 0).await;
  assert_eq!(
    names(&added),
    pairs(&[
      (EMONA_SESSIONS, "The Emona Sessions"),
      (SPECTRA, "Spectra"),
      (ALBUM_B, "Live at the Lagerfeuer"),
      (SECOND_FLAC, "Second Album (FLAC)"),
    ])
  );
  assert_eq!(added.total, Some(6));
  assert!(added.has_more);

  let played = rig.browse(Some("recently-played"), 10, 0).await;
  assert_eq!(
    names(&played),
    pairs(&[
      (ALBUM_A, "8-bit lagerfeuer"),
      (ALBUM_B, "Live at the Lagerfeuer"),
      (EMONA_SESSIONS, "The Emona Sessions"),
    ]),
    "the kept copy carries the newest play of every copy"
  );
  assert_eq!(played.total, Some(3));
  assert!(!played.has_more);
}

#[tokio::test]
async fn an_album_node_lists_its_tracks_as_plex_uris_with_their_artist_album_and_art() {
  let rig = boot().await;
  let page = rig.browse(Some(ALBUM_A), 2, 0).await;
  assert_eq!(page.total, Some(3));
  assert!(page.has_more);
  let tracks: Vec<(String, String, String, String, String, bool)> = items(&page)
    .into_iter()
    .map(|item| match item {
      LibraryItem::Track(track) => (
        track.id,
        track.name,
        track.artist.id,
        track.album.id,
        track.image_id,
        track.saved,
      ),
      other => panic!("an album lists tracks, got {other:?}"),
    })
    .collect();
  let cover = format!("plex/img/96/{COVER_A}");
  let row = |id: &str, name: &str| {
    (
      id.to_owned(),
      name.to_owned(),
      "plex:artist:machine-a:11".to_owned(),
      ALBUM_A.to_owned(),
      cover.clone(),
      false,
    )
  };
  assert_eq!(tracks, vec![row(ROBOT, "Sad Robot"), row(INVADERS, "Space Invaders")]);

  let artist = rig.browse(Some("plex:artist:machine-a:11"), 10, 0).await;
  assert_eq!(
    names(&artist),
    pairs(&[
      (ALBUM_A, "8-bit lagerfeuer"),
      (SECOND, "Second Album"),
      (SECOND_FLAC, "Second Album (FLAC)"),
    ])
  );
}

#[tokio::test]
async fn search_honors_the_requested_kinds_and_keeps_one_copy_of_each_hit() {
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
  let ids: Vec<&str> = result
    .items
    .iter()
    .map(|item| match item {
      LibraryItem::Track(track) => track.id.as_str(),
      other => panic!("only tracks were asked for, got {other:?}"),
    })
    .collect();
  assert_eq!(
    ids,
    vec![ROBOT, LIVE_ROBOT],
    "the other servers' copies of sad robot fold away"
  );
  let searches: Vec<Seen> = rig
    .stub
    .seen()
    .into_iter()
    .filter(|seen| seen.path.ends_with("/search"))
    .collect();
  assert_eq!(searches.len(), 3, "one search per server: {searches:?}");
  assert!(
    searches
      .iter()
      .all(|seen| seen.query.get("type").map(String::as_str) == Some("10"))
  );

  let albums = rig
    .provider
    .search(LibrarySearchRequest {
      query: "lagerfeuer".into(),
      kinds: Some(vec![ItemKind::Album]),
      limit: 5,
      offset: 0,
    })
    .await
    .expect("the album search");
  let albums: Vec<&str> = albums
    .items
    .iter()
    .map(|item| match item {
      LibraryItem::Album(album) => album.id.as_str(),
      other => panic!("only albums were asked for, got {other:?}"),
    })
    .collect();
  assert_eq!(albums, vec![ALBUM_A, ALBUM_B]);

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
  assert!(
    matches!(playlists.items.as_slice(), [LibraryItem::Playlist(list)] if list.uri == "plex:playlist:machine-a:501")
  );
}

#[tokio::test]
async fn playing_an_album_streams_each_track_directly_or_through_the_transcoder() {
  let rig = boot().await;
  rig.play(ALBUM_A, None).await.expect("the album plays");
  let tower = rig.stub.url(MACHINE_A);
  let signature = format!("X-Plex-Client-Identifier={CLIENT_ID}&X-Plex-Product=bridgething&X-Plex-Token={TOKEN_A}");
  assert_eq!(
    rig.played_urls(),
    vec![format!("{tower}/library/parts/1001/1700000000/file.mp3?{signature}")]
  );
  assert!(!rig.backend.last_source().expect("a source").live);

  let state = rig.snapshot_for(ROBOT).await;
  let track = state.track.expect("the catalog track");
  assert_eq!(track.title.as_deref(), Some("Sad Robot"));
  assert_eq!(track.artist.as_deref(), Some("Pornophonique"));
  assert_eq!(track.artist_uri.as_deref(), Some("plex:artist:machine-a:11"));
  assert_eq!(track.album.as_deref(), Some("8-bit lagerfeuer"));
  assert_eq!(track.album_uri.as_deref(), Some(ALBUM_A));
  assert_eq!(track.artwork_id, Some(format!("plex/img/248/{COVER_A}")));
  assert_eq!(track.duration_ms, Some(212_000));
  assert_eq!(track.liked, Some(false));
  assert_eq!(track.track_number, Some(1));
  assert_eq!(state.playback.queue_index, Some(0));
  assert_eq!(state.playback.queue_count, Some(3));
  let context = state.context.expect("the album is the context");
  assert_eq!(context.uri, ALBUM_A);
  assert_eq!(context.name.as_deref(), Some("8-bit lagerfeuer"));
  let queue = rig.queue_with(&[INVADERS, MACHINE]).await;
  assert_eq!(queue.items[0].artwork_id, Some(format!("plex/img/96/{COVER_A}")));
  assert_eq!(rig.hub.now_playing().current_source().as_deref(), Some("plex"));
  assert!(
    eventually(|| !rig.backend.presentations().is_empty()).await,
    "the lock screen hears about the track"
  );
  let shown = rig.backend.presentations().pop().expect("a presentation");
  assert_eq!(shown.title, "Sad Robot");
  assert_eq!(shown.artist.as_deref(), Some("Pornophonique"));
  assert_eq!(shown.album.as_deref(), Some("8-bit lagerfeuer"));
  assert_eq!(
    shown.artwork.as_deref(),
    Some(b"art:machine-a:/library/metadata/101/thumb/1700000000".as_slice()),
    "the cover rides to the lock screen through the photo transcoder"
  );

  rig
    .backend
    .last_sink()
    .expect("the first sink")
    .on_status(StreamStatus::Ended);
  rig.snapshot_for(INVADERS).await;
  assert!(eventually(|| rig.played_urls().len() == 2).await);
  assert_eq!(
    rig.played_urls().last(),
    Some(&format!("{tower}/library/parts/1002/1700000000/file.flac?{signature}"))
  );

  rig
    .backend
    .last_sink()
    .expect("the second sink")
    .on_status(StreamStatus::Ended);
  rig.snapshot_for(MACHINE).await;
  assert!(eventually(|| rig.played_urls().len() == 3).await);
  let transcoded = rig.played_urls().pop().expect("the third url");
  assert!(
    transcoded.starts_with(&format!(
      "{tower}/music/:/transcode/universal/start.mp3?path=%2Flibrary%2Fmetadata%2F1003&"
    )),
    "wma goes through the mp3 transcoder: {transcoded}"
  );
  assert!(transcoded.ends_with(&format!("X-Plex-Token={TOKEN_A}")));
  assert!(signed(
    &rig.stub.on(MACHINE_A, "/music/:/transcode/universal/start.mp3"),
    TOKEN_A
  ));
  assert!(
    !rig.backend.transport_calls().contains(&StreamCall::Stop),
    "an ended track is not stopped again"
  );
}

#[tokio::test]
async fn skip_verbs_walk_the_queue() {
  let rig = boot().await;
  rig.play(ROBOT, Some(ALBUM_A)).await.expect("the album plays");
  rig.snapshot_for(ROBOT).await;

  rig.provider.skip_next().await.expect("skip next");
  rig.snapshot_for(INVADERS).await;
  assert!(rig.backend.transport_calls().contains(&StreamCall::Stop));
  rig.provider.skip_to_index(0).await.expect("skip to the next upcoming");
  rig.snapshot_for(MACHINE).await;
  assert!(rig.provider.skip_to_index(0).await.is_err());

  rig.provider.skip_prev().await.expect("skip prev");
  let played = rig.played_urls();
  assert_eq!(played.len(), 4);
  assert!(played[3].contains("/library/parts/1002/"));

  rig.provider.skip_next().await.expect("skip next");
  rig.provider.skip_next().await.expect("skip next past the end stops");
  assert!(eventually(|| rig.hub.now_playing().current_source().is_none()).await);
}

#[tokio::test]
async fn the_timeline_follows_play_pause_resume_skip_and_end_on_the_owning_server() {
  let rig = boot().await;
  rig.play(ALBUM_B, None).await.expect("the album plays");
  rig.snapshot_for(LIVE_ROBOT).await;
  let sink = rig.backend.last_sink().expect("the sink");
  assert!(
    rig.timeline(MACHINE_B, "2002").is_empty(),
    "nothing reports before the phone plays"
  );

  sink.on_status(StreamStatus::Playing);
  assert!(eventually(|| rig.timeline(MACHINE_B, "2002").len() == 1).await);
  sink.on_timing(timing(5_000));
  sink.on_status(StreamStatus::Paused);
  sink.on_status(StreamStatus::Playing);
  assert!(eventually(|| rig.timeline(MACHINE_B, "2002").len() == 3).await);
  rig.provider.skip_next().await.expect("skip next");
  assert!(
    eventually(|| rig.timeline(MACHINE_B, "2002").len() == 4).await,
    "the skip stops the timeline: {:?}",
    rig.timeline(MACHINE_B, "2002")
  );

  let beats = rig.timeline(MACHINE_B, "2002");
  let states: Vec<&str> = beats.iter().map(|(state, _, _)| state.as_str()).collect();
  assert_eq!(states, ["playing", "paused", "playing", "stopped"]);
  assert!(beats[0].1 < 1_000, "the first beat starts at the top: {beats:?}");
  assert_eq!(beats[1].1, 5_000);
  assert!((5_000..6_000).contains(&beats[2].1), "{beats:?}");
  assert!((beats[2].1..6_000).contains(&beats[3].1), "{beats:?}");
  assert!(beats.iter().all(|(_, _, duration)| *duration == Some(200_000)));
  assert!(signed(&rig.stub.on(MACHINE_B, "/:/timeline"), TOKEN_B));
  assert!(rig.stub.on(MACHINE_A, "/:/timeline").is_empty());

  rig.snapshot_for(IVY).await;
  let sink = rig.backend.last_sink().expect("the second sink");
  sink.on_status(StreamStatus::Playing);
  assert!(eventually(|| rig.timeline(MACHINE_B, "2003").len() == 1).await);
  sink.on_status(StreamStatus::Ended);
  assert!(eventually(|| rig.timeline(MACHINE_B, "2003").len() == 2).await);
  let ivy = rig.timeline(MACHINE_B, "2003");
  assert_eq!(ivy[1], ("stopped".to_owned(), 190_000, Some(190_000)));
}

#[tokio::test]
async fn favorites_rate_on_the_owning_server_and_flip_liked_on_the_wire() {
  let rig = boot().await;
  rig.play(ROBOT, None).await.expect("the track plays");
  rig.snapshot_for(ROBOT).await;

  rig
    .provider
    .favorites_set(
      ItemRef {
        uri: ROBOT.into(),
        kind: ItemKind::Track,
        persistent_id: None,
      },
      true,
    )
    .await
    .expect("like");
  let rate = rig.stub.on(MACHINE_A, "/:/rate");
  assert_eq!(rate.len(), 1);
  assert_eq!(rate[0].method, "PUT");
  assert_eq!(rate[0].query.get("key").map(String::as_str), Some("1001"));
  assert_eq!(rate[0].query.get("rating").map(String::as_str), Some("10"));
  assert_eq!(
    rate[0].query.get("identifier").map(String::as_str),
    Some("com.plexapp.plugins.library")
  );
  assert!(signed(&rate, TOKEN_A));
  rig
    .peer
    .wait("the liked flag on the wire", |msg| {
      snapshot_of(msg).filter(|state| state.track.as_ref().and_then(|track| track.liked) == Some(true))
    })
    .await;
  assert_eq!(
    rig.contains(&[ROBOT, INVADERS, GAME_OVER, IVY, ALBUM_A]).await,
    vec![true, false, true, true, false]
  );

  rig
    .provider
    .favorites_toggle(ItemRef {
      uri: ROBOT.into(),
      kind: ItemKind::Track,
      persistent_id: None,
    })
    .await
    .expect("toggle off");
  let rate = rig.stub.on(MACHINE_A, "/:/rate");
  assert_eq!(rate.len(), 2);
  assert_eq!(rate[1].query.get("rating").map(String::as_str), Some("-1"));
  assert_eq!(rig.contains(&[ROBOT]).await, vec![false]);
  assert!(rig.stub.on(MACHINE_B, "/:/rate").is_empty());
  assert!(
    rig
      .provider
      .favorites_set(
        ItemRef {
          uri: "plex:playlist:machine-a:501".into(),
          kind: ItemKind::Playlist,
          persistent_id: None,
        },
        true,
      )
      .await
      .is_err(),
    "playlists cannot be rated"
  );
}

#[tokio::test]
async fn liking_an_album_updates_the_index_in_place() {
  let rig = boot().await;
  rig.browse(Some("albums"), 5, 0).await;
  let album = |uri: &str| ItemRef {
    uri: uri.into(),
    kind: ItemKind::Album,
    persistent_id: None,
  };
  rig.provider.favorites_set(album(ALBUM_A), true).await.expect("like");
  let rate = rig.stub.on(MACHINE_A, "/:/rate");
  assert_eq!(rate.len(), 1);
  assert_eq!(rate[0].query.get("key").map(String::as_str), Some("101"));
  assert_eq!(rig.contains(&[ALBUM_A, SPECTRA]).await, vec![true, false]);
  let page = rig
    .provider
    .favorites_list(LibraryFavoritesListRequest { limit: 10, offset: 0 })
    .await
    .expect("the liked page");
  assert!(
    page
      .items
      .iter()
      .any(|item| matches!(item, LibraryItem::Album(liked) if liked.id == ALBUM_A)),
    "{:?}",
    page.items
  );

  rig.provider.favorites_set(album(ALBUM_A), false).await.expect("unlike");
  assert_eq!(rig.contains(&[ALBUM_A]).await, vec![false]);
  assert_eq!(rig.album_fetches(), [1, 1, 1], "a like never refetches the index");
}

#[tokio::test]
async fn liked_songs_play_the_liked_tracks_of_every_server_newest_first() {
  let rig = boot().await;
  rig.play(LIKED, None).await.expect("the liked songs play");
  let state = rig.snapshot_for(IVY).await;
  let context = state.context.expect("a context");
  assert_eq!(context.uri, LIKED);
  assert_eq!(context.name.as_deref(), Some("Liked Songs"));
  assert_eq!(state.track.and_then(|track| track.liked), Some(true));
  rig.queue_with(&[GAME_OVER]).await;
  assert!(rig.played_urls()[0].starts_with(&rig.stub.url(MACHINE_B)));
  for host in [MACHINE_A, MACHINE_B] {
    let liked = rig
      .stub
      .seen()
      .into_iter()
      .find(|seen| seen.host == host && seen.query.contains_key("userRating"))
      .expect("each server was asked for its likes");
    assert_eq!(liked.query.get("userRating").map(String::as_str), Some("10"));
    assert_eq!(liked.query.get("sort").map(String::as_str), Some("lastRatedAt:desc"));
  }
}

#[tokio::test]
async fn assets_resolve_through_the_photo_transcoder_of_the_owning_server() {
  let rig = boot().await;
  let asset = rig
    .provider
    .asset(&format!("plex/img/96/{COVER_B}"))
    .await
    .expect("the asset call")
    .expect("bytes");
  assert_eq!(
    asset.bytes,
    b"art:machine-b:/library/metadata/202/thumb/1700000000@96".to_vec()
  );
  assert_eq!(asset.mime.as_deref(), Some("image/jpeg"));
  let photo = rig.stub.on(MACHINE_B, "/photo/:/transcode");
  assert!(signed(&photo, TOKEN_B));
  assert_eq!(
    photo[0].query.get("url").map(String::as_str),
    Some("/library/metadata/202/thumb/1700000000")
  );
  assert_eq!(photo[0].query.get("width").map(String::as_str), Some("600"));
  assert!(rig.stub.on(MACHINE_A, "/photo/:/transcode").is_empty());
  assert_eq!(rig.provider.asset("spotify/img/96/iabc").await.unwrap(), None);
}

#[tokio::test]
async fn lyrics_come_from_the_playing_tracks_streams_synced_first_then_plain() {
  let rig = boot().await;
  assert_eq!(rig.lyrics().await, None, "nothing plays, so nothing is sung");

  rig.play(ROBOT, None).await.expect("the track plays");
  rig.snapshot_for(ROBOT).await;
  let synced = rig.lyrics().await.expect("synced lyrics");
  assert_eq!(synced.source, "plex");
  assert_eq!(synced.plain.as_deref(), Some("first line\nsecond line"));
  let lines = synced.synced.expect("synced lines");
  assert_eq!(lines.len(), 2);
  assert_eq!(lines[1].start_ms, 4_200);
  assert_eq!(lines[1].text, "second line");

  rig.play(INVADERS, None).await.expect("the next track plays");
  rig.snapshot_for(INVADERS).await;
  let plain = rig.lyrics().await.expect("plain lyrics");
  assert_eq!(plain.synced, None);
  assert_eq!(plain.plain.as_deref(), Some(PLAIN_LYRICS));
  assert_eq!(rig.stub.on(MACHINE_A, "/library/streams/9002").len(), 1);
  assert_eq!(rig.stub.on(MACHINE_A, "/library/streams/9003").len(), 1);

  rig.play(EMONA, None).await.expect("a track with untimed lrc plays");
  rig.snapshot_for(EMONA).await;
  let fallback = rig.lyrics().await.expect("the text stream after the untimed lrc");
  assert_eq!(fallback.synced, None);
  assert_eq!(fallback.plain.as_deref(), Some(EMONA_LYRICS));
  assert_eq!(rig.stub.on(MACHINE_A, "/library/streams/9004").len(), 1);

  rig.play(MACHINE, None).await.expect("a track without lyrics plays");
  rig.snapshot_for(MACHINE).await;
  assert_eq!(rig.lyrics().await, None);

  let streams: Vec<Seen> = rig
    .stub
    .seen()
    .into_iter()
    .filter(|seen| seen.path.starts_with("/library/streams/"))
    .collect();
  assert_eq!(streams.len(), 5);
  assert!(
    streams.iter().all(|seen| seen
      .accept
      .as_deref()
      .is_some_and(|accept| !accept.contains("application/json"))),
    "a pms answers 404 to a json accept on lyric streams: {streams:?}"
  );
  assert!(signed(&streams, TOKEN_A));
}

#[tokio::test]
async fn a_track_station_uses_the_servers_nearest_tracks_after_the_seed() {
  let rig = boot().await;
  let station = "plex:station:machine-a:track:1001";
  rig.play(station, None).await.expect("the station plays");
  let state = rig.snapshot_for(ROBOT).await;
  assert_eq!(state.context.map(|context| context.uri).as_deref(), Some(station));
  rig.queue_with(&[GAME_OVER, EMONA]).await;
  let nearest = rig.stub.on(MACHINE_A, "/library/metadata/1001/nearest");
  assert_eq!(nearest[0].query.get("limit").map(String::as_str), Some("50"));
  assert!(rig.stub.on(MACHINE_A, "/library/metadata/11/similar").is_empty());
}

#[tokio::test]
async fn a_track_station_without_nearest_tracks_falls_back_to_the_artist_mix() {
  let rig = boot().await;
  rig
    .play("plex:station:machine-b:track:2002", None)
    .await
    .expect("the station plays");
  rig.snapshot_for(LIVE_ROBOT).await;
  rig
    .queue_with(&[IVY, HUMAN, OUTRO, FOCUS, ATTIC_GAME_OVER, ATTIC_ROBOT])
    .await;
  assert_eq!(rig.stub.on(MACHINE_B, "/library/metadata/2002/nearest").len(), 1);
  assert_eq!(rig.stub.on(MACHINE_B, "/library/metadata/21/similar").len(), 1);
  let popular: Vec<String> = rig
    .stub
    .on(MACHINE_B, "/library/sections/3/all")
    .into_iter()
    .filter(|seen| seen.query.get("sort").map(String::as_str) == Some("ratingCount:desc"))
    .filter_map(|seen| seen.query.get("artist.id").cloned())
    .collect();
  assert_eq!(popular.len(), 2);
  assert!(popular.contains(&"21".to_owned()) && popular.contains(&"22".to_owned()));
}

async fn voice_pick(rig: &Rig, target: &str, kind: Option<NluTargetType>) -> Option<String> {
  let resolver = rig
    .provider
    .voice_resolver()
    .expect("a signed-in account resolves voice");
  resolver
    .decorate(NluResolvedIntent {
      intent: "PLAY".into(),
      slots: NluSlots {
        target: Some(target.into()),
        target_type: kind,
        ..NluSlots::default()
      },
      transcript: String::new(),
      alternates: None,
    })
    .await
    .expect("the servers answered")
    .slots
    .uri
}

#[tokio::test]
async fn voice_names_an_artist_liked_songs_and_an_artist_station() {
  let rig = boot().await;
  assert_eq!(
    voice_pick(&rig, "chipzel", Some(NluTargetType::Artist))
      .await
      .as_deref(),
    Some("plex:artist:machine-c:31"),
    "the shared local server is preferred over the owned remote one"
  );
  assert_eq!(voice_pick(&rig, "my liked songs", None).await.as_deref(), Some(LIKED));
  let station = voice_pick(&rig, "maya filipic", Some(NluTargetType::Station))
    .await
    .expect("a station");
  assert_eq!(station, "plex:station:machine-a:artist:12");
  rig.play(&station, None).await.expect("the station plays");
  rig.snapshot_for(EMONA).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_catalog_offers_plex_only_when_the_host_can_play() {
  let session = boot_session(SESSION_APP, Arc::new(MemorySecrets::default()), None);
  assert!(session.provider_infos().iter().all(|info| info.id != "plex"));

  let (session, _) = stream_session(SESSION_APP);
  let info = session
    .provider_infos()
    .into_iter()
    .find(|info| info.id == "plex")
    .expect("plex is offered");
  assert_eq!(info.display_name, "Plex");
  assert_eq!(info.sign_in, SignInMethod::Handshake);
  assert!(info.available && !info.connected);
}

#[tokio::test(flavor = "multi_thread")]
async fn plex_refuses_handed_credentials_and_disconnecting_forgets_the_token() {
  let (session, secrets) = stream_session(SESSION_APP);
  for credentials in [
    ProviderCredentials::ServerLogin {
      server_url: "https://plex.example:32400".into(),
      username: "demo".into(),
      password: "demo".into(),
    },
    ProviderCredentials::OauthTokens {
      access_token: "a".into(),
      refresh_token: "r".into(),
    },
  ] {
    assert!(session.complete_provider_auth("plex", credentials).await.is_err());
  }
  assert_eq!(secrets.get(KEY_TOKEN.into()), None);
  assert!(session.provider_infos().iter().all(|info| !info.connected));

  secrets.set(KEY_TOKEN.into(), ACCOUNT_TOKEN.into());
  session.disconnect_provider("plex").await;
  assert_eq!(secrets.get(KEY_TOKEN.into()), None);
}
