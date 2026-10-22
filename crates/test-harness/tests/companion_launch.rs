use std::time::Duration;

use bridgething_test_harness::Harness;
use futures::StreamExt;
use libbridgething::{
  client::{BridgeToClientSystemMsgEvent, CompanionLaunchSet},
  gateway::BridgeToGatewaySystemMsgEvent,
};

const EVENT_WAIT: Duration = Duration::from_secs(3);

#[tokio::test]
async fn a_fresh_device_opens_the_companion_and_keeps_a_change_across_a_restart() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");

  let initial = client.system().companion_launch_get().await.expect("launch get");
  assert!(
    initial.enabled,
    "a device that has never been told otherwise opens the companion"
  );
  let announced = client.system().version_request().await.expect("version");
  assert!(
    announced.companion_launch,
    "the meta every host already reads carries the setting"
  );

  client
    .system()
    .companion_launch_set(CompanionLaunchSet { enabled: false })
    .await
    .expect("launch set");

  let reader = harness.connect_command_client().await.expect("second client");
  loop {
    if !reader
      .system()
      .companion_launch_get()
      .await
      .expect("launch get")
      .enabled
    {
      break;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
  }
  assert!(!harness.state().meta.companion_launch());

  let harness = harness.restart().await.expect("restart");
  assert!(
    !harness.state().meta.companion_launch(),
    "the choice outlives the daemon"
  );
  let client = harness.connect_command_client().await.expect("command client");
  let announced = client.system().version_request().await.expect("version");
  assert!(!announced.companion_launch);
}

#[tokio::test]
async fn a_change_on_the_device_reaches_clients_and_the_companion() {
  let harness = Harness::start().await.expect("harness start");
  let companion = harness.connect_android().await.expect("connect companion");
  let setter = harness.connect_command_client().await.expect("command client");
  let listener = harness.connect_command_client().await.expect("listening client");
  let mut client_events = Box::pin(listener.system().events());
  let mut gateway_events = Box::pin(companion.system().events());

  setter
    .system()
    .companion_launch_set(CompanionLaunchSet { enabled: false })
    .await
    .expect("launch set");

  let changed = loop {
    let event = tokio::time::timeout(EVENT_WAIT, client_events.next())
      .await
      .expect("client hears the change")
      .expect("stream open");
    if let BridgeToClientSystemMsgEvent::CompanionLaunchChanged(reply) = event {
      break reply;
    }
  };
  assert!(!changed.enabled);

  let changed = loop {
    let event = tokio::time::timeout(EVENT_WAIT, gateway_events.next())
      .await
      .expect("the phone hears a change made on the device")
      .expect("stream open");
    if let BridgeToGatewaySystemMsgEvent::CompanionLaunchChanged(reply) = event {
      break reply;
    }
  };
  assert!(!changed.enabled);
}

#[tokio::test]
async fn the_companion_reads_and_sets_the_choice_over_the_gateway() {
  let harness = Harness::start().await.expect("harness start");
  let companion = harness.connect_android().await.expect("connect companion");
  let webapp = harness.connect_command_client().await.expect("command client");
  let mut client_events = Box::pin(webapp.system().events());

  let initial = companion
    .system()
    .companion_launch_get()
    .await
    .expect("companion launch get");
  assert!(initial.enabled);

  companion
    .system()
    .companion_launch_set(libbridgething::gateway::CompanionLaunchSet { enabled: false })
    .await
    .expect("companion launch set");

  let changed = loop {
    let event = tokio::time::timeout(EVENT_WAIT, client_events.next())
      .await
      .expect("client hears the phone's change")
      .expect("stream open");
    if let BridgeToClientSystemMsgEvent::CompanionLaunchChanged(reply) = event {
      break reply;
    }
  };
  assert!(!changed.enabled);
  assert!(!harness.state().meta.companion_launch());
}
