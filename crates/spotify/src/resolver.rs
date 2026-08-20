use std::collections::{HashMap, HashSet};

use crate::{
  client::{FlatItem, Release, SpotifyClient},
  error::Error,
  model::BrowseItem,
};

pub type VoiceResult<T> = std::result::Result<T, VoiceResolveError>;

const SEARCH_LIMIT: u32 = 20;
const ALTERNATIVES: usize = 4;
const NEW_RELEASES_TAG: &str = "tag:new";
const CHART_QUERY: &str = "top hits";
const DISCOGRAPHY_DEPTH: usize = 8;
const DISCOGRAPHY_LOOKUP_DEPTH: usize = 24;
const DISCOGRAPHY_FULL_DEPTH: usize = 60;
const RECENT_POOL: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceTargetKind {
  Track,
  Album,
  Artist,
  Playlist,
  Show,
  Episode,
  Station,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicePopularity {
  Top5,
  Top10,
  Popular,
  Recent,
  New,
  First,
  Random,
}

#[derive(Debug, Clone, Default)]
pub struct VoiceResolveRequest {
  pub target: Option<String>,
  pub target_type: Option<VoiceTargetKind>,
  pub mood: Option<String>,
  pub genre: Option<String>,
  pub era: Option<String>,
  pub popularity_filter: Option<VoicePopularity>,
  pub position: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceAlternative {
  pub uri: String,
  pub display: String,
  pub kind: VoiceTargetKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceResolved {
  pub uri: String,
  pub context_uri: Option<String>,
  pub display: String,
  pub kind: VoiceTargetKind,
  pub artist: Option<String>,
  pub year: Option<i32>,
  pub alternatives: Vec<VoiceAlternative>,
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceResolveError {
  #[error("nothing matched the request")]
  NoMatch,
  #[error("a position needs a context and nothing is playing")]
  NoAnchorContext,
  #[error("{0}")]
  Spotify(#[from] Error),
}

pub(crate) async fn resolve(client: &SpotifyClient, req: VoiceResolveRequest) -> VoiceResult<VoiceResolved> {
  let query = compose_query(&req);
  if req.target_type == Some(VoiceTargetKind::Station) && !query.is_empty() {
    if wants_personal_mix(&req)
      && let Some(mixes) = personal_mix(client, &query).await
    {
      return head_of(mixes, None);
    }
    return resolve_station(client, &query, req.target.as_deref()).await;
  }
  if let Some(position) = req.position {
    return resolve_position(client, &req, &query, position).await;
  }
  if let Some(kind) = anchored_kind(&req, &query) {
    return resolve_anchor(client, kind).await;
  }
  match req.popularity_filter {
    Some(VoicePopularity::Random) => resolve_random(client, &req, &query).await,
    Some(VoicePopularity::Recent) => resolve_recent(client, &req, &query).await,
    Some(VoicePopularity::New) => resolve_new(client, &req, &query).await,
    Some(VoicePopularity::First) => resolve_first(client, &req, &query).await,
    Some(filter) => resolve_popular(client, &req, &query, filter.depth()).await,
    None => resolve_search(client, &req, &query).await,
  }
}

async fn resolve_first(client: &SpotifyClient, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    let anchor = client
      .playback_anchor()
      .await
      .ok_or(VoiceResolveError::NoAnchorContext)?;
    let artist_uri = anchor.artist_uri.ok_or(VoiceResolveError::NoAnchorContext)?;
    let named = candidates_of(client.hydrate_uris(std::slice::from_ref(&artist_uri)).await);
    let display = named.first().map(|c| c.display.clone()).unwrap_or_default();
    return first_of_discography(client, &artist_uri, &display, req).await;
  }
  let items = client.search_flat(query, SEARCH_LIMIT).await?;
  if let Some(artist) = artist_anchor(&items, req)
    && let Ok(out) = first_of_discography(client, &artist.uri, &artist.display, req).await
  {
    return Ok(out);
  }
  resolve_search(client, req, query).await
}

async fn first_of_discography(
  client: &SpotifyClient,
  artist_uri: &str,
  display: &str,
  req: &VoiceResolveRequest,
) -> VoiceResult<VoiceResolved> {
  let albums_only = req.target_type == Some(VoiceTargetKind::Album);
  let releases = client
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
  releases
    .into_iter()
    .map(|r| Candidate {
      uri: r.uri,
      display: r.name,
      kind: VoiceTargetKind::Album,
      artist: Some(artist.to_string()),
      year: Some(r.released.0),
    })
    .collect()
}

async fn resolve_search(client: &SpotifyClient, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    return Err(VoiceResolveError::NoMatch);
  }
  if wants_personal_mix(req)
    && matches!(req.target_type, None | Some(VoiceTargetKind::Playlist))
    && let Some(mixes) = personal_mix(client, query).await
  {
    return head_of(mixes, None);
  }
  let items = client.search_flat(query, SEARCH_LIMIT).await?;
  let mut ranked = ranked_search(&items, req);
  if let Some(want) = want_of(req, &items) {
    ranked = scored(ranked, &want);
    if req.target_type == Some(VoiceTargetKind::Album) && needs_discography(&ranked, &want) {
      ranked = scored(with_discography(client, ranked, &want, &items).await, &want);
    }
  }
  head_of(ranked, None)
}

async fn resolve_random(client: &SpotifyClient, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    return fresh_pick(client).await;
  }
  let items = client.search_flat(query, SEARCH_LIMIT).await?;
  let mut ranked = ranked_search(&items, req);
  if ranked.is_empty() {
    return fresh_pick(client).await;
  }
  let chosen = rand::random_range(0..ranked.len());
  ranked.rotate_left(chosen);
  head_of(ranked, None)
}

async fn resolve_recent(client: &SpotifyClient, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  let pool = recent_pool(client).await;
  if !pool.is_empty() {
    let named: Vec<Candidate> = candidates_of(client.hydrate_uris(&pool).await);
    let matched = narrow(named, typed_pick(req), query);
    if let Some(head) = matched.first().cloned() {
      return Ok(resolved(head, &matched[1..], None));
    }
  }
  if query.is_empty() {
    return fresh_pick(client).await;
  }
  resolve_search(client, req, query).await
}

async fn resolve_new(client: &SpotifyClient, req: &VoiceResolveRequest, query: &str) -> VoiceResult<VoiceResolved> {
  if !query.is_empty() && req.target.is_some() {
    let items = client.search_flat(query, SEARCH_LIMIT).await?;
    if let Some(artist) = artist_anchor(&items, req) {
      let albums_only = req.target_type == Some(VoiceTargetKind::Album);
      let releases = client
        .artist_releases(&artist.uri, albums_only, DISCOGRAPHY_DEPTH)
        .await?;
      let latest = latest_first(releases, &artist.display);
      if let Some(head) = latest.first().cloned() {
        return Ok(resolved(head, &latest[1..], Some(artist.uri)));
      }
    }
  }
  let items = client
    .search_flat(&tagged(query, NEW_RELEASES_TAG), SEARCH_LIMIT)
    .await?;
  let ranked = rank(&items, typed_pick(req));
  if let Some(head) = ranked.first().cloned() {
    return Ok(resolved(head, &ranked[1..], None));
  }
  if query.is_empty() {
    return fresh_pick(client).await;
  }
  resolve_search(client, req, query).await
}

async fn resolve_popular(
  client: &SpotifyClient,
  req: &VoiceResolveRequest,
  query: &str,
  depth: Option<usize>,
) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    return chart_pick(client, depth).await;
  }
  let items = client.search_flat(query, SEARCH_LIMIT).await?;
  if let Some(artist) = artist_anchor(&items, req) {
    let page = client
      .browse_container(&artist.uri, depth.unwrap_or(ALTERNATIVES + 1) as u32, 0)
      .await?;
    let mut top = candidates_of(page.items);
    truncate(&mut top, depth);
    if let Some(head) = top.first().cloned() {
      return Ok(resolved(head, &top[1..], Some(artist.uri)));
    }
  }
  let ranked = by_popularity(client, ranked_search(&items, req), depth).await;
  if let Some(head) = ranked.first().cloned() {
    return Ok(resolved(head, &ranked[1..], None));
  }
  chart_pick(client, depth).await
}

