use voice_resolver::{
  Catalog, CatalogError, CatalogItem, CatalogResult, LibraryPlaylist, NowPlaying, Release, VoiceTargetKind,
};

use super::{
  Core, Kind, STARRED_ID, STARRED_TITLE,
  client::{Album, AlbumList, Artist, Playlist, SearchCounts, Song, SubsonicError},
  page, parse_uri, station_id, uri,
};

const RECENT_LIMIT: u32 = 40;
const FRESH_ALBUMS: u32 = 20;

impl From<SubsonicError> for CatalogError {
  fn from(error: SubsonicError) -> Self {
    CatalogError::new(error)
  }
}

fn song_item(song: &Song) -> CatalogItem {
  CatalogItem {
    uri: uri(Kind::Track, &song.id),
    name: song.title.clone(),
    artist: song.artist.clone(),
    year: song.year,
  }
}

fn album_item(album: &Album) -> CatalogItem {
  CatalogItem {
    uri: uri(Kind::Album, &album.id),
    name: album.name.clone(),
    artist: album.artist.clone(),
    year: album.release_date.and_then(|date| date.year).or(album.year),
  }
}

fn artist_item(artist: &Artist) -> CatalogItem {
  CatalogItem {
    uri: uri(Kind::Artist, &artist.id),
    name: artist.name.clone(),
    ..CatalogItem::default()
  }
}

fn playlist_item(playlist: &Playlist) -> CatalogItem {
  CatalogItem {
    uri: uri(Kind::Playlist, &playlist.id),
    name: playlist.name.clone(),
    ..CatalogItem::default()
  }
}

fn starred_item() -> CatalogItem {
  CatalogItem {
    uri: uri(Kind::Starred, STARRED_ID),
    name: STARRED_TITLE.into(),
    ..CatalogItem::default()
  }
}

fn full_album(album: &Album) -> bool {
  album.release_types.is_empty()
    || album
      .release_types
      .iter()
      .any(|kind| kind.eq_ignore_ascii_case("album"))
}

fn released(album: &Album) -> Option<(i32, i32, i32)> {
  let date = album.release_date;
  let year = date.and_then(|date| date.year).or(album.year)?;
  Some((
    year,
    date.and_then(|date| date.month).unwrap_or(0),
    date.and_then(|date| date.day).unwrap_or(0),
  ))
}

impl Core {
  async fn item(&self, at: &str) -> Option<CatalogItem> {
    let (kind, id) = parse_uri(at)?;
    let found = match kind {
      Kind::Track => song_item(&self.client.song(id).await.ok()?),
      Kind::Album => album_item(&self.client.album(id).await.ok()?.album),
      Kind::Artist => artist_item(&self.client.artist(id).await.ok()?.artist),
      Kind::Playlist => playlist_item(&self.client.playlist(id).await.ok()?.playlist),
      Kind::Starred => starred_item(),
      Kind::Station => CatalogItem {
        uri: at.to_owned(),
        name: self.seed_name(id).await.ok()?.0,
        ..CatalogItem::default()
      },
    };
    Some(found)
  }

  async fn recent_albums(&self, limit: u32) -> Vec<Album> {
    self
      .client
      .album_list(AlbumList::RecentlyPlayed, limit, 0)
      .await
      .unwrap_or_default()
  }
}

#[async_trait::async_trait]
impl Catalog for Core {
  fn kind_of(&self, at: &str) -> Option<VoiceTargetKind> {
    let (kind, _) = parse_uri(at)?;
    Some(match kind {
      Kind::Track => VoiceTargetKind::Track,
      Kind::Album => VoiceTargetKind::Album,
      Kind::Artist => VoiceTargetKind::Artist,
      Kind::Playlist | Kind::Starred => VoiceTargetKind::Playlist,
      Kind::Station => VoiceTargetKind::Station,
    })
  }

  fn is_liked_songs(&self, at: &str) -> bool {
    matches!(parse_uri(at), Some((Kind::Starred, _)))
  }

  fn seeds_station(&self, kind: VoiceTargetKind) -> bool {
    matches!(kind, VoiceTargetKind::Artist | VoiceTargetKind::Track)
  }

  async fn search(&self, query: &str, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    let counts = SearchCounts {
      artists: limit,
      albums: limit,
      songs: limit,
    };
    let (hits, playlists) = tokio::join!(self.client.search(query, counts, 0), self.matching_playlists(query));
    let mut hits = hits?;
    let spoken = query.to_lowercase();
    if hits.artists.is_empty()
      && hits.albums.is_empty()
      && hits.songs.is_empty()
      && let Some((work, _)) = spoken.rsplit_once(" by ")
    {
      hits = self.client.search(work, counts, 0).await?;
    }
    Ok(
      hits
        .artists
        .iter()
        .map(artist_item)
        .chain(hits.albums.iter().map(album_item))
        .chain(hits.songs.iter().map(song_item))
        .chain(
          playlists
            .unwrap_or_default()
            .iter()
            .take(limit as usize)
            .map(playlist_item),
        )
        .collect(),
    )
  }

