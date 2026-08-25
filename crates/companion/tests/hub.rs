#[path = "dispatch/flow.rs"]
mod flow;
#[path = "support/poll.rs"]
mod poll;
#[path = "dispatch/quiet.rs"]
mod quiet;
#[path = "dispatch/support.rs"]
mod support;

use std::{
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::{
  api::{CapabilityFlags, HostInfo},
  dispatch::player::PlayerDispatcher,
  hub::Hub,
  provider::{AssetBytes, PlayerTransport, Provider, ProviderError, ProviderLink, ProviderRegistry, ResumeTarget},
  voice::dispatcher::{CatalogError, VoiceCatalogResolver},
};
use bridgething_gateway::{Gateway, PlayerHandler};
use libbridgething::{
  BrowseResult, CompanionAuthorityScope, FavoritesPage, GatewayCapabilities, ItemRef, Lyrics, MediaItem, MusicProvider,
  NluResolvedIntent, NluSlots, Playback, PlaybackState, PlayerOptions, PlayerState, RecommendationsResult,
  SearchResult,
  gateway::{
    AuthorityClaim, AuthorityRelease, ContextResolveReply, FavoritesSet, GatewayToBridgeAuthorityMsg,
    GatewayToBridgeCapabilitiesMsg, GatewayToBridgeMsg, GatewayToBridgeMsgData, GatewayToBridgePlayerMsg,
    LibraryBrowseRequest, LibraryFavoritesContainsRequest, LibraryFavoritesListRequest, LibraryRecommendationsRequest,
    LibrarySearchRequest, PlayUri, PlayerErrorReply, QueueSnapshot, TrackIdentity,
  },
};
use poll::eventually;
use support::Peer;

fn host() -> HostInfo {
  HostInfo {
    app_name: "hub-test".into(),
    app_version: "0.0.1".into(),
    os_name: "test".into(),
    os_version: String::new(),
    host_identifier: String::new(),
  }
}

fn flags() -> CapabilityFlags {
  CapabilityFlags {
    geo: true,
    notifications: false,
    net_fetch: true,
    net_ws: true,
    audio_tts: true,
    voice_model: true,
  }
}

fn snapshot(state: PlaybackState, uri: &str) -> PlayerState {
  PlayerState {
    track: Some(MediaItem {
      uri: Some(uri.into()),
      title: Some("t".into()),
      ..MediaItem::default()
    }),
    playback: Playback {
      state,
      ..Playback::default()
    },
    queue: Vec::new(),
    options: PlayerOptions::default(),
    context: None,
    target: None,
  }
}

fn claim_of(scope: CompanionAuthorityScope) -> impl Fn(&GatewayToBridgeMsg) -> Option<AuthorityClaim> {
  move |msg| match &msg.data {
    GatewayToBridgeMsgData::Authority(GatewayToBridgeAuthorityMsg::Claim(claim)) if claim.scope == scope => {
      Some(claim.clone())
    }
    _ => None,
  }
}

fn release_of(scope: CompanionAuthorityScope) -> impl Fn(&GatewayToBridgeMsg) -> Option<AuthorityRelease> {
  move |msg| match &msg.data {
    GatewayToBridgeMsgData::Authority(GatewayToBridgeAuthorityMsg::Release(release)) if release.scope == scope => {
      Some(*release)
    }
    _ => None,
  }
}

fn announce(msg: &GatewayToBridgeMsg) -> Option<GatewayCapabilities> {
  match &msg.data {
    GatewayToBridgeMsgData::Capabilities(GatewayToBridgeCapabilitiesMsg::Announce(caps)) => Some(caps.clone()),
    _ => None,
  }
}

fn player_error(msg: &GatewayToBridgeMsg) -> Option<PlayerErrorReply> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::ErrorEvent(reply)) => Some(reply.clone()),
    _ => None,
  }
}

struct HubCatalog {
  uri: String,
}

#[async_trait::async_trait]
impl VoiceCatalogResolver for HubCatalog {
  async fn decorate(&self, mut resolved: NluResolvedIntent) -> Result<NluResolvedIntent, CatalogError> {
    resolved.slots.uri = Some(self.uri.clone());
    Ok(resolved)
  }
}

