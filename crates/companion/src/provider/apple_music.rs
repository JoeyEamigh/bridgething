use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_io::{HttpExecutor, HttpTransport as IoHttpTransport};
use libbridgething::{
  Album as WireAlbum, Artist as WireArtist, BrowseEntry, BrowseFolder, BrowseResult, FavoritesPage, ItemKind, ItemRef,
  LibraryItem, Lyrics, MediaItem, MediaItemUpdate, MusicProvider, NowPlayingUpdate, Playback, PlaybackState,
  PlaybackUpdate, PlayerOptions, PlayerState, Playlist, QueueItem, RecommendationsResult, RepeatMode, SearchResult,
  ShuffleMode, Station, Track as WireTrack,
  gateway::{
    ContextResolveReply, FavoritesSet, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, PlayUri, QueueUri, TrackIdentity,
  },
};
use tokio::task::JoinHandle;

use crate::{
  backend::{
    AmActionSink, AmAuthSink, AmAuthStatus, AmCatalogSink, AmEntry, AmFavoritesSink, AmFlagSink, AmItem, AmItemSink,
    AmKind, AmLibraryScope, AmPage, AmPageSink, AmPlayerCommand, AmPlayerInbox, AmPlayerSnapshot, AmRepeatMode,
    AmSearchSink, AmShelvesSink, AmSnapshotSink, AppleMusicBackend, ImageScaler,
  },
  dispatch::tell,
  provider::{
    AssetBytes, AuthObserver, NowPlayingObserver, PlayerTransport, Provider, ProviderAuthState, ProviderError,
    ProviderLink, ProviderNowPlaying,
    art::{ArtCache, ImageAssetCodec},
    none_if_empty,
  },
};

pub const PROVIDER_NAME: &str = "apple-music";
pub(crate) const KEY_CONNECTED: &str = "apple_music.connected";
const APPLE_MUSIC_APP_BUNDLE: &str = "com.apple.Music";
const REC_NODE_PREFIX: &str = "rec:";
const PLAYLISTS_NODE_ID: &str = "playlists";
const ALBUMS_NODE_ID: &str = "albums";
const ARTISTS_NODE_ID: &str = "artists";
const SONGS_NODE_ID: &str = "songs";
const RECENTS_NODE_ID: &str = "recently-played";
const DEFAULT_HERO_EDGE: u32 = 248;
const DEFAULT_THUMB_EDGE: u32 = 96;
const DEFAULT_ROOT_PREVIEW: u32 = 8;
const BACKEND_DEADLINE: Duration = Duration::from_secs(10);

const IMAGE_CODEC: ImageAssetCodec = ImageAssetCodec {
  namespace: "applemusic/img/",
  short_form: None,
};

async fn reply<T>(rx: tokio::sync::oneshot::Receiver<Result<T, String>>) -> Result<T, ProviderError> {
  match tokio::time::timeout(BACKEND_DEADLINE, rx).await {
    Ok(Ok(Ok(value))) => Ok(value),
    Ok(Ok(Err(reason))) => Err(ProviderError::Failed(reason)),
    Ok(Err(_)) => Err(ProviderError::Failed("the backend dropped the call".into())),
    Err(_) => Err(ProviderError::Failed("the backend did not answer the call".into())),
  }
}

fn sized_artwork_url(template: &str, edge: u32) -> String {
  template
    .replace("{w}", &edge.to_string())
    .replace("{h}", &edge.to_string())
}

#[derive(Default)]
struct Shared {
  authorized: bool,
  last_snapshot: Option<AmPlayerSnapshot>,
  liked_cache: HashMap<String, bool>,
  liked_fetch_uri: Option<String>,
  hero_edge: u32,
  thumb_edge: u32,
}

struct Core {
  backend: Arc<dyn AppleMusicBackend>,
  art_cache: ArtCache,
  link: Mutex<Option<ProviderLink>>,
  shared: Mutex<Shared>,
  np_observer: Mutex<Option<NowPlayingObserver>>,
  auth_observer: Mutex<Option<AuthObserver>>,
  auth_task: Mutex<Option<JoinHandle<()>>>,
  observe_task: Mutex<Option<JoinHandle<()>>>,
}

pub struct AppleMusicProvider {
  core: Arc<Core>,
}

impl Drop for AppleMusicProvider {
  fn drop(&mut self) {
    for slot in [&self.core.auth_task, &self.core.observe_task] {
      if let Some(task) = slot.lock().unwrap().take() {
        task.abort();
      }
    }
  }
}

