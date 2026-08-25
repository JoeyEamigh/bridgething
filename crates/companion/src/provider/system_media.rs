use std::sync::{Arc, Mutex};

use libbridgething::{
  BrowseResult, FavoritesPage, ItemRef, Lyrics, MediaItem, MusicProvider, Playback, PlaybackContext, PlaybackState,
  PlayerOptions, PlayerState, QueueItem, RecommendationsResult, RepeatMode, SearchResult, ShuffleMode,
  gateway::{
    ContextResolveReply, FavoritesSet, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, QueueSnapshot, TrackIdentity,
  },
};
use tokio::task::JoinHandle;

use crate::{
  backend::{
    MediaArtSink, MediaControl, MediaQueueEntry, MediaRepeatMode, MediaSessionBackend, MediaSessionInbox,
    MediaSessionSnapshot, MediaSnapshotSink,
  },
  dispatch::tell,
  hub::NowPlayingSink,
  provider::{AssetBytes, PlayerTransport, Provider, ProviderError, ProviderLink},
};

pub const SOURCE_ID: &str = "system";
pub const ASSET_ID_PREFIX: &str = "system-art:";

pub type OwnedBundles = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

#[derive(Default)]
struct Audible {
  package: Option<String>,
  upcoming: Vec<MediaQueueEntry>,
  last_player: Option<(String, MediaSessionSnapshot)>,
  last_queue: Option<(String, Vec<MediaQueueEntry>)>,
  last_liked: Option<bool>,
}

struct Core {
  backend: Arc<dyn MediaSessionBackend>,
  owned: OwnedBundles,
  state: Mutex<Audible>,
  sink: Mutex<Option<NowPlayingSink>>,
}

impl Core {
  fn current_sink(&self) -> Option<NowPlayingSink> {
    self.sink.lock().unwrap().clone()
  }

  fn audible_package(&self) -> Option<String> {
    self.state.lock().unwrap().package.clone()
  }

  async fn control(&self, cmd: MediaControl) {
    let Some(package) = self.audible_package() else { return };
    tell(&self.backend, move |backend| backend.control(package, cmd)).await;
  }

  async fn snapshots(&self) -> Vec<MediaSessionSnapshot> {
    let granted = {
      let backend = self.backend.clone();
      tokio::task::spawn_blocking(move || backend.is_access_granted())
        .await
        .unwrap_or(false)
    };
    if !granted {
      return Vec::new();
    }
    let (sink, rx) = MediaSnapshotSink::channel();
    tell(&self.backend, move |backend| backend.snapshot_all(sink)).await;
    rx.await.unwrap_or_default()
  }

  async fn recompute(&self) {
    let Some(sink) = self.current_sink() else { return };
    let owned = (self.owned)();
    let sessions = self.snapshots().await;
    let visible: Vec<MediaSessionSnapshot> = sessions
      .into_iter()
      .filter(|snap| !owned.contains(&snap.package))
      .collect();
    let held = self.state.lock().unwrap().package.clone();
    let picked = visible.iter().find(|snap| snap.playing).or_else(|| {
      held
        .as_deref()
        .and_then(|package| visible.iter().find(|snap| snap.package == package))
    });

    let Some(snap) = picked else {
      let cleared = {
        let mut state = self.state.lock().unwrap();
        let had = state.last_player.is_some();
        *state = Audible::default();
        had
      };
      if cleared {
        tracing::debug!(
          held = ?held.as_deref(),
          roster = ?visible.iter().map(|snap| snap.package.as_str()).collect::<Vec<_>>(),
          "the session behind the system source is gone; dropping the source"
        );
        sink.clear_source(SOURCE_ID);
      }
      return;
    };
    tracing::trace!(
      picked = %snap.package,
      playing = snap.playing,
      held = ?held.as_deref(),
      roster = ?visible.iter().map(|snap| snap.package.as_str()).collect::<Vec<_>>(),
      "the system source picked a session"
    );

    let upcoming = upcoming_window(snap);
    let package = snap.package.clone();
    let player_key = MediaSessionSnapshot {
      queue: Vec::new(),
      active_queue_id: None,
      position_age_ms: None,
      ..snap.clone()
    };
    let (push_player, push_queue) = {
      let mut state = self.state.lock().unwrap();
      state.package = Some(package.clone());
      state.upcoming = upcoming.clone();
      state.last_liked = snap.liked;
      let push_player = state.last_player.as_ref() != Some(&(package.clone(), player_key.clone()));
      if push_player {
        state.last_player = Some((package.clone(), player_key));
      }
      let push_queue = state.last_queue.as_ref() != Some(&(package.clone(), upcoming.clone()));
      if push_queue {
        state.last_queue = Some((package.clone(), upcoming.clone()));
      }
      (push_player, push_queue)
    };
    if push_player {
      let has_item = snap.title.is_some() || snap.artist.is_some();
      sink.submit_player(SOURCE_ID, to_player_state(snap), &package, has_item, false);
    }
    if push_queue {
      sink.submit_queue(SOURCE_ID, to_queue_snapshot(&upcoming, &package));
    }
  }