struct HubProvider {
  name: String,
  schemes: Vec<String>,
  calls: Mutex<Vec<String>>,
  peer_connects: Mutex<Vec<bool>>,
  resume_targets: Mutex<Vec<ResumeTarget>>,
  resolver: Option<Arc<dyn VoiceCatalogResolver>>,
}

impl HubProvider {
  fn new(name: &str, schemes: &[&str]) -> Arc<Self> {
    HubProvider::build(name, schemes, None)
  }

  fn resolving(name: &str, schemes: &[&str], uri: &str) -> Arc<Self> {
    HubProvider::build(name, schemes, Some(Arc::new(HubCatalog { uri: uri.into() })))
  }

  fn build(name: &str, schemes: &[&str], resolver: Option<Arc<dyn VoiceCatalogResolver>>) -> Arc<Self> {
    Arc::new(Self {
      name: name.into(),
      schemes: schemes.iter().map(|s| (*s).to_owned()).collect(),
      calls: Mutex::new(Vec::new()),
      peer_connects: Mutex::new(Vec::new()),
      resume_targets: Mutex::new(Vec::new()),
      resolver,
    })
  }

  fn saw(&self, what: &str) -> bool {
    self.calls.lock().unwrap().iter().any(|call| call == what)
  }

  fn peer_connects(&self) -> Vec<bool> {
    self.peer_connects.lock().unwrap().clone()
  }

  fn resume_targets(&self) -> Vec<ResumeTarget> {
    self.resume_targets.lock().unwrap().clone()
  }
}

#[async_trait::async_trait]
impl PlayerTransport for HubProvider {
  async fn play(&self, uri: PlayUri) -> Result<(), ProviderError> {
    self.calls.lock().unwrap().push(format!("play:{}", uri.uri));
    Ok(())
  }

  async fn pause(&self) -> Result<(), ProviderError> {
    self.calls.lock().unwrap().push("pause".into());
    Ok(())
  }
}

#[async_trait::async_trait]
impl Provider for HubProvider {
  fn name(&self) -> &str {
    &self.name
  }

  fn display_name(&self) -> &str {
    &self.name
  }

  fn uri_schemes(&self) -> Vec<String> {
    self.schemes.clone()
  }

  fn music_provider(&self) -> MusicProvider {
    MusicProvider::None
  }

  fn voice_resolver(&self) -> Option<Arc<dyn VoiceCatalogResolver>> {
    self.resolver.clone()
  }

  fn set_resume_target(&self, target: ResumeTarget) {
    self.resume_targets.lock().unwrap().push(target);
  }

  async fn attach(&self, _link: ProviderLink) -> Result<(), ProviderError> {
    Ok(())
  }

  async fn detach(&self) {}

  async fn handle_peer_connected(&self, allow_auto_resume: bool) {
    self.peer_connects.lock().unwrap().push(allow_auto_resume);
  }

  async fn asset(&self, id: &str) -> Result<Option<AssetBytes>, ProviderError> {
    if id.starts_with(&format!("{}/", self.name)) {
      return Ok(Some(AssetBytes {
        bytes: b"art".to_vec(),
        mime: Some("image/jpeg".into()),
      }));
    }
    Ok(None)
  }

