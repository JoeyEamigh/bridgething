mod client;

use std::sync::{Arc, Mutex};

use bridgething_io::{HttpExecutor, HttpTransport as IoHttpTransport};
use libbridgething::{
  Album as WireAlbum, Artist as WireArtist, BrowseEntry, BrowseFolder, BrowseResult, FavoritesPage, ItemKind, ItemRef,
  LibraryItem, LyricLine, Lyrics, MusicProvider, PlaybackContext, Playlist as WirePlaylist, RecommendationsResult,
  SearchResult, Track as WireTrack,
  gateway::{
    ContextResolveReply, FavoritesSet, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, PlayUri, QueueUri, TrackIdentity,
  },
};
use tokio::task::JoinHandle;

use self::client::{Album, AlbumList, Artist, Playlist, SearchCounts, Song, StarTarget};
pub use self::client::{SubsonicClient, SubsonicError, normalize_server_url};
use crate::{
  backend::{ImageScaler, StreamBackend},
  provider::{
    AssetBytes, AuthObserver, PlayerTransport, Provider, ProviderAuthState, ProviderError, ProviderLink,
    art::{ArtCache, ArtResolver, ImageAssetCodec},
    none_if_empty,
    playback::{Presentation, QueueEntry, StreamPlayback},
  },
};

pub const PROVIDER_NAME: &str = "subsonic";
pub const SCHEME: &str = "subsonic";
pub const KEY_SERVER_URL: &str = "subsonic.server_url";
pub const KEY_USERNAME: &str = "subsonic.username";
pub const KEY_PASSWORD: &str = "subsonic.password";

const COVER_SOURCE_PREFIX: &str = "cover:";
const IMAGE_CODEC: ImageAssetCodec = ImageAssetCodec {
  namespace: "subsonic/img/",
  short_form: Some(('c', COVER_SOURCE_PREFIX)),
};
const DEFAULT_HERO_EDGE: u32 = 248;
const DEFAULT_THUMB_EDGE: u32 = 96;
const DEFAULT_ROOT_PREVIEW: u32 = 8;
const ARTIST_PLAY_ALBUM_CAP: usize = 20;
const PLAYLISTS_NODE_ID: &str = "playlists";
const ALBUMS_NODE_ID: &str = "albums";
const ARTISTS_NODE_ID: &str = "artists";
const RECENTS_NODE_ID: &str = libbridgething::RECENTS_NODE_ID;
const NEWEST_NODE_ID: &str = "recently-added";
const RANDOM_NODE_ID: &str = "random";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubsonicConfig {
  pub server_url: String,
  pub username: String,
  pub password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
  Track,
  Album,
  Artist,
  Playlist,
}

impl Kind {
  fn tag(self) -> &'static str {
    match self {
      Kind::Track => "track",
      Kind::Album => "album",
      Kind::Artist => "artist",
      Kind::Playlist => "playlist",
    }
  }
}

fn uri(kind: Kind, id: &str) -> String {
  format!("{SCHEME}:{}:{id}", kind.tag())
}

fn parse_uri(uri: &str) -> Option<(Kind, &str)> {
  let rest = uri.strip_prefix(SCHEME)?.strip_prefix(':')?;
  let (kind, id) = rest.split_once(':')?;
  let kind = match kind {
    "track" => Kind::Track,
    "album" => Kind::Album,
    "artist" => Kind::Artist,
    "playlist" => Kind::Playlist,
    _ => return None,
  };
  (!id.is_empty()).then_some((kind, id))
}

fn cover_source(cover_art: Option<&str>) -> Option<String> {
  cover_art
    .filter(|id| !id.is_empty())
    .map(|id| format!("{COVER_SOURCE_PREFIX}{id}"))
}

fn failed(error: SubsonicError) -> ProviderError {
  match error {
    SubsonicError::Server { code: 40 | 41 | 50, .. } => ProviderError::NotAuthenticated,
    other => ProviderError::Failed(other.to_string()),
  }
}

struct Art {
  client: Arc<SubsonicClient>,
}

