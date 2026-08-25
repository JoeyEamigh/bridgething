use std::{
  collections::{HashMap, HashSet},
  sync::Arc,
  time::Duration,
};

use bridgething_io::{HttpExecutor, HttpTransport, WsTransport};
use futures::future::join_all;
use librespot_protocol::{connect::Cluster, player::PlayerState as PbPlayerState};
use serde_json::json;
use tokio::{
  sync::{Mutex, Notify},
  task::JoinHandle,
};

use crate::{
  aplogin,
  auth::{Auth, DeviceFlow},
  dealer::{self, Dealer, DealerEvent, DealerWriter, active_device, cluster_playing, phone_device, start_device},
  error::{Error, Result},
  http::SpHttp,
  model::{
    self, AuthState, BrowseItem, BrowsePage, Device, LibraryScope, PlayerState, ProductState, Queue, QueuePosition,
    RepeatMode, SearchResults, Shelf, Track,
  },
  resolver::{self, VoiceResolveRequest, VoiceResolved, VoiceResult},
  spclient::SpClient,
  util::gid_to_base62,
};

const NODE_RECENTS: &str = "recently-played";
const NODE_PLAYLISTS: &str = "playlists";
const NODE_ALBUMS: &str = "albums";
const NODE_ARTISTS: &str = "artists";
const NODE_PODCASTS: &str = "podcasts";
const PREVIEW: u32 = 14;
const RECENTS_CACHE_TTL: Duration = Duration::from_secs(60);
const HYDRATE_CACHE_CAP: usize = 4096;
const LIBRARY_CHANGE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(750);
const DJ_URI: &str = "spotify:playlist:37i9dQZF1EYkqdzj48dyYq";
const UPCOMING_CAP: usize = 80;

pub trait Observer: Send + Sync {
  fn on_player(&self, state: PlayerState);
  fn on_queue(&self, queue: Queue);
  fn on_devices(&self, devices: Vec<Device>);
  fn on_auth(&self, state: AuthState);
  fn on_library_changed(&self, scope: LibraryScope);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
  UserPlay,
  ConnectResume,
}

pub use crate::dealer::Placement;

pub trait DeviceWaker: Send + Sync {
  fn wake_device(&self, reason: WakeReason, allow_play_tap: bool);
}

const DEVICE_WAKE_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_RESUME_CLUSTER_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_RESUME_WAKE_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_RESUME_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);
const PLAY_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);

fn is_stale_target(e: &Error) -> bool {
  matches!(e, Error::Status { status: 404, body, .. } if body.contains("DEVICE_NOT_FOUND"))
}

async fn target_of(shared: &Shared, my_id: &str) -> Option<String> {
  let last_active = shared.last_active.lock().await.clone();
  let guard = shared.cluster.lock().await;
  active_device(guard.as_ref()?, my_id, last_active.as_deref())
}

async fn start_target_of(shared: &Shared, my_id: &str) -> Option<String> {
  let placement = *shared.placement.lock().unwrap();
  let last_active = shared.last_active.lock().await.clone();
  let guard = shared.cluster.lock().await;
  start_device(guard.as_ref()?, my_id, last_active.as_deref(), placement)
}

async fn await_start_target_of(shared: &Shared, my_id: &str) -> String {
  let notified = shared.cluster_changed.notified();
  tokio::pin!(notified);
  loop {
    notified.as_mut().enable();
    if let Some(t) = start_target_of(shared, my_id).await {
      return t;
    }
    notified.as_mut().await;
    notified.set(shared.cluster_changed.notified());
  }
}

async fn allow_play_tap_of(shared: &Shared) -> bool {
  shared.writer.lock().await.is_none()
}

async fn wake_fresh_target_of(shared: &Shared, my_id: &str) -> Result<String> {
  let changed = shared.cluster_changed.notified();
  tokio::pin!(changed);
  changed.as_mut().enable();
  let waker = shared.device_waker.lock().unwrap().clone();
  let Some(waker) = waker else {
    return Err(Error::other("target unreachable and no platform waker"));
  };
  waker.wake_device(WakeReason::UserPlay, allow_play_tap_of(shared).await);
  if tokio::time::timeout(CONNECT_RESUME_WAKE_TIMEOUT, changed)
    .await
    .is_err()
  {
    return Err(Error::other("no cluster update after wake"));
  }
  match tokio::time::timeout(DEVICE_WAKE_TIMEOUT, await_start_target_of(shared, my_id)).await {
    Ok(t) => Ok(t),
    Err(_) => Err(Error::other("no device appeared after wake")),
  }
}
const TRANSFER_SETTLE_TIMEOUT: Duration = Duration::from_secs(4);

struct Shared {
  writer: Mutex<Option<DealerWriter>>,
  cluster: Mutex<Option<Cluster>>,
  last_active: Mutex<Option<String>>,
  device_waker: std::sync::Mutex<Option<Arc<dyn DeviceWaker>>>,
  placement: std::sync::Mutex<Placement>,
  cluster_changed: Notify,
  connect_resume: Mutex<()>,
  play_recover: Mutex<()>,
}

pub struct SpotifyClient {
  auth: Arc<Auth>,
  http: SpHttp,
  spc: SpClient,
  dealer: Dealer,
  exec: HttpExecutor,
  observer: Arc<dyn Observer>,
  shared: Arc<Shared>,
  username: Mutex<Option<String>>,
  liked: Arc<Mutex<Option<Vec<String>>>>,
  browse_cache: Arc<Mutex<BrowseCache>>,
  loop_handle: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct BrowseCache {
  rootlist: Option<Vec<String>>,
  collections: HashMap<String, Vec<String>>,
  recents: Option<(tokio::time::Instant, Recents)>,
  hydrated: HashMap<String, (u64, BrowseItem)>,
  counter: u64,
}

#[derive(Clone, Default)]
struct Recents {
  tracks: Vec<String>,
  contexts: Vec<String>,
}

impl BrowseCache {
  fn note_saved_changed(&mut self) {
    self.collections.clear();
  }

  fn note_playlists_changed(&mut self) {
    self.rootlist = None;
    self
      .hydrated
      .retain(|uri, _| !uri.starts_with("spotify:playlist:") && !uri.ends_with(":collection"));
  }

  fn hydrated_get(&self, uri: &str) -> Option<BrowseItem> {
    self.hydrated.get(uri).map(|(_, item)| item.clone())
  }

  fn hydrated_put(&mut self, uri: String, item: BrowseItem) {
    self.counter += 1;
    self.hydrated.insert(uri, (self.counter, item));
    if self.hydrated.len() > HYDRATE_CACHE_CAP {
      let floor = self.counter.saturating_sub(HYDRATE_CACHE_CAP as u64);
      self.hydrated.retain(|_, (at, _)| *at > floor);
    }
  }
}

impl SpotifyClient {
  pub fn new(auth: Arc<Auth>, device_id: String, exec: HttpExecutor, observer: Arc<dyn Observer>) -> Self {
    let http = SpHttp::new(auth.clone(), exec.clone());
    let spc = SpClient::new(http.clone());
    let dealer = Dealer::new(http.clone(), device_id);
    SpotifyClient {
      auth,
      http,
      spc,
      dealer,
      exec,
      observer,
      shared: Arc::new(Shared {
        writer: Mutex::new(None),
        cluster: Mutex::new(None),
        last_active: Mutex::new(None),
        device_waker: std::sync::Mutex::new(None),
        placement: std::sync::Mutex::new(Placement::default()),
        cluster_changed: Notify::new(),
        connect_resume: Mutex::new(()),
        play_recover: Mutex::new(()),
      }),
      username: Mutex::new(None),
      liked: Arc::new(Mutex::new(None)),
      browse_cache: Arc::new(Mutex::new(BrowseCache::default())),
      loop_handle: Mutex::new(None),
    }
  }

  fn spawn_events_loop(&self) -> JoinHandle<()> {
    tokio::spawn(events_loop(
      self.dealer.clone(),
      self.spc.clone(),
      self.observer.clone(),
      self.shared.clone(),
      self.liked.clone(),
      self.browse_cache.clone(),
      self.dealer.device_id().to_string(),
    ))
  }

  async fn username(&self) -> Result<String> {
    self.username.lock().await.clone().ok_or(Error::NoUsername)
  }

  async fn writer(&self) -> Result<DealerWriter> {
    self
      .shared
      .writer
      .lock()
      .await
      .clone()
      .ok_or_else(|| Error::other("dealer not connected"))
  }

  async fn target(&self) -> Result<String> {
    {
      let guard = self.shared.cluster.lock().await;
      guard.as_ref().ok_or_else(|| Error::other("no cluster yet"))?;
    }
    match target_of(&self.shared, self.dealer.device_id()).await {
      Some(target) => Ok(target),
      None => {
        let devices = self.shared.cluster.lock().await.as_ref().map_or(0, |c| c.device.len());
        tracing::warn!(
          devices,
          "spotify command: no reachable target device (phone spotify likely not an active connect device)"
        );
        Err(Error::other("no reachable target device"))
      }
    }
  }

  async fn start_target(&self) -> Result<String> {
    {
      let guard = self.shared.cluster.lock().await;
      guard.as_ref().ok_or_else(|| Error::other("no cluster yet"))?;
    }
    match start_target_of(&self.shared, self.dealer.device_id()).await {
      Some(target) => Ok(target),
      None => {
        let devices = self.shared.cluster.lock().await.as_ref().map_or(0, |c| c.device.len());
        tracing::warn!(
          devices,
          "spotify play: no eligible start device (phone spotify likely not an active connect device)"
        );
        Err(Error::other("no reachable target device"))
      }
    }
  }

  async fn start_target_or_wake(&self) -> Result<String> {
    if let Ok(t) = self.start_target().await {
      return Ok(t);
    }
    let waker = self.shared.device_waker.lock().unwrap().clone();
    let Some(waker) = waker else {
      return self.start_target().await;
    };
    tracing::info!("spotify play: no eligible start device; asking platform to wake the phone's spotify");
    waker.wake_device(WakeReason::UserPlay, allow_play_tap_of(&self.shared).await);
    match tokio::time::timeout(DEVICE_WAKE_TIMEOUT, self.await_start_target()).await {
      Ok(res) => res,
      Err(_) => {
        tracing::warn!("spotify play: no eligible device registered within wake timeout");
        Err(Error::other("no device appeared after wake"))
      }
    }
  }

  async fn await_start_target(&self) -> Result<String> {
    Ok(await_start_target_of(&self.shared, self.dealer.device_id()).await)
  }

  async fn wake_fresh_target(&self) -> Result<String> {
    wake_fresh_target_of(&self.shared, self.dealer.device_id()).await
  }

  async fn verified_play(&self, cmd: serde_json::Value, context: &str) -> Result<()> {
    let writer = self.writer().await?;
    let target = self.start_target_or_wake().await?;
    match writer.play(&target, cmd.clone()).await {
      Ok(_) => {}
      Err(e) if is_stale_target(&e) => {
        tracing::warn!(%target, error = %e, "spotify play: connect no longer knows the target; waking and retrying");
        let fresh = self.wake_fresh_target().await?;
        writer.play(&fresh, cmd).await?;
        return Ok(());
      }
      Err(e) => return Err(e),
    }
    self.spawn_play_confirm(writer, cmd, context.to_string());
    Ok(())
  }

  fn spawn_play_confirm(&self, writer: DealerWriter, cmd: serde_json::Value, context: String) {
    if self.shared.device_waker.lock().unwrap().is_none() {
      return;
    }
    let shared = self.shared.clone();
    let my_id = self.dealer.device_id().to_string();
    tokio::spawn(async move {
      let Ok(_recovering) = shared.play_recover.try_lock() else {
        return;
      };
      let confirmed = tokio::time::timeout(PLAY_CONFIRM_TIMEOUT, async {
        let notified = shared.cluster_changed.notified();
        tokio::pin!(notified);
        loop {
          notified.as_mut().enable();
          {
            let guard = shared.cluster.lock().await;
            if let Some(c) = guard.as_ref()
              && cluster_playing(c)
              && c.player_state.context_uri == context
            {
              return;
            }
          }
          notified.as_mut().await;
          notified.set(shared.cluster_changed.notified());
        }
      })
      .await
      .is_ok();
      if confirmed {
        return;
      }
      tracing::warn!(%context, "spotify play: accepted but the cluster never confirmed it; waking and replaying");
      match wake_fresh_target_of(&shared, &my_id).await {
        Ok(fresh) => {
          if let Err(e) = writer.play(&fresh, cmd).await {
            tracing::warn!(error = %e, "spotify play: replay after wake failed");
          }
        }
        Err(e) => tracing::warn!(error = %e, "spotify play: could not recover an unconfirmed play"),
      }
    });
  }

  fn fire_waker(&self, reason: WakeReason, allow_play_tap: bool) -> bool {
    let waker = self.shared.device_waker.lock().unwrap().clone();
    match waker {
      Some(w) => {
        w.wake_device(reason, allow_play_tap);
        true
      }
      None => false,
    }
  }

  async fn await_cluster(&self) -> Cluster {
    let notified = self.shared.cluster_changed.notified();
    tokio::pin!(notified);
    loop {
      notified.as_mut().enable();
      if let Some(c) = self.shared.cluster.lock().await.clone() {
        return c;
      }
      notified.as_mut().await;
      notified.set(self.shared.cluster_changed.notified());
    }
  }

