use std::collections::{HashMap, HashSet};

use voice_resolver::{
  Catalog, CatalogError, CatalogItem, CatalogResult, LibraryPlaylist, NowPlaying, Release, VoiceTargetKind, norm,
};

use super::{FlatItem, SpotifyClient, some_uri};
use crate::{error::Error, model, model::BrowseItem};

const NEW_RELEASES_TAG: &str = "tag:new";
const MIX_SEARCH_LIMIT: u32 = 20;
const MADE_FOR_YOU_PREFIX: &str = "spotify:playlist:37i9dQZF1E";

pub fn made_for_you(uri: &str) -> bool {
  uri.starts_with(MADE_FOR_YOU_PREFIX)
}

pub fn kind_of_uri(uri: &str) -> Option<VoiceTargetKind> {
  match uri.split(':').nth(1)? {
    "track" => Some(VoiceTargetKind::Track),
    "album" => Some(VoiceTargetKind::Album),
    "artist" => Some(VoiceTargetKind::Artist),
    "playlist" => Some(VoiceTargetKind::Playlist),
    "show" => Some(VoiceTargetKind::Show),
    "episode" => Some(VoiceTargetKind::Episode),
    "station" => Some(VoiceTargetKind::Station),
    "user" if is_liked_songs(uri) => Some(VoiceTargetKind::Playlist),
    _ => None,
  }
}

fn is_liked_songs(uri: &str) -> bool {
  uri.starts_with("spotify:user:") && uri.ends_with(":collection")
}

fn station_uri(seed: &str) -> String {
  seed.replacen("spotify:", "spotify:station:", 1)
}

fn tagged(query: &str, tag: &str) -> String {
  match query.is_empty() {
    true => tag.to_string(),
    false => format!("{query} {tag}"),
  }
}

impl From<Error> for CatalogError {
  fn from(error: Error) -> Self {
    CatalogError::new(error)
  }
}

fn found(items: Vec<FlatItem>) -> Vec<CatalogItem> {
  items
    .into_iter()
    .map(|i| CatalogItem {
      uri: i.uri,
      name: i.name,
      artist: i.artist,
      year: i.year,
    })
    .collect()
}

fn named(items: Vec<BrowseItem>) -> Vec<CatalogItem> {
  items
    .into_iter()
    .map(|i| CatalogItem {
      artist: i.artists.first().map(|a| a.name.clone()),
      uri: i.uri,
      name: i.title,
      year: None,
    })
    .collect()
}

#[async_trait::async_trait]
impl Catalog for SpotifyClient {
  fn kind_of(&self, uri: &str) -> Option<VoiceTargetKind> {
    kind_of_uri(uri)
  }

  fn is_liked_songs(&self, uri: &str) -> bool {
    is_liked_songs(uri)
  }

  fn seeds_station(&self, kind: VoiceTargetKind) -> bool {
    matches!(
      kind,
      VoiceTargetKind::Artist | VoiceTargetKind::Track | VoiceTargetKind::Album | VoiceTargetKind::Playlist
    )
  }

  async fn search(&self, query: &str, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    Ok(found(self.search_flat(query, limit).await?))
  }

  async fn hydrate(&self, uris: &[String]) -> Vec<CatalogItem> {
    named(self.hydrate_uris(uris).await)
  }

  async fn container(&self, uri: &str, limit: u32, offset: u32) -> CatalogResult<Vec<CatalogItem>> {
    Ok(named(self.browse_container(uri, limit, offset).await?.items))
  }

  async fn artist_top_tracks(
    &self,
    artist_uri: &str,
    _artist_name: &str,
    limit: u32,
  ) -> CatalogResult<Vec<CatalogItem>> {
    Ok(named(self.browse_container(artist_uri, limit, 0).await?.items))
  }

  async fn artist_releases(&self, artist_uri: &str, albums_only: bool, depth: usize) -> CatalogResult<Vec<Release>> {
    let owned = artist_uri.to_string();
    let artists = self.spc.get_artists(std::slice::from_ref(&owned)).await?;
    let Some(artist) = artists.get(artist_uri) else {
      return Ok(Vec::new());
    };
    let uris = model::artist_release_uris(artist, albums_only, depth);
    if uris.is_empty() {
      return Ok(Vec::new());
    }
    let albums = self.spc.get_albums(&uris).await?;
    Ok(
      uris
        .iter()
        .filter_map(|u| {
          albums.get(u).map(|a| Release {
            uri: u.clone(),
            name: a.name().to_string(),
            released: (a.date.year(), a.date.month(), a.date.day()),
            popularity: a.popularity(),
          })
        })
        .collect(),
    )
  }

  async fn station(&self, seed_uri: &str) -> Option<String> {
    Some(station_uri(seed_uri))
  }

  async fn now_playing(&self) -> Option<NowPlaying> {
    let guard = self.shared.cluster.lock().await;
    let state = &guard.as_ref()?.player_state;
    let track = &state.track;
    let held = |direct: &str, key: &str| some_uri(direct).or_else(|| track.metadata.get(key).and_then(|u| some_uri(u)));
    Some(NowPlaying {
      track: some_uri(&track.uri)?,
      album: held(&track.album_uri, "album_uri"),
      artist: held(&track.artist_uri, "artist_uri"),
      context: some_uri(&state.context_uri),
    })
  }

  async fn current_context(&self) -> Option<String> {
    self.current_context_uri().await
  }

  async fn recently_played(&self, limit: usize) -> Vec<CatalogItem> {
    let Ok(recents) = self.recents().await else {
      return Vec::new();
    };
    let mut seen = HashSet::new();
    let pool: Vec<String> = recents
      .contexts
      .into_iter()
      .chain(recents.tracks)
      .filter(|u| kind_of_uri(u).is_some())
      .filter(|u| seen.insert(u.clone()))
      .take(limit)
      .collect();
    if pool.is_empty() {
      return Vec::new();
    }
    named(self.hydrate_uris(&pool).await)
  }

  async fn recent_contexts(&self) -> Vec<String> {
    self.recents().await.map(|r| r.contexts).unwrap_or_default()
  }

  async fn fresh_pool(&self) -> Vec<String> {
    let (playlists, home) = tokio::join!(self.playlist_uris(), self.spc.get_home("en"));
    let mut pool = playlists.unwrap_or_default();
    if let Ok(home) = home {
      pool.extend(home.body.sections.iter().flat_map(|s| super::carousel_of(s).1));
    }
    pool
  }

  async fn liked_songs(&self) -> CatalogResult<CatalogItem> {
    let uri = self.liked_songs_uri().await?;
    named(self.hydrate_uris(std::slice::from_ref(&uri)).await)
      .into_iter()
      .next()
      .ok_or_else(|| CatalogError::new("the liked songs collection did not hydrate"))
  }

  async fn saved_albums(&self, limit: Option<usize>) -> CatalogResult<Vec<CatalogItem>> {
    let uris: Vec<String> = self
      .saved_album_uris()
      .await?
      .into_iter()
      .take(limit.unwrap_or(usize::MAX))
      .collect();
    Ok(named(self.hydrate_uris(&uris).await))
  }

  async fn followed_artists(&self) -> CatalogResult<Vec<CatalogItem>> {
    let uris = self.saved_artist_uris().await?;
    Ok(named(self.hydrate_uris(&uris).await))
  }

