use std::time::Duration;

use bridgething_gateway::Gateway;
use bridgething_test_harness::Harness;
use futures::StreamExt;
use libbridgething::{
  CompanionAuthorityScope, GatewayCapabilities, GatewayInfo, GeoAccuracy, MediaItem, MusicProvider, NluResolvedIntent,
  NluSlots, Notification, NotificationAction, NotificationApp, NotificationCategory, NotificationFlags, PlaybackTarget,
  PlaybackTargetKind, PlayerState, SurfaceAvailability, VoiceDispatchErrorCode,
  client::{BridgeToClientVoiceMsgEvent, VoiceActivity, VoicePhase},
  gateway::{
    AuthorityClaim, BridgeToGatewayAudioMsg, BridgeToGatewayGeoMsg, BridgeToGatewayMsgData,
    BridgeToGatewayNotificationsMsg, BridgeToGatewayPlayerMsg, BridgeToGatewayVoiceMsg, PlaybackTargets,
    PlayerSnapshotAck, PlayerSnapshotRequest, VoiceDispatch,
  },
};
use uuid::Uuid;

const CONVERGE: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(600);

fn caps(name: &str, provider: MusicProvider) -> GatewayCapabilities {
  GatewayCapabilities {
    gateway: GatewayInfo {
      address: String::new(),
      name: name.into(),
      os_name: "android".into(),
      app_name: name.into(),
      app_version: "0.0.0".into(),
      adapter_version: "harness".into(),
      lib_version: "0.0.0".into(),
      libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
    },
    music_provider: provider,
    ..Default::default()
  }
}

async fn claim_now_playing(gateway: &Gateway) {
  for scope in [
    CompanionAuthorityScope::NowPlayingMetadata,
    CompanionAuthorityScope::NowPlayingPlayback,
  ] {
    gateway
      .authority()
      .claim(AuthorityClaim {
        scope,
        app_bundle: Some("com.spotify.client".into()),
      })
      .await
      .expect("claim");
  }
}

fn phone_snapshot() -> PlayerState {
  PlayerState {
    track: Some(MediaItem {
      persistent_id: Some("spotify:track:phone".into()),
      title: Some("Phone Track".into()),
      artist: Some("Phone Artist".into()),
      duration_ms: Some(240_000),
      ..MediaItem::default()
    }),
    ..PlayerState::default()
  }
}

async fn connect_primary(harness: &Harness) -> Gateway {
  let phone = harness.connect_android().await.expect("phone connect");
  phone
    .capabilities()
    .announce(caps("harness-phone", MusicProvider::Spotify))
    .await
    .expect("phone announce");
  claim_now_playing(&phone).await;
  phone.player().snapshot(phone_snapshot()).await.expect("phone snapshot");
  phone
}

async fn connect_secondary(harness: &Harness) -> Gateway {
  let desktop = harness.connect_android().await.expect("desktop connect");
  desktop
    .capabilities()
    .announce(caps("harness-desktop", MusicProvider::None))
    .await
    .expect("desktop announce");
  desktop
}

fn merged_title(harness: &Harness) -> Option<String> {
  harness.state().player.state_reply().state.track.and_then(|t| t.title)
}

async fn await_primary_ready(harness: &Harness) {
  let ready = harness
    .wait_for(
      |s| {
        s.authority
          .is_authoritative(CompanionAuthorityScope::NowPlayingMetadata)
          && s.player.state_reply().state.track.and_then(|t| t.title).as_deref() == Some("Phone Track")
      },
      CONVERGE,
    )
    .await;
  assert!(ready, "primary companion never became the merged source");
}

#[tokio::test]
async fn secondary_drop_leaves_primary_authority_intact() {
  let harness = Harness::start().await.expect("harness start");
  let _phone = connect_primary(&harness).await;
  let desktop = connect_secondary(&harness).await;
  await_primary_ready(&harness).await;

  drop(desktop);
  tokio::time::sleep(SETTLE).await;

  assert!(
    harness
      .state()
      .authority
      .is_authoritative(CompanionAuthorityScope::NowPlayingMetadata),
    "a secondary companion leaving dropped the primary's metadata authority"
  );
  assert!(
    harness
      .state()
      .authority
      .is_authoritative(CompanionAuthorityScope::NowPlayingPlayback),
    "a secondary companion leaving dropped the primary's playback authority"
  );
}