impl AppleMusicProvider {
  pub fn new(
    backend: Arc<dyn AppleMusicBackend>,
    http: Arc<dyn IoHttpTransport>,
    scaler: Option<Arc<dyn ImageScaler>>,
  ) -> Arc<Self> {
    Arc::new(Self {
      core: Arc::new(Core {
        backend,
        art_cache: ArtCache::new(HttpExecutor::new(http), scaler),
        link: Mutex::new(None),
        shared: Mutex::new(Shared {
          hero_edge: DEFAULT_HERO_EDGE,
          thumb_edge: DEFAULT_THUMB_EDGE,
          ..Shared::default()
        }),
        np_observer: Mutex::new(None),
        auth_observer: Mutex::new(None),
        auth_task: Mutex::new(None),
        observe_task: Mutex::new(None),
      }),
    })
  }
}

impl Core {
  fn start_observation(self: &Arc<Self>) {
    let core = self.clone();
    let (inbox, mut rx) = AmPlayerInbox::channel();
    let backend = core.backend.clone();
    let task = tokio::spawn(async move {
      tell(&backend, move |backend| backend.start(inbox)).await;
      if core.snapshot().await.is_some_and(|snap| snap.entry.is_some()) {
        core.emit_current().await;
      }
      while rx.recv().await.is_some() {
        core.emit_current().await;
      }
    });
    if let Some(previous) = self.observe_task.lock().unwrap().replace(task) {
      previous.abort();
    }
  }

  fn link(&self) -> Option<ProviderLink> {
    self.link.lock().unwrap().clone()
  }

  fn notify_auth(&self, state: ProviderAuthState) {
    if let Some(observer) = self.auth_observer.lock().unwrap().clone() {
      observer(state);
    }
  }

  fn notify_now_playing(&self, playing: Option<ProviderNowPlaying>) {
    if let Some(observer) = self.np_observer.lock().unwrap().clone() {
      observer(playing);
    }
  }

  fn art_edges(&self) -> (u32, u32) {
    let shared = self.shared.lock().unwrap();
    (shared.hero_edge, shared.thumb_edge)
  }

  async fn snapshot(&self) -> Option<AmPlayerSnapshot> {
    let (sink, rx) = AmSnapshotSink::channel();
    tell(&self.backend, move |backend| backend.snapshot(sink)).await;
    reply(rx).await.ok()
  }

  async fn emit_current(self: &Arc<Self>) {
    if !self.shared.lock().unwrap().authorized || self.link().is_none() {
      return;
    }
    let Some(snap) = self.snapshot().await else { return };
    self.shared.lock().unwrap().last_snapshot = Some(snap.clone());
    let (hero, _) = self.art_edges();
    let liked = self.like_for(snap.entry.as_ref());
    self.notify_now_playing(Some(ProviderNowPlaying {
      update: make_update(&snap, hero, liked),
      artwork_url: snap
        .entry
        .as_ref()
        .and_then(|entry| entry.artwork_url.as_ref())
        .map(|template| sized_artwork_url(template, hero)),
    }));
    let has_item = snap.entry.is_some();
    if let Some(link) = self.link() {
      link.sink.submit_player(
        PROVIDER_NAME,
        make_snapshot(&snap, hero, liked),
        APPLE_MUSIC_APP_BUNDLE,
        has_item,
        false,
      );
    }
    self.refresh_liked_if_needed(snap.entry.as_ref());
  }

  fn like_for(&self, entry: Option<&AmEntry>) -> Option<bool> {
    let uri = entry?.uri.as_ref()?;
    self.shared.lock().unwrap().liked_cache.get(uri).copied()
  }