async fn chart_pick(client: &SpotifyClient, depth: Option<usize>) -> VoiceResult<VoiceResolved> {
  let items = client.search_flat(CHART_QUERY, SEARCH_LIMIT).await?;
  let mut ranked = rank(&items, Pick::Playlist);
  if ranked.is_empty() {
    ranked = rank(&items, Pick::Any);
  }
  truncate(&mut ranked, depth);
  match ranked.first().cloned() {
    Some(head) => Ok(resolved(head, &ranked[1..], None)),
    None => fresh_pick(client).await,
  }
}

async fn by_popularity(client: &SpotifyClient, ranked: Vec<Candidate>, depth: Option<usize>) -> Vec<Candidate> {
  if ranked.len() < 2 {
    return ranked;
  }
  let uris: Vec<String> = ranked.iter().map(|c| c.uri.clone()).collect();
  let scores = client.popularity_of(&uris).await;
  rank_by(ranked, &scores, depth)
}

fn rank_by(ranked: Vec<Candidate>, scores: &HashMap<String, i32>, depth: Option<usize>) -> Vec<Candidate> {
  let mut out = ranked;
  out.sort_by_key(|c| std::cmp::Reverse(scores.get(&c.uri).copied().unwrap_or(0)));
  truncate(&mut out, depth);
  out
}

async fn recent_pool(client: &SpotifyClient) -> Vec<String> {
  let (contexts, tracks) = tokio::join!(client.recent_context_uris(), client.recent_track_uris());
  let mut seen = HashSet::new();
  contexts
    .unwrap_or_default()
    .into_iter()
    .chain(tracks.unwrap_or_default())
    .filter(|u| kind_of_uri(u).is_some())
    .filter(|u| seen.insert(u.clone()))
    .take(RECENT_POOL)
    .collect()
}