impl ArtResolver for Art {
  fn asset_id(&self, source: &str, max_edge: u32) -> Option<String> {
    IMAGE_CODEC.asset_id(source, max_edge)
  }

  fn url(&self, source: &str) -> String {
    match source.strip_prefix(COVER_SOURCE_PREFIX) {
      Some(id) => self.client.cover_art_url(id),
      None => source.to_owned(),
    }
  }
}

fn page<T: Clone>(items: &[T], limit: u32, offset: u32) -> (Vec<T>, bool) {
  let start = (offset as usize).min(items.len());
  let end = (start + limit as usize).min(items.len());
  (items[start..end].to_vec(), end < items.len())
}

struct Core {
  client: Arc<SubsonicClient>,
  art: Arc<ArtCache>,
  resolver: Arc<Art>,
  playback: StreamPlayback,
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
    Ok(())
  }

  async fn sign_in(self: Arc<Self>) {
    match self.client.ping().await {
      Ok(()) => {
        *self.authorized.lock().unwrap() = true;
        self.notify_auth(ProviderAuthState::Authenticated);
      }
      Err(error) => {
        tracing::warn!(%error, server = self.client.base(), "the subsonic sign-in failed");
        self.notify_auth(ProviderAuthState::Failed {
          reason: match error {
            SubsonicError::Server { code: 40 | 41, .. } => "the server rejected the username or password".into(),
            other => other.to_string(),
          },
        });
      }
    }
  }

  fn track_item(&self, song: &Song) -> LibraryItem {
    let (_, thumb) = self.edges();
    let artist = WireArtist {
      id: song
        .artist_id
        .as_deref()
        .map(|id| uri(Kind::Artist, id))
        .unwrap_or_default(),
      name: song.artist.clone().unwrap_or_default(),
      artwork_id: None,
    };
    LibraryItem::Track(WireTrack {
      id: uri(Kind::Track, &song.id),
      name: song.title.clone(),
      album: WireAlbum {
        id: song
          .album_id
          .as_deref()
          .map(|id| uri(Kind::Album, id))
          .unwrap_or_default(),
        name: song.album.clone().unwrap_or_default(),
        artwork_id: None,
      },
      artists: vec![artist.clone()],
      artist,
      duration_ms: song.duration.unwrap_or(0).saturating_mul(1000),
      image_id: cover_source(song.cover_art.as_deref())
        .and_then(|source| IMAGE_CODEC.asset_id(&source, thumb))
        .unwrap_or_default(),
      saved: song.starred.is_some(),
    })
  }

  fn album_item(&self, album: &Album) -> LibraryItem {
    let (hero, _) = self.edges();
    LibraryItem::Album(WireAlbum {
      id: uri(Kind::Album, &album.id),
      name: album.name.clone(),
      artwork_id: cover_source(album.cover_art.as_deref()).and_then(|source| IMAGE_CODEC.asset_id(&source, hero)),
    })
  }

  fn artist_item(&self, artist: &Artist) -> LibraryItem {
    let (hero, _) = self.edges();
    LibraryItem::Artist(WireArtist {
      id: uri(Kind::Artist, &artist.id),
      name: artist.name.clone(),
      artwork_id: cover_source(artist.cover_art.as_deref()).and_then(|source| IMAGE_CODEC.asset_id(&source, hero)),
    })
  }

  fn playlist_item(&self, playlist: &Playlist) -> LibraryItem {
    let (hero, _) = self.edges();
    LibraryItem::Playlist(WirePlaylist {
      uri: uri(Kind::Playlist, &playlist.id),
      name: playlist.name.clone(),
      owner_name: playlist.owner.clone(),
      track_count: playlist.song_count,
      artwork_id: cover_source(playlist.cover_art.as_deref()).and_then(|source| IMAGE_CODEC.asset_id(&source, hero)),
    })
  }

  fn entry(&self, song: &Song) -> QueueEntry {
    QueueEntry {
      url: self.client.stream_url(&song.id),
      presentation: Presentation {
        uri: uri(Kind::Track, &song.id),
        title: none_if_empty(&song.title),
        artist: song.artist.clone(),
        artist_uri: song.artist_id.as_deref().map(|id| uri(Kind::Artist, id)),
        album: song.album.clone(),
        album_uri: song.album_id.as_deref().map(|id| uri(Kind::Album, id)),
        artwork: cover_source(song.cover_art.as_deref()),
        duration_ms: song.duration.map(|seconds| seconds.saturating_mul(1000)),
        liked: Some(song.starred.is_some()),
        track_number: song.track.and_then(|number| u16::try_from(number).ok()),
      },
      queued: false,
    }
  }

  async fn songs_of(&self, kind: Kind, id: &str) -> Result<(Vec<Song>, Option<PlaybackContext>), ProviderError> {
    match kind {
      Kind::Track => Ok((vec![self.client.song(id).await.map_err(failed)?], None)),
      Kind::Album => {
        let album = self.client.album(id).await.map_err(failed)?;
        let context = PlaybackContext {
          uri: uri(Kind::Album, id),
          name: none_if_empty(&album.album.name),
        };
        Ok((album.songs, Some(context)))
      }
      Kind::Playlist => {
        let playlist = self.client.playlist(id).await.map_err(failed)?;
        let context = PlaybackContext {
          uri: uri(Kind::Playlist, id),
          name: none_if_empty(&playlist.playlist.name),
        };
        Ok((playlist.songs, Some(context)))
      }
      Kind::Artist => {
        let artist = self.client.artist(id).await.map_err(failed)?;
        let mut songs = Vec::new();
        for album in artist.albums.iter().take(ARTIST_PLAY_ALBUM_CAP) {
          songs.extend(self.client.album(&album.id).await.map_err(failed)?.songs);
        }
        let context = PlaybackContext {
          uri: uri(Kind::Artist, id),
          name: none_if_empty(&artist.artist.name),
        };
        Ok((songs, Some(context)))
      }
    }
  }

  async fn root_browse(&self, sections: Option<u32>, preview: Option<u32>) -> Result<BrowseResult, ProviderError> {
    let preview_count = preview.unwrap_or(DEFAULT_ROOT_PREVIEW);
    let staples: [(&str, &str); 6] = [
      (PLAYLISTS_NODE_ID, "Playlists"),
      (ALBUMS_NODE_ID, "Albums"),
      (ARTISTS_NODE_ID, "Artists"),
      (RECENTS_NODE_ID, "Recently played"),
      (NEWEST_NODE_ID, "Recently added"),
      (RANDOM_NODE_ID, "Random songs"),
    ];
    let mut folders = Vec::new();
    for (node_id, title) in staples {
      let (children, total) = if preview_count == 0 {
        (Vec::new(), None)
      } else {
        match self.node(node_id, preview_count, 0).await {
          Ok(page) => (page.entries, page.total),
          Err(error) => {
            tracing::debug!(%error, node_id, "a root preview did not load");
            (Vec::new(), None)
          }
        }
      };
      folders.push(BrowseFolder {
        node_id: node_id.into(),
        title: title.into(),
        subtitle: None,
        artwork_id: None,
        total,
        preview_children: (!children.is_empty()).then_some(children),
      });
    }
    if let Some(cap) = sections {
      folders.truncate(cap as usize);
    }
    Ok(BrowseResult {
      total: Some(folders.len() as u32),
      entries: folders.into_iter().map(BrowseEntry::Folder).collect(),
      has_more: false,
    })
  }

  fn listing(&self, items: Vec<LibraryItem>, total: Option<u32>, has_more: bool) -> BrowseResult {
    BrowseResult {
      entries: items.into_iter().map(BrowseEntry::Item).collect(),
      total,
      has_more,
    }
  }

  async fn node(&self, node_id: &str, limit: u32, offset: u32) -> Result<BrowseResult, ProviderError> {
    let client = &self.client;
    match node_id {
      PLAYLISTS_NODE_ID => {
        let all = client.playlists().await.map_err(failed)?;
        let (items, has_more) = page(&all, limit, offset);
        let items = items.iter().map(|playlist| self.playlist_item(playlist)).collect();
        Ok(self.listing(items, Some(all.len() as u32), has_more))
      }
      ARTISTS_NODE_ID => {
        let all = client.artists().await.map_err(failed)?;
        let (items, has_more) = page(&all, limit, offset);
        let items = items.iter().map(|artist| self.artist_item(artist)).collect();
        Ok(self.listing(items, Some(all.len() as u32), has_more))
      }
      ALBUMS_NODE_ID | RECENTS_NODE_ID | NEWEST_NODE_ID => {
        let list = match node_id {
          ALBUMS_NODE_ID => AlbumList::Alphabetical,
          RECENTS_NODE_ID => AlbumList::RecentlyPlayed,
          _ => AlbumList::Newest,
        };
        let albums = client.album_list(list, limit, offset).await.map_err(failed)?;
        let has_more = albums.len() as u32 >= limit && limit > 0;
        let items = albums.iter().map(|album| self.album_item(album)).collect();
        Ok(self.listing(items, None, has_more))
      }
      RANDOM_NODE_ID => {
        let songs = client.random_songs(limit.max(1)).await.map_err(failed)?;
        let items = songs.iter().map(|song| self.track_item(song)).collect();
        Ok(self.listing(items, None, false))
      }
      other => {
        let Some((kind, id)) = parse_uri(other) else {
          return Err(ProviderError::Failed(format!("{other} is not a subsonic node")));
        };
        let (items, total): (Vec<LibraryItem>, usize) = match kind {
          Kind::Album => {
            let album = client.album(id).await.map_err(failed)?;
            let (songs, _) = page(&album.songs, limit, offset);
            (
              songs.iter().map(|song| self.track_item(song)).collect(),
              album.songs.len(),
            )
          }
          Kind::Playlist => {
            let playlist = client.playlist(id).await.map_err(failed)?;
            let (songs, _) = page(&playlist.songs, limit, offset);
            (
              songs.iter().map(|song| self.track_item(song)).collect(),
              playlist.songs.len(),
            )
          }
          Kind::Artist => {
            let artist = client.artist(id).await.map_err(failed)?;
            let (albums, _) = page(&artist.albums, limit, offset);
            (
              albums.iter().map(|album| self.album_item(album)).collect(),
              artist.albums.len(),
            )
          }
          Kind::Track => return Err(ProviderError::Failed("a track has no children".into())),
        };
        let has_more = offset as usize + items.len() < total;
        Ok(self.listing(items, Some(total as u32), has_more))
      }
    }
  }

  async fn starred_uris(&self) -> Result<Vec<String>, ProviderError> {
    let hits = self.client.starred().await.map_err(failed)?;
    Ok(
      hits
        .songs
        .iter()
        .map(|song| uri(Kind::Track, &song.id))
        .chain(hits.albums.iter().map(|album| uri(Kind::Album, &album.id)))
        .chain(hits.artists.iter().map(|artist| uri(Kind::Artist, &artist.id)))
        .collect(),
    )
  }

  async fn star(&self, item: &ItemRef, starred: bool) -> Result<(), ProviderError> {
    let Some((kind, id)) = parse_uri(&item.uri) else {
      return Err(ProviderError::Failed(format!("{} is not a subsonic uri", item.uri)));
    };
    let target = match kind {
      Kind::Track => StarTarget::Song(id),
      Kind::Album => StarTarget::Album(id),
      Kind::Artist => StarTarget::Artist(id),
      Kind::Playlist => return Err(ProviderError::NotImplemented),
    };
    self.client.set_starred(target, starred).await.map_err(failed)?;
    self.playback.set_liked(&item.uri, starred);
    Ok(())
  }
}

