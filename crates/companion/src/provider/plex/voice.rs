use voice_resolver::{
  Catalog, CatalogError, CatalogItem, CatalogResult, LibraryPlaylist, NowPlaying, Release, VoiceTargetKind,
};

use super::{
  Core, Found, Indexed, Kind, LIKED_ID, LIKED_TITLE, Order, Seed,
  client::{ItemType, Metadata, PlexError},
  kind_of_item, parse_uri, uri,
};
use crate::provider::library::page;

const RECENT_LIMIT: u32 = 40;
const FRESH_ALBUMS: u32 = 20;

impl From<PlexError> for CatalogError {
  fn from(error: PlexError) -> Self {
    CatalogError::new(error)
  }
}

fn year(item: &Metadata) -> Option<i32> {
  item.year.or_else(|| {
    item
      .originally_available_at
      .as_deref()
      .and_then(|date| date.get(..4))
      .and_then(|year| year.parse().ok())
  })
}

fn released(item: &Metadata) -> Option<(i32, i32, i32)> {
  let mut parts = item
    .originally_available_at
    .as_deref()
    .unwrap_or_default()
    .split('-')
    .map(|part| part.parse::<i32>().ok());
  let year = parts.next().flatten().or(item.year)?;
  Some((
    year,
    parts.next().flatten().unwrap_or(0),
    parts.next().flatten().unwrap_or(0),
  ))
}

fn full_album(album: &Metadata) -> bool {
  let tags = album.format.iter().chain(&album.subformat);
  album.format.is_empty() && album.subformat.is_empty()
    || tags.into_iter().any(|tag| tag.tag.eq_ignore_ascii_case("album"))
}

fn catalog_item((server, item): &Found) -> Option<CatalogItem> {
  let kind = kind_of_item(item)?;
  let artist = match kind {
    Kind::Track => item.artist_name().map(str::to_owned),
    Kind::Album => item.parent_title.clone(),
    _ => None,
  };
  Some(CatalogItem {
    uri: uri(kind, &server.machine, &item.rating_key),
    name: item.title.clone(),
    artist,
    year: match kind {
      Kind::Track => item.year,
      Kind::Album => year(item),
      _ => None,
    },
  })
}

fn catalog_items(found: &[Found]) -> Vec<CatalogItem> {
  found.iter().filter_map(catalog_item).collect()
}

fn liked_item() -> CatalogItem {
  CatalogItem {
    uri: uri(Kind::Liked, "", LIKED_ID),
    name: LIKED_TITLE.into(),
    ..CatalogItem::default()
  }
}

impl Core {
  async fn hydrated(&self, at: &str) -> Option<CatalogItem> {
    let target = parse_uri(at)?;
    match target.kind {
      Kind::Liked => Some(liked_item()),
      Kind::Station(_) => Some(CatalogItem {
        uri: at.to_owned(),
        name: self.seed_name(&target).await.ok()?.0,
        ..CatalogItem::default()
      }),
      _ => {
        let server = self.server(&target.server).ok()?;
        let item = server.metadata(&target.key).await.ok()?;
        catalog_item(&(server, item))
      }
    }
  }

  async fn albums(&self, order: Order, limit: u32) -> CatalogResult<Vec<Found>> {
    Ok(self.indexed(Indexed::Albums, order, 0, limit).await?.items)
  }
}

#[async_trait::async_trait]
impl Catalog for Core {
  fn kind_of(&self, at: &str) -> Option<VoiceTargetKind> {
    Some(match parse_uri(at)?.kind {
      Kind::Track => VoiceTargetKind::Track,
      Kind::Album => VoiceTargetKind::Album,
      Kind::Artist => VoiceTargetKind::Artist,
      Kind::Playlist | Kind::Liked => VoiceTargetKind::Playlist,
      Kind::Station(_) => VoiceTargetKind::Station,
    })
  }

  fn is_liked_songs(&self, at: &str) -> bool {
    parse_uri(at).is_some_and(|target| target.kind == Kind::Liked)
  }

  fn seeds_station(&self, kind: VoiceTargetKind) -> bool {
    matches!(kind, VoiceTargetKind::Artist | VoiceTargetKind::Track)
  }

  async fn search(&self, query: &str, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    let spoken = query.to_lowercase();
    let mut texts = vec![query.to_owned()];
    if let Some((work, _)) = spoken.rsplit_once(" by ") {
      texts.push(work.to_owned());
    }
    let mut found = Vec::new();
    for text in &texts {
      for kind in [ItemType::Artist, ItemType::Album, ItemType::Track] {
        found.extend(self.searched(kind, text, 0, limit).await?.items);
      }
      if !found.is_empty() {
        break;
      }
    }
    let playlists = self.matching_playlists(query).await.unwrap_or_default();
    found.extend(playlists.into_iter().take(limit as usize));
    Ok(catalog_items(&found))
  }