async fn resolve_station(client: &SpotifyClient, query: &str, target: Option<&str>) -> VoiceResult<VoiceResolved> {
  if query.is_empty() {
    return Err(VoiceResolveError::NoMatch);
  }
  let items = client.search_flat(query, SEARCH_LIMIT).await?;
  let named = target.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(query);
  let seeds: Vec<Candidate> = station_seeds(&items, named).iter().map(as_station).collect();
  let head = seeds.first().cloned().ok_or(VoiceResolveError::NoMatch)?;
  Ok(resolved(head, &seeds[1..], None))
}

fn station_seeds(items: &[FlatItem], named: &str) -> Vec<Candidate> {
  let named = norm(named);
  let mut seeds = rank(items, Pick::Seed);
  let named_artist = seeds
    .iter()
    .position(|c| c.kind == VoiceTargetKind::Artist && norm(&c.display) == named);
  if let Some(idx) = named_artist {
    seeds[..=idx].rotate_right(1);
  }
  seeds
}

fn station_uri(seed: &str) -> String {
  seed.replacen("spotify:", "spotify:station:", 1)
}

fn as_station(seed: &Candidate) -> Candidate {
  Candidate {
    uri: station_uri(&seed.uri),
    display: seed.display.clone(),
    kind: VoiceTargetKind::Station,
    artist: seed.artist.clone(),
    year: None,
  }
}

const MADE_FOR_YOU_PREFIX: &str = "spotify:playlist:37i9dQZF1E";

pub fn made_for_you(uri: &str) -> bool {
  uri.starts_with(MADE_FOR_YOU_PREFIX)
}

fn wants_personal_mix(req: &VoiceResolveRequest) -> bool {
  req.target.is_none() && req.popularity_filter.is_none() && (req.genre.is_some() || req.mood.is_some())
}

async fn personal_mix(client: &SpotifyClient, label: &str) -> Option<Vec<Candidate>> {
  let want = format!("{} mix", norm(label));
  let items = client.search_flat(&format!("{label} mix"), SEARCH_LIMIT).await.ok()?;
  let mut ranked = rank(&items, Pick::Playlist);
  let mine = ranked
    .iter()
    .position(|c| made_for_you(&c.uri) && norm(&c.display) == want)?;
  ranked[..=mine].rotate_right(1);
  Some(ranked)
}

fn anchored_kind(req: &VoiceResolveRequest, query: &str) -> Option<VoiceTargetKind> {
  (query.is_empty() && req.popularity_filter.is_none())
    .then_some(req.target_type)
    .flatten()
}

async fn resolve_anchor(client: &SpotifyClient, kind: VoiceTargetKind) -> VoiceResult<VoiceResolved> {
  let anchor = client
    .playback_anchor()
    .await
    .ok_or(VoiceResolveError::NoAnchorContext)?;
  let found = [
    Some(anchor.track_uri),
    anchor.album_uri,
    anchor.artist_uri.clone(),
    anchor.context_uri.clone(),
  ]
  .into_iter()
  .flatten()
  .find(|uri| kind_of_uri(uri) == Some(kind));
  let uri = match (found, kind) {
    (Some(uri), _) => uri,
    (None, VoiceTargetKind::Station) => station_uri(&anchor.artist_uri.ok_or(VoiceResolveError::NoAnchorContext)?),
    (None, _) => return Err(VoiceResolveError::NoAnchorContext),
  };
  let context = matches!(kind, VoiceTargetKind::Track | VoiceTargetKind::Episode)
    .then_some(anchor.context_uri)
    .flatten();
  head_of(candidates_of(client.hydrate_uris(&[uri]).await), context)
}

async fn resolve_position(
  client: &SpotifyClient,
  req: &VoiceResolveRequest,
  query: &str,
  position: u32,
) -> VoiceResult<VoiceResolved> {
  let context = match target_container(client, req, query).await? {
    Some(uri) => uri,
    None => client
      .current_context_uri()
      .await
      .ok_or(VoiceResolveError::NoAnchorContext)?,
  };
  let offset = offset_of(position);
  let page = client
    .browse_container(&context, 1 + ALTERNATIVES as u32, offset)
    .await?;
  head_of(candidates_of(page.items), Some(context))
}

async fn target_container(
  client: &SpotifyClient,
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
  let items = client.search_flat(query, SEARCH_LIMIT).await?;
  let mut ranked = rank(&items, pick);
  if let Some(want) = want_of(req, &items) {
    ranked = scored(ranked, &want);
  }
  Ok(ranked.into_iter().next().map(|c| c.uri))
}

