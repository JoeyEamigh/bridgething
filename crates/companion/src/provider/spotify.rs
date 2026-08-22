use std::{
  collections::HashMap,
  sync::{Arc, Mutex, Weak},
};

use bridgething_gateway::OutboundLinkExt;
use bridgething_io::{HttpExecutor, HttpTransport as IoHttpTransport, WsTransport as IoWsTransport};
use libbridgething::{
  Album as WireAlbum, Artist as WireArtist, BrowseEntry, BrowseFolder, BrowseResult, FavoritesPage, ItemKind, ItemRef,
  LibraryItem, Lyrics, MediaItem, MediaItemUpdate, MusicProvider, NluPopularityFilter, NluResolvedIntent, NluSlots,
  NluTargetType, NowPlayingUpdate, Playback, PlaybackContext, PlaybackState, PlaybackTarget, PlaybackTargetKind,
  PlaybackUpdate, PlayerOptions, PlayerState, Playlist, PodcastEpisode, QueueItem, RecommendationsResult, RepeatMode,
  SearchResult, Show, ShuffleMode, Station, Track as WireTrack,
  gateway::{
    ContextResolveReply, FavoritesSet, GatewayToBridgeAudioMsgEvent, GatewayToBridgeLibraryMsgEvent,
    GatewayToBridgePlayerMsgCommand, LibraryBrowseRequest, LibraryChanged, LibraryFavoritesContainsRequest,
    LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest, PlayUri, QueueSnapshot, QueueUri,
    SpotifyWakeRequest, TrackIdentity, VolumeChanged,
  },
};
use spotify::{
  auth::{Auth, TokenStore as SpTokenStore},
  client::{DeviceWaker, Observer as SpObserver, Placement, SpotifyClient, WakeReason},
  model::{
    AuthState as SpAuthState, BrowseItem as SpBrowseItem, Device as SpDevice, DeviceKind as SpDeviceKind,
    LibraryScope as SpLibraryScope, PlayerState as SpPlayerState, Queue as SpQueue, QueuePosition as SpQueuePosition,
    RepeatMode as SpRepeat, Shelf as SpShelf, Track as SpTrack,
  },
  resolver::{VoicePopularity, VoiceResolveRequest, VoiceTargetKind},
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
  backend::{DeviceWaker as PlatformWaker, ImageScaler, SecretStore, WakeReason as PlatformWakeReason},
  provider::{
    AssetBytes, PlayerTransport, Provider, ProviderAuthState, ProviderError, ProviderLink, ProviderNowPlaying,
    ResumeTarget,
    art::{ArtCache, ImageAssetCodec},
    none_if_empty,
  },
  voice::dispatcher::{CatalogError, VoiceCatalogResolver},
};

pub const PROVIDER_NAME: &str = "spotify";
const SCDN_IMAGE_PREFIX: &str = "https://i.scdn.co/image/";
const BUILTIN_REF_PREFIX: &str = "builtin:";
const BUILTIN_ASSET_ID_PREFIX: &str = "builtin/img/";
const DEFAULT_HERO_EDGE: u32 = 248;
const DEFAULT_THUMB_EDGE: u32 = 96;
const QUEUE_MAX: usize = 50;
const QUEUE_RUNWAY_FLOOR: usize = 8;
const SPOTIFY_APP_BUNDLE: &str = "com.spotify.client";
const SPOTIFY_ANDROID_PACKAGE: &str = "com.spotify.music";
const SPOTIFY_MPRIS_PLAYER: &str = "spotify";
const SPOTIFY_WINDOWS_AUMIDS: [&str; 2] = ["Spotify.exe", "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"];
const VOLUME_STEP_PERCENT: f64 = 6.25;

const IMAGE_CODEC: ImageAssetCodec = ImageAssetCodec {
  namespace: "spotify/img/",
  short_form: Some(('i', SCDN_IMAGE_PREFIX)),
};

pub(crate) const KEY_REFRESH_TOKEN: &str = "spotify.refresh_token";
pub(crate) const KEY_USERNAME: &str = "spotify.username";
pub(crate) const KEY_DEVICE_ID: &str = "spotify.device_id";

struct SecretTokenStore(Arc<dyn SecretStore>);

impl SpTokenStore for SecretTokenStore {
  fn load_refresh_token(&self) -> Option<String> {
    self.0.get(KEY_REFRESH_TOKEN.into())
  }

  fn save_refresh_token(&self, token: String) {
    self.0.set(KEY_REFRESH_TOKEN.into(), token);
  }

  fn load_username(&self) -> Option<String> {
    self.0.get(KEY_USERNAME.into())
  }

  fn save_username(&self, username: String) {
    self.0.set(KEY_USERNAME.into(), username);
  }
}

#[derive(Clone)]
pub struct SpotifyConfig {
  pub worker_base: String,
  pub psk: String,
  pub device_id: String,
}

enum EmitJob {
  Player {
    snapshot: Box<PlayerState>,
    has_item: bool,
    on_remote: bool,
  },
  Queue {
    entries: Vec<QueueItem>,
    thumb_edge: u32,
  },
  Targets {
    targets: Vec<PlaybackTarget>,
  },
}

struct Session {
  client: Arc<SpotifyClient>,
  link: ProviderLink,
  connect_task: JoinHandle<()>,
  emit_task: JoinHandle<()>,
  emit_tx: mpsc::UnboundedSender<EmitJob>,
  resolver: Arc<SpotifyVoiceResolver>,
}

impl Drop for Session {
  fn drop(&mut self) {
    self.connect_task.abort();
    self.emit_task.abort();
  }
}

#[derive(Default)]
struct Shared {
  liked_override: HashMap<String, bool>,
  hero_edge: u32,
  thumb_edge: u32,
  last_state: Option<SpPlayerState>,
  last_state_at: Option<tokio::time::Instant>,
  last_queue_items: Vec<QueueItem>,
  last_devices: Vec<SpDevice>,
  last_sent_queue_order: Vec<String>,
  last_sent_thumb_edge: u32,
  on_remote_speaker: bool,
  last_had_item: bool,
  last_emitted_remote_volume: Option<f32>,
  connectivity_available: Option<bool>,
}

type NowPlayingObserver = Arc<dyn Fn(Option<ProviderNowPlaying>) + Send + Sync>;
type AuthObserver = Arc<dyn Fn(ProviderAuthState) + Send + Sync>;

struct Core {
  config: SpotifyConfig,
  ws: Arc<dyn IoWsTransport>,
  auth: Arc<Auth>,
  waker: Option<Arc<dyn PlatformWaker>>,
  exec: HttpExecutor,
  session: Mutex<Option<Session>>,
  shared: Mutex<Shared>,
  art_cache: ArtCache,
  np_observer: Mutex<Option<NowPlayingObserver>>,
  auth_observer: Mutex<Option<AuthObserver>>,
  resume_target: Mutex<ResumeTarget>,
}

fn placement_of(target: ResumeTarget) -> Placement {
  match target {
    ResumeTarget::PhoneOnly => Placement::Car,
    ResumeTarget::AnySpeaker => Placement::Desk,
  }
}

pub struct SpotifyProvider {
  core: Arc<Core>,
}