  async fn lyrics(&self, _track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError> {
    Ok(None)
  }

  async fn browse(&self, _request: LibraryBrowseRequest) -> Result<BrowseResult, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn resolve_context(&self, _uri: &str) -> Result<ContextResolveReply, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn search(&self, _request: LibrarySearchRequest) -> Result<SearchResult, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn recommendations(
    &self,
    _request: LibraryRecommendationsRequest,
  ) -> Result<RecommendationsResult, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn favorites_list(&self, _request: LibraryFavoritesListRequest) -> Result<FavoritesPage, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn favorites_contains(&self, _request: LibraryFavoritesContainsRequest) -> Result<Vec<bool>, ProviderError> {
    Err(ProviderError::NotImplemented)
  }

  async fn favorites_toggle(&self, _item: ItemRef) -> Result<(), ProviderError> {
    Ok(())
  }

  async fn favorites_set(&self, _item: ItemRef, _liked: bool) -> Result<(), ProviderError> {
    Ok(())
  }

  async fn favorites_set_many(&self, _entries: Vec<FavoritesSet>) -> Result<(), ProviderError> {
    Ok(())
  }

  async fn set_art_profile(&self, _hero_px: u32, _thumb_px: u32) {}
}

fn hub(gateway: Gateway) -> Arc<Hub> {
  let hub = Hub::new(Arc::new(gateway), host(), flags());
  hub.start();
  hub
}

fn hub_on(gateway: Gateway, os_name: &str) -> Arc<Hub> {
  let hub = Hub::new(
    Arc::new(gateway),
    HostInfo {
      os_name: os_name.into(),
      ..host()
    },
    flags(),
  );
  hub.start();
  hub
}

// ---- arbitration -------------------------------------------------------------

#[tokio::test]
async fn a_playing_source_wins_over_a_paused_one() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Paused, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );
  sink.submit_player(
    "applemusic",
    snapshot(PlaybackState::Playing, "applemusic:song:b"),
    "com.apple.Music",
    true,
    false,
  );
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("applemusic")).await,
    "apple music became audible"
  );
}

#[tokio::test]
async fn recency_picks_when_there_is_no_incumbent() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "applemusic",
    snapshot(PlaybackState::Paused, "applemusic:song:b"),
    "com.apple.Music",
    true,
    false,
  );
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("applemusic")).await,
    "with nothing playing and nobody holding the floor, the freshest source is the only evidence there is"
  );
}

#[tokio::test]
async fn a_paused_incumbent_keeps_the_floor_against_a_chattering_rival() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "applemusic",
    snapshot(PlaybackState::Paused, "applemusic:song:b"),
    "com.apple.Music",
    true,
    false,
  );
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("applemusic")).await,
    "apple music took the floor"
  );

  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Paused, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );

  assert!(
    quiet_for(Duration::from_millis(300), || {
      hub.now_playing().current_source().as_deref() == Some("applemusic")
    })
    .await,
    "seq means most recently updated, not most recently played; letting it decide hands the transport to \
     whichever source last said anything, which is how pause on one player starts driving another"
  );
}

#[tokio::test]
async fn a_playing_rival_takes_the_floor_from_a_paused_incumbent() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "applemusic",
    snapshot(PlaybackState::Paused, "applemusic:song:b"),
    "com.apple.Music",
    true,
    false,
  );
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("applemusic")).await,
    "apple music took the floor"
  );

  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Playing, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );

  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("spotify")).await,
    "stickiness is not a lock: something that actually starts playing is the user's intent and takes over"
  );
}

async fn quiet_for(window: Duration, holds: impl Fn() -> bool) -> bool {
  let deadline = tokio::time::Instant::now() + window;
  while tokio::time::Instant::now() < deadline {
    if !holds() {
      return false;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  holds()
}

#[tokio::test]
async fn a_playing_source_takes_over_from_a_playing_one() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Playing, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("spotify")).await,
    "spotify was current"
  );
  sink.submit_player(
    "applemusic",
    snapshot(PlaybackState::Playing, "applemusic:song:b"),
    "com.apple.Music",
    true,
    false,
  );
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("applemusic")).await,
    "the newer playing source took over"
  );
}

#[tokio::test]
async fn clearing_the_current_source_falls_back_to_the_other() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Paused, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );
  sink.submit_player(
    "applemusic",
    snapshot(PlaybackState::Playing, "applemusic:song:b"),
    "com.apple.Music",
    true,
    false,
  );
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("applemusic")).await,
    "apple music was current"
  );
  sink.clear_source("applemusic");
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("spotify")).await,
    "the pick fell back to spotify"
  );
}