  async fn set_liked(&self, liked: bool) {
    self.control(MediaControl::SetLiked { liked }).await;
  }

  async fn toggle_liked(&self) {
    let current = self.state.lock().unwrap().last_liked.unwrap_or(false);
    self.set_liked(!current).await;
  }
}

pub struct SystemMediaProvider {
  core: Arc<Core>,
  listener: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for SystemMediaProvider {
  fn drop(&mut self) {
    if let Some(task) = self.listener.lock().unwrap().take() {
      task.abort();
    }
  }
}

impl SystemMediaProvider {
  pub fn new(backend: Arc<dyn MediaSessionBackend>, owned: OwnedBundles) -> Arc<Self> {
    Arc::new(Self {
      core: Arc::new(Core {
        backend,
        owned,
        state: Mutex::new(Audible::default()),
        sink: Mutex::new(None),
      }),
      listener: Mutex::new(None),
    })
  }

  pub async fn refresh(&self) {
    self.core.recompute().await;
  }
}

fn upcoming_window(snap: &MediaSessionSnapshot) -> Vec<MediaQueueEntry> {
  if snap.queue.is_empty() {
    return Vec::new();
  }
  let active = snap
    .active_queue_id
    .and_then(|id| snap.queue.iter().position(|entry| entry.queue_id == id));
  match active {
    Some(at) => snap.queue.iter().skip(at + 1).cloned().collect(),
    None => snap.queue.clone(),
  }
}

fn stable_hash(text: &str) -> i32 {
  text
    .chars()
    .fold(0i32, |acc, c| acc.wrapping_mul(31).wrapping_add(c as i32))
}

fn to_queue_snapshot(entries: &[MediaQueueEntry], package: &str) -> QueueSnapshot {
  let items: Vec<QueueItem> = entries
    .iter()
    .map(|entry| {
      let uri = format!("system:{package}:q{}", entry.queue_id);
      QueueItem {
        uri: uri.clone(),
        title: entry.title.clone(),
        artist: entry.subtitle.clone(),
        artist_uri: None,
        album: None,
        album_uri: None,
        artwork_id: entry
          .art_token
          .as_ref()
          .map(|token| format!("{ASSET_ID_PREFIX}{token}")),
        duration_ms: None,
        persistent_id: Some(uri),
        queued: false,
      }
    })
    .collect();
  QueueSnapshot {
    order: items.iter().map(|item| item.uri.clone()).collect(),
    items,
  }
}

fn to_player_state(snap: &MediaSessionSnapshot) -> PlayerState {
  let track = if snap.title.is_none() && snap.artist.is_none() {
    None
  } else {
    let uri = format!(
      "system:{}:{}",
      snap.package,
      stable_hash(snap.title.as_deref().unwrap_or(""))
    );
    Some(MediaItem {
      uri: Some(uri.clone()),
      persistent_id: Some(uri),
      title: snap.title.clone(),
      album: snap.album.clone(),
      album_uri: None,
      album_artist: None,
      artist: snap.artist.clone(),
      artist_uri: None,
      liked: if snap.like_supported { snap.liked } else { None },
      artwork_id: snap.art_token.as_ref().map(|token| format!("{ASSET_ID_PREFIX}{token}")),
      duration_ms: snap
        .duration_ms
        .filter(|ms| *ms > 0)
        .map(|ms| ms.min(u32::MAX as i64) as u32),
      media_types: None,
      track_number: None,
      track_count: None,
      is_like_supported: if snap.like_supported { Some(true) } else { None },
      is_ban_supported: None,
      is_banned: None,
      chapter_count: None,
    })
  };
  let playback = Playback {
    state: if snap.playing {
      PlaybackState::Playing
    } else {
      PlaybackState::Paused
    },
    position_ms: snap.position_ms.clamp(0, u32::MAX as i64) as u32,
    position_age_ms: snap.position_age_ms.map(|age| age.clamp(0, u32::MAX as i64) as u32),
    shuffle: snap.shuffle.unwrap_or(false),
    shuffle_mode: snap
      .shuffle
      .map(|on| if on { ShuffleMode::Songs } else { ShuffleMode::Off }),
    repeat: snap.repeat.map(wire_repeat).unwrap_or(RepeatMode::Off),
    queue_index: None,
    queue_count: None,
    queue_chapter_index: None,
    set_elapsed_time_available: Some(snap.can_seek),
    queue_list_avail: None,
    apple_music_radio_ad: None,
  };
  PlayerState {
    track,
    playback,
    queue: Vec::new(),
    options: PlayerOptions {
      speed: snap.speed.unwrap_or(1.0),
      crossfade_ms: None,
    },
    context: snap.queue_title.as_ref().map(|title| PlaybackContext {
      uri: format!("system:{}:context", snap.package),
      name: Some(title.clone()),
    }),
    target: None,
  }
}

fn wire_repeat(mode: MediaRepeatMode) -> RepeatMode {
  match mode {
    MediaRepeatMode::Off => RepeatMode::Off,
    MediaRepeatMode::One => RepeatMode::One,
    MediaRepeatMode::All => RepeatMode::All,
  }
}

fn media_repeat(mode: RepeatMode) -> MediaRepeatMode {
  match mode {
    RepeatMode::Off => MediaRepeatMode::Off,
    RepeatMode::One => MediaRepeatMode::One,
    RepeatMode::All => MediaRepeatMode::All,
  }
}

#[async_trait::async_trait]
impl PlayerTransport for SystemMediaProvider {
  async fn pause(&self) -> Result<(), ProviderError> {
    self.core.control(MediaControl::Pause).await;
    Ok(())
  }