impl SpotifyProvider {
  pub fn new(
    config: SpotifyConfig,
    http: Arc<dyn IoHttpTransport>,
    ws: Arc<dyn IoWsTransport>,
    secrets: Arc<dyn SecretStore>,
    scaler: Option<Arc<dyn ImageScaler>>,
    waker: Option<Arc<dyn PlatformWaker>>,
  ) -> Arc<Self> {
    let exec = HttpExecutor::new(http);
    let auth = Arc::new(Auth::new(
      config.worker_base.clone(),
      config.psk.clone(),
      Box::new(SecretTokenStore(secrets)),
      exec.clone(),
    ));
    Arc::new(Self {
      core: Arc::new(Core {
        config,
        ws,
        auth,
        waker,
        art_cache: ArtCache::new(exec.clone(), scaler),
        exec,
        session: Mutex::new(None),
        shared: Mutex::new(Shared {
          hero_edge: DEFAULT_HERO_EDGE,
          thumb_edge: DEFAULT_THUMB_EDGE,
          last_sent_thumb_edge: DEFAULT_THUMB_EDGE,
          ..Shared::default()
        }),
        np_observer: Mutex::new(None),
        auth_observer: Mutex::new(None),
        resume_target: Mutex::new(ResumeTarget::default()),
      }),
    })
  }

  pub async fn connectivity_changed(&self, available: bool) {
    let restored = {
      let mut shared = self.core.shared.lock().unwrap();
      let restored = shared.connectivity_available == Some(false) && available;
      shared.connectivity_available = Some(available);
      restored
    };
    if restored && let Some(client) = self.core.client() {
      client.resync().await;
    }
  }
}

impl Core {
  fn client(&self) -> Option<Arc<SpotifyClient>> {
    self
      .session
      .lock()
      .unwrap()
      .as_ref()
      .map(|session| session.client.clone())
  }

  fn require(&self) -> Result<Arc<SpotifyClient>, ProviderError> {
    self.client().ok_or(ProviderError::Detached)
  }

  fn link(&self) -> Option<ProviderLink> {
    self
      .session
      .lock()
      .unwrap()
      .as_ref()
      .map(|session| session.link.clone())
  }

  fn notify_auth(&self, state: ProviderAuthState) {
    if let Some(observer) = self.auth_observer.lock().unwrap().clone() {
      observer(state);
    }
  }

  fn notify_now_playing(&self, playing: Option<ProviderNowPlaying>) {
    if let Some(observer) = self.np_observer.lock().unwrap().clone() {
      observer(playing);
    }
  }

  fn art_edges(&self) -> (u32, u32) {
    let shared = self.shared.lock().unwrap();
    (shared.hero_edge, shared.thumb_edge)
  }

  fn enqueue(&self, job: EmitJob) {
    if let Some(session) = self.session.lock().unwrap().as_ref() {
      let _ = session.emit_tx.send(job);
    }
  }

  fn monotonic_age_ms(&self) -> Option<u32> {
    let shared = self.shared.lock().unwrap();
    shared
      .last_state_at
      .map(|at| u32::try_from(at.elapsed().as_millis()).unwrap_or(u32::MAX))
  }

  fn on_player(&self, state: SpPlayerState) {
    if self.link().is_none() {
      return;
    }
    let (hero, _) = self.art_edges();
    let (liked, like_supported) = self.like_fields(state.track.as_ref());
    self.notify_now_playing(Some(ProviderNowPlaying {
      update: make_update(&state, hero, liked, like_supported),
      artwork_url: state.track.as_ref().and_then(|t| raw_artwork_url(&best_hex(t))),
    }));
    {
      let mut shared = self.shared.lock().unwrap();
      shared.last_state = Some(state.clone());
      shared.last_state_at = Some(tokio::time::Instant::now());
    }
    let snapshot = self.make_snapshot(&state, hero, liked, like_supported, None);
    let has_item = state.track.is_some();
    let on_remote = state.on_remote_speaker;
    self.enqueue(EmitJob::Player {
      snapshot: Box::new(snapshot),
      has_item,
      on_remote,
    });
  }

  fn on_queue(&self, queue: SpQueue) {
    let (_, thumb) = self.art_edges();
    let entries: Vec<QueueItem> = queue
      .next
      .iter()
      .take(QUEUE_MAX)
      .map(|t| queue_item(t, thumb))
      .collect();
    self.shared.lock().unwrap().last_queue_items = entries.clone();
    self.enqueue(EmitJob::Queue {
      entries,
      thumb_edge: thumb,
    });
  }

  fn on_devices(&self, devices: Vec<SpDevice>) {
    if self.link().is_none() {
      return;
    }
    self.shared.lock().unwrap().last_devices = devices.clone();
    self.enqueue(EmitJob::Targets {
      targets: devices.iter().map(playback_target).collect(),
    });
  }

  fn on_library_changed(self: &Arc<Self>, scope: SpLibraryScope) {
    let Some(link) = self.link() else { return };
    let wire = match scope {
      SpLibraryScope::Saved => libbridgething::gateway::LibraryScope::Saved,
      SpLibraryScope::Playlists => libbridgething::gateway::LibraryScope::Playlists,
    };
    tokio::spawn(async move {
      let _ = link
        .outbound
        .event(GatewayToBridgeLibraryMsgEvent::LibraryChanged(LibraryChanged {
          scope: wire,
        }))
        .await;
    });
  }

  fn on_auth(self: &Arc<Self>, state: SpAuthState) {
    match state {
      SpAuthState::LoggedIn { .. } => {
        self.notify_auth(ProviderAuthState::Authenticated);
        let core = self.clone();
        tokio::spawn(async move { core.check_premium().await });
      }
      SpAuthState::LoggedOut => {
        self.handle_auth_down();
        self.notify_auth(ProviderAuthState::Pending {
          user_code: None,
          verification_url: None,
          verification_url_complete: None,
        });
      }
      SpAuthState::Pending { url, code } => self.notify_auth(ProviderAuthState::Pending {
        user_code: Some(code),
        verification_url: Some(url.clone()),
        verification_url_complete: Some(url),
      }),
      SpAuthState::Failed { reason } => {
        self.handle_auth_down();
        self.notify_auth(ProviderAuthState::Failed { reason });
      }
    }
  }

  fn handle_auth_down(&self) {
    self.notify_now_playing(None);
    {
      let mut shared = self.shared.lock().unwrap();
      shared.on_remote_speaker = false;
      shared.last_had_item = false;
      shared.last_emitted_remote_volume = None;
    }
    if let Some(link) = self.link() {
      link.sink.clear_source(PROVIDER_NAME);
    }
  }

  async fn check_premium(&self) {
    let Some(client) = self.client() else { return };
    let Ok(product) = client.product().await else { return };
    if !product.can_use_superbird {
      self.notify_auth(ProviderAuthState::Failed {
        reason: "Spotify Premium is required".into(),
      });
    }
  }

  async fn handle_emit(&self, job: EmitJob) {
    let Some(link) = self.link() else { return };
    match job {
      EmitJob::Player {
        snapshot,
        has_item,
        on_remote,
      } => {
        let gained_remote = {
          let mut shared = self.shared.lock().unwrap();
          let gained = has_item && on_remote && !shared.on_remote_speaker;
          shared.on_remote_speaker = has_item && on_remote;
          shared.last_had_item = has_item;
          if !shared.on_remote_speaker {
            shared.last_emitted_remote_volume = None;
          }
          gained
        };
        if has_item && on_remote {
          self.emit_remote_volume_from_cluster(gained_remote).await;
        }
        link
          .sink
          .submit_player(PROVIDER_NAME, *snapshot, SPOTIFY_APP_BUNDLE, has_item, on_remote);
      }
      EmitJob::Queue { entries, thumb_edge } => {
        self.send_queue_changed_if_needed(&link, entries, thumb_edge);
      }
      EmitJob::Targets { targets } => {
        link
          .sink
          .submit_targets(PROVIDER_NAME, libbridgething::gateway::PlaybackTargets { targets });
      }
    }
  }

