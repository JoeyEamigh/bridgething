use std::time::Duration;

use bridgething_test_harness::Harness;
use futures::StreamExt;
use libbridgething::{
  ForwardMessage, ForwardRouted,
  client::BridgeToClientMsgData,
  gateway::{BridgeToGatewayMsgData, ExtensionsRunning},
};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(3);

async fn plant(harness: &Harness) -> Uuid {
  let id = Uuid::now_v7();
  let dir = harness.state_dir().join("webapps").join(id.simple().to_string());
  std::fs::create_dir_all(&dir).expect("bundle dir");
  std::fs::write(dir.join("index.html"), b"<h1>planted</h1>").expect("index");
  std::fs::write(
    dir.join("manifest.json"),
    format!(r#"{{"id":"{id}","name":"planted","version":"0.1.0"}}"#),
  )
  .expect("manifest");
  harness.state().webapps.rescan().await;
  id
}

async fn forward_available(harness: &Harness) -> bool {
  harness.state().capabilities.snapshot().available.forward
}

#[tokio::test]
async fn a_client_forward_reaches_the_gateway_stamped_with_the_active_webapp() {
  let harness = Harness::start().await.expect("harness start");
  let active = plant(&harness).await;
  harness.state().set_active_webapp(active).await.expect("activate");

  let companion = harness.connect_android().await.expect("connect companion");
  let mut inbound = companion.events();
  let client = harness.connect_command_client().await.expect("connect client");

  client
    .event(ForwardMessage::Text("from the webapp".into()))
    .await
    .expect("send forward");

  let routed = tokio::time::timeout(WAIT, async {
    loop {
      let msg = inbound.recv().await.expect("gateway link alive");
      if let BridgeToGatewayMsgData::Forward(forward) = msg.data
        && let Some(event) = forward.into_event()
      {
        return event;
      }
    }
  })
  .await
  .expect("a forward should reach the gateway");

  let libbridgething::gateway::BridgeToGatewayForwardMsgEvent::Routed(routed) = routed;
  assert_eq!(routed.webapp, active, "the daemon stamps the active webapp id");
  assert_eq!(routed.message, ForwardMessage::Text("from the webapp".into()));
}

#[tokio::test]
async fn a_gateway_forward_only_reaches_the_client_when_it_names_the_active_webapp() {
  let harness = Harness::start().await.expect("harness start");
  let active = plant(&harness).await;
  let other = plant(&harness).await;
  harness.state().set_active_webapp(active).await.expect("activate");

  let companion = harness.connect_android().await.expect("connect companion");
  let client = harness.connect_command_client().await.expect("connect client");
  let mut inbound = client.events();

  companion
    .forward()
    .routed(ForwardRouted {
      webapp: other,
      message: ForwardMessage::Text("for the wrong app".into()),
    })
    .await
    .expect("send a misaddressed forward");
  companion
    .forward()
    .routed(ForwardRouted {
      webapp: active,
      message: ForwardMessage::Text("for the active app".into()),
    })
    .await
    .expect("send an addressed forward");

  let delivered = tokio::time::timeout(WAIT, async {
    loop {
      let msg = inbound.recv().await.expect("client link alive");
      if let BridgeToClientMsgData::Forward(message) = msg.data {
        return message;
      }
    }
  })
  .await
  .expect("the addressed forward should reach the client");

  assert_eq!(
    delivered,
    ForwardMessage::Text("for the active app".into()),
    "the forward for the inactive webapp is dropped, not queued behind the active one"
  );
}

#[tokio::test]
async fn forward_availability_tracks_the_running_set_and_the_active_webapp() {
  let harness = Harness::start().await.expect("harness start");
  let active = plant(&harness).await;
  let other = plant(&harness).await;
  harness.state().set_active_webapp(active).await.expect("activate");

  let companion = harness.connect_android().await.expect("connect companion");
  assert!(!forward_available(&harness).await, "no host has reported anything yet");

  companion
    .forward()
    .extensions_running(ExtensionsRunning { webapps: vec![other] })
    .await
    .expect("report a running extension for another webapp");
  assert!(
    !harness
      .wait_for(
        |state| state.capabilities.snapshot().available.forward,
        Duration::from_millis(300)
      )
      .await,
    "an extension for a webapp that is not active does not make forward available"
  );

  companion
    .forward()
    .extensions_running(ExtensionsRunning {
      webapps: vec![other, active],
    })
    .await
    .expect("report a running extension for the active webapp");
  assert!(
    harness
      .wait_for(|state| state.capabilities.snapshot().available.forward, WAIT)
      .await,
    "the active webapp now has a running extension"
  );

  harness.state().set_active_webapp(other).await.expect("switch away");
  assert!(
    harness
      .wait_for(|state| state.capabilities.snapshot().available.forward, WAIT)
      .await,
    "the other webapp also has a running extension"
  );

  let third = plant(&harness).await;
  harness
    .state()
    .set_active_webapp(third)
    .await
    .expect("switch to an app with no extension");
  assert!(
    !harness
      .wait_for(
        |state| state.capabilities.snapshot().available.forward,
        Duration::from_millis(300)
      )
      .await,
    "switching to a webapp with no running extension clears forward"
  );
}

#[tokio::test]
async fn forward_availability_clears_when_the_reporting_gateway_disconnects() {
  let harness = Harness::start().await.expect("harness start");
  let active = plant(&harness).await;
  harness.state().set_active_webapp(active).await.expect("activate");

  let companion = harness.connect_android().await.expect("connect companion");
  companion
    .forward()
    .extensions_running(ExtensionsRunning { webapps: vec![active] })
    .await
    .expect("report a running extension");
  assert!(
    harness
      .wait_for(|state| state.capabilities.snapshot().available.forward, WAIT)
      .await,
    "forward is available while the host is attached"
  );

  drop(companion);

  assert!(
    harness
      .wait_for(|state| !state.capabilities.snapshot().available.forward, WAIT)
      .await,
    "the running set dies with the gateway link that reported it, announced or not"
  );
}

#[tokio::test]
async fn a_client_forward_with_no_active_webapp_is_dropped_rather_than_broadcast() {
  let harness = Harness::start().await.expect("harness start");
  assert_eq!(
    harness.state().active_webapp().await.expect("active webapp"),
    None,
    "a harness with no bundles installed has nothing active to stamp"
  );

  let companion = harness.connect_android().await.expect("connect companion");
  let mut inbound = companion.events();
  let client = harness.connect_command_client().await.expect("connect client");

  client
    .event(ForwardMessage::Text("into the void".into()))
    .await
    .expect("send forward");

  let leaked = tokio::time::timeout(Duration::from_millis(400), async {
    loop {
      let msg = inbound.recv().await.expect("gateway link alive");
      if let BridgeToGatewayMsgData::Forward(_) = msg.data {
        return;
      }
    }
  })
  .await;
  assert!(
    leaked.is_err(),
    "an unstampable forward must be dropped, not broadcast unrouted"
  );
}

#[tokio::test]
async fn the_capabilities_broadcast_carries_the_forward_flip_to_the_webapp() {
  let harness = Harness::start().await.expect("harness start");
  let active = plant(&harness).await;
  harness.state().set_active_webapp(active).await.expect("activate");

  let companion = harness.connect_android().await.expect("connect companion");
  let client = harness.connect_command_client().await.expect("connect client");
  let mut updates = Box::pin(client.capabilities().events());

  companion
    .forward()
    .extensions_running(ExtensionsRunning { webapps: vec![active] })
    .await
    .expect("report a running extension");

  let flipped = tokio::time::timeout(WAIT, async {
    loop {
      let libbridgething::client::BridgeToClientCapabilitiesMsgEvent::Update(update) =
        updates.next().await.expect("capabilities stream alive");
      if update.capabilities.available.forward {
        return update;
      }
    }
  })
  .await
  .expect("the webapp should be told forward became available");
  assert!(flipped.capabilities.available.forward);
}
