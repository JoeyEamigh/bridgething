use std::collections::HashSet;

use voice_resolver::{
  Catalog, CatalogError, CatalogItem, CatalogResult, LibraryPlaylist, NowPlaying, Release, VoiceTargetKind, norm,
};

use super::{Core, LIBRARY_SEGMENT, SEARCH_LIMIT_MAX, URI_SCHEME, parse_uri, uri_of, window};
use crate::{
  backend::{AmItem, AmKind, AmLibraryScope},
  provider::ProviderError,
};

const LIBRARY_SONGS_ID: &str = "all";
const LIBRARY_SONGS_TITLE: &str = "Library Songs";
const FAVORITE_SONGS: &str = "favorite songs";
const RECENT_LIMIT: u32 = 40;

fn library_songs_uri() -> String {
  format!("{URI_SCHEME}{LIBRARY_SEGMENT}songs:{LIBRARY_SONGS_ID}")
}

impl From<ProviderError> for CatalogError {
  fn from(error: ProviderError) -> Self {
    CatalogError::new(error)
  }
}

fn kind_of(kind: AmKind) -> VoiceTargetKind {
  match kind {
    AmKind::Song => VoiceTargetKind::Track,
    AmKind::Album => VoiceTargetKind::Album,
    AmKind::Artist => VoiceTargetKind::Artist,
    AmKind::Playlist | AmKind::Songs => VoiceTargetKind::Playlist,
    AmKind::Station => VoiceTargetKind::Station,
  }
}

fn catalog_item(item: &AmItem) -> CatalogItem {
  CatalogItem {
    uri: uri_of(&item.item),
    name: item.title.clone(),
    artist: item.artist_name.clone(),
    year: item.released.map(|date| date.year),
  }
}

fn catalog_items<'a>(items: impl IntoIterator<Item = &'a AmItem>) -> Vec<CatalogItem> {
  items.into_iter().map(catalog_item).collect()
}

fn songs_of(items: &[AmItem]) -> Vec<AmItem> {
  items
    .iter()
    .filter(|item| item.item.kind == AmKind::Song)
    .cloned()
    .collect()
}

fn full_album(item: &AmItem) -> bool {
  let title = item.title.trim_end();
  item.single != Some(true) && !title.ends_with(" - Single") && !title.ends_with(" - EP")
}

fn favorites(playlists: &[AmItem]) -> Option<&AmItem> {
  playlists
    .iter()
    .find(|playlist| norm(&playlist.title) == FAVORITE_SONGS)
}

impl Core {
  async fn library_items(&self, scope: AmLibraryScope) -> CatalogResult<Vec<CatalogItem>> {
    Ok(catalog_items(self.whole_library(scope).await?.iter()))
  }

  async fn artist_children(&self, artist_uri: &str) -> CatalogResult<std::sync::Arc<Vec<AmItem>>> {
    let artist =
      parse_uri(artist_uri).ok_or_else(|| CatalogError::new(format!("not an apple music uri: {artist_uri}")))?;
    Ok(self.children(&artist).await?)
  }
}

#[async_trait::async_trait]
impl Catalog for Core {
  fn kind_of(&self, uri: &str) -> Option<VoiceTargetKind> {
    parse_uri(uri).map(|item| kind_of(item.kind))
  }

  fn is_liked_songs(&self, uri: &str) -> bool {
    uri == library_songs_uri()
      || self
        .held_library(AmLibraryScope::Playlists)
        .is_some_and(|playlists| favorites(&playlists).is_some_and(|favorite| uri_of(&favorite.item) == uri))
  }

  fn seeds_station(&self, kind: VoiceTargetKind) -> bool {
    matches!(kind, VoiceTargetKind::Artist | VoiceTargetKind::Track)
  }

  async fn search(&self, query: &str, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < limit {
      let page = (limit - offset).min(SEARCH_LIMIT_MAX);
      let results = self.search_backend(query.to_owned(), page, offset).await?;
      let full = [
        &results.songs,
        &results.albums,
        &results.artists,
        &results.playlists,
        &results.stations,
      ]
      .iter()
      .any(|bucket| bucket.len() as u32 >= page);
      for item in results
        .top
        .iter()
        .chain(&results.songs)
        .chain(&results.albums)
        .chain(&results.artists)
        .chain(&results.playlists)
        .chain(&results.stations)
      {
        if seen.insert(uri_of(&item.item)) {
          out.push(catalog_item(item));
        }
      }
      if !full {
        break;
      }
      offset += page;
    }
    Ok(out)
  }

  async fn hydrate(&self, uris: &[String]) -> Vec<CatalogItem> {
    let lookups = uris.iter().map(|uri| async move {
      if *uri == library_songs_uri() {
        return CatalogItem {
          uri: uri.clone(),
          name: LIBRARY_SONGS_TITLE.into(),
          ..CatalogItem::default()
        };
      }
      let found = match parse_uri(uri) {
        Some(item) => self.resolve(item).await.ok(),
        None => None,
      };
      found.map(|item| catalog_item(&item)).unwrap_or_else(|| CatalogItem {
        uri: uri.clone(),
        ..CatalogItem::default()
      })
    });
    futures::future::join_all(lookups).await
  }

