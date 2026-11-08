use std::time::Duration;

use bridgething_gateway::Gateway;
use bridgething_test_harness::Harness;
use libbridgething::{
  DimSettings, DisplaySettings, HardwareError, PeerCompanionStatus,
  gateway::{BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewaySystemMsg, HostState, HostWatch},
};
use tokio::sync::broadcast;

const SETTLE: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(400);
const PAST_TIMER: Duration = Duration::from_secs(17);
const WAKE_WINDOW: Duration = Duration::from_secs(30);

fn dim(after_s: Option<u32>) -> DimSettings {
  DimSettings { after_s, level: 0.3 }
}

async fn try_dim(harness: &Harness, settings: DimSettings) -> Result<(), HardwareError> {
  let display = &harness.state().display;
  display
    .set_settings(DisplaySettings {
      dim: settings,
      ..display.settings()
    })
    .await
    .expect("the display actor answers")
}

async fn set_dim(harness: &Harness, settings: DimSettings) {
  try_dim(harness, settings).await.expect("a setting the device honors");
}

async fn report(gateway: &Gateway, away: Option<bool>, idle: bool) {
  gateway
    .system()
    .host_state(HostState { away, idle })
    .await
    .expect("host state");
}

async fn dimmed(harness: &Harness, want: bool, within: Duration) -> bool {
  harness
    .wait_for(|state| state.display.snapshot().brightness.dimmed == want, within)
    .await
}

async fn stays(harness: &Harness, want: bool, window: Duration) -> bool {
  !harness
    .wait_for(|state| state.display.snapshot().brightness.dimmed != want, window)
    .await
}

async fn link_gone(harness: &Harness) -> bool {
  harness
    .wait_for(
      |state| {
        state
          .peers
          .snapshot()
          .peers
          .values()
          .all(|peer| peer.companion == PeerCompanionStatus::None)
      },
      SETTLE,
    )
    .await
}

async fn next_watch(inbound: &mut broadcast::Receiver<BridgeToGatewayMsg>, within: Duration) -> Option<HostWatch> {
  tokio::time::timeout(within, async {
    loop {
      match inbound.recv().await {
        Ok(BridgeToGatewayMsg {
          data: BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::HostWatch(watch)),
          ..
        }) => return Some(watch),
        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
        Err(broadcast::error::RecvError::Closed) => return None,
      }
    }
  })
  .await
  .ok()
  .flatten()
}

#[tokio::test]
async fn a_fresh_device_never_dims_and_asks_nothing_of_a_computer() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");
  let reply = client.hardware().state_get().await.expect("state get");
  assert_eq!(reply.state.dim, DimSettings::default());
  assert!(!reply.state.brightness.dimmed);

  let (_gateway, mut inbound) = harness.connect_android_subscribed().await.expect("gateway");
  assert_eq!(
    next_watch(&mut inbound, QUIET).await,
    None,
    "without a timer no watch is sent"
  );
}

#[tokio::test]
async fn dim_settings_are_kept_across_a_restart() {
  let harness = Harness::start().await.expect("harness start");
  let chosen = DimSettings {
    after_s: Some(900),
    level: 0.0,
  };
  set_dim(&harness, chosen).await;

  let harness = harness.restart().await.expect("restart");
  let reader = harness.connect_command_client().await.expect("command client");
  let stored = reader.hardware().state_get().await.expect("state get");
  assert_eq!(stored.state.dim, chosen);
}

#[tokio::test]
async fn a_setting_the_device_cannot_honor_changes_nothing() {
  let harness = Harness::start().await.expect("harness start");
  let kept = dim(Some(60));
  set_dim(&harness, kept).await;

  for (bad, error) in [
    (dim(Some(14)), HardwareError::DimTimerOutOfRange),
    (dim(Some(86_401)), HardwareError::DimTimerOutOfRange),
    (DimSettings { level: 1.5, ..kept }, HardwareError::LevelOutOfRange),
  ] {
    assert_eq!(try_dim(&harness, bad).await, Err(error));
  }
  assert_eq!(harness.state().display.snapshot().dim, kept);
}

#[tokio::test]
async fn a_computer_hears_the_watch_when_it_connects_while_a_timer_is_set_and_whenever_the_timer_changes() {
  let harness = Harness::start().await.expect("harness start");
  let (_early, mut early) = harness.connect_android_subscribed().await.expect("gateway");

  set_dim(&harness, dim(Some(60))).await;
  assert_eq!(
    next_watch(&mut early, SETTLE).await,
    Some(HostWatch { idle_after_s: Some(60) }),
    "a linked computer hears the change"
  );

  let (_late, mut late) = harness.connect_android_subscribed().await.expect("gateway");
  assert_eq!(
    next_watch(&mut late, SETTLE).await,
    Some(HostWatch { idle_after_s: Some(60) }),
    "a computer that connects later hears the watch on link-up"
  );

  set_dim(&harness, dim(None)).await;
  for inbound in [&mut early, &mut late] {
    assert_eq!(
      next_watch(inbound, SETTLE).await,
      Some(HostWatch { idle_after_s: None }),
      "clearing the timer stops idle reports"
    );
  }

  let (_last, mut last) = harness.connect_android_subscribed().await.expect("gateway");
  assert_eq!(next_watch(&mut last, QUIET).await, None);
}

