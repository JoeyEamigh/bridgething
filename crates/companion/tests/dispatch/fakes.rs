use std::{
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::provider::{
  AssetBytes, PlayerTransport, Provider, ProviderError, ProviderLink, ProviderRegistry,
};
use libbridgething::{
  BrowseResult, FavoritesPage, ItemRef, Lyrics, MusicProvider, RecommendationsResult, SearchResult,
  gateway::{
    ContextResolveReply, FavoritesSet, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, TrackIdentity,
  },
};

pub type AssetAnswer = Box<dyn Fn(&str) -> Result<Option<AssetBytes>, ProviderError> + Send + Sync>;
pub type LyricsAnswer = Box<dyn Fn(&TrackIdentity) -> Result<Option<Lyrics>, ProviderError> + Send + Sync>;
pub type BrowseAnswer = Box<dyn Fn(LibraryBrowseRequest) -> Result<BrowseResult, ProviderError> + Send + Sync>;
pub type SearchAnswer = Box<dyn Fn(LibrarySearchRequest) -> Result<SearchResult, ProviderError> + Send + Sync>;
pub type FavoritesAnswer =
  Box<dyn Fn(LibraryFavoritesListRequest) -> Result<FavoritesPage, ProviderError> + Send + Sync>;
pub type WriteAnswer = Box<dyn Fn() -> Result<(), ProviderError> + Send + Sync>;

#[derive(Default)]
pub struct FakeProvider {
  pub name: String,
  pub calls: Mutex<Vec<String>>,
  pub art_profile: Mutex<Option<(u32, u32)>>,
  pub delay: Option<Duration>,
  pub browse_delay: Mutex<Option<Duration>>,
  pub on_asset: Option<AssetAnswer>,
  pub on_lyrics: Option<LyricsAnswer>,
  pub on_browse: Option<BrowseAnswer>,
  pub on_search: Option<SearchAnswer>,
  pub on_favorites_list: Option<FavoritesAnswer>,
  pub on_write: Option<WriteAnswer>,
}

impl FakeProvider {
  pub fn named(name: &str) -> Self {
    Self {
      name: name.to_owned(),
      ..Self::default()
    }
  }

  pub fn bare(name: &str) -> Arc<Self> {
    Arc::new(Self::named(name))
  }

  fn called(&self, what: &str) {
    self.calls.lock().unwrap().push(what.to_owned());
  }

  pub fn saw(&self, what: &str) -> bool {
    self.calls.lock().unwrap().iter().any(|call| call == what)
  }
}

impl PlayerTransport for FakeProvider {}

#[async_trait::async_trait]
impl Provider for FakeProvider {
  fn name(&self) -> &str {
    &self.name
  }

  fn display_name(&self) -> &str {
    &self.name
  }

  fn uri_schemes(&self) -> Vec<String> {
    vec![self.name.clone()]
  }

  fn music_provider(&self) -> MusicProvider {
    MusicProvider::None
  }

  async fn attach(&self, _link: ProviderLink) -> Result<(), ProviderError> {
    Ok(())
  }

  async fn detach(&self) {}

  async fn asset(&self, id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    self.called(&format!("asset:{id}"));
    if let Some(delay) = self.delay {
      tokio::time::sleep(delay).await;
    }
    match &self.on_asset {
      Some(answer) => answer(id),
      None => Ok(None),
    }
  }

  async fn lyrics(&self, track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError> {
    self.called("lyrics");
    match &self.on_lyrics {
      Some(answer) => answer(track),
      None => Ok(None),
    }
  }

  async fn browse(&self, request: LibraryBrowseRequest) -> Result<BrowseResult, ProviderError> {
    self.called("browse");
    let delay = *self.browse_delay.lock().unwrap();
    if let Some(delay) = delay {
      tokio::time::sleep(delay).await;
    }
    match &self.on_browse {
      Some(answer) => answer(request),
      None => Err(ProviderError::NotImplemented),
    }
  }

  async fn resolve_context(&self, uri: &str) -> Result<ContextResolveReply, ProviderError> {
    self.called(&format!("resolveContext:{uri}"));
    Err(ProviderError::NotImplemented)
  }

  async fn search(&self, request: LibrarySearchRequest) -> Result<SearchResult, ProviderError> {
    self.called(&format!("search:{}", request.query));
    match &self.on_search {
      Some(answer) => answer(request),
      None => Err(ProviderError::NotImplemented),
    }
  }

  async fn recommendations(
    &self,
    _request: LibraryRecommendationsRequest,
  ) -> Result<RecommendationsResult, ProviderError> {
    self.called("recommendations");
    Err(ProviderError::NotImplemented)
  }

  async fn favorites_list(&self, request: LibraryFavoritesListRequest) -> Result<FavoritesPage, ProviderError> {
    self.called("favoritesList");
    match &self.on_favorites_list {
      Some(answer) => answer(request),
      None => Err(ProviderError::NotImplemented),
    }
  }

  async fn favorites_contains(&self, _request: LibraryFavoritesContainsRequest) -> Result<Vec<bool>, ProviderError> {
    self.called("favoritesContains");
    Err(ProviderError::NotImplemented)
  }

  async fn favorites_toggle(&self, item: ItemRef) -> Result<(), ProviderError> {
    self.called(&format!("favoritesToggle:{}", item.uri));
    match &self.on_write {
      Some(answer) => answer(),
      None => Ok(()),
    }
  }

  async fn favorites_set(&self, item: ItemRef, liked: bool) -> Result<(), ProviderError> {
    self.called(&format!("favoritesSet:{}:{liked}", item.uri));
    match &self.on_write {
      Some(answer) => answer(),
      None => Ok(()),
    }
  }

  async fn favorites_set_many(&self, entries: Vec<FavoritesSet>) -> Result<(), ProviderError> {
    self.called(&format!("favoritesSetMany:{}", entries.len()));
    match &self.on_write {
      Some(answer) => answer(),
      None => Ok(()),
    }
  }

  async fn set_art_profile(&self, hero_px: u32, thumb_px: u32) {
    *self.art_profile.lock().unwrap() = Some((hero_px, thumb_px));
  }
}

#[derive(Default)]
pub struct FakeRegistry {
  pub providers: Vec<Arc<FakeProvider>>,
  pub library: Option<usize>,
  pub audible: Option<usize>,
}

impl FakeRegistry {
  pub fn with(provider: Arc<FakeProvider>) -> Arc<Self> {
    Arc::new(Self {
      providers: vec![provider],
      library: Some(0),
      audible: None,
    })
  }

  pub fn of(providers: Vec<Arc<FakeProvider>>) -> Arc<Self> {
    Arc::new(Self {
      providers,
      library: Some(0),
      audible: None,
    })
  }

  pub fn empty() -> Arc<Self> {
    Arc::new(Self::default())
  }
}

impl ProviderRegistry for FakeRegistry {
  fn library(&self) -> Option<Arc<dyn Provider>> {
    self.library.map(|at| self.providers[at].clone() as Arc<dyn Provider>)
  }

  fn audible(&self) -> Option<Arc<dyn Provider>> {
    self.audible.map(|at| self.providers[at].clone() as Arc<dyn Provider>)
  }

  fn for_uri(&self, uri: &str) -> Option<Arc<dyn Provider>> {
    let scheme = uri.split(':').next()?;
    self
      .providers
      .iter()
      .find(|provider| provider.name == scheme)
      .map(|provider| provider.clone() as Arc<dyn Provider>)
  }

  fn all(&self) -> Vec<Arc<dyn Provider>> {
    self
      .providers
      .iter()
      .map(|provider| provider.clone() as Arc<dyn Provider>)
      .collect()
  }
}
