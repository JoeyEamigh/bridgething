use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
  },
  time::{Duration, Instant},
};

use bridgething_gateway::{OutboundLink, OutboundLinkExt};
use bridgething_io::{
  DownloadBody, HttpExecutor, HttpHeader, HttpMethod, HttpRequest, HttpTransport as IoHttpTransport,
};
use libbridgething::{
  BrowseResult, FavoritesPage, ItemRef, Lyrics, MediaItem, MusicProvider, Playback, PlaybackState, PlayerError,
  PlayerState, RecommendationsResult, SearchResult,
  gateway::{
    ContextResolveReply, FavoritesSet, GatewayToBridgePlayerMsgEvent, LibraryBrowseRequest,
    LibraryFavoritesContainsRequest, LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest,
    PlayUri, PlayerErrorReply, TrackIdentity,
  },
};
use tokio::task::JoinHandle;

use crate::{
  backend::{
    ImageScaler, StreamBackend, StreamEvent, StreamMetadata, StreamSink, StreamSource, StreamStatus, StreamTiming,
  },
  dispatch::tell,
  provider::{
    AssetBytes, PlayerTransport, Provider, ProviderError, ProviderLink,
    art::{ArtCache, ImageAssetCodec},
  },
};

pub const SOURCE_ID: &str = "stream";
const DEFAULT_HERO_EDGE: u32 = 248;
const PROBE_DEADLINE: Duration = Duration::from_secs(3);
const IMAGE_CODEC: ImageAssetCodec = ImageAssetCodec {
  namespace: "stream/img/",
  short_form: None,
};

struct Current {
  url: String,
  live: bool,
  station: Option<String>,
  status: StreamStatus,
  metadata: StreamMetadata,
  timing: StreamTiming,
  timed_at: Instant,
}

struct Core {
  backend: Arc<dyn StreamBackend>,
  app_bundle: String,
  http: HttpExecutor,
  art_cache: ArtCache,
  epoch: AtomicU64,
  state: Mutex<Option<Current>>,
  link: Mutex<Option<ProviderLink>>,
  hero_edge: Mutex<u32>,
  listener: Mutex<Option<JoinHandle<()>>>,
}

impl Core {
  fn outbound(&self) -> Option<Arc<dyn OutboundLink>> {
    self.link.lock().unwrap().as_ref().map(|link| link.outbound.clone())
  }

  fn submit(&self) {
    let link = self.link.lock().unwrap();
    let Some(link) = link.as_ref() else { return };
    let hero_edge = *self.hero_edge.lock().unwrap();
    match self.state.lock().unwrap().as_ref() {
      Some(current) => link.sink.submit_player(
        SOURCE_ID,
        player_state(current, hero_edge),
        &self.app_bundle,
        true,
        false,
      ),
      None => link.sink.clear_source(SOURCE_ID),
    }
  }

  fn update(&self, apply: impl FnOnce(&mut Current)) {
    if let Some(current) = self.state.lock().unwrap().as_mut() {
      apply(current);
    }
    self.submit();
  }

  fn clear(&self) {
    *self.state.lock().unwrap() = None;
    self.submit();
  }

  async fn on_event(&self, event: StreamEvent) {
    match event {
      StreamEvent::Status(StreamStatus::Ended) => self.clear(),
      StreamEvent::Status(StreamStatus::Failed { reason }) => {
        tracing::warn!(%reason, "the stream failed");
        self.clear();
        if let Some(outbound) = self.outbound() {
          let _ = outbound
            .event(GatewayToBridgePlayerMsgEvent::ErrorEvent(PlayerErrorReply {
              error: PlayerError::PlayFailed { reason },
            }))
            .await;
        }
      }
      StreamEvent::Status(status) => self.update(|current| current.status = status),
      StreamEvent::Metadata(metadata) => self.update(|current| current.metadata = metadata),
      StreamEvent::Timing(timing) => self.update(|current| {
        current.timing = timing;
        current.timed_at = Instant::now();
      }),
    }
  }
}

fn stream_title(url: &str) -> String {
  url::Url::parse(url)
    .ok()
    .and_then(|parsed| parsed.host_str().map(str::to_owned))
    .unwrap_or_else(|| url.to_owned())
}

struct HeaderPeek {
  seen: Arc<Mutex<Option<Vec<HttpHeader>>>>,
}

impl DownloadBody for HeaderPeek {
  fn on_response(&mut self, _status: u16, headers: &[HttpHeader], _content_length: Option<u64>) -> bool {
    *self.seen.lock().unwrap() = Some(headers.to_vec());
    false
  }

  fn write(&mut self, _chunk: &[u8]) -> Result<(), String> {
    Ok(())
  }
}

fn icy_origin(headers: &[HttpHeader]) -> (bool, Option<String>) {
  let header = |name: &str| {
    headers
      .iter()
      .find(|header| header.name.eq_ignore_ascii_case(name))
      .map(|header| header.value.trim().to_owned())
  };
  let station = header("icy-name").filter(|name| !name.is_empty());
  let live = station.is_some() || header("icy-metaint").is_some();
  (live, station)
}

async fn probe(http: &HttpExecutor, url: &str) -> (bool, Option<String>) {
  let seen = Arc::new(Mutex::new(None));
  let request = HttpRequest {
    method: HttpMethod::Get,
    url: url.to_owned(),
    headers: vec![HttpHeader {
      name: "Icy-MetaData".into(),
      value: "1".into(),
    }],
    body: Vec::new(),
    timeout_ms: PROBE_DEADLINE.as_millis() as u32,
  };
  let peek = Box::new(HeaderPeek { seen: seen.clone() });
  let _ = tokio::time::timeout(PROBE_DEADLINE, http.download(request, peek)).await;
  let headers = seen.lock().unwrap().take().unwrap_or_default();
  icy_origin(&headers)
}