#[tokio::test]
async fn a_transport_verb_routes_to_the_audible_source() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway.clone());
  let spotify = HubProvider::new("spotify", &["spotify"]);
  let apple = HubProvider::new("applemusic", &["applemusic"]);
  hub.attach(spotify.clone()).await.unwrap();
  hub.attach(apple.clone()).await.unwrap();
  let sink = hub.sink();
  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Paused, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );
  sink.submit_player(
    "applemusic",
    snapshot(PlaybackState::Playing, "applemusic:song:b"),
    "com.apple.Music",
    true,
    false,
  );
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("applemusic")).await,
    "apple music was audible"
  );

  let dispatch = PlayerDispatcher::new(hub.clone(), Arc::new(gateway));
  dispatch.pause().await.unwrap();

  assert!(apple.saw("pause"));
  assert!(!spotify.saw("pause"));
}

// ---- authority ---------------------------------------------------------------

#[tokio::test]
async fn the_winning_bundle_claims_the_now_playing_scopes() {
  let (gateway, peer) = Peer::link();
  let hub = hub(gateway);
  hub.sink().submit_player(
    "spotify",
    snapshot(PlaybackState::Playing, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );
  let claim = peer
    .wait(
      "a playback claim",
      claim_of(CompanionAuthorityScope::NowPlayingPlayback),
    )
    .await;
  assert_eq!(claim.app_bundle.as_deref(), Some("com.spotify.client"));
  peer
    .wait(
      "a metadata claim",
      claim_of(CompanionAuthorityScope::NowPlayingMetadata),
    )
    .await;
  peer
    .quiet(
      "a volume claim nobody asked for",
      claim_of(CompanionAuthorityScope::Volume),
    )
    .await;
}

#[tokio::test]
async fn the_volume_scope_is_claimed_only_when_the_audible_source_wants_it() {
  let (gateway, peer) = Peer::link();
  let hub = hub(gateway);
  hub.sink().submit_player(
    "spotify",
    snapshot(PlaybackState::Playing, "spotify:track:a"),
    "com.spotify.client",
    true,
    true,
  );
  peer
    .wait("the volume claim", claim_of(CompanionAuthorityScope::Volume))
    .await;
}

#[tokio::test]
async fn the_volume_scope_is_released_when_the_source_leaves_the_remote_speaker() {
  let (gateway, peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Playing, "spotify:track:a"),
    "com.spotify.client",
    true,
    true,
  );
  peer
    .wait("the volume claim", claim_of(CompanionAuthorityScope::Volume))
    .await;
  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Playing, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );
  peer
    .wait("the volume release", release_of(CompanionAuthorityScope::Volume))
    .await;
}

#[tokio::test]
async fn losing_the_last_item_releases_every_held_scope() {
  let (gateway, peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Playing, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );
  peer
    .wait(
      "the playback claim",
      claim_of(CompanionAuthorityScope::NowPlayingPlayback),
    )
    .await;
  sink.clear_source("spotify");
  peer
    .wait(
      "the playback release",
      release_of(CompanionAuthorityScope::NowPlayingPlayback),
    )
    .await;
  peer
    .wait(
      "the metadata release",
      release_of(CompanionAuthorityScope::NowPlayingMetadata),
    )
    .await;
}

// ---- queue and reconnect -----------------------------------------------------

#[tokio::test]
async fn a_queue_from_a_non_current_source_stays_home() {
  let (gateway, peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Playing, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );
  assert!(
    eventually(|| hub.now_playing().current_source().as_deref() == Some("spotify")).await,
    "spotify was current"
  );
  sink.submit_queue(
    "applemusic",
    QueueSnapshot {
      order: vec!["applemusic:song:x".into()],
      items: Vec::new(),
    },
  );
  peer
    .quiet("a queue from the losing source", |msg| match &msg.data {
      GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::QueueChanged(queue))
        if queue.order == vec!["applemusic:song:x".to_string()] =>
      {
        Some(())
      }
      _ => None,
    })
    .await;
}