  async fn await_phone(&self, me: &str) -> String {
    let notified = self.shared.cluster_changed.notified();
    tokio::pin!(notified);
    loop {
      notified.as_mut().enable();
      if let Some(phone) = self
        .shared
        .cluster
        .lock()
        .await
        .as_ref()
        .and_then(|c| phone_device(c, me))
      {
        return phone;
      }
      notified.as_mut().await;
      notified.set(self.shared.cluster_changed.notified());
    }
  }

  async fn await_active(&self, device_id: &str) {
    let notified = self.shared.cluster_changed.notified();
    tokio::pin!(notified);
    loop {
      notified.as_mut().enable();
      if self
        .shared
        .cluster
        .lock()
        .await
        .as_ref()
        .is_some_and(|c| c.active_device_id == device_id)
      {
        return;
      }
      notified.as_mut().await;
      notified.set(self.shared.cluster_changed.notified());
    }
  }

  async fn cluster_playing_now(&self) -> bool {
    self.shared.cluster.lock().await.as_ref().is_some_and(cluster_playing)
  }

  async fn await_playing(&self) {
    let notified = self.shared.cluster_changed.notified();
    tokio::pin!(notified);
    loop {
      notified.as_mut().enable();
      if self.cluster_playing_now().await {
        return;
      }
      notified.as_mut().await;
      notified.set(self.shared.cluster_changed.notified());
    }
  }

  async fn album_for_track(&self, uri: &str) -> Option<String> {
    let tracks = self.spc.get_tracks(&[uri.to_string()]).await.ok()?;
    let t = tracks.get(uri)?;
    if t.album.gid().is_empty() {
      None
    } else {
      Some(format!("spotify:album:{}", gid_to_base62(t.album.gid())))
    }
  }

  async fn liked_uris(&self, username: &str) -> Result<Vec<String>> {
    if let Some(cached) = self.liked.lock().await.clone() {
      return Ok(cached);
    }
    let uris = fetch_liked_uris(&self.spc, username).await?;
    *self.liked.lock().await = Some(uris.clone());
    Ok(uris)
  }

  fn spawn_liked_warm(&self, username: String) {
    let spc = self.spc.clone();
    let liked = self.liked.clone();
    tokio::spawn(async move {
      if let Ok(uris) = fetch_liked_uris(&spc, &username).await {
        *liked.lock().await = Some(uris);
      }
    });
  }

  async fn recents(&self) -> Result<Recents> {
    if let Some((at, cached)) = self.browse_cache.lock().await.recents.clone()
      && at.elapsed() < RECENTS_CACHE_TTL
    {
      return Ok(cached);
    }
    let user = self.username().await?;
    let rp = self.spc.recently_played(&user, 50).await?;
    let (mut seen_track, mut seen_ctx) = (HashSet::new(), HashSet::new());
    let mut out = Recents::default();
    for e in &rp.items {
      if e.track_uri.starts_with("spotify:track:") && seen_track.insert(e.track_uri.clone()) {
        out.tracks.push(e.track_uri.clone());
      }
      if !e.context_uri.is_empty() && seen_ctx.insert(e.context_uri.clone()) {
        out.contexts.push(e.context_uri.clone());
      }
    }
    self.browse_cache.lock().await.recents = Some((tokio::time::Instant::now(), out.clone()));
    Ok(out)
  }

  pub(crate) async fn recent_track_uris(&self) -> Result<Vec<String>> {
    Ok(self.recents().await?.tracks)
  }

  pub(crate) async fn recent_context_uris(&self) -> Result<Vec<String>> {
    Ok(self.recents().await?.contexts)
  }

  pub(crate) async fn playlist_uris(&self) -> Result<Vec<String>> {
    if let Some(cached) = self.browse_cache.lock().await.rootlist.clone() {
      return Ok(cached);
    }
    let user = self.username().await?;
    let rl = self.spc.rootlist(&user).await?;
    let uris: Vec<String> = rl
      .contents
      .items
      .iter()
      .map(|i| i.uri().to_string())
      .filter(|u| u.starts_with("spotify:playlist:"))
      .collect();
    self.browse_cache.lock().await.rootlist = Some(uris.clone());
    Ok(uris)
  }

  async fn collection_uris(&self, set: &str, kind: Option<&str>) -> Result<Vec<String>> {
    let key = format!("{set}:{}", kind.unwrap_or(""));
    if let Some(cached) = self.browse_cache.lock().await.collections.get(&key).cloned() {
      return Ok(cached);
    }
    let user = self.username().await?;
    let mut items = self.spc.collection_paging(&user, set, 500).await?;
    items.sort_by_key(|item| std::cmp::Reverse(item.added_at));
    let uris: Vec<String> = items
      .into_iter()
      .map(|i| i.uri)
      .filter(|u| kind.is_none_or(|k| u.split(':').nth(1) == Some(k)))
      .collect();
    self.browse_cache.lock().await.collections.insert(key, uris.clone());
    Ok(uris)
  }

  async fn hydrate_map(&self, uris: &[String]) -> HashMap<String, BrowseItem> {
    let mut out = HashMap::new();
    let missing: Vec<String> = {
      let cache = self.browse_cache.lock().await;
      uris
        .iter()
        .filter(|u| {
          if let Some(item) = cache.hydrated_get(u) {
            out.insert((*u).clone(), item);
            false
          } else {
            true
          }
        })
        .cloned()
        .collect()
    };
    if missing.is_empty() {
      return out;
    }
    let fetched = self.hydrate_map_uncached(&missing).await;
    let mut cache = self.browse_cache.lock().await;
    for (u, item) in fetched {
      cache.hydrated_put(u.clone(), item.clone());
      out.insert(u, item);
    }
    out
  }

  async fn hydrate_map_uncached(&self, uris: &[String]) -> HashMap<String, BrowseItem> {
    let mut by_kind: HashMap<&str, Vec<String>> = HashMap::new();
    let mut playlists: Vec<String> = Vec::new();
    for u in uris {
      match u.split(':').nth(1) {
        Some("playlist") => playlists.push(u.clone()),
        Some(k) => by_kind.entry(k).or_default().push(u.clone()),
        None => {}
      }
    }
    let empty: Vec<String> = Vec::new();
    let ids = |k: &str| by_kind.get(k).unwrap_or(&empty).clone();
    let (track_ids, album_ids, artist_ids, show_ids, episode_ids) =
      (ids("track"), ids("album"), ids("artist"), ids("show"), ids("episode"));
    let (tracks, albums, artists, shows, episodes, pls) = tokio::join!(
      self.spc.get_tracks(&track_ids),
      self.spc.get_albums(&album_ids),
      self.spc.get_artists(&artist_ids),
      self.spc.get_shows(&show_ids),
      self.spc.get_episodes(&episode_ids),
      self.hydrate_playlists(&playlists),
    );
    let tracks = tracks.unwrap_or_default();
    let albums = albums.unwrap_or_default();
    let artists = artists.unwrap_or_default();
    let shows = shows.unwrap_or_default();
    let episodes = episodes.unwrap_or_default();

    let mut out = HashMap::new();
    for u in uris {
      let item = match u.split(':').nth(1) {
        Some("track") => tracks.get(u).map(|t| model::browse_track(u, t)),
        Some("album") => albums.get(u).map(|a| model::browse_album(u, a)),
        Some("artist") => artists.get(u).map(|a| model::browse_artist(u, a)),
        Some("show") => shows.get(u).map(|s| model::browse_show(u, s)),
        Some("episode") => episodes.get(u).map(|e| model::browse_episode(u, e)),
        Some("playlist") => pls.get(u).cloned(),
        _ if u.ends_with(":collection") => Some(liked_songs_item(u)),
        _ => None,
      };
      if let Some(it) = item {
        out.insert(u.clone(), it);
      }
    }
    out
  }

  async fn hydrate_playlists(&self, uris: &[String]) -> HashMap<String, BrowseItem> {
    let fetches = uris.iter().map(|u| {
      let spc = self.spc.clone();
      let u = u.clone();
      async move {
        let id = u.rsplit(':').next().unwrap_or("").to_string();
        (u, spc.get_playlist(&id, 0, Some(4)).await.ok())
      }
    });
    let mut out = HashMap::new();
    let mut need_cover: Vec<(String, String)> = Vec::new();
    for (u, pl) in join_all(fetches).await {
      let Some(pl) = pl else { continue };
      let img = model::playlist_image_hex(&pl.attributes);
      if img.is_empty()
        && let Some(first) = pl.contents.items.iter().find(|i| i.uri().starts_with("spotify:track:"))
      {
        need_cover.push((u.clone(), first.uri().to_string()));
      }
      out.insert(u.clone(), model::browse_playlist(&u, pl.attributes.name(), &img));
    }
    if !need_cover.is_empty() {
      let track_ids: Vec<String> = need_cover.iter().map(|(_, t)| t.clone()).collect();
      if let Ok(tracks) = self.spc.get_tracks(&track_ids).await {
        for (pl_uri, track_uri) in &need_cover {
          if let Some(t) = tracks.get(track_uri) {
            let cover = crate::util::image_hex(&t.album.cover_group);
            if !cover.is_empty()
              && let Some(item) = out.get_mut(pl_uri)
            {
              item.image_id = cover;
            }
          }
        }
      }
    }
    out
  }

  pub(crate) async fn hydrate_uris(&self, uris: &[String]) -> Vec<BrowseItem> {
    let map = self.hydrate_map(uris).await;
    uris
      .iter()
      .map(|u| {
        map.get(u).cloned().unwrap_or_else(|| BrowseItem {
          uri: u.clone(),
          ..Default::default()
        })
      })
      .collect()
  }

  pub(crate) async fn browse_container(&self, uri: &str, limit: u32, offset: u32) -> Result<BrowsePage> {
    match uri.split(':').nth(1).unwrap_or("") {
      "playlist" => {
        let id = uri.rsplit(':').next().unwrap_or("");
        let pl = self.spc.get_playlist(id, offset, Some(limit)).await?;
        let total = pl.length().max(0) as u32;
        let page: Vec<String> = pl
          .contents
          .items
          .iter()
          .map(|i| i.uri().to_string())
          .filter(|u| u.starts_with("spotify:track:"))
          .collect();
        let count = page.len() as u32;
        let items = self.hydrate_uris(&page).await;
        Ok(BrowsePage {
          items,
          total: Some(total),
          has_more: offset + count < total,
        })
      }
      "album" => {
        let albums = self.spc.get_albums(&[uri.to_string()]).await?;
        let all = albums.get(uri).map(model::album_track_uris).unwrap_or_default();
        self.page_uris(&all, offset, limit).await
      }
      "artist" => {
        let artists = self.spc.get_artists(&[uri.to_string()]).await?;
        let all = artists.get(uri).map(model::artist_top_track_uris).unwrap_or_default();
        self.page_uris(&all, offset, limit).await
      }
      "show" => {
        let shows = self.spc.get_shows(&[uri.to_string()]).await?;
        let all = shows.get(uri).map(model::show_episode_uris).unwrap_or_default();
        self.page_uris(&all, offset, limit).await
      }
      _ if uri.ends_with(":collection") => {
        let user = self.username().await?;
        let all = self.liked_uris(&user).await?;
        self.page_uris(&all, offset, limit).await
      }
      _ => {
        let home = self.spc.get_home("en").await?;
        let all = home
          .body
          .sections
          .iter()
          .find(|s| s.id.uri == uri)
          .map(|s| carousel_of(s).1)
          .unwrap_or_default();
        self.page_uris(&all, offset, limit).await
      }
    }
  }

  async fn page_uris(&self, all: &[String], offset: u32, limit: u32) -> Result<BrowsePage> {
    let total = all.len() as u32;
    let page: Vec<String> = all.iter().skip(offset as usize).take(limit as usize).cloned().collect();
    let count = page.len() as u32;
    let items = self.hydrate_uris(&page).await;
    Ok(BrowsePage {
      items,
      total: Some(total),
      has_more: offset + count < total,
    })
  }

  async fn play_dj(&self) -> Result<()> {
    let writer = self.writer().await?;
    if self.current_context_uri().await.as_deref() == Some(DJ_URI) {
      writer.dj_signal(&self.target().await?).await?;
      return Ok(());
    }
    let cmd = json!({
        "endpoint": "play",
        "context": {
          "uri": DJ_URI,
          "entity_uri": DJ_URI,
          "url": format!("hm://lexicon-session-provider/context-resolve/v2/session?contextUri={DJ_URI}"),
          "metadata": {},
        },
        "play_origin": {"feature_identifier": "harmony", "feature_version": "9.1.52.1394", "referrer_identifier": "home"},
        "prepare_play_options": {"license": "premium"},
        "play_options": {"reason": "interactive", "operation": "replace", "trigger": "immediately"},
    });
    self.verified_play(cmd, DJ_URI).await
  }

  pub(crate) async fn current_context_uri(&self) -> Option<String> {
    let guard = self.shared.cluster.lock().await;
    some_uri(&guard.as_ref()?.player_state.context_uri)
  }

  pub(crate) async fn playback_anchor(&self) -> Option<PlaybackAnchor> {
    let guard = self.shared.cluster.lock().await;
    let state = &guard.as_ref()?.player_state;
    let track = &state.track;
    let held = |direct: &str, key: &str| some_uri(direct).or_else(|| track.metadata.get(key).and_then(|u| some_uri(u)));
    Some(PlaybackAnchor {
      track_uri: some_uri(&track.uri)?,
      album_uri: held(&track.album_uri, "album_uri"),
      artist_uri: held(&track.artist_uri, "artist_uri"),
      context_uri: some_uri(&state.context_uri),
    })
  }

