use std::collections::{HashMap, HashSet};

use crate::{
  Catalog, CatalogItem, LibraryPlaylist, Release, VoiceAlternative, VoicePopularity, VoiceResolveError,
  VoiceResolveRequest, VoiceResolved, VoiceResult, VoiceTargetKind,
  text::{era_years, live_recording, norm},
};

const SEARCH_LIMIT: u32 = 20;
const ALTERNATIVES: usize = 4;
const DISCOGRAPHY_DEPTH: usize = 8;
const DISCOGRAPHY_LOOKUP_DEPTH: usize = 24;
const DISCOGRAPHY_FULL_DEPTH: usize = 60;
const RECENT_POOL: usize = 40;

const LIKED_SONGS: &[&str] = &[
  "liked songs",
  "liked song",
  "liked tracks",
  "liked music",
  "favorite songs",
  "favourite songs",
  "favorite tracks",
  "favourite tracks",
  "favorite music",
  "favourite music",
  "saved songs",
  "saved tracks",
  "saved music",
  "library songs",
  "library s songs",
  "songs in my library",
  "songs i like",
  "songs i liked",
  "songs i ve liked",
  "songs i have liked",
  "songs i saved",
  "tracks i like",
  "tracks i liked",
];

const MY_LIKED_SONGS: &[&str] = &[
  "likes",
  "liked",
  "favorites",
  "favourites",
  "favorite",
  "favourite",
  "faves",
  "songs",
  "tracks",
  "music",
  "library",
  "saved",
];

const SAVED_ALBUMS: &[&str] = &[
  "saved albums",
  "library albums",
  "liked albums",
  "favorite albums",
  "favourite albums",
  "albums in my library",
  "albums i saved",
  "albums i ve saved",
  "albums i like",
];

const MY_SAVED_ALBUMS: &[&str] = &["albums", "records"];

pub async fn resolve(catalog: &dyn Catalog, req: VoiceResolveRequest) -> VoiceResult<VoiceResolved> {
  if let Some(collection) = collection_of(&req) {
    return resolve_collection(catalog, collection, &req).await;
  }
  let query = compose_query(&req);
  if req.target_type == Some(VoiceTargetKind::Station) && !query.is_empty() {
    if wants_personal_mix(&req)
      && let Some(mixes) = personal_mix(catalog, &query).await
    {
      return head_of(mixes, None);
    }
    return resolve_station(catalog, &query, req.target.as_deref()).await;
  }
  if let Some(position) = req.position {
    return resolve_position(catalog, &req, &query, position).await;
  }
  if let Some(kind) = anchored_kind(&req, &query) {
    return resolve_anchor(catalog, kind).await;
  }
  match req.popularity_filter {
    Some(VoicePopularity::Random) => resolve_random(catalog, &req, &query).await,
    Some(VoicePopularity::Recent) => resolve_recent(catalog, &req, &query).await,
    Some(VoicePopularity::New) => resolve_new(catalog, &req, &query).await,
    Some(VoicePopularity::First) => resolve_first(catalog, &req, &query).await,
    Some(filter) => resolve_popular(catalog, &req, &query, filter.depth()).await,
    None => resolve_search(catalog, &req, &query).await,
  }
}

async fn searched(catalog: &dyn Catalog, query: &str) -> VoiceResult<Vec<Candidate>> {
  Ok(typed(catalog, catalog.search(query, SEARCH_LIMIT).await?))
}

async fn hydrated(catalog: &dyn Catalog, uris: &[String]) -> Vec<Candidate> {
  typed(catalog, catalog.hydrate(uris).await)
}

async fn resolve_first(catalog: &dyn Catalog, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    let anchor = catalog.now_playing().await.ok_or(VoiceResolveError::NoAnchorContext)?;
    let artist_uri = anchor.artist.ok_or(VoiceResolveError::NoAnchorContext)?;
    let named = hydrated(catalog, std::slice::from_ref(&artist_uri)).await;
    let display = named.first().map(|c| c.display.clone()).unwrap_or_default();
    return first_of_discography(catalog, &artist_uri, &display, req).await;
  }
  let items = searched(catalog, query).await?;
  if let Some(artist) = artist_anchor(&items, req)
    && let Ok(out) = first_of_discography(catalog, &artist.uri, &artist.display, req).await
  {
    return Ok(out);
  }
  resolve_search(catalog, req, query).await
}

async fn first_of_discography(
  catalog: &dyn Catalog,
  artist_uri: &str,
  display: &str,
  req: &VoiceResolveRequest,
) -> VoiceResult<VoiceResolved> {
  let albums_only = req.target_type == Some(VoiceTargetKind::Album);
  let releases = catalog
    .artist_releases(artist_uri, albums_only, DISCOGRAPHY_FULL_DEPTH)
    .await?;
  let first = earliest_first(releases, display);
  match first.first().cloned() {
    Some(head) => Ok(resolved(head, &first[1..], Some(artist_uri.to_string()))),
    None => Err(VoiceResolveError::NoMatch),
  }
}

fn earliest_first(releases: Vec<Release>, artist: &str) -> Vec<Candidate> {
  let mut releases = releases;
  releases.sort_by_key(|r| (live_recording(&r.name), r.released, std::cmp::Reverse(r.popularity)));
  releases.into_iter().map(|r| released_album(r, artist)).collect()
}

fn latest_first(releases: Vec<Release>, artist: &str) -> Vec<Candidate> {
  let mut releases = releases;
  releases.sort_by_key(|r| {
    (
      live_recording(&r.name),
      std::cmp::Reverse(r.released),
      std::cmp::Reverse(r.popularity),
    )
  });
  releases.into_iter().map(|r| released_album(r, artist)).collect()
}

fn released_album(r: Release, artist: &str) -> Candidate {
  Candidate {
    uri: r.uri,
    display: r.name,
    kind: VoiceTargetKind::Album,
    artist: Some(artist.to_string()),
    year: Some(r.released.0),
  }
}

