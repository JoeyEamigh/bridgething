use std::time::Duration;

use bridgething_test_harness::{Harness, MockWsClient};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(3);
const STOCK_RESET: &str = r#"{"type":"device","action":"factory_reset"}"#;

async fn ask_for_reset(client: &mut MockWsClient) -> Uuid {
  ask_for_reset_as(client, "request").await
}

async fn ask_for_reset_as(client: &mut MockWsClient, kind: &str) -> Uuid {
  let id = Uuid::now_v7();
  client
    .send_text(format!(
      r#"{{"id":"{id}","meta":{{"kind":"{kind}"}},"data":{{"type":"system","data":{{"event":"factoryReset"}}}}}}"#
    ))
    .await
    .expect("send factory reset");
  id
}

async fn answer_to(client: &mut MockWsClient, id: Uuid, within: Duration) -> Option<String> {
  tokio::time::timeout(within, async {
    while let Some(text) = client.recv().await {
      if text.contains(&id.to_string()) {
        return Some(text);
      }
    }
    None
  })
  .await
  .ok()
  .flatten()
}

async fn refusal(client: &mut MockWsClient, id: Uuid, refusal: &str) -> String {
  let answer = answer_to(client, id, WAIT)
    .await
    .expect("a reset that did not happen must be answered");
  assert!(
    answer.contains("\"factoryResetRefused\"") && answer.contains(&format!("\"refusal\":\"{refusal}\"")),
    "the refusal is the reset's own error, kind {refusal}: {answer}"
  );
  answer
}

async fn restarted(harness: &Harness) {
  assert!(
    harness.wait_for(|_| harness.reset_rig().reboots() > 0, WAIT).await,
    "the reset never restarted the device"
  );
}

#[tokio::test]
async fn a_factory_reset_arms_the_wipe_restarts_and_says_it_started() {
  let harness = Harness::start().await.expect("harness start");
  let mut client = harness.connect_modern_client().await.expect("modern client");

  let id = ask_for_reset(&mut client).await;

  let answer = answer_to(&mut client, id, WAIT)
    .await
    .expect("a reset that is underway is answered");
  assert!(answer.contains("\"factoryResetStarted\""), "got {answer}");
  restarted(&harness).await;
  let rig = harness.reset_rig();
  assert!(rig.is_armed(), "the wipe is armed for the restart");
  assert_eq!((rig.arms(), rig.reboots()), (1, 1));
}

#[tokio::test]
async fn a_stock_factory_reset_runs_the_same_reset() {
  let harness = Harness::start().await.expect("harness start");
  let mut stock = harness.connect_stock_client().await.expect("stock client");

  stock.send_text(STOCK_RESET).await.expect("send stock factory reset");

  restarted(&harness).await;
  let rig = harness.reset_rig();
  assert!(rig.is_armed());
  assert_eq!((rig.arms(), rig.reboots()), (1, 1));
}

#[tokio::test]
async fn an_image_without_the_wipe_step_refuses_the_reset_and_arms_nothing() {
  let harness = Harness::start().await.expect("harness start");
  harness.reset_rig().remove_wipe_step();
  let mut client = harness.connect_modern_client().await.expect("modern client");

  for kind in ["command", "request"] {
    let id = ask_for_reset_as(&mut client, kind).await;

    let answer = refusal(&mut client, id, "noWipeStep").await;
    assert!(
      answer.contains("this system image cannot erase the device"),
      "the refusal to a {kind} says why: {answer}"
    );
  }
  let rig = harness.reset_rig();
  assert!(!rig.is_armed(), "nothing would consume the flag, so it is never set");
  assert_eq!((rig.arms(), rig.reboots()), (0, 0));
}

#[tokio::test]
async fn a_stock_reset_on_an_image_without_the_wipe_step_arms_nothing() {
  let harness = Harness::start().await.expect("harness start");
  harness.reset_rig().remove_wipe_step();
  let mut stock = harness.connect_stock_client().await.expect("stock client");

  stock.send_text(STOCK_RESET).await.expect("first stock factory reset");
  stock.send_text(STOCK_RESET).await.expect("second stock factory reset");

  assert!(
    harness.wait_for(|_| harness.reset_rig().probes() == 2, WAIT).await,
    "both requests reach the reset"
  );
  let rig = harness.reset_rig();
  assert!(!rig.is_armed());
  assert_eq!((rig.arms(), rig.reboots()), (0, 0));
}

