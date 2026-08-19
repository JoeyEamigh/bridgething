pub mod apple_music;
pub mod art;
pub mod catalog;
pub mod spotify;
pub mod system_media;

use std::sync::Arc;

use bridgething_gateway::OutboundLink;
use libbridgething::{
  BrowseResult, FavoritesPage, ItemRef, Lyrics, MusicProvider, RecommendationsResult, RepeatMode, SearchResult,
  gateway::{
    ContextResolveReply, FavoritesSet, LibraryBrowseRequest, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, PlayUri, QueueUri, TrackIdentity,
  },
};

use crate::{hub::NowPlayingSink, voice::dispatcher::VoiceCatalogResolver};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetBytes {
  pub bytes: Vec<u8>,
  pub mime: Option<String>,
}

pub(crate) fn none_if_empty(text: &str) -> Option<String> {
  (!text.is_empty()).then(|| text.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
  #[error("not implemented")]
  NotImplemented,
  #[error("not authenticated")]
  NotAuthenticated,
  #[error("the provider is detached")]
  Detached,
  #[error("{0}")]
  Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderNowPlaying {
  pub update: libbridgething::NowPlayingUpdate,
  pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAuthState {
  Pending {
    user_code: Option<String>,
    verification_url: Option<String>,
    verification_url_complete: Option<String>,
  },
  Authenticated,
  Failed {
    reason: String,
  },
}

#[async_trait::async_trait]
pub trait PlayerTransport: Send + Sync {
  async fn play(&self, _uri: PlayUri) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn queue(&self, _req: QueueUri) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn pause(&self) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn resume(&self) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn skip_next(&self) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn skip_prev(&self) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn skip_to_index(&self, _index: u32) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn seek_to(&self, _position_ms: u32) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn set_shuffle(&self, _on: bool) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn set_repeat(&self, _mode: RepeatMode) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn set_speed(&self, _speed: f32) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn set_crossfade(&self, _duration_ms: Option<u32>) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }
}

#[derive(Clone)]
pub struct ProviderLink {
  pub sink: NowPlayingSink,
  pub outbound: Arc<dyn OutboundLink>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum ResumeTarget {
  #[default]
  PhoneOnly,
  AnySpeaker,
}

#[async_trait::async_trait]
pub trait Provider: PlayerTransport {
  fn name(&self) -> &str;
  fn display_name(&self) -> &str;
  fn uri_schemes(&self) -> Vec<String>;
  fn music_provider(&self) -> MusicProvider;
  fn supports_playback_targets(&self) -> bool {
    false
  }
  fn app_bundles(&self) -> Vec<String> {
    Vec::new()
  }

  async fn attach(&self, link: ProviderLink) -> Result<(), ProviderError>;
  async fn detach(&self);
  async fn handle_peer_connected(&self, _allow_auto_resume: bool) {}
  async fn resumed(&self) {}
  async fn connectivity_changed(&self, _online: bool) {}
  fn set_resume_target(&self, _target: ResumeTarget) {}

  fn set_now_playing_observer(&self, _observer: Option<Arc<dyn Fn(Option<ProviderNowPlaying>) + Send + Sync>>) {}
  fn set_auth_observer(&self, _observer: Option<Arc<dyn Fn(ProviderAuthState) + Send + Sync>>) {}

  fn voice_resolver(&self) -> Option<Arc<dyn VoiceCatalogResolver>> {
    None
  }

  async fn owns_volume(&self) -> bool {
    false
  }
  async fn volume_up(&self) -> Result<f32, ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn volume_down(&self) -> Result<f32, ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn set_volume(&self, _level: f32) -> Result<f32, ProviderError> {
    Err(ProviderError::NotImplemented)
  }
  async fn transfer_to(&self, _target_id: &str) -> Result<(), ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn asset(&self, id: &str) -> Result<Option<AssetBytes>, ProviderError>;
  async fn lyrics(&self, track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError>;

  async fn browse(&self, request: LibraryBrowseRequest) -> Result<BrowseResult, ProviderError>;
  async fn resolve_context(&self, uri: &str) -> Result<ContextResolveReply, ProviderError>;
  async fn search(&self, request: LibrarySearchRequest) -> Result<SearchResult, ProviderError>;
  async fn recommendations(
    &self,
    request: LibraryRecommendationsRequest,
  ) -> Result<RecommendationsResult, ProviderError>;
  async fn favorites_list(&self, request: LibraryFavoritesListRequest) -> Result<FavoritesPage, ProviderError>;
  async fn favorites_contains(&self, request: LibraryFavoritesContainsRequest) -> Result<Vec<bool>, ProviderError>;
  async fn favorites_toggle(&self, item: ItemRef) -> Result<(), ProviderError>;
  async fn favorites_set(&self, item: ItemRef, liked: bool) -> Result<(), ProviderError>;
  async fn favorites_set_many(&self, entries: Vec<FavoritesSet>) -> Result<(), ProviderError>;

  async fn set_art_profile(&self, hero_px: u32, thumb_px: u32);
}

pub trait ProviderRegistry: Send + Sync {
  fn library(&self) -> Option<Arc<dyn Provider>>;
  fn audible(&self) -> Option<Arc<dyn Provider>>;
  fn for_uri(&self, uri: &str) -> Option<Arc<dyn Provider>>;
  fn all(&self) -> Vec<Arc<dyn Provider>>;
}
