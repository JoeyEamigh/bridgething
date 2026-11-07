mod client;
mod index;
mod voice;

use std::{
  cmp::Reverse,
  sync::{Arc, Mutex},
  time::{Duration, Instant},
};

use bridgething_io::{HttpExecutor, HttpTransport as IoHttpTransport};
use libbridgething::{
  Album as WireAlbum, Artist as WireArtist, BrowseEntry, BrowseResult, FavoritesPage, ItemKind, ItemRef, LibraryItem,
  Lyrics, MusicProvider, PlaybackContext, Playlist as WirePlaylist, RecommendationsResult, SearchResult,
  Track as WireTrack,
  gateway::{
    ContextResolveReply, FavoritesSet, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, PlayUri, QueueUri, TrackIdentity,
  },
};
use tokio::{sync::mpsc, task::JoinHandle};

pub use self::client::{PLEX_TV, PlexError, PlexServer};
use self::{
  client::{Directory, ItemType, LIKED_RATING, Metadata, PlexAccount, Resource, TimelineState},
  index::{Found, Indexed, Indexes, Library, Query, Registry, Servers, dedupe, gather, interleave},
};
use crate::{
  backend::{SecretStore, StreamBackend},
  lyrics::lrc,
  provider::{
    AssetBytes, AuthObserver, PlayerTransport, Provider, ProviderAuthState, ProviderError, ProviderLink,
    art::{ArtCache, ArtResolver, ImageAssetCodec},
    library::{
      ALBUMS_NODE_ID, ARTISTS_NODE_ID, NEWEST_NODE_ID, PLAYLISTS_NODE_ID, RANDOM_NODE_ID, RECENTS_NODE_ID, listing,
      page, staple_root,
    },
    none_if_empty,
    playback::{PlayReport, PlayReporter, Presentation, QueueEntry, ReportedState, StreamPlayback},
  },
  voice::{catalog::CatalogResolver, dispatcher::VoiceCatalogResolver},
};

pub const PROVIDER_NAME: &str = "plex";
pub const SCHEME: &str = "plex";
pub const KEY_TOKEN: &str = "plex.token";
pub const KEY_CLIENT_ID: &str = "plex.client_id";