#[tokio::test]
async fn secondary_drop_leaves_primary_now_playing_intact() {
  let harness = Harness::start().await.expect("harness start");
  let _phone = connect_primary(&harness).await;
  let desktop = connect_secondary(&harness).await;
  await_primary_ready(&harness).await;

  drop(desktop);
  tokio::time::sleep(SETTLE).await;

  assert_eq!(
    merged_title(&harness).as_deref(),
    Some("Phone Track"),
    "a secondary companion leaving wiped the primary's now-playing state"
  );
}

#[tokio::test]
async fn secondary_drop_leaves_playback_targets_intact() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_primary(&harness).await;
  phone
    .player()
    .targets_changed(PlaybackTargets {
      targets: vec![PlaybackTarget {
        id: "speaker".into(),
        name: "Kitchen".into(),
        kind: PlaybackTargetKind::Speaker,
        is_active: true,
        volume_percent: Some(40),
      }],
    })
    .await
    .expect("targets");
  let visible = harness
    .wait_for(|s| s.playback_targets.current().targets.len() == 1, CONVERGE)
    .await;
  assert!(visible, "targets never became visible");

  let desktop = connect_secondary(&harness).await;
  await_primary_ready(&harness).await;

  drop(desktop);
  tokio::time::sleep(SETTLE).await;

  assert_eq!(
    harness.state().playback_targets.current().targets.len(),
    1,
    "a secondary companion leaving cleared the primary's playback targets"
  );
}

#[tokio::test]
async fn an_incapable_secondary_does_not_win_the_capability_snapshot() {
  let harness = Harness::start().await.expect("harness start");
  let _phone = connect_primary(&harness).await;
  let _desktop = connect_secondary(&harness).await;
  await_primary_ready(&harness).await;

  let snapshot = harness.state().capabilities.snapshot();
  assert_eq!(
    snapshot.gateway.map(|g| g.name).as_deref(),
    Some("harness-phone"),
    "the published capability snapshot did not follow the elected primary"
  );
  assert_eq!(
    snapshot.music_provider,
    MusicProvider::Spotify,
    "an incapable secondary clobbered the primary's music provider"
  );
}

#[tokio::test]
async fn primary_drop_still_clears_its_own_state() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_primary(&harness).await;
  let _desktop = connect_secondary(&harness).await;
  await_primary_ready(&harness).await;

  drop(phone);

  let cleared = harness
    .wait_for(
      |s| {
        !s.authority
          .is_authoritative(CompanionAuthorityScope::NowPlayingMetadata)
          && !s
            .authority
            .is_authoritative(CompanionAuthorityScope::NowPlayingPlayback)
          && s.player.state_reply().state.track.and_then(|t| t.title).as_deref() != Some("Phone Track")
      },
      CONVERGE,
    )
    .await;
  assert!(
    cleared,
    "the primary companion leaving left its authority and now-playing state behind"
  );
}

fn track(id: &str, title: &str) -> PlayerState {
  PlayerState {
    track: Some(MediaItem {
      persistent_id: Some(id.into()),
      title: Some(title.into()),
      duration_ms: Some(180_000),
      ..MediaItem::default()
    }),
    ..PlayerState::default()
  }
}

struct Serving {
  asked: tokio::sync::mpsc::UnboundedReceiver<()>,
  task: tokio::task::JoinHandle<()>,
}

impl Serving {
  async fn recv(&mut self) -> Option<()> {
    self.asked.recv().await
  }

  async fn stop(self) {
    self.task.abort();
    let _ = self.task.await;
  }
}

fn serve_snapshot_requests(gateway: &Gateway, state: PlayerState) -> Serving {
  let (asked_tx, asked_rx) = tokio::sync::mpsc::unbounded_channel();
  let mut events = gateway.events();
  let serving = gateway.clone();
  let task = tokio::spawn(async move {
    loop {
      let msg = match events.recv().await {
        Ok(msg) => msg,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
      };
      let BridgeToGatewayMsgData::Player(BridgeToGatewayPlayerMsg::SnapshotRequest(_)) = &msg.data else {
        continue;
      };
      let _ = serving
        .connection()
        .respond_to::<PlayerSnapshotRequest>(msg.id, PlayerSnapshotAck {})
        .await;
      let _ = serving.player().snapshot(state.clone()).await;
      let _ = asked_tx.send(());
    }
  });
  Serving { asked: asked_rx, task }
}