  pub(crate) async fn home_uris(&self) -> Vec<String> {
    let Ok(home) = self.spc.get_home("en").await else {
      return Vec::new();
    };
    home.body.sections.iter().flat_map(|s| carousel_of(s).1).collect()
  }

  pub(crate) async fn search_flat(&self, query: &str, limit: u32) -> Result<Vec<FlatItem>> {
    let resp = self.spc.search(query, limit.max(20)).await?;
    Ok(flatten_search(&resp))
  }

  pub(crate) async fn popularity_of(&self, uris: &[String]) -> HashMap<String, i32> {
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

  pub(crate) async fn artist_releases(
    &self,
    artist_uri: &str,
    albums_only: bool,
    depth: usize,
  ) -> Result<Vec<Release>> {
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

  async fn upcoming_state(&self) -> Option<PbPlayerState> {
    let guard = self.shared.cluster.lock().await;
    let ps = &guard.as_ref()?.player_state;
    (!ps.next_tracks.is_empty()).then(|| (**ps).clone())
  }
}

impl SpotifyClient {
  pub fn set_ws_transport(&self, transport: Arc<dyn WsTransport>) {
    self.dealer.set_transport(transport);
  }

  pub fn set_http_transport(&self, transport: Arc<dyn HttpTransport>) {
    self.exec.set(transport);
  }

  pub fn set_placement(&self, placement: Placement) {
    *self.shared.placement.lock().unwrap() = placement;
  }

  pub fn set_device_waker(&self, waker: Arc<dyn DeviceWaker>) {
    *self.shared.device_waker.lock().unwrap() = Some(waker);
  }

  pub async fn connect(&self) -> Result<()> {
    if let Some(prior) = self.loop_handle.lock().await.take()
      && !prior.is_finished()
    {
      prior.abort();
    }

    let mut paired = self.auth.is_paired().await;
    tracing::info!(paired, "spotify connect: starting auth lifecycle");
    if paired {
      match self.auth.bearer().await {
        Ok(_) => {}
        Err(e) if is_auth_terminal(&e) => {
          tracing::warn!(error = %e, "spotify connect: stored token rejected; re-pairing");
          paired = false;
        }
        Err(e) => tracing::warn!(error = %e, "spotify connect: token check failed transiently; proceeding"),
      }
    }
    if !paired {
      tracing::info!("spotify connect: requesting device code from worker");
      let flow = match self.auth.begin_device_flow().await {
        Ok(f) => f,
        Err(e) => {
          tracing::warn!(error = %e, "spotify connect: device-code request failed");
          self.observer.on_auth(AuthState::Failed { reason: e.to_string() });
          return Err(e);
        }
      };
      tracing::info!("spotify connect: device code issued; emitting pending and awaiting approval");
      self.observer.on_auth(AuthState::Pending {
        url: flow.verification_uri.clone(),
        code: flow.user_code.clone(),
      });
      if let Err(e) = self.auth.complete_device_flow(&flow).await {
        tracing::warn!(error = %e, "spotify connect: device flow did not complete");
        self.observer.on_auth(AuthState::Failed { reason: e.to_string() });
        return Err(e);
      }
      tracing::info!("spotify connect: device flow approved");
    }

    let username = match aplogin::resolve_and_cache(&self.auth, &self.http, self.dealer.device_id()).await {
      Ok(u) => Some(u),
      Err(e) if is_auth_terminal(&e) => {
        tracing::warn!(error = %e, "spotify connect: terminal auth error resolving username");
        self.observer.on_auth(AuthState::Failed { reason: e.to_string() });
        return Err(e);
      }
      Err(e) => {
        tracing::warn!("username resolution failed, continuing bearer-only: {e}");
        None
      }
    };
    *self.username.lock().await = username.clone();

    if let Ok(p) = self.product().await {
      self.http.set_market(&p.country, &p.catalogue).await;
    }
    if let Some(user) = &username {
      self.spawn_liked_warm(user.clone());
    }

    tracing::info!(
      has_username = username.is_some(),
      "spotify connect: logged in, spawning events loop"
    );
    self.observer.on_auth(AuthState::LoggedIn {
      username: username.unwrap_or_default(),
    });

    *self.loop_handle.lock().await = Some(self.spawn_events_loop());
    Ok(())
  }

  pub async fn resync(&self) {
    let mut guard = self.loop_handle.lock().await;
    if guard.is_none() {
      return;
    }
    tracing::info!("spotify resync: re-establishing dealer on request");
    if let Some(h) = guard.take() {
      h.abort();
    }
    *guard = Some(self.spawn_events_loop());
  }

  pub async fn disconnect(&self) {
    if let Some(h) = self.loop_handle.lock().await.take() {
      h.abort();
    }
    *self.shared.writer.lock().await = None;
    *self.shared.cluster.lock().await = None;
    *self.shared.last_active.lock().await = None;
    *self.browse_cache.lock().await = BrowseCache::default();
  }

  pub async fn current_position_ms(&self) -> Option<u32> {
    let guard = self.shared.cluster.lock().await;
    let cluster = guard.as_ref()?;
    (!cluster.player_state.track.uri.is_empty()).then(|| model::position_now(&cluster.player_state))
  }

  // ---- commands -----------------------------------------------------------

  pub async fn pause(&self) -> Result<()> {
    self.writer().await?.pause(&self.target().await?).await?;
    Ok(())
  }
  pub async fn resume(&self) -> Result<()> {
    let writer = self.writer().await?;
    let target = self.start_target_or_wake().await?;
    let active = self
      .shared
      .cluster
      .lock()
      .await
      .as_ref()
      .map(|c| c.active_device_id.clone())
      .unwrap_or_default();
    if !active.is_empty() && active != target {
      tracing::info!(from = %active, to = %target, "spotify resume: transferring the parked session before resuming");
      match writer.transfer(&target).await {
        Ok(_) => {
          let _ = tokio::time::timeout(TRANSFER_SETTLE_TIMEOUT, self.await_active(&target)).await;
          if self.cluster_playing_now().await {
            return Ok(());
          }
        }
        Err(e) => tracing::warn!(error = %e, "spotify resume: transfer failed; attempting direct resume"),
      }
    }
    match writer.resume(&target).await {
      Ok(_) => Ok(()),
      Err(e) if is_stale_target(&e) => {
        tracing::warn!(%target, error = %e, "spotify resume: connect no longer knows the target; waking and retrying");
        let fresh = self.wake_fresh_target().await?;
        writer.resume(&fresh).await?;
        Ok(())
      }
      Err(e) => Err(e),
    }
  }

  pub async fn resume_on_connect(&self) -> Result<()> {
    let Ok(_running) = self.shared.connect_resume.try_lock() else {
      tracing::debug!("connect resume already in flight; dropping duplicate");
      return Ok(());
    };
    let cluster = match tokio::time::timeout(CONNECT_RESUME_CLUSTER_TIMEOUT, self.await_cluster()).await {
      Ok(c) => c,
      Err(_) => {
        tracing::info!("connect resume: no cluster within timeout; falling back to platform wake");
        let allow_tap = allow_play_tap_of(&self.shared).await;
        self.fire_waker(WakeReason::ConnectResume, allow_tap);
        return Ok(());
      }
    };
    if cluster_playing(&cluster) {
      tracing::info!("connect resume: a device is actively playing; standing down");
      return Ok(());
    }
    let me = self.dealer.device_id().to_string();
    if *self.shared.placement.lock().unwrap() == Placement::Desk {
      let last_active = self.shared.last_active.lock().await.clone();
      let parked = active_device(&cluster, &me, last_active.as_deref())
        .filter(|id| cluster.device.contains_key(id))
        .filter(|id| phone_device(&cluster, &me).as_deref() != Some(id.as_str()));
      if let Some(target) = parked {
        let writer = self.writer().await?;
        tracing::info!(%target, "connect resume: resuming the parked session in place");
        if let Err(e) = writer.resume(&target).await {
          tracing::warn!(error = %e, "connect resume: in-place resume failed");
        }
        return Ok(());
      }
    }
    let mut woke = false;
    let phone = match phone_device(&cluster, &me) {
      Some(p) => p,
      None => {
        tracing::info!("connect resume: phone spotify not in the cluster; waking it");
        woke = true;
        if !self.fire_waker(WakeReason::ConnectResume, false) {
          return Ok(());
        }
        match tokio::time::timeout(CONNECT_RESUME_WAKE_TIMEOUT, self.await_phone(&me)).await {
          Ok(p) => p,
          Err(_) => {
            tracing::warn!("connect resume: no phone registered after wake; standing down");
            return Ok(());
          }
        }
      }
    };
    if self.cluster_playing_now().await {
      tracing::info!("connect resume: playback started while reconciling; standing down");
      return Ok(());
    }
    let active = self
      .shared
      .cluster
      .lock()
      .await
      .as_ref()
      .map(|c| c.active_device_id.clone())
      .unwrap_or_default();
    let writer = self.writer().await?;
    if !active.is_empty() && active != phone {
      tracing::info!(from = %active, to = %phone, "connect resume: transferring parked session to the phone");
      match writer.transfer(&phone).await {
        Ok(_) => {
          let _ = tokio::time::timeout(TRANSFER_SETTLE_TIMEOUT, self.await_active(&phone)).await;
          if self.cluster_playing_now().await {
            return Ok(());
          }
        }
        Err(e) => tracing::warn!(error = %e, "connect resume: transfer failed; attempting direct resume"),
      }
    }
    tracing::info!(target = %phone, "connect resume: resuming playback on the phone");
    if let Err(e) = writer.resume(&phone).await {
      tracing::warn!(error = %e, "connect resume: resume command failed");
    }
    if tokio::time::timeout(CONNECT_RESUME_CONFIRM_TIMEOUT, self.await_playing())
      .await
      .is_ok()
    {
      return Ok(());
    }
    if woke {
      tracing::warn!("connect resume: playback never started after wake + resume; standing down");
      return Ok(());
    }

    tracing::info!("connect resume: playback did not start; escalating to platform wake");
    let changed = self.shared.cluster_changed.notified();
    tokio::pin!(changed);
    changed.as_mut().enable();
    if !self.fire_waker(WakeReason::ConnectResume, false) {
      return Ok(());
    }
    if tokio::time::timeout(CONNECT_RESUME_WAKE_TIMEOUT, changed)
      .await
      .is_err()
    {
      tracing::warn!("connect resume: no cluster update after wake; standing down");
      return Ok(());
    }
    if self.cluster_playing_now().await {
      tracing::info!("connect resume: the wake resumed playback on its own");
      return Ok(());
    }
    let phone = match self
      .shared
      .cluster
      .lock()
      .await
      .as_ref()
      .and_then(|c| phone_device(c, &me))
    {
      Some(p) => p,
      None => {
        tracing::warn!("connect resume: no phone in the cluster after wake; standing down");
        return Ok(());
      }
    };
    tracing::info!(target = %phone, "connect resume: retrying resume after wake");
    writer.resume(&phone).await?;
    if tokio::time::timeout(CONNECT_RESUME_CONFIRM_TIMEOUT, self.await_playing())
      .await
      .is_err()
    {
      tracing::warn!("connect resume: playback never started after wake + resume; standing down");
    }
    Ok(())
  }
  pub async fn skip_next(&self) -> Result<()> {
    self.writer().await?.skip_next(&self.target().await?).await?;
    Ok(())
  }
  pub async fn skip_prev(&self) -> Result<()> {
    self.writer().await?.skip_prev(&self.target().await?).await?;
    Ok(())
  }
  pub async fn seek(&self, position_ms: i64) -> Result<()> {
    self.writer().await?.seek_to(&self.target().await?, position_ms).await?;
    Ok(())
  }
  pub async fn set_shuffle(&self, on: bool) -> Result<()> {
    self.writer().await?.set_shuffle(&self.target().await?, on).await?;
    Ok(())
  }
  pub async fn set_repeat(&self, mode: RepeatMode) -> Result<()> {
    let writer = self.writer().await?;
    let target = self.target().await?;
    writer.set_repeat_context(&target, mode == RepeatMode::Context).await?;
    writer.set_repeat_track(&target, mode == RepeatMode::Track).await?;
    Ok(())
  }
  pub async fn set_volume(&self, percent: f64) -> Result<()> {
    self.writer().await?.set_volume(&self.target().await?, percent).await?;
    Ok(())
  }
  pub async fn active_device_volume_percent(&self) -> Option<f64> {
    let guard = self.shared.cluster.lock().await;
    let cluster = guard.as_ref()?;
    if cluster.active_device_id.is_empty() {
      return None;
    }
    let info = cluster.device.get(&cluster.active_device_id)?;
    Some(f64::from(info.volume) / 65535.0 * 100.0)
  }
  pub async fn volume_step(&self, delta_percent: f64) -> Result<f64> {
    let current = self.active_device_volume_percent().await.unwrap_or(50.0);
    let target = (current + delta_percent).clamp(0.0, 100.0);
    self.set_volume(target).await?;
    Ok(target)
  }
  pub async fn queue_uri(&self, uri: &str, position: QueuePosition) -> Result<()> {
    let writer = self.writer().await?;
    let target = self.target().await?;
    let filtered_index = match position {
      QueuePosition::Append => {
        writer.add_to_queue(&target, uri).await?;
        return Ok(());
      }
      QueuePosition::Next => 0,
      QueuePosition::Index { at } => at,
    };

    let Some(state) = self.upcoming_state().await else {
      writer.add_to_queue(&target, uri).await?;
      return Ok(());
    };
    ensure_insertable(&state)?;
    let Err(err) = splice_queue(&writer, &target, uri, filtered_index, &state).await else {
      return Ok(());
    };
    tracing::warn!(error = %err, "spotify queue: positional insert refused; re-reading the cluster once");
    let Some(fresh) = writer
      .cluster()
      .await
      .ok()
      .map(|c| (*c.player_state).clone())
      .filter(|ps| !ps.next_tracks.is_empty())
    else {
      return Err(err);
    };
    ensure_insertable(&fresh)?;
    splice_queue(&writer, &target, uri, filtered_index, &fresh).await
  }
  pub async fn transfer(&self, device_id: &str) -> Result<()> {
    self.writer().await?.transfer(device_id).await?;
    Ok(())
  }

  pub async fn play(&self, uri: &str, skip_to_uri: Option<String>) -> Result<()> {
    if uri == DJ_URI {
      return self.play_dj().await;
    }
    let (context, skip) = if uri.starts_with("spotify:track:") && skip_to_uri.is_none() {
      match self.album_for_track(uri).await {
        Some(album) => (album, Some(uri.to_string())),
        None => (uri.to_string(), None),
      }
    } else {
      (uri.to_string(), skip_to_uri)
    };
    let mut ppo = json!({ "license": "premium" });
    if let Some(s) = skip {
      ppo["skip_to"] = json!({ "track_uri": s });
    }
    let cmd = json!({
        "endpoint": "play",
        "context": {"uri": context, "url": format!("context://{context}"), "metadata": {}},
        "play_origin": {"feature_identifier": "harmony", "feature_version": "9.1.52.1394", "referrer_identifier": "home"},
        "prepare_play_options": ppo,
        "play_options": {"reason": "interactive", "operation": "replace", "trigger": "immediately"},
    });
    self.verified_play(cmd, &context).await
  }

  // ---- content ------------------------------------------------------------

  pub async fn search(&self, query: &str, limit: u32) -> Result<SearchResults> {
    Ok(bucket_search(self.search_flat(query, limit).await?, limit))
  }

  pub async fn resolve_voice(&self, req: VoiceResolveRequest) -> VoiceResult<VoiceResolved> {
    resolver::resolve(self, req).await
  }

  pub async fn product(&self) -> Result<ProductState> {
    let v = self.spc.product_state().await?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let product = s("product");
    let catalogue = s("catalogue");
    let is_premium = product == "premium" || catalogue == "premium";
    let country = s("country");
    self.http.set_market(&country, &catalogue).await;
    Ok(ProductState {
      country,
      is_premium,
      can_use_superbird: is_premium,
      product,
      catalogue,
    })
  }

  pub async fn root_browse(&self, sections: Option<u32>, preview: Option<u32>) -> Result<Vec<Shelf>> {
    let preview = preview.unwrap_or(PREVIEW).min(PREVIEW) as usize;
    let user = self.username().await.ok();
    let (home, playlists, albums, artists, shows) = tokio::join!(
      self.spc.get_home("en"),
      self.playlist_uris(),
      self.collection_uris("collection", Some("album")),
      self.collection_uris("artist", None),
      self.collection_uris("show", None),
    );
    let albums = albums.unwrap_or_default();
    let artists = artists.unwrap_or_default();
    let shows = shows.unwrap_or_default();
    let mut playlists_all: Vec<String> = Vec::new();
    if let Some(u) = &user {
      playlists_all.push(format!("spotify:user:{u}:collection"));
    }
    playlists_all.extend(playlists.unwrap_or_default());

    let casita_rows: Vec<(String, String, Vec<String>, usize)> = match home {
      Ok(h) => h
        .body
        .sections
        .iter()
        .filter_map(|s| {
          let (title, uris) = carousel_of(s);
          if uris.is_empty() || title.is_empty() {
            return None;
          }
          let total = uris.len();
          Some((s.id.uri.clone(), title, uris.into_iter().take(preview).collect(), total))
        })
        .collect(),
      Err(_) => Vec::new(),
    };

    let take = |v: &[String]| v.iter().take(preview).cloned().collect::<Vec<_>>();
    let mut rows: Vec<(String, String, Vec<String>, usize)> = vec![
      (
        NODE_PLAYLISTS.into(),
        "Playlists".into(),
        take(&playlists_all),
        playlists_all.len(),
      ),
      (NODE_ALBUMS.into(), "Albums".into(), take(&albums), albums.len()),
      (NODE_ARTISTS.into(), "Artists".into(), take(&artists), artists.len()),
      (NODE_PODCASTS.into(), "Podcasts".into(), take(&shows), shows.len()),
    ];
    rows.extend(casita_rows);
    rows.retain(|(_, _, _, total)| *total > 0);
    if let Some(cap) = sections {
      rows.truncate(cap as usize);
    }

    if preview == 0 {
      return Ok(
        rows
          .into_iter()
          .map(|(id, title, _, total)| Shelf {
            id,
            title,
            items: Vec::new(),
            total: total as u32,
          })
          .collect(),
      );
    }

    let mut union: Vec<String> = Vec::new();
    for (_, _, uris, _) in &rows {
      union.extend(uris.iter().cloned());
    }
    let mut seen = std::collections::HashSet::new();
    union.retain(|u| seen.insert(u.clone()));
    let map = self.hydrate_map(&union).await;

    Ok(
      rows
        .into_iter()
        .filter_map(|(id, title, uris, total)| {
          let items: Vec<BrowseItem> = uris
            .iter()
            .filter_map(|u| map.get(u).cloned())
            .filter(|i| !i.title.is_empty())
            .collect();
          if items.is_empty() {
            return None;
          }
          Some(Shelf {
            id,
            title,
            items,
            total: total as u32,
          })
        })
        .collect(),
    )
  }

  pub async fn browse(&self, node_id: &str, limit: u32, offset: u32) -> Result<BrowsePage> {
    let all: Vec<String> = match node_id {
      NODE_RECENTS => self.recent_track_uris().await?,
      NODE_PLAYLISTS => {
        let mut u = Vec::new();
        if let Ok(user) = self.username().await {
          u.push(format!("spotify:user:{user}:collection"));
        }
        u.extend(self.playlist_uris().await.unwrap_or_default());
        u
      }
      NODE_ALBUMS => self.collection_uris("collection", Some("album")).await?,
      NODE_ARTISTS => self.collection_uris("artist", None).await?,
      NODE_PODCASTS => self.collection_uris("show", None).await?,
      _ => return self.browse_container(node_id, limit, offset).await,
    };
    self.page_uris(&all, offset, limit).await
  }

  pub async fn resolve_context(&self, uri: &str) -> Result<BrowseItem> {
    Ok(
      self
        .hydrate_uris(std::slice::from_ref(&uri.to_string()))
        .await
        .into_iter()
        .next()
        .unwrap_or_default(),
    )
  }

  pub async fn favorites_list(&self, limit: u32, offset: u32) -> Result<BrowsePage> {
    let user = self.username().await?;
    let all = self.liked_uris(&user).await?;
    let mut page = self.page_uris(&all, offset, limit).await?;
    for it in page.items.iter_mut() {
      it.saved = true;
    }
    Ok(page)
  }

  // ---- favorites ----------------------------------------------------------

  pub async fn favorites_contains(&self, uris: Vec<String>) -> Result<Vec<bool>> {
    let user = self.username().await?;
    let liked = self.liked_uris(&user).await?;
    let set: std::collections::HashSet<&String> = liked.iter().collect();
    Ok(uris.iter().map(|u| set.contains(u)).collect())
  }

  pub async fn favorites_set(&self, uri: &str, liked: bool) -> Result<()> {
    let user = self.username().await?;
    let one = [uri.to_string()];
    if liked {
      self.spc.collection_write(&user, "collection", &one, &[]).await?;
    } else {
      self.spc.collection_write(&user, "collection", &[], &one).await?;
    }
    let mut guard = self.liked.lock().await;
    if let Some(cache) = guard.as_mut() {
      cache.retain(|u| u != uri);
      if liked {
        cache.insert(0, uri.to_string());
      }
    }
    drop(guard);
    self.browse_cache.lock().await.note_saved_changed();
    Ok(())
  }

  // ---- pairing ------------------------------------------------------------

  pub async fn begin_device_flow(&self) -> Result<DeviceFlow> {
    self.auth.begin_device_flow().await
  }

  pub async fn complete_device_flow(&self, flow: DeviceFlow) -> Result<()> {
    self.auth.complete_device_flow(&flow).await
  }
}

const LIKED_SONGS_ART_REF: &str = "builtin:liked-songs";

fn liked_songs_item(uri: &str) -> BrowseItem {
  BrowseItem {
    uri: uri.to_string(),
    title: "Liked Songs".to_string(),
    subtitle: "Playlist".to_string(),
    image_id: LIKED_SONGS_ART_REF.to_string(),
    playable: true,
    has_children: true,
    ..Default::default()
  }
}

pub(crate) fn carousel_of(section: &crate::proto::custom::casita_home::Section) -> (String, Vec<String>) {
  for car in [&section.shortcuts, &section.carousel, &section.list_carousel] {
    if let Some(c) = car.as_ref() {
      let uris: Vec<String> = c.items.inner.items.iter().map(|i| i.uri.clone()).collect();
      if !uris.is_empty() {
        return (c.header.title.text.clone(), uris);
      }
    }
  }
  (String::new(), Vec::new())
}

pub(crate) struct FlatItem {
  pub(crate) uri: String,
  pub(crate) name: String,
  image: String,
  pub(crate) artist: Option<String>,
  pub(crate) year: Option<i32>,
}

#[derive(Clone)]
pub(crate) struct Release {
  pub(crate) uri: String,
  pub(crate) name: String,
  pub(crate) released: (i32, i32, i32),
  pub(crate) popularity: i32,
}

fn some_uri(uri: &str) -> Option<String> {
  (!uri.is_empty()).then(|| uri.to_string())
}

pub(crate) struct PlaybackAnchor {
  pub(crate) track_uri: String,
  pub(crate) album_uri: Option<String>,
  pub(crate) artist_uri: Option<String>,
  pub(crate) context_uri: Option<String>,
}

pub(crate) fn flatten_search(resp: &crate::proto::custom::searchview::SearchResponse) -> Vec<FlatItem> {
  fn named(name: &str) -> Option<String> {
    (!name.is_empty()).then(|| name.to_string())
  }
  fn meta_of(it: &crate::proto::custom::searchview::SearchItem) -> (Option<String>, Option<i32>) {
    if let Some(album) = it.album.as_ref() {
      let artist = named(&album.artist_name).or_else(|| album.artists.first().and_then(|a| named(&a.name)));
      (artist, (album.year > 0).then_some(album.year))
    } else if let Some(track) = it.track.as_ref() {
      (track.artists.first().and_then(|a| named(&a.name)), None)
    } else {
      (None, None)
    }
  }
  let mut out = Vec::new();
  let mut seen = std::collections::HashSet::new();
  let mut push = |it: &crate::proto::custom::searchview::SearchItem, out: &mut Vec<FlatItem>| {
    if !it.uri.is_empty() && seen.insert(it.uri.clone()) {
      let (artist, year) = meta_of(it);
      out.push(FlatItem {
        uri: it.uri.clone(),
        name: it.name.clone(),
        image: it.image.clone(),
        artist,
        year,
      });
    }
  };
  for it in &resp.items {
    if let Some(section) = it.section.as_ref() {
      for entry in &section.entries {
        push(&entry.item.entity, &mut out);
      }
    } else {
      push(it, &mut out);
    }
  }
  out
}

fn bucket_search(items: Vec<FlatItem>, limit: u32) -> SearchResults {
  let mut out = SearchResults::default();
  for item in items {
    let kind = item.uri.split(':').nth(1);
    let leaf = matches!(kind, Some("track" | "episode"));
    let bucket = match kind {
      Some("track") => &mut out.tracks,
      Some("album") => &mut out.albums,
      Some("artist") => &mut out.artists,
      Some("playlist") => &mut out.playlists,
      Some("show") => &mut out.shows,
      Some("episode") => &mut out.episodes,
      _ => continue,
    };
    if bucket.len() < limit as usize {
      bucket.push(BrowseItem {
        image_id: model::cdn_image_ref(&item.image),
        uri: item.uri,
        title: item.name,
        playable: true,
        has_children: !leaf,
        ..Default::default()
      });
    }
  }
  out
}

fn is_auth_terminal(e: &Error) -> bool {
  matches!(e, Error::InvalidGrant | Error::NotPaired)
}

async fn fetch_liked_uris(spc: &SpClient, username: &str) -> Result<Vec<String>> {
  let items = spc.collection_paging(username, "collection", 1000).await?;
  Ok(
    items
      .into_iter()
      .map(|i| i.uri)
      .filter(|u| u.starts_with("spotify:track:"))
      .collect(),
  )
}

async fn resolve_context_name(spc: &SpClient, uri: &str) -> Option<String> {
  match uri.split(':').nth(1) {
    Some("playlist") => {
      let id = uri.rsplit(':').next()?;
      let pl = spc.get_playlist(id, 0, Some(1)).await.ok()?;
      let name = pl.attributes.name();
      (!name.is_empty()).then(|| name.to_string())
    }
    Some("album") => {
      let map = spc.get_albums(std::slice::from_ref(&uri.to_string())).await.ok()?;
      map.get(uri).map(|a| a.name().to_string())
    }
    Some("artist") => {
      let map = spc.get_artists(std::slice::from_ref(&uri.to_string())).await.ok()?;
      map.get(uri).map(|a| a.name().to_string())
    }
    _ if uri.ends_with(":collection") => Some("Liked Songs".to_string()),
    _ => None,
  }
}

const RECONNECT_BASE: Duration = Duration::from_secs(2);
const RECONNECT_CEILING: Duration = Duration::from_secs(64);

fn reconnect_delay(attempt: u32) -> Duration {
  let unjittered = RECONNECT_BASE
    .checked_mul(1u32.checked_shl(attempt).unwrap_or(u32::MAX))
    .unwrap_or(RECONNECT_CEILING)
    .min(RECONNECT_CEILING);
  let half = unjittered / 2;
  half + half.mul_f64(rand::random::<f64>())
}

async fn events_loop(
  dealer: Dealer,
  spc: SpClient,
  observer: Arc<dyn Observer>,
  shared: Arc<Shared>,
  liked: Arc<Mutex<Option<Vec<String>>>>,
  browse_cache: Arc<Mutex<BrowseCache>>,
  me: String,
) {
  let mut attempt: u32 = 0;
  loop {
    match dealer.open().await {
      Ok((mut stream, writer)) => match writer.cluster().await {
        Ok(cluster) => {
          attempt = 0;
          tracing::info!(
            active_device = %cluster.active_device_id,
            devices = cluster.device.len(),
            "dealer connected"
          );
          *browse_cache.lock().await = BrowseCache::default();
          *shared.writer.lock().await = Some(writer);
          let mut emitter = Emitter {
            spc: &spc,
            observer: &observer,
            liked: &liked,
            me: &me,
            last_np: String::new(),
            last_q: String::new(),
            last_dev: String::new(),
            hydrated: None,
            q_hydrate: HashMap::new(),
            ctx_names: HashMap::new(),
          };
          emitter.emit(&cluster, true).await;
          if !cluster.active_device_id.is_empty() {
            *shared.last_active.lock().await = Some(cluster.active_device_id.clone());
          }
          *shared.cluster.lock().await = Some(cluster);
          shared.cluster_changed.notify_waiters();
          let mut pending_saved = false;
          let mut pending_playlists = false;
          let debounce = tokio::time::sleep(LIBRARY_CHANGE_DEBOUNCE);
          tokio::pin!(debounce);
          loop {
            tokio::select! {
                event = stream.next_event() => match event {
                  Ok(Some(DealerEvent::Cluster(cluster))) => {
                    emitter.emit(&cluster, false).await;
                    if !cluster.active_device_id.is_empty() {
                      *shared.last_active.lock().await = Some(cluster.active_device_id.clone());
                    }
                    *shared.cluster.lock().await = Some(cluster);
            shared.cluster_changed.notify_waiters();
                  }
                  Ok(Some(DealerEvent::LibraryChanged(scope))) => {
                    match scope {
                      LibraryScope::Saved => {
                        *liked.lock().await = None;
                        browse_cache.lock().await.note_saved_changed();
                        pending_saved = true;
                      }
                      LibraryScope::Playlists => {
                        browse_cache.lock().await.note_playlists_changed();
                        pending_playlists = true;
                      }
                    }
                    debounce.as_mut().reset(tokio::time::Instant::now() + LIBRARY_CHANGE_DEBOUNCE);
                  }
                  Ok(None) => break,
                  Err(e) => {
                    tracing::warn!("dealer read error: {e}");
                    break;
                  }
                },
                _ = &mut debounce, if pending_saved || pending_playlists => {
                  if std::mem::take(&mut pending_saved) {
                    observer.on_library_changed(LibraryScope::Saved);
                  }
                  if std::mem::take(&mut pending_playlists) {
                    observer.on_library_changed(LibraryScope::Playlists);
                  }
                }
              }
          }
          if std::mem::take(&mut pending_saved) {
            observer.on_library_changed(LibraryScope::Saved);
          }
          if std::mem::take(&mut pending_playlists) {
            observer.on_library_changed(LibraryScope::Playlists);
          }
        }
        Err(e) if is_auth_terminal(&e) => {
          observer.on_auth(AuthState::Failed { reason: e.to_string() });
          return;
        }
        Err(e) => tracing::warn!("cluster register failed: {e}"),
      },
      Err(e) if is_auth_terminal(&e) => {
        observer.on_auth(AuthState::Failed { reason: e.to_string() });
        return;
      }
      Err(e) => tracing::warn!("dealer open failed: {e}"),
    }
    *shared.writer.lock().await = None;
    let delay = reconnect_delay(attempt);
    tracing::debug!(?delay, attempt, "dealer: backing off before the next attempt");
    attempt = attempt.saturating_add(1);
    tokio::time::sleep(delay).await;
  }
}

fn ensure_insertable(state: &PbPlayerState) -> Result<()> {
  let r = &state.restrictions;
  for reasons in [
    &r.disallow_set_queue_reasons,
    &r.disallow_inserting_into_next_tracks_reasons,
  ] {
    if !reasons.is_empty() {
      return Err(Error::other(format!(
        "the active device forbids positional queue inserts: {}",
        reasons.join(", ")
      )));
    }
  }
  Ok(())
}

async fn splice_queue(
  writer: &DealerWriter,
  target: &str,
  uri: &str,
  filtered_index: u32,
  state: &PbPlayerState,
) -> Result<()> {
  let mut next = state.next_tracks.clone();
  let at = model::raw_next_index(&next, filtered_index);
  next.insert(at, dealer::queued_track(uri));
  next.truncate(UPCOMING_CAP.max(at + 1));
  writer
    .set_queue(target, &next, &state.prev_tracks, &state.queue_revision)
    .await?;
  Ok(())
}

struct Emitter<'a> {
  spc: &'a SpClient,
  observer: &'a Arc<dyn Observer>,
  liked: &'a Mutex<Option<Vec<String>>>,
  me: &'a str,
  last_np: String,
  last_q: String,
  last_dev: String,
  hydrated: Option<(String, Track)>,
  q_hydrate: HashMap<String, Track>,
  ctx_names: HashMap<String, String>,
}

impl Emitter<'_> {
  async fn emit(&mut self, cluster: &Cluster, force: bool) {
    let ps = &cluster.player_state;
    let o = &ps.options;
    let r = &ps.restrictions;
    let np_sig = format!(
      "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
      ps.track.uri,
      ps.is_paused,
      ps.position_as_of_timestamp,
      cluster.active_device_id,
      o.shuffling_context,
      o.repeating_context,
      o.repeating_track,
      r.disallow_seeking_reasons.is_empty(),
      r.disallow_skipping_next_reasons.is_empty(),
      r.disallow_skipping_prev_reasons.is_empty(),
      r.disallow_toggling_shuffle_reasons.is_empty(),
      r.disallow_toggling_repeat_context_reasons.is_empty(),
      r.disallow_toggling_repeat_track_reasons.is_empty(),
      r.disallow_set_queue_reasons.is_empty(),
      r.disallow_inserting_into_next_tracks_reasons.is_empty(),
      r.disallow_add_to_queue_reasons.is_empty(),
    );
    let dev_sig = cluster
      .device
      .iter()
      .map(|(id, info)| format!("{id}:{}:{}", info.volume, *id == cluster.active_device_id))
      .collect::<Vec<_>>()
      .join(",");
    if force || dev_sig != self.last_dev {
      self.last_dev = dev_sig;
      self.observer.on_devices(model::devices(cluster, self.me));
    }

    if force || np_sig != self.last_np {
      self.last_np = np_sig;
      let mut state = model::player_state(cluster);
      if let Some(track) = state.track.as_mut() {
        self.hydrate_track(track).await;
        self.fill_saved(track).await;
        if state.duration_ms == 0 {
          state.duration_ms = track.duration_ms;
        }
      }
      if state.context_name.is_empty()
        && !state.context_uri.is_empty()
        && let Some(name) = self.context_name(&state.context_uri).await
      {
        state.context_name = name;
      }
      self.observer.on_player(state);
    }

    let q_sig = ps
      .next_tracks
      .iter()
      .map(|t| t.uri.as_str())
      .collect::<Vec<_>>()
      .join(",");
    if force || q_sig != self.last_q {
      self.last_q = q_sig;
      let mut queue = model::queue(cluster);
      self.hydrate_queue(&mut queue).await;
      self.observer.on_queue(queue);
    }
  }