  fn send_queue_changed_if_needed(&self, link: &ProviderLink, entries: Vec<QueueItem>, thumb: u32) {
    let order: Vec<String> = entries.iter().map(|item| item.uri.clone()).collect();
    {
      let mut shared = self.shared.lock().unwrap();
      let edge_changed = thumb != shared.last_sent_thumb_edge;
      shared.last_sent_thumb_edge = thumb;
      if !edge_changed
        && let Some(runway) = forward_slide_runway(&shared.last_sent_queue_order, &order)
        && runway >= QUEUE_RUNWAY_FLOOR
      {
        return;
      }
      shared.last_sent_queue_order = order.clone();
    }
    link
      .sink
      .submit_queue(PROVIDER_NAME, QueueSnapshot { order, items: entries });
  }

  fn reset_queue_dedup(&self) {
    self.shared.lock().unwrap().last_sent_queue_order = Vec::new();
  }

  async fn emit_remote_volume_from_cluster(&self, force: bool) {
    let Some(client) = self.client() else { return };
    let Some(pct) = client.active_device_volume_percent().await else {
      return;
    };
    self.emit_remote_volume((pct / 100.0) as f32, force).await;
  }

  async fn emit_remote_volume(&self, level: f32, force: bool) {
    let Some(link) = self.link() else { return };
    let changed = {
      let mut shared = self.shared.lock().unwrap();
      if !shared.on_remote_speaker {
        false
      } else if !force
        && let Some(last) = shared.last_emitted_remote_volume
        && (last - level).abs() < 0.005
      {
        false
      } else {
        shared.last_emitted_remote_volume = Some(level);
        true
      }
    };
    if changed {
      let _ = link
        .outbound
        .event(GatewayToBridgeAudioMsgEvent::VolumeChanged(VolumeChanged {
          level,
          muted: false,
        }))
        .await;
    }
  }

  fn note_remote_volume(&self, level: f32) {
    let mut shared = self.shared.lock().unwrap();
    if shared.on_remote_speaker {
      shared.last_emitted_remote_volume = Some(level);
    }
  }

  fn like_fields(&self, track: Option<&SpTrack>) -> (Option<bool>, Option<bool>) {
    let Some(track) = track else { return (None, None) };
    if !track.uri.starts_with("spotify:") {
      return (None, None);
    }
    let mut shared = self.shared.lock().unwrap();
    let liked = match shared.liked_override.get(&track.uri).copied() {
      Some(withheld) => {
        if withheld == track.saved {
          shared.liked_override.remove(&track.uri);
        }
        withheld
      }
      None => track.saved,
    };
    (Some(liked), Some(true))
  }

  async fn apply_liked_change(&self, uri: &str, liked: bool) {
    self.shared.lock().unwrap().liked_override.insert(uri.to_owned(), liked);
    self.reemit_snapshot_if_current(uri).await;
  }

  async fn reemit_snapshot_if_current(&self, uri: &str) {
    if self.link().is_none() {
      return;
    }
    let pending = self.shared.lock().unwrap().last_state.clone();
    let Some(pending) = pending else { return };
    if pending.track.as_ref().map(|t| t.uri.as_str()) != Some(uri) {
      return;
    }
    let age = self.monotonic_age_ms();
    let (hero, _) = self.art_edges();
    let (liked, supported) = self.like_fields(pending.track.as_ref());
    let snapshot = self.make_snapshot(&pending, hero, liked, supported, age);
    let has_item = pending.track.is_some();
    let on_remote = pending.on_remote_speaker;
    self.enqueue(EmitJob::Player {
      snapshot: Box::new(snapshot),
      has_item,
      on_remote,
    });
  }

  fn make_snapshot(
    &self,
    state: &SpPlayerState,
    hero_edge: u32,
    liked: Option<bool>,
    like_supported: Option<bool>,
    position_age_ms: Option<u32>,
  ) -> PlayerState {
    let track = state.track.as_ref().map(|t| MediaItem {
      uri: Some(t.uri.clone()),
      persistent_id: Some(t.uri.clone()),
      title: none_if_empty(&t.name),
      album: none_if_empty(&t.album.name),
      album_uri: none_if_empty(&t.album.uri),
      album_artist: None,
      artist: artist_names(t),
      artist_uri: t.artists.first().map(|artist| artist.uri.clone()),
      liked,
      artwork_id: art_asset_id(&best_hex(t), hero_edge),
      duration_ms: Some(t.duration_ms),
      media_types: None,
      track_number: None,
      track_count: None,
      is_like_supported: like_supported,
      is_ban_supported: None,
      is_banned: None,
      chapter_count: None,
    });
    let playback = Playback {
      state: if state.is_paused {
        PlaybackState::Paused
      } else {
        PlaybackState::Playing
      },
      position_ms: state.position_ms,
      position_age_ms,
      shuffle: state.shuffle,
      shuffle_mode: Some(if state.shuffle {
        ShuffleMode::Songs
      } else {
        ShuffleMode::Off
      }),
      repeat: map_repeat(state.repeat),
      queue_index: None,
      queue_count: None,
      queue_chapter_index: None,
      set_elapsed_time_available: Some(state.can_seek),
      queue_list_avail: None,
      apple_music_radio_ad: None,
    };
    let context = (!state.context_uri.is_empty()).then(|| PlaybackContext {
      uri: state.context_uri.clone(),
      name: none_if_empty(&state.context_name),
    });
    PlayerState {
      track,
      playback,
      queue: Vec::new(),
      options: PlayerOptions {
        speed: 1.0,
        crossfade_ms: None,
      },
      context,
      target: self.active_target(state),
    }
  }

  fn active_target(&self, state: &SpPlayerState) -> Option<PlaybackTarget> {
    if !state.playing_remotely || state.remote_device_id.is_empty() {
      return None;
    }
    let shared = self.shared.lock().unwrap();
    shared
      .last_devices
      .iter()
      .find(|device| device.id == state.remote_device_id)
      .map(playback_target)
  }

  fn warm_art(self: &Arc<Self>, result: &BrowseResult) {
    let mut ids = collect_art_ids(&result.entries);
    ids.sort();
    ids.dedup();
    for id in ids {
      let Some((url, _)) = IMAGE_CODEC.parse(&id) else {
        continue;
      };
      let core = self.clone();
      tokio::spawn(async move {
        let _ = core.art_cache.master(&url).await;
      });
    }
  }
}

struct ObserverBridge(Weak<Core>);

impl SpObserver for ObserverBridge {
  fn on_player(&self, state: SpPlayerState) {
    if let Some(core) = self.0.upgrade() {
      core.on_player(state);
    }
  }

  fn on_queue(&self, queue: SpQueue) {
    if let Some(core) = self.0.upgrade() {
      core.on_queue(queue);
    }
  }

  fn on_devices(&self, devices: Vec<SpDevice>) {
    if let Some(core) = self.0.upgrade() {
      core.on_devices(devices);
    }
  }

  fn on_auth(&self, state: SpAuthState) {
    if let Some(core) = self.0.upgrade() {
      core.on_auth(state);
    }
  }

  fn on_library_changed(&self, scope: SpLibraryScope) {
    if let Some(core) = self.0.upgrade() {
      core.on_library_changed(scope);
    }
  }
}

struct GatewayWaker(Weak<Core>);

