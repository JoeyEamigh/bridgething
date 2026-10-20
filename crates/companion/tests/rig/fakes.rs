use std::sync::{Arc, Mutex};

use bridgething_companion::{
  provider::{AssetBytes, PlayerTransport, Provider, ProviderAuthState, ProviderError, ProviderLink},
  voice::dispatcher::{CatalogError, VoiceCatalogResolver},
};
use libbridgething::{
  BrowseResult, FavoritesPage, ItemRef, Lyrics, MusicProvider, NluResolvedIntent, PlayerState, RecommendationsResult,
  SearchResult,
  gateway::{
    ContextResolveReply, FavoritesSet, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, TrackIdentity,
  },
};

pub type AuthObserver = Arc<dyn Fn(ProviderAuthState) + Send + Sync>;

#[derive(Default)]
pub struct SourceCatalog {
  pub uri: String,
  pub searched: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl VoiceCatalogResolver for SourceCatalog {
  async fn decorate(&self, mut resolved: NluResolvedIntent) -> Result<NluResolvedIntent, CatalogError> {
    let Some(target) = resolved.slots.target.clone() else {
      return Ok(resolved);
    };
    self.searched.lock().unwrap().push(target);
    resolved.slots.uri = Some(self.uri.clone());
    Ok(resolved)
  }
}

pub struct FakeSource {
  pub name: String,
  pub link: Mutex<Option<ProviderLink>>,
  pub auth: Mutex<Option<AuthObserver>>,
  pub connectivity: Mutex<Vec<bool>>,
  pub catalog: Option<Arc<SourceCatalog>>,
  pub library: Mutex<Option<Result<BrowseResult, ProviderError>>>,
}

impl FakeSource {
  pub fn new(name: &str) -> Arc<Self> {
    FakeSource::build(name, None)
  }

  pub fn build(name: &str, catalog: Option<Arc<SourceCatalog>>) -> Arc<Self> {
    Arc::new(Self {
      name: name.to_owned(),
      link: Mutex::new(None),
      auth: Mutex::new(None),
      connectivity: Mutex::new(Vec::new()),
      catalog,
      library: Mutex::new(None),
    })
  }

  pub fn submit(&self, state: PlayerState) {
    let held = self.link.lock().unwrap();
    let link = held.as_ref().expect("the source is attached");
    link
      .sink
      .submit_player(&self.name, state, "dev.bridgething.rig", true, false);
  }
}

#[async_trait::async_trait]
impl PlayerTransport for FakeSource {}

#[async_trait::async_trait]
impl Provider for FakeSource {
  fn name(&self) -> &str {
    &self.name
  }
  fn display_name(&self) -> &str {
    "Rig"
  }
  fn uri_schemes(&self) -> Vec<String> {
    vec![self.name.clone()]
  }
  fn music_provider(&self) -> MusicProvider {
    MusicProvider::None
  }

  fn voice_resolver(&self) -> Option<Arc<dyn VoiceCatalogResolver>> {
    self
      .catalog
      .clone()
      .map(|catalog| catalog as Arc<dyn VoiceCatalogResolver>)
  }

  fn set_auth_observer(&self, observer: Option<Arc<dyn Fn(ProviderAuthState) + Send + Sync>>) {
    *self.auth.lock().unwrap() = observer;
  }

  async fn attach(&self, link: ProviderLink) -> Result<(), ProviderError> {
    *self.link.lock().unwrap() = Some(link);
    Ok(())
  }

  async fn detach(&self) {
    *self.link.lock().unwrap() = None;
    *self.auth.lock().unwrap() = None;
  }

  async fn connectivity_changed(&self, online: bool) {
    self.connectivity.lock().unwrap().push(online);
  }

  async fn asset(&self, _id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    Ok(None)
  }
  async fn lyrics(&self, _track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError> {
    Ok(None)
  }
  async fn browse(&self, _request: LibraryBrowseRequest) -> Result<BrowseResult, ProviderError> {
    self
      .library
      .lock()
      .unwrap()
      .clone()
      .unwrap_or(Err(ProviderError::NotImplemented))
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
    Err(ProviderError::NotImplemented)
  }
  async fn favorites_set(&self, _item: ItemRef, _liked: bool) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn favorites_set_many(&self, _entries: Vec<FavoritesSet>) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn set_art_profile(&self, _hero_px: u32, _thumb_px: u32) {}
}