  async fn library_playlists(&self) -> CatalogResult<Vec<LibraryPlaylist>> {
    Ok(self.rootlist_playlists().await?)
  }

  async fn popularity(&self, uris: &[String]) -> HashMap<String, i32> {
    let mut by_kind: HashMap<&str, Vec<String>> = HashMap::new();
    for u in uris {
      if let Some(kind) = u.split(':').nth(1) {
        by_kind.entry(kind).or_default().push(u.clone());
      }
    }
    let empty: Vec<String> = Vec::new();
    let ids = |k: &str| by_kind.get(k).unwrap_or(&empty).clone();
    let (track_ids, album_ids, artist_ids) = (ids("track"), ids("album"), ids("artist"));
    let (tracks, albums, artists) = tokio::join!(
      self.spc.get_tracks(&track_ids),
      self.spc.get_albums(&album_ids),
      self.spc.get_artists(&artist_ids),
    );
    let mut out = HashMap::new();
    for (uri, t) in tracks.unwrap_or_default() {
      out.insert(uri, t.popularity());
    }
    for (uri, a) in albums.unwrap_or_default() {
      out.insert(uri, a.popularity());
    }
    for (uri, a) in artists.unwrap_or_default() {
      out.insert(uri, a.popularity());
    }
    out
  }

  async fn new_releases(&self, query: &str, limit: u32) -> CatalogResult<Vec<CatalogItem>> {
    Ok(found(self.search_flat(&tagged(query, NEW_RELEASES_TAG), limit).await?))
  }

  async fn personal_mix(&self, label: &str) -> Option<Vec<CatalogItem>> {
    let want = format!("{} mix", norm(label));
    let items = self.search_flat(&format!("{label} mix"), MIX_SEARCH_LIMIT).await.ok()?;
    let mut playlists: Vec<CatalogItem> = found(items)
      .into_iter()
      .filter(|i| kind_of_uri(&i.uri) == Some(VoiceTargetKind::Playlist))
      .collect();
    let mine = playlists
      .iter()
      .position(|i| made_for_you(&i.uri) && norm(&i.name) == want)?;
    playlists[..=mine].rotate_right(1);
    Some(playlists)
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use voice_resolver::{CHART_QUERY, VoicePopularity, VoiceResolveError, VoiceResolveRequest, VoiceResolved, resolve};

  use super::*;
  use crate::client::tests::{
    LibrarySeed, NullObserver, Playing, album_hit, library_client, named_hit, playing_client, search_item,
    searching_client, searching_client_items, test_client, track_hit,
  };

  fn req(target: Option<&str>, target_type: Option<VoiceTargetKind>) -> VoiceResolveRequest {
    VoiceResolveRequest {
      target: target.map(str::to_string),
      target_type,
      ..Default::default()
    }
  }

  fn filtered(filter: VoicePopularity) -> VoiceResolveRequest {
    VoiceResolveRequest {
      popularity_filter: Some(filter),
      ..Default::default()
    }
  }

  fn picked(out: &VoiceResolved) -> Vec<&str> {
    std::iter::once(out.uri.as_str())
      .chain(out.alternatives.iter().map(|a| a.uri.as_str()))
      .collect()
  }

  const LIKED: &str = "spotify:user:user-1:collection";

  fn stranger_liked_songs() -> Vec<crate::proto::custom::searchview::SearchItem> {
    vec![named_hit("spotify:playlist:stranger", "Liked Songs")]
  }

  fn shelf() -> LibrarySeed<'static> {
    LibrarySeed {
      playlists: &[
        ("spotify:playlist:followed", "Road Trip", false),
        ("spotify:playlist:mine", "Road Trip", true),
        ("spotify:playlist:other", "Gym", true),
      ],
      saved: &[
        ("spotify:track:t9", "Newest Song", "A"),
        ("spotify:album:vessel", "Vessel", "Twenty One Pilots"),
        ("spotify:track:t8", "Older Song", "B"),
        ("spotify:album:trench", "Trench", "Twenty One Pilots"),
      ],
      artists: &[("spotify:artist:bastille-mine", "Bastille")],
    }
  }

  #[tokio::test]
  async fn liked_songs_phrases_resolve_to_the_users_collection_before_any_search() {
    let (client, log) = library_client(stranger_liked_songs(), shelf()).await;
    for target in [
      "liked songs",
      "Liked Songs",
      "my liked songs",
      "all my liked songs",
      "liked songs playlist",
      "my likes",
      "my favorites",
      "my favourites",
      "favorite songs",
      "my favorite songs",
      "saved songs",
      "my saved tracks",
      "my library",
      "my library's songs",
      "my music",
      "songs i've liked",
    ] {
      for kind in [None, Some(VoiceTargetKind::Playlist), Some(VoiceTargetKind::Track)] {
        let out = resolve(&client, req(Some(target), kind)).await.unwrap();
        assert_eq!(out.uri, LIKED, "{target:?} as {kind:?}");
        assert_eq!(out.kind, VoiceTargetKind::Playlist);
        assert_eq!(out.display, "Liked Songs");
      }
    }
    assert!(
      log.queries().is_empty(),
      "the catalog is never asked, so a stranger's \"Liked Songs\" cannot win"
    );
  }

  #[tokio::test]
  async fn the_bare_words_need_my_to_name_the_collection() {
    let (client, _log) = library_client(vec![named_hit("spotify:playlist:p", "Favorites")], shelf()).await;
    let out = resolve(&client, req(Some("favorites"), None)).await.unwrap();
    assert_eq!(
      out.uri, "spotify:playlist:p",
      "\"favorites\" alone is a title, not the collection"
    );
  }

  #[tokio::test]
  async fn a_station_or_an_album_kind_never_reads_liked_songs() {
    let (client, _log) = library_client(vec![album_hit("spotify:album:ls", "Liked Songs", "X", 2020)], shelf()).await;
    let out = resolve(&client, req(Some("liked songs"), Some(VoiceTargetKind::Album)))
      .await
      .unwrap();
    assert_eq!(out.uri, "spotify:album:ls");
  }