async fn fresh_pick(client: &SpotifyClient) -> VoiceResult<VoiceResolved> {
  let (playlists, home, recents, current) = tokio::join!(
    client.playlist_uris(),
    client.home_uris(),
    client.recent_context_uris(),
    client.current_context_uri(),
  );
  let mut pool = playlists.unwrap_or_default();
  pool.extend(home);
  let mut excluded: HashSet<String> = recents.unwrap_or_default().into_iter().collect();
  excluded.extend(current);
  let pool = fresh_candidates(&pool, &excluded);
  if pool.is_empty() {
    return Err(VoiceResolveError::NoMatch);
  }
  let chosen = pool[rand::random_range(0..pool.len())].clone();
  let mut order = vec![chosen.clone()];
  order.extend(pool.iter().filter(|u| **u != chosen).take(ALTERNATIVES).cloned());
  head_of(candidates_of(client.hydrate_uris(&order).await), None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
  uri: String,
  display: String,
  kind: VoiceTargetKind,
  artist: Option<String>,
  year: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Want {
  title: String,
  artist: Option<String>,
  years: Option<(i32, i32)>,
  artist_shaped: bool,
}

fn want_of(req: &VoiceResolveRequest, items: &[FlatItem]) -> Option<Want> {
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

fn returned_artists(items: &[FlatItem]) -> HashSet<String> {
  items
    .iter()
    .flat_map(|i| {
      let named = (kind_of_uri(&i.uri) == Some(VoiceTargetKind::Artist)).then(|| norm(&i.name));
      let credited = i.artist.as_deref().map(norm);
      [named, credited]
    })
    .flatten()
    .collect()
}

pub fn norm(s: &str) -> String {
  let lowered = s.to_lowercase().replace('&', " and ");
  let tokens = lowered.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty());
  merge_number_words(tokens).join(" ")
}

fn number_word(token: &str) -> Option<u32> {
  let units = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
  ];
  let tens = [
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
  ];
  if let Some(n) = units.iter().position(|u| *u == token) {
    return Some(n as u32);
  }
  tens.iter().position(|t| *t == token).map(|n| (n as u32 + 2) * 10)
}

fn merge_number_words<'a>(tokens: impl Iterator<Item = &'a str>) -> Vec<String> {
  let mut out: Vec<String> = Vec::new();
  let mut pending_tens: Option<u32> = None;
  for token in tokens {
    match (pending_tens.take(), number_word(token)) {
      (Some(tens), Some(unit)) if (1..=9).contains(&unit) => out.push((tens + unit).to_string()),
      (carried, spoken) => {
        if let Some(tens) = carried {
          out.push(tens.to_string());
        }
        match spoken {
          Some(n) if n >= 20 && n % 10 == 0 => pending_tens = Some(n),
          Some(n) => out.push(n.to_string()),
          None => out.push(token.to_string()),
        }
      }
    }
  }
  if let Some(tens) = pending_tens {
    out.push(tens.to_string());
  }
  out
}

fn era_years(era: &str) -> Option<(i32, i32)> {
  let normalized = norm(era);
  let e = normalized.strip_prefix("the ").unwrap_or(&normalized);
  let e = match e {
    "sixties" => "60s",
    "seventies" => "70s",
    "eighties" => "80s",
    "nineties" => "90s",
    other => other,
  };
  if let Ok(year) = e.parse::<i32>() {
    return (1900..=2099).contains(&year).then_some((year, year));
  }
  let decade = e.strip_suffix('s')?.parse::<i32>().ok()?;
  let base = match decade {
    0..=29 => 2000 + decade,
    30..=99 => 1900 + decade,
    1900..=2090 => decade,
    _ => return None,
  };
  (base % 10 == 0).then_some((base, base + 9))
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
  client: &SpotifyClient,
  ranked: Vec<Candidate>,
  want: &Want,
  items: &[FlatItem],
) -> Vec<Candidate> {
  let artists = rank(items, Pick::Kind(VoiceTargetKind::Artist));
  let named = want
    .artist
    .as_deref()
    .or(want.artist_shaped.then_some(want.title.as_str()));
  let anchor = named
    .and_then(|name| artists.iter().find(|c| norm(&c.display) == name))
    .or(artists.first());
  let Some(anchor) = anchor.cloned() else {
    return ranked;
  };
  let Ok(releases) = client
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
      None => fresh.push(Candidate {
        uri: r.uri,
        display: r.name,
        kind: VoiceTargetKind::Album,
        artist: Some(artist.to_string()),
        year: Some(r.released.0),
      }),
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
  Seed,
}

impl Pick {
  fn keeps(self, kind: VoiceTargetKind) -> bool {
    match self {
      Pick::Kind(want) => want == kind,
      Pick::Any => true,
      Pick::Container => is_container(kind),
      Pick::Playlist => kind == VoiceTargetKind::Playlist,
      Pick::Seed => is_station_seed(kind),
    }
  }
}

impl VoicePopularity {
  fn depth(self) -> Option<usize> {
    match self {
      VoicePopularity::Top5 => Some(5),
      VoicePopularity::Top10 => Some(10),
      _ => None,
    }
  }
}