async fn resolve_search(catalog: &dyn Catalog, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    return Err(VoiceResolveError::NoMatch);
  }
  if wants_personal_mix(req)
    && matches!(req.target_type, None | Some(VoiceTargetKind::Playlist))
    && let Some(mixes) = personal_mix(catalog, query).await
  {
    return head_of(mixes, None);
  }
  let (library, searched) = futures::join!(library_hits(catalog, req), catalog.search(query, SEARCH_LIMIT));
  let items = match searched {
    Ok(items) => typed(catalog, items),
    Err(_) if !library.is_empty() => Vec::new(),
    Err(e) => return Err(e.into()),
  };
  let mut ranked = ranked_search(&items, req);
  if let Some(want) = want_of(req, &items) {
    ranked = scored(ranked, &want);
    if req.target_type == Some(VoiceTargetKind::Album) && needs_discography(&ranked, &want) {
      ranked = scored(with_discography(catalog, ranked, &want, &items).await, &want);
    }
  }
  head_of(library_first(library, ranked), None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Collection {
  LikedSongs,
  SavedAlbums,
}

fn collection_of(req: &VoiceResolveRequest) -> Option<Collection> {
  let target = req.target.as_deref()?;
  let (mine, phrase) = collection_phrase(target);
  let named =
    |always: &[&str], owned: &[&str]| always.contains(&phrase.as_str()) || (mine && owned.contains(&phrase.as_str()));
  let songs = matches!(
    req.target_type,
    None | Some(VoiceTargetKind::Playlist | VoiceTargetKind::Track)
  );
  let albums = matches!(req.target_type, None | Some(VoiceTargetKind::Album));
  if songs && named(LIKED_SONGS, MY_LIKED_SONGS) {
    return Some(Collection::LikedSongs);
  }
  if albums && named(SAVED_ALBUMS, MY_SAVED_ALBUMS) {
    return Some(Collection::SavedAlbums);
  }
  None
}

fn collection_phrase(target: &str) -> (bool, String) {
  let normalized = norm(target);
  let mut words: Vec<&str> = normalized.split(' ').filter(|w| !w.is_empty()).collect();
  let mut mine = false;
  while let Some(first) = words.first().copied() {
    match first {
      "my" => mine = true,
      "the" | "all" | "of" => {}
      _ => break,
    }
    words.remove(0);
  }
  while matches!(words.last().copied(), Some("playlist" | "collection")) {
    words.pop();
  }
  (mine, words.join(" "))
}

async fn resolve_collection(
  catalog: &dyn Catalog,
  collection: Collection,
  req: &VoiceResolveRequest,
) -> VoiceResult<VoiceResolved> {
  let items = match collection {
    Collection::LikedSongs => vec![catalog.liked_songs().await?],
    Collection::SavedAlbums => catalog.saved_albums(Some(1 + ALTERNATIVES)).await?,
  };
  let containers = typed(catalog, items);
  let head = containers.first().cloned().ok_or(VoiceResolveError::NoMatch)?;
  match req.position {
    Some(position) => {
      let page = catalog
        .container(&head.uri, 1 + ALTERNATIVES as u32, offset_of(position))
        .await?;
      head_of(typed(catalog, page), Some(head.uri))
    }
    None => head_of(containers, None),
  }
}

async fn library_hits(catalog: &dyn Catalog, req: &VoiceResolveRequest) -> Vec<Candidate> {
  if req.era.is_some() || req.mood.is_some() || req.genre.is_some() {
    return Vec::new();
  }
  let Some(target) = req.target.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
    return Vec::new();
  };
  let wants = library_names(target);
  let allows = |kind: VoiceTargetKind| req.target_type.is_none_or(|k| k == kind);
  let (playlists, albums, artists) = futures::join!(
    async {
      if !allows(VoiceTargetKind::Playlist) {
        return Vec::new();
      }
      catalog.library_playlists().await.unwrap_or_default()
    },
    async {
      if !allows(VoiceTargetKind::Album) {
        return Vec::new();
      }
      typed(catalog, catalog.saved_albums(None).await.unwrap_or_default())
    },
    async {
      if !allows(VoiceTargetKind::Artist) {
        return Vec::new();
      }
      typed(catalog, catalog.followed_artists().await.unwrap_or_default())
    },
  );
  let mut mine: Vec<LibraryPlaylist> = playlists
    .into_iter()
    .filter(|p| wants.contains(&norm(&p.name)))
    .collect();
  mine.sort_by_key(|p| !p.owned);
  let titled = |c: &Candidate| wants.contains(&norm(&c.display));
  let credited = |c: &Candidate| {
    c.artist
      .as_deref()
      .is_some_and(|a| wants.contains(&format!("{} by {}", norm(&c.display), norm(a))))
  };
  mine
    .into_iter()
    .map(|p| Candidate {
      uri: p.uri,
      display: p.name,
      kind: VoiceTargetKind::Playlist,
      artist: None,
      year: None,
    })
    .chain(albums.into_iter().filter(|c| titled(c) || credited(c)))
    .chain(artists.into_iter().filter(titled))
    .collect()
}

fn library_names(target: &str) -> HashSet<String> {
  let whole = norm(target);
  let bare = ["my ", "the "]
    .iter()
    .find_map(|p| whole.strip_prefix(p))
    .unwrap_or(&whole)
    .to_string();
  let mut out = HashSet::new();
  for name in [whole.clone(), bare] {
    for suffix in [" playlist", " album"] {
      if let Some(core) = name.strip_suffix(suffix) {
        out.insert(core.to_string());
      }
    }
    out.insert(name);
  }
  out.remove("");
  out
}

fn library_first(library: Vec<Candidate>, catalog: Vec<Candidate>) -> Vec<Candidate> {
  let held: HashSet<String> = library.iter().map(|c| c.uri.clone()).collect();
  library
    .into_iter()
    .chain(catalog.into_iter().filter(|c| !held.contains(&c.uri)))
    .collect()
}

async fn resolve_random(catalog: &dyn Catalog, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    return fresh_pick(catalog).await;
  }
  let items = searched(catalog, query).await?;
  let mut ranked = ranked_search(&items, req);
  if ranked.is_empty() {
    return fresh_pick(catalog).await;
  }
  let chosen = rand::random_range(0..ranked.len());
  ranked.rotate_left(chosen);
  head_of(ranked, None)
}

async fn resolve_recent(catalog: &dyn Catalog, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  let history = typed(catalog, catalog.recently_played(RECENT_POOL).await);
  let matched = narrow(history, typed_pick(req), query);
  if let Some(head) = matched.first().cloned() {
    return Ok(resolved(head, &matched[1..], None));
  }
  if query.is_empty() {
    return fresh_pick(catalog).await;
  }
  resolve_search(catalog, req, query).await
}