async fn connect_capable_secondary(harness: &Harness) -> (Gateway, Serving) {
  let desktop = harness.connect_android().await.expect("desktop connect");
  desktop
    .capabilities()
    .announce(caps("harness-capable-desktop", MusicProvider::Spotify))
    .await
    .expect("desktop announce");
  let asked = serve_snapshot_requests(&desktop, track("spotify:track:desktop", "Desktop Track"));
  (desktop, asked)
}

#[tokio::test]
async fn a_newly_primary_companion_is_asked_to_resend_its_state() {
  let harness = Harness::start().await.expect("harness start");
  let _phone = connect_primary(&harness).await;
  await_primary_ready(&harness).await;

  let (desktop, mut asked) = connect_capable_secondary(&harness).await;
  claim_now_playing(&desktop).await;

  let request = tokio::time::timeout(CONVERGE, asked.recv()).await;
  assert!(
    matches!(request, Ok(Some(()))),
    "the companion that took over authority was never asked to resend its state"
  );
}

#[tokio::test]
async fn a_resynced_companion_repopulates_now_playing() {
  let harness = Harness::start().await.expect("harness start");
  let _phone = connect_primary(&harness).await;
  await_primary_ready(&harness).await;

  let (desktop, _asked) = connect_capable_secondary(&harness).await;
  claim_now_playing(&desktop).await;

  let took_over = harness
    .wait_for(
      |s| s.player.state_reply().state.track.and_then(|t| t.title).as_deref() == Some("Desktop Track"),
      CONVERGE,
    )
    .await;
  assert!(
    took_over,
    "the new primary's re-pushed state never became the merged now-playing"
  );
}

#[tokio::test]
async fn losing_the_primary_asks_the_promoted_companion_to_resend() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_primary(&harness).await;
  let mut phone_asked = serve_snapshot_requests(&phone, track("spotify:track:phone", "Phone Track"));
  await_primary_ready(&harness).await;

  let (desktop, mut desktop_asked) = connect_capable_secondary(&harness).await;
  claim_now_playing(&desktop).await;
  assert!(
    matches!(tokio::time::timeout(CONVERGE, desktop_asked.recv()).await, Ok(Some(()))),
    "the desktop never took the merge over"
  );

  desktop_asked.stop().await;
  drop(desktop);

  assert!(
    matches!(tokio::time::timeout(CONVERGE, phone_asked.recv()).await, Ok(Some(()))),
    "the promoted companion was never asked to resend its state"
  );
  let restored = harness
    .wait_for(
      |s| s.player.state_reply().state.track.and_then(|t| t.title).as_deref() == Some("Phone Track"),
      CONVERGE,
    )
    .await;
  assert!(restored, "the promoted companion's re-pushed state never landed");
}

fn caps_claiming(name: &str, provider: MusicProvider, schemes: &[&str]) -> GatewayCapabilities {
  GatewayCapabilities {
    uri_schemes: schemes.iter().map(|s| s.to_string()).collect(),
    ..caps(name, provider)
  }
}

async fn next_player_command(
  inbound: &mut tokio::sync::broadcast::Receiver<libbridgething::gateway::BridgeToGatewayMsg>,
  within: Duration,
) -> Option<BridgeToGatewayPlayerMsg> {
  tokio::time::timeout(within, async {
    loop {
      match inbound.recv().await {
        Ok(msg) => {
          if let BridgeToGatewayMsgData::Player(player) = msg.data
            && !matches!(player, BridgeToGatewayPlayerMsg::SnapshotRequest(_))
          {
            return Some(player);
          }
        }
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
      }
    }
  })
  .await
  .ok()
  .flatten()
}

fn played_uri(command: Option<BridgeToGatewayPlayerMsg>) -> Option<String> {
  match command {
    Some(BridgeToGatewayPlayerMsg::Play(play)) => Some(play.uri),
    _ => None,
  }
}

async fn two_claiming_companions(harness: &Harness) -> (Gateway, Gateway) {
  let phone = harness.connect_android().await.expect("phone connect");
  phone
    .capabilities()
    .announce(caps_claiming("harness-phone", MusicProvider::Spotify, &["spotify"]))
    .await
    .expect("phone announce");
  claim_now_playing(&phone).await;
  phone.player().snapshot(phone_snapshot()).await.expect("phone snapshot");
  await_primary_ready(harness).await;

  let desktop = harness.connect_android().await.expect("desktop connect");
  desktop
    .capabilities()
    .announce(caps_claiming(
      "harness-desktop",
      MusicProvider::None,
      &["http", "https"],
    ))
    .await
    .expect("desktop announce");
  let announced = harness
    .wait_for(
      |s| {
        let schemes = s.capabilities.snapshot().uri_schemes;
        schemes.iter().any(|s| s == "https") && schemes.iter().any(|s| s == "spotify")
      },
      CONVERGE,
    )
    .await;
  assert!(
    announced,
    "both companions' schemes never reached the capability snapshot"
  );
  (phone, desktop)
}

