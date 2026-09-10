use std::sync::{Arc, Mutex};

use libbridgething::{
  BrowseResult, FavoritesPage, ItemRef, Lyrics, MediaItem, MusicProvider, Playback, PlaybackState, PlayerState,
  RecommendationsResult, SearchResult,
  gateway::{
    ContextResolveReply, FavoritesSet, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, PlayUri, TrackIdentity,
  },
};
use tokio::task::JoinHandle;

use crate::{
  backend::{StreamBackend, StreamEvent, StreamSink},
  dispatch::tell,
  hub::NowPlayingSink,
  provider::{AssetBytes, PlayerTransport, Provider, ProviderError, ProviderLink},
};

pub const SOURCE_ID: &str = "stream";

struct Current {
  url: String,
  title: String,
  playing: bool,
}

struct Core {
  backend: Arc<dyn StreamBackend>,
  state: Mutex<Option<Current>>,
  sink: Mutex<Option<NowPlayingSink>>,
  listener: Mutex<Option<JoinHandle<()>>>,
}

impl Core {
  fn current_sink(&self) -> Option<NowPlayingSink> {
    self.sink.lock().unwrap().clone()
  }

  fn submit(&self) {
    let Some(sink) = self.current_sink() else { return };
    match self.state.lock().unwrap().as_ref() {
      Some(current) => sink.submit_player(SOURCE_ID, player_state(current), SOURCE_ID, true, false),
      None => sink.clear_source(SOURCE_ID),
    }
  }

  fn on_event(&self, event: StreamEvent) {
    match event {
      StreamEvent::Started => {
        if let Some(current) = self.state.lock().unwrap().as_mut() {
          current.playing = true;
        }
        self.submit();
      }
      StreamEvent::Stopped { error } => {
        if let Some(reason) = error {
          tracing::warn!(reason, "the stream stopped with an error");
        }
        *self.state.lock().unwrap() = None;
        self.submit();
      }
    }
  }
}

fn stream_title(url: &str) -> String {
  url::Url::parse(url)
    .ok()
    .and_then(|parsed| parsed.host_str().map(str::to_owned))
    .unwrap_or_else(|| url.to_owned())
}

fn player_state(current: &Current) -> PlayerState {
  PlayerState {
    track: Some(MediaItem {
      uri: Some(current.url.clone()),
      persistent_id: Some(current.url.clone()),
      title: Some(current.title.clone()),
      ..Default::default()
    }),
    playback: Playback {
      state: if current.playing {
        PlaybackState::Playing
      } else {
        PlaybackState::Paused
      },
      set_elapsed_time_available: Some(false),
      ..Default::default()
    },
    ..Default::default()
  }
}

pub struct StreamProvider {
  core: Arc<Core>,
}

impl StreamProvider {
  pub fn new(backend: Arc<dyn StreamBackend>) -> Arc<Self> {
    Arc::new(Self {
      core: Arc::new(Core {
        backend,
        state: Mutex::new(None),
        sink: Mutex::new(None),
        listener: Mutex::new(None),
      }),
    })
  }

  async fn play_url(&self, url: String) {
    self.stop_current().await;
    let (sink, mut rx) = StreamSink::channel();
    let backend = self.core.backend.clone();
    let url_for_backend = url.clone();
    tell(&backend, move |backend| backend.play(url_for_backend, sink)).await;
    *self.core.state.lock().unwrap() = Some(Current {
      title: stream_title(&url),
      url,
      playing: true,
    });
    self.core.submit();
    let core = self.core.clone();
    let task = tokio::spawn(async move {
      while let Some(event) = rx.recv().await {
        core.on_event(event);
      }
    });
    if let Some(previous) = self.core.listener.lock().unwrap().replace(task) {
      previous.abort();
    }
  }

  async fn stop_current(&self) {
    if let Some(task) = self.core.listener.lock().unwrap().take() {
      task.abort();
    }
    if self.core.state.lock().unwrap().is_some() {
      tell(&self.core.backend, |backend| backend.stop()).await;
      *self.core.state.lock().unwrap() = None;
      self.core.submit();
    }
  }
}

impl Drop for StreamProvider {
  fn drop(&mut self) {
    if let Some(task) = self.core.listener.lock().unwrap().take() {
      task.abort();
    }
  }
}

#[async_trait::async_trait]
impl PlayerTransport for StreamProvider {
  async fn play(&self, uri: PlayUri) -> Result<(), ProviderError> {
    self.play_url(uri.uri).await;
    Ok(())
  }

  async fn pause(&self) -> Result<(), ProviderError> {
    if self.core.state.lock().unwrap().is_some() {
      tell(&self.core.backend, |backend| backend.pause()).await;
      if let Some(current) = self.core.state.lock().unwrap().as_mut() {
        current.playing = false;
      }
      self.core.submit();
    }
    Ok(())
  }

  async fn resume(&self) -> Result<(), ProviderError> {
    if self.core.state.lock().unwrap().is_some() {
      tell(&self.core.backend, |backend| backend.resume()).await;
      if let Some(current) = self.core.state.lock().unwrap().as_mut() {
        current.playing = true;
      }
      self.core.submit();
    }
    Ok(())
  }
}

#[async_trait::async_trait]
impl Provider for StreamProvider {
  fn name(&self) -> &str {
    SOURCE_ID
  }

  fn display_name(&self) -> &str {
    "Stream"
  }

  fn uri_schemes(&self) -> Vec<String> {
    vec!["http".to_string(), "https".to_string()]
  }

  fn music_provider(&self) -> MusicProvider {
    MusicProvider::None
  }

  async fn attach(&self, link: ProviderLink) -> Result<(), ProviderError> {
    *self.core.sink.lock().unwrap() = Some(link.sink);
    Ok(())
  }

  async fn detach(&self) {
    self.stop_current().await;
    *self.core.sink.lock().unwrap() = None;
  }

  async fn asset(&self, _id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    Ok(None)
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
    Ok(Vec::new())
  }

  async fn favorites_toggle(&self, _item: ItemRef) -> Result<(), ProviderError> {
    Ok(())
  }

  async fn favorites_set(&self, _item: ItemRef, _liked: bool) -> Result<(), ProviderError> {
    Ok(())
  }

  async fn favorites_set_many(&self, _entries: Vec<FavoritesSet>) -> Result<(), ProviderError> {
    Ok(())
  }

  async fn set_art_profile(&self, _hero_px: u32, _thumb_px: u32) {}
}