  fn refresh_liked_if_needed(self: &Arc<Self>, entry: Option<&AmEntry>) {
    let Some(uri) = entry.and_then(|entry| entry.uri.clone()) else {
      return;
    };
    let needs_fetch = {
      let mut shared = self.shared.lock().unwrap();
      if shared.liked_cache.contains_key(&uri) || shared.liked_fetch_uri.as_ref() == Some(&uri) {
        false
      } else {
        shared.liked_fetch_uri = Some(uri.clone());
        true
      }
    };
    if !needs_fetch {
      return;
    }
    let core = self.clone();
    tokio::spawn(async move {
      let (sink, rx) = AmFavoritesSink::channel();
      let asked = uri.clone();
      tell(&core.backend, move |backend| backend.is_favorite(vec![asked], sink)).await;
      let Ok(favorites) = reply(rx).await else { return };
      let Some(liked) = favorites.first().copied() else {
        return;
      };
      let still_current = {
        let mut shared = core.shared.lock().unwrap();
        shared.liked_cache.insert(uri.clone(), liked);
        shared
          .last_snapshot
          .as_ref()
          .and_then(|snap| snap.entry.as_ref())
          .and_then(|entry| entry.uri.as_ref())
          == Some(&uri)
      };
      if still_current {
        core.emit_current().await;
      }
    });
  }

  async fn apply_liked_change(self: &Arc<Self>, uri: &str, liked: bool) {
    let is_current = {
      let mut shared = self.shared.lock().unwrap();
      shared.liked_cache.insert(uri.to_owned(), liked);
      shared
        .last_snapshot
        .as_ref()
        .and_then(|snap| snap.entry.as_ref())
        .and_then(|entry| entry.uri.as_deref())
        == Some(uri)
    };
    if is_current {
      self.emit_current().await;
    }
  }

  async fn run_auth(self: Arc<Self>) {
    let mut status = {
      let (sink, rx) = AmAuthSink::channel();
      tell(&self.backend, move |backend| backend.auth_status(sink)).await;
      reply(rx).await.ok()
    };
    if status == Some(AmAuthStatus::NotDetermined) {
      let (sink, rx) = AmAuthSink::channel();
      tell(&self.backend, move |backend| backend.request_authorization(sink)).await;
      status = reply(rx).await.ok();
    }
    if status != Some(AmAuthStatus::Authorized) {
      tracing::warn!(?status, "media library authorization refused");
      self.notify_auth(ProviderAuthState::Failed {
        reason: "Apple Music access is not allowed. Enable it in Settings > Privacy > Media & Apple Music.".into(),
      });
      return;
    }
    let can_play = {
      let (sink, rx) = AmCatalogSink::channel();
      tell(&self.backend, move |backend| backend.can_play_catalog_content(sink)).await;
      reply(rx).await.ok().flatten()
    };
    if can_play == Some(false) {
      self.notify_auth(ProviderAuthState::Failed {
        reason: "An Apple Music subscription is required".into(),
      });
      return;
    }
    self.shared.lock().unwrap().authorized = true;
    self.notify_auth(ProviderAuthState::Authenticated);
    self.start_observation();
  }

  async fn action(
    &self,
    run: impl FnOnce(&dyn AppleMusicBackend, Arc<AmActionSink>) + Send + 'static,
  ) -> Result<(), ProviderError> {
    let (sink, rx) = AmActionSink::channel();
    let backend = self.backend.clone();
    tokio::task::spawn_blocking(move || run(backend.as_ref(), sink))
      .await
      .map_err(|error| ProviderError::Failed(error.to_string()))?;
    reply(rx).await
  }

  async fn command(&self, cmd: AmPlayerCommand) -> Result<(), ProviderError> {
    self.action(move |backend, sink| backend.command(cmd, sink)).await
  }

  async fn page(&self, scope: AmLibraryScope, limit: u32, offset: u32) -> Result<AmPage, ProviderError> {
    let (sink, rx) = AmPageSink::channel();
    tell(&self.backend, move |backend| {
      backend.library(scope, limit, offset, sink)
    })
    .await;
    reply(rx).await
  }

  async fn shelves(&self) -> Result<Vec<crate::backend::AmShelf>, ProviderError> {
    let (sink, rx) = AmShelvesSink::channel();
    tell(&self.backend, move |backend| backend.recommendations(sink)).await;
    reply(rx).await
  }

  fn art_asset_id(&self, template: Option<&str>, edge: u32) -> Option<String> {
    let template = template?;
    if template.is_empty() {
      return None;
    }
    IMAGE_CODEC.asset_id(&sized_artwork_url(template, edge), edge)
  }