impl DeviceWaker for GatewayWaker {
  fn wake_device(&self, _reason: WakeReason, allow_play_tap: bool) {
    let Some(core) = self.0.upgrade() else { return };
    let Some(link) = core.link() else { return };
    tokio::spawn(async move {
      let _ = link
        .outbound
        .command(GatewayToBridgePlayerMsgCommand::RequestSpotifyWake(
          SpotifyWakeRequest { allow_play_tap },
        ))
        .await;
    });
  }
}

struct LocalWaker(Arc<dyn PlatformWaker>);

impl DeviceWaker for LocalWaker {
  fn wake_device(&self, reason: WakeReason, allow_play_tap: bool) {
    let waker = self.0.clone();
    let reason = match reason {
      WakeReason::UserPlay => PlatformWakeReason::UserPlay,
      WakeReason::ConnectResume => PlatformWakeReason::ConnectResume,
    };
    tokio::task::spawn_blocking(move || waker.wake_device(reason, allow_play_tap));
  }
}

pub struct SpotifyVoiceResolver {
  client: Arc<SpotifyClient>,
}

pub fn catalog_intent(intent: &str) -> bool {
  matches!(
    intent,
    "PLAY" | "ADD_TO_QUEUE" | "ADD_TO_PLAYLIST" | "SEARCH" | "THUMBS_UP"
  )
}

fn clean(value: &Option<String>) -> Option<String> {
  value
    .as_ref()
    .map(|text| text.trim().to_owned())
    .filter(|text| !text.is_empty())
}

fn voice_kind(kind: NluTargetType) -> VoiceTargetKind {
  match kind {
    NluTargetType::Artist => VoiceTargetKind::Artist,
    NluTargetType::Track => VoiceTargetKind::Track,
    NluTargetType::Album => VoiceTargetKind::Album,
    NluTargetType::Playlist => VoiceTargetKind::Playlist,
    NluTargetType::Podcast => VoiceTargetKind::Show,
    NluTargetType::Episode => VoiceTargetKind::Episode,
    NluTargetType::Station => VoiceTargetKind::Station,
  }
}

pub fn voice_request(slots: &NluSlots) -> VoiceResolveRequest {
  VoiceResolveRequest {
    target: clean(&slots.target),
    target_type: slots.target_type.map(voice_kind),
    mood: clean(&slots.mood),
    genre: clean(&slots.genre),
    era: clean(&slots.era),
    popularity_filter: slots.popularity_filter.map(voice_popularity),
    position: slots.position,
  }
}

pub fn names_nothing(req: &VoiceResolveRequest) -> bool {
  req.target.is_none()
    && req.target_type.is_none()
    && req.mood.is_none()
    && req.genre.is_none()
    && req.era.is_none()
    && req.position.is_none()
    && req.popularity_filter.is_none()
}

fn voice_popularity(filter: NluPopularityFilter) -> VoicePopularity {
  match filter {
    NluPopularityFilter::Top5 => VoicePopularity::Top5,
    NluPopularityFilter::Top10 => VoicePopularity::Top10,
    NluPopularityFilter::Popular => VoicePopularity::Popular,
    NluPopularityFilter::Recent => VoicePopularity::Recent,
    NluPopularityFilter::New => VoicePopularity::New,
    NluPopularityFilter::First => VoicePopularity::First,
    NluPopularityFilter::Random => VoicePopularity::Random,
  }
}

#[async_trait::async_trait]
impl VoiceCatalogResolver for SpotifyVoiceResolver {
  async fn decorate(&self, resolved: NluResolvedIntent) -> Result<NluResolvedIntent, CatalogError> {
    if !catalog_intent(&resolved.intent) {
      return Ok(resolved);
    }
    let request = voice_request(&resolved.slots);
    if names_nothing(&request) {
      return Ok(resolved);
    }
    let answer = self
      .client
      .resolve_voice(request)
      .await
      .map_err(|error| CatalogError::Failed(error.to_string()))?;
    let mut resolved = resolved;
    resolved.slots.uri = Some(answer.uri);
    resolved.slots.context_uri = answer.context_uri;
    Ok(resolved)
  }
}

fn artist_names(track: &SpTrack) -> Option<String> {
  let joined = track
    .artists
    .iter()
    .map(|artist| artist.name.as_str())
    .collect::<Vec<_>>()
    .join(", ");
  none_if_empty(&joined)
}

fn best_hex(track: &SpTrack) -> String {
  if track.image_id.is_empty() {
    track.album.image_id.clone()
  } else {
    track.image_id.clone()
  }
}

fn art_asset_id(reference: &str, edge: u32) -> Option<String> {
  if reference.is_empty() {
    return None;
  }
  if let Some(rest) = reference.strip_prefix(BUILTIN_REF_PREFIX) {
    return Some(format!("{BUILTIN_ASSET_ID_PREFIX}{rest}"));
  }
  let url = if reference.starts_with("http") {
    reference.to_owned()
  } else {
    format!("{SCDN_IMAGE_PREFIX}{reference}")
  };
  IMAGE_CODEC.asset_id(&url, edge)
}

fn raw_artwork_url(reference: &str) -> Option<String> {
  if reference.is_empty() {
    None
  } else if reference.starts_with("http") {
    Some(reference.to_owned())
  } else {
    Some(format!("{SCDN_IMAGE_PREFIX}{reference}"))
  }
}

fn kind_of(uri: &str) -> &str {
  uri.split(':').nth(1).unwrap_or("")
}

fn map_repeat(mode: SpRepeat) -> RepeatMode {
  match mode {
    SpRepeat::Off => RepeatMode::Off,
    SpRepeat::Context => RepeatMode::All,
    SpRepeat::Track => RepeatMode::One,
  }
}

fn playback_target(device: &SpDevice) -> PlaybackTarget {
  PlaybackTarget {
    id: device.id.clone(),
    name: device.name.clone(),
    kind: target_kind(device.kind),
    is_active: device.is_active,
    volume_percent: (device.volume > 0.0).then(|| (device.volume * 100.0).round() as u32),
  }
}

fn target_kind(kind: SpDeviceKind) -> PlaybackTargetKind {
  match kind {
    SpDeviceKind::Phone => PlaybackTargetKind::Phone,
    SpDeviceKind::Tablet => PlaybackTargetKind::Tablet,
    SpDeviceKind::Computer => PlaybackTargetKind::Computer,
    SpDeviceKind::Speaker => PlaybackTargetKind::Speaker,
    SpDeviceKind::Tv => PlaybackTargetKind::Tv,
    SpDeviceKind::GameConsole => PlaybackTargetKind::GameConsole,
    SpDeviceKind::Automobile => PlaybackTargetKind::Automobile,
    SpDeviceKind::Wearable => PlaybackTargetKind::Wearable,
    SpDeviceKind::Unknown => PlaybackTargetKind::Unknown,
  }
}

fn map_track(item: &SpBrowseItem, edge: u32) -> WireTrack {
  WireTrack {
    id: item.uri.clone(),
    name: item.title.clone(),
    album: WireAlbum {
      id: item.album.uri.clone(),
      name: item.album.name.clone(),
      artwork_id: None,
    },
    artist: WireArtist {
      id: item
        .artists
        .first()
        .map(|artist| artist.uri.clone())
        .unwrap_or_default(),
      name: item
        .artists
        .first()
        .map(|artist| artist.name.clone())
        .unwrap_or_default(),
      artwork_id: None,
    },
    artists: item
      .artists
      .iter()
      .map(|artist| WireArtist {
        id: artist.uri.clone(),
        name: artist.name.clone(),
        artwork_id: None,
      })
      .collect(),
    duration_ms: item.duration_ms,
    image_id: art_asset_id(&item.image_id, edge).unwrap_or_default(),
    saved: item.saved,
  }
}