  #[tokio::test]
  async fn a_position_counts_into_liked_songs_newest_first() {
    let (client, _log) = library_client(Vec::new(), shelf()).await;
    let out = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("my liked songs".into()),
        position: Some(2),
        ..Default::default()
      },
    )
    .await
    .unwrap();
    assert_eq!(out.uri, "spotify:track:t8");
    assert_eq!(out.context_uri.as_deref(), Some(LIKED));
  }

  #[tokio::test]
  async fn saved_albums_play_newest_first() {
    let (client, log) = library_client(stranger_liked_songs(), shelf()).await;
    for target in ["my albums", "saved albums", "my saved albums"] {
      for kind in [None, Some(VoiceTargetKind::Album)] {
        let out = resolve(&client, req(Some(target), kind)).await.unwrap();
        assert_eq!(
          picked(&out),
          ["spotify:album:vessel", "spotify:album:trench"],
          "{target:?}"
        );
        assert_eq!(out.kind, VoiceTargetKind::Album);
      }
    }
    assert!(log.queries().is_empty());
  }

  #[tokio::test]
  async fn the_users_own_playlist_wins_by_name_over_the_catalog() {
    let (client, _log) = library_client(vec![named_hit("spotify:playlist:public", "Road Trip")], shelf()).await;
    for target in ["road trip", "my road trip playlist", "Road Trip"] {
      for kind in [None, Some(VoiceTargetKind::Playlist)] {
        let out = resolve(&client, req(Some(target), kind)).await.unwrap();
        assert_eq!(
          picked(&out),
          [
            "spotify:playlist:mine",
            "spotify:playlist:followed",
            "spotify:playlist:public"
          ],
          "{target:?}: owned, then followed, then the catalog"
        );
      }
    }
  }

  #[tokio::test]
  async fn a_saved_album_wins_by_title_or_by_phrase() {
    let catalog = vec![album_hit("spotify:album:cover", "Vessel", "Someone Else", 2024)];
    let (client, _log) = library_client(catalog, shelf()).await;
    for target in ["vessel", "vessel by twenty one pilots", "my vessel album"] {
      let out = resolve(&client, req(Some(target), Some(VoiceTargetKind::Album)))
        .await
        .unwrap();
      assert_eq!(out.uri, "spotify:album:vessel", "{target:?}");
    }
    let untyped = resolve(&client, req(Some("vessel"), None)).await.unwrap();
    assert_eq!(untyped.uri, "spotify:album:vessel");
  }

  #[tokio::test]
  async fn a_followed_artist_wins_by_name() {
    let catalog = vec![named_hit("spotify:artist:bastille-catalog", "Bastille")];
    let (client, _log) = library_client(catalog, shelf()).await;
    let out = resolve(&client, req(Some("bastille"), Some(VoiceTargetKind::Artist)))
      .await
      .unwrap();
    assert_eq!(out.uri, "spotify:artist:bastille-mine");
  }

  #[tokio::test]
  async fn the_library_only_answers_a_plain_name_of_a_kind_it_holds() {
    let catalog = vec![
      track_hit("spotify:track:vessel-song", "Vessel", "Band"),
      album_hit("spotify:album:vessel-2013", "Vessel", "Twenty One Pilots", 2013),
    ];
    let (client, _log) = library_client(catalog, shelf()).await;
    let track = resolve(&client, req(Some("vessel"), Some(VoiceTargetKind::Track)))
      .await
      .unwrap();
    assert_eq!(
      track.uri, "spotify:track:vessel-song",
      "the library holds no tracks by name"
    );
    let qualified = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("vessel".into()),
        target_type: Some(VoiceTargetKind::Album),
        era: Some("2013".into()),
        ..Default::default()
      },
    )
    .await
    .unwrap();
    assert_eq!(
      qualified.uri, "spotify:album:vessel-2013",
      "a qualified request belongs to the catalog"
    );
  }

  #[tokio::test]
  async fn a_position_counts_into_the_users_own_playlist() {
    let (client, log) = library_client(vec![named_hit("spotify:playlist:public", "Road Trip")], shelf()).await;
    let _ = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("road trip".into()),
        position: Some(3),
        ..Default::default()
      },
    )
    .await;
    assert!(log.fetched("/playlist/v2/playlist/mine"));
    assert!(!log.fetched("/playlist/v2/playlist/public"));
  }

  #[tokio::test]
  async fn a_station_with_nothing_to_seed_from_matches_nothing() {
    let client = test_client(Arc::new(NullObserver));
    let err = resolve(&client, req(None, Some(VoiceTargetKind::Station)))
      .await
      .unwrap_err();
    assert!(
      matches!(err, VoiceResolveError::NoAnchorContext),
      "a station is never synthesized from thin air; with nothing on there is nothing to seed it: {err:?}"
    );
  }

  #[tokio::test]
  async fn an_empty_request_matches_nothing_rather_than_resuming() {
    let client = test_client(Arc::new(NullObserver));
    let err = resolve(&client, VoiceResolveRequest::default()).await.unwrap_err();
    assert!(
      matches!(err, VoiceResolveError::NoMatch),
      "no slots and no random filter is not a resume: {err:?}"
    );
  }

  #[test]
  fn the_new_release_tag_rides_the_composed_query_and_stands_alone_without_one() {
    assert_eq!(tagged("80s rock", NEW_RELEASES_TAG), "80s rock tag:new");
    assert_eq!(
      tagged("", NEW_RELEASES_TAG),
      "tag:new",
      "with nothing named the tag is the whole query"
    );
  }

  #[tokio::test]
  async fn a_first_request_with_no_discography_degrades_to_the_plain_search() {
    let (client, _log) = searching_client(Arc::new(NullObserver), &["spotify:album:a1"]);
    let out = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("some band".into()),
        target_type: Some(VoiceTargetKind::Album),
        popularity_filter: Some(VoicePopularity::First),
        ..Default::default()
      },
    )
    .await
    .expect("an unreachable discography degrades rather than failing");
    assert_eq!(out.uri, "spotify:album:a1");
  }

  #[tokio::test]
  async fn a_bare_first_request_with_nothing_playing_is_a_typed_error() {
    let client = test_client(Arc::new(NullObserver));
    let err = resolve(&client, filtered(VoicePopularity::First)).await.unwrap_err();
    assert!(
      matches!(err, VoiceResolveError::NoAnchorContext),
      "a debut of nothing named and nothing playing has no anchor: {err:?}"
    );
  }

  #[tokio::test]
  async fn a_year_tie_prefers_the_studio_cut_but_a_named_live_album_still_wins() {
    let items = vec![
      named_hit("spotify:artist:top", "Twenty One Pilots"),
      album_hit(
        "spotify:album:live",
        "More Than We Ever Imagined (Live in Mexico City)",
        "Twenty One Pilots",
        2025,
      ),
      album_hit("spotify:album:breach", "Breach", "Twenty One Pilots", 2025),
    ];
    let uri = resolved_uri(
      items.clone(),
      VoiceResolveRequest {
        target: Some("twenty one pilots".into()),
        target_type: Some(VoiceTargetKind::Album),
        era: Some("2025".into()),
        ..Default::default()
      },
    )
    .await;
    assert_eq!(uri, "spotify:album:breach", "same year, the studio cut wins");
    let named = resolved_uri(
      items,
      req(
        Some("more than we ever imagined live in mexico city"),
        Some(VoiceTargetKind::Album),
      ),
    )
    .await;
    assert_eq!(
      named, "spotify:album:live",
      "naming the live album is the explicit request"
    );
  }

  #[tokio::test]
  async fn a_filter_with_nothing_named_resolves_instead_of_failing() {
    for filter in [
      VoicePopularity::Popular,
      VoicePopularity::Top5,
      VoicePopularity::Top10,
      VoicePopularity::New,
    ] {
      let (client, _) = searching_client(Arc::new(NullObserver), &["spotify:playlist:p1", "spotify:album:a1"]);
      let out = resolve(&client, filtered(filter))
        .await
        .unwrap_or_else(|e| panic!("{filter:?} with an empty query must resolve, got {e:?}"));
      assert!(kind_of_uri(&out.uri).is_some(), "{filter:?} landed on {}", out.uri);
    }
  }

  #[tokio::test]
  async fn nothing_named_asks_the_chart_for_hits_and_the_tag_for_new_releases() {
    let (client, log) = searching_client(Arc::new(NullObserver), &["spotify:playlist:p1"]);
    resolve(&client, filtered(VoicePopularity::Popular)).await.unwrap();
    assert_eq!(log.queries(), [CHART_QUERY], "a bare hits request is the live chart");

    let (client, log) = searching_client(Arc::new(NullObserver), &["spotify:album:a1"]);
    resolve(&client, filtered(VoicePopularity::New)).await.unwrap();
    assert_eq!(log.queries(), [NEW_RELEASES_TAG]);
  }

  #[tokio::test]
  async fn a_new_release_request_without_an_artist_tags_the_composed_query() {
    let (client, log) = searching_client(Arc::new(NullObserver), &["spotify:album:a1"]);
    let out = resolve(
      &client,
      VoiceResolveRequest {
        era: Some("80s".into()),
        genre: Some("rock".into()),
        popularity_filter: Some(VoicePopularity::New),
        ..Default::default()
      },
    )
    .await
    .unwrap();
    assert_eq!(
      log.queries(),
      ["80s rock tag:new"],
      "modifiers with no name never take the discography path"
    );
    assert_eq!(out.uri, "spotify:album:a1");
  }

  #[tokio::test]
  async fn a_history_request_that_the_history_cannot_answer_falls_back_to_the_catalog() {
    let (client, log) = searching_client(Arc::new(NullObserver), &["spotify:playlist:p1"]);
    let out = resolve(
      &client,
      VoiceResolveRequest {
        genre: Some("jazz".into()),
        popularity_filter: Some(VoicePopularity::Recent),
        ..Default::default()
      },
    )
    .await
    .expect("an unreachable history degrades rather than failing");
    assert_eq!(log.queries(), ["jazz"], "the fallback is the plain unfiltered search");
    assert_eq!(picked(&out), ["spotify:playlist:p1"]);
  }

  #[tokio::test]
  async fn a_station_with_no_seed_still_answers_when_a_filter_carries_the_request() {
    let (client, log) = searching_client(Arc::new(NullObserver), &["spotify:album:a1"]);
    let out = resolve(
      &client,
      VoiceResolveRequest {
        target_type: Some(VoiceTargetKind::Station),
        popularity_filter: Some(VoicePopularity::New),
        ..Default::default()
      },
    )
    .await
    .expect("a seedless station degrades to the filter rather than failing");
    assert_eq!(log.queries(), [NEW_RELEASES_TAG]);
    assert_eq!(out.uri, "spotify:album:a1");
  }

  #[tokio::test]
  async fn a_named_station_still_outranks_a_filter() {
    let (client, log) = searching_client(Arc::new(NullObserver), &["spotify:artist:r1"]);
    let out = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("r1".into()),
        target_type: Some(VoiceTargetKind::Station),
        popularity_filter: Some(VoicePopularity::Popular),
        ..Default::default()
      },
    )
    .await
    .expect("station resolve");
    assert_eq!(log.queries(), ["r1"], "a seeded station never reaches the filter paths");
    assert_eq!(out.uri, "spotify:station:artist:r1");
  }

  #[tokio::test]
  async fn a_position_without_a_target_or_playback_is_a_typed_error() {
    let client = test_client(Arc::new(NullObserver));
    let err = resolve(
      &client,
      VoiceResolveRequest {
        position: Some(3),
        ..Default::default()
      },
    )
    .await
    .unwrap_err();
    assert!(
      matches!(err, VoiceResolveError::NoAnchorContext),
      "nothing playing means nothing to count into: {err:?}"
    );
  }

  fn top_catalog() -> Vec<crate::proto::custom::searchview::SearchItem> {
    vec![
      named_hit("spotify:artist:top", "Twenty One Pilots"),
      album_hit("spotify:album:breach", "Breach", "Twenty One Pilots", 2025),
      album_hit("spotify:album:clancy", "Clancy", "Twenty One Pilots", 2024),
      album_hit(
        "spotify:album:live",
        "More Than We Ever Imagined (Live in Mexico City)",
        "Twenty One Pilots",
        2025,
      ),
      album_hit(
        "spotify:album:selftitled",
        "Twenty One Pilots",
        "Twenty One Pilots",
        2009,
      ),
      album_hit("spotify:album:vessel", "Vessel", "Twenty One Pilots", 2013),
    ]
  }

  async fn resolved_uri(items: Vec<crate::proto::custom::searchview::SearchItem>, req: VoiceResolveRequest) -> String {
    let (client, _) = searching_client_items(Arc::new(NullObserver), items);
    resolve(&client, req).await.expect("a pick").uri
  }

  #[tokio::test]
  async fn an_exact_title_outranks_the_recency_relevance_order() {
    for target in ["twenty one pilots", "21 pilots"] {
      let uri = resolved_uri(top_catalog(), req(Some(target), Some(VoiceTargetKind::Album))).await;
      assert_eq!(
        uri, "spotify:album:selftitled",
        "{target:?} names the self-titled album; relevance floats the newest release first"
      );
    }
  }

  #[tokio::test]
  async fn a_by_phrase_confirmed_against_a_returned_artist_matches_on_its_title_half() {
    for target in ["twenty one pilots by twenty one pilots", "21 pilots by 21 pilots"] {
      let uri = resolved_uri(top_catalog(), req(Some(target), Some(VoiceTargetKind::Album))).await;
      assert_eq!(uri, "spotify:album:selftitled", "{target:?} splits at the last by");
    }
  }

  #[tokio::test]
  async fn an_untyped_confirmed_by_phrase_names_a_work_never_the_artist() {
    let uri = resolved_uri(top_catalog(), req(Some("twenty one pilots by twenty one pilots"), None)).await;
    assert_eq!(
      uri, "spotify:album:selftitled",
      "nobody says 'the artist by the artist'; the head names a work"
    );
  }

  #[tokio::test]
  async fn a_by_phrase_matching_no_returned_artist_stays_whole() {
    let items = vec![
      track_hit("spotify:track:decoy", "Stand", "Somebody Else"),
      track_hit("spotify:track:standbyme", "Stand By Me", "Ben E. King"),
    ];
    let uri = resolved_uri(items, req(Some("stand by me"), Some(VoiceTargetKind::Track))).await;
    assert_eq!(
      uri, "spotify:track:standbyme",
      "no artist named 'me' came back, so the by is part of the title"
    );
  }

  #[tokio::test]
  async fn a_year_qualified_request_reads_the_year_over_the_title() {
    let vessel = resolved_uri(
      top_catalog(),
      VoiceResolveRequest {
        target: Some("twenty one pilots".into()),
        target_type: Some(VoiceTargetKind::Album),
        era: Some("2013".into()),
        ..Default::default()
      },
    )
    .await;
    assert_eq!(
      vessel, "spotify:album:vessel",
      "the year is the discriminator; the artist-named self-titled album must not shadow it"
    );
  }

  #[tokio::test]
  async fn a_year_match_on_the_wrong_artist_never_beats_the_named_artist() {
    let items = vec![
      named_hit("spotify:artist:top", "Twenty One Pilots"),
      album_hit("spotify:album:meek", "2013", "Meek Mill", 2013),
      album_hit(
        "spotify:album:selftitled",
        "Twenty One Pilots",
        "Twenty One Pilots",
        2009,
      ),
    ];
    let uri = resolved_uri(
      items,
      VoiceResolveRequest {
        target: Some("twenty one pilots".into()),
        target_type: Some(VoiceTargetKind::Album),
        era: Some("2013".into()),
        ..Default::default()
      },
    )
    .await;
    assert_eq!(
      uri, "spotify:album:selftitled",
      "a stranger's album that happens to carry the year is not the request"
    );
  }

  #[tokio::test]
  async fn an_era_decade_bounds_the_years() {
    let items = vec![
      album_hit("spotify:album:innuendo", "Innuendo", "Queen", 1991),
      album_hit("spotify:album:thegame", "The Game", "Queen", 1980),
    ];
    let uri = resolved_uri(
      items,
      VoiceResolveRequest {
        target: Some("queen".into()),
        target_type: Some(VoiceTargetKind::Album),
        era: Some("80s".into()),
        ..Default::default()
      },
    )
    .await;
    assert_eq!(uri, "spotify:album:thegame");
  }

  #[tokio::test]
  async fn an_edition_suffix_still_matches_its_title() {
    let items = vec![
      album_hit("spotify:album:trench", "Trench", "Twenty One Pilots", 2018),
      album_hit(
        "spotify:album:blurryface",
        "Blurryface (Deluxe)",
        "Twenty One Pilots",
        2015,
      ),
    ];
    let uri = resolved_uri(items, req(Some("blurryface"), Some(VoiceTargetKind::Album))).await;
    assert_eq!(
      uri, "spotify:album:blurryface",
      "a parenthetical edition is the same album"
    );
  }

  #[tokio::test]
  async fn a_title_tie_breaks_on_the_by_phrase_artist() {
    let items = vec![
      album_hit("spotify:album:xhits", "Greatest Hits", "Xylo", 2001),
      album_hit("spotify:album:yhits", "Greatest Hits", "Yodel", 2003),
    ];
    let uri = resolved_uri(items, req(Some("greatest hits by yodel"), Some(VoiceTargetKind::Album))).await;
    assert_eq!(uri, "spotify:album:yhits");
  }

  #[tokio::test]
  async fn requests_without_a_target_keep_the_relevance_order() {
    let items = vec![
      album_hit("spotify:album:first", "Jazz Classics", "Various", 2001),
      album_hit("spotify:album:second", "Smooth Jazz", "Various", 2020),
    ];
    let uri = resolved_uri(
      items,
      VoiceResolveRequest {
        genre: Some("jazz".into()),
        target_type: Some(VoiceTargetKind::Album),
        ..Default::default()
      },
    )
    .await;
    assert_eq!(uri, "spotify:album:first", "nothing named means relevance decides");
  }

  fn mix_catalog() -> Vec<crate::proto::custom::searchview::SearchItem> {
    vec![
      named_hit("spotify:playlist:37i9dQZF1DWmarrow0000000", "MARROW"),
      named_hit("spotify:playlist:37i9dQZF1EIgalt00000000", "Alternative Mix"),
      named_hit("spotify:playlist:37i9dQZF1EIfeel0000000", "Feel Good Alternative Mix"),
    ]
  }

  fn generic(kind: Option<VoiceTargetKind>, genre: &str) -> VoiceResolveRequest {
    VoiceResolveRequest {
      target_type: kind,
      genre: Some(genre.into()),
      ..Default::default()
    }
  }

  #[tokio::test]
  async fn a_generic_genre_station_plays_the_users_own_mix() {
    let uri = resolved_uri(mix_catalog(), generic(Some(VoiceTargetKind::Station), "alternative")).await;
    assert_eq!(
      uri, "spotify:playlist:37i9dQZF1EIgalt00000000",
      "generic means for-you; the editorial station is only for those who name it"
    );
  }

  #[tokio::test]
  async fn a_generic_genre_play_lands_on_the_users_own_mix() {
    for kind in [None, Some(VoiceTargetKind::Playlist)] {
      let uri = resolved_uri(mix_catalog(), generic(kind, "alternative")).await;
      assert_eq!(uri, "spotify:playlist:37i9dQZF1EIgalt00000000", "{kind:?}");
    }
  }

  #[tokio::test]
  async fn a_named_editorial_station_stays_name_addressable() {
    let items = mix_catalog();
    let uri = resolved_uri(
      items,
      VoiceResolveRequest {
        target: Some("marrow".into()),
        target_type: Some(VoiceTargetKind::Station),
        ..Default::default()
      },
    )
    .await;
    assert_eq!(
      uri, "spotify:station:playlist:37i9dQZF1DWmarrow0000000",
      "naming a station is not a generic request"
    );
  }

  #[tokio::test]
  async fn without_a_made_for_you_mix_a_generic_station_seeds_editorial_as_before() {
    let items = vec![
      named_hit("spotify:playlist:37i9dQZF1DWmarrow0000000", "MARROW"),
      named_hit("spotify:playlist:5FJ4jarantitled0000000", "Alternative Mix"),
    ];
    let uri = resolved_uri(items, generic(Some(VoiceTargetKind::Station), "alternative")).await;
    assert_eq!(
      uri, "spotify:station:playlist:37i9dQZF1DWmarrow0000000",
      "a stranger's playlist that happens to be named like a mix is not the user's mix"
    );
  }

  #[tokio::test]
  async fn a_generic_track_request_never_answers_with_a_playlist_mix() {
    let items = vec![
      track_hit("spotify:track:alt1", "Some Alternative Song", "Somebody"),
      named_hit("spotify:playlist:37i9dQZF1EIgalt00000000", "Alternative Mix"),
    ];
    let uri = resolved_uri(items, generic(Some(VoiceTargetKind::Track), "alternative")).await;
    assert_eq!(uri, "spotify:track:alt1", "a song request answers with a song");
  }

  #[tokio::test]
  async fn a_filtered_genre_request_belongs_to_its_filter_not_the_mix() {
    let (client, log) = searching_client_items(Arc::new(NullObserver), mix_catalog());
    let out = resolve(
      &client,
      VoiceResolveRequest {
        genre: Some("alternative".into()),
        popularity_filter: Some(VoicePopularity::New),
        ..Default::default()
      },
    )
    .await
    .expect("the filter answers");
    assert_eq!(
      log.queries().first().map(String::as_str),
      Some("alternative tag:new"),
      "a new-releases request asks the catalog, not the mixes"
    );
    assert!(!out.uri.is_empty());
  }

  fn bare(kind: VoiceTargetKind) -> VoiceResolveRequest {
    req(None, Some(kind))
  }

  async fn anchored_client(playing: Playing<'_>) -> SpotifyClient {
    playing_client(Arc::new(NullObserver), playing).await
  }

  #[tokio::test]
  async fn a_bare_album_kind_plays_the_album_of_the_current_track() {
    let client = anchored_client(Playing {
      track: "spotify:track:t1",
      album: "spotify:album:a1",
      artist: "spotify:artist:r1",
      context: "spotify:playlist:p1",
    })
    .await;
    let out = resolve(&client, bare(VoiceTargetKind::Album)).await.expect("anchor");
    assert_eq!(out.uri, "spotify:album:a1");
    assert_eq!(out.kind, VoiceTargetKind::Album);
    assert_eq!(out.context_uri, None, "an album is its own context");
    assert!(out.alternatives.is_empty(), "the anchor is the only answer");
  }

  #[tokio::test]
  async fn a_bare_artist_kind_plays_the_artist_of_the_current_track() {
    let client = anchored_client(Playing {
      track: "spotify:track:t1",
      album: "spotify:album:a1",
      artist: "spotify:artist:r1",
      ..Default::default()
    })
    .await;
    let out = resolve(&client, bare(VoiceTargetKind::Artist)).await.expect("anchor");
    assert_eq!(out.uri, "spotify:artist:r1");
    assert_eq!(out.context_uri, None);
  }

  #[tokio::test]
  async fn a_bare_track_kind_replays_the_current_track_inside_its_context() {
    let client = anchored_client(Playing {
      track: "spotify:track:t1",
      album: "spotify:album:a1",
      context: "spotify:playlist:p1",
      ..Default::default()
    })
    .await;
    let out = resolve(&client, bare(VoiceTargetKind::Track)).await.expect("anchor");
    assert_eq!(out.uri, "spotify:track:t1");
    assert_eq!(
      out.context_uri.as_deref(),
      Some("spotify:playlist:p1"),
      "a leaf keeps the context it is playing inside"
    );
  }

  #[tokio::test]
  async fn a_bare_playlist_kind_takes_the_context_that_is_playing() {
    let client = anchored_client(Playing {
      track: "spotify:track:t1",
      album: "spotify:album:a1",
      context: "spotify:playlist:p1",
      ..Default::default()
    })
    .await;
    let out = resolve(&client, bare(VoiceTargetKind::Playlist)).await.expect("anchor");
    assert_eq!(out.uri, "spotify:playlist:p1");
    assert_eq!(out.context_uri, None);
  }

  #[tokio::test]
  async fn a_bare_playlist_kind_over_an_album_context_is_a_typed_error() {
    let client = anchored_client(Playing {
      track: "spotify:track:t1",
      album: "spotify:album:a1",
      context: "spotify:album:a1",
      ..Default::default()
    })
    .await;
    let err = resolve(&client, bare(VoiceTargetKind::Playlist)).await.unwrap_err();
    assert!(
      matches!(err, VoiceResolveError::NoAnchorContext),
      "no playlist is playing, so there is nothing to read the kind against: {err:?}"
    );
  }

  #[tokio::test]
  async fn a_bare_episode_kind_plays_the_episode_that_is_on() {
    let client = anchored_client(Playing {
      track: "spotify:episode:e1",
      context: "spotify:show:s1",
      ..Default::default()
    })
    .await;
    let out = resolve(&client, bare(VoiceTargetKind::Episode)).await.expect("anchor");
    assert_eq!(
      out.uri, "spotify:episode:e1",
      "the playing episode is the leaf, not the show"
    );
    assert_eq!(out.context_uri.as_deref(), Some("spotify:show:s1"));

    let show = resolve(&client, bare(VoiceTargetKind::Show)).await.expect("anchor");
    assert_eq!(show.uri, "spotify:show:s1");
    assert_eq!(show.context_uri, None);
  }

  #[tokio::test]
  async fn a_bare_station_kind_needs_a_station_context() {
    let client = anchored_client(Playing {
      track: "spotify:track:t1",
      context: "spotify:station:artist:r1",
      ..Default::default()
    })
    .await;
    let out = resolve(&client, bare(VoiceTargetKind::Station)).await.expect("anchor");
    assert_eq!(out.uri, "spotify:station:artist:r1");
  }

  #[tokio::test]
  async fn a_bare_station_kind_over_a_plain_context_seeds_radio_from_the_playing_artist() {
    let client = anchored_client(Playing {
      track: "spotify:track:t1",
      artist: "spotify:artist:a1",
      context: "spotify:playlist:p1",
      ..Default::default()
    })
    .await;
    let out = resolve(&client, bare(VoiceTargetKind::Station)).await.expect("seeded");
    assert_eq!(out.uri, "spotify:station:artist:a1");
  }

  #[tokio::test]
  async fn a_bare_station_kind_with_no_artist_on_the_track_is_a_typed_error() {
    let client = anchored_client(Playing {
      track: "spotify:track:t1",
      context: "spotify:playlist:p1",
      ..Default::default()
    })
    .await;
    let out = resolve(&client, bare(VoiceTargetKind::Station)).await;
    assert!(matches!(out, Err(VoiceResolveError::NoAnchorContext)));
  }

  #[tokio::test]
  async fn a_bare_kind_with_nothing_playing_is_a_typed_error() {
    let client = test_client(Arc::new(NullObserver));
    for kind in [
      VoiceTargetKind::Album,
      VoiceTargetKind::Artist,
      VoiceTargetKind::Track,
      VoiceTargetKind::Playlist,
      VoiceTargetKind::Show,
      VoiceTargetKind::Episode,
      VoiceTargetKind::Station,
    ] {
      let err = resolve(&client, bare(kind)).await.unwrap_err();
      assert!(
        matches!(err, VoiceResolveError::NoAnchorContext),
        "{kind:?} has nothing to anchor against: {err:?}"
      );
    }
  }

  #[tokio::test]
  async fn a_bare_kind_carrying_a_filter_still_belongs_to_the_filter() {
    let (client, log) = searching_client(Arc::new(NullObserver), &["spotify:playlist:p1"]);
    let out = resolve(
      &client,
      VoiceResolveRequest {
        target_type: Some(VoiceTargetKind::Album),
        popularity_filter: Some(VoicePopularity::Popular),
        ..Default::default()
      },
    )
    .await
    .expect("the filter answers");
    assert_eq!(log.queries(), [CHART_QUERY], "a filter asks the world, not what is on");
    assert_eq!(out.uri, "spotify:playlist:p1");
  }

  #[test]
  fn the_collection_uri_is_a_playable_playlist_kind() {
    assert_eq!(kind_of_uri(LIKED), Some(VoiceTargetKind::Playlist));
    assert!(is_liked_songs(LIKED));
    assert_eq!(kind_of_uri("spotify:user:nobody"), None);
  }

  #[test]
  fn unplayable_uris_never_become_candidates() {
    assert_eq!(kind_of_uri("spotify:genre:rock"), None);
    assert_eq!(kind_of_uri("spotify:user:nobody"), None);
    assert_eq!(kind_of_uri("spotify:track:t1"), Some(VoiceTargetKind::Track));
  }

  #[tokio::test]
  async fn a_station_wraps_its_seed_uri_whatever_the_seed_kind() {
    for (seed, want) in [
      ("spotify:artist:r1", "spotify:station:artist:r1"),
      ("spotify:track:t1", "spotify:station:track:t1"),
      ("spotify:album:a1", "spotify:station:album:a1"),
      ("spotify:playlist:p1", "spotify:station:playlist:p1"),
    ] {
      let (client, _) = searching_client_items(Arc::new(NullObserver), vec![named_hit(seed, "Seed")]);
      let out = resolve(&client, req(Some("whatever"), Some(VoiceTargetKind::Station)))
        .await
        .expect("a station");
      assert_eq!(out.uri, want);
      assert_eq!(out.kind, VoiceTargetKind::Station);
      assert_eq!(out.display, "Seed", "the seed name is the display");
      assert_eq!(
        kind_of_uri(&out.uri),
        Some(VoiceTargetKind::Station),
        "a synthesized station uri reads back as one"
      );
    }
  }

  #[tokio::test]
  async fn a_station_never_seeds_off_a_podcast() {
    let uri = resolved_uri(
      vec![
        search_item("spotify:show:s1"),
        search_item("spotify:episode:e1"),
        search_item("spotify:album:a1"),
      ],
      req(Some("q"), Some(VoiceTargetKind::Station)),
    )
    .await;
    assert_eq!(
      uri, "spotify:station:album:a1",
      "the server rejects podcast seeds outright"
    );
  }
}