  async fn hydrate_track(&mut self, track: &mut Track) {
    if !(track.uri.starts_with("spotify:track:") && (track.artists.is_empty() || track.duration_ms == 0)) {
      return;
    }
    if let Some((uri, cached)) = &self.hydrated
      && uri == &track.uri
    {
      model::fill_track_from_cached(track, cached);
      return;
    }
    if let Ok(map) = self.spc.get_tracks(std::slice::from_ref(&track.uri)).await
      && let Some(t) = map.get(&track.uri)
    {
      model::fill_track_from_proto(track, t);
      self.hydrated = Some((track.uri.clone(), track.clone()));
    }
  }

  async fn hydrate_queue(&mut self, q: &mut Queue) {
    let need: Vec<String> = q
      .next
      .iter()
      .filter(|t| t.uri.starts_with("spotify:track:") && !self.q_hydrate.contains_key(&t.uri))
      .map(|t| t.uri.clone())
      .collect();
    if !need.is_empty()
      && let Ok(map) = self.spc.get_tracks(&need).await
    {
      for t in &q.next {
        if self.q_hydrate.contains_key(&t.uri) {
          continue;
        }
        if let Some(proto) = map.get(&t.uri) {
          let mut filled = t.clone();
          model::fill_track_from_proto(&mut filled, proto);
          self.q_hydrate.insert(t.uri.clone(), filled);
        }
      }
    }
    for t in q.next.iter_mut() {
      if let Some(cached) = self.q_hydrate.get(&t.uri) {
        model::fill_track_from_cached(t, cached);
      }
    }
    let live: HashSet<&str> = q.next.iter().map(|t| t.uri.as_str()).collect();
    self.q_hydrate.retain(|k, _| live.contains(k.as_str()));
  }