#[tokio::test]
async fn a_play_goes_only_to_the_companion_that_claims_its_scheme() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, desktop) = two_claiming_companions(&harness).await;

  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();
  let client = harness.connect_command_client().await.expect("client connect");

  client
    .player()
    .play(libbridgething::client::PlayUri {
      uri: "https://radio.example/live".into(),
      context: None,
    })
    .await
    .expect("play a stream");
  assert_eq!(
    played_uri(next_player_command(&mut desktop_inbound, CONVERGE).await).as_deref(),
    Some("https://radio.example/live"),
    "the companion claiming https never got the play"
  );
  assert!(
    played_uri(next_player_command(&mut phone_inbound, SETTLE).await).is_none(),
    "the primary companion was handed a uri it does not claim"
  );

  client
    .player()
    .play(libbridgething::client::PlayUri {
      uri: "spotify:track:abc".into(),
      context: None,
    })
    .await
    .expect("play a spotify uri");
  assert_eq!(
    played_uri(next_player_command(&mut phone_inbound, CONVERGE).await).as_deref(),
    Some("spotify:track:abc")
  );
  assert!(
    next_player_command(&mut desktop_inbound, SETTLE).await.is_none(),
    "a spotify uri leaked to the companion that only claims http"
  );
}

#[tokio::test]
async fn a_play_on_another_companion_stops_the_one_holding_playback() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, desktop) = two_claiming_companions(&harness).await;

  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();
  let client = harness.connect_command_client().await.expect("client connect");

  client
    .player()
    .play(libbridgething::client::PlayUri {
      uri: "https://radio.example/live".into(),
      context: None,
    })
    .await
    .expect("play a stream");
  assert_eq!(
    played_uri(next_player_command(&mut desktop_inbound, CONVERGE).await).as_deref(),
    Some("https://radio.example/live"),
    "the companion claiming https never got the play"
  );
  assert!(
    matches!(
      next_player_command(&mut phone_inbound, CONVERGE).await,
      Some(BridgeToGatewayPlayerMsg::Pause)
    ),
    "playback is exclusive across companions too: the one that held the output has to be told to stop"
  );
}

#[tokio::test]
async fn a_play_on_the_companion_already_holding_playback_does_not_stop_it() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, _desktop) = two_claiming_companions(&harness).await;

  let mut phone_inbound = phone.events();
  let client = harness.connect_command_client().await.expect("client connect");

  client
    .player()
    .play(libbridgething::client::PlayUri {
      uri: "spotify:track:abc".into(),
      context: None,
    })
    .await
    .expect("play a spotify uri");
  assert_eq!(
    played_uri(next_player_command(&mut phone_inbound, CONVERGE).await).as_deref(),
    Some("spotify:track:abc"),
    "a companion does not hand off to itself; the play is the first thing it hears"
  );
}

#[tokio::test]
async fn transport_verbs_reach_only_the_companion_holding_playback() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_primary(&harness).await;
  let desktop = connect_secondary(&harness).await;
  await_primary_ready(&harness).await;

  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();
  let client = harness.connect_command_client().await.expect("client connect");
  client.player().pause().await.expect("pause");

  assert!(
    matches!(
      next_player_command(&mut phone_inbound, CONVERGE).await,
      Some(BridgeToGatewayPlayerMsg::Pause)
    ),
    "the companion holding playback never got the pause"
  );
  assert!(
    next_player_command(&mut desktop_inbound, SETTLE).await.is_none(),
    "a companion without playback authority was told to pause"
  );
}

fn voice(stream_id: Uuid, intent: &str, slots: NluSlots) -> VoiceDispatch {
  VoiceDispatch {
    stream_id,
    resolved: Box::new(NluResolvedIntent {
      intent: intent.into(),
      slots,
      transcript: intent.to_ascii_lowercase(),
      alternates: None,
    }),
    stage: None,
    asr_confidence: Some(0.9),
  }
}