  async fn hydrate(&self, uris: &[String]) -> Vec<CatalogItem> {
    let lookups = uris.iter().map(|at| async move {
      self.item(at).await.unwrap_or_else(|| CatalogItem {
        uri: at.clone(),
        ..CatalogItem::default()
      })
    });
    futures::future::join_all(lookups).await
  }

  async fn container(&self, at: &str, limit: u32, offset: u32) -> CatalogResult<Vec<CatalogItem>> {
    let (kind, id) = parse_uri(at).ok_or_else(|| CatalogError::new(format!("{at} is not a subsonic uri")))?;
    let (songs, _) = self.songs_of(kind, id).await?;
    Ok(page(&songs, limit, offset).0.iter().map(song_item).collect())
  }

  async fn artist_top_tracks(
    &self,
    _artist_uri: &str,
    artist_name: &str,
    limit: u32,
  ) -> CatalogResult<Vec<CatalogItem>> {
    Ok(
      self
        .client
        .top_songs(artist_name, limit)
        .await?
        .iter()
        .map(song_item)
        .collect(),
    )
  }

  async fn artist_releases(&self, artist_uri: &str, albums_only: bool, _depth: usize) -> CatalogResult<Vec<Release>> {
    let Some((Kind::Artist, id)) = parse_uri(artist_uri) else {
      return Err(CatalogError::new(format!("{artist_uri} is not a subsonic artist")));
    };
    let artist = self.client.artist(id).await?;
    Ok(
      artist
        .albums
        .iter()
        .filter(|album| !albums_only || full_album(album))
        .filter_map(|album| {
          released(album).map(|released| Release {
            uri: uri(Kind::Album, &album.id),
            name: album.name.clone(),
            released,
            popularity: 0,
          })
        })
        .collect(),
    )
  }

  async fn station(&self, seed_uri: &str) -> Option<String> {
    let (seed, id) = parse_uri(seed_uri)?;
    station_id(seed, id).map(|station| uri(Kind::Station, &station))
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
    self.recent_albums(limit as u32).await.iter().map(album_item).collect()
  }

  async fn recent_contexts(&self) -> Vec<String> {
    self
      .recent_albums(RECENT_LIMIT)
      .await
      .iter()
      .map(|album| uri(Kind::Album, &album.id))
      .collect()
  }

  async fn fresh_pool(&self) -> Vec<String> {
    let (playlists, albums) = tokio::join!(
      self.client.playlists(),
      self.client.album_list(AlbumList::Random, FRESH_ALBUMS, 0)
    );
    playlists
      .unwrap_or_default()
      .iter()
      .map(|playlist| uri(Kind::Playlist, &playlist.id))
      .chain(
        albums
          .unwrap_or_default()
          .iter()
          .map(|album| uri(Kind::Album, &album.id)),
      )
      .collect()
  }

  async fn liked_songs(&self) -> CatalogResult<CatalogItem> {
    Ok(starred_item())
  }

  async fn saved_albums(&self, limit: Option<usize>) -> CatalogResult<Vec<CatalogItem>> {
    let mut albums = self.client.starred().await?.albums;
    albums.sort_by(|a, b| b.starred.cmp(&a.starred));
    Ok(
      albums
        .iter()
        .take(limit.unwrap_or(usize::MAX))
        .map(album_item)
        .collect(),
    )
  }

  async fn followed_artists(&self) -> CatalogResult<Vec<CatalogItem>> {
    Ok(self.client.starred().await?.artists.iter().map(artist_item).collect())
  }

  async fn library_playlists(&self) -> CatalogResult<Vec<LibraryPlaylist>> {
    let me = self.client.username();
    Ok(
      self
        .client
        .playlists()
        .await?
        .into_iter()
        .map(|playlist| LibraryPlaylist {
          uri: uri(Kind::Playlist, &playlist.id),
          owned: playlist.owner.as_deref() == Some(me),
          name: playlist.name,
        })
        .collect(),
    )
  }

  async fn charts(&self, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    Ok(
      self
        .client
        .album_list(AlbumList::Frequent, limit, 0)
        .await?
        .iter()
        .map(album_item)
        .collect(),
    )
  }

  async fn new_releases(&self, query: &str, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    if !query.is_empty() {
      return Ok(Vec::new());
    }
    Ok(
      self
        .client
        .album_list(AlbumList::Newest, limit, 0)
        .await?
        .iter()
        .map(album_item)
        .collect(),
    )
  }
}