#[tokio::test]
async fn reconnect_reclaims_and_replays_the_current_source() {
  let (gateway, peer) = Peer::link();
  let hub = hub(gateway);
  let sink = hub.sink();
  sink.submit_player(
    "spotify",
    snapshot(PlaybackState::Playing, "spotify:track:a"),
    "com.spotify.client",
    true,
    false,
  );
  peer
    .wait("the first claim", claim_of(CompanionAuthorityScope::NowPlayingPlayback))
    .await;
  hub.now_playing().on_connect();
  peer
    .wait_for(
      "two playback claims",
      2,
      claim_of(CompanionAuthorityScope::NowPlayingPlayback),
    )
    .await;
  peer
    .wait_for("two snapshots", 2, |msg| match &msg.data {
      GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::Snapshot(_)) => Some(()),
      _ => None,
    })
    .await;
}

// ---- routing and capabilities ------------------------------------------------

#[tokio::test]
async fn announced_schemes_are_the_union_of_attached_providers() {
  let (gateway, peer) = Peer::link();
  let hub = hub(gateway);
  hub.attach(HubProvider::new("spotify", &["spotify"])).await.unwrap();
  hub.attach(HubProvider::new("second", &["second"])).await.unwrap();
  let caps = peer
    .wait("an announce carrying both schemes", |msg| {
      announce(msg).filter(|caps| caps.uri_schemes.len() == 2)
    })
    .await;
  assert_eq!(caps.uri_schemes, vec!["second".to_string(), "spotify".to_string()]);
}

#[tokio::test]
async fn priority_orders_the_announced_schemes() {
  let (gateway, peer) = Peer::link();
  let hub = hub(gateway);
  hub.attach(HubProvider::new("spotify", &["spotify"])).await.unwrap();
  hub
    .attach(HubProvider::new("applemusic", &["applemusic"]))
    .await
    .unwrap();
  hub.set_priority(vec!["spotify".into(), "applemusic".into()]).await;
  peer
    .wait("the ranked announce", |msg| {
      announce(msg).filter(|caps| caps.uri_schemes == vec!["spotify".to_string(), "applemusic".to_string()])
    })
    .await;
}

#[tokio::test]
async fn play_routes_by_uri_scheme() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway.clone());
  let first = HubProvider::new("spotify", &["spotify"]);
  let second = HubProvider::new("second", &["second"]);
  hub.attach(first.clone()).await.unwrap();
  hub.attach(second.clone()).await.unwrap();

  let dispatch = PlayerDispatcher::new(hub.clone(), Arc::new(gateway));
  dispatch
    .play(PlayUri {
      uri: "second:track:xyz".into(),
      context: None,
    })
    .await
    .unwrap();

  assert!(second.saw("play:second:track:xyz"));
  assert!(!first.calls.lock().unwrap().iter().any(|call| call.starts_with("play:")));
}

#[tokio::test]
async fn play_for_an_unclaimed_scheme_is_dropped_and_reported() {
  let (gateway, peer) = Peer::link();
  let hub = hub(gateway.clone());
  let first = HubProvider::new("spotify", &["spotify"]);
  let second = HubProvider::new("second", &["second"]);
  hub.attach(first.clone()).await.unwrap();
  hub.attach(second.clone()).await.unwrap();

  let dispatch = PlayerDispatcher::new(hub.clone(), Arc::new(gateway));
  dispatch
    .play(PlayUri {
      uri: "tidal:track:xyz".into(),
      context: None,
    })
    .await
    .unwrap();

  let reported = peer.wait("the scheme-unclaimed report", player_error).await;
  match reported.error {
    libbridgething::PlayerError::SchemeUnclaimed { scheme } => assert_eq!(scheme, "tidal"),
    other => panic!("expected schemeUnclaimed, got {other:?}"),
  }
  assert!(!first.calls.lock().unwrap().iter().any(|call| call.starts_with("play:")));
  assert!(
    !second
      .calls
      .lock()
      .unwrap()
      .iter()
      .any(|call| call.starts_with("play:"))
  );
}

#[tokio::test]
async fn play_marks_the_provider_as_last_played_from() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway.clone());
  let first = HubProvider::new("spotify", &["spotify"]);
  let second = HubProvider::new("second", &["second"]);
  hub.attach(first.clone()).await.unwrap();
  hub.attach(second.clone()).await.unwrap();
  hub.set_priority(vec!["spotify".into(), "second".into()]).await;

  let dispatch = PlayerDispatcher::new(hub.clone(), Arc::new(gateway));
  dispatch
    .play(PlayUri {
      uri: "second:track:xyz".into(),
      context: None,
    })
    .await
    .unwrap();

  assert_eq!(hub.library().unwrap().name(), "second");
}