fn player_state(current: &Current, hero_edge: u32) -> PlayerState {
  let title = current
    .metadata
    .title
    .clone()
    .filter(|title| !title.trim().is_empty())
    .or_else(|| current.station.clone())
    .unwrap_or_else(|| stream_title(&current.url));
  let state = match current.status {
    StreamStatus::Buffering | StreamStatus::Playing => PlaybackState::Playing,
    StreamStatus::Paused => PlaybackState::Paused,
    StreamStatus::Ended | StreamStatus::Failed { .. } => PlaybackState::Stopped,
  };
  let position_age_ms = current.timed_at.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
  PlayerState {
    track: Some(MediaItem {
      uri: Some(current.url.clone()),
      persistent_id: Some(current.url.clone()),
      title: Some(title),
      artist: current.metadata.artist.clone(),
      album: current.metadata.album.clone(),
      artwork_id: current
        .metadata
        .artwork_url
        .as_deref()
        .and_then(|url| IMAGE_CODEC.asset_id(url, hero_edge)),
      duration_ms: current.timing.duration_ms.filter(|_| !current.live),
      ..Default::default()
    }),
    playback: Playback {
      state,
      position_ms: current.timing.position_ms,
      position_age_ms: Some(position_age_ms),
      set_elapsed_time_available: Some(current.timing.seekable && !current.live),
      ..Default::default()
    },
    ..Default::default()
  }
}

pub struct StreamProvider {
  core: Arc<Core>,
}

impl StreamProvider {
  pub fn new(
    backend: Arc<dyn StreamBackend>,
    app_bundle: String,
    http: Arc<dyn IoHttpTransport>,
    scaler: Option<Arc<dyn ImageScaler>>,
  ) -> Arc<Self> {
    Arc::new(Self {
      core: Arc::new(Core {
        backend,
        app_bundle,
        http: HttpExecutor::new(http.clone()),
        art_cache: ArtCache::new(HttpExecutor::new(http), scaler),
        epoch: AtomicU64::new(0),
        state: Mutex::new(None),
        link: Mutex::new(None),
        hero_edge: Mutex::new(DEFAULT_HERO_EDGE),
        listener: Mutex::new(None),
      }),
    })
  }

  async fn play_url(&self, url: String) {
    self.stop_current().await;
    let epoch = self.core.epoch.fetch_add(1, Ordering::SeqCst) + 1;
    *self.core.state.lock().unwrap() = Some(Current {
      url: url.clone(),
      live: false,
      station: None,
      status: StreamStatus::Buffering,
      metadata: StreamMetadata::default(),
      timing: StreamTiming::default(),
      timed_at: Instant::now(),
    });
    self.core.submit();
    let (live, station) = probe(&self.core.http, &url).await;
    if self.core.epoch.load(Ordering::SeqCst) != epoch {
      return;
    }
    self.core.update(|current| {
      current.live = live;
      current.station = station.clone();
    });
    let (sink, mut rx) = StreamSink::channel();
    let core = self.core.clone();
    let task = tokio::spawn(async move {
      while let Some(event) = rx.recv().await {
        core.on_event(event).await;
      }
    });
    if let Some(previous) = self.core.listener.lock().unwrap().replace(task) {
      previous.abort();
    }
    let source = StreamSource { url, live, station };
    tell(&self.core.backend, move |backend| backend.play(source, sink)).await;
  }

  async fn stop_current(&self) {
    self.core.epoch.fetch_add(1, Ordering::SeqCst);
    if let Some(task) = self.core.listener.lock().unwrap().take() {
      task.abort();
    }
    if self.core.state.lock().unwrap().is_some() {
      tell(&self.core.backend, |backend| backend.stop()).await;
      self.core.clear();
    }
  }

  fn has_stream(&self) -> bool {
    self.core.state.lock().unwrap().is_some()
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
    if self.has_stream() {
      tell(&self.core.backend, |backend| backend.pause()).await;
    }
    Ok(())
  }

  async fn resume(&self) -> Result<(), ProviderError> {
    if self.has_stream() {
      tell(&self.core.backend, |backend| backend.resume()).await;
    }
    Ok(())
  }

  async fn seek_to(&self, position_ms: u32) -> Result<(), ProviderError> {
    let seekable = self
      .core
      .state
      .lock()
      .unwrap()
      .as_ref()
      .is_some_and(|current| current.timing.seekable && !current.live);
    if !seekable {
      return Err(ProviderError::NotImplemented);
    }
    tell(&self.core.backend, move |backend| backend.seek_to(position_ms)).await;
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

  fn app_bundles(&self) -> Vec<String> {
    vec![self.core.app_bundle.clone()]
  }

  async fn attach(&self, link: ProviderLink) -> Result<(), ProviderError> {
    *self.core.link.lock().unwrap() = Some(link);
    Ok(())
  }

  async fn detach(&self) {
    self.stop_current().await;
    *self.core.link.lock().unwrap() = None;
  }

  async fn last_peer_gone(&self) {
    self.stop_current().await;
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

  async fn set_art_profile(&self, hero_px: u32, _thumb_px: u32) {
    *self.core.hero_edge.lock().unwrap() = hero_px.max(1);
    self.core.submit();
  }
}