#[cfg(all(test, feature = "native-io"))]
mod live {
  use std::{path::PathBuf, sync::Arc};

  use voice_resolver::{VoicePopularity, VoiceResolveRequest, VoiceResolved, resolve};

  use super::*;
  use crate::{
    auth::{Auth, DEFAULT_WORKER_BASE},
    client::Observer,
    http::{SpHttp, random_hex},
    httpx,
    model::{AuthState, Device, LibraryScope, PlayerState, Queue},
    spclient::SpClient,
    store::FileTokenStore,
  };

  struct Silent;
  impl Observer for Silent {
    fn on_player(&self, _state: PlayerState) {}
    fn on_queue(&self, _queue: Queue) {}
    fn on_devices(&self, _devices: Vec<Device>) {}
    fn on_auth(&self, _state: AuthState) {}
    fn on_library_changed(&self, _scope: LibraryScope) {}
  }

  fn enabled() -> bool {
    std::env::var("SPOTIFY_LIVE").as_deref() == Ok("1")
  }

  fn state_dir() -> PathBuf {
    match std::env::var("SPOTIFY_PRIVATE_STATE") {
      Ok(dir) => PathBuf::from(dir),
      Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.spotify-private"),
    }
  }

  static AUTH: tokio::sync::OnceCell<Arc<Auth>> = tokio::sync::OnceCell::const_new();