  async fn container(&self, uri: &str, limit: u32, offset: u32) -> CatalogResult<Vec<CatalogItem>> {
    if uri == library_songs_uri() {
      return Ok(catalog_items(
        &self.page(AmLibraryScope::Songs, limit, offset).await?.items,
      ));
    }
    let container = parse_uri(uri).ok_or_else(|| CatalogError::new(format!("not an apple music uri: {uri}")))?;
    let children = self.children(&container).await?;
    let tracks = songs_of(&children);
    Ok(catalog_items(window(&tracks, limit, offset).0))
  }

  async fn artist_top_tracks(
    &self,
    artist_uri: &str,
    _artist_name: &str,
    limit: u32,
  ) -> CatalogResult<Vec<CatalogItem>> {
    let children = self.artist_children(artist_uri).await?;
    Ok(catalog_items(songs_of(&children).iter().take(limit as usize)))
  }

  async fn artist_releases(&self, artist_uri: &str, albums_only: bool, _depth: usize) -> CatalogResult<Vec<Release>> {
    let children = self.artist_children(artist_uri).await?;
    Ok(
      children
        .iter()
        .filter(|item| item.item.kind == AmKind::Album)
        .filter(|item| !albums_only || full_album(item))
        .filter_map(|item| {
          item.released.map(|date| Release {
            uri: uri_of(&item.item),
            name: item.title.clone(),
            released: (date.year, date.month as i32, date.day as i32),
            popularity: 0,
          })
        })
        .collect(),
    )
  }

  async fn station(&self, seed_uri: &str) -> Option<String> {
    let seed = parse_uri(seed_uri)?;
    let station = self.relations(seed).await.ok()?.station?;
    Some(uri_of(&station))
  }

  async fn now_playing(&self) -> Option<NowPlaying> {
    let song = self.snapshot().await?.entry?.item?;
    let relations = self.relations(song.clone()).await.unwrap_or_default();
    Some(NowPlaying {
      track: uri_of(&song),
      album: relations.album.as_ref().map(uri_of),
      artist: relations.artist.as_ref().map(uri_of),
      context: None,
    })
  }

  async fn current_context(&self) -> Option<String> {
    None
  }

  async fn recently_played(&self, limit: usize) -> Vec<CatalogItem> {
    match self.recents(limit as u32, 0).await {
      Ok(page) => catalog_items(&page.items),
      Err(_) => Vec::new(),
    }
  }

  async fn recent_contexts(&self) -> Vec<String> {
    match self.recents(RECENT_LIMIT, 0).await {
      Ok(page) => page.items.iter().map(|item| uri_of(&item.item)).collect(),
      Err(_) => Vec::new(),
    }
  }

  async fn fresh_pool(&self) -> Vec<String> {
    let (playlists, shelves) = tokio::join!(self.whole_library(AmLibraryScope::Playlists), self.shelves());
    let mut pool: Vec<String> = playlists
      .map(|items| items.iter().map(|item| uri_of(&item.item)).collect())
      .unwrap_or_default();
    if let Ok(shelves) = shelves {
      pool.extend(
        shelves
          .iter()
          .flat_map(|shelf| shelf.items.iter().map(|item| uri_of(&item.item))),
      );
    }
    pool
  }

  async fn liked_songs(&self) -> CatalogResult<CatalogItem> {
    let playlists = self.whole_library(AmLibraryScope::Playlists).await?;
    Ok(match favorites(&playlists) {
      Some(favorite) => catalog_item(favorite),
      None => CatalogItem {
        uri: library_songs_uri(),
        name: LIBRARY_SONGS_TITLE.into(),
        ..CatalogItem::default()
      },
    })
  }

  async fn saved_albums(&self, limit: Option<usize>) -> CatalogResult<Vec<CatalogItem>> {
    match limit {
      Some(limit) => Ok(catalog_items(
        &self.page(AmLibraryScope::Albums, limit as u32, 0).await?.items,
      )),
      None => self.library_items(AmLibraryScope::Albums).await,
    }
  }

  async fn followed_artists(&self) -> CatalogResult<Vec<CatalogItem>> {
    self.library_items(AmLibraryScope::Artists).await
  }

  async fn library_playlists(&self) -> CatalogResult<Vec<LibraryPlaylist>> {
    let playlists = self.whole_library(AmLibraryScope::Playlists).await?;
    Ok(
      playlists
        .iter()
        .map(|playlist| LibraryPlaylist {
          uri: uri_of(&playlist.item),
          name: playlist.title.clone(),
          owned: playlist.subtitle.is_none(),
        })
        .collect(),
    )
  }

  async fn personal_mix(&self, label: &str) -> Option<Vec<CatalogItem>> {
    let want = format!("{} mix", norm(label));
    let shelves = self.shelves().await.ok()?;
    let mixes: Vec<CatalogItem> = shelves
      .iter()
      .flat_map(|shelf| shelf.items.iter())
      .filter(|item| item.item.kind == AmKind::Playlist && norm(&item.title) == want)
      .map(catalog_item)
      .collect();
    (!mixes.is_empty()).then_some(mixes)
  }
}
