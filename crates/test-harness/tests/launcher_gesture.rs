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