  async fn auth() -> Arc<Auth> {
    AUTH
      .get_or_init(|| async {
        let psk = std::env::var("SPOTIFY_AUTH_PSK").expect("SPOTIFY_AUTH_PSK gates the private-auth worker");
        let base = std::env::var("SPOTIFY_AUTH_BASE").unwrap_or_else(|_| DEFAULT_WORKER_BASE.to_string());
        let dir = state_dir();
        let store = FileTokenStore::new(&dir).expect("private state dir");
        let auth = Arc::new(Auth::new(base, psk, Box::new(store), httpx::executor()));
        assert!(
          auth.is_paired().await,
          "live lane needs a paired refresh token in {}; run `sfp pair` first",
          dir.display()
        );
        auth
      })
      .await
      .clone()
  }

  async fn client() -> SpotifyClient {
    let client = SpotifyClient::new(auth().await, random_hex(20), httpx::executor(), Arc::new(Silent));
    client.connect().await.expect("live connect");
    client
  }

  fn show(label: &str, out: &VoiceResolved) {
    println!(
      "[{label}] {} kind={:?} context={:?} display={:?} alts={}",
      out.uri,
      out.kind,
      out.context_uri,
      out.display,
      out.alternatives.len()
    );
  }

  const TOP_SELF_TITLED: &str = "spotify:album:6rgWZP4QFBjEFF0n6JWEOa";
  const TOP_VESSEL: &str = "spotify:album:2r2r78NE05YjyHyVbVgqFn";
  const TOP_TRENCH: &str = "spotify:album:621cXqrTSSJi1WqDMSLmbL";