async fn resolve_new(catalog: &dyn Catalog, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  if !query.is_empty() && req.target.is_some() {
    let items = searched(catalog, query).await?;
    if let Some(artist) = artist_anchor(&items, req) {
      let albums_only = req.target_type == Some(VoiceTargetKind::Album);
      let releases = catalog
        .artist_releases(&artist.uri, albums_only, DISCOGRAPHY_DEPTH)
        .await?;
      let latest = latest_first(releases, &artist.display);
      if let Some(head) = latest.first().cloned() {
        return Ok(resolved(head, &latest[1..], Some(artist.uri)));
      }
    }
  }
  let items = typed(catalog, catalog.new_releases(query, SEARCH_LIMIT).await?);
  let ranked = rank(&items, typed_pick(req));
  if let Some(head) = ranked.first().cloned() {
    return Ok(resolved(head, &ranked[1..], None));
  }
  if query.is_empty() {
    return fresh_pick(catalog).await;
  }
  resolve_search(catalog, req, query).await
}

async fn resolve_popular(
  catalog: &dyn Catalog,
  req: &VoiceResolveRequest,
  query: &str,
  depth: Option<usize>,
) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    return chart_pick(catalog, depth).await;
  }
  let items = searched(catalog, query).await?;
  if let Some(artist) = artist_anchor(&items, req) {
    let page = catalog
      .artist_top_tracks(&artist.uri, &artist.display, depth.unwrap_or(ALTERNATIVES + 1) as u32)
      .await?;
    let mut top = typed(catalog, page);
    truncate(&mut top, depth);
    if let Some(head) = top.first().cloned() {
      return Ok(resolved(head, &top[1..], Some(artist.uri)));
    }
  }
  let ranked = by_popularity(catalog, ranked_search(&items, req), depth).await;
  if let Some(head) = ranked.first().cloned() {
    return Ok(resolved(head, &ranked[1..], None));
  }
  chart_pick(catalog, depth).await
}

async fn chart_pick(catalog: &dyn Catalog, depth: Option<usize>) -> VoiceResult<VoiceResolved> {
  let items = typed(catalog, catalog.charts(SEARCH_LIMIT).await?);
  let mut ranked = rank(&items, Pick::Playlist);
  if ranked.is_empty() {
    ranked = rank(&items, Pick::Any);
  }
  truncate(&mut ranked, depth);
  match ranked.first().cloned() {
    Some(head) => Ok(resolved(head, &ranked[1..], None)),
    None => fresh_pick(catalog).await,
  }
}

async fn by_popularity(catalog: &dyn Catalog, ranked: Vec<Candidate>, depth: Option<usize>) -> Vec<Candidate> {
  if ranked.len() < 2 {
    return ranked;
  }
  let uris: Vec<String> = ranked.iter().map(|c| c.uri.clone()).collect();
  let scores = catalog.popularity(&uris).await;
  rank_by(ranked, &scores, depth)
}

fn rank_by(ranked: Vec<Candidate>, scores: &HashMap<String, i32>, depth: Option<usize>) -> Vec<Candidate> {
  let mut out = ranked;
  out.sort_by_key(|c| std::cmp::Reverse(scores.get(&c.uri).copied().unwrap_or(0)));
  truncate(&mut out, depth);
  out
}

async fn resolve_station(catalog: &dyn Catalog, query: &str, target: Option<&str>) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    return Err(VoiceResolveError::NoMatch);
  }
  let items = searched(catalog, query).await?;
  let named = target.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(query);
  let stations = stations_of(catalog, station_seeds(catalog, &items, named)).await;
  let head = stations.first().cloned().ok_or(VoiceResolveError::NoMatch)?;
  Ok(resolved(head, &stations[1..], None))
}

fn station_seeds(catalog: &dyn Catalog, items: &[Candidate], named: &str) -> Vec<Candidate> {
  let named = norm(named);
  let mut seeds: Vec<Candidate> = items
    .iter()
    .filter(|c| c.kind == VoiceTargetKind::Station || catalog.seeds_station(c.kind))
    .cloned()
    .collect();
  let named_artist = seeds
    .iter()
    .position(|c| c.kind == VoiceTargetKind::Artist && norm(&c.display) == named);
  if let Some(idx) = named_artist {
    seeds[..=idx].rotate_right(1);
  }
  seeds
}

async fn stations_of(catalog: &dyn Catalog, seeds: Vec<Candidate>) -> Vec<Candidate> {
  let mut out = Vec::new();
  for seed in seeds {
    if out.len() > ALTERNATIVES {
      break;
    }
    if seed.kind == VoiceTargetKind::Station {
      out.push(seed);
    } else if let Some(uri) = catalog.station(&seed.uri).await {
      out.push(Candidate {
        uri,
        display: seed.display,
        kind: VoiceTargetKind::Station,
        artist: seed.artist,
        year: None,
      });
    }
  }
  out
}

fn wants_personal_mix(req: &VoiceResolveRequest) -> bool {
  req.target.is_none() && req.popularity_filter.is_none() && (req.genre.is_some() || req.mood.is_some())
}

async fn personal_mix(catalog: &dyn Catalog, label: &str) -> Option<Vec<Candidate>> {
  let mixes = typed(catalog, catalog.personal_mix(label).await?);
  (!mixes.is_empty()).then_some(mixes)
}

fn anchored_kind(req: &VoiceResolveRequest, query: &str) -> Option<VoiceTargetKind> {
  (query.is_empty() && req.popularity_filter.is_none())
    .then_some(req.target_type)
    .flatten()
}

async fn resolve_anchor(catalog: &dyn Catalog, kind: VoiceTargetKind) -> VoiceResult<VoiceResolved> {
  let anchor = catalog.now_playing().await.ok_or(VoiceResolveError::NoAnchorContext)?;
  let found = [
    Some(anchor.track),
    anchor.album,
    anchor.artist.clone(),
    anchor.context.clone(),
  ]
  .into_iter()
  .flatten()
  .find(|uri| catalog.kind_of(uri) == Some(kind));
  let uri = match (found, kind) {
    (Some(uri), _) => uri,
    (None, VoiceTargetKind::Station) => {
      let artist = anchor.artist.ok_or(VoiceResolveError::NoAnchorContext)?;
      catalog
        .station(&artist)
        .await
        .ok_or(VoiceResolveError::NoAnchorContext)?
    }
    (None, _) => return Err(VoiceResolveError::NoAnchorContext),
  };
  let context = matches!(kind, VoiceTargetKind::Track | VoiceTargetKind::Episode)
    .then_some(anchor.context)
    .flatten();
  head_of(hydrated(catalog, &[uri]).await, context)
}

