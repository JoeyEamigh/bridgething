use std::collections::HashMap;

use crate::VoiceTargetKind;

pub const CHART_QUERY: &str = "top hits";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CatalogError(String);

impl CatalogError {
  pub fn new(reason: impl std::fmt::Display) -> Self {
    Self(reason.to_string())
  }
}

pub type CatalogResult<T> = Result<T, CatalogError>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogItem {
  pub uri: String,
  pub name: String,
  pub artist: Option<String>,
  pub year: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
  pub uri: String,
  pub name: String,
  pub released: (i32, i32, i32),
  pub popularity: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryPlaylist {
  pub uri: String,
  pub name: String,
  pub owned: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NowPlaying {
  pub track: String,
  pub album: Option<String>,
  pub artist: Option<String>,
  pub context: Option<String>,
}

#[async_trait::async_trait]
pub trait Catalog: Send + Sync {
  fn kind_of(&self, uri: &str) -> Option<VoiceTargetKind>;
  fn is_liked_songs(&self, uri: &str) -> bool;
  fn seeds_station(&self, kind: VoiceTargetKind) -> bool;

  async fn search(&self, query: &str, limit: u32) -> CatalogResult<Vec<CatalogItem>>;
  async fn hydrate(&self, uris: &[String]) -> Vec<CatalogItem>;
  async fn container(&self, uri: &str, limit: u32, offset: u32) -> CatalogResult<Vec<CatalogItem>>;
  async fn artist_top_tracks(&self, artist_uri: &str, artist_name: &str, limit: u32)
  -> CatalogResult<Vec<CatalogItem>>;
  async fn artist_releases(&self, artist_uri: &str, albums_only: bool, depth: usize) -> CatalogResult<Vec<Release>>;
  async fn station(&self, seed_uri: &str) -> Option<String>;

  async fn now_playing(&self) -> Option<NowPlaying>;
  async fn current_context(&self) -> Option<String>;
  async fn recently_played(&self, limit: usize) -> Vec<CatalogItem>;
  async fn recent_contexts(&self) -> Vec<String>;
  async fn fresh_pool(&self) -> Vec<String>;

  async fn liked_songs(&self) -> CatalogResult<CatalogItem>;
  async fn saved_albums(&self, limit: Option<usize>) -> CatalogResult<Vec<CatalogItem>>;
  async fn followed_artists(&self) -> CatalogResult<Vec<CatalogItem>>;
  async fn library_playlists(&self) -> CatalogResult<Vec<LibraryPlaylist>>;

  async fn popularity(&self, _uris: &[String]) -> HashMap<String, i32> {
    HashMap::new()
  }

  async fn new_releases(&self, _query: &str, _limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    Ok(Vec::new())
  }

  async fn charts(&self, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    self.search(CHART_QUERY, limit).await
  }

  async fn personal_mix(&self, _label: &str) -> Option<Vec<CatalogItem>> {
    None
  }
}