#[tokio::test]
async fn the_asset_id_prefix_names_the_provider_that_minted_it() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let first = HubProvider::new("fake", &["fake"]);
  let second = HubProvider::new("second", &["second"]);
  hub.attach(first.clone()).await.unwrap();
  hub.attach(second.clone()).await.unwrap();

  let owner = hub.for_uri("fake:img").unwrap();
  assert_eq!(owner.name(), "fake");
  let bytes = first.asset("fake/img/248/abc").await.unwrap();
  assert!(bytes.is_some());
}

// ---- connect resume ----------------------------------------------------------

#[tokio::test]
async fn only_one_provider_may_auto_resume_on_connect() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let first = HubProvider::new("spotify", &["spotify"]);
  let second = HubProvider::new("second", &["second"]);
  hub.attach(first.clone()).await.unwrap();
  hub.attach(second.clone()).await.unwrap();

  hub.peer_connected("carthing-1").await;

  let first_allowed = first.peer_connects().contains(&true);
  let second_allowed = second.peer_connects().contains(&true);
  assert!(
    !(first_allowed && second_allowed),
    "only one provider may resume on connect"
  );
  assert!(first_allowed || second_allowed, "someone must be allowed to resume");
}

#[tokio::test]
async fn the_last_played_from_provider_wins_the_resume() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let first = HubProvider::new("spotify", &["spotify"]);
  let second = HubProvider::new("second", &["second"]);
  hub.attach(first.clone()).await.unwrap();
  hub.attach(second.clone()).await.unwrap();
  hub.set_priority(vec!["spotify".into(), "second".into()]).await;
  hub.mark_played_from("second");

  hub.peer_connected("carthing-1").await;

  assert_eq!(second.peer_connects(), vec![true]);
  assert_eq!(first.peer_connects(), vec![false]);
}

#[tokio::test]
async fn a_reconnect_after_the_cooldown_resumes_again() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  hub.set_auto_resume_cooldown(Duration::from_millis(50));
  let provider = HubProvider::new("spotify", &["spotify"]);
  hub.attach(provider.clone()).await.unwrap();

  hub.peer_connected("carthing-1").await;
  tokio::time::sleep(Duration::from_millis(80)).await;
  hub.peer_connected("carthing-1").await;

  assert_eq!(
    provider.peer_connects(),
    vec![true, true],
    "the drop is recent but the last resume is not, so this connect must resume"
  );
}

#[tokio::test]
async fn a_second_connect_inside_the_cooldown_does_not_resume_again() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let provider = HubProvider::new("spotify", &["spotify"]);
  hub.attach(provider.clone()).await.unwrap();

  hub.peer_connected("carthing-1").await;
  hub.peer_connected("carthing-1").await;

  assert_eq!(
    provider.peer_connects(),
    vec![true, false],
    "a re-dial inside the cooldown must not resume a second time"
  );
}

#[tokio::test]
async fn where_an_unset_device_resumes_follows_the_kind_of_host_this_is() {
  for phone in ["ios", "android", "iOS", "Android"] {
    let (gateway, _peer) = Peer::link();
    assert_eq!(
      hub_on(gateway, phone).default_resume_target(),
      ResumeTarget::PhoneOnly,
      "{phone} is carried around and is the speaker of last resort, so resuming onto it is what was meant"
    );
  }

  for desk in ["macos", "windows", "linux"] {
    let (gateway, _peer) = Peer::link();
    assert_eq!(
      hub_on(gateway, desk).default_resume_target(),
      ResumeTarget::AnySpeaker,
      "{desk} is not carried anywhere, so resuming onto it alone is the one thing the user did not ask for"
    );
  }
}