  async fn live_album(client: &SpotifyClient, target: &str, era: Option<&str>) -> VoiceResolved {
    resolve(
      client,
      VoiceResolveRequest {
        target: Some(target.into()),
        target_type: Some(VoiceTargetKind::Album),
        era: era.map(str::to_string),
        ..Default::default()
      },
    )
    .await
    .unwrap_or_else(|e| panic!("album resolve for {target:?} era {era:?}: {e:?}"))
  }

  #[tokio::test]
  async fn live_a_self_titled_album_beats_the_recency_ranking_even_as_asr_digits() {
    if !enabled() {
      return;
    }
    let client = client().await;
    for target in [
      "twenty one pilots",
      "21 pilots",
      "twenty one pilots by twenty one pilots",
      "21 pilots by 21 pilots",
    ] {
      let out = live_album(&client, target, None).await;
      show(&format!("self-titled {target}"), &out);
      assert_eq!(out.uri, TOP_SELF_TITLED, "{target:?} names the 2009 self-titled album");
    }
    client.disconnect().await;
  }

  #[tokio::test]
  async fn live_a_year_qualified_album_resolves_through_the_discography() {
    if !enabled() {
      return;
    }
    let client = client().await;
    for (era, want) in [("2009", TOP_SELF_TITLED), ("2013", TOP_VESSEL)] {
      let out = live_album(&client, "twenty one pilots", Some(era)).await;
      show(&format!("year {era}"), &out);
      assert_eq!(out.uri, want, "era {era} discriminates the release");
    }
    client.disconnect().await;
  }