pub struct SubsonicProvider {
  core: Arc<Core>,
}

impl SubsonicProvider {
  pub fn new(
    config: SubsonicConfig,
    backend: Arc<dyn StreamBackend>,
    http: Arc<dyn IoHttpTransport>,
    scaler: Option<Arc<dyn ImageScaler>>,
  ) -> Arc<Self> {
    let client = Arc::new(SubsonicClient::new(
      &config.server_url,
      &config.username,
      &config.password,
      HttpExecutor::new(http.clone()),
    ));
    let art = Arc::new(ArtCache::new(HttpExecutor::new(http.clone()), scaler));
    let resolver = Arc::new(Art { client: client.clone() });
    Arc::new(Self {
      core: Arc::new(Core {
        playback: StreamPlayback::new(backend, http, art.clone(), PROVIDER_NAME, resolver.clone()),
        client,
        art,
        resolver,
        edges: Mutex::new((DEFAULT_HERO_EDGE, DEFAULT_THUMB_EDGE)),
        link: Mutex::new(None),
        auth_observer: Mutex::new(None),
        auth_task: Mutex::new(None),
        authorized: Mutex::new(false),
      }),
    })
  }
}

impl Drop for SubsonicProvider {
  fn drop(&mut self) {
    if let Some(task) = self.core.auth_task.lock().unwrap().take() {
      task.abort();
    }
  }
}