  fn library_item(&self, item: &AmItem) -> Option<LibraryItem> {
    let (hero, thumb) = self.art_edges();
    let art = self.art_asset_id(item.artwork_url.as_deref(), hero);
    match item.kind {
      AmKind::Song => Some(LibraryItem::Track(WireTrack {
        id: item.uri.clone(),
        name: item.title.clone(),
        album: WireAlbum {
          id: item.album_uri.clone().unwrap_or_default(),
          name: item.album_name.clone().unwrap_or_default(),
          artwork_id: None,
        },
        artist: WireArtist {
          id: item.artist_uri.clone().unwrap_or_default(),
          name: item.artist_name.clone().unwrap_or_default(),
          artwork_id: None,
        },
        artists: item
          .artist_name
          .as_ref()
          .map(|name| {
            vec![WireArtist {
              id: item.artist_uri.clone().unwrap_or_default(),
              name: name.clone(),
              artwork_id: None,
            }]
          })
          .unwrap_or_default(),
        duration_ms: item.duration_ms.unwrap_or(0),
        image_id: self
          .art_asset_id(item.artwork_url.as_deref(), thumb)
          .unwrap_or_default(),
        saved: false,
      })),
      AmKind::Album => Some(LibraryItem::Album(WireAlbum {
        id: item.uri.clone(),
        name: item.title.clone(),
        artwork_id: art,
      })),
      AmKind::Artist => Some(LibraryItem::Artist(WireArtist {
        id: item.uri.clone(),
        name: item.title.clone(),
        artwork_id: art,
      })),
      AmKind::Playlist => Some(LibraryItem::Playlist(Playlist {
        uri: item.uri.clone(),
        name: item.title.clone(),
        owner_name: item.subtitle.clone(),
        track_count: item.track_count,
        artwork_id: art,
      })),
      AmKind::Station => Some(LibraryItem::Station(Station {
        uri: item.uri.clone(),
        name: item.title.clone(),
        seed: None,
        artwork_id: art,
      })),
    }
  }

  fn page_result(&self, page: AmPage) -> BrowseResult {
    BrowseResult {
      entries: page
        .items
        .iter()
        .filter_map(|item| self.library_item(item).map(BrowseEntry::Item))
        .collect(),
      total: page.total,
      has_more: page.has_more,
    }
  }

