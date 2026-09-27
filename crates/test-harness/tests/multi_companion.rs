use std::time::Duration;

use bridgething_gateway::Gateway;
use bridgething_test_harness::Harness;
use libbridgething::{
  CompanionAuthorityScope, GatewayCapabilities, GatewayInfo, MediaItem, MusicProvider, PlaybackTarget,
  PlaybackTargetKind, PlayerState,
  gateway::{
    AuthorityClaim, BridgeToGatewayMsgData, BridgeToGatewayPlayerMsg, PlaybackTargets, PlayerSnapshotAck,
    PlayerSnapshotRequest,
  },
};

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