fn catalog_play(stream_id: Uuid, uri: &str) -> VoiceDispatch {
  voice(
    stream_id,
    "PLAY",
    NluSlots {
      target: Some("mitski".into()),
      uri: Some(uri.into()),
      ..NluSlots::default()
    },
  )
}

async fn collect<T>(
  inbound: &mut tokio::sync::broadcast::Receiver<libbridgething::gateway::BridgeToGatewayMsg>,
  within: Duration,
  pick: impl Fn(BridgeToGatewayMsgData) -> Option<T>,
) -> Vec<T> {
  let mut hits = Vec::new();
  let _ = tokio::time::timeout(within, async {
    loop {
      match inbound.recv().await {
        Ok(msg) => hits.extend(pick(msg.data)),
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
      }
    }
  })
  .await;
  hits
}

fn play_of(data: BridgeToGatewayMsgData) -> Option<String> {
  match data {
    BridgeToGatewayMsgData::Player(BridgeToGatewayPlayerMsg::Play(play)) => Some(play.uri),
    _ => None,
  }
}

fn volume_of(data: BridgeToGatewayMsgData) -> Option<f32> {
  match data {
    BridgeToGatewayMsgData::Audio(BridgeToGatewayAudioMsg::SetVolume(set)) => Some(set.level),
    _ => None,
  }
}

fn invoke_of(data: BridgeToGatewayMsgData) -> Option<String> {
  match data {
    BridgeToGatewayMsgData::Notifications(BridgeToGatewayNotificationsMsg::InvokePositive(invoke)) => Some(invoke.id),
    _ => None,
  }
}

fn voice_outcome(data: BridgeToGatewayMsgData) -> Option<BridgeToGatewayVoiceMsg> {
  match data {
    BridgeToGatewayMsgData::Voice(
      voice @ (BridgeToGatewayVoiceMsg::Dispatched(_) | BridgeToGatewayVoiceMsg::DispatchFailed(_)),
    ) => Some(voice),
    _ => None,
  }
}

fn stream_open_of(data: BridgeToGatewayMsgData) -> Option<Uuid> {
  match data {
    BridgeToGatewayMsgData::Voice(BridgeToGatewayVoiceMsg::StreamOpen(open)) => Some(open.stream_id),
    _ => None,
  }
}

fn listening(caps: GatewayCapabilities) -> GatewayCapabilities {
  GatewayCapabilities {
    available: SurfaceAvailability {
      voice: true,
      ..caps.available
    },
    ..caps
  }
}

async fn next_failed_activity(
  activity: &mut (impl futures::Stream<Item = BridgeToClientVoiceMsgEvent> + Unpin),
  within: Duration,
) -> Option<VoiceActivity> {
  tokio::time::timeout(within, async {
    while let Some(event) = activity.next().await {
      if let BridgeToClientVoiceMsgEvent::Activity(a) = event
        && a.phase == VoicePhase::Failed
      {
        return Some(a);
      }
    }
    None
  })
  .await
  .ok()
  .flatten()
}

async fn claim(gateway: &Gateway, scope: CompanionAuthorityScope) {
  gateway
    .authority()
    .claim(AuthorityClaim {
      scope,
      app_bundle: Some("com.spotify.client".into()),
    })
    .await
    .expect("claim");
}

async fn two_spotify_companions(harness: &Harness, phone_claims: bool) -> (Gateway, Gateway) {
  let phone = harness.connect_android().await.expect("phone connect");
  phone
    .capabilities()
    .announce(caps_claiming("harness-phone", MusicProvider::Spotify, &["spotify"]))
    .await
    .expect("phone announce");
  if phone_claims {
    claim_now_playing(&phone).await;
    claim(&phone, CompanionAuthorityScope::Volume).await;
    phone.player().snapshot(phone_snapshot()).await.expect("phone snapshot");
    await_primary_ready(harness).await;
  }

  let desktop = harness.connect_android().await.expect("desktop connect");
  desktop
    .capabilities()
    .announce(listening(caps_claiming(
      "harness-desktop",
      MusicProvider::Spotify,
      &["spotify", "https"],
    )))
    .await
    .expect("desktop announce");
  let announced = harness
    .wait_for(
      |s| s.capabilities.snapshot().uri_schemes.iter().any(|s| s == "https"),
      CONVERGE,
    )
    .await;
  assert!(
    announced,
    "the desktop's announce never reached the capability snapshot"
  );
  (phone, desktop)
}