  async fn hydrate(&self, uris: &[String]) -> Vec<CatalogItem> {
    let lookups = uris.iter().map(|at| async move {
      self.hydrated(at).await.unwrap_or_else(|| CatalogItem {
        uri: at.clone(),
        ..CatalogItem::default()
      })
    });
    futures::future::join_all(lookups).await
  }

  async fn container(&self, at: &str, limit: u32, offset: u32) -> CatalogResult<Vec<CatalogItem>> {
    let target = parse_uri(at).ok_or_else(|| CatalogError::new(format!("{at} is not a plex uri")))?;
    let (tracks, _) = self.tracks_of(&target).await?;
    Ok(catalog_items(&page(&tracks, limit, offset).0))
  }

  async fn artist_top_tracks(
    &self,
    artist_uri: &str,
    _artist_name: &str,
    limit: u32,
  ) -> CatalogResult<Vec<CatalogItem>> {
    let target = parse_uri(artist_uri)
      .filter(|target| target.kind == Kind::Artist)
      .ok_or_else(|| CatalogError::new(format!("{artist_uri} is not a plex artist")))?;
    let server = self.server(&target.server)?;
    Ok(catalog_items(&self.popular(&server, &target.key, limit).await?))
  }

  async fn artist_releases(&self, artist_uri: &str, albums_only: bool, _depth: usize) -> CatalogResult<Vec<Release>> {
    let target = parse_uri(artist_uri)
      .filter(|target| target.kind == Kind::Artist)
      .ok_or_else(|| CatalogError::new(format!("{artist_uri} is not a plex artist")))?;
    let server = self.server(&target.server)?;
    let albums = server.children(&target.key).await?;
    Ok(
      albums
        .iter()
        .filter(|album| album.kind == "album" && (!albums_only || full_album(album)))
        .filter_map(|album| {
          released(album).map(|released| Release {
            uri: uri(Kind::Album, &server.machine, &album.rating_key),
            name: album.title.clone(),
            released,
            popularity: 0,
          })
        })
        .collect(),
    )
  }

  async fn station(&self, seed_uri: &str) -> Option<String> {
    let target = parse_uri(seed_uri)?;
    let seed = match target.kind {
      Kind::Artist => Seed::Artist,
      Kind::Track => Seed::Track,
      _ => return None,
    };
    Some(uri(Kind::Station(seed), &target.server, &target.key))
  }

  async fn now_playing(&self) -> Option<NowPlaying> {
    let (playing, context) = self.playback.current()?;
    Some(NowPlaying {
      track: playing.uri,
      album: playing.album_uri,
      artist: playing.artist_uri,
      context: context.map(|context| context.uri),
    })
  }

  async fn current_context(&self) -> Option<String> {
    self.playback.current()?.1.map(|context| context.uri)
  }

  async fn recently_played(&self, limit: usize) -> Vec<CatalogItem> {
    catalog_items(&self.albums(Order::Viewed, limit as u32).await.unwrap_or_default())
  }

  async fn recent_contexts(&self) -> Vec<String> {
    self
      .albums(Order::Viewed, RECENT_LIMIT)
      .await
      .unwrap_or_default()
      .iter()
      .map(|(server, album)| uri(Kind::Album, &server.machine, &album.rating_key))
      .collect()
  }

  async fn fresh_pool(&self) -> Vec<String> {
    let (playlists, albums) = tokio::join!(self.playlists(), self.random(ItemType::Album, FRESH_ALBUMS));
    playlists
      .unwrap_or_default()
      .iter()
      .chain(albums.unwrap_or_default().iter())
      .filter_map(|(server, item)| kind_of_item(item).map(|kind| uri(kind, &server.machine, &item.rating_key)))
      .collect()
  }

  async fn liked_songs(&self) -> CatalogResult<CatalogItem> {
    Ok(liked_item())
  }

  async fn saved_albums(&self, limit: Option<usize>) -> CatalogResult<Vec<CatalogItem>> {
    let mut albums = catalog_items(&self.liked(ItemType::Album).await);
    albums.truncate(limit.unwrap_or(usize::MAX));
    Ok(albums)
  }

  async fn followed_artists(&self) -> CatalogResult<Vec<CatalogItem>> {
    Ok(catalog_items(&self.liked(ItemType::Artist).await))
  }

  async fn library_playlists(&self) -> CatalogResult<Vec<LibraryPlaylist>> {
    Ok(
      self
        .playlists()
        .await?
        .into_iter()
        .map(|(server, playlist)| LibraryPlaylist {
          uri: uri(Kind::Playlist, &server.machine, &playlist.rating_key),
          owned: true,
          name: playlist.title,
        })
        .collect(),
    )
  }

  async fn charts(&self, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    Ok(catalog_items(&self.albums(Order::Plays, limit).await?))
  }

  async fn new_releases(&self, query: &str, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    if !query.is_empty() {
      return Ok(Vec::new());
    }
    Ok(catalog_items(&self.albums(Order::Added, limit).await?))
  }
}