  #[tokio::test]
  async fn live_a_first_request_reads_the_discography_from_the_oldest_end() {
    if !enabled() {
      return;
    }
    let client = client().await;
    for (target, want_year) in [("twenty one pilots", 2009), ("taylor swift", 2006)] {
      let out = resolve(
        &client,
        VoiceResolveRequest {
          target: Some(target.into()),
          target_type: Some(VoiceTargetKind::Album),
          popularity_filter: Some(VoicePopularity::First),
          ..Default::default()
        },
      )
      .await
      .unwrap_or_else(|e| panic!("first resolve for {target:?}: {e:?}"));
      show(&format!("first {target}"), &out);
      assert_eq!(out.kind, VoiceTargetKind::Album);
      assert_eq!(
        out.year,
        Some(want_year),
        "{target:?} debut year; got {:?}",
        out.display
      );
    }
    client.disconnect().await;
  }

  #[tokio::test]
  async fn live_a_named_album_title_still_resolves_directly() {
    if !enabled() {
      return;
    }
    let client = client().await;
    let out = live_album(&client, "trench by twenty one pilots", None).await;
    show("trench", &out);
    assert_eq!(out.uri, TOP_TRENCH);
    client.disconnect().await;
  }

  #[tokio::test]
  async fn live_resolution_lands_on_real_uris_without_ever_commanding_playback() {
    if !enabled() {
      return;
    }
    let client = client().await;

    let artist = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("taylor swift".into()),
        target_type: Some(VoiceTargetKind::Artist),
        ..Default::default()
      },
    )
    .await
    .expect("typed artist resolve");
    show("artist", &artist);
    assert!(artist.uri.starts_with("spotify:artist:"), "got {}", artist.uri);
    assert_eq!(artist.kind, VoiceTargetKind::Artist);
    assert_eq!(artist.context_uri, None, "an artist uri is already the context");

    let untyped = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("bohemian rhapsody".into()),
        ..Default::default()
      },
    )
    .await
    .expect("untyped resolve");
    show("untyped", &untyped);
    assert!(kind_of_uri(&untyped.uri).is_some(), "got {}", untyped.uri);

    let genre = resolve(
      &client,
      VoiceResolveRequest {
        genre: Some("jazz".into()),
        mood: Some("chill".into()),
        ..Default::default()
      },
    )
    .await
    .expect("genre resolve");
    show("genre+mood", &genre);
    assert!(kind_of_uri(&genre.uri).is_some(), "got {}", genre.uri);

    let era = resolve(
      &client,
      VoiceResolveRequest {
        era: Some("80s".into()),
        genre: Some("rock".into()),
        ..Default::default()
      },
    )
    .await
    .expect("era resolve");
    show("era+genre", &era);
    assert!(kind_of_uri(&era.uri).is_some(), "got {}", era.uri);

    let position = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("rumours fleetwood mac".into()),
        target_type: Some(VoiceTargetKind::Album),
        position: Some(3),
        ..Default::default()
      },
    )
    .await
    .expect("position resolve");
    show("position", &position);
    assert!(position.uri.starts_with("spotify:track:"), "got {}", position.uri);
    assert!(
      position
        .context_uri
        .as_deref()
        .is_some_and(|c| c.starts_with("spotify:album:")),
      "a position always reports the container it counted into: {:?}",
      position.context_uri
    );

    let playing = client.current_context_uri().await;
    let recents = client.recent_contexts().await;
    let random = resolve(
      &client,
      VoiceResolveRequest {
        popularity_filter: Some(VoicePopularity::Random),
        ..Default::default()
      },
    )
    .await
    .expect("fresh pick");
    show("random", &random);
    assert!(
      matches!(
        kind_of_uri(&random.uri),
        Some(VoiceTargetKind::Album | VoiceTargetKind::Artist | VoiceTargetKind::Playlist)
      ),
      "a fresh pick is a playable music context: {}",
      random.uri
    );
    assert_ne!(
      Some(random.uri.clone()),
      playing,
      "a fresh pick must never be a resume of what is already on"
    );
    assert!(
      !recents.contains(&random.uri),
      "a fresh pick must never replay a recent context: {}",
      random.uri
    );

    for seed in ["elvis presley", "bohemian rhapsody"] {
      let station = resolve(
        &client,
        VoiceResolveRequest {
          target: Some(seed.into()),
          target_type: Some(VoiceTargetKind::Station),
          ..Default::default()
        },
      )
      .await
      .expect("station resolve");
      show(&format!("station {seed}"), &station);
      assert!(station.uri.starts_with("spotify:station:"), "got {}", station.uri);
      assert_eq!(station.kind, VoiceTargetKind::Station);
      assert_eq!(station.context_uri, None, "a station uri is already the context");
      assert_station_resolves(&station.uri).await;
    }

    client.disconnect().await;
  }

  #[tokio::test]
  async fn live_every_popularity_filter_answers_with_nothing_else_named() {
    if !enabled() {
      return;
    }
    let client = client().await;
    for filter in [
      VoicePopularity::Popular,
      VoicePopularity::Top5,
      VoicePopularity::Top10,
      VoicePopularity::New,
      VoicePopularity::Recent,
      VoicePopularity::Random,
    ] {
      let out = resolve(
        &client,
        VoiceResolveRequest {
          popularity_filter: Some(filter),
          ..Default::default()
        },
      )
      .await
      .unwrap_or_else(|e| panic!("a bare {filter:?} request must never fail the turn: {e:?}"));
      show(&format!("bare {filter:?}"), &out);
      assert!(
        kind_of_uri(&out.uri).is_some(),
        "{filter:?} landed on an unplayable uri: {}",
        out.uri
      );
    }
    client.disconnect().await;
  }

  #[tokio::test]
  async fn live_a_named_artist_reads_their_own_discography_and_top_tracks() {
    if !enabled() {
      return;
    }
    let client = client().await;

    let latest = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("taylor swift".into()),
        target_type: Some(VoiceTargetKind::Album),
        popularity_filter: Some(VoicePopularity::New),
        ..Default::default()
      },
    )
    .await
    .expect("latest album resolve");
    show("latest album", &latest);
    assert!(latest.uri.starts_with("spotify:album:"), "got {}", latest.uri);
    assert_eq!(latest.kind, VoiceTargetKind::Album);
    assert_eq!(
      latest.context_uri.as_deref(),
      Some("spotify:artist:06HL4z0CvFAxyc27GXpf02"),
      "a discography pick reports the artist it counted into"
    );

    let hit = resolve(
      &client,
      VoiceResolveRequest {
        target: Some("taylor swift".into()),
        popularity_filter: Some(VoicePopularity::Top5),
        ..Default::default()
      },
    )
    .await
    .expect("top track resolve");
    show("top track", &hit);
    assert!(hit.uri.starts_with("spotify:track:"), "got {}", hit.uri);
    assert!(
      hit.alternatives.len() <= 4,
      "a counted filter never offers more than the alternatives cap"
    );

    client.disconnect().await;
  }

  #[tokio::test]
  async fn live_the_new_release_tag_still_narrows_the_catalog() {
    if !enabled() {
      return;
    }
    let client = client().await;
    let items = client.search_flat(NEW_RELEASES_TAG, 20).await.expect("tagged search");
    let uris: Vec<String> = items.iter().map(|i| i.uri.clone()).collect();
    assert!(!uris.is_empty(), "the tag returned nothing at all");
    assert!(
      uris.iter().all(|u| u.starts_with("spotify:album:")),
      "the tag is an album-only narrowing; got {uris:?}"
    );
    let releases = client.popularity(&uris).await;
    assert!(
      !releases.is_empty(),
      "tagged hits must hydrate as real albums, not phantom uris"
    );
    client.disconnect().await;
  }

  #[tokio::test]
  async fn live_liked_songs_phrases_resolve_to_the_users_collection() {
    if !enabled() {
      return;
    }
    let client = client().await;
    for target in ["liked songs", "my favorites", "my library"] {
      let out = resolve(
        &client,
        VoiceResolveRequest {
          target: Some(target.into()),
          ..Default::default()
        },
      )
      .await
      .unwrap_or_else(|e| panic!("{target:?}: {e:?}"));
      show(target, &out);
      assert!(is_liked_songs(&out.uri), "{target:?} landed on {}", out.uri);
    }
    let liked = client.favorites_list(1, 0).await.expect("liked songs page");
    println!("liked songs total {:?}", liked.total);
    assert!(
      liked.total.is_some_and(|n| n > 0),
      "the collection pages into real tracks"
    );
    client.disconnect().await;
  }

  async fn assert_station_resolves(uri: &str) {
    let spc = SpClient::new(SpHttp::new(auth().await, httpx::executor()));
    let ctx = spc.context_resolve(uri).await.expect("station context resolves");
    let pages = ctx.get("pages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let tracks = pages
      .first()
      .and_then(|p| p.get("tracks"))
      .and_then(|v| v.as_array())
      .map(Vec::len)
      .unwrap_or(0);
    let shuffle_reasons = ctx
      .get("restrictions")
      .and_then(|r| r.get("disallow_toggling_shuffle_reasons"))
      .and_then(|v| v.as_array())
      .cloned()
      .unwrap_or_default();
    println!("  {uri} -> {tracks} tracks, shuffle disallowed {shuffle_reasons:?}");
    assert!(tracks > 0, "a station resolves to real tracks");
    assert!(
      shuffle_reasons.iter().any(|r| r.as_str() == Some("radio")),
      "expected the radio shuffle restriction on {uri}, got {shuffle_reasons:?}"
    );
  }
}