  async fn resume(&self) -> Result<(), ProviderError> {
    self.core.control(MediaControl::Play).await;
    Ok(())
  }

  async fn skip_next(&self) -> Result<(), ProviderError> {
    self.core.control(MediaControl::SkipNext).await;
    Ok(())
  }

  async fn skip_prev(&self) -> Result<(), ProviderError> {
    self.core.control(MediaControl::SkipPrev).await;
    Ok(())
  }

  async fn skip_to_index(&self, index: u32) -> Result<(), ProviderError> {
    let entry = self.core.state.lock().unwrap().upcoming.get(index as usize).cloned();
    let Some(entry) = entry else { return Ok(()) };
    self
      .core
      .control(MediaControl::SkipToQueueItem {
        queue_id: entry.queue_id,
      })
      .await;
    Ok(())
  }

  async fn seek_to(&self, position_ms: u32) -> Result<(), ProviderError> {
    self
      .core
      .control(MediaControl::SeekTo {
        position_ms: position_ms as i64,
      })
      .await;
    Ok(())
  }

  async fn set_shuffle(&self, on: bool) -> Result<(), ProviderError> {
    self.core.control(MediaControl::SetShuffle { on }).await;
    Ok(())
  }

  async fn set_repeat(&self, mode: RepeatMode) -> Result<(), ProviderError> {
    self
      .core
      .control(MediaControl::SetRepeat {
        mode: media_repeat(mode),
      })
      .await;
    Ok(())
  }