fn library_item(item: &SpBrowseItem, edge: u32) -> Option<LibraryItem> {
  let art = art_asset_id(&item.image_id, edge);
  match kind_of(&item.uri) {
    "track" => Some(LibraryItem::Track(map_track(item, edge))),
    "album" => Some(LibraryItem::Album(WireAlbum {
      id: item.uri.clone(),
      name: item.title.clone(),
      artwork_id: art,
    })),
    "artist" => Some(LibraryItem::Artist(WireArtist {
      id: item.uri.clone(),
      name: item.title.clone(),
      artwork_id: art,
    })),
    "playlist" => Some(LibraryItem::Playlist(Playlist {
      uri: item.uri.clone(),
      name: item.title.clone(),
      owner_name: None,
      track_count: None,
      artwork_id: art,
    })),
    "user" if item.uri.ends_with(":collection") => Some(LibraryItem::Playlist(Playlist {
      uri: item.uri.clone(),
      name: item.title.clone(),
      owner_name: None,
      track_count: None,
      artwork_id: art,
    })),
    "show" => Some(LibraryItem::Show(Show {
      uri: item.uri.clone(),
      name: item.title.clone(),
      publisher: none_if_empty(&item.subtitle),
      episode_count: None,
      artwork_id: art,
    })),
    "episode" => Some(LibraryItem::PodcastEpisode(PodcastEpisode {
      uri: item.uri.clone(),
      name: item.title.clone(),
      show_name: none_if_empty(&item.subtitle),
      duration_ms: Some(item.duration_ms),
      published_at_unix_s: None,
      artwork_id: art,
    })),
    "station" => Some(LibraryItem::Station(Station {
      uri: item.uri.clone(),
      name: item.title.clone(),
      seed: None,
      artwork_id: art,
    })),
    _ => None,
  }
}

fn folder(shelf: &SpShelf, edge: u32) -> BrowseFolder {
  let children: Vec<BrowseEntry> = shelf
    .items
    .iter()
    .filter_map(|item| library_item(item, edge).map(BrowseEntry::Item))
    .collect();
  BrowseFolder {
    node_id: shelf.id.clone(),
    title: shelf.title.clone(),
    subtitle: None,
    artwork_id: None,
    total: Some(shelf.total),
    preview_children: (!children.is_empty()).then_some(children),
  }
}

fn queue_item(track: &SpTrack, edge: u32) -> QueueItem {
  QueueItem {
    uri: track.uri.clone(),
    title: none_if_empty(&track.name),
    artist: artist_names(track),
    artist_uri: track.artists.first().map(|artist| artist.uri.clone()),
    album: none_if_empty(&track.album.name),
    album_uri: none_if_empty(&track.album.uri),
    artwork_id: art_asset_id(&best_hex(track), edge),
    duration_ms: Some(track.duration_ms),
    persistent_id: None,
    queued: track.queued,
  }
}

fn make_update(
  state: &SpPlayerState,
  hero_edge: u32,
  liked: Option<bool>,
  like_supported: Option<bool>,
) -> NowPlayingUpdate {
  let media = state.track.as_ref().map(|t| MediaItemUpdate {
    persistent_id: Some(t.uri.clone()),
    title: none_if_empty(&t.name),
    album: none_if_empty(&t.album.name),
    album_uri: none_if_empty(&t.album.uri),
    album_artist: None,
    artist: artist_names(t),
    artist_uri: t.artists.first().map(|artist| artist.uri.clone()),
    liked,
    artwork_id: art_asset_id(&best_hex(t), hero_edge),
    duration_ms: Some(t.duration_ms),
    media_types: None,
    track_number: None,
    track_count: None,
    is_like_supported: like_supported,
    is_ban_supported: None,
    is_banned: None,
    is_resident_on_device: None,
    chapter_count: None,
  });
  let playback = PlaybackUpdate {
    playing: Some(!state.is_paused),
    position_ms: Some(state.position_ms),
    shuffle: Some(state.shuffle),
    shuffle_mode: Some(if state.shuffle {
      ShuffleMode::Songs
    } else {
      ShuffleMode::Off
    }),
    repeat: Some(map_repeat(state.repeat)),
    app_bundle: Some(SPOTIFY_APP_BUNDLE.into()),
    app_display_name: Some("Spotify".into()),
    queue_index: None,
    queue_count: None,
    queue_chapter_index: None,
    playback_speed: None,
    set_elapsed_time_available: Some(state.can_seek),
    queue_list_avail: None,
    apple_music_radio_ad: None,
    apple_music_radio_station_name: None,
  };
  NowPlayingUpdate {
    media_item: media,
    playback: Some(playback),
  }
}

fn forward_slide_runway(last: &[String], new: &[String]) -> Option<usize> {
  if last.is_empty() {
    return None;
  }
  for k in 1..last.len() {
    let suffix = &last[k..];
    if new.len() >= suffix.len() && &new[..suffix.len()] == suffix {
      return Some(suffix.len());
    }
  }
  None
}

fn collect_art_ids(entries: &[BrowseEntry]) -> Vec<String> {
  let mut out = Vec::new();
  for entry in entries {
    match entry {
      BrowseEntry::Folder(folder) => {
        if let Some(id) = &folder.artwork_id {
          out.push(id.clone());
        }
        if let Some(children) = &folder.preview_children {
          out.extend(collect_art_ids(children));
        }
      }
      BrowseEntry::Item(item) => {
        if let Some(id) = library_item_artwork_id(item) {
          out.push(id);
        }
      }
    }
  }
  out
}

fn library_item_artwork_id(item: &LibraryItem) -> Option<String> {
  match item {
    LibraryItem::Track(track) => none_if_empty(&track.image_id),
    LibraryItem::Playlist(playlist) => playlist.artwork_id.clone(),
    LibraryItem::PodcastEpisode(episode) => episode.artwork_id.clone(),
    LibraryItem::Show(show) => show.artwork_id.clone(),
    LibraryItem::Station(station) => station.artwork_id.clone(),
    LibraryItem::Album(album) => album.artwork_id.clone(),
    LibraryItem::Artist(artist) => artist.artwork_id.clone(),
  }
}

fn failed(error: spotify::Error) -> ProviderError {
  ProviderError::Failed(error.to_string())
}

fn is_root(node_id: Option<&str>) -> bool {
  matches!(node_id, None | Some("") | Some("root"))
}

#[async_trait::async_trait]
impl PlayerTransport for SpotifyProvider {
  async fn play(&self, uri: PlayUri) -> Result<(), ProviderError> {
    let client = self.core.require()?;
    match uri.context {
      Some(context) => client.play(&context.context_uri, Some(uri.uri)).await.map_err(failed),
      None => client.play(&uri.uri, None).await.map_err(failed),
    }
  }

  async fn queue(&self, req: QueueUri) -> Result<(), ProviderError> {
    let client = self.core.require()?;
    let position = match req.position {
      libbridgething::QueuePosition::Append => SpQueuePosition::Append,
      libbridgething::QueuePosition::Next => SpQueuePosition::Next,
      libbridgething::QueuePosition::Index(at) => SpQueuePosition::Index { at },
    };
    client.queue_uri(&req.uri, position).await.map_err(failed)
  }

  async fn pause(&self) -> Result<(), ProviderError> {
    self.core.require()?.pause().await.map_err(failed)
  }