#[tokio::test]
async fn a_reset_whose_restart_fails_is_called_off_and_reported() {
  let harness = Harness::start().await.expect("harness start");
  harness.reset_rig().fail_reboots();
  let mut client = harness.connect_modern_client().await.expect("modern client");

  let id = ask_for_reset(&mut client).await;

  let answer = refusal(&mut client, id, "failed").await;
  assert!(
    answer.contains("the device did not restart, nothing was erased"),
    "the refusal says why: {answer}"
  );
  let rig = harness.reset_rig();
  assert_eq!((rig.arms(), rig.reboots()), (1, 1));
  assert!(!rig.is_armed(), "a reset that did not restart must not stay armed");
}

#[tokio::test]
async fn a_reset_that_cannot_be_called_off_warns_that_the_next_restart_erases_the_device() {
  let harness = Harness::start().await.expect("harness start");
  harness.reset_rig().fail_reboots();
  harness.reset_rig().fail_disarms();
  let mut client = harness.connect_modern_client().await.expect("modern client");

  let id = ask_for_reset(&mut client).await;

  let answer = refusal(&mut client, id, "stillArmed").await;
  assert!(
    answer.contains("the reset is still armed") && answer.contains("the next restart erases it"),
    "the refusal says the wipe is still pending: {answer}"
  );
  assert!(harness.reset_rig().is_armed());
}

#[tokio::test]
async fn a_failed_reset_can_be_asked_for_again() {
  let harness = Harness::start().await.expect("harness start");
  harness.reset_rig().remove_wipe_step();
  let mut client = harness.connect_modern_client().await.expect("modern client");
  let refused = ask_for_reset(&mut client).await;
  refusal(&mut client, refused, "noWipeStep").await;

  let state = harness.state();
  assert!(state.factory_reset().await.is_err());
  assert_eq!(harness.reset_rig().probes(), 2, "a refusal does not latch");
}

#[tokio::test]
async fn a_reset_asked_for_again_while_underway_asks_for_the_restart_again() {
  let harness = Harness::start().await.expect("harness start");
  let state = harness.state();

  let (first, second) = tokio::join!(state.factory_reset(), state.factory_reset());
  first.expect("first request");
  second.expect("second request");
  state.factory_reset().await.expect("late request");

  let rig = harness.reset_rig();
  assert!(rig.is_armed());
  assert_eq!(
    (rig.probes(), rig.arms(), rig.reboots()),
    (1, 1, 3),
    "the wipe is armed once and every request asks for the restart that has not happened yet"
  );
}

#[tokio::test]
async fn a_repeated_reset_whose_restart_fails_is_called_off_and_reported() {
  let harness = Harness::start().await.expect("harness start");
  let mut client = harness.connect_modern_client().await.expect("modern client");
  let started = ask_for_reset(&mut client).await;
  let answer = answer_to(&mut client, started, WAIT)
    .await
    .expect("the first reset is answered");
  assert!(answer.contains("\"factoryResetStarted\""), "got {answer}");
  harness.reset_rig().fail_reboots();

  let again = ask_for_reset(&mut client).await;

  let answer = refusal(&mut client, again, "failed").await;
  assert!(
    answer.contains("the device did not restart, nothing was erased"),
    "the refusal says why: {answer}"
  );
  let rig = harness.reset_rig();
  assert_eq!((rig.arms(), rig.reboots()), (1, 2));
  assert!(
    !rig.is_armed(),
    "a reset that never restarted the device must not stay armed"
  );
}

#[tokio::test]
async fn the_started_answer_reaches_the_page_before_the_daemon_exits() {
  let harness = Harness::start().await.expect("harness start");
  harness.stop_daemon_on_reboot();
  let mut client = harness.connect_modern_client().await.expect("modern client");

  let id = ask_for_reset(&mut client).await;

  let (frames, closed) = tokio::time::timeout(WAIT, client.recv_until_closed())
    .await
    .expect("the daemon closes the page's socket as it exits");
  let answer = frames
    .iter()
    .find(|frame| frame.contains(&id.to_string()))
    .expect("the answer is written before the socket closes");
  assert!(answer.contains("\"factoryResetStarted\""), "got {answer}");
  assert_eq!(closed, Some((1000, "bye".to_string())));
  assert!(harness.wait_for(|_| harness.daemon_exited(), WAIT).await);
}

#[tokio::test]
async fn a_reset_left_armed_by_an_earlier_boot_is_called_off_at_startup() {
  let harness = Harness::start().await.expect("harness start");
  harness.reset_rig().leave_armed();

  let harness = harness.restart().await.expect("restart");

  let rig = harness.reset_rig();
  assert!(
    !rig.is_armed(),
    "a flag no boot step consumed must not wait for a later image to erase the device"
  );
  assert_eq!((rig.arms(), rig.reboots()), (0, 0));
}