fn ranked_search(items: &[FlatItem], req: &VoiceResolveRequest) -> Vec<Candidate> {
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

fn candidates_of(items: Vec<BrowseItem>) -> Vec<Candidate> {
  items
    .into_iter()
    .filter_map(|i| {
      kind_of_uri(&i.uri).map(|kind| Candidate {
        artist: i.artists.first().map(|a| a.name.clone()),
        uri: i.uri,
        display: i.title,
        kind,
        year: None,
      })
    })
    .collect()
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

fn artist_anchor(items: &[FlatItem], req: &VoiceResolveRequest) -> Option<Candidate> {
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

fn live_recording(name: &str) -> bool {
  let n = norm(name);
  let padded = format!(" {n} ");
  ["live at", "live in", "live from"]
    .iter()
    .any(|p| padded.contains(&format!(" {p} ")))
    || n.ends_with(" live")
    || padded.contains(" unplugged ")
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
  releases
    .into_iter()
    .map(|r| Candidate {
      uri: r.uri,
      display: r.name,
      kind: VoiceTargetKind::Album,
      artist: Some(artist.to_string()),
      year: Some(r.released.0),
    })
    .collect()
}

fn tagged(query: &str, tag: &str) -> String {
  match query.is_empty() {
    true => tag.to_string(),
    false => format!("{query} {tag}"),
  }
}

fn truncate(candidates: &mut Vec<Candidate>, depth: Option<usize>) {
  if let Some(depth) = depth {
    candidates.truncate(depth);
  }
}

fn kind_of_uri(uri: &str) -> Option<VoiceTargetKind> {
  match uri.split(':').nth(1)? {
    "track" => Some(VoiceTargetKind::Track),
    "album" => Some(VoiceTargetKind::Album),
    "artist" => Some(VoiceTargetKind::Artist),
    "playlist" => Some(VoiceTargetKind::Playlist),
    "show" => Some(VoiceTargetKind::Show),
    "episode" => Some(VoiceTargetKind::Episode),
    "station" => Some(VoiceTargetKind::Station),
    _ => None,
  }
}

fn is_station_seed(kind: VoiceTargetKind) -> bool {
  matches!(
    kind,
    VoiceTargetKind::Artist | VoiceTargetKind::Track | VoiceTargetKind::Album | VoiceTargetKind::Playlist
  )
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

fn rank(items: &[FlatItem], pick: Pick) -> Vec<Candidate> {
  items
    .iter()
    .filter_map(|i| {
      let kind = kind_of_uri(&i.uri)?;
      pick.keeps(kind).then(|| Candidate {
        uri: i.uri.clone(),
        display: i.name.clone(),
        kind,
        artist: i.artist.clone(),
        year: i.year,
      })
    })
    .collect()
}

fn fresh_candidates(pool: &[String], excluded: &HashSet<String>) -> Vec<String> {
  let mut seen = HashSet::new();
  pool
    .iter()
    .filter(|u| kind_of_uri(u).is_some_and(is_fresh_context))
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
  use std::sync::Arc;

  use super::*;
  use crate::client::{
    flatten_search,
    tests::{
      NullObserver, Playing, album_hit, named_hit, playing_client, search_response, searching_client,
      searching_client_items, test_client, track_hit,
    },
  };

  fn flat(loose: &[&str]) -> Vec<FlatItem> {
    flatten_search(&search_response(loose, &[]))
  }

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

  fn cand(uri: &str, display: &str) -> Candidate {
    Candidate {
      uri: uri.to_string(),
      display: display.to_string(),
      kind: kind_of_uri(uri).expect("a test candidate is playable"),
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

  fn picked(out: &VoiceResolved) -> Vec<&str> {
    std::iter::once(out.uri.as_str())
      .chain(out.alternatives.iter().map(|a| a.uri.as_str()))
      .collect()
  }

  #[test]
  fn a_requested_kind_picks_the_first_item_of_that_kind() {
    let items = flat(&[
      "spotify:track:t1",
      "spotify:album:a1",
      "spotify:album:a2",
      "spotify:artist:r1",
    ]);
    let ranked = rank(&items, Pick::Kind(VoiceTargetKind::Album));
    assert_eq!(
      ranked.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
      ["spotify:album:a1", "spotify:album:a2"],
      "a typed pick sees only that kind, still in relevance order"
    );
    assert_eq!(ranked[0].display, "A1");
    assert_eq!(ranked[0].kind, VoiceTargetKind::Album);
  }

  #[test]
  fn an_untyped_pick_keeps_the_global_relevance_order_across_kinds() {
    let items = flat(&["spotify:artist:r1", "spotify:track:t1", "spotify:album:a1"]);
    let ranked = rank(&items, Pick::Any);
    assert_eq!(
      ranked.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
      ["spotify:artist:r1", "spotify:track:t1", "spotify:album:a1"],
      "bucketing would float the track to the front; the flat order must survive"
    );
  }

  #[test]
  fn unplayable_uris_never_become_candidates() {
    let items = flat(&["spotify:user:nobody", "spotify:genre:rock", "spotify:track:t1"]);
    let ranked = rank(&items, Pick::Any);
    assert_eq!(
      ranked.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
      ["spotify:track:t1"]
    );
  }

  #[test]
  fn containers_exclude_leaves() {
    let items = flat(&[
      "spotify:track:t1",
      "spotify:episode:e1",
      "spotify:playlist:p1",
      "spotify:show:s1",
    ]);
    let ranked = rank(&items, Pick::Container);
    assert_eq!(
      ranked.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
      ["spotify:playlist:p1", "spotify:show:s1"],
      "a position counts into a container, never into a track"
    );
  }

  #[test]
  fn alternatives_are_the_next_ranked_candidates_and_are_capped() {
    let items = flat(&[
      "spotify:track:t1",
      "spotify:track:t2",
      "spotify:track:t3",
      "spotify:track:t4",
      "spotify:track:t5",
      "spotify:track:t6",
    ]);
    let ranked = rank(&items, Pick::Any);
    let out = resolved(ranked[0].clone(), &ranked[1..], None);
    assert_eq!(out.uri, "spotify:track:t1");
    assert_eq!(out.context_uri, None, "a track carries no context of its own");
    assert_eq!(
      out.alternatives.iter().map(|a| a.uri.as_str()).collect::<Vec<_>>(),
      [
        "spotify:track:t2",
        "spotify:track:t3",
        "spotify:track:t4",
        "spotify:track:t5"
      ]
    );
  }

  #[test]
  fn a_container_pick_is_its_own_context() {
    let items = flat(&["spotify:album:a1"]);
    let ranked = rank(&items, Pick::Kind(VoiceTargetKind::Album));
    let out = resolved(ranked[0].clone(), &[], None);
    assert_eq!(out.uri, "spotify:album:a1");
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
    let pool: Vec<String> = [
      "spotify:playlist:p1",
      "spotify:playlist:p2",
      "spotify:album:a1",
      "spotify:playlist:p3",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let excluded: HashSet<String> = ["spotify:playlist:p2", "spotify:playlist:p3"]
      .iter()
      .map(|s| s.to_string())
      .collect();
    assert_eq!(
      fresh_candidates(&pool, &excluded),
      ["spotify:playlist:p1", "spotify:album:a1"],
      "a fresh pick can never resume the current context or replay a recent one"
    );
  }

  #[test]
  fn a_fresh_pick_only_considers_playable_music_contexts() {
    let pool: Vec<String> = [
      "spotify:track:t1",
      "spotify:episode:e1",
      "spotify:show:s1",
      "spotify:user:me:collection",
      "spotify:artist:r1",
      "spotify:playlist:p1",
      "spotify:playlist:p1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
      fresh_candidates(&pool, &HashSet::new()),
      ["spotify:artist:r1", "spotify:playlist:p1"],
      "leaves, shows and duplicates are not fresh contexts"
    );
  }

  #[test]
  fn a_station_wraps_its_seed_uri_whatever_the_seed_kind() {
    for (seed, want) in [
      ("spotify:artist:r1", "spotify:station:artist:r1"),
      ("spotify:track:t1", "spotify:station:track:t1"),
      ("spotify:album:a1", "spotify:station:album:a1"),
      ("spotify:playlist:p1", "spotify:station:playlist:p1"),
    ] {
      let items = flat(&[seed]);
      let seeds = station_seeds(&items, "whatever");
      let out = as_station(&seeds[0]);
      assert_eq!(out.uri, want);
      assert_eq!(out.kind, VoiceTargetKind::Station);
      assert_eq!(out.display, seeds[0].display, "the seed name is the display");
      assert_eq!(
        kind_of_uri(&out.uri),
        Some(VoiceTargetKind::Station),
        "a synthesized station uri reads back as one"
      );
    }
  }

  #[test]
  fn a_station_never_seeds_off_a_podcast() {
    let items = flat(&["spotify:show:s1", "spotify:episode:e1", "spotify:album:a1"]);
    assert_eq!(
      station_seeds(&items, "q")
        .iter()
        .map(|c| c.uri.as_str())
        .collect::<Vec<_>>(),
      ["spotify:album:a1"],
      "the server rejects podcast seeds outright"
    );
  }

  #[test]
  fn a_station_prefers_the_artist_over_a_same_named_playlist() {
    let items = flat(&["spotify:playlist:p1", "spotify:track:t1", "spotify:artist:r1"]);
    let seeds = station_seeds(&items, "R1");
    assert_eq!(
      seeds.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
      ["spotify:artist:r1", "spotify:playlist:p1", "spotify:track:t1"],
      "search floats the editorial radio playlist first; the artist itself is the better seed"
    );
  }

  #[test]
  fn a_station_matches_the_artist_name_against_the_target_not_the_modifiers() {
    let items = flat(&["spotify:playlist:p1", "spotify:artist:r1"]);
    let composed = compose_query(&VoiceResolveRequest {
      era: Some("80s".into()),
      target: Some("r1".into()),
      target_type: Some(VoiceTargetKind::Station),
      ..Default::default()
    });
    assert_eq!(composed, "80s r1");
    assert_eq!(
      station_seeds(&items, "r1").first().map(|c| c.uri.as_str()),
      Some("spotify:artist:r1"),
      "an era modifier must not stop the artist name from matching"
    );
    assert_eq!(
      station_seeds(&items, &composed).first().map(|c| c.uri.as_str()),
      Some("spotify:playlist:p1"),
      "matching on the composed query is what the target-slot match avoids"
    );
  }

  #[test]
  fn a_station_falls_back_to_relevance_when_no_artist_carries_the_query_name() {
    let items = flat(&["spotify:track:t1", "spotify:artist:r1"]);
    let seeds = station_seeds(&items, "bohemian rhapsody");
    assert_eq!(
      seeds.first().map(|c| c.uri.as_str()),
      Some("spotify:track:t1"),
      "naming a song must still seed a track station"
    );
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

  // ---- popularity filters --------------------------------------------------

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
    let mut pool: Vec<Candidate> = (1..=8).map(|n| cand(&format!("spotify:track:t{n}"), "T")).collect();
    truncate(&mut pool, Some(5));
    assert_eq!(pool.len(), 5);
    let mut whole: Vec<Candidate> = (1..=8).map(|n| cand(&format!("spotify:track:t{n}"), "T")).collect();
    truncate(&mut whole, None);
    assert_eq!(whole.len(), 8, "no depth keeps every candidate");
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
      cand("spotify:playlist:p1", "Deep Focus"),
      cand("spotify:album:a1", "Deep Cuts"),
      cand("spotify:track:t1", "Focus Deep Down"),
    ];
    assert_eq!(
      narrow(history.clone(), Pick::Any, "deep focus")
        .iter()
        .map(|c| c.uri.as_str())
        .collect::<Vec<_>>(),
      ["spotify:playlist:p1", "spotify:track:t1"],
      "word order is not position; both entries carry both words, in relevance order"
    );
    assert_eq!(
      narrow(history.clone(), Pick::Kind(VoiceTargetKind::Track), "deep")
        .iter()
        .map(|c| c.uri.as_str())
        .collect::<Vec<_>>(),
      ["spotify:track:t1"],
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
    let items = flat(&["spotify:artist:r1", "spotify:artist:r2", "spotify:track:t1"]);
    assert_eq!(
      artist_anchor(&items, &req(Some("r2"), None)).map(|c| c.uri),
      Some("spotify:artist:r2".to_string()),
      "the artist the user named is the anchor even when search floats another first"
    );
    assert_eq!(
      artist_anchor(&items, &req(Some("bohemian rhapsody"), None)),
      None,
      "an untyped name that matches no artist is not silently read as one"
    );
    assert_eq!(
      artist_anchor(&items, &req(Some("bohemian rhapsody"), Some(VoiceTargetKind::Album))).map(|c| c.uri),
      Some("spotify:artist:r1".to_string()),
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
        release("spotify:album:newer", (2015, 5, 17), 90),
        release("spotify:album:debut", (2009, 12, 29), 55),
        release("spotify:album:debutlive", (2009, 12, 29), 12),
      ],
      "Some Artist",
    );
    assert_eq!(
      first.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
      ["spotify:album:debut", "spotify:album:debutlive", "spotify:album:newer"],
      "a same-day sibling is a cut of one release; popularity picks the canonical one"
    );
    assert_eq!(first[0].kind, VoiceTargetKind::Album);
    assert_eq!(first[0].artist.as_deref(), Some("Some Artist"));
    assert_eq!(first[0].year, Some(2009));
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

  #[test]
  fn the_latest_release_is_the_newest_date_and_the_canonical_cut_of_it() {
    let latest = latest_first(
      vec![
        release("spotify:album:deluxe", (2026, 5, 15), 84),
        release("spotify:album:flagship", (2026, 5, 15), 96),
        release("spotify:album:older", (2025, 2, 14), 99),
      ],
      "Some Artist",
    );
    assert_eq!(
      latest.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
      ["spotify:album:flagship", "spotify:album:deluxe", "spotify:album:older"],
      "a same-day sibling is a cut of one release; popularity picks the canonical one"
    );
    assert_eq!(latest[0].kind, VoiceTargetKind::Album);
  }

  fn named_release(uri: &str, name: &str, released: (i32, i32, i32)) -> Release {
    Release {
      uri: uri.to_string(),
      name: name.to_string(),
      released,
      popularity: 50,
    }
  }

  #[test]
  fn a_live_recording_never_outranks_a_studio_release_by_date_alone() {
    let releases = vec![
      named_release("spotify:album:live", "More Than We Ever Imagined (Live in Mexico City)", (2026, 3, 1)),
      named_release("spotify:album:studio", "Breach", (2025, 9, 12)),
      named_release("spotify:album:debutlive", "Early Days Live", (2008, 1, 1)),
      named_release("spotify:album:debut", "Twenty One Pilots", (2009, 12, 29)),
    ];
    assert_eq!(
      latest_first(releases.clone(), "A").iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
      [
        "spotify:album:studio",
        "spotify:album:debut",
        "spotify:album:live",
        "spotify:album:debutlive"
      ],
      "the newest album is the newest studio album; live cuts sit after every studio release"
    );
    assert_eq!(
      earliest_first(releases, "A").first().map(|c| c.uri.as_str()),
      Some("spotify:album:debut"),
      "the debut is the earliest studio release, not an earlier live tape"
    );
  }

  #[test]
  fn live_detection_reads_recording_shapes_not_the_word_alone() {
    for name in [
      "More Than We Ever Imagined (Live in Mexico City)",
      "Live at Wembley",
      "MTV Unplugged (Live)",
      "Alchemy Live",
      "Live From the Fillmore",
    ] {
      assert!(live_recording(name), "{name:?} is a live recording");
    }
    for name in ["Live Through This", "Alive", "Living Things", "Breach", "Lively"] {
      assert!(!live_recording(name), "{name:?} is not a live recording");
    }
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
      req(Some("more than we ever imagined live in mexico city"), Some(VoiceTargetKind::Album)),
    )
    .await;
    assert_eq!(named, "spotify:album:live", "naming the live album is the explicit request");
  }

  #[test]
  fn popularity_reorders_the_hits_and_sinks_the_kinds_that_carry_no_score() {
    let ranked = vec![
      cand("spotify:track:t1", "T1"),
      cand("spotify:playlist:p1", "P1"),
      cand("spotify:track:t2", "T2"),
      cand("spotify:playlist:p2", "P2"),
    ];
    let scores = HashMap::from([
      ("spotify:track:t1".to_string(), 41),
      ("spotify:track:t2".to_string(), 84),
    ]);
    assert_eq!(
      rank_by(ranked, &scores, None)
        .iter()
        .map(|c| c.uri.as_str())
        .collect::<Vec<_>>(),
      [
        "spotify:track:t2",
        "spotify:track:t1",
        "spotify:playlist:p1",
        "spotify:playlist:p2"
      ],
      "playlists have no popularity field; they sink together and keep relevance order"
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

  // ---- the pick reads the evidence, not just relevance order ---------------

  #[test]
  fn normalization_speaks_asr_digits_and_catalog_words_alike() {
    assert_eq!(norm("Twenty One Pilots"), "21 pilots");
    assert_eq!(norm("21 Pilots"), "21 pilots");
    assert_eq!(norm("Blink-182"), "blink 182");
    assert_eq!(norm("Thirty Seconds To Mars"), "30 seconds to mars");
    assert_eq!(norm("Florence & The Machine"), "florence and the machine");
    assert_eq!(norm("Seventy"), "70", "a bare tens word is not swallowed");
    assert_eq!(norm("Twenty Twenty"), "20 20", "tens never merge with tens");
    assert_eq!(norm("One Direction"), "1 direction");
    assert_eq!(norm("  Trench  "), "trench");
  }

  #[test]
  fn eras_parse_as_year_ranges_or_not_at_all() {
    assert_eq!(era_years("2009"), Some((2009, 2009)));
    assert_eq!(era_years("80s"), Some((1980, 1989)));
    assert_eq!(era_years("the 80s"), Some((1980, 1989)));
    assert_eq!(era_years("eighties"), Some((1980, 1989)));
    assert_eq!(era_years("1980s"), Some((1980, 1989)));
    assert_eq!(era_years("20s"), Some((2020, 2029)), "a bare 20s is this century");
    assert_eq!(era_years("2000s"), Some((2000, 2009)));
    assert_eq!(era_years("chill"), None);
    assert_eq!(era_years("85s"), None, "not a decade");
    assert_eq!(era_years("1850"), None, "outside the catalog era");
  }

  #[test]
  fn discography_releases_fill_gaps_and_extend_without_displacing_search_order() {
    let ranked = vec![cand("spotify:album:a1", "Breach")];
    let merged = merge_releases(
      ranked,
      vec![
        release("spotify:album:a1", (2025, 9, 12), 90),
        release("spotify:album:a2", (2009, 12, 29), 40),
      ],
      "Twenty One Pilots",
    );
    assert_eq!(
      merged.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
      ["spotify:album:a1", "spotify:album:a2"],
      "search hits keep their slot; unseen releases append"
    );
    assert_eq!(merged[0].year, Some(2025), "a search hit gains the year it lacked");
    assert_eq!(merged[0].artist.as_deref(), Some("Twenty One Pilots"));
    assert_eq!(merged[1].year, Some(2009));
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

  // ---- generic vibe requests prefer the user's own mixes --------------------

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

  // ---- bare kinds anchored to now playing ----------------------------------

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
}

#[cfg(all(test, feature = "native-io"))]
mod live {
  use std::{path::PathBuf, sync::Arc};

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
      assert_eq!(out.year, Some(want_year), "{target:?} debut year; got {:?}", out.display);
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
    let recents = client.recent_context_uris().await.unwrap_or_default();
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
      kind_of_uri(&random.uri).is_some_and(is_fresh_context),
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
    let items = client
      .search_flat(NEW_RELEASES_TAG, SEARCH_LIMIT)
      .await
      .expect("tagged search");
    let uris: Vec<String> = items.iter().map(|i| i.uri.clone()).collect();
    assert!(!uris.is_empty(), "the tag returned nothing at all");
    assert!(
      uris.iter().all(|u| u.starts_with("spotify:album:")),
      "the tag is an album-only narrowing; got {uris:?}"
    );
    let releases = client.popularity_of(&uris).await;
    assert!(
      !releases.is_empty(),
      "tagged hits must hydrate as real albums, not phantom uris"
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