#[tokio::test]
async fn a_desktop_hosts_unset_device_resumes_onto_any_speaker() {
  let (gateway, _peer) = Peer::link();
  let hub = hub_on(gateway, "macos");
  let provider = HubProvider::new("spotify", &["spotify"]);
  hub.attach(provider.clone()).await.unwrap();

  hub.peer_connected("carthing-1").await;

  assert_eq!(
    provider.resume_targets().last().copied(),
    Some(ResumeTarget::AnySpeaker),
    "the default reaches the provider that acts on it, not just the getter that reports it"
  );
}

#[tokio::test]
async fn a_disabled_device_never_resumes() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let provider = HubProvider::new("spotify", &["spotify"]);
  hub.attach(provider.clone()).await.unwrap();
  hub.set_device_auto_resume("carthing-1", false);

  hub.peer_connected("carthing-1").await;

  assert_eq!(
    provider.peer_connects(),
    vec![false],
    "auto-resume off must veto regardless of timing"
  );
}

// ---- registry ----------------------------------------------------------------

#[tokio::test]
async fn the_library_pick_is_sticky_to_last_played_from_then_priority() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  let first = HubProvider::new("applemusic", &["applemusic"]);
  let second = HubProvider::new("spotify", &["spotify"]);
  hub.attach(first.clone()).await.unwrap();
  hub.attach(second.clone()).await.unwrap();

  assert_eq!(hub.library().unwrap().name(), "applemusic", "sorted fallback");
  hub.set_priority(vec!["spotify".into()]).await;
  assert_eq!(hub.library().unwrap().name(), "spotify", "priority beats the sort");
  hub.mark_played_from("applemusic");
  assert_eq!(
    hub.library().unwrap().name(),
    "applemusic",
    "last played-from is stickiest"
  );
  hub.detach("applemusic").await;
  assert_eq!(
    hub.library().unwrap().name(),
    "spotify",
    "a detach clears the sticky pick"
  );
}

fn play_naming(target: &str) -> NluResolvedIntent {
  NluResolvedIntent {
    intent: "PLAY".into(),
    slots: NluSlots {
      target: Some(target.into()),
      ..NluSlots::default()
    },
    transcript: format!("play {target}"),
    alternates: None,
  }
}

#[tokio::test]
async fn a_voice_turn_resolves_through_the_library_provider() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  hub
    .attach(HubProvider::resolving("spotify", &["spotify"], "spotify:album:strokes"))
    .await
    .unwrap();

  let resolved = hub.decorate(play_naming("the strokes")).await.unwrap();
  assert_eq!(resolved.slots.uri.as_deref(), Some("spotify:album:strokes"));
}

#[tokio::test]
async fn a_voice_turn_with_no_provider_attached_stays_unresolved() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);

  let resolved = hub.decorate(play_naming("the strokes")).await.unwrap();
  assert_eq!(resolved.slots.uri, None);
}

#[tokio::test]
async fn a_detached_provider_stops_answering_voice_turns() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  hub
    .attach(HubProvider::resolving("spotify", &["spotify"], "spotify:album:strokes"))
    .await
    .unwrap();
  assert!(
    hub
      .decorate(play_naming("the strokes"))
      .await
      .unwrap()
      .slots
      .uri
      .is_some()
  );

  hub.detach("spotify").await;
  let resolved = hub.decorate(play_naming("the strokes")).await.unwrap();
  assert_eq!(
    resolved.slots.uri, None,
    "a resolver held past detach would play from a provider that is gone"
  );
}

#[tokio::test]
async fn the_library_pick_decides_which_provider_answers_a_voice_turn() {
  let (gateway, _peer) = Peer::link();
  let hub = hub(gateway);
  hub
    .attach(HubProvider::resolving(
      "applemusic",
      &["applemusic"],
      "applemusic:album:1",
    ))
    .await
    .unwrap();
  hub
    .attach(HubProvider::resolving("spotify", &["spotify"], "spotify:album:strokes"))
    .await
    .unwrap();

  hub.mark_played_from("spotify");
  let resolved = hub.decorate(play_naming("the strokes")).await.unwrap();
  assert_eq!(
    resolved.slots.uri.as_deref(),
    Some("spotify:album:strokes"),
    "the provider the device last played from answers the search"
  );
}