#[tokio::test]
async fn a_voice_catalog_play_reaches_only_the_companion_holding_playback() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, desktop) = two_spotify_companions(&harness, true).await;
  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();
  let mut outcome = desktop.events();
  let turn = harness.resolving_voice_turn(&desktop).await.expect("voice turn");

  desktop
    .voice()
    .dispatch(catalog_play(turn, "spotify:track:abc"))
    .await
    .expect("dispatch");

  let phone_plays = collect(&mut phone_inbound, SETTLE, play_of).await;
  let desktop_plays = collect(&mut desktop_inbound, SETTLE, play_of).await;
  assert_eq!(
    (phone_plays, desktop_plays),
    (vec!["spotify:track:abc".to_string()], vec![]),
    "a voice play reaches the one companion holding playback, never every companion that can play the scheme"
  );
  assert!(
    matches!(
      collect(&mut outcome, SETTLE, voice_outcome).await.first(),
      Some(BridgeToGatewayVoiceMsg::Dispatched(_))
    ),
    "the voice turn that did land was not reported as dispatched"
  );
}

#[tokio::test]
async fn one_turn_answered_by_two_companions_acts_once_and_refuses_the_second() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, desktop) = two_spotify_companions(&harness, false).await;
  phone
    .capabilities()
    .announce(listening(caps_claiming(
      "harness-phone",
      MusicProvider::Spotify,
      &["spotify"],
    )))
    .await
    .expect("phone announce");
  let phone_elected = harness
    .wait_for(
      |s| {
        s.capabilities
          .snapshot()
          .gateway
          .is_some_and(|g| g.name == "harness-phone")
      },
      CONVERGE,
    )
    .await;
  assert!(phone_elected, "the phone's voice announce never settled");

  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();
  let mut phone_outcome = phone.events();
  let mut desktop_outcome = desktop.events();
  let turn = harness.resolving_voice_turn(&phone).await.expect("voice turn");

  let (phone_voice, desktop_voice) = (phone.voice(), desktop.voice());
  let (from_phone, from_desktop) = tokio::join!(
    phone_voice.dispatch(catalog_play(turn, "spotify:track:abc")),
    desktop_voice.dispatch(catalog_play(turn, "spotify:track:abc")),
  );
  from_phone.expect("phone dispatch");
  from_desktop.expect("desktop dispatch");

  let plays = collect(&mut phone_inbound, SETTLE, play_of).await.len()
    + collect(&mut desktop_inbound, SETTLE, play_of).await.len();
  assert_eq!(plays, 1, "one spoken turn must reach the player exactly once");

  let outcomes: Vec<BridgeToGatewayVoiceMsg> = collect(&mut phone_outcome, SETTLE, voice_outcome)
    .await
    .into_iter()
    .chain(collect(&mut desktop_outcome, SETTLE, voice_outcome).await)
    .collect();
  let landed = outcomes
    .iter()
    .filter(|o| matches!(o, BridgeToGatewayVoiceMsg::Dispatched(d) if d.stream_id == turn))
    .count();
  let refused = outcomes
    .iter()
    .filter(|o| {
      matches!(o, BridgeToGatewayVoiceMsg::DispatchFailed(f)
        if f.stream_id == turn && f.code == VoiceDispatchErrorCode::StaleTurn)
    })
    .count();
  assert_eq!(
    (landed, refused),
    (1, 1),
    "the first answer lands and the second is told the turn is no longer waiting: {outcomes:?}"
  );
}

#[tokio::test]
async fn a_turn_streams_only_to_the_companion_that_listens() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, desktop) = two_spotify_companions(&harness, true).await;
  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();

  let turn = harness.resolving_voice_turn(&desktop).await.expect("voice turn");

  assert_eq!(
    collect(&mut desktop_inbound, SETTLE, stream_open_of).await,
    vec![turn],
    "the companion that announced voice never got the turn"
  );
  assert!(
    collect(&mut phone_inbound, SETTLE, stream_open_of).await.is_empty(),
    "the primary has no voice pipeline, so it must not be handed the audio"
  );
}

#[tokio::test]
async fn the_primary_listens_when_more_than_one_companion_can() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, desktop) = two_spotify_companions(&harness, true).await;
  phone
    .capabilities()
    .announce(listening(caps_claiming(
      "harness-phone",
      MusicProvider::Spotify,
      &["spotify"],
    )))
    .await
    .expect("phone announce");
  let phone_addr = harness.state().capabilities.primary_addr();
  let settled = harness
    .wait_for(|s| s.capabilities.voice_listener() == phone_addr, CONVERGE)
    .await;
  assert!(settled, "the voice pick never moved to the primary");
  let mut desktop_inbound = desktop.events();

  let turn = harness.resolving_voice_turn(&phone).await.expect("voice turn");

  assert!(
    !collect(&mut desktop_inbound, SETTLE, stream_open_of)
      .await
      .contains(&turn),
    "only the elected primary transcribes when two companions could"
  );
}