  async fn resume(&self) -> Result<(), ProviderError> {
    self.core.require()?.resume().await.map_err(failed)
  }

  async fn skip_next(&self) -> Result<(), ProviderError> {
    self.core.require()?.skip_next().await.map_err(failed)
  }

  async fn skip_prev(&self) -> Result<(), ProviderError> {
    self.core.require()?.skip_prev().await.map_err(failed)
  }

  async fn skip_to_index(&self, index: u32) -> Result<(), ProviderError> {
    let client = self.core.require()?;
    let (target, context) = {
      let shared = self.core.shared.lock().unwrap();
      let target = shared.last_queue_items.get(index as usize).map(|item| item.uri.clone());
      let context = shared
        .last_state
        .as_ref()
        .map(|state| state.context_uri.clone())
        .filter(|uri| !uri.is_empty());
      (target, context)
    };
    let (Some(target), Some(context)) = (target, context) else {
      return Err(ProviderError::Failed(format!("queue index {index} out of range")));
    };
    client.play(&context, Some(target)).await.map_err(failed)
  }

  async fn seek_to(&self, position_ms: u32) -> Result<(), ProviderError> {
    self.core.require()?.seek(position_ms as i64).await.map_err(failed)
  }

  async fn set_shuffle(&self, on: bool) -> Result<(), ProviderError> {
    self.core.require()?.set_shuffle(on).await.map_err(failed)
  }

  async fn set_repeat(&self, mode: RepeatMode) -> Result<(), ProviderError> {
    let mapped = match mode {
      RepeatMode::Off => SpRepeat::Off,
      RepeatMode::All => SpRepeat::Context,
      RepeatMode::One => SpRepeat::Track,
    };
    self.core.require()?.set_repeat(mapped).await.map_err(failed)
  }
}

#[async_trait::async_trait]
impl Provider for SpotifyProvider {
  fn name(&self) -> &str {
    PROVIDER_NAME
  }

  fn display_name(&self) -> &str {
    "Spotify"
  }

  fn uri_schemes(&self) -> Vec<String> {
    vec!["spotify".into()]
  }

  fn music_provider(&self) -> MusicProvider {
    MusicProvider::Spotify
  }

  fn supports_playback_targets(&self) -> bool {
    true
  }

  fn voice_resolver(&self) -> Option<Arc<dyn VoiceCatalogResolver>> {
    self
      .core
      .session
      .lock()
      .unwrap()
      .as_ref()
      .map(|session| session.resolver.clone() as Arc<dyn VoiceCatalogResolver>)
  }

  fn app_bundles(&self) -> Vec<String> {
    [SPOTIFY_APP_BUNDLE, SPOTIFY_ANDROID_PACKAGE, SPOTIFY_MPRIS_PLAYER]
      .into_iter()
      .chain(SPOTIFY_WINDOWS_AUMIDS)
      .map(str::to_owned)
      .collect()
  }

  fn set_now_playing_observer(&self, observer: Option<Arc<dyn Fn(Option<ProviderNowPlaying>) + Send + Sync>>) {
    *self.core.np_observer.lock().unwrap() = observer;
  }

  fn set_auth_observer(&self, observer: Option<Arc<dyn Fn(ProviderAuthState) + Send + Sync>>) {
    *self.core.auth_observer.lock().unwrap() = observer;
  }

  async fn attach(&self, link: ProviderLink) -> Result<(), ProviderError> {
    if self.core.session.lock().unwrap().is_some() {
      let np = self.core.np_observer.lock().unwrap().clone();
      let auth = self.core.auth_observer.lock().unwrap().clone();
      self.detach().await;
      *self.core.np_observer.lock().unwrap() = np;
      *self.core.auth_observer.lock().unwrap() = auth;
    }
    self.core.reset_queue_dedup();

    let client = Arc::new(SpotifyClient::new(
      self.core.auth.clone(),
      self.core.config.device_id.clone(),
      self.core.exec.clone(),
      Arc::new(ObserverBridge(Arc::downgrade(&self.core))),
    ));
    client.set_ws_transport(self.core.ws.clone());
    client.set_placement(placement_of(*self.core.resume_target.lock().unwrap()));
    client.set_device_waker(match &self.core.waker {
      Some(waker) => Arc::new(LocalWaker(waker.clone())),
      None => Arc::new(GatewayWaker(Arc::downgrade(&self.core))) as Arc<dyn DeviceWaker>,
    });

    let (emit_tx, mut emit_rx) = mpsc::unbounded_channel();
    let emitter_core = Arc::downgrade(&self.core);
    let emit_task = tokio::spawn(async move {
      while let Some(job) = emit_rx.recv().await {
        let Some(core) = emitter_core.upgrade() else { break };
        core.handle_emit(job).await;
      }
    });

    self.core.notify_auth(ProviderAuthState::Pending {
      user_code: None,
      verification_url: None,
      verification_url_complete: None,
    });
    let connect_client = client.clone();
    let connect_core = Arc::downgrade(&self.core);
    let connect_task = tokio::spawn(async move {
      if let Err(error) = connect_client.connect().await
        && let Some(core) = connect_core.upgrade()
      {
        core.notify_auth(ProviderAuthState::Failed {
          reason: format!("sign-in error: {error}"),
        });
      }
    });

    self.core.shared.lock().unwrap().connectivity_available = None;
    let resolver = Arc::new(SpotifyVoiceResolver { client: client.clone() });
    *self.core.session.lock().unwrap() = Some(Session {
      client,
      link,
      connect_task,
      emit_task,
      emit_tx,
      resolver,
    });
    Ok(())
  }

  async fn detach(&self) {
    let session = self.core.session.lock().unwrap().take();
    let Some(session) = session else { return };
    session.client.disconnect().await;
    session.link.sink.clear_source(PROVIDER_NAME);
    self.core.notify_now_playing(None);
    *self.core.np_observer.lock().unwrap() = None;
    *self.core.auth_observer.lock().unwrap() = None;
    self.core.reset_queue_dedup();
    let mut shared = self.core.shared.lock().unwrap();
    shared.liked_override.clear();
    shared.last_state = None;
    shared.last_state_at = None;
    shared.last_queue_items = Vec::new();
    shared.on_remote_speaker = false;
    shared.last_had_item = false;
    shared.last_emitted_remote_volume = None;
    shared.connectivity_available = None;
  }

  fn set_resume_target(&self, target: ResumeTarget) {
    *self.core.resume_target.lock().unwrap() = target;
    if let Some(client) = self.core.client() {
      client.set_placement(placement_of(target));
    }
  }

  async fn handle_peer_connected(&self, allow_auto_resume: bool) {
    let Some(client) = self.core.client() else { return };
    if allow_auto_resume {
      let resume_client = client.clone();
      tokio::spawn(async move {
        if let Err(error) = resume_client.resume_on_connect().await {
          tracing::info!(%error, "connect auto-resume did not complete");
        }
      });
    }
    self.core.reset_queue_dedup();
    let pending = self
      .core
      .shared
      .lock()
      .unwrap()
      .last_state
      .clone()
      .filter(|state| state.track.is_some());
    if let Some(mut pending) = pending {
      let age = match client.current_position_ms().await {
        Some(fresh) => {
          pending.position_ms = fresh;
          None
        }
        None => self.core.monotonic_age_ms(),
      };
      let (hero, _) = self.core.art_edges();
      let (liked, supported) = self.core.like_fields(pending.track.as_ref());
      let snapshot = self.core.make_snapshot(&pending, hero, liked, supported, age);
      let on_remote = pending.on_remote_speaker;
      self.core.enqueue(EmitJob::Player {
        snapshot: Box::new(snapshot),
        has_item: true,
        on_remote,
      });
    }
    let (queue, thumb) = {
      let shared = self.core.shared.lock().unwrap();
      (shared.last_queue_items.clone(), shared.thumb_edge)
    };
    if !queue.is_empty() {
      self.core.enqueue(EmitJob::Queue {
        entries: queue,
        thumb_edge: thumb,
      });
    }
  }