  async fn fill_saved(&self, track: &mut Track) {
    if track.saved {
      return;
    }
    if let Some(cache) = self.liked.lock().await.as_ref() {
      track.saved = cache.iter().any(|u| u == &track.uri);
    }
  }

  async fn context_name(&mut self, uri: &str) -> Option<String> {
    if let Some(name) = self.ctx_names.get(uri) {
      return Some(name.clone());
    }
    let name = resolve_context_name(self.spc, uri).await?;
    self.ctx_names.insert(uri.to_string(), name.clone());
    Some(name)
  }
}

#[cfg(test)]
pub(crate) mod tests {
  use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicUsize, Ordering},
  };

  use bridgething_io::{HttpDownloadSink, HttpRequest, HttpResponse, HttpSink, HttpTransport};
  use librespot_protocol::{
    connect::{Cluster, DeviceInfo},
    devices::DeviceType,
  };
  use protobuf::Message;

  use super::*;
  use crate::auth::TokenStore;

  #[test]
  fn reconnect_delay_doubles_under_a_ceiling_and_never_repeats_the_same_instant() {
    for attempt in 0..40u32 {
      let unjittered = RECONNECT_BASE
        .checked_mul(1u32.checked_shl(attempt).unwrap_or(u32::MAX))
        .unwrap_or(RECONNECT_CEILING)
        .min(RECONNECT_CEILING);
      for _ in 0..64 {
        let delay = reconnect_delay(attempt);
        assert!(
          delay >= unjittered / 2 && delay < unjittered,
          "attempt {attempt} delay {delay:?} left the half-to-full jitter band around {unjittered:?}"
        );
      }
    }

    assert!(
      reconnect_delay(0) < reconnect_delay(8),
      "a persistent failure has to wait longer than the first retry"
    );

    let mut seen = std::collections::HashSet::new();
    for _ in 0..64 {
      seen.insert(reconnect_delay(4));
    }
    assert!(
      seen.len() > 1,
      "a fleet retrying in lockstep is what jitter exists to break"
    );
  }

  struct SeedStore;
  impl TokenStore for SeedStore {
    fn load_refresh_token(&self) -> Option<String> {
      Some("refresh-token".to_string())
    }
    fn save_refresh_token(&self, _token: String) {}
    fn load_username(&self) -> Option<String> {
      None
    }
    fn save_username(&self, _username: String) {}
  }

  pub(crate) struct NullObserver;
  impl Observer for NullObserver {
    fn on_player(&self, _state: PlayerState) {}
    fn on_queue(&self, _queue: Queue) {}
    fn on_devices(&self, _devices: Vec<Device>) {}
    fn on_auth(&self, _state: AuthState) {}
    fn on_library_changed(&self, _scope: LibraryScope) {}
  }

  #[derive(Default)]
  struct RouteTransport {
    hits: Arc<StdMutex<Vec<(String, String)>>>,
    #[allow(clippy::type_complexity)]
    resume_flip: Arc<StdMutex<Option<(Arc<Shared>, Cluster)>>>,
    #[allow(clippy::type_complexity)]
    play_flip: Arc<StdMutex<Option<(Arc<Shared>, Cluster)>>>,
    set_queue_failures: Arc<AtomicUsize>,
    player_404s: Arc<AtomicUsize>,
    cluster_bytes: Arc<StdMutex<Option<Vec<u8>>>>,
    search_bytes: Arc<StdMutex<Option<Vec<u8>>>>,
  }
  impl HttpTransport for RouteTransport {
    fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>) {
      let url = request.url.clone();
      if url.contains("clienttoken") {
        sink.fail("no client token in tests".to_string());
        return;
      }
      if url.contains("/api/token") {
        sink.complete(HttpResponse {
          status: 200,
          headers: Vec::new(),
          body: br#"{"access_token":"bearer","expires_in":3600}"#.to_vec(),
        });
        return;
      }
      let body = String::from_utf8_lossy(&request.body).into_owned();
      let resumed = body.contains("\"endpoint\":\"resume\"");
      let stale_queue = body.contains("\"endpoint\":\"set_queue\"")
        && self
          .set_queue_failures
          .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1))
          .is_ok();
      let played = body.contains("\"endpoint\":\"play\"");
      self.hits.lock().unwrap().push((url.clone(), body));
      if url.contains("/player/command/")
        && self
          .player_404s
          .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1))
          .is_ok()
      {
        sink.complete(HttpResponse {
          status: 404,
          headers: Vec::new(),
          body: br#"{"error_type":"DEVICE_NOT_FOUND","message":"Device not found, from edgeproxy"}"#.to_vec(),
        });
        return;
      }
      if resumed && let Some((shared, cluster)) = self.resume_flip.lock().unwrap().clone() {
        tokio::spawn(async move {
          *shared.cluster.lock().await = Some(cluster);
          shared.cluster_changed.notify_waiters();
        });
      }
      if played && let Some((shared, cluster)) = self.play_flip.lock().unwrap().clone() {
        tokio::spawn(async move {
          *shared.cluster.lock().await = Some(cluster);
          shared.cluster_changed.notify_waiters();
        });
      }
      if stale_queue {
        sink.complete(HttpResponse {
          status: 409,
          headers: Vec::new(),
          body: b"stale queue_revision".to_vec(),
        });
        return;
      }
      let canned = if url.contains("/connect-state/v1/devices/") {
        self.cluster_bytes.lock().unwrap().clone()
      } else if url.contains("/searchview/v3/search") {
        self.search_bytes.lock().unwrap().clone()
      } else {
        None
      };
      sink.complete(HttpResponse {
        status: 200,
        headers: Vec::new(),
        body: canned.unwrap_or_default(),
      });
    }

    fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      sink.on_failed("the test transport has no streaming arm".to_string());
    }
  }

  type Hits = Arc<StdMutex<Vec<(String, String)>>>;

  fn command_hits(hits: &Hits) -> Vec<(String, String)> {
    hits
      .lock()
      .unwrap()
      .iter()
      .filter(|(url, _)| url.contains("connect-state"))
      .cloned()
      .collect()
  }

  fn play_targets(hits: &Hits) -> Vec<String> {
    command_hits(hits)
      .iter()
      .filter(|(_, body)| body.contains("\"endpoint\":\"play\""))
      .filter_map(|(url, _)| url.split("/player/command/from/me-device/to/").nth(1))
      .map(str::to_string)
      .collect()
  }

  fn transfer_targets(hits: &Hits) -> Vec<String> {
    command_hits(hits)
      .iter()
      .filter_map(|(url, _)| url.split("/connect/transfer/from/me-device/to/").nth(1))
      .map(str::to_string)
      .collect()
  }

  fn resume_targets(hits: &Hits) -> Vec<String> {
    command_hits(hits)
      .iter()
      .filter(|(_, body)| body.contains("\"endpoint\":\"resume\""))
      .filter_map(|(url, _)| url.split("/player/command/from/me-device/to/").nth(1))
      .map(str::to_string)
      .collect()
  }

  struct FakeWaker {
    calls: Arc<AtomicUsize>,
    wakes: Arc<StdMutex<Vec<(WakeReason, bool)>>>,
    inject: Option<(Arc<Shared>, Cluster)>,
  }
  impl DeviceWaker for FakeWaker {
    fn wake_device(&self, reason: WakeReason, allow_play_tap: bool) {
      self.calls.fetch_add(1, Ordering::SeqCst);
      self.wakes.lock().unwrap().push((reason, allow_play_tap));
      if let Some((shared, cluster)) = self.inject.clone() {
        tokio::spawn(async move {
          *shared.cluster.lock().await = Some(cluster);
          shared.cluster_changed.notify_waiters();
        });
      }
    }
  }

  fn device_info(kind: DeviceType) -> DeviceInfo {
    let mut di = DeviceInfo::new();
    di.device_type = kind.into();
    di
  }

  fn cluster(active: &str, playing: bool, devices: &[(&str, DeviceType)]) -> Cluster {
    let mut c = Cluster::new();
    c.active_device_id = active.to_string();
    let ps = c.player_state.mut_or_insert_default();
    ps.is_playing = playing;
    ps.is_paused = !playing;
    for (id, kind) in devices {
      c.device.insert(id.to_string(), device_info(*kind));
    }
    c
  }

  fn active_cluster(id: &str) -> Cluster {
    cluster(id, false, &[(id, DeviceType::SMARTPHONE)])
  }

  struct Rig {
    client: SpotifyClient,
    hits: Hits,
    wake_calls: Arc<AtomicUsize>,
    wakes: Arc<StdMutex<Vec<(WakeReason, bool)>>>,
    #[allow(clippy::type_complexity)]
    resume_flip: Arc<StdMutex<Option<(Arc<Shared>, Cluster)>>>,
    #[allow(clippy::type_complexity)]
    play_flip: Arc<StdMutex<Option<(Arc<Shared>, Cluster)>>>,
    set_queue_failures: Arc<AtomicUsize>,
    player_404s: Arc<AtomicUsize>,
    cluster_bytes: Arc<StdMutex<Option<Vec<u8>>>>,
  }

  impl Rig {
    async fn flip_to_on_resume(&self, cluster: Cluster) {
      *self.resume_flip.lock().unwrap() = Some((self.client.shared.clone(), cluster));
    }

    async fn flip_to_on_play(&self, cluster: Cluster) {
      *self.play_flip.lock().unwrap() = Some((self.client.shared.clone(), cluster));
    }

    fn fail_player_commands(&self, n: usize) {
      self.player_404s.store(n, Ordering::SeqCst);
    }

    fn refuse_set_queue(&self, count: usize, refreshed: &Cluster) {
      self.set_queue_failures.store(count, Ordering::SeqCst);
      *self.cluster_bytes.lock().unwrap() = Some(refreshed.write_to_bytes().unwrap());
    }
  }

  async fn rig(initial_cluster: Option<Cluster>, wake_inject: Option<Cluster>) -> Rig {
    let transport = RouteTransport::default();
    let hits = transport.hits.clone();
    let resume_flip = transport.resume_flip.clone();
    let play_flip = transport.play_flip.clone();
    let set_queue_failures = transport.set_queue_failures.clone();
    let player_404s = transport.player_404s.clone();
    let cluster_bytes = transport.cluster_bytes.clone();
    let exec = HttpExecutor::new(Arc::new(transport));
    let auth = Arc::new(Auth::new(
      "https://worker.invalid",
      "psk",
      Box::new(SeedStore),
      exec.clone(),
    ));
    let client = SpotifyClient::new(auth, "me-device".to_string(), exec, Arc::new(NullObserver));
    *client.shared.writer.lock().await = Some(DealerWriter::for_test(client.http.clone(), "me-device"));
    if let Some(c) = initial_cluster {
      *client.shared.cluster.lock().await = Some(c);
    }
    let wake_calls = Arc::new(AtomicUsize::new(0));
    let wakes = Arc::new(StdMutex::new(Vec::new()));
    client.set_device_waker(Arc::new(FakeWaker {
      calls: wake_calls.clone(),
      wakes: wakes.clone(),
      inject: wake_inject.map(|c| (client.shared.clone(), c)),
    }));
    Rig {
      client,
      hits,
      wake_calls,
      wakes,
      resume_flip,
      play_flip,
      set_queue_failures,
      player_404s,
      cluster_bytes,
    }
  }

  pub(crate) fn test_client(observer: Arc<dyn Observer>) -> SpotifyClient {
    client_over(RouteTransport::default(), observer)
  }

  #[derive(Default)]
  pub(crate) struct Playing<'a> {
    pub(crate) track: &'a str,
    pub(crate) album: &'a str,
    pub(crate) artist: &'a str,
    pub(crate) context: &'a str,
  }

  pub(crate) async fn playing_client(observer: Arc<dyn Observer>, playing: Playing<'_>) -> SpotifyClient {
    let client = test_client(observer);
    let mut c = cluster("dev1", true, &[]);
    let ps = c.player_state.mut_or_insert_default();
    ps.context_uri = playing.context.to_string();
    let track = ps.track.mut_or_insert_default();
    track.uri = playing.track.to_string();
    track.album_uri = playing.album.to_string();
    track.artist_uri = playing.artist.to_string();
    *client.shared.cluster.lock().await = Some(c);
    client
  }

  pub(crate) fn searching_client(observer: Arc<dyn Observer>, results: &[&str]) -> (SpotifyClient, SearchLog) {
    let transport = RouteTransport {
      search_bytes: Arc::new(StdMutex::new(Some(
        search_response(results, &[]).write_to_bytes().unwrap(),
      ))),
      ..Default::default()
    };
    let hits = transport.hits.clone();
    (client_over(transport, observer), SearchLog(hits))
  }

  pub(crate) struct SearchLog(Hits);

  impl SearchLog {
    pub(crate) fn queries(&self) -> Vec<String> {
      self
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|(url, _)| url.contains("/searchview/v3/search"))
        .filter_map(|(url, _)| {
          url
            .split('?')
            .nth(1)?
            .split('&')
            .find_map(|p| p.strip_prefix("query="))
            .map(|q| q.replace('+', " ").replace("%3A", ":"))
        })
        .collect()
    }
  }

  fn client_over(transport: RouteTransport, observer: Arc<dyn Observer>) -> SpotifyClient {
    let exec = HttpExecutor::new(Arc::new(transport));
    let auth = Arc::new(Auth::new(
      "https://example.invalid",
      "psk",
      Box::new(SeedStore),
      exec.clone(),
    ));
    SpotifyClient::new(auth, "me-device".to_string(), exec, observer)
  }

  #[tokio::test]
  async fn a_play_rejected_as_device_not_found_wakes_and_retries_on_the_fresh_cluster() {
    let rig = rig(Some(active_cluster("dev1")), Some(active_cluster("dev2"))).await;
    rig.fail_player_commands(1);
    rig.client.play("spotify:album:x", None).await.unwrap();
    assert_eq!(rig.wake_calls.load(Ordering::SeqCst), 1);
    assert_eq!(rig.wakes.lock().unwrap()[0], (WakeReason::UserPlay, false));
    assert_eq!(play_targets(&rig.hits), vec!["dev1".to_string(), "dev2".to_string()]);
  }

  async fn eventually(what: &str, holds: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(120), async {
      loop {
        if holds() {
          return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
      }
    })
    .await
    .unwrap_or_else(|_| panic!("never held: {what}"));
  }

  #[tokio::test(start_paused = true)]
  async fn an_accepted_play_the_cluster_never_confirms_wakes_and_replays() {
    let rig = rig(Some(active_cluster("dev1")), Some(active_cluster("dev1"))).await;
    rig.client.play("spotify:album:x", None).await.unwrap();
    eventually("the recovery replayed", || {
      rig.wake_calls.load(Ordering::SeqCst) == 1 && play_targets(&rig.hits).len() == 2
    })
    .await;
    assert_eq!(rig.wakes.lock().unwrap()[0], (WakeReason::UserPlay, false));
  }

  #[tokio::test(start_paused = true)]
  async fn a_play_the_cluster_confirms_never_wakes() {
    let rig = rig(Some(active_cluster("dev1")), None).await;
    let mut confirmed = cluster("dev1", true, &[]);
    confirmed.player_state.mut_or_insert_default().context_uri = "spotify:album:x".to_string();
    rig.flip_to_on_play(confirmed).await;
    rig.client.play("spotify:album:x", None).await.unwrap();
    tokio::time::sleep(PLAY_CONFIRM_TIMEOUT + CONNECT_RESUME_WAKE_TIMEOUT).await;
    assert_eq!(rig.wake_calls.load(Ordering::SeqCst), 0);
    assert_eq!(play_targets(&rig.hits).len(), 1);
  }

  #[tokio::test]
  async fn start_target_or_wake_returns_the_phone_without_waking() {
    let client = test_client(Arc::new(NullObserver));
    *client.shared.cluster.lock().await = Some(active_cluster("dev1"));
    let calls = Arc::new(AtomicUsize::new(0));
    client.set_device_waker(Arc::new(FakeWaker {
      calls: calls.clone(),
      wakes: Arc::new(StdMutex::new(Vec::new())),
      inject: None,
    }));

    let target = client.start_target_or_wake().await.unwrap();
    assert_eq!(target, "dev1");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no wake when a phone already exists");
  }

  #[tokio::test]
  async fn start_target_or_wake_wakes_and_resolves_when_the_phone_registers() {
    let client = test_client(Arc::new(NullObserver));
    let calls = Arc::new(AtomicUsize::new(0));
    let wakes = Arc::new(StdMutex::new(Vec::new()));
    client.set_device_waker(Arc::new(FakeWaker {
      calls: calls.clone(),
      wakes: wakes.clone(),
      inject: Some((client.shared.clone(), active_cluster("dev1"))),
    }));

    let target = client.start_target_or_wake().await.unwrap();
    assert_eq!(target, "dev1");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "woke exactly once");
    assert_eq!(
      *wakes.lock().unwrap(),
      [(WakeReason::UserPlay, true)],
      "user-initiated play wakes as UserPlay; with the dealer down the tap is the only lever"
    );
  }

  // ---- resume_on_connect: phone-preferred-unless-actively-playing ----------

  #[tokio::test]
  async fn connect_resume_stands_down_when_a_device_is_actively_playing() {
    let r = rig(
      Some(cluster(
        "avr-1",
        true,
        &[("avr-1", DeviceType::AUDIO_DONGLE), ("phone-1", DeviceType::SMARTPHONE)],
      )),
      None,
    )
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 0, "active playback must not wake");
    assert!(
      command_hits(&r.hits).is_empty(),
      "active playback must not be disturbed: {:?}",
      command_hits(&r.hits)
    );
  }

  #[tokio::test]
  async fn connect_resume_transfers_parked_session_to_the_phone() {
    let r = rig(
      Some(cluster(
        "avr-1",
        false,
        &[("avr-1", DeviceType::AUDIO_DONGLE), ("phone-1", DeviceType::SMARTPHONE)],
      )),
      None,
    )
    .await;
    r.flip_to_on_resume(cluster(
      "phone-1",
      true,
      &[("avr-1", DeviceType::AUDIO_DONGLE), ("phone-1", DeviceType::SMARTPHONE)],
    ))
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 0, "a confirmed resume never wakes");
    assert_eq!(
      transfer_targets(&r.hits),
      ["phone-1"],
      "parked session moves to the phone"
    );
    assert_eq!(resume_targets(&r.hits), ["phone-1"], "resume lands on the phone");
    assert!(
      !command_hits(&r.hits).iter().any(|(url, _)| url.contains("avr-1")),
      "the idle speaker is never commanded"
    );
  }

  #[tokio::test]
  async fn connect_resume_resumes_directly_when_the_phone_is_active() {
    let r = rig(
      Some(cluster(
        "phone-1",
        false,
        &[("phone-1", DeviceType::SMARTPHONE), ("avr-1", DeviceType::AUDIO_DONGLE)],
      )),
      None,
    )
    .await;
    r.flip_to_on_resume(cluster(
      "phone-1",
      true,
      &[("phone-1", DeviceType::SMARTPHONE), ("avr-1", DeviceType::AUDIO_DONGLE)],
    ))
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(transfer_targets(&r.hits), Vec::<String>::new(), "no transfer needed");
    assert_eq!(resume_targets(&r.hits), ["phone-1"]);
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 0, "a confirmed resume never wakes");
  }

  #[tokio::test]
  async fn connect_resume_resumes_at_the_phone_when_no_session_is_active() {
    let r = rig(
      Some(cluster(
        "",
        false,
        &[("avr-1", DeviceType::AUDIO_DONGLE), ("phone-1", DeviceType::SMARTPHONE)],
      )),
      None,
    )
    .await;
    r.flip_to_on_resume(cluster(
      "phone-1",
      true,
      &[("avr-1", DeviceType::AUDIO_DONGLE), ("phone-1", DeviceType::SMARTPHONE)],
    ))
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(transfer_targets(&r.hits), Vec::<String>::new(), "nothing to transfer");
    assert_eq!(
      resume_targets(&r.hits),
      ["phone-1"],
      "the phone wins over the idle speaker"
    );
  }

  #[tokio::test]
  async fn connect_resume_wakes_and_targets_the_phone_when_absent() {
    let woken = cluster(
      "",
      false,
      &[("avr-1", DeviceType::AUDIO_DONGLE), ("phone-1", DeviceType::SMARTPHONE)],
    );
    let r = rig(
      Some(cluster("", false, &[("avr-1", DeviceType::AUDIO_DONGLE)])),
      Some(woken),
    )
    .await;
    r.flip_to_on_resume(cluster(
      "phone-1",
      true,
      &[("avr-1", DeviceType::AUDIO_DONGLE), ("phone-1", DeviceType::SMARTPHONE)],
    ))
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 1, "woke exactly once");
    assert_eq!(
      *r.wakes.lock().unwrap(),
      [(WakeReason::ConnectResume, false)],
      "on-connect wakes as ConnectResume and never permits a play tap while the dealer is up"
    );
    assert_eq!(resume_targets(&r.hits), ["phone-1"]);
    assert!(
      !command_hits(&r.hits).iter().any(|(url, _)| url.contains("avr-1")),
      "the speaker is never the fallback target"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn connect_resume_stands_down_when_no_phone_appears_after_wake() {
    let r = rig(Some(cluster("", false, &[("avr-1", DeviceType::AUDIO_DONGLE)])), None).await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 1, "the wake is still attempted");
    assert!(
      command_hits(&r.hits).is_empty(),
      "no phone means no playback command at all: {:?}",
      command_hits(&r.hits)
    );
  }

  #[tokio::test(start_paused = true)]
  async fn connect_resume_escalates_to_wake_when_the_resume_lands_nowhere() {
    let woken = cluster("", false, &[("phone-1", DeviceType::SMARTPHONE)]);
    let r = rig(
      Some(cluster("", false, &[("phone-1", DeviceType::SMARTPHONE)])),
      Some(woken),
    )
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(
      r.wake_calls.load(Ordering::SeqCst),
      1,
      "unconfirmed resume escalates to a wake"
    );
    assert_eq!(*r.wakes.lock().unwrap(), [(WakeReason::ConnectResume, false)]);
    assert_eq!(
      resume_targets(&r.hits),
      ["phone-1", "phone-1"],
      "resume is retried after the wake"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn connect_resume_escalation_stands_down_when_spotify_never_registers() {
    let r = rig(Some(cluster("", false, &[("phone-1", DeviceType::SMARTPHONE)])), None).await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 1, "the wake is still attempted");
    assert_eq!(
      resume_targets(&r.hits),
      ["phone-1"],
      "no blind retry without a cluster update"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn connect_resume_escalation_targets_the_phone_that_registers_after_wake() {
    let woken = cluster("", false, &[("phone-2", DeviceType::SMARTPHONE)]);
    let r = rig(
      Some(cluster("", false, &[("phone-1", DeviceType::SMARTPHONE)])),
      Some(woken),
    )
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
      resume_targets(&r.hits),
      ["phone-1", "phone-2"],
      "the post-wake resume targets the freshly registered phone"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn connect_resume_escalation_stands_down_when_the_wake_resumes_playback() {
    let woken = cluster("phone-1", true, &[("phone-1", DeviceType::SMARTPHONE)]);
    let r = rig(
      Some(cluster("", false, &[("phone-1", DeviceType::SMARTPHONE)])),
      Some(woken),
    )
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
      resume_targets(&r.hits),
      ["phone-1"],
      "no resume retry once the wake started playback"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn connect_resume_never_wakes_twice() {
    let woken = cluster(
      "",
      false,
      &[("avr-1", DeviceType::AUDIO_DONGLE), ("phone-1", DeviceType::SMARTPHONE)],
    );
    let r = rig(
      Some(cluster("", false, &[("avr-1", DeviceType::AUDIO_DONGLE)])),
      Some(woken),
    )
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(
      r.wake_calls.load(Ordering::SeqCst),
      1,
      "exactly one wake per connect resume"
    );
    assert_eq!(resume_targets(&r.hits), ["phone-1"]);
  }

  #[tokio::test(start_paused = true)]
  async fn connect_resume_falls_back_to_the_platform_wake_when_the_dealer_never_connects() {
    let r = rig(None, None).await;
    *r.client.shared.writer.lock().await = None;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(
      r.wake_calls.load(Ordering::SeqCst),
      1,
      "offline: the platform wake is the only lever"
    );
    assert_eq!(
      *r.wakes.lock().unwrap(),
      [(WakeReason::ConnectResume, true)],
      "a tap can only resume locally while truly offline"
    );
    assert!(command_hits(&r.hits).is_empty(), "no cluster means no dealer commands");
  }

  #[tokio::test(start_paused = true)]
  async fn connect_resume_wake_is_launch_only_when_the_dealer_is_up_but_the_cluster_is_missing() {
    let r = rig(None, None).await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
      *r.wakes.lock().unwrap(),
      [(WakeReason::ConnectResume, false)],
      "a live dealer means the session may be parked remotely; a tap could blast it"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn connect_resume_is_single_flight() {
    let r = rig(None, None).await;
    let client = Arc::new(r.client);
    let first = {
      let c = client.clone();
      tokio::spawn(async move { c.resume_on_connect().await })
    };
    tokio::task::yield_now().await;
    client.resume_on_connect().await.unwrap();
    first.await.unwrap().unwrap();
    assert_eq!(
      r.wake_calls.load(Ordering::SeqCst),
      1,
      "overlapping connect resumes collapse to one run"
    );
  }

  // ---- user-initiated start verbs: phone-pinned in a car, active-first at a desk ----

  fn car_cluster_with_parked_speaker() -> Cluster {
    cluster(
      "spk-1",
      false,
      &[("spk-1", DeviceType::SPEAKER), ("phone-1", DeviceType::SMARTPHONE)],
    )
  }

  #[tokio::test]
  async fn a_user_play_in_a_car_targets_the_phone_not_the_parked_speaker() {
    let rig = rig(Some(car_cluster_with_parked_speaker()), None).await;
    rig.client.play("spotify:album:x", None).await.unwrap();
    assert_eq!(play_targets(&rig.hits), ["phone-1"]);
    assert!(
      !command_hits(&rig.hits).iter().any(|(url, _)| url.contains("spk-1")),
      "the remote speaker is never commanded"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_user_play_in_a_car_wakes_the_phone_instead_of_targeting_a_speakers_only_cluster() {
    let woken = cluster(
      "spk-1",
      false,
      &[("spk-1", DeviceType::SPEAKER), ("phone-1", DeviceType::SMARTPHONE)],
    );
    let rig = rig(
      Some(cluster("spk-1", false, &[("spk-1", DeviceType::SPEAKER)])),
      Some(woken),
    )
    .await;
    rig.client.play("spotify:album:x", None).await.unwrap();
    assert_eq!(rig.wakes.lock().unwrap()[0], (WakeReason::UserPlay, false));
    assert_eq!(play_targets(&rig.hits), ["phone-1"]);
  }

  #[tokio::test]
  async fn a_user_resume_in_a_car_transfers_the_parked_session_to_the_phone() {
    let rig = rig(Some(car_cluster_with_parked_speaker()), None).await;
    rig.client.resume().await.unwrap();
    assert_eq!(transfer_targets(&rig.hits), ["phone-1"]);
    assert_eq!(resume_targets(&rig.hits), ["phone-1"]);
  }

  #[tokio::test]
  async fn a_user_play_at_a_desk_targets_the_active_speaker() {
    let rig = rig(Some(car_cluster_with_parked_speaker()), None).await;
    rig.client.set_placement(Placement::Desk);
    rig.client.play("spotify:album:x", None).await.unwrap();
    assert_eq!(play_targets(&rig.hits), ["spk-1"]);
  }

  #[tokio::test]
  async fn a_user_resume_at_a_desk_resumes_the_active_speaker_without_transferring() {
    let rig = rig(Some(car_cluster_with_parked_speaker()), None).await;
    rig.client.set_placement(Placement::Desk);
    rig.client.resume().await.unwrap();
    assert_eq!(transfer_targets(&rig.hits), Vec::<String>::new());
    assert_eq!(resume_targets(&rig.hits), ["spk-1"]);
  }

  #[tokio::test]
  async fn a_user_play_at_a_desk_with_nothing_active_targets_the_phone_never_a_guessed_speaker() {
    let rig = rig(
      Some(cluster(
        "",
        false,
        &[("spk-1", DeviceType::SPEAKER), ("phone-1", DeviceType::SMARTPHONE)],
      )),
      None,
    )
    .await;
    rig.client.set_placement(Placement::Desk);
    rig.client.play("spotify:album:x", None).await.unwrap();
    assert_eq!(play_targets(&rig.hits), ["phone-1"]);
  }

  #[tokio::test]
  async fn connect_resume_at_a_desk_resumes_the_parked_session_in_place() {
    let r = rig(Some(car_cluster_with_parked_speaker()), None).await;
    r.client.set_placement(Placement::Desk);
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(r.wake_calls.load(Ordering::SeqCst), 0);
    assert_eq!(transfer_targets(&r.hits), Vec::<String>::new());
    assert_eq!(
      resume_targets(&r.hits),
      ["spk-1"],
      "a desk unit picks up where it left off, next to the user"
    );
  }

  #[tokio::test]
  async fn connect_resume_at_a_desk_with_nothing_active_falls_back_to_the_phone_flow() {
    let r = rig(
      Some(cluster(
        "",
        false,
        &[("spk-1", DeviceType::SPEAKER), ("phone-1", DeviceType::SMARTPHONE)],
      )),
      None,
    )
    .await;
    r.client.set_placement(Placement::Desk);
    r.flip_to_on_resume(cluster(
      "phone-1",
      true,
      &[("spk-1", DeviceType::SPEAKER), ("phone-1", DeviceType::SMARTPHONE)],
    ))
    .await;
    r.client.resume_on_connect().await.unwrap();
    assert_eq!(resume_targets(&r.hits), ["phone-1"]);
  }

  // ---- search bucketing ----------------------------------------------------

  pub(crate) fn search_item(uri: &str) -> crate::proto::custom::searchview::SearchItem {
    let id = uri.rsplit(':').next().unwrap();
    let mut it = crate::proto::custom::searchview::SearchItem::new();
    it.uri = uri.to_string();
    it.name = id.to_uppercase();
    it.image = format!("https://i.scdn.co/image/{id}");
    it
  }

  pub(crate) fn named_hit(uri: &str, name: &str) -> crate::proto::custom::searchview::SearchItem {
    let mut it = search_item(uri);
    it.name = name.to_string();
    it
  }

  pub(crate) fn album_hit(
    uri: &str,
    name: &str,
    artist: &str,
    year: i32,
  ) -> crate::proto::custom::searchview::SearchItem {
    let mut it = named_hit(uri, name);
    let mut meta = crate::proto::custom::searchview::AlbumMeta::new();
    meta.artist_name = artist.to_string();
    meta.year = year;
    it.album = protobuf::MessageField::some(meta);
    it
  }

  pub(crate) fn track_hit(uri: &str, name: &str, artist: &str) -> crate::proto::custom::searchview::SearchItem {
    let mut it = named_hit(uri, name);
    let mut meta = crate::proto::custom::searchview::TrackMeta::new();
    let mut by = crate::proto::custom::searchview::EntityRef::new();
    by.name = artist.to_string();
    meta.artists.push(by);
    it.track = protobuf::MessageField::some(meta);
    it
  }

  pub(crate) fn searching_client_items(
    observer: Arc<dyn Observer>,
    items: Vec<crate::proto::custom::searchview::SearchItem>,
  ) -> (SpotifyClient, SearchLog) {
    let mut resp = crate::proto::custom::searchview::SearchResponse::new();
    resp.items.extend(items);
    let transport = RouteTransport {
      search_bytes: Arc::new(StdMutex::new(Some(resp.write_to_bytes().unwrap()))),
      ..Default::default()
    };
    let hits = transport.hits.clone();
    (client_over(transport, observer), SearchLog(hits))
  }

  pub(crate) fn search_response(
    loose: &[&str],
    sectioned: &[&str],
  ) -> crate::proto::custom::searchview::SearchResponse {
    use protobuf::MessageField;

    let mut resp = crate::proto::custom::searchview::SearchResponse::new();
    resp.items.extend(loose.iter().map(|u| search_item(u)));
    if !sectioned.is_empty() {
      let mut section = crate::proto::custom::searchview::Section::new();
      for uri in sectioned {
        let mut wrapper = crate::proto::custom::searchview::EntityWrapper::new();
        wrapper.entity = MessageField::some(search_item(uri));
        let mut entry = crate::proto::custom::searchview::SectionEntry::new();
        entry.item = MessageField::some(wrapper);
        section.entries.push(entry);
      }
      let mut holder = crate::proto::custom::searchview::SearchItem::new();
      holder.section = MessageField::some(section);
      resp.items.push(holder);
    }
    resp
  }

  fn uris(items: &[BrowseItem]) -> Vec<&str> {
    items.iter().map(|i| i.uri.as_str()).collect()
  }

  #[test]
  fn search_buckets_shows_and_episodes_alongside_music() {
    let resp = search_response(
      &["spotify:track:t1", "spotify:album:a1", "spotify:artist:r1"],
      &[
        "spotify:playlist:p1",
        "spotify:show:s1",
        "spotify:episode:e1",
        "spotify:user:nobody",
      ],
    );
    let out = bucket_search(flatten_search(&resp), 10);
    assert_eq!(uris(&out.tracks), ["spotify:track:t1"]);
    assert_eq!(uris(&out.albums), ["spotify:album:a1"]);
    assert_eq!(uris(&out.artists), ["spotify:artist:r1"]);
    assert_eq!(uris(&out.playlists), ["spotify:playlist:p1"]);
    assert_eq!(uris(&out.shows), ["spotify:show:s1"], "shows get their own bucket");
    assert_eq!(
      uris(&out.episodes),
      ["spotify:episode:e1"],
      "episodes get their own bucket"
    );
  }

  #[test]
  fn search_treats_episodes_as_leaves_and_shows_as_containers() {
    let resp = search_response(&["spotify:show:s1", "spotify:episode:e1", "spotify:track:t1"], &[]);
    let out = bucket_search(flatten_search(&resp), 10);
    assert!(out.shows[0].has_children, "a show browses to its episodes");
    assert!(!out.episodes[0].has_children, "an episode is playable, not browsable");
    assert!(!out.tracks[0].has_children);
    assert_eq!(out.episodes[0].image_id, "e1", "cdn urls collapse to bare image refs");
  }

  #[test]
  fn search_caps_each_bucket_at_the_limit() {
    let resp = search_response(
      &[
        "spotify:show:s1",
        "spotify:show:s2",
        "spotify:episode:e1",
        "spotify:episode:e2",
      ],
      &[],
    );
    let out = bucket_search(flatten_search(&resp), 1);
    assert_eq!(uris(&out.shows), ["spotify:show:s1"]);
    assert_eq!(uris(&out.episodes), ["spotify:episode:e1"]);
  }

  // ---- queue positioning ---------------------------------------------------

  fn queued(uri: &str) -> librespot_protocol::player::ProvidedTrack {
    let mut t = librespot_protocol::player::ProvidedTrack::new();
    t.uri = uri.to_string();
    t
  }

  fn queue_cluster(next: &[&str], revision: &str) -> Cluster {
    let mut c = cluster("phone-1", true, &[("phone-1", DeviceType::SMARTPHONE)]);
    let ps = c.player_state.mut_or_insert_default();
    ps.queue_revision = revision.to_string();
    ps.next_tracks = next.iter().map(|u| queued(u)).collect();
    ps.prev_tracks = vec![queued("spotify:track:played")];
    c
  }

  fn commands(hits: &Hits, endpoint: &str) -> Vec<serde_json::Value> {
    command_hits(hits)
      .iter()
      .filter_map(|(_, body)| serde_json::from_str::<serde_json::Value>(body).ok())
      .map(|v| v["command"].clone())
      .filter(|c| c["endpoint"] == endpoint)
      .collect()
  }

  fn sent_next_uris(command: &serde_json::Value) -> Vec<String> {
    command["next_tracks"]
      .as_array()
      .unwrap()
      .iter()
      .map(|t| t["uri"].as_str().unwrap_or_default().to_string())
      .collect()
  }

  #[tokio::test]
  async fn queue_append_stays_on_the_add_to_queue_endpoint() {
    let r = rig(Some(queue_cluster(&["spotify:track:a"], "rev-1")), None).await;
    r.client
      .queue_uri("spotify:track:new", QueuePosition::Append)
      .await
      .unwrap();
    assert_eq!(commands(&r.hits, "add_to_queue").len(), 1);
    assert!(
      commands(&r.hits, "set_queue").is_empty(),
      "append never rewrites the queue"
    );
  }

  #[tokio::test]
  async fn queue_next_splices_at_the_head_and_echoes_the_read_revision() {
    let r = rig(
      Some(queue_cluster(&["spotify:track:a", "spotify:track:b"], "rev-1")),
      None,
    )
    .await;
    r.client
      .queue_uri("spotify:track:new", QueuePosition::Next)
      .await
      .unwrap();

    let sent = commands(&r.hits, "set_queue");
    assert_eq!(sent.len(), 1);
    assert_eq!(
      sent_next_uris(&sent[0]),
      ["spotify:track:new", "spotify:track:a", "spotify:track:b"],
      "next lands at the head and nothing already upcoming is lost"
    );
    assert_eq!(sent[0]["queue_revision"], "rev-1");
    assert_eq!(
      sent[0]["prev_tracks"][0]["uri"], "spotify:track:played",
      "history is echoed back or set_queue wipes it"
    );
    assert_eq!(
      sent[0]["next_tracks"][0]["provider"], "queue",
      "the spliced row carries the queued markers"
    );
    assert_eq!(sent[0]["next_tracks"][0]["metadata"]["is_queued"], "true");
  }

  #[tokio::test]
  async fn queue_index_is_an_index_into_the_delimiter_free_list() {
    let r = rig(
      Some(queue_cluster(
        &["spotify:track:a", "spotify:delimiter", "spotify:track:b"],
        "rev-1",
      )),
      None,
    )
    .await;
    r.client
      .queue_uri("spotify:track:new", QueuePosition::Index { at: 1 })
      .await
      .unwrap();

    let sent = commands(&r.hits, "set_queue");
    assert_eq!(
      sent_next_uris(&sent[0]),
      [
        "spotify:track:a",
        "spotify:delimiter",
        "spotify:track:new",
        "spotify:track:b"
      ],
      "index 1 of the list a webapp sees is raw slot 2, past the delimiter"
    );
  }

  #[tokio::test]
  async fn queue_splice_drops_the_tail_at_the_upcoming_cap() {
    let full: Vec<String> = (0..UPCOMING_CAP).map(|i| format!("spotify:track:{i}")).collect();
    let refs: Vec<&str> = full.iter().map(String::as_str).collect();
    let r = rig(Some(queue_cluster(&refs, "rev-1")), None).await;
    r.client
      .queue_uri("spotify:track:new", QueuePosition::Next)
      .await
      .unwrap();

    let sent = sent_next_uris(&commands(&r.hits, "set_queue")[0]);
    assert_eq!(sent.len(), UPCOMING_CAP, "a full window never grows");
    assert_eq!(sent[0], "spotify:track:new");
    assert_eq!(sent.last().unwrap(), "spotify:track:78", "the last row falls off");
  }

  #[tokio::test]
  async fn queue_index_past_the_end_lands_at_the_tail() {
    let r = rig(Some(queue_cluster(&["spotify:track:a"], "rev-1")), None).await;
    r.client
      .queue_uri("spotify:track:new", QueuePosition::Index { at: 9 })
      .await
      .unwrap();

    let sent = commands(&r.hits, "set_queue");
    assert_eq!(
      sent_next_uris(&sent[0]),
      ["spotify:track:a", "spotify:track:new"],
      "a tail insert is the one splice allowed to grow the list"
    );
  }

  #[tokio::test]
  async fn queue_falls_back_to_append_when_nothing_is_upcoming() {
    for position in [QueuePosition::Next, QueuePosition::Index { at: 0 }] {
      let r = rig(Some(queue_cluster(&[], "rev-1")), None).await;
      r.client.queue_uri("spotify:track:new", position).await.unwrap();
      assert_eq!(
        commands(&r.hits, "add_to_queue").len(),
        1,
        "{position:?} has nothing to splice into"
      );
      assert!(commands(&r.hits, "set_queue").is_empty(), "{position:?}");
    }
  }

  #[tokio::test]
  async fn queue_positional_insert_is_refused_when_the_device_disallows_it() {
    for reason in [
      "disallow_set_queue_reasons",
      "disallow_inserting_into_next_tracks_reasons",
    ] {
      let mut c = queue_cluster(&["spotify:track:a"], "rev-1");
      let r = c
        .player_state
        .mut_or_insert_default()
        .restrictions
        .mut_or_insert_default();
      match reason {
        "disallow_set_queue_reasons" => r.disallow_set_queue_reasons.push("not_now".to_string()),
        _ => r
          .disallow_inserting_into_next_tracks_reasons
          .push("not_now".to_string()),
      }
      let rig = rig(Some(c), None).await;
      let err = rig
        .client
        .queue_uri("spotify:track:new", QueuePosition::Next)
        .await
        .unwrap_err();
      assert!(err.to_string().contains("not_now"), "{reason}: {err}");
      assert!(
        commands(&rig.hits, "set_queue").is_empty(),
        "{reason}: a refused insert never reaches the wire"
      );
      assert!(commands(&rig.hits, "add_to_queue").is_empty(), "{reason}");
    }
  }

  #[tokio::test]
  async fn queue_retries_once_against_a_freshly_read_revision() {
    let r = rig(Some(queue_cluster(&["spotify:track:a"], "rev-1")), None).await;
    r.refuse_set_queue(1, &queue_cluster(&["spotify:track:z"], "rev-2"));

    r.client
      .queue_uri("spotify:track:new", QueuePosition::Next)
      .await
      .unwrap();

    let sent = commands(&r.hits, "set_queue");
    assert_eq!(sent.len(), 2, "one refusal, one retry, and no more");
    assert_eq!(sent[0]["queue_revision"], "rev-1");
    assert_eq!(sent[1]["queue_revision"], "rev-2", "the retry re-reads the revision");
    assert_eq!(
      sent_next_uris(&sent[1]),
      ["spotify:track:new", "spotify:track:z"],
      "the retry splices into the list it just read, not the stale one"
    );
  }

  #[tokio::test]
  async fn queue_gives_up_after_one_retry() {
    let r = rig(Some(queue_cluster(&["spotify:track:a"], "rev-1")), None).await;
    r.refuse_set_queue(2, &queue_cluster(&["spotify:track:z"], "rev-2"));

    let err = r
      .client
      .queue_uri("spotify:track:new", QueuePosition::Next)
      .await
      .unwrap_err();
    assert!(err.to_string().contains("409"), "the caller sees the failure: {err}");
    assert_eq!(commands(&r.hits, "set_queue").len(), 2, "the retry is bounded at one");
  }
}
