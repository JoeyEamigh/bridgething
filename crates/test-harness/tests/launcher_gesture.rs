use std::time::Duration;

use bridgething_test_harness::Harness;
use futures::StreamExt;
use libbridgething::{
  LauncherGesture,
  client::{BridgeToClientSystemMsgEvent, LauncherGestureSet},
};

const EVENT_WAIT: Duration = Duration::from_secs(3);

#[tokio::test]
async fn the_launcher_gesture_defaults_to_five_press_and_survives_a_change() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");

  let initial = client.system().launcher_gesture_get().await.expect("gesture get");
  assert_eq!(
    initial.gesture,
    LauncherGesture::FivePress,
    "a device that has never been told otherwise keeps five-press"
  );

  client
    .system()
    .launcher_gesture_set(LauncherGestureSet {
      gesture: LauncherGesture::LongPress,
    })
    .await
    .expect("gesture set");

  let reader = harness.connect_command_client().await.expect("second client");
  let stored = loop {
    let reply = reader.system().launcher_gesture_get().await.expect("gesture get");
    if reply.gesture == LauncherGesture::LongPress {
      break reply.gesture;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
  };
  assert_eq!(stored, LauncherGesture::LongPress);
  assert_eq!(harness.state().meta.launcher_gesture(), LauncherGesture::LongPress);
}

#[tokio::test]
async fn the_device_meta_announces_the_gesture_so_hosts_never_have_to_ask() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");

  let announced = client.system().version_request().await.expect("version");
  assert_eq!(
    announced.launcher_gesture,
    LauncherGesture::FivePress,
    "the meta every host already reads carries the gesture"
  );

  client
    .system()
    .launcher_gesture_set(LauncherGestureSet {
      gesture: LauncherGesture::LongPress,
    })
    .await
    .expect("gesture set");

  let reader = harness.connect_command_client().await.expect("second client");
  let stored = loop {
    let announced = reader.system().version_request().await.expect("version");
    if announced.launcher_gesture == LauncherGesture::LongPress {
      break announced.launcher_gesture;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
  };
  assert_eq!(stored, LauncherGesture::LongPress);
}

#[tokio::test]
async fn a_gesture_change_reaches_every_connected_client() {
  let harness = Harness::start().await.expect("harness start");
  let setter = harness.connect_command_client().await.expect("command client");
  let listener = harness.connect_command_client().await.expect("listening client");
  let mut events = Box::pin(listener.system().events());

  setter
    .system()
    .launcher_gesture_set(LauncherGestureSet {
      gesture: LauncherGesture::LongPress,
    })
    .await
    .expect("gesture set");

  let changed = loop {
    let event = tokio::time::timeout(EVENT_WAIT, events.next())
      .await
      .expect("gesture change event")
      .expect("stream open");
    if let BridgeToClientSystemMsgEvent::LauncherGestureChanged(reply) = event {
      break reply;
    }
  };
  assert_eq!(changed.gesture, LauncherGesture::LongPress);
}

#[tokio::test]
async fn the_companion_reads_and_sets_the_gesture_over_the_gateway() {
  let harness = Harness::start().await.expect("harness start");
  let companion = harness.connect_android().await.expect("connect companion");
  let webapp = harness.connect_command_client().await.expect("command client");
  let mut client_events = Box::pin(webapp.system().events());

  let initial = companion
    .system()
    .launcher_gesture_get()
    .await
    .expect("companion gesture get");
  assert_eq!(initial.gesture, LauncherGesture::FivePress);

  companion
    .system()
    .launcher_gesture_set(libbridgething::gateway::LauncherGestureSet {
      gesture: LauncherGesture::LongPress,
    })
    .await
    .expect("companion gesture set");

  let changed = loop {
    let event = tokio::time::timeout(EVENT_WAIT, client_events.next())
      .await
      .expect("client hears the phone's change")
      .expect("stream open");
    if let BridgeToClientSystemMsgEvent::LauncherGestureChanged(reply) = event {
      break reply;
    }
  };
  assert_eq!(changed.gesture, LauncherGesture::LongPress);
  assert_eq!(harness.state().meta.launcher_gesture(), LauncherGesture::LongPress);
}

#[tokio::test]
async fn a_change_made_on_the_device_reaches_the_companion() {
  let harness = Harness::start().await.expect("harness start");
  let companion = harness.connect_android().await.expect("connect companion");
  let webapp = harness.connect_command_client().await.expect("command client");
  let mut gateway_events = Box::pin(companion.system().events());

  webapp
    .system()
    .launcher_gesture_set(LauncherGestureSet {
      gesture: LauncherGesture::LongPress,
    })
    .await
    .expect("gesture set");

  let changed = loop {
    let event = tokio::time::timeout(EVENT_WAIT, gateway_events.next())
      .await
      .expect("the phone hears a change made on the device")
      .expect("stream open");
    if let libbridgething::gateway::BridgeToGatewaySystemMsgEvent::LauncherGestureChanged(reply) = event {
      break reply;
    }
  };
  assert_eq!(changed.gesture, LauncherGesture::LongPress);
}