  async fn resumed(&self) {
    if let Some(client) = self.core.client() {
      client.resync().await;
    }
  }

  async fn connectivity_changed(&self, online: bool) {
    SpotifyProvider::connectivity_changed(self, online).await;
  }

  async fn owns_volume(&self) -> bool {
    self.core.shared.lock().unwrap().on_remote_speaker
  }

  async fn volume_up(&self) -> Result<f32, ProviderError> {
    let target = self
      .core
      .require()?
      .volume_step(VOLUME_STEP_PERCENT)
      .await
      .map_err(failed)?;
    let level = (target / 100.0) as f32;
    self.core.note_remote_volume(level);
    Ok(level)
  }

  async fn volume_down(&self) -> Result<f32, ProviderError> {
    let target = self
      .core
      .require()?
      .volume_step(-VOLUME_STEP_PERCENT)
      .await
      .map_err(failed)?;
    let level = (target / 100.0) as f32;
    self.core.note_remote_volume(level);
    Ok(level)
  }

  async fn set_volume(&self, level: f32) -> Result<f32, ProviderError> {
    self
      .core
      .require()?
      .set_volume(f64::from(level) * 100.0)
      .await
      .map_err(failed)?;
    self.core.note_remote_volume(level);
    Ok(level)
  }

  async fn transfer_to(&self, target_id: &str) -> Result<(), ProviderError> {
    self.core.require()?.transfer(target_id).await.map_err(failed)
  }

  async fn asset(&self, id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    let Some((url, max_edge)) = IMAGE_CODEC.parse(id) else {
      return Ok(None);
    };
    let scaled = self.core.art_cache.scaled(&url, max_edge).await;
    Ok(scaled.map(|bytes| AssetBytes {
      bytes,
      mime: Some("image/jpeg".into()),
    }))
  }

  async fn lyrics(&self, _track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError> {
    Ok(None)
  }

  async fn browse(&self, req: LibraryBrowseRequest) -> Result<BrowseResult, ProviderError> {
    let client = self.core.require()?;
    let (edge, _) = self.core.art_edges();
    let result = if is_root(req.node_id.as_deref()) {
      let shelves = client.root_browse(req.sections, req.preview).await.map_err(failed)?;
      BrowseResult {
        total: Some(shelves.len() as u32),
        entries: shelves
          .iter()
          .map(|shelf| BrowseEntry::Folder(folder(shelf, edge)))
          .collect(),
        has_more: false,
      }
    } else {
      let node = req.node_id.as_deref().unwrap_or_default();
      let page = client.browse(node, req.limit, req.offset).await.map_err(failed)?;
      BrowseResult {
        entries: page
          .items
          .iter()
          .filter_map(|item| library_item(item, edge).map(BrowseEntry::Item))
          .collect(),
        total: page.total,
        has_more: page.has_more,
      }
    };
    self.core.warm_art(&result);
    Ok(result)
  }

  async fn resolve_context(&self, uri: &str) -> Result<ContextResolveReply, ProviderError> {
    let client = self.core.require()?;
    let item = client.resolve_context(uri).await.map_err(failed)?;
    let (edge, _) = self.core.art_edges();
    Ok(ContextResolveReply {
      name: none_if_empty(&item.title),
      artwork_id: art_asset_id(&item.image_id, edge),
      subtitle: none_if_empty(&item.subtitle),
    })
  }

  async fn search(&self, req: LibrarySearchRequest) -> Result<SearchResult, ProviderError> {
    let client = self.core.require()?;
    let kinds = match &req.kinds {
      Some(kinds) if !kinds.is_empty() => kinds.clone(),
      _ => vec![
        ItemKind::Track,
        ItemKind::Album,
        ItemKind::Artist,
        ItemKind::Playlist,
        ItemKind::Show,
        ItemKind::PodcastEpisode,
      ],
    };
    let results = client.search(&req.query, req.limit).await.map_err(failed)?;
    let (edge, _) = self.core.art_edges();
    let limit = req.limit as usize;
    let mut items = Vec::new();
    let mut present = Vec::new();
    let mut full = false;
    for kind in kinds {
      let bucket = match kind {
        ItemKind::Track => &results.tracks,
        ItemKind::Album => &results.albums,
        ItemKind::Artist => &results.artists,
        ItemKind::Playlist => &results.playlists,
        ItemKind::Show => &results.shows,
        ItemKind::PodcastEpisode => &results.episodes,
        ItemKind::Station => continue,
      };
      let mapped: Vec<LibraryItem> = bucket.iter().filter_map(|item| library_item(item, edge)).collect();
      if !mapped.is_empty() {
        present.push(kind);
        if mapped.len() >= limit {
          full = true;
        }
      }
      items.extend(mapped);
    }
    Ok(SearchResult {
      items,
      kinds: present,
      total: None,
      has_more: full,
    })
  }

  async fn recommendations(&self, req: LibraryRecommendationsRequest) -> Result<RecommendationsResult, ProviderError> {
    let client = self.core.require()?;
    let (edge, _) = self.core.art_edges();
    if let Some(artist) = req.seeds.iter().find(|seed| seed.kind == ItemKind::Artist) {
      let page = client.browse(&artist.uri, req.limit, 0).await.map_err(failed)?;
      return Ok(RecommendationsResult {
        items: page.items.iter().filter_map(|item| library_item(item, edge)).collect(),
        total: None,
        has_more: false,
      });
    }
    Ok(RecommendationsResult {
      items: Vec::new(),
      total: None,
      has_more: false,
    })
  }

  async fn favorites_list(&self, req: LibraryFavoritesListRequest) -> Result<FavoritesPage, ProviderError> {
    let client = self.core.require()?;
    let (edge, _) = self.core.art_edges();
    let page = client.favorites_list(req.limit, req.offset).await.map_err(failed)?;
    Ok(FavoritesPage {
      items: page.items.iter().filter_map(|item| library_item(item, edge)).collect(),
      total: page.total,
      has_more: page.has_more,
    })
  }

  async fn favorites_contains(&self, req: LibraryFavoritesContainsRequest) -> Result<Vec<bool>, ProviderError> {
    self.core.require()?.favorites_contains(req.uris).await.map_err(failed)
  }

  async fn favorites_toggle(&self, item: ItemRef) -> Result<(), ProviderError> {
    let client = self.core.require()?;
    let saved = client
      .favorites_contains(vec![item.uri.clone()])
      .await
      .map_err(failed)?
      .first()
      .copied()
      .unwrap_or(false);
    client.favorites_set(&item.uri, !saved).await.map_err(failed)?;
    self.core.apply_liked_change(&item.uri, !saved).await;
    Ok(())
  }

  async fn favorites_set(&self, item: ItemRef, liked: bool) -> Result<(), ProviderError> {
    self
      .core
      .require()?
      .favorites_set(&item.uri, liked)
      .await
      .map_err(failed)?;
    self.core.apply_liked_change(&item.uri, liked).await;
    Ok(())
  }

  async fn favorites_set_many(&self, entries: Vec<FavoritesSet>) -> Result<(), ProviderError> {
    let client = self.core.require()?;
    for entry in entries {
      client
        .favorites_set(&entry.item.uri, entry.liked)
        .await
        .map_err(failed)?;
      self.core.apply_liked_change(&entry.item.uri, entry.liked).await;
    }
    Ok(())
  }

  async fn set_art_profile(&self, hero_px: u32, thumb_px: u32) {
    let mut shared = self.core.shared.lock().unwrap();
    shared.hero_edge = hero_px.max(1);
    shared.thumb_edge = thumb_px.max(1);
  }
}

#[cfg(test)]
mod tests {
  use spotify::model::{Album as SpAlbum, Artist as SpArtist};

