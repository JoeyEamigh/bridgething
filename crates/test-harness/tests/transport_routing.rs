use std::time::Duration;

use bridgething::Iap2TransportCommand;
use bridgething_gateway::Gateway;
use bridgething_test_harness::Harness;
use libbridgething::{
  GatewayCapabilities, GatewayInfo, NluResolvedIntent, NluSlots, PlayerError, SurfaceAvailability,
  VoiceDispatchErrorCode, VoiceDispatchTarget,
  client::{BridgeToClientMsgData, BridgeToClientPlayerMsg},
  gateway::{
    BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewayPlayerMsg, BridgeToGatewayVoiceMsg, VoiceDispatch,
  },
};
use tokio::sync::broadcast;
use uuid::Uuid;

const CONVERGE: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(600);

fn desktop_caps() -> GatewayCapabilities {
  GatewayCapabilities {
    gateway: GatewayInfo {
      address: String::new(),
      name: "harness-desktop".into(),
      os_name: "macos".into(),
      app_name: "harness-desktop".into(),
      app_version: "0.0.0".into(),
      adapter_version: "harness".into(),
      lib_version: "0.0.0".into(),
      libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
    },
    available: SurfaceAvailability {
      voice: true,
      ..Default::default()
    },
    ..Default::default()
  }
}

async fn connect_desktop(harness: &Harness) -> Gateway {
  let desktop = harness.connect_android().await.expect("desktop connect");
  desktop
    .capabilities()
    .announce(desktop_caps())
    .await
    .expect("desktop announce");
  let announced = harness
    .wait_for(|s| s.capabilities.snapshot().gateway.is_some(), CONVERGE)
    .await;
  assert!(announced, "the desktop's capabilities never landed");
  desktop
}

fn play(stream_id: Uuid, slots: NluSlots) -> VoiceDispatch {
  VoiceDispatch {
    stream_id,
    resolved: Box::new(NluResolvedIntent {
      intent: "PLAY".into(),
      slots,
      transcript: "play".into(),
      alternates: None,
    }),
    stage: None,
    asr_confidence: Some(0.9),
  }
}

fn bare_play(stream_id: Uuid) -> VoiceDispatch {
  play(stream_id, NluSlots::default())
}

async fn next_matching<T>(
  inbound: &mut broadcast::Receiver<BridgeToGatewayMsg>,
  within: Duration,
  pick: impl Fn(BridgeToGatewayMsgData) -> Option<T>,
) -> Option<T> {
  tokio::time::timeout(within, async {
    loop {
      match inbound.recv().await {
        Ok(msg) => {
          if let Some(hit) = pick(msg.data) {
            return Some(hit);
          }
        }
        Err(broadcast::error::RecvError::Lagged(_)) => continue,
        Err(broadcast::error::RecvError::Closed) => return None,
      }
    }
  })
  .await
  .ok()
  .flatten()
}

fn voice_outcome(data: BridgeToGatewayMsgData) -> Option<BridgeToGatewayVoiceMsg> {
  match data {
    BridgeToGatewayMsgData::Voice(
      voice @ (BridgeToGatewayVoiceMsg::Dispatched(_) | BridgeToGatewayVoiceMsg::DispatchFailed(_)),
    ) => Some(voice),
    _ => None,
  }
}

fn player_command(data: BridgeToGatewayMsgData) -> Option<BridgeToGatewayPlayerMsg> {
  match data {
    BridgeToGatewayMsgData::Player(player) if !matches!(player, BridgeToGatewayPlayerMsg::SnapshotRequest(_)) => {
      Some(player)
    }
    _ => None,
  }
}

#[tokio::test]
async fn voice_play_over_a_companion_only_link_reaches_the_companion() {
  let harness = Harness::start().await.expect("harness start");
  let mut iap2_outbound = harness.observe_iap2_outbound();
  let desktop = connect_desktop(&harness).await;
  let mut player_inbound = desktop.events();
  let mut voice_inbound = desktop.events();
  let turn = harness.resolving_voice_turn(&desktop).await.expect("voice turn");

  desktop.voice().dispatch(bare_play(turn)).await.expect("dispatch");

  assert!(
    matches!(
      next_matching(&mut player_inbound, CONVERGE, player_command).await,
      Some(BridgeToGatewayPlayerMsg::Resume)
    ),
    "a bare PLAY with no iPhone attached never reached the companion that is connected"
  );
  match next_matching(&mut voice_inbound, CONVERGE, voice_outcome).await {
    Some(BridgeToGatewayVoiceMsg::Dispatched(dispatched)) => {
      assert_eq!(dispatched.target, VoiceDispatchTarget::Playback);
    }
    other => panic!("expected a Playback dispatch, got {other:?}"),
  }
  let pulses = iap2_outbound.collect_for(SETTLE).await;
  assert!(
    !pulses.iter().any(|c| matches!(c, Iap2TransportCommand::Hid(_))),
    "a PLAY was pressed into an iAP2 link that does not exist: {pulses:?}"
  );
}