#[tokio::test]
async fn a_turn_with_no_companion_to_listen_fails_at_once() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_primary(&harness).await;
  let mut phone_inbound = phone.events();
  let client = harness.connect_command_client().await.expect("client connect");
  let mut activity = Box::pin(client.voice().events());

  client.voice().push_to_talk().await.expect("push to talk");

  let failed = next_failed_activity(&mut activity, CONVERGE)
    .await
    .expect("a turn nobody can transcribe never reported a failure");
  assert_eq!(failed.error.map(|e| e.code), Some(VoiceDispatchErrorCode::NoCompanion));
  assert!(
    collect(&mut phone_inbound, SETTLE, stream_open_of).await.is_empty(),
    "a companion without a voice pipeline was streamed audio it cannot use"
  );
}

#[tokio::test]
async fn a_listener_dropping_mid_capture_fails_the_turn_without_waiting_out_the_cap() {
  let harness = Harness::start().await.expect("harness start");
  let (_phone, desktop) = two_spotify_companions(&harness, true).await;
  let mut desktop_inbound = desktop.events();
  let client = harness.connect_command_client().await.expect("client connect");
  let mut activity = Box::pin(client.voice().events());

  client.voice().push_to_talk().await.expect("push to talk");
  let turn = collect(&mut desktop_inbound, SETTLE, stream_open_of).await;
  assert_eq!(turn.len(), 1, "the listener never got the turn");
  drop(desktop);

  let failed = next_failed_activity(&mut activity, CONVERGE)
    .await
    .expect("the turn hung after its listener left");
  assert_eq!(failed.stream_id, turn.first().copied());
  assert_eq!(failed.error.map(|e| e.code), Some(VoiceDispatchErrorCode::NoCompanion));
  assert!(!harness.state().mic.snapshot().await.capturing);
}

#[tokio::test]
async fn a_listener_dropping_while_resolving_fails_the_turn_without_waiting_out_the_deadline() {
  let harness = Harness::start().await.expect("harness start");
  let (_phone, desktop) = two_spotify_companions(&harness, true).await;
  let client = harness.connect_command_client().await.expect("client connect");
  let mut activity = Box::pin(client.voice().events());

  let turn = harness.resolving_voice_turn(&desktop).await.expect("voice turn");
  drop(desktop);

  let failed = next_failed_activity(&mut activity, CONVERGE)
    .await
    .expect("the turn waited on a listener that had already left");
  assert_eq!(failed.stream_id, Some(turn));
  assert_eq!(failed.error.map(|e| e.code), Some(VoiceDispatchErrorCode::NoCompanion));
  assert_eq!(harness.state().mic.snapshot().await.phase, VoicePhase::Idle);
}

#[tokio::test]
async fn a_voice_volume_change_reaches_only_the_companion_holding_volume() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, desktop) = two_spotify_companions(&harness, true).await;
  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();
  let turn = harness.resolving_voice_turn(&desktop).await.expect("voice turn");

  desktop
    .voice()
    .dispatch(voice(
      turn,
      "SET_VOLUME",
      NluSlots {
        level: Some(40),
        ..NluSlots::default()
      },
    ))
    .await
    .expect("dispatch");

  let phone_volumes = collect(&mut phone_inbound, SETTLE, volume_of).await;
  let desktop_volumes = collect(&mut desktop_inbound, SETTLE, volume_of).await;
  assert_eq!(
    (phone_volumes, desktop_volumes),
    (vec![0.4], vec![]),
    "a voice volume change reaches the companion holding the volume authority and nobody else"
  );
}

#[tokio::test]
async fn a_client_set_volume_reaches_only_the_companion_holding_volume() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, desktop) = two_spotify_companions(&harness, true).await;
  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();
  let client = harness.connect_command_client().await.expect("client connect");

  client
    .audio()
    .set_volume(libbridgething::client::SetVolume { level: 0.25 })
    .await
    .expect("set volume");

  let phone_volumes = collect(&mut phone_inbound, SETTLE, volume_of).await;
  let desktop_volumes = collect(&mut desktop_inbound, SETTLE, volume_of).await;
  assert_eq!(
    (phone_volumes, desktop_volumes),
    (vec![0.25], vec![]),
    "a webapp volume change reaches the companion holding the volume authority and nobody else"
  );
}