  use super::*;

  #[test]
  fn a_request_carrying_only_a_popularity_filter_still_reaches_the_catalog() {
    for filter in [
      NluPopularityFilter::Top5,
      NluPopularityFilter::Top10,
      NluPopularityFilter::Popular,
      NluPopularityFilter::Recent,
      NluPopularityFilter::New,
      NluPopularityFilter::First,
      NluPopularityFilter::Random,
    ] {
      let request = VoiceResolveRequest {
        popularity_filter: Some(voice_popularity(filter)),
        ..Default::default()
      };
      assert!(
        !names_nothing(&request),
        "{filter:?} is the whole request; dropping it here would silently ignore the filter"
      );
    }
  }

  #[test]
  fn a_request_naming_nothing_at_all_never_reaches_the_catalog() {
    assert!(names_nothing(&VoiceResolveRequest::default()));
    assert!(
      !names_nothing(&VoiceResolveRequest {
        target_type: Some(VoiceTargetKind::Album),
        ..Default::default()
      }),
      "a bare kind is read against now playing, so the resolver has to see it"
    );
  }

  fn browse_item(uri: &str, title: &str) -> SpBrowseItem {
    SpBrowseItem {
      uri: uri.into(),
      title: title.into(),
      subtitle: "Sub".into(),
      image_id: "ab67616d00001e02deadbeef".into(),
      artists: vec![SpArtist {
        uri: "spotify:artist:1".into(),
        name: "Artist".into(),
      }],
      album: SpAlbum {
        uri: "spotify:album:1".into(),
        name: "Album".into(),
        image_id: "ab67616d00001e02deadbeef".into(),
      },
      duration_ms: 1000,
      saved: false,
      playable: true,
      has_children: false,
    }
  }

  #[test]
  fn a_root_shelf_maps_to_a_folder_with_its_real_total() {
    let shelf = SpShelf {
      id: "playlists".into(),
      title: "Playlists".into(),
      items: vec![browse_item("spotify:playlist:1", "Mix")],
      total: 12,
    };
    let folder = folder(&shelf, 248);
    assert_eq!(folder.node_id, "playlists");
    assert_eq!(folder.title, "Playlists");
    assert_eq!(folder.total, Some(12), "the shelf's real total, not the preview count");
    assert_eq!(folder.preview_children.as_ref().unwrap().len(), 1);
  }

  #[test]
  fn an_empty_shelf_folds_its_preview_to_nothing() {
    let shelf = SpShelf {
      id: "albums".into(),
      title: "Albums".into(),
      items: Vec::new(),
      total: 3,
    };
    assert!(folder(&shelf, 248).preview_children.is_none());
  }

  #[test]
  fn library_items_map_by_uri_kind() {
    #[allow(clippy::type_complexity)]
    let cases: [(&str, fn(&LibraryItem) -> bool); 7] = [
      ("spotify:track:1", |item| matches!(item, LibraryItem::Track(_))),
      ("spotify:album:1", |item| matches!(item, LibraryItem::Album(_))),
      ("spotify:artist:1", |item| matches!(item, LibraryItem::Artist(_))),
      ("spotify:playlist:1", |item| matches!(item, LibraryItem::Playlist(_))),
      ("spotify:user:u:collection", |item| {
        matches!(item, LibraryItem::Playlist(_))
      }),
      ("spotify:show:1", |item| matches!(item, LibraryItem::Show(_))),
      ("spotify:episode:1", |item| {
        matches!(item, LibraryItem::PodcastEpisode(_))
      }),
    ];
    for (uri, is_expected) in cases {
      let item = library_item(&browse_item(uri, "T"), 248).unwrap_or_else(|| panic!("{uri} maps"));
      assert!(is_expected(&item), "{uri} mapped to the wrong kind");
    }
    assert!(library_item(&browse_item("spotify:user:u", "T"), 248).is_none());
    assert!(library_item(&browse_item("spotify:unknown:1", "T"), 248).is_none());
  }

  #[test]
  fn a_track_item_wraps_its_art_in_an_asset_id() {
    let LibraryItem::Track(track) = library_item(&browse_item("spotify:track:1", "Song"), 248).unwrap() else {
      panic!("a track maps to a track");
    };
    assert!(track.image_id.starts_with("spotify/img/248/i"), "{}", track.image_id);
    assert_eq!(track.artists.len(), 1);
  }

  #[test]
  fn art_asset_ids_cover_the_three_reference_shapes() {
    assert_eq!(art_asset_id("", 248), None);
    assert_eq!(
      art_asset_id("builtin:liked-songs", 248).as_deref(),
      Some("builtin/img/liked-songs")
    );
    assert_eq!(
      art_asset_id("deadbeef", 96).as_deref(),
      Some("spotify/img/96/ideadbeef"),
      "a bare hex ref is an scdn image"
    );
    assert!(
      art_asset_id("https://example.com/a.png", 96)
        .unwrap()
        .starts_with("spotify/img/96/u"),
      "a foreign url encodes whole"
    );
  }

  #[test]
  fn the_forward_slide_runway_is_the_surviving_suffix() {
    let last: Vec<String> = (1..=10).map(|i| format!("t{i}")).collect();
    let slid: Vec<String> = (2..=11).map(|i| format!("t{i}")).collect();
    assert_eq!(forward_slide_runway(&last, &slid), Some(9));
    let replaced: Vec<String> = (20..=29).map(|i| format!("t{i}")).collect();
    assert_eq!(forward_slide_runway(&last, &replaced), None);
    assert_eq!(forward_slide_runway(&[], &slid), None);
  }

  #[test]
  fn the_now_playing_update_carries_the_app_identity() {
    let state = SpPlayerState {
      track: Some(SpTrack {
        uri: "spotify:track:1".into(),
        name: "Song".into(),
        album: SpAlbum {
          uri: "spotify:album:1".into(),
          name: "Album".into(),
          image_id: "deadbeef".into(),
        },
        artists: vec![SpArtist {
          uri: "spotify:artist:1".into(),
          name: "Artist".into(),
        }],
        duration_ms: 1000,
        ..SpTrack::default()
      }),
      is_paused: false,
      shuffle: true,
      ..SpPlayerState::default()
    };
    let update = make_update(&state, 248, Some(true), Some(true));
    let media = update.media_item.unwrap();
    assert_eq!(media.title.as_deref(), Some("Song"));
    assert_eq!(media.artist.as_deref(), Some("Artist"));
    assert_eq!(media.liked, Some(true));
    let playback = update.playback.unwrap();
    assert_eq!(playback.app_bundle.as_deref(), Some("com.spotify.client"));
    assert_eq!(playback.app_display_name.as_deref(), Some("Spotify"));
    assert_eq!(playback.playing, Some(true));
    assert_eq!(playback.shuffle_mode, Some(ShuffleMode::Songs));
  }
}