async fn resolve_position(
  catalog: &dyn Catalog,
  req: &VoiceResolveRequest,
  query: &str,
  position: u32,
) -> VoiceResult<VoiceResolved> {
  let context = match target_container(catalog, req, query).await? {
    Some(uri) => uri,
    None => catalog
      .current_context()
      .await
      .ok_or(VoiceResolveError::NoAnchorContext)?,
  };
  let page = catalog
    .container(&context, 1 + ALTERNATIVES as u32, offset_of(position))
    .await?;
  head_of(typed(catalog, page), Some(context))
}

async fn target_container(
  catalog: &dyn Catalog,
  req: &VoiceResolveRequest,
  query: &str,
) -> VoiceResult<Option<String>> {
  if req.target.is_none() {
    return Ok(None);
  }
  let pick = match req.target_type {
    Some(kind) if is_container(kind) => Pick::Kind(kind),
    Some(_) => return Ok(None),
    None => Pick::Container,
  };
  let (library, searched) = futures::join!(library_hits(catalog, req), catalog.search(query, SEARCH_LIMIT));
  if let Some(mine) = library.into_iter().find(|c| pick.keeps(c.kind)) {
    return Ok(Some(mine.uri));
  }
  let items = typed(catalog, searched?);
  let mut ranked = rank(&items, pick);
  if let Some(want) = want_of(req, &items) {
    ranked = scored(ranked, &want);
  }
  Ok(ranked.into_iter().next().map(|c| c.uri))
}