#[async_trait::async_trait]
impl PlayerTransport for SubsonicProvider {
  async fn play(&self, request: PlayUri) -> Result<(), ProviderError> {
    self.core.require()?;
    let Some((kind, id)) = parse_uri(&request.uri) else {
      return Err(ProviderError::Failed(format!("{} is not a subsonic uri", request.uri)));
    };
    let (songs, context) = match request
      .context
      .as_ref()
      .and_then(|context| parse_uri(&context.context_uri))
    {
      Some((container, container_id)) if kind == Kind::Track && container != Kind::Track => {
        self.core.songs_of(container, container_id).await?
      }
      _ => self.core.songs_of(kind, id).await?,
    };
    let entries: Vec<QueueEntry> = songs.iter().map(|song| self.core.entry(song)).collect();
    let start = match kind {
      Kind::Track => entries
        .iter()
        .position(|entry| entry.presentation.uri == request.uri)
        .unwrap_or(0),
      _ => 0,
    };
    if entries.is_empty() {
      return Err(ProviderError::Failed(format!("{} has nothing to play", request.uri)));
    }
    self.core.playback.play(entries, start, context).await;
    Ok(())
  }

  async fn queue(&self, request: QueueUri) -> Result<(), ProviderError> {
    self.core.require()?;
    let Some((kind, id)) = parse_uri(&request.uri) else {
      return Err(ProviderError::Failed(format!("{} is not a subsonic uri", request.uri)));
    };
    let (songs, _) = self.core.songs_of(kind, id).await?;
    let entries: Vec<QueueEntry> = songs
      .iter()
      .map(|song| QueueEntry {
        queued: true,
        ..self.core.entry(song)
      })
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
impl Provider for SubsonicProvider {
  fn name(&self) -> &str {
    PROVIDER_NAME
  }

  fn display_name(&self) -> &str {
    "Subsonic"
  }

  fn uri_schemes(&self) -> Vec<String> {
    vec![SCHEME.to_string()]
  }

  fn music_provider(&self) -> MusicProvider {
    MusicProvider::Subsonic
  }

  fn app_bundles(&self) -> Vec<String> {
    self.core.playback.app_bundles()
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
    let core = self.core.clone();
    let task = tokio::spawn(async move { core.sign_in().await });
    if let Some(previous) = self.core.auth_task.lock().unwrap().replace(task) {
      previous.abort();
    }
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

  async fn asset(&self, id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    let Some((source, max_edge)) = IMAGE_CODEC.parse(id) else {
      return Ok(None);
    };
    let scaled = self.core.art.scaled(&self.core.resolver.url(&source), max_edge).await;
    Ok(scaled.map(|bytes| AssetBytes {
      bytes,
      mime: Some("image/jpeg".into()),
    }))
  }

  async fn lyrics(&self, track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError> {
    self.core.require()?;
    let current = self
      .core
      .playback
      .current_uri()
      .and_then(|uri| parse_uri(&uri).map(|(_, id)| id.to_owned()));
    if let Some(id) = current
      && let Ok(Some(structured)) = self.core.client.lyrics_by_song(&id).await
    {
      let plain: Vec<&str> = structured.lines.iter().map(|line| line.value.as_str()).collect();
      return Ok(Some(Lyrics {
        synced: structured.synced.then(|| {
          structured
            .lines
            .iter()
            .map(|line| LyricLine {
              start_ms: line.start.unwrap_or(0),
              text: line.value.clone(),
            })
            .collect()
        }),
        plain: Some(plain.join("\n")),
        source: PROVIDER_NAME.into(),
      }));
    }
    let plain = self
      .core
      .client
      .lyrics(&track.artist, &track.track)
      .await
      .map_err(failed)?;
    Ok(plain.map(|text| Lyrics {
      synced: None,
      plain: Some(text),
      source: PROVIDER_NAME.into(),
    }))
  }

  async fn browse(&self, request: LibraryBrowseRequest) -> Result<BrowseResult, ProviderError> {
    self.core.require()?;
    match request.node_id.as_deref() {
      None | Some("") | Some("root") => self.core.root_browse(request.sections, request.preview).await,
      Some(node_id) => self.core.node(node_id, request.limit, request.offset).await,
    }
  }

  async fn resolve_context(&self, context: &str) -> Result<ContextResolveReply, ProviderError> {
    self.core.require()?;
    let Some((kind, id)) = parse_uri(context) else {
      return Err(ProviderError::Failed(format!("{context} is not a subsonic uri")));
    };
    let (hero, _) = self.core.edges();
    let (name, cover, subtitle) = match kind {
      Kind::Album => {
        let album = self.core.client.album(id).await.map_err(failed)?;
        (album.album.name, album.album.cover_art, album.album.artist)
      }
      Kind::Playlist => {
        let playlist = self.core.client.playlist(id).await.map_err(failed)?;
        (
          playlist.playlist.name,
          playlist.playlist.cover_art,
          playlist.playlist.owner,
        )
      }
      Kind::Artist => {
        let artist = self.core.client.artist(id).await.map_err(failed)?;
        (artist.artist.name, artist.artist.cover_art, None)
      }
      Kind::Track => {
        let song = self.core.client.song(id).await.map_err(failed)?;
        (song.title, song.cover_art, song.artist)
      }
    };
    Ok(ContextResolveReply {
      name: none_if_empty(&name),
      artwork_id: cover_source(cover.as_deref()).and_then(|source| IMAGE_CODEC.asset_id(&source, hero)),
      subtitle,
    })
  }

  async fn search(&self, request: LibrarySearchRequest) -> Result<SearchResult, ProviderError> {
    self.core.require()?;
    let kinds = match &request.kinds {
      Some(kinds) if !kinds.is_empty() => kinds.clone(),
      _ => vec![ItemKind::Track, ItemKind::Album, ItemKind::Artist, ItemKind::Playlist],
    };
    let wants = |kind: ItemKind| kinds.contains(&kind);
    let counts = SearchCounts {
      artists: if wants(ItemKind::Artist) { request.limit } else { 0 },
      albums: if wants(ItemKind::Album) { request.limit } else { 0 },
      songs: if wants(ItemKind::Track) { request.limit } else { 0 },
    };
    let hits = self
      .core
      .client
      .search(&request.query, counts, request.offset)
      .await
      .map_err(failed)?;
    let needle = request.query.to_lowercase();
    let playlists: Vec<Playlist> = if wants(ItemKind::Playlist) {
      let all = self.core.client.playlists().await.map_err(failed)?;
      let matching: Vec<Playlist> = all
        .into_iter()
        .filter(|playlist| playlist.name.to_lowercase().contains(&needle))
        .collect();
      page(&matching, request.limit, request.offset).0
    } else {
      Vec::new()
    };
    let limit = request.limit as usize;
    let mut items = Vec::new();
    let mut present = Vec::new();
    let mut full = false;
    for kind in kinds {
      let mapped: Vec<LibraryItem> = match kind {
        ItemKind::Track => hits.songs.iter().map(|song| self.core.track_item(song)).collect(),
        ItemKind::Album => hits.albums.iter().map(|album| self.core.album_item(album)).collect(),
        ItemKind::Artist => hits
          .artists
          .iter()
          .map(|artist| self.core.artist_item(artist))
          .collect(),
        ItemKind::Playlist => playlists
          .iter()
          .map(|playlist| self.core.playlist_item(playlist))
          .collect(),
        _ => continue,
      };
      if !mapped.is_empty() {
        present.push(kind);
        if mapped.len() >= limit {
          full = true;
        }
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
      let Some((kind, id)) = parse_uri(&seed.uri) else {
        continue;
      };
      let node = match kind {
        Kind::Artist | Kind::Album | Kind::Playlist => {
          self.core.node(&uri(kind, id), request.limit, request.offset).await?
        }
        Kind::Track => continue,
      };
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
    let songs = self
      .core
      .client
      .random_songs(request.limit.max(1))
      .await
      .map_err(failed)?;
    Ok(RecommendationsResult {
      items: songs.iter().map(|song| self.core.track_item(song)).collect(),
      total: None,
      has_more: false,
    })
  }

  async fn favorites_list(&self, request: LibraryFavoritesListRequest) -> Result<FavoritesPage, ProviderError> {
    self.core.require()?;
    let hits = self.core.client.starred().await.map_err(failed)?;
    let all: Vec<LibraryItem> = hits
      .songs
      .iter()
      .map(|song| self.core.track_item(song))
      .chain(hits.albums.iter().map(|album| self.core.album_item(album)))
      .chain(hits.artists.iter().map(|artist| self.core.artist_item(artist)))
      .collect();
    let (items, has_more) = page(&all, request.limit, request.offset);
    Ok(FavoritesPage {
      items,
      total: Some(all.len() as u32),
      has_more,
    })
  }

  async fn favorites_contains(&self, request: LibraryFavoritesContainsRequest) -> Result<Vec<bool>, ProviderError> {
    self.core.require()?;
    let starred = self.core.starred_uris().await?;
    Ok(request.uris.iter().map(|uri| starred.contains(uri)).collect())
  }

  async fn favorites_toggle(&self, item: ItemRef) -> Result<(), ProviderError> {
    self.core.require()?;
    let starred = self.core.starred_uris().await?.contains(&item.uri);
    self.core.star(&item, !starred).await
  }

  async fn favorites_set(&self, item: ItemRef, liked: bool) -> Result<(), ProviderError> {
    self.core.require()?;
    self.core.star(&item, liked).await
  }

  async fn favorites_set_many(&self, entries: Vec<FavoritesSet>) -> Result<(), ProviderError> {
    self.core.require()?;
    for entry in entries {
      self.core.star(&entry.item, entry.liked).await?;
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

  #[test]
  fn uris_round_trip_through_their_kind() {
    for kind in [Kind::Track, Kind::Album, Kind::Artist, Kind::Playlist] {
      let made = uri(kind, "ab-12");
      assert_eq!(parse_uri(&made), Some((kind, "ab-12")));
    }
    assert_eq!(parse_uri("subsonic:show:1"), None);
    assert_eq!(parse_uri("subsonic:track:"), None);
    assert_eq!(parse_uri("spotify:track:1"), None);
  }

  #[test]
  fn cover_sources_mint_short_asset_ids_that_parse_back() {
    let source = cover_source(Some("al-9_abc")).unwrap();
    let id = IMAGE_CODEC.asset_id(&source, 248).unwrap();
    assert_eq!(id, "subsonic/img/248/cal-9_abc");
    assert_eq!(IMAGE_CODEC.parse(&id), Some((source, 248)));
    assert_eq!(cover_source(Some("")), None);
    assert_eq!(cover_source(None), None);
  }

  #[test]
  fn paging_reports_whether_more_follows() {
    let items = [1, 2, 3, 4, 5];
    assert_eq!(page(&items, 2, 0), (vec![1, 2], true));
    assert_eq!(page(&items, 2, 4), (vec![5], false));
    assert_eq!(page(&items, 2, 9), (vec![], false));
  }
}