#[tokio::test]
async fn a_computer_dims_the_device_only_while_it_follows() {
  let harness = Harness::start().await.expect("harness start");
  let (gateway, _inbound) = harness.connect_android_subscribed().await.expect("gateway");

  report(&gateway, None, false).await;
  assert!(
    stays(&harness, false, QUIET).await,
    "a computer that does not follow leaves the screen alone"
  );

  report(&gateway, Some(true), false).await;
  assert!(
    dimmed(&harness, true, SETTLE).await,
    "a following computer that is away dims"
  );

  report(&gateway, Some(false), false).await;
  assert!(dimmed(&harness, false, SETTLE).await, "the computer coming back undims");

  report(&gateway, Some(true), false).await;
  assert!(dimmed(&harness, true, SETTLE).await);
  report(&gateway, None, false).await;
  assert!(
    dimmed(&harness, false, SETTLE).await,
    "a computer that stops following gives the screen back"
  );
}

#[tokio::test]
async fn every_report_is_acked_once_the_screen_already_follows_it() {
  let harness = Harness::start().await.expect("harness start");
  let (gateway, _inbound) = harness.connect_android_subscribed().await.expect("gateway");
  let link_up = harness
    .wait_for(
      |state| {
        state
          .peers
          .snapshot()
          .peers
          .values()
          .any(|peer| peer.companion != PeerCompanionStatus::None)
      },
      SETTLE,
    )
    .await;
  assert!(link_up, "the gateway link came up");

  for away in [true, false, true] {
    gateway
      .system()
      .host_state(HostState {
        away: Some(away),
        idle: false,
      })
      .await
      .expect("the device acks the report");
    assert_eq!(
      harness.state().display.snapshot().brightness.dimmed,
      away,
      "the ack came before the screen followed the report"
    );
  }
}

async fn held(harness: &Harness) {
  let (gateway, inbound) = harness.connect_android_subscribed().await.expect("gateway");
  report(&gateway, Some(true), false).await;
  assert!(dimmed(harness, true, SETTLE).await);
  drop((gateway, inbound));
  assert!(link_gone(harness).await, "the daemon saw the link go");
  assert!(
    stays(harness, true, QUIET).await,
    "a sleeping computer going away does not light the screen"
  );
}

#[tokio::test]
async fn a_computer_that_drops_its_link_while_away_leaves_the_screen_dimmed() {
  let harness = Harness::start().await.expect("harness start");
  held(&harness).await;

  let touched = tokio::time::Instant::now();
  harness.state().display.note_input().await;
  assert!(dimmed(&harness, false, SETTLE).await, "a tap lights the screen");
  assert!(
    dimmed(&harness, true, WAKE_WINDOW + SETTLE).await,
    "the computer is still away, so the screen dims again"
  );
  assert!(
    touched.elapsed() >= WAKE_WINDOW - Duration::from_secs(1),
    "the screen stayed lit for the wake window, redimmed after {:?}",
    touched.elapsed()
  );

  let (back, _inbound) = harness.connect_android_subscribed().await.expect("gateway");
  report(&back, Some(false), false).await;
  assert!(
    dimmed(&harness, false, SETTLE).await,
    "a computer reporting in use releases the hold"
  );
}

#[tokio::test]
async fn a_computer_that_comes_back_no_longer_following_releases_the_hold() {
  let harness = Harness::start().await.expect("harness start");
  held(&harness).await;

  let (back, _inbound) = harness.connect_android_subscribed().await.expect("gateway");
  report(&back, None, false).await;
  assert!(
    dimmed(&harness, false, SETTLE).await,
    "a computer that is back and does not want dimming lights the screen"
  );
  assert!(stays(&harness, false, QUIET).await);
}

#[tokio::test]
async fn a_computer_that_drops_its_link_while_in_use_leaves_the_screen_to_the_timer() {
  let harness = Harness::start().await.expect("harness start");
  set_dim(&harness, dim(Some(15))).await;

  let (gateway, inbound) = harness.connect_android_subscribed().await.expect("gateway");
  report(&gateway, Some(false), false).await;
  assert!(
    stays(&harness, false, PAST_TIMER).await,
    "the computer in use holds the timer off"
  );
  drop((gateway, inbound));
  assert!(
    dimmed(&harness, true, SETTLE).await,
    "with the computer gone the device timer, long overdue, dims the screen"
  );
}

#[tokio::test]
async fn a_computer_in_use_keeps_the_timer_from_dimming() {
  let harness = Harness::start().await.expect("harness start");
  set_dim(&harness, dim(Some(15))).await;
  let (gateway, _inbound) = harness.connect_android_subscribed().await.expect("gateway");
  report(&gateway, None, false).await;

  assert!(
    stays(&harness, false, PAST_TIMER).await,
    "the computer reporting input counts as activity past the timer"
  );
  report(&gateway, None, true).await;
  assert!(
    dimmed(&harness, true, SETTLE).await,
    "an idle computer lets the timer dim"
  );
}

#[tokio::test]
async fn a_report_sent_the_moment_a_computer_connects_still_counts() {
  let harness = Harness::start().await.expect("harness start");
  for _ in 0..5 {
    let (gateway, _inbound) = harness.connect_android_subscribed().await.expect("gateway");
    gateway
      .system()
      .host_state(HostState {
        away: Some(true),
        idle: false,
      })
      .await
      .expect("the device acks a report that races its own link-up");
    assert!(harness.state().display.snapshot().brightness.dimmed);
    drop(gateway);
    assert!(link_gone(&harness).await, "the link went down");
    report_back(&harness).await;
  }
}

async fn report_back(harness: &Harness) {
  let (gateway, _inbound) = harness.connect_android_subscribed().await.expect("gateway");
  report(&gateway, Some(false), false).await;
  assert!(dimmed(harness, false, SETTLE).await);
  drop(gateway);
  assert!(link_gone(harness).await);
}