#[tokio::test]
async fn a_notification_action_goes_back_to_the_companion_that_posted_it() {
  let harness = Harness::start().await.expect("harness start");
  let (phone, desktop) = two_spotify_companions(&harness, true).await;
  desktop
    .notifications()
    .posted(Notification {
      id: "desktop-mail-1".into(),
      app: NotificationApp {
        bundle_id: "com.apple.mail".into(),
        display_name: Some("Mail".into()),
        icon_asset_id: None,
      },
      category: NotificationCategory::Email,
      title: Some("Lunch?".into()),
      subtitle: None,
      message: None,
      timestamp_unix_s: None,
      flags: NotificationFlags::default(),
      positive_action: Some(NotificationAction { label: "Reply".into() }),
      negative_action: None,
    })
    .await
    .expect("post");
  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();
  let client = harness.connect_command_client().await.expect("client connect");
  tokio::time::sleep(SETTLE).await;

  client
    .notifications()
    .invoke_positive(libbridgething::client::NotificationInvoke {
      id: "desktop-mail-1".into(),
    })
    .await
    .expect("invoke");

  let phone_invokes = collect(&mut phone_inbound, SETTLE, invoke_of).await;
  let desktop_invokes = collect(&mut desktop_inbound, SETTLE, invoke_of).await;
  assert_eq!(
    (phone_invokes, desktop_invokes),
    (vec![], vec!["desktop-mail-1".to_string()]),
    "a notification action belongs to the companion that posted the notification"
  );
}

fn geo_of(data: BridgeToGatewayMsgData) -> Option<BridgeToGatewayGeoMsg> {
  match data {
    BridgeToGatewayMsgData::Geo(geo @ (BridgeToGatewayGeoMsg::Watch(_) | BridgeToGatewayGeoMsg::Unwatch)) => Some(geo),
    _ => None,
  }
}

async fn announce_geo(gateway: &Gateway, name: &str) {
  let mut announce = caps_claiming(name, MusicProvider::Spotify, &["spotify"]);
  announce.available.geo = true;
  gateway.capabilities().announce(announce).await.expect("announce");
}

#[tokio::test]
async fn a_geo_watch_runs_on_one_companion_and_follows_the_primary() {
  let harness = Harness::start().await.expect("harness start");
  let phone = harness.connect_android().await.expect("phone connect");
  announce_geo(&phone, "harness-phone").await;
  claim_now_playing(&phone).await;
  phone.player().snapshot(phone_snapshot()).await.expect("phone snapshot");
  await_primary_ready(&harness).await;
  let desktop = harness.connect_android().await.expect("desktop connect");
  announce_geo(&desktop, "harness-desktop").await;
  harness
    .activate_webapp_declaring(&["geo"])
    .await
    .expect("activate a geo webapp");
  tokio::time::sleep(SETTLE).await;

  let mut phone_inbound = phone.events();
  let mut desktop_inbound = desktop.events();
  let client = harness.connect_command_client().await.expect("client connect");
  client
    .geo()
    .watch(libbridgething::client::GeoWatch {
      accuracy: GeoAccuracy::Fine,
      min_interval_ms: 1000,
    })
    .await
    .expect("geo watch");

  let phone_geo = collect(&mut phone_inbound, SETTLE, geo_of).await;
  let desktop_geo = collect(&mut desktop_inbound, SETTLE, geo_of).await;
  assert!(
    matches!(phone_geo.as_slice(), [BridgeToGatewayGeoMsg::Watch(_)]) && desktop_geo.is_empty(),
    "one location source runs at a time, on the primary; phone got {phone_geo:?}, desktop got {desktop_geo:?}"
  );

  drop(phone);
  let promoted = tokio::time::timeout(CONVERGE, async {
    loop {
      match desktop_inbound.recv().await {
        Ok(msg) => {
          if let Some(geo) = geo_of(msg.data) {
            return Some(geo);
          }
        }
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
      }
    }
  })
  .await
  .ok()
  .flatten();
  assert!(
    matches!(promoted, Some(BridgeToGatewayGeoMsg::Watch(_))),
    "the companion left holding the primary never took the running watch over: {promoted:?}"
  );
}