async fn fresh_pick(catalog: &dyn Catalog) -> VoiceResult<VoiceResolved> {
  let (pool, recents, current) = futures::join!(
    catalog.fresh_pool(),
    catalog.recent_contexts(),
    catalog.current_context(),
  );
  let mut excluded: HashSet<String> = recents.into_iter().collect();
  excluded.extend(current);
  let pool = fresh_candidates(catalog, &pool, &excluded);
  if pool.is_empty() {
    return Err(VoiceResolveError::NoMatch);
  }
  let chosen = pool[rand::random_range(0..pool.len())].clone();
  let mut order = vec![chosen.clone()];
  order.extend(pool.iter().filter(|u| **u != chosen).take(ALTERNATIVES).cloned());
  head_of(hydrated(catalog, &order).await, None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
  uri: String,
  display: String,
  kind: VoiceTargetKind,
  artist: Option<String>,
  year: Option<i32>,
}

fn typed(catalog: &dyn Catalog, items: Vec<CatalogItem>) -> Vec<Candidate> {
  items
    .into_iter()
    .filter_map(|i| {
      catalog.kind_of(&i.uri).map(|kind| Candidate {
        uri: i.uri,
        display: i.name,
        kind,
        artist: i.artist,
        year: i.year,
      })
    })
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Want {
  title: String,
  artist: Option<String>,
  years: Option<(i32, i32)>,
  artist_shaped: bool,
}

fn want_of(req: &VoiceResolveRequest, items: &[Candidate]) -> Option<Want> {
  let target = req.target.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
  let whole = norm(target);
  let artists = returned_artists(items);
  let (title, artist) = match whole.rsplit_once(" by ") {
    Some((head, tail)) if !head.is_empty() && artists.contains(tail) => (head.to_string(), Some(tail.to_string())),
    _ => (whole.clone(), None),
  };
  let artist_shaped = artists.contains(&title);
  Some(Want {
    title,
    artist,
    years: req.era.as_deref().and_then(era_years),
    artist_shaped,
  })
}

fn returned_artists(items: &[Candidate]) -> HashSet<String> {
  items
    .iter()
    .flat_map(|c| {
      let named = (c.kind == VoiceTargetKind::Artist).then(|| norm(&c.display));
      let credited = c.artist.as_deref().map(norm);
      [named, credited]
    })
    .flatten()
    .collect()
}

fn title_tier(display: &str, want: &Want) -> u8 {
  let name = norm(display);
  if name == want.title {
    return 4;
  }
  let padded = |s: &str| format!(" {s} ");
  if padded(&name).contains(&padded(&want.title)) {
    return 3;
  }
  if padded(&want.title).contains(&padded(&name)) {
    return 2;
  }
  let words: HashSet<&str> = name.split(' ').collect();
  u8::from(want.title.split(' ').all(|t| words.contains(t)))
}

fn artist_hint(want: &Want) -> Option<&str> {
  want
    .artist
    .as_deref()
    .or(want.artist_shaped.then_some(want.title.as_str()))
}

fn artist_hit(c: &Candidate, want: &Want) -> bool {
  match (&want.artist, &c.artist) {
    (Some(w), Some(a)) => norm(a) == *w,
    _ => false,
  }
}

fn artist_conflict(c: &Candidate, want: &Want) -> bool {
  match (artist_hint(want), &c.artist) {
    (Some(w), Some(a)) => norm(a) != w,
    _ => false,
  }
}

fn year_hit(c: &Candidate, want: &Want) -> bool {
  match (want.years, c.year) {
    (Some((lo, hi)), Some(y)) => (lo..=hi).contains(&y) && !artist_conflict(c, want),
    _ => false,
  }
}

fn scored(mut ranked: Vec<Candidate>, want: &Want) -> Vec<Candidate> {
  ranked.sort_by_key(|c| {
    std::cmp::Reverse((
      year_hit(c, want),
      title_tier(&c.display, want),
      want.artist.is_some() && c.kind != VoiceTargetKind::Artist,
      artist_hit(c, want),
      !live_recording(&c.display),
    ))
  });
  ranked
}

fn needs_discography(ranked: &[Candidate], want: &Want) -> bool {
  if want.years.is_some() && !ranked.iter().any(|c| year_hit(c, want)) {
    return true;
  }
  let best = ranked.first().map(|c| title_tier(&c.display, want)).unwrap_or(0);
  best < 4 && (want.artist.is_some() || want.artist_shaped)
}

async fn with_discography(
  catalog: &dyn Catalog,
  ranked: Vec<Candidate>,
  want: &Want,
  items: &[Candidate],
) -> Vec<Candidate> {
  let artists = rank(items, Pick::Kind(VoiceTargetKind::Artist));
  let anchor = artist_hint(want)
    .and_then(|name| artists.iter().find(|c| norm(&c.display) == name))
    .or(artists.first());
  let Some(anchor) = anchor.cloned() else {
    return ranked;
  };
  let Ok(releases) = catalog
    .artist_releases(&anchor.uri, true, DISCOGRAPHY_LOOKUP_DEPTH)
    .await
  else {
    return ranked;
  };
  merge_releases(ranked, releases, &anchor.display)
}

fn merge_releases(mut ranked: Vec<Candidate>, releases: Vec<Release>, artist: &str) -> Vec<Candidate> {
  let mut fresh: Vec<Candidate> = Vec::new();
  for r in releases {
    match ranked.iter_mut().find(|c| c.uri == r.uri) {
      Some(held) => {
        held.artist.get_or_insert_with(|| artist.to_string());
        held.year.get_or_insert(r.released.0);
      }
      None => fresh.push(released_album(r, artist)),
    }
  }
  ranked.extend(fresh);
  ranked
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
  Kind(VoiceTargetKind),
  Any,
  Container,
  Playlist,
}

impl Pick {
  fn keeps(self, kind: VoiceTargetKind) -> bool {
    match self {
      Pick::Kind(want) => want == kind,
      Pick::Any => true,
      Pick::Container => is_container(kind),
      Pick::Playlist => kind == VoiceTargetKind::Playlist,
    }
  }
}

fn ranked_search(items: &[Candidate], req: &VoiceResolveRequest) -> Vec<Candidate> {
  let pick = match (req.target.is_some(), req.target_type) {
    (_, Some(kind)) => Pick::Kind(kind),
    (true, None) => Pick::Any,
    (false, None) => Pick::Playlist,
  };
  let ranked = rank(items, pick);
  if ranked.is_empty() && pick == Pick::Playlist {
    return rank(items, Pick::Any);
  }
  ranked
}

fn typed_pick(req: &VoiceResolveRequest) -> Pick {
  match req.target_type {
    Some(VoiceTargetKind::Station) | None => Pick::Any,
    Some(kind) => Pick::Kind(kind),
  }
}

fn head_of(ranked: Vec<Candidate>, context_uri: Option<String>) -> VoiceResult<VoiceResolved> {
  let head = ranked.first().cloned().ok_or(VoiceResolveError::NoMatch)?;
  Ok(resolved(head, &ranked[1..], context_uri))
}

fn narrow(candidates: Vec<Candidate>, pick: Pick, query: &str) -> Vec<Candidate> {
  let words: Vec<String> = norm(query).split(' ').map(str::to_string).collect();
  candidates
    .into_iter()
    .filter(|c| pick.keeps(c.kind))
    .filter(|c| {
      let display = norm(&c.display);
      words.iter().all(|w| display.contains(w.as_str()))
    })
    .collect()
}

fn artist_anchor(items: &[Candidate], req: &VoiceResolveRequest) -> Option<Candidate> {
  let target = req.target.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
  let named = norm(target);
  let artists = rank(items, Pick::Kind(VoiceTargetKind::Artist));
  artists
    .iter()
    .find(|c| norm(&c.display) == named)
    .or_else(|| {
      matches!(req.target_type, Some(VoiceTargetKind::Artist | VoiceTargetKind::Album))
        .then(|| artists.first())
        .flatten()
    })
    .cloned()
}

fn truncate(candidates: &mut Vec<Candidate>, depth: Option<usize>) {
  if let Some(depth) = depth {
    candidates.truncate(depth);
  }
}

fn is_container(kind: VoiceTargetKind) -> bool {
  matches!(
    kind,
    VoiceTargetKind::Album | VoiceTargetKind::Artist | VoiceTargetKind::Playlist | VoiceTargetKind::Show
  )
}

fn is_fresh_context(kind: VoiceTargetKind) -> bool {
  matches!(
    kind,
    VoiceTargetKind::Album | VoiceTargetKind::Artist | VoiceTargetKind::Playlist
  )
}

fn compose_query(req: &VoiceResolveRequest) -> String {
  [
    req.era.as_deref(),
    req.mood.as_deref(),
    req.genre.as_deref(),
    req.target.as_deref(),
  ]
  .into_iter()
  .flatten()
  .map(str::trim)
  .filter(|s| !s.is_empty())
  .collect::<Vec<_>>()
  .join(" ")
}

fn rank(items: &[Candidate], pick: Pick) -> Vec<Candidate> {
  items.iter().filter(|c| pick.keeps(c.kind)).cloned().collect()
}

fn fresh_candidates(catalog: &dyn Catalog, pool: &[String], excluded: &HashSet<String>) -> Vec<String> {
  let mut seen = HashSet::new();
  pool
    .iter()
    .filter(|u| catalog.kind_of(u).is_some_and(is_fresh_context) && !catalog.is_liked_songs(u))
    .filter(|u| !excluded.contains(*u))
    .filter(|u| seen.insert((*u).clone()))
    .cloned()
    .collect()
}

fn offset_of(position: u32) -> u32 {
  position.saturating_sub(1)
}

fn resolved(head: Candidate, rest: &[Candidate], context_uri: Option<String>) -> VoiceResolved {
  VoiceResolved {
    uri: head.uri,
    context_uri,
    display: head.display,
    kind: head.kind,
    artist: head.artist,
    year: head.year,
    alternatives: rest
      .iter()
      .take(ALTERNATIVES)
      .map(|c| VoiceAlternative {
        uri: c.uri.clone(),
        display: c.display.clone(),
        kind: c.kind,
      })
      .collect(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    NowPlaying,
    fake::{FakeCatalog, credited, item},
  };

  fn typed_items(items: &[&str]) -> Vec<Candidate> {
    let fake = FakeCatalog::default();
    let named = items
      .iter()
      .map(|uri| item(uri, &uri.rsplit(':').next().unwrap().to_uppercase()))
      .collect();
    typed(&fake, named)
  }

  fn uris(ranked: &[Candidate]) -> Vec<&str> {
    ranked.iter().map(|c| c.uri.as_str()).collect()
  }

  fn req(target: Option<&str>, target_type: Option<VoiceTargetKind>) -> VoiceResolveRequest {
    VoiceResolveRequest {
      target: target.map(str::to_string),
      target_type,
      ..Default::default()
    }
  }

  fn cand(uri: &str, display: &str) -> Candidate {
    Candidate {
      uri: uri.to_string(),
      display: display.to_string(),
      kind: crate::fake::kind_of(uri).expect("a test candidate is playable"),
      artist: None,
      year: None,
    }
  }

  fn release(uri: &str, released: (i32, i32, i32), popularity: i32) -> Release {
    Release {
      uri: uri.to_string(),
      name: uri.rsplit(':').next().unwrap().to_uppercase(),
      released,
      popularity,
    }
  }

  fn named_release(uri: &str, name: &str, released: (i32, i32, i32)) -> Release {
    Release {
      uri: uri.to_string(),
      name: name.to_string(),
      released,
      popularity: 50,
    }
  }

  fn picked(out: &VoiceResolved) -> Vec<&str> {
    std::iter::once(out.uri.as_str())
      .chain(out.alternatives.iter().map(|a| a.uri.as_str()))
      .collect()
  }

  #[test]
  fn a_requested_kind_picks_the_first_item_of_that_kind() {
    let items = typed_items(&["x:track:t1", "x:album:a1", "x:album:a2", "x:artist:r1"]);
    let ranked = rank(&items, Pick::Kind(VoiceTargetKind::Album));
    assert_eq!(
      uris(&ranked),
      ["x:album:a1", "x:album:a2"],
      "a typed pick sees only that kind, still in relevance order"
    );
    assert_eq!(ranked[0].display, "A1");
    assert_eq!(ranked[0].kind, VoiceTargetKind::Album);
  }

  #[test]
  fn an_untyped_pick_keeps_the_global_relevance_order_across_kinds() {
    let items = typed_items(&["x:artist:r1", "x:track:t1", "x:album:a1"]);
    assert_eq!(
      uris(&rank(&items, Pick::Any)),
      ["x:artist:r1", "x:track:t1", "x:album:a1"],
      "bucketing would float the track to the front; the flat order must survive"
    );
  }

  #[test]
  fn unplayable_uris_never_become_candidates() {
    let items = typed_items(&["x:user:nobody", "x:genre:rock", "x:track:t1"]);
    assert_eq!(uris(&items), ["x:track:t1"]);
  }

  #[test]
  fn containers_exclude_leaves() {
    let items = typed_items(&["x:track:t1", "x:episode:e1", "x:playlist:p1", "x:show:s1"]);
    assert_eq!(
      uris(&rank(&items, Pick::Container)),
      ["x:playlist:p1", "x:show:s1"],
      "a position counts into a container, never into a track"
    );
  }

  #[test]
  fn alternatives_are_the_next_ranked_candidates_and_are_capped() {
    let ranked = typed_items(&[
      "x:track:t1",
      "x:track:t2",
      "x:track:t3",
      "x:track:t4",
      "x:track:t5",
      "x:track:t6",
    ]);
    let out = resolved(ranked[0].clone(), &ranked[1..], None);
    assert_eq!(out.uri, "x:track:t1");
    assert_eq!(out.context_uri, None, "a track carries no context of its own");
    assert_eq!(
      out.alternatives.iter().map(|a| a.uri.as_str()).collect::<Vec<_>>(),
      ["x:track:t2", "x:track:t3", "x:track:t4", "x:track:t5"]
    );
  }

  #[test]
  fn a_container_pick_is_its_own_context() {
    let ranked = rank(&typed_items(&["x:album:a1"]), Pick::Kind(VoiceTargetKind::Album));
    let out = resolved(ranked[0].clone(), &[], None);
    assert_eq!(out.uri, "x:album:a1");
    assert_eq!(
      out.context_uri, None,
      "the album uri is the context; it is not repeated"
    );
  }

  #[test]
  fn positions_are_one_based_offsets() {
    assert_eq!(offset_of(1), 0);
    assert_eq!(offset_of(3), 2);
    assert_eq!(offset_of(0), 0, "a zeroth item is the first item, not an underflow");
  }

  #[test]
  fn query_composition_reads_era_mood_genre_then_target() {
    let composed = |era, mood, genre, target: Option<&str>| {
      compose_query(&VoiceResolveRequest {
        era: Option::<&str>::map(era, str::to_string),
        mood: Option::<&str>::map(mood, str::to_string),
        genre: Option::<&str>::map(genre, str::to_string),
        target: target.map(str::to_string),
        ..Default::default()
      })
    };
    assert_eq!(composed(Some("80s"), None, Some("rock"), None), "80s rock");
    assert_eq!(composed(None, Some("chill"), Some("jazz"), None), "chill jazz");
    assert_eq!(composed(Some("90s"), None, None, Some("radiohead")), "90s radiohead");
    assert_eq!(composed(None, None, None, Some(" daft punk ")), "daft punk");
    assert_eq!(composed(None, None, None, None), "");
  }

  #[test]
  fn a_fresh_pick_drops_recents_and_whatever_is_playing() {
    let pool: Vec<String> = ["x:playlist:p1", "x:playlist:p2", "x:album:a1", "x:playlist:p3"]
      .iter()
      .map(|s| s.to_string())
      .collect();
    let excluded: HashSet<String> = ["x:playlist:p2", "x:playlist:p3"]
      .iter()
      .map(|s| s.to_string())
      .collect();
    assert_eq!(
      fresh_candidates(&FakeCatalog::default(), &pool, &excluded),
      ["x:playlist:p1", "x:album:a1"],
      "a fresh pick can never resume the current context or replay a recent one"
    );
  }

  #[test]
  fn a_fresh_pick_only_considers_playable_music_contexts() {
    let pool: Vec<String> = [
      "x:track:t1",
      "x:episode:e1",
      "x:show:s1",
      "x:liked:me",
      "x:artist:r1",
      "x:playlist:p1",
      "x:playlist:p1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
      fresh_candidates(&FakeCatalog::default(), &pool, &HashSet::new()),
      ["x:artist:r1", "x:playlist:p1"],
      "leaves, shows, the liked-songs collection and duplicates are not fresh contexts"
    );
  }

  #[test]
  fn a_station_never_seeds_off_a_kind_the_catalog_cannot_seed() {
    let fake = FakeCatalog::searching(Vec::new());
    let items = typed_items(&["x:show:s1", "x:episode:e1", "x:album:a1"]);
    assert_eq!(uris(&station_seeds(&fake, &items, "q")), ["x:album:a1"]);
    let narrow = FakeCatalog {
      station_kinds: vec![VoiceTargetKind::Artist],
      ..FakeCatalog::searching(Vec::new())
    };
    let items = typed_items(&["x:album:a1", "x:artist:r1"]);
    assert_eq!(uris(&station_seeds(&narrow, &items, "q")), ["x:artist:r1"]);
  }

  #[test]
  fn a_station_prefers_the_artist_over_a_same_named_playlist() {
    let fake = FakeCatalog::searching(Vec::new());
    let items = typed_items(&["x:playlist:p1", "x:track:t1", "x:artist:r1"]);
    assert_eq!(
      uris(&station_seeds(&fake, &items, "R1")),
      ["x:artist:r1", "x:playlist:p1", "x:track:t1"],
      "search floats the editorial radio playlist first; the artist itself is the better seed"
    );
  }

  #[test]
  fn a_station_matches_the_artist_name_against_the_target_not_the_modifiers() {
    let fake = FakeCatalog::searching(Vec::new());
    let items = typed_items(&["x:playlist:p1", "x:artist:r1"]);
    let composed = compose_query(&VoiceResolveRequest {
      era: Some("80s".into()),
      target: Some("r1".into()),
      target_type: Some(VoiceTargetKind::Station),
      ..Default::default()
    });
    assert_eq!(composed, "80s r1");
    assert_eq!(
      station_seeds(&fake, &items, "r1").first().map(|c| c.uri.as_str()),
      Some("x:artist:r1"),
      "an era modifier must not stop the artist name from matching"
    );
    assert_eq!(
      station_seeds(&fake, &items, &composed).first().map(|c| c.uri.as_str()),
      Some("x:playlist:p1"),
      "matching on the composed query is what the target-slot match avoids"
    );
  }

  #[test]
  fn a_station_falls_back_to_relevance_when_no_artist_carries_the_query_name() {
    let fake = FakeCatalog::searching(Vec::new());
    let items = typed_items(&["x:track:t1", "x:artist:r1"]);
    assert_eq!(
      station_seeds(&fake, &items, "bohemian rhapsody")
        .first()
        .map(|c| c.uri.as_str()),
      Some("x:track:t1"),
      "naming a song must still seed a track station"
    );
  }

  #[tokio::test]
  async fn a_station_the_catalog_returns_is_played_as_is_and_unseedable_hits_are_skipped() {
    let fake = FakeCatalog {
      station_kinds: vec![VoiceTargetKind::Artist],
      ..FakeCatalog::searching(vec![
        item("x:album:a1", "Rock Album"),
        item("x:station:s1", "Rock Station"),
        item("x:artist:r1", "Rocker"),
      ])
    };
    let out = resolve(&fake, req(Some("rock"), Some(VoiceTargetKind::Station)))
      .await
      .unwrap();
    assert_eq!(picked(&out), ["x:station:s1", "x:station:artist:r1"]);
    assert_eq!(out.kind, VoiceTargetKind::Station);
  }

  #[tokio::test]
  async fn a_catalog_without_stations_answers_a_station_request_with_no_match() {
    let fake = FakeCatalog {
      stations: false,
      ..FakeCatalog::searching(vec![item("x:artist:r1", "R1")])
    };
    let err = resolve(&fake, req(Some("r1"), Some(VoiceTargetKind::Station)))
      .await
      .unwrap_err();
    assert!(matches!(err, VoiceResolveError::NoMatch), "{err:?}");
  }

  #[test]
  fn only_the_counted_filters_bound_how_deep_a_ranking_is_read() {
    assert_eq!(VoicePopularity::Top5.depth(), Some(5));
    assert_eq!(VoicePopularity::Top10.depth(), Some(10));
    assert_eq!(
      VoicePopularity::Popular.depth(),
      None,
      "an uncounted filter reads the whole ranking"
    );
  }

  #[test]
  fn a_depth_bounds_the_pool_the_alternatives_come_from() {
    let mut pool: Vec<Candidate> = (1..=8).map(|n| cand(&format!("x:track:t{n}"), "T")).collect();
    truncate(&mut pool, Some(5));
    assert_eq!(pool.len(), 5);
    let mut whole: Vec<Candidate> = (1..=8).map(|n| cand(&format!("x:track:t{n}"), "T")).collect();
    truncate(&mut whole, None);
    assert_eq!(whole.len(), 8, "no depth keeps every candidate");
  }

  #[test]
  fn a_filtered_pick_narrows_by_kind_but_never_by_station() {
    assert_eq!(typed_pick(&req(None, None)), Pick::Any);
    assert_eq!(
      typed_pick(&req(None, Some(VoiceTargetKind::Album))),
      Pick::Kind(VoiceTargetKind::Album)
    );
    assert_eq!(
      typed_pick(&req(None, Some(VoiceTargetKind::Station))),
      Pick::Any,
      "search never returns a station uri, so narrowing to one would empty every ranking"
    );
  }

  #[test]
  fn history_keeps_only_entries_carrying_every_word_that_was_spoken() {
    let history = vec![
      cand("x:playlist:p1", "Deep Focus"),
      cand("x:album:a1", "Deep Cuts"),
      cand("x:track:t1", "Focus Deep Down"),
    ];
    assert_eq!(
      uris(&narrow(history.clone(), Pick::Any, "deep focus")),
      ["x:playlist:p1", "x:track:t1"],
      "word order is not position; both entries carry both words, in relevance order"
    );
    assert_eq!(
      uris(&narrow(history.clone(), Pick::Kind(VoiceTargetKind::Track), "deep")),
      ["x:track:t1"],
      "a named kind still narrows the history"
    );
    assert_eq!(
      narrow(history, Pick::Any, "").len(),
      3,
      "nothing spoken keeps the whole history"
    );
  }

  #[test]
  fn the_named_artist_outranks_relevance_as_the_anchor_of_a_filter() {
    let items = typed_items(&["x:artist:r1", "x:artist:r2", "x:track:t1"]);
    assert_eq!(
      artist_anchor(&items, &req(Some("r2"), None)).map(|c| c.uri),
      Some("x:artist:r2".to_string()),
      "the artist the user named is the anchor even when search floats another first"
    );
    assert_eq!(
      artist_anchor(&items, &req(Some("bohemian rhapsody"), None)),
      None,
      "an untyped name that matches no artist is not silently read as one"
    );
    assert_eq!(
      artist_anchor(&items, &req(Some("bohemian rhapsody"), Some(VoiceTargetKind::Album))).map(|c| c.uri),
      Some("x:artist:r1".to_string()),
      "asking for an album by a name takes the best artist even without an exact match"
    );
    assert_eq!(
      artist_anchor(&items, &req(None, Some(VoiceTargetKind::Artist))),
      None,
      "no name means no anchor, whatever the kind"
    );
  }

  #[test]
  fn the_first_release_is_the_oldest_date_and_the_canonical_cut_of_it() {
    let first = earliest_first(
      vec![
        release("x:album:newer", (2015, 5, 17), 90),
        release("x:album:debut", (2009, 12, 29), 55),
        release("x:album:debutlive", (2009, 12, 29), 12),
      ],
      "Some Artist",
    );
    assert_eq!(
      uris(&first),
      ["x:album:debut", "x:album:debutlive", "x:album:newer"],
      "a same-day sibling is a cut of one release; popularity picks the canonical one"
    );
    assert_eq!(first[0].kind, VoiceTargetKind::Album);
    assert_eq!(first[0].artist.as_deref(), Some("Some Artist"));
    assert_eq!(first[0].year, Some(2009));
  }

  #[test]
  fn the_latest_release_is_the_newest_date_and_the_canonical_cut_of_it() {
    let latest = latest_first(
      vec![
        release("x:album:deluxe", (2026, 5, 15), 84),
        release("x:album:flagship", (2026, 5, 15), 96),
        release("x:album:older", (2025, 2, 14), 99),
      ],
      "Some Artist",
    );
    assert_eq!(
      uris(&latest),
      ["x:album:flagship", "x:album:deluxe", "x:album:older"],
      "a same-day sibling is a cut of one release; popularity picks the canonical one"
    );
    assert_eq!(latest[0].kind, VoiceTargetKind::Album);
  }

  #[test]
  fn a_live_recording_never_outranks_a_studio_release_by_date_alone() {
    let releases = vec![
      named_release(
        "x:album:live",
        "More Than We Ever Imagined (Live in Mexico City)",
        (2026, 3, 1),
      ),
      named_release("x:album:studio", "Breach", (2025, 9, 12)),
      named_release("x:album:debutlive", "Early Days Live", (2008, 1, 1)),
      named_release("x:album:debut", "Twenty One Pilots", (2009, 12, 29)),
    ];
    assert_eq!(
      uris(&latest_first(releases.clone(), "A")),
      ["x:album:studio", "x:album:debut", "x:album:live", "x:album:debutlive"],
      "the newest album is the newest studio album; live cuts sit after every studio release"
    );
    assert_eq!(
      earliest_first(releases, "A").first().map(|c| c.uri.as_str()),
      Some("x:album:debut"),
      "the debut is the earliest studio release, not an earlier live tape"
    );
  }

  #[test]
  fn popularity_reorders_the_hits_and_sinks_the_kinds_that_carry_no_score() {
    let ranked = vec![
      cand("x:track:t1", "T1"),
      cand("x:playlist:p1", "P1"),
      cand("x:track:t2", "T2"),
      cand("x:playlist:p2", "P2"),
    ];
    let scores = HashMap::from([("x:track:t1".to_string(), 41), ("x:track:t2".to_string(), 84)]);
    assert_eq!(
      uris(&rank_by(ranked, &scores, None)),
      ["x:track:t2", "x:track:t1", "x:playlist:p1", "x:playlist:p2"],
      "playlists have no popularity field; they sink together and keep relevance order"
    );
  }

  #[tokio::test]
  async fn a_catalog_without_popularity_keeps_relevance_order() {
    let fake = FakeCatalog::searching(vec![item("x:track:t1", "Song One"), item("x:track:t2", "Song Two")]);
    let out = resolve(
      &fake,
      VoiceResolveRequest {
        target: Some("song".into()),
        popularity_filter: Some(VoicePopularity::Popular),
        ..Default::default()
      },
    )
    .await
    .unwrap();
    assert_eq!(picked(&out), ["x:track:t1", "x:track:t2"]);
  }

  #[test]
  fn discography_releases_fill_gaps_and_extend_without_displacing_search_order() {
    let ranked = vec![cand("x:album:a1", "Breach")];
    let merged = merge_releases(
      ranked,
      vec![
        release("x:album:a1", (2025, 9, 12), 90),
        release("x:album:a2", (2009, 12, 29), 40),
      ],
      "Twenty One Pilots",
    );
    assert_eq!(
      uris(&merged),
      ["x:album:a1", "x:album:a2"],
      "search hits keep their slot; unseen releases append"
    );
    assert_eq!(merged[0].year, Some(2025), "a search hit gains the year it lacked");
    assert_eq!(merged[0].artist.as_deref(), Some("Twenty One Pilots"));
    assert_eq!(merged[1].year, Some(2009));
  }

  #[tokio::test]
  async fn a_catalog_without_new_releases_falls_back_to_a_fresh_pick_or_the_search() {
    let fake = FakeCatalog {
      pool: vec!["x:playlist:mine".into()],
      ..FakeCatalog::searching(vec![item("x:album:a1", "Jazz Album")])
    };
    let bare = resolve(
      &fake,
      VoiceResolveRequest {
        popularity_filter: Some(VoicePopularity::New),
        ..Default::default()
      },
    )
    .await
    .unwrap();
    assert_eq!(bare.uri, "x:playlist:mine");
    let genre = resolve(
      &fake,
      VoiceResolveRequest {
        genre: Some("jazz".into()),
        popularity_filter: Some(VoicePopularity::New),
        ..Default::default()
      },
    )
    .await
    .unwrap();
    assert_eq!(genre.uri, "x:album:a1");
  }

  #[tokio::test]
  async fn a_bare_station_with_no_station_context_seeds_from_the_playing_artist() {
    let fake = FakeCatalog {
      playing: Some(NowPlaying {
        track: "x:track:t1".into(),
        artist: Some("x:artist:r1".into()),
        context: Some("x:playlist:p1".into()),
        ..Default::default()
      }),
      ..FakeCatalog::searching(Vec::new())
    };
    let out = resolve(&fake, req(None, Some(VoiceTargetKind::Station))).await.unwrap();
    assert_eq!(out.uri, "x:station:artist:r1");
  }

  #[tokio::test]
  async fn the_users_collections_never_reach_the_search() {
    let fake = FakeCatalog {
      liked: Some(item("x:liked:me", "Liked Songs")),
      saved_albums: vec![
        credited("x:album:newest", "Newest", "A", None),
        credited("x:album:older", "Older", "B", None),
      ],
      ..FakeCatalog::searching(vec![item("x:playlist:stranger", "Liked Songs")])
    };
    let liked = resolve(&fake, req(Some("my liked songs"), None)).await.unwrap();
    assert_eq!(liked.uri, "x:liked:me");
    let albums = resolve(&fake, req(Some("my albums"), None)).await.unwrap();
    assert_eq!(picked(&albums), ["x:album:newest", "x:album:older"]);
    assert!(fake.asked().is_empty(), "{:?}", fake.asked());
  }
}
