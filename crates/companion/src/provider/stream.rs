use std::sync::Arc;

use bridgething_io::{HttpExecutor, HttpTransport as IoHttpTransport};
use libbridgething::{
  BrowseResult, FavoritesPage, ItemRef, Lyrics, MusicProvider, PlaybackContext, RecommendationsResult, SearchResult,
  gateway::{
    ContextResolveReply, FavoritesSet, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, PlayUri, TrackIdentity,
  },
};

use crate::{
  backend::{ImageScaler, StreamBackend},
  provider::{
    AssetBytes, PlayerTransport, Provider, ProviderError, ProviderLink,
    art::{ArtCache, ImageAssetCodec},
    playback::{QueueEntry, StreamPlayback},
  },
};

pub const SOURCE_ID: &str = "stream";
const IMAGE_CODEC: ImageAssetCodec = ImageAssetCodec {
  namespace: "stream/img/",
  short_form: None,
};

pub struct StreamProvider {
  playback: StreamPlayback,
  art: Arc<ArtCache>,
}

impl StreamProvider {
  pub fn new(
    backend: Arc<dyn StreamBackend>,
    http: Arc<dyn IoHttpTransport>,
    scaler: Option<Arc<dyn ImageScaler>>,
  ) -> Arc<Self> {
    let art = Arc::new(ArtCache::new(HttpExecutor::new(http.clone()), scaler));
    Arc::new(Self {
      playback: StreamPlayback::new(backend, http, art.clone(), SOURCE_ID, Arc::new(IMAGE_CODEC)),
      art,
    })
  }
}

#[async_trait::async_trait]
impl PlayerTransport for StreamProvider {
  async fn play(&self, uri: PlayUri) -> Result<(), ProviderError> {
    let context = uri
      .context
      .filter(|c| !c.context_uri.is_empty())
      .map(|c| PlaybackContext {
        uri: c.context_uri,
        name: None,
      });
    self.playback.play(vec![QueueEntry::bare(&uri.uri)], 0, context).await;
    Ok(())
  }

  async fn pause(&self) -> Result<(), ProviderError> {
    self.playback.pause().await;
    Ok(())
  }

  async fn resume(&self) -> Result<(), ProviderError> {
    self.playback.resume().await;
    Ok(())
  }

  async fn seek_to(&self, position_ms: u32) -> Result<(), ProviderError> {
    self.playback.seek_to(position_ms).await
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
    self.playback.app_bundles()
  }

  async fn attach(&self, link: ProviderLink) -> Result<(), ProviderError> {
    self.playback.attach(link).await;
    Ok(())
  }

  async fn detach(&self) {
    self.playback.detach().await;
  }

  async fn asset(&self, id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    let Some((url, max_edge)) = IMAGE_CODEC.parse(id) else {
      return Ok(None);
    };
    let scaled = self.art.scaled(&url, max_edge).await;
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

  async fn set_art_profile(&self, hero_px: u32, thumb_px: u32) {
    self.playback.set_art_edges(hero_px, thumb_px);
  }
}
