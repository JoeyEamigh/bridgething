use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
};

use crate::{Catalog, CatalogError, CatalogItem, CatalogResult, LibraryPlaylist, NowPlaying, Release, VoiceTargetKind};

#[derive(Default)]
pub(crate) struct FakeCatalog {
  pub(crate) hits: Vec<CatalogItem>,
  pub(crate) items: HashMap<String, CatalogItem>,
  pub(crate) containers: HashMap<String, Vec<CatalogItem>>,
  pub(crate) top_tracks: HashMap<String, Vec<CatalogItem>>,
  pub(crate) releases: HashMap<String, Vec<Release>>,
  pub(crate) playing: Option<NowPlaying>,
  pub(crate) liked: Option<CatalogItem>,
  pub(crate) saved_albums: Vec<CatalogItem>,
  pub(crate) followed: Vec<CatalogItem>,
  pub(crate) playlists: Vec<LibraryPlaylist>,
  pub(crate) recents: Vec<CatalogItem>,
  pub(crate) pool: Vec<String>,
  pub(crate) station_kinds: Vec<VoiceTargetKind>,
  pub(crate) stations: bool,
  pub(crate) log: Arc<Mutex<Vec<String>>>,
}

pub(crate) fn kind_of(uri: &str) -> Option<VoiceTargetKind> {
  match uri.split(':').nth(1)? {
    "track" => Some(VoiceTargetKind::Track),
    "album" => Some(VoiceTargetKind::Album),
    "artist" => Some(VoiceTargetKind::Artist),
    "playlist" | "liked" => Some(VoiceTargetKind::Playlist),
    "show" => Some(VoiceTargetKind::Show),
    "episode" => Some(VoiceTargetKind::Episode),
    "station" => Some(VoiceTargetKind::Station),
    _ => None,
  }
}

pub(crate) fn item(uri: &str, name: &str) -> CatalogItem {
  CatalogItem {
    uri: uri.into(),
    name: name.into(),
    ..Default::default()
  }
}

pub(crate) fn credited(uri: &str, name: &str, artist: &str, year: Option<i32>) -> CatalogItem {
  CatalogItem {
    uri: uri.into(),
    name: name.into(),
    artist: Some(artist.into()),
    year,
  }
}

impl FakeCatalog {
  pub(crate) fn searching(hits: Vec<CatalogItem>) -> Self {
    Self {
      hits,
      station_kinds: vec![
        VoiceTargetKind::Artist,
        VoiceTargetKind::Track,
        VoiceTargetKind::Album,
        VoiceTargetKind::Playlist,
      ],
      stations: true,
      ..Default::default()
    }
  }

  pub(crate) fn asked(&self) -> Vec<String> {
    self.log.lock().unwrap().clone()
  }

  fn note(&self, entry: String) {
    self.log.lock().unwrap().push(entry);
  }
}

#[async_trait::async_trait]
impl Catalog for FakeCatalog {
  fn kind_of(&self, uri: &str) -> Option<VoiceTargetKind> {
    kind_of(uri)
  }

  fn is_liked_songs(&self, uri: &str) -> bool {
    uri.split(':').nth(1) == Some("liked")
  }

  fn seeds_station(&self, kind: VoiceTargetKind) -> bool {
    self.station_kinds.contains(&kind)
  }

  async fn search(&self, query: &str, _limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    self.note(format!("search {query}"));
    Ok(self.hits.clone())
  }

  async fn hydrate(&self, uris: &[String]) -> Vec<CatalogItem> {
    uris
      .iter()
      .map(|uri| self.items.get(uri).cloned().unwrap_or_else(|| item(uri, "")))
      .collect()
  }

  async fn container(&self, uri: &str, limit: u32, offset: u32) -> CatalogResult<Vec<CatalogItem>> {
    self.note(format!("container {uri}"));
    let all = self
      .containers
      .get(uri)
      .ok_or_else(|| CatalogError::new(format!("no container {uri}")))?;
    Ok(all.iter().skip(offset as usize).take(limit as usize).cloned().collect())
  }

  async fn artist_top_tracks(
    &self,
    artist_uri: &str,
    _artist_name: &str,
    limit: u32,
  ) -> CatalogResult<Vec<CatalogItem>> {
    let all = self.top_tracks.get(artist_uri).cloned().unwrap_or_default();
    Ok(all.into_iter().take(limit as usize).collect())
  }

  async fn artist_releases(&self, artist_uri: &str, _albums_only: bool, _depth: usize) -> CatalogResult<Vec<Release>> {
    self
      .releases
      .get(artist_uri)
      .cloned()
      .ok_or_else(|| CatalogError::new(format!("no discography for {artist_uri}")))
  }

  async fn station(&self, seed_uri: &str) -> Option<String> {
    self.stations.then(|| seed_uri.replacen(':', ":station:", 1))
  }

  async fn now_playing(&self) -> Option<NowPlaying> {
    self.playing.clone()
  }

  async fn current_context(&self) -> Option<String> {
    self.playing.as_ref().and_then(|p| p.context.clone())
  }

  async fn recently_played(&self, limit: usize) -> Vec<CatalogItem> {
    self.recents.iter().take(limit).cloned().collect()
  }

  async fn recent_contexts(&self) -> Vec<String> {
    self.recents.iter().map(|i| i.uri.clone()).collect()
  }

  async fn fresh_pool(&self) -> Vec<String> {
    self.pool.clone()
  }

  async fn liked_songs(&self) -> CatalogResult<CatalogItem> {
    self.liked.clone().ok_or_else(|| CatalogError::new("no liked songs"))
  }

  async fn saved_albums(&self, limit: Option<usize>) -> CatalogResult<Vec<CatalogItem>> {
    Ok(
      self
        .saved_albums
        .iter()
        .take(limit.unwrap_or(usize::MAX))
        .cloned()
        .collect(),
    )
  }

  async fn followed_artists(&self) -> CatalogResult<Vec<CatalogItem>> {
    Ok(self.followed.clone())
  }

  async fn library_playlists(&self) -> CatalogResult<Vec<LibraryPlaylist>> {
    Ok(self.playlists.clone())
  }
}