#[tokio::test]
async fn voice_play_that_nothing_can_carry_answers_a_failure() {
  let harness = Harness::start().await.expect("harness start");
  let desktop = connect_desktop(&harness).await;
  let mut voice_inbound = desktop.events();
  let turn = harness.resolving_voice_turn(&desktop).await.expect("voice turn");

  let unclaimed = NluSlots {
    target: Some("mitski".into()),
    uri: Some("spotify:track:abc".into()),
    ..NluSlots::default()
  };
  desktop.voice().dispatch(play(turn, unclaimed)).await.expect("dispatch");

  match next_matching(&mut voice_inbound, CONVERGE, voice_outcome).await {
    Some(BridgeToGatewayVoiceMsg::DispatchFailed(failed)) => {
      assert_eq!(failed.code, VoiceDispatchErrorCode::PlaybackFailed);
      assert_eq!(failed.intent, "PLAY");
      assert_eq!(failed.stream_id, turn);
    }
    other => panic!("a PLAY that no companion can carry must fail, got {other:?}"),
  }
}

#[tokio::test]
async fn a_dispatch_for_a_turn_that_is_not_resolving_is_refused() {
  let harness = Harness::start().await.expect("harness start");
  let mut iap2_outbound = harness.observe_iap2_outbound();
  let desktop = connect_desktop(&harness).await;
  let mut player_inbound = desktop.events();
  let mut voice_inbound = desktop.events();
  let stray = Uuid::now_v7();

  desktop.voice().dispatch(bare_play(stray)).await.expect("dispatch");

  match next_matching(&mut voice_inbound, CONVERGE, voice_outcome).await {
    Some(BridgeToGatewayVoiceMsg::DispatchFailed(failed)) => {
      assert_eq!(failed.code, VoiceDispatchErrorCode::StaleTurn);
      assert_eq!(failed.stream_id, stray);
    }
    other => panic!("a dispatch naming no open turn must be refused, got {other:?}"),
  }
  assert!(
    next_matching(&mut player_inbound, SETTLE, player_command)
      .await
      .is_none(),
    "a refused dispatch still moved the player"
  );
  assert!(
    iap2_outbound.collect_for(SETTLE).await.is_empty(),
    "a refused dispatch still pressed a key on the phone"
  );
}

#[tokio::test]
async fn an_identified_iphone_wins_over_a_companion_without_playback() {
  let harness = Harness::start().await.expect("harness start");
  let mut iap2_outbound = harness.observe_iap2_outbound();
  let desktop = connect_desktop(&harness).await;
  let mut desktop_inbound = desktop.events();
  let iphone = harness.iap2_peer();
  harness.identify_iphone(iphone).await.expect("identify iphone");

  let client = harness.connect_command_client().await.expect("client connect");
  client.player().pause().await.expect("pause");

  let pressed = iap2_outbound
    .wait_for(CONVERGE, |c| matches!(c, Iap2TransportCommand::Hid(_)))
    .await;
  assert!(pressed.is_some(), "the identified iPhone never got the pause press");
  assert!(
    next_matching(&mut desktop_inbound, SETTLE, player_command)
      .await
      .is_none(),
    "a companion that never claimed playback was told to pause while an iPhone owns it"
  );
}

#[tokio::test]
async fn a_transport_verb_with_no_phone_answers_no_gateway() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("client connect");
  let mut replies = client.events();

  client.player().pause().await.expect("pause");

  let answered = tokio::time::timeout(CONVERGE, async {
    loop {
      match replies.recv().await {
        Ok(msg) => {
          if let BridgeToClientMsgData::Player(BridgeToClientPlayerMsg::ErrorReply(reply)) = msg.data {
            return Some(reply.error);
          }
        }
        Err(broadcast::error::RecvError::Lagged(_)) => continue,
        Err(broadcast::error::RecvError::Closed) => return None,
      }
    }
  })
  .await
  .ok()
  .flatten();
  assert_eq!(answered, Some(PlayerError::NoGateway));
}