  async fn root_browse(&self, sections: Option<u32>, preview: Option<u32>) -> Result<BrowseResult, ProviderError> {
    let preview_count = preview.unwrap_or(DEFAULT_ROOT_PREVIEW);
    let mut folders: Vec<BrowseFolder> = Vec::new();
    let staples: [(&str, &str, AmLibraryScope); 4] = [
      (PLAYLISTS_NODE_ID, "Playlists", AmLibraryScope::Playlists),
      (ALBUMS_NODE_ID, "Albums", AmLibraryScope::Albums),
      (ARTISTS_NODE_ID, "Artists", AmLibraryScope::Artists),
      (SONGS_NODE_ID, "Songs", AmLibraryScope::Songs),
    ];
    for (node_id, title, scope) in staples {
      if preview_count == 0 {
        folders.push(BrowseFolder {
          node_id: node_id.into(),
          title: title.into(),
          subtitle: None,
          artwork_id: None,
          total: None,
          preview_children: None,
        });
        continue;
      }
      let page = self.page(scope, preview_count, 0).await.unwrap_or(AmPage {
        items: Vec::new(),
        total: None,
        has_more: false,
      });
      let children: Vec<BrowseEntry> = page
        .items
        .iter()
        .filter_map(|item| self.library_item(item).map(BrowseEntry::Item))
        .collect();
      folders.push(BrowseFolder {
        node_id: node_id.into(),
        title: title.into(),
        subtitle: None,
        artwork_id: None,
        total: page.total,
        preview_children: (!children.is_empty()).then_some(children),
      });
    }
    if let Ok(rails) = self.shelves().await {
      for rail in rails {
        let children: Vec<BrowseEntry> = if preview_count == 0 {
          Vec::new()
        } else {
          rail
            .items
            .iter()
            .take(preview_count as usize)
            .filter_map(|item| self.library_item(item).map(BrowseEntry::Item))
            .collect()
        };
        folders.push(BrowseFolder {
          node_id: format!("{REC_NODE_PREFIX}{}", rail.id),
          title: rail.title.clone(),
          subtitle: None,
          artwork_id: None,
          total: rail.total.or(Some(rail.items.len() as u32)),
          preview_children: (!children.is_empty()).then_some(children),
        });
      }
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
}

fn map_repeat(mode: AmRepeatMode) -> RepeatMode {
  match mode {
    AmRepeatMode::Off => RepeatMode::Off,
    AmRepeatMode::All => RepeatMode::All,
    AmRepeatMode::One => RepeatMode::One,
  }
}

fn am_repeat(mode: RepeatMode) -> AmRepeatMode {
  match mode {
    RepeatMode::Off => AmRepeatMode::Off,
    RepeatMode::All => AmRepeatMode::All,
    RepeatMode::One => AmRepeatMode::One,
  }
}

fn make_snapshot(snap: &AmPlayerSnapshot, hero_edge: u32, liked: Option<bool>) -> PlayerState {
  let art = |entry: &AmEntry| {
    entry
      .artwork_url
      .as_deref()
      .filter(|template| !template.is_empty())
      .and_then(|template| IMAGE_CODEC.asset_id(&sized_artwork_url(template, hero_edge), hero_edge))
  };
  let track = snap.entry.as_ref().map(|entry| MediaItem {
    uri: Some(entry.uri.clone().unwrap_or_default()),
    persistent_id: entry.uri.clone(),
    title: none_if_empty(&entry.title),
    album: entry.album_name.clone(),
    album_uri: None,
    album_artist: None,
    artist: entry.artist_name.clone(),
    artist_uri: None,
    liked,
    artwork_id: art(entry),
    duration_ms: entry.duration_ms,
    media_types: None,
    track_number: None,
    track_count: None,
    is_like_supported: entry.uri.is_some().then_some(true),
    is_ban_supported: None,
    is_banned: None,
    chapter_count: None,
  });
  PlayerState {
    track,
    playback: Playback {
      state: if snap.playing {
        PlaybackState::Playing
      } else {
        PlaybackState::Paused
      },
      position_ms: snap.position_ms,
      position_age_ms: None,
      shuffle: snap.shuffle,
      shuffle_mode: Some(if snap.shuffle {
        ShuffleMode::Songs
      } else {
        ShuffleMode::Off
      }),
      repeat: map_repeat(snap.repeat),
      queue_index: None,
      queue_count: None,
      queue_chapter_index: None,
      set_elapsed_time_available: Some(snap.can_seek),
      queue_list_avail: None,
      apple_music_radio_ad: None,
    },
    queue: Vec::<QueueItem>::new(),
    options: PlayerOptions {
      speed: 1.0,
      crossfade_ms: None,
    },
    context: None,
    target: None,
  }
}

fn make_update(snap: &AmPlayerSnapshot, hero_edge: u32, liked: Option<bool>) -> NowPlayingUpdate {
  let media = snap.entry.as_ref().map(|entry| MediaItemUpdate {
    persistent_id: entry.uri.clone(),
    title: none_if_empty(&entry.title),
    album: entry.album_name.clone(),
    album_uri: None,
    album_artist: None,
    artist: entry.artist_name.clone(),
    artist_uri: None,
    liked,
    artwork_id: entry
      .artwork_url
      .as_deref()
      .filter(|template| !template.is_empty())
      .and_then(|template| IMAGE_CODEC.asset_id(&sized_artwork_url(template, hero_edge), hero_edge)),
    duration_ms: entry.duration_ms,
    media_types: None,
    track_number: None,
    track_count: None,
    is_like_supported: entry.uri.is_some().then_some(true),
    is_ban_supported: None,
    is_banned: None,
    is_resident_on_device: None,
    chapter_count: None,
  });
  NowPlayingUpdate {
    media_item: media,
    playback: Some(PlaybackUpdate {
      playing: Some(snap.playing),
      position_ms: Some(snap.position_ms),
      shuffle: Some(snap.shuffle),
      shuffle_mode: Some(if snap.shuffle {
        ShuffleMode::Songs
      } else {
        ShuffleMode::Off
      }),
      repeat: Some(map_repeat(snap.repeat)),
      app_bundle: Some(APPLE_MUSIC_APP_BUNDLE.into()),
      app_display_name: Some("Apple Music".into()),
      queue_index: None,
      queue_count: None,
      queue_chapter_index: None,
      playback_speed: None,
      set_elapsed_time_available: Some(snap.can_seek),
      queue_list_avail: None,
      apple_music_radio_ad: None,
      apple_music_radio_station_name: None,
    }),
  }
}

#[async_trait::async_trait]
impl PlayerTransport for AppleMusicProvider {
  async fn play(&self, uri: PlayUri) -> Result<(), ProviderError> {
    match uri.context {
      Some(context) => {
        self
          .core
          .action(move |backend, sink| backend.play_context(context.context_uri, Some(uri.uri), sink))
          .await
      }
      None => {
        self
          .core
          .action(move |backend, sink| backend.play_context(uri.uri, None, sink))
          .await
      }
    }
  }

  async fn queue(&self, req: QueueUri) -> Result<(), ProviderError> {
    let next = match req.position {
      libbridgething::QueuePosition::Append => false,
      libbridgething::QueuePosition::Next => true,
      libbridgething::QueuePosition::Index(_) => return Err(ProviderError::NotImplemented),
    };
    self
      .core
      .action(move |backend, sink| backend.queue_insert(req.uri, next, sink))
      .await
  }

  async fn pause(&self) -> Result<(), ProviderError> {
    self.core.command(AmPlayerCommand::Pause).await
  }

  async fn resume(&self) -> Result<(), ProviderError> {
    self.core.command(AmPlayerCommand::Play).await
  }

  async fn skip_next(&self) -> Result<(), ProviderError> {
    self.core.command(AmPlayerCommand::SkipNext).await
  }

  async fn skip_prev(&self) -> Result<(), ProviderError> {
    self.core.command(AmPlayerCommand::SkipPrev).await
  }

  async fn seek_to(&self, position_ms: u32) -> Result<(), ProviderError> {
    self.core.command(AmPlayerCommand::SeekTo { position_ms }).await
  }

  async fn set_shuffle(&self, on: bool) -> Result<(), ProviderError> {
    self.core.command(AmPlayerCommand::SetShuffle { on }).await
  }

  async fn set_repeat(&self, mode: RepeatMode) -> Result<(), ProviderError> {
    self
      .core
      .command(AmPlayerCommand::SetRepeat { mode: am_repeat(mode) })
      .await
  }
}

#[async_trait::async_trait]
impl Provider for AppleMusicProvider {
  fn name(&self) -> &str {
    PROVIDER_NAME
  }

  fn display_name(&self) -> &str {
    "Apple Music"
  }

  fn uri_schemes(&self) -> Vec<String> {
    vec!["applemusic".into()]
  }

  fn music_provider(&self) -> MusicProvider {
    MusicProvider::AppleMusic
  }

  fn app_bundles(&self) -> Vec<String> {
    vec![APPLE_MUSIC_APP_BUNDLE.into()]
  }

  fn set_now_playing_observer(&self, observer: Option<Arc<dyn Fn(Option<ProviderNowPlaying>) + Send + Sync>>) {
    *self.core.np_observer.lock().unwrap() = observer;
  }

  fn set_auth_observer(&self, observer: Option<Arc<dyn Fn(ProviderAuthState) + Send + Sync>>) {
    *self.core.auth_observer.lock().unwrap() = observer;
  }

  async fn attach(&self, link: ProviderLink) -> Result<(), ProviderError> {
    if self.core.link.lock().unwrap().is_some() {
      self.detach().await;
    }
    *self.core.link.lock().unwrap() = Some(link);
    self.core.notify_auth(ProviderAuthState::Pending {
      user_code: None,
      verification_url: None,
      verification_url_complete: None,
    });
    let core = self.core.clone();
    let task = tokio::spawn(async move { core.run_auth().await });
    if let Some(previous) = self.core.auth_task.lock().unwrap().replace(task) {
      previous.abort();
    }
    Ok(())
  }

  async fn detach(&self) {
    for slot in [&self.core.auth_task, &self.core.observe_task] {
      if let Some(task) = slot.lock().unwrap().take() {
        task.abort();
      }
    }
    tell(&self.core.backend, |backend| backend.stop()).await;
    let link = self.core.link.lock().unwrap().take();
    self.core.notify_now_playing(None);
    if let Some(link) = link {
      link.sink.clear_source(PROVIDER_NAME);
    }
    let mut shared = self.core.shared.lock().unwrap();
    shared.authorized = false;
    shared.last_snapshot = None;
    shared.liked_cache.clear();
    shared.liked_fetch_uri = None;
  }

  async fn resumed(&self) {
    if self.core.link().is_none() {
      return;
    }
    let authorized = self.core.shared.lock().unwrap().authorized;
    if authorized {
      self.core.start_observation();
    }
  }

  async fn handle_peer_connected(&self, allow_auto_resume: bool) {
    if self.core.link().is_none() {
      return;
    }
    let authorized = self.core.shared.lock().unwrap().authorized;
    if authorized {
      self.core.start_observation();
    }
    let has_entry = {
      let shared = self.core.shared.lock().unwrap();
      shared.authorized && shared.last_snapshot.as_ref().is_some_and(|snap| snap.entry.is_some())
    };
    if has_entry {
      self.core.emit_current().await;
    }
    if !allow_auto_resume || !authorized {
      return;
    }
    if self
      .core
      .shared
      .lock()
      .unwrap()
      .last_snapshot
      .as_ref()
      .is_some_and(|snap| snap.playing)
    {
      return;
    }
    let other_audio = {
      let (sink, rx) = AmFlagSink::channel();
      tell(&self.core.backend, move |backend| backend.is_other_audio_playing(sink)).await;
      reply(rx).await
    };
    match other_audio {
      Ok(false) => {}
      Ok(true) => {
        tracing::info!("peer connect: other audio active; not resuming");
        return;
      }
      Err(error) => {
        tracing::warn!(%error, "peer connect: the other-audio query failed; not resuming");
        return;
      }
    }
    if let Err(error) = self.core.command(AmPlayerCommand::Play).await {
      tracing::info!(%error, "connect auto-resume did not complete");
    }
  }

  async fn asset(&self, id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    let Some((url, max_edge)) = IMAGE_CODEC.parse(id) else {
      return Ok(None);
    };
    let scaled = self.core.art_cache.scaled(&url, max_edge).await;
    Ok(scaled.map(|bytes| AssetBytes {
      bytes,
      mime: Some("image/jpeg".into()),
    }))
  }

  async fn lyrics(&self, _track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError> {
    Ok(None)
  }

  async fn browse(&self, req: LibraryBrowseRequest) -> Result<BrowseResult, ProviderError> {
    match req.node_id.as_deref() {
      None | Some("") | Some("root") => self.core.root_browse(req.sections, req.preview).await,
      Some(PLAYLISTS_NODE_ID) => Ok(
        self
          .core
          .page_result(self.core.page(AmLibraryScope::Playlists, req.limit, req.offset).await?),
      ),
      Some(ALBUMS_NODE_ID) => Ok(
        self
          .core
          .page_result(self.core.page(AmLibraryScope::Albums, req.limit, req.offset).await?),
      ),
      Some(ARTISTS_NODE_ID) => Ok(
        self
          .core
          .page_result(self.core.page(AmLibraryScope::Artists, req.limit, req.offset).await?),
      ),
      Some(SONGS_NODE_ID) => Ok(
        self
          .core
          .page_result(self.core.page(AmLibraryScope::Songs, req.limit, req.offset).await?),
      ),
      Some(RECENTS_NODE_ID) => Ok(
        self.core.page_result(
          self
            .core
            .page(AmLibraryScope::RecentlyPlayed, req.limit, req.offset)
            .await?,
        ),
      ),
      Some(node) if node.starts_with(REC_NODE_PREFIX) => {
        let rail_id = node.trim_start_matches(REC_NODE_PREFIX).to_owned();
        let shelves = self.core.shelves().await?;
        let Some(shelf) = shelves.into_iter().find(|shelf| shelf.id == rail_id) else {
          return Ok(BrowseResult {
            entries: Vec::new(),
            total: Some(0),
            has_more: false,
          });
        };
        let page: Vec<&AmItem> = shelf
          .items
          .iter()
          .skip(req.offset as usize)
          .take(req.limit as usize)
          .collect();
        let taken = page.len();
        Ok(BrowseResult {
          entries: page
            .into_iter()
            .filter_map(|item| self.core.library_item(item).map(BrowseEntry::Item))
            .collect(),
          total: shelf.total.or(Some(shelf.items.len() as u32)),
          has_more: req.offset as usize + taken < shelf.items.len(),
        })
      }
      Some(node) => {
        let scope = AmLibraryScope::Children { uri: node.to_owned() };
        Ok(
          self
            .core
            .page_result(self.core.page(scope, req.limit, req.offset).await?),
        )
      }
    }
  }

  async fn resolve_context(&self, uri: &str) -> Result<ContextResolveReply, ProviderError> {
    let (sink, rx) = AmItemSink::channel();
    let asked = uri.to_owned();
    tell(&self.core.backend, move |backend| backend.resolve(asked, sink)).await;
    let item = reply(rx).await?;
    let (hero, _) = self.core.art_edges();
    Ok(ContextResolveReply {
      name: none_if_empty(&item.title),
      artwork_id: self.core.art_asset_id(item.artwork_url.as_deref(), hero),
      subtitle: item.subtitle.clone(),
    })
  }

  async fn search(&self, req: LibrarySearchRequest) -> Result<SearchResult, ProviderError> {
    let kinds = match &req.kinds {
      Some(kinds) if !kinds.is_empty() => kinds.clone(),
      _ => vec![ItemKind::Track, ItemKind::Album, ItemKind::Artist, ItemKind::Playlist],
    };
    let (sink, rx) = AmSearchSink::channel();
    let query = req.query.clone();
    let limit = req.limit;
    tell(&self.core.backend, move |backend| backend.search(query, limit, sink)).await;
    let results = reply(rx).await?;
    let limit = req.limit as usize;
    let mut items = Vec::new();
    let mut present = Vec::new();
    let mut full = false;
    for kind in kinds {
      let bucket = match kind {
        ItemKind::Track => &results.songs,
        ItemKind::Album => &results.albums,
        ItemKind::Artist => &results.artists,
        ItemKind::Playlist => &results.playlists,
        _ => continue,
      };
      let mapped: Vec<LibraryItem> = bucket.iter().filter_map(|item| self.core.library_item(item)).collect();
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

  async fn recommendations(&self, req: LibraryRecommendationsRequest) -> Result<RecommendationsResult, ProviderError> {
    if let Some(artist) = req.seeds.iter().find(|seed| seed.kind == ItemKind::Artist) {
      let scope = AmLibraryScope::Children {
        uri: artist.uri.clone(),
      };
      let page = self.core.page(scope, req.limit, req.offset).await?;
      return Ok(RecommendationsResult {
        items: page
          .items
          .iter()
          .filter_map(|item| self.core.library_item(item))
          .collect(),
        total: page.total,
        has_more: page.has_more,
      });
    }
    Ok(RecommendationsResult {
      items: Vec::new(),
      total: None,
      has_more: false,
    })
  }

  async fn favorites_list(&self, _req: LibraryFavoritesListRequest) -> Result<FavoritesPage, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn favorites_contains(&self, req: LibraryFavoritesContainsRequest) -> Result<Vec<bool>, ProviderError> {
    let (sink, rx) = AmFavoritesSink::channel();
    tell(&self.core.backend, move |backend| backend.is_favorite(req.uris, sink)).await;
    reply(rx).await
  }

  async fn favorites_toggle(&self, item: ItemRef) -> Result<(), ProviderError> {
    let cached = self.core.shared.lock().unwrap().liked_cache.get(&item.uri).copied();
    let current = match cached {
      Some(liked) => liked,
      None => self
        .favorites_contains(LibraryFavoritesContainsRequest {
          uris: vec![item.uri.clone()],
        })
        .await?
        .first()
        .copied()
        .unwrap_or(false),
    };
    if current {
      return Err(ProviderError::NotImplemented);
    }
    let uri = item.uri.clone();
    self
      .core
      .action(move |backend, sink| backend.add_favorite(uri, sink))
      .await?;
    self.core.apply_liked_change(&item.uri, true).await;
    Ok(())
  }

  async fn favorites_set(&self, item: ItemRef, liked: bool) -> Result<(), ProviderError> {
    if !liked {
      return Err(ProviderError::NotImplemented);
    }
    let uri = item.uri.clone();
    self
      .core
      .action(move |backend, sink| backend.add_favorite(uri, sink))
      .await?;
    self.core.apply_liked_change(&item.uri, true).await;
    Ok(())
  }

  async fn favorites_set_many(&self, entries: Vec<FavoritesSet>) -> Result<(), ProviderError> {
    for entry in entries {
      if !entry.liked {
        tracing::info!(uri = %entry.item.uri, "skipping unfavorite: apple music favorites are add-only");
        continue;
      }
      let uri = entry.item.uri.clone();
      self
        .core
        .action(move |backend, sink| backend.add_favorite(uri, sink))
        .await?;
      self.core.apply_liked_change(&entry.item.uri, true).await;
    }
    Ok(())
  }

  async fn set_art_profile(&self, hero_px: u32, thumb_px: u32) {
    let mut shared = self.core.shared.lock().unwrap();
    shared.hero_edge = hero_px.max(1);
    shared.thumb_edge = thumb_px.max(1);
  }
}