const ART_SOURCE_PREFIX: &str = "art:";
const IMAGE_CODEC: ImageAssetCodec = ImageAssetCodec {
  namespace: "plex/img/",
  short_form: Some(('a', ART_SOURCE_PREFIX)),
};
const DEFAULT_HERO_EDGE: u32 = 248;
const DEFAULT_THUMB_EDGE: u32 = 96;
const PIN_POLL: Duration = Duration::from_secs(2);
const PIN_LIFETIME: Duration = Duration::from_secs(1800);
const LIKED_ID: &str = "songs";
const LIKED_TITLE: &str = "Liked Songs";
const LIKED_CAP: u32 = 1000;
const PLAYLIST_CAP: u32 = 5000;
const STATION_SIZE: u32 = 50;
const STATION_SEED_TRACKS: u32 = 10;
const STATION_SIMILAR_ARTISTS: usize = 8;
const STATION_SIMILAR_TRACKS: u32 = 5;
const LIKED_FILTER: (&str, &str) = ("userRating", "10");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlexConfig {
  pub tv_base: String,
  pub client_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seed {
  Artist,
  Track,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
  Track,
  Album,
  Artist,
  Playlist,
  Station(Seed),
  Liked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Ref {
  kind: Kind,
  server: String,
  key: String,
}

fn uri(kind: Kind, server: &str, key: &str) -> String {
  match kind {
    Kind::Liked => format!("{SCHEME}:liked:{LIKED_ID}"),
    Kind::Station(Seed::Artist) => format!("{SCHEME}:station:{server}:artist:{key}"),
    Kind::Station(Seed::Track) => format!("{SCHEME}:station:{server}:track:{key}"),
    Kind::Track => format!("{SCHEME}:track:{server}:{key}"),
    Kind::Album => format!("{SCHEME}:album:{server}:{key}"),
    Kind::Artist => format!("{SCHEME}:artist:{server}:{key}"),
    Kind::Playlist => format!("{SCHEME}:playlist:{server}:{key}"),
  }
}

fn parse_uri(uri: &str) -> Option<Ref> {
  let rest = uri.strip_prefix(SCHEME)?.strip_prefix(':')?;
  let (tag, rest) = rest.split_once(':')?;
  if tag == "liked" {
    return (rest == LIKED_ID).then(|| Ref {
      kind: Kind::Liked,
      server: String::new(),
      key: LIKED_ID.into(),
    });
  }
  let (server, key) = rest.split_once(':')?;
  let (kind, key) = match tag {
    "track" => (Kind::Track, key),
    "album" => (Kind::Album, key),
    "artist" => (Kind::Artist, key),
    "playlist" => (Kind::Playlist, key),
    "station" => match key.split_once(':')? {
      ("artist", seed) => (Kind::Station(Seed::Artist), seed),
      ("track", seed) => (Kind::Station(Seed::Track), seed),
      _ => return None,
    },
    _ => return None,
  };
  (!server.is_empty() && !key.is_empty() && !key.contains(':')).then(|| Ref {
    kind,
    server: server.to_owned(),
    key: key.to_owned(),
  })
}

fn art_source(server: &str, path: Option<&str>) -> Option<String> {
  path
    .filter(|path| !path.is_empty())
    .map(|path| format!("{ART_SOURCE_PREFIX}{server}:{path}"))
}

fn failed(error: PlexError) -> ProviderError {
  match error {
    PlexError::Unauthorized => ProviderError::NotAuthenticated,
    other => ProviderError::Failed(other.to_string()),
  }
}

fn kind_of_item(item: &Metadata) -> Option<Kind> {
  match item.kind.as_str() {
    "track" => Some(Kind::Track),
    "album" => Some(Kind::Album),
    "artist" => Some(Kind::Artist),
    "playlist" => Some(Kind::Playlist),
    _ => None,
  }
}

struct Art {
  registry: Arc<Registry>,
}

impl ArtResolver for Art {
  fn asset_id(&self, source: &str, max_edge: u32) -> Option<String> {
    IMAGE_CODEC.asset_id(source, max_edge)
  }

  fn url(&self, source: &str) -> String {
    let Some((machine, path)) = source
      .strip_prefix(ART_SOURCE_PREFIX)
      .and_then(|rest| rest.split_once(':'))
    else {
      return source.to_owned();
    };
    match self.registry.server(machine) {
      Some(server) => server.photo_url(path),
      None => source.to_owned(),
    }
  }
}

struct Beat {
  server: Arc<PlexServer>,
  key: String,
  state: TimelineState,
  position_ms: u32,
  duration_ms: Option<u32>,
}

struct Timeline {
  registry: Arc<Registry>,
  worker: Mutex<Option<mpsc::UnboundedSender<Beat>>>,
}

impl Timeline {
  fn send(&self, beat: Beat) {
    let mut worker = self.worker.lock().unwrap();
    let sender = worker.get_or_insert_with(|| {
      let (tx, mut rx) = mpsc::unbounded_channel::<Beat>();
      tokio::spawn(async move {
        while let Some(beat) = rx.recv().await {
          if let Err(error) = beat
            .server
            .timeline(&beat.key, beat.state, beat.position_ms, beat.duration_ms)
            .await
          {
            tracing::debug!(%error, key = beat.key, "a plex timeline update did not land");
          }
        }
      });
      tx
    });
    let _ = sender.send(beat);
  }
}

impl PlayReporter for Timeline {
  fn report(&self, report: PlayReport) {
    let PlayReport::Progress {
      uri,
      state,
      position_ms,
      duration_ms,
    } = report
    else {
      return;
    };
    let Some(Ref {
      kind: Kind::Track,
      server,
      key,
    }) = parse_uri(&uri)
    else {
      return;
    };
    let Some(server) = self.registry.server(&server) else {
      return;
    };
    let state = match state {
      ReportedState::Playing => TimelineState::Playing,
      ReportedState::Paused => TimelineState::Paused,
      ReportedState::Stopped => TimelineState::Stopped,
    };
    self.send(Beat {
      server,
      key,
      state,
      position_ms,
      duration_ms,
    });
  }
}

struct Listed {
  items: Vec<Found>,
  total: Option<u32>,
  has_more: bool,
}

struct Core {
  account: PlexAccount,
  secrets: Arc<dyn SecretStore>,
  registry: Arc<Registry>,
  art: Arc<ArtCache>,
  art_origin: HttpExecutor,
  http: HttpExecutor,
  resolver: Arc<Art>,
  playback: StreamPlayback,
  indexes: Indexes,
  edges: Mutex<(u32, u32)>,
  link: Mutex<Option<ProviderLink>>,
  auth_observer: Mutex<Option<AuthObserver>>,
  auth_task: Mutex<Option<JoinHandle<()>>>,
  authorized: Mutex<bool>,
}

impl Core {
  fn notify_auth(&self, state: ProviderAuthState) {
    let observer = self.auth_observer.lock().unwrap().clone();
    if let Some(observer) = observer {
      observer(state);
    }
  }

  fn edges(&self) -> (u32, u32) {
    *self.edges.lock().unwrap()
  }

  fn require(&self) -> Result<(), ProviderError> {
    if self.link.lock().unwrap().is_none() {
      return Err(ProviderError::Detached);
    }
    if !*self.authorized.lock().unwrap() {
      return Err(ProviderError::NotAuthenticated);
    }
    Ok(())
  }

  fn token(&self) -> Option<String> {
    self.secrets.get(KEY_TOKEN.into()).filter(|token| !token.is_empty())
  }

  fn server(&self, machine: &str) -> Result<Arc<PlexServer>, ProviderError> {
    self
      .registry
      .server(machine)
      .ok_or_else(|| ProviderError::Failed(format!("the plex server {machine} is not reachable")))
  }

  async fn sign_in(self: Arc<Self>) {
    let token = match self.token() {
      Some(token) => token,
      None => match self.pair().await {
        Some(token) => token,
        None => return,
      },
    };
    match self.discover(&token).await {
      Ok(()) => {
        *self.authorized.lock().unwrap() = true;
        self.notify_auth(ProviderAuthState::Authenticated);
      }
      Err(PlexError::Unauthorized) => {
        self.secrets.remove(KEY_TOKEN.into());
        self.notify_auth(ProviderAuthState::Failed {
          reason: "plex signed this device out; sign in again".into(),
        });
      }
      Err(error) => {
        tracing::warn!(%error, "the plex sign-in failed");
        self.notify_auth(ProviderAuthState::Failed {
          reason: error.to_string(),
        });
      }
    }
  }

  async fn pair(&self) -> Option<String> {
    let pin = match self.account.create_pin().await {
      Ok(pin) => pin,
      Err(error) => {
        self.notify_auth(ProviderAuthState::Failed {
          reason: error.to_string(),
        });
        return None;
      }
    };
    let url = self.account.auth_url(&pin.code);
    self.notify_auth(ProviderAuthState::Pending {
      user_code: None,
      verification_url: Some(url.clone()),
      verification_url_complete: Some(url),
    });
    let lifetime = pin.expires_in.map_or(PIN_LIFETIME, Duration::from_secs);
    let deadline = Instant::now() + lifetime;
    while Instant::now() < deadline {
      tokio::time::sleep(PIN_POLL).await;
      match self.account.pin_token(pin.id).await {
        Ok(Some(token)) => {
          self.secrets.set(KEY_TOKEN.into(), token.clone());
          return Some(token);
        }
        Ok(None) => {}
        Err(PlexError::Http(404)) => break,
        Err(error) => tracing::debug!(%error, "a plex pin poll missed"),
      }
    }
    self.notify_auth(ProviderAuthState::Failed {
      reason: "the plex sign-in expired; try again".into(),
    });
    None
  }

  async fn discover(&self, token: &str) -> Result<(), PlexError> {
    let resources = self.account.resources(token).await?;
    let connected = futures::future::join_all(
      resources
        .into_iter()
        .filter(|resource| resource.is_server() && resource.access_token.is_some())
        .map(|resource| self.connect(resource)),
    )
    .await;
    let mut connected: Vec<_> = connected.into_iter().flatten().collect();
    connected.sort_by(|(a, _, a_rank), (b, _, b_rank)| a_rank.cmp(b_rank).then_with(|| a.name.cmp(&b.name)));
    let mut servers = Servers::default();
    for (server, sections, _) in connected {
      servers.libraries.extend(sections.into_iter().map(|section| Library {
        server: server.clone(),
        section: section.key,
      }));
      servers.servers.push(server);
    }
    tracing::info!(
      servers = servers.servers.len(),
      libraries = servers.libraries.len(),
      "plex music libraries found"
    );
    self.registry.replace(servers);
    Ok(())
  }

  async fn connect(&self, resource: Resource) -> Option<(Arc<PlexServer>, Vec<Directory>, (u8, bool))> {
    let token = resource.access_token.clone()?;
    let mut connections = resource.connections.clone();
    connections.sort_by_key(|connection| connection.rank());
    let reachable = futures::future::join_all(
      connections
        .iter()
        .map(|connection| self.account.reachable(&connection.uri, &token)),
    )
    .await;
    let Some((chosen, _)) = connections.iter().zip(reachable).find(|(_, ok)| *ok) else {
      tracing::info!(server = resource.name, "no plex connection answered");
      return None;
    };
    let server = Arc::new(PlexServer::new(
      &resource.client_identifier,
      &resource.name,
      &chosen.uri,
      &token,
      self.account.client_id(),
      self.http.clone(),
    ));
    match server.music_sections().await {
      Ok(sections) if !sections.is_empty() => Some((server, sections, (chosen.rank(), !resource.owned))),
      Ok(_) => None,
      Err(error) => {
        tracing::info!(%error, server = resource.name, "a plex server did not list its libraries");
        None
      }
    }
  }

  fn track_item(&self, (server, track): &Found) -> LibraryItem {
    let (_, thumb) = self.edges();
    let machine = &server.machine;
    let artist = WireArtist {
      id: track
        .grandparent_rating_key
        .as_deref()
        .map(|key| uri(Kind::Artist, machine, key))
        .unwrap_or_default(),
      name: track.artist_name().unwrap_or_default().to_owned(),
      artwork_id: None,
    };
    LibraryItem::Track(WireTrack {
      id: uri(Kind::Track, machine, &track.rating_key),
      name: track.title.clone(),
      album: WireAlbum {
        id: track
          .parent_rating_key
          .as_deref()
          .map(|key| uri(Kind::Album, machine, key))
          .unwrap_or_default(),
        name: track.parent_title.clone().unwrap_or_default(),
        artwork_id: None,
      },
      artists: vec![artist.clone()],
      artist,
      duration_ms: track.duration.unwrap_or(0),
      image_id: art_source(machine, track.artwork())
        .and_then(|source| IMAGE_CODEC.asset_id(&source, thumb))
        .unwrap_or_default(),
      saved: track.liked(),
    })
  }

  fn hero(&self, server: &PlexServer, item: &Metadata) -> Option<String> {
    let (hero, _) = self.edges();
    art_source(&server.machine, item.artwork()).and_then(|source| IMAGE_CODEC.asset_id(&source, hero))
  }

  fn item(&self, found: &Found) -> Option<LibraryItem> {
    let (server, item) = found;
    let machine = &server.machine;
    Some(match kind_of_item(item)? {
      Kind::Track => self.track_item(found),
      Kind::Album => LibraryItem::Album(WireAlbum {
        id: uri(Kind::Album, machine, &item.rating_key),
        name: item.title.clone(),
        artwork_id: self.hero(server, item),
      }),
      Kind::Artist => LibraryItem::Artist(WireArtist {
        id: uri(Kind::Artist, machine, &item.rating_key),
        name: item.title.clone(),
        artwork_id: self.hero(server, item),
      }),
      Kind::Playlist => LibraryItem::Playlist(WirePlaylist {
        uri: uri(Kind::Playlist, machine, &item.rating_key),
        name: item.title.clone(),
        owner_name: None,
        track_count: item.leaf_count,
        artwork_id: self.hero(server, item),
      }),
      Kind::Station(_) | Kind::Liked => return None,
    })
  }

  fn items(&self, found: &[Found]) -> Vec<LibraryItem> {
    found.iter().filter_map(|found| self.item(found)).collect()
  }

  fn entry(&self, (server, track): &Found) -> Option<QueueEntry> {
    let machine = &server.machine;
    Some(QueueEntry {
      url: server.stream_url(track)?,
      presentation: Presentation {
        uri: uri(Kind::Track, machine, &track.rating_key),
        title: none_if_empty(&track.title),
        artist: track.artist_name().map(str::to_owned),
        artist_uri: track
          .grandparent_rating_key
          .as_deref()
          .map(|key| uri(Kind::Artist, machine, key)),
        album: track.parent_title.clone(),
        album_uri: track
          .parent_rating_key
          .as_deref()
          .map(|key| uri(Kind::Album, machine, key)),
        artwork: art_source(machine, track.artwork()),
        duration_ms: track.duration,
        liked: Some(track.liked()),
        track_number: track.index.and_then(|number| u16::try_from(number).ok()),
      },
      queued: false,
    })
  }

  async fn indexed(&self, which: Indexed, order: Order, offset: u32, limit: u32) -> Result<Listed, ProviderError> {
    let all = self
      .indexes
      .get(self.registry.snapshot(), which)
      .await
      .map_err(failed)?;
    let mut picked: Vec<&Found> = all.iter().filter(|(_, item)| order.keeps(item)).collect();
    order.sort(&mut picked);
    let (shown, has_more) = page(&picked, limit, offset);
    Ok(Listed {
      items: shown.into_iter().cloned().collect(),
      total: Some(picked.len() as u32),
      has_more,
    })
  }

  async fn gathered(&self, query: Query<'_>, reach: u32) -> Result<index::Gathered, ProviderError> {
    gather(&self.registry.snapshot(), query, reach).await.map_err(failed)
  }

  async fn liked(&self, kind: ItemType) -> Vec<Found> {
    let found = match kind {
      ItemType::Album => self.indexed(Indexed::Albums, Order::Liked, 0, u32::MAX).await,
      ItemType::Artist => self.indexed(Indexed::Artists, Order::Liked, 0, u32::MAX).await,
      ItemType::Track => {
        let query = Query::Items {
          kind,
          sort: "lastRatedAt:desc",
          filter: Some(LIKED_FILTER),
        };
        self.gathered(query, LIKED_CAP).await.map(|gathered| {
          let mut items = gathered.items;
          items.sort_by_key(|(_, item)| Reverse(item.last_rated_at));
          Listed {
            items,
            total: None,
            has_more: false,
          }
        })
      }
    };
    match found {
      Ok(listed) => listed.items,
      Err(error) => {
        tracing::debug!(%error, "the plex likes did not load");
        Vec::new()
      }
    }
  }

  async fn playlists(&self) -> Result<Vec<Found>, ProviderError> {
    let servers = self.registry.snapshot();
    let lists = futures::future::join_all(servers.servers.iter().map(|server| server.playlists())).await;
    let mut all = Vec::new();
    for (server, playlists) in servers.servers.iter().zip(lists) {
      all.extend(
        playlists
          .map_err(failed)?
          .into_iter()
          .map(|item| (server.clone(), item)),
      );
    }
    Ok(dedupe(&servers, all))
  }

  async fn matching_playlists(&self, text: &str) -> Result<Vec<Found>, ProviderError> {
    let needle = text.to_lowercase();
    Ok(
      self
        .playlists()
        .await?
        .into_iter()
        .filter(|(_, playlist)| playlist.title.to_lowercase().contains(&needle))
        .collect(),
    )
  }

  async fn random(&self, kind: ItemType, limit: u32) -> Result<Vec<Found>, ProviderError> {
    let query = Query::Items {
      kind,
      sort: "random",
      filter: None,
    };
    let mut mixed = self.gathered(query, limit).await?.items;
    mixed.truncate(limit as usize);
    Ok(mixed)
  }

  async fn searched(&self, kind: ItemType, text: &str, offset: u32, limit: u32) -> Result<Listed, ProviderError> {
    let reach = offset.saturating_add(limit);
    let gathered = self.gathered(Query::Search { kind, text }, reach).await?;
    let (items, more) = page(&gathered.items, limit, offset);
    Ok(Listed {
      items,
      total: None,
      has_more: more || gathered.more,
    })
  }

  async fn popular(&self, server: &Arc<PlexServer>, artist: &str, limit: u32) -> Result<Vec<Found>, ProviderError> {
    let section = server
      .metadata(artist)
      .await
      .map_err(failed)?
      .library_section_id
      .ok_or_else(|| ProviderError::Failed(format!("the plex artist {artist} names no library")))?
      .to_string();
    let filter = [("artist.id", artist)];
    let page = server
      .items(&section, ItemType::Track, "ratingCount:desc", &filter, 0, limit)
      .await
      .map_err(failed)?;
    Ok(page.items.into_iter().map(|item| (server.clone(), item)).collect())
  }

  async fn artist_mix(&self, server: &Arc<PlexServer>, artist: &str) -> Result<Vec<Found>, ProviderError> {
    let seed = self.popular(server, artist, STATION_SEED_TRACKS).await?;
    let similar = server.similar(artist).await.unwrap_or_default();
    let neighbors = futures::future::join_all(
      similar
        .iter()
        .filter(|item| item.kind == "artist")
        .take(STATION_SIMILAR_ARTISTS)
        .map(|item| self.popular(server, &item.rating_key, STATION_SIMILAR_TRACKS)),
    )
    .await;
    let mut lists = vec![seed];
    lists.extend(neighbors.into_iter().filter_map(Result::ok));
    let mut mix = interleave(lists);
    mix.truncate(STATION_SIZE as usize);
    Ok(mix)
  }

  async fn station(&self, server: &Arc<PlexServer>, seed: Seed, key: &str) -> Result<Vec<Found>, ProviderError> {
    match seed {
      Seed::Artist => self.artist_mix(server, key).await,
      Seed::Track => {
        let track = server.metadata(key).await.map_err(failed)?;
        let near = server.nearest(key, STATION_SIZE).await.unwrap_or_default();
        let rest: Vec<Found> = match (near.is_empty(), track.grandparent_rating_key.as_deref()) {
          (false, _) => near.into_iter().map(|item| (server.clone(), item)).collect(),
          (true, Some(artist)) => self.artist_mix(server, artist).await?,
          (true, None) => Vec::new(),
        };
        let mut mix = vec![(server.clone(), track)];
        mix.extend(rest.into_iter().filter(|(_, item)| item.rating_key != key));
        Ok(mix)
      }
    }
  }

  async fn tracks_of(&self, at: &Ref) -> Result<(Vec<Found>, Option<PlaybackContext>), ProviderError> {
    let key = at.key.as_str();
    let context = |name: Option<String>| {
      Some(PlaybackContext {
        uri: uri(at.kind, &at.server, key),
        name,
      })
    };
    let titled = |item: &Metadata| context(none_if_empty(&item.title));
    let tag = |server: &Arc<PlexServer>, items: Vec<Metadata>| -> Vec<Found> {
      items.into_iter().map(|item| (server.clone(), item)).collect()
    };
    match at.kind {
      Kind::Liked => Ok((self.liked(ItemType::Track).await, context(Some(LIKED_TITLE.into())))),
      Kind::Track => {
        let server = self.server(&at.server)?;
        let track = server.metadata(key).await.map_err(failed)?;
        Ok((tag(&server, vec![track]), None))
      }
      Kind::Album => {
        let server = self.server(&at.server)?;
        let (album, tracks) = tokio::try_join!(server.metadata(key), server.children(key)).map_err(failed)?;
        Ok((tag(&server, tracks), titled(&album)))
      }
      Kind::Artist => {
        let server = self.server(&at.server)?;
        let (artist, tracks) = tokio::try_join!(server.metadata(key), server.all_tracks(key)).map_err(failed)?;
        Ok((tag(&server, tracks), titled(&artist)))
      }
      Kind::Playlist => {
        let server = self.server(&at.server)?;
        let (playlist, tracks) =
          tokio::try_join!(server.metadata(key), server.playlist_items(key, 0, PLAYLIST_CAP)).map_err(failed)?;
        Ok((tag(&server, tracks.items), titled(&playlist)))
      }
      Kind::Station(seed) => {
        let server = self.server(&at.server)?;
        Ok((self.station(&server, seed, key).await?, context(None)))
      }
    }
  }

  async fn node(&self, node_id: &str, limit: u32, offset: u32) -> Result<BrowseResult, ProviderError> {
    let listed = match node_id {
      PLAYLISTS_NODE_ID => {
        let all = self.playlists().await?;
        let (items, has_more) = page(&all, limit, offset);
        Listed {
          items,
          total: Some(all.len() as u32),
          has_more,
        }
      }
      ARTISTS_NODE_ID => self.indexed(Indexed::Artists, Order::Title, offset, limit).await?,
      ALBUMS_NODE_ID => self.indexed(Indexed::Albums, Order::Title, offset, limit).await?,
      RECENTS_NODE_ID => self.indexed(Indexed::Albums, Order::Viewed, offset, limit).await?,
      NEWEST_NODE_ID => self.indexed(Indexed::Albums, Order::Added, offset, limit).await?,
      RANDOM_NODE_ID => Listed {
        items: self.random(ItemType::Track, limit.max(1)).await?,
        total: None,
        has_more: false,
      },
      other => {
        let at = parse_uri(other).ok_or_else(|| ProviderError::Failed(format!("{other} is not a plex node")))?;
        self.children_of(&at, limit, offset).await?
      }
    };
    Ok(listing(self.items(&listed.items), listed.total, listed.has_more))
  }

  async fn children_of(&self, at: &Ref, limit: u32, offset: u32) -> Result<Listed, ProviderError> {
    let all = match at.kind {
      Kind::Track => return Err(ProviderError::Failed("a track has no children".into())),
      Kind::Playlist => {
        let server = self.server(&at.server)?;
        let tracks = server.playlist_items(&at.key, offset, limit).await.map_err(failed)?;
        let total = tracks.total;
        let items: Vec<Found> = tracks.items.into_iter().map(|item| (server.clone(), item)).collect();
        let has_more = offset + (items.len() as u32) < total;
        return Ok(Listed {
          items,
          total: Some(total),
          has_more,
        });
      }
      Kind::Artist => {
        let server = self.server(&at.server)?;
        let albums = server.children(&at.key).await.map_err(failed)?;
        albums.into_iter().map(|item| (server.clone(), item)).collect()
      }
      Kind::Album | Kind::Station(_) | Kind::Liked => self.tracks_of(at).await?.0,
    };
    let (items, has_more) = page(&all, limit, offset);
    Ok(Listed {
      items,
      total: Some(all.len() as u32),
      has_more,
    })
  }

  async fn liked_uris(&self) -> Vec<String> {
    let (tracks, albums, artists) = tokio::join!(
      self.liked(ItemType::Track),
      self.liked(ItemType::Album),
      self.liked(ItemType::Artist)
    );
    tracks
      .iter()
      .chain(&albums)
      .chain(&artists)
      .filter_map(|(server, item)| kind_of_item(item).map(|kind| uri(kind, &server.machine, &item.rating_key)))
      .collect()
  }

  async fn rate(&self, item: &ItemRef, liked: bool) -> Result<(), ProviderError> {
    let at = parse_uri(&item.uri).ok_or_else(|| ProviderError::Failed(format!("{} is not a plex uri", item.uri)))?;
    if !matches!(at.kind, Kind::Track | Kind::Album | Kind::Artist) {
      return Err(ProviderError::NotImplemented);
    }
    self.server(&at.server)?.rate(&at.key, liked).await.map_err(failed)?;
    let now = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map_or(0, |elapsed| elapsed.as_secs() as i64);
    self
      .indexes
      .rated(&at.server, &at.key, liked.then_some(LIKED_RATING), now)
      .await;
    self.playback.set_liked(&item.uri, liked);
    Ok(())
  }

  async fn seed_name(&self, at: &Ref) -> Result<(String, Option<String>), ProviderError> {
    let server = self.server(&at.server)?;
    let seed = server.metadata(&at.key).await.map_err(failed)?;
    let art = self.hero(&server, &seed);
    Ok((seed.title, art))
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
  Title,
  Added,
  Viewed,
  Plays,
  Liked,
}

impl Order {
  fn keeps(self, item: &Metadata) -> bool {
    match self {
      Order::Viewed => item.last_viewed_at.is_some(),
      Order::Liked => item.liked(),
      Order::Title | Order::Added | Order::Plays => true,
    }
  }

  fn sort(self, items: &mut [&Found]) {
    match self {
      Order::Title => {
        items.sort_by_cached_key(|(_, item)| item.title_sort.as_deref().unwrap_or(&item.title).to_lowercase())
      }
      Order::Added => items.sort_by_key(|(_, item)| Reverse(item.added_at)),
      Order::Viewed => items.sort_by_key(|(_, item)| Reverse(item.last_viewed_at)),
      Order::Plays => items.sort_by_key(|(_, item)| Reverse(item.view_count)),
      Order::Liked => items.sort_by_key(|(_, item)| Reverse(item.last_rated_at)),
    }
  }
}

pub struct PlexProvider {
  core: Arc<Core>,
}

impl PlexProvider {
  pub fn new(
    config: PlexConfig,
    secrets: Arc<dyn SecretStore>,
    backend: Arc<dyn StreamBackend>,
    http: Arc<dyn IoHttpTransport>,
    art: Arc<ArtCache>,
  ) -> Arc<Self> {
    let registry = Arc::new(Registry::default());
    let resolver = Arc::new(Art {
      registry: registry.clone(),
    });
    let timeline = Arc::new(Timeline {
      registry: registry.clone(),
      worker: Mutex::new(None),
    });
    Arc::new(Self {
      core: Arc::new(Core {
        account: PlexAccount::new(&config.tv_base, &config.client_id, HttpExecutor::new(http.clone())),
        secrets,
        registry,
        playback: StreamPlayback::new(
          backend,
          http.clone(),
          art.clone(),
          PROVIDER_NAME,
          resolver.clone(),
          Some(timeline),
        ),
        art,
        art_origin: HttpExecutor::new(http.clone()),
        http: HttpExecutor::new(http),
        resolver,
        indexes: Indexes::default(),
        edges: Mutex::new((DEFAULT_HERO_EDGE, DEFAULT_THUMB_EDGE)),
        link: Mutex::new(None),
        auth_observer: Mutex::new(None),
        auth_task: Mutex::new(None),
        authorized: Mutex::new(false),
      }),
    })
  }

  fn start_sign_in(&self) {
    let core = self.core.clone();
    let task = tokio::spawn(async move { core.sign_in().await });
    if let Some(previous) = self.core.auth_task.lock().unwrap().replace(task) {
      previous.abort();
    }
  }
}

impl Drop for PlexProvider {
  fn drop(&mut self) {
    if let Some(task) = self.core.auth_task.lock().unwrap().take() {
      task.abort();
    }
  }
}

#[async_trait::async_trait]
impl PlayerTransport for PlexProvider {
  async fn play(&self, request: PlayUri) -> Result<(), ProviderError> {
    self.core.require()?;
    let at =
      parse_uri(&request.uri).ok_or_else(|| ProviderError::Failed(format!("{} is not a plex uri", request.uri)))?;
    let container = request
      .context
      .as_ref()
      .and_then(|context| parse_uri(&context.context_uri))
      .filter(|container| at.kind == Kind::Track && container.kind != Kind::Track);
    let (tracks, context) = self.core.tracks_of(container.as_ref().unwrap_or(&at)).await?;
    let entries: Vec<QueueEntry> = tracks.iter().filter_map(|track| self.core.entry(track)).collect();
    if entries.is_empty() {
      return Err(ProviderError::Failed(format!("{} has nothing to play", request.uri)));
    }
    let start = match at.kind {
      Kind::Track => entries
        .iter()
        .position(|entry| entry.presentation.uri == request.uri)
        .unwrap_or(0),
      _ => 0,
    };
    self.core.playback.play(entries, start, context).await;
    Ok(())
  }

  async fn queue(&self, request: QueueUri) -> Result<(), ProviderError> {
    self.core.require()?;
    let at =
      parse_uri(&request.uri).ok_or_else(|| ProviderError::Failed(format!("{} is not a plex uri", request.uri)))?;
    let (tracks, _) = self.core.tracks_of(&at).await?;
    let entries: Vec<QueueEntry> = tracks
      .iter()
      .filter_map(|track| self.core.entry(track))
      .map(|entry| QueueEntry { queued: true, ..entry })
      .collect();
    if entries.is_empty() {
      return Err(ProviderError::Failed(format!("{} has nothing to queue", request.uri)));
    }
    self.core.playback.enqueue(entries, request.position).await;
    Ok(())
  }

  async fn pause(&self) -> Result<(), ProviderError> {
    self.core.playback.pause().await;
    Ok(())
  }

  async fn resume(&self) -> Result<(), ProviderError> {
    self.core.playback.resume().await;
    Ok(())
  }

  async fn skip_next(&self) -> Result<(), ProviderError> {
    self.core.playback.skip_next().await;
    Ok(())
  }

  async fn skip_prev(&self) -> Result<(), ProviderError> {
    self.core.playback.skip_prev().await;
    Ok(())
  }

  async fn skip_to_index(&self, index: u32) -> Result<(), ProviderError> {
    self.core.playback.skip_to_index(index).await
  }

  async fn seek_to(&self, position_ms: u32) -> Result<(), ProviderError> {
    self.core.playback.seek_to(position_ms).await
  }
}

#[async_trait::async_trait]
impl Provider for PlexProvider {
  fn name(&self) -> &str {
    PROVIDER_NAME
  }

  fn display_name(&self) -> &str {
    "Plex"
  }

  fn uri_schemes(&self) -> Vec<String> {
    vec![SCHEME.to_string()]
  }

  fn music_provider(&self) -> MusicProvider {
    MusicProvider::Plex
  }

  fn app_bundles(&self) -> Vec<String> {
    self.core.playback.app_bundles()
  }

  fn voice_resolver(&self) -> Option<Arc<dyn VoiceCatalogResolver>> {
    self
      .core
      .require()
      .is_ok()
      .then(|| CatalogResolver::new(self.core.clone()) as Arc<dyn VoiceCatalogResolver>)
  }

  fn set_auth_observer(&self, observer: Option<AuthObserver>) {
    *self.core.auth_observer.lock().unwrap() = observer;
  }

  async fn attach(&self, link: ProviderLink) -> Result<(), ProviderError> {
    if self.core.link.lock().unwrap().is_some() {
      self.detach().await;
    }
    self.core.playback.attach(link.clone()).await;
    *self.core.link.lock().unwrap() = Some(link);
    self.core.notify_auth(ProviderAuthState::Pending {
      user_code: None,
      verification_url: None,
      verification_url_complete: None,
    });
    self.start_sign_in();
    Ok(())
  }

  async fn detach(&self) {
    if let Some(task) = self.core.auth_task.lock().unwrap().take() {
      task.abort();
    }
    self.core.playback.detach().await;
    *self.core.link.lock().unwrap() = None;
    *self.core.authorized.lock().unwrap() = false;
  }

  async fn connectivity_changed(&self, online: bool) {
    if !online || self.core.require().is_err() {
      return;
    }
    let Some(token) = self.core.token() else { return };
    if let Err(error) = self.core.discover(&token).await {
      tracing::info!(%error, "plex servers did not re-resolve after a network change");
    }
  }

  async fn asset(&self, id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    let Some((source, max_edge)) = IMAGE_CODEC.parse(id) else {
      return Ok(None);
    };
    let scaled = self
      .core
      .art
      .scaled(&self.core.art_origin, &self.core.resolver.url(&source), max_edge)
      .await;
    Ok(scaled.map(|bytes| AssetBytes {
      bytes,
      mime: Some("image/jpeg".into()),
    }))
  }

  async fn lyrics(&self, _track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError> {
    self.core.require()?;
    let Some(Ref {
      kind: Kind::Track,
      server,
      key,
    }) = self
      .core
      .playback
      .current()
      .and_then(|(playing, _)| parse_uri(&playing.uri))
    else {
      return Ok(None);
    };
    let Some(lyric) = self.core.server(&server)?.lyrics(&key).await.map_err(failed)? else {
      return Ok(None);
    };
    if !lyric.timed {
      return Ok(Some(Lyrics {
        synced: None,
        plain: Some(lyric.text),
        source: PROVIDER_NAME.into(),
      }));
    }
    let lines = lrc::parse(&lyric.text);
    let plain: Vec<&str> = lines
      .iter()
      .map(|line| line.text.as_str())
      .filter(|text| !text.is_empty())
      .collect();
    Ok(Some(Lyrics {
      plain: Some(plain.join("\n")),
      synced: (!lines.is_empty()).then_some(lines),
      source: PROVIDER_NAME.into(),
    }))
  }

  async fn browse(&self, request: LibraryBrowseRequest) -> Result<BrowseResult, ProviderError> {
    self.core.require()?;
    match request.node_id.as_deref() {
      None | Some("") | Some("root") => Ok(
        staple_root(request.sections, request.preview, |node_id, count| {
          self.core.node(node_id, count, 0)
        })
        .await,
      ),
      Some(node_id) => self.core.node(node_id, request.limit, request.offset).await,
    }
  }

  async fn resolve_context(&self, context: &str) -> Result<ContextResolveReply, ProviderError> {
    self.core.require()?;
    let at = parse_uri(context).ok_or_else(|| ProviderError::Failed(format!("{context} is not a plex uri")))?;
    if at.kind == Kind::Liked {
      return Ok(ContextResolveReply {
        name: Some(LIKED_TITLE.into()),
        artwork_id: None,
        subtitle: None,
      });
    }
    let server = self.core.server(&at.server)?;
    let item = server.metadata(&at.key).await.map_err(failed)?;
    let subtitle = match at.kind {
      Kind::Album => item.parent_title.clone(),
      Kind::Track => item.artist_name().map(str::to_owned),
      Kind::Artist | Kind::Playlist | Kind::Station(_) | Kind::Liked => None,
    };
    Ok(ContextResolveReply {
      name: none_if_empty(&item.title),
      artwork_id: self.core.hero(&server, &item),
      subtitle,
    })
  }

  async fn search(&self, request: LibrarySearchRequest) -> Result<SearchResult, ProviderError> {
    self.core.require()?;
    let kinds = match &request.kinds {
      Some(kinds) if !kinds.is_empty() => kinds.clone(),
      _ => vec![ItemKind::Track, ItemKind::Album, ItemKind::Artist, ItemKind::Playlist],
    };
    let limit = request.limit as usize;
    let mut items = Vec::new();
    let mut present = Vec::new();
    let mut full = false;
    for kind in kinds {
      let found = match kind {
        ItemKind::Track | ItemKind::Album | ItemKind::Artist => {
          let kind = match kind {
            ItemKind::Track => ItemType::Track,
            ItemKind::Album => ItemType::Album,
            _ => ItemType::Artist,
          };
          let listed = self
            .core
            .searched(kind, &request.query, request.offset, request.limit)
            .await?;
          full |= listed.has_more;
          listed.items
        }
        ItemKind::Playlist => {
          let matching = self.core.matching_playlists(&request.query).await?;
          let (shown, more) = page(&matching, request.limit, request.offset);
          full |= more;
          shown
        }
        _ => continue,
      };
      let mapped = self.core.items(&found);
      if !mapped.is_empty() {
        present.push(kind);
        full |= mapped.len() >= limit;
      }
      items.extend(mapped);
    }
    Ok(SearchResult {
      items,
      kinds: present,
      total: None,
      has_more: full,
    })
  }

  async fn recommendations(
    &self,
    request: LibraryRecommendationsRequest,
  ) -> Result<RecommendationsResult, ProviderError> {
    self.core.require()?;
    for seed in &request.seeds {
      let Some(at) = parse_uri(&seed.uri) else { continue };
      if !matches!(at.kind, Kind::Artist | Kind::Album | Kind::Playlist) {
        continue;
      }
      let node = self.core.node(&seed.uri, request.limit, request.offset).await?;
      return Ok(RecommendationsResult {
        items: node
          .entries
          .into_iter()
          .filter_map(|entry| match entry {
            BrowseEntry::Item(item) => Some(item),
            BrowseEntry::Folder(_) => None,
          })
          .collect(),
        total: node.total,
        has_more: node.has_more,
      });
    }
    let tracks = self.core.random(ItemType::Track, request.limit.max(1)).await?;
    Ok(RecommendationsResult {
      items: self.core.items(&tracks),
      total: None,
      has_more: false,
    })
  }

  async fn favorites_list(&self, request: LibraryFavoritesListRequest) -> Result<FavoritesPage, ProviderError> {
    self.core.require()?;
    let (tracks, albums, artists) = tokio::join!(
      self.core.liked(ItemType::Track),
      self.core.liked(ItemType::Album),
      self.core.liked(ItemType::Artist)
    );
    let all: Vec<LibraryItem> = self.core.items(&[tracks, albums, artists].concat());
    let (items, has_more) = page(&all, request.limit, request.offset);
    Ok(FavoritesPage {
      items,
      total: Some(all.len() as u32),
      has_more,
    })
  }

  async fn favorites_contains(&self, request: LibraryFavoritesContainsRequest) -> Result<Vec<bool>, ProviderError> {
    self.core.require()?;
    let liked = self.core.liked_uris().await;
    Ok(request.uris.iter().map(|uri| liked.contains(uri)).collect())
  }

  async fn favorites_toggle(&self, item: ItemRef) -> Result<(), ProviderError> {
    self.core.require()?;
    let liked = self.core.liked_uris().await.contains(&item.uri);
    self.core.rate(&item, !liked).await
  }

  async fn favorites_set(&self, item: ItemRef, liked: bool) -> Result<(), ProviderError> {
    self.core.require()?;
    self.core.rate(&item, liked).await
  }

  async fn favorites_set_many(&self, entries: Vec<FavoritesSet>) -> Result<(), ProviderError> {
    self.core.require()?;
    for entry in entries {
      self.core.rate(&entry.item, entry.liked).await?;
    }
    Ok(())
  }

  async fn set_art_profile(&self, hero_px: u32, thumb_px: u32) {
    *self.core.edges.lock().unwrap() = (hero_px.max(1), thumb_px.max(1));
    self.core.playback.set_art_edges(hero_px, thumb_px);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const MACHINE: &str = "dc5ae35f423d59cb55846a4548033548cf2a665d";

  #[test]
  fn uris_round_trip_through_their_kind_and_server() {
    for kind in [
      Kind::Track,
      Kind::Album,
      Kind::Artist,
      Kind::Playlist,
      Kind::Station(Seed::Artist),
      Kind::Station(Seed::Track),
    ] {
      let made = uri(kind, MACHINE, "28038");
      assert_eq!(
        parse_uri(&made),
        Some(Ref {
          kind,
          server: MACHINE.into(),
          key: "28038".into(),
        }),
        "{made}"
      );
    }
    assert_eq!(
      parse_uri(&uri(Kind::Liked, "", LIKED_ID)).map(|at| at.kind),
      Some(Kind::Liked)
    );
    assert_eq!(parse_uri("plex:track:28038"), None);
    assert_eq!(parse_uri("plex:show:m:1"), None);
    assert_eq!(parse_uri("plex:station:m:album:1"), None);
    assert_eq!(parse_uri("plex:liked:albums"), None);
    assert_eq!(parse_uri("subsonic:track:m:1"), None);
  }

  #[test]
  fn art_sources_mint_short_asset_ids_that_parse_back() {
    let source = art_source(MACHINE, Some("/library/metadata/28037/thumb/1784011349")).unwrap();
    let id = IMAGE_CODEC.asset_id(&source, 248).unwrap();
    assert_eq!(
      id,
      format!("plex/img/248/a{MACHINE}:/library/metadata/28037/thumb/1784011349")
    );
    assert_eq!(IMAGE_CODEC.parse(&id), Some((source, 248)));
    assert_eq!(art_source(MACHINE, Some("")), None);
  }
}