  async fn set_speed(&self, speed: f32) -> Result<(), ProviderError> {
    self.core.control(MediaControl::SetSpeed { speed }).await;
    Ok(())
  }
}

#[async_trait::async_trait]
impl Provider for SystemMediaProvider {
  fn name(&self) -> &str {
    SOURCE_ID
  }

  fn display_name(&self) -> &str {
    "System"
  }

  fn uri_schemes(&self) -> Vec<String> {
    vec![SOURCE_ID.to_string()]
  }

  fn music_provider(&self) -> MusicProvider {
    MusicProvider::None
  }

  async fn attach(&self, link: ProviderLink) -> Result<(), ProviderError> {
    *self.core.sink.lock().unwrap() = Some(link.sink);
    let (inbox, mut rx) = MediaSessionInbox::channel();
    tell(&self.core.backend, move |backend| backend.start(inbox)).await;
    let core = self.core.clone();
    let task = tokio::spawn(async move {
      core.recompute().await;
      while rx.recv().await.is_some() {
        core.recompute().await;
      }
    });
    if let Some(previous) = self.listener.lock().unwrap().replace(task) {
      previous.abort();
    }
    Ok(())
  }

  async fn detach(&self) {
    if let Some(task) = self.listener.lock().unwrap().take() {
      task.abort();
    }
    tell(&self.core.backend, |backend| backend.stop()).await;
    let sink = self.core.sink.lock().unwrap().take();
    *self.core.state.lock().unwrap() = Audible::default();
    if let Some(sink) = sink {
      sink.clear_source(SOURCE_ID);
    }
  }

  async fn asset(&self, id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    let Some(token) = id.strip_prefix(ASSET_ID_PREFIX) else {
      return Ok(None);
    };
    let Some(package) = self.core.audible_package() else {
      return Ok(None);
    };
    let (sink, rx) = MediaArtSink::channel();
    let token = token.to_string();
    tell(&self.core.backend, move |backend| backend.art(package, token, sink)).await;
    Ok(rx.await.ok().flatten().map(|art| AssetBytes {
      bytes: art.bytes,
      mime: Some(art.mime),
    }))
  }

  async fn lyrics(&self, _track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError> {
    Ok(None)
  }

  async fn browse(&self, _request: LibraryBrowseRequest) -> Result<BrowseResult, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn resolve_context(&self, _uri: &str) -> Result<ContextResolveReply, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn search(&self, _request: LibrarySearchRequest) -> Result<SearchResult, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn recommendations(
    &self,
    _request: LibraryRecommendationsRequest,
  ) -> Result<RecommendationsResult, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn favorites_list(&self, _request: LibraryFavoritesListRequest) -> Result<FavoritesPage, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn favorites_contains(&self, _request: LibraryFavoritesContainsRequest) -> Result<Vec<bool>, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn favorites_toggle(&self, _item: ItemRef) -> Result<(), ProviderError> {
    self.core.toggle_liked().await;
    Ok(())
  }

  async fn favorites_set(&self, _item: ItemRef, liked: bool) -> Result<(), ProviderError> {
    self.core.set_liked(liked).await;
    Ok(())
  }

  async fn favorites_set_many(&self, entries: Vec<FavoritesSet>) -> Result<(), ProviderError> {
    for entry in entries {
      self.core.set_liked(entry.liked).await;
    }
    Ok(())
  }

  async fn set_art_profile(&self, _hero_px: u32, _thumb_px: u32) {}
}
