use std::time::Duration;

use bridgething_gateway::RequestFailure;
use bridgething_test_harness::Harness;
use futures::{Stream, StreamExt};
use libbridgething::{
  BrightnessMode, DimSettings, DisplaySettings, HardwareError,
  client::{BridgeToClientHardwareMsgEvent, DisplaySetLevel},
  gateway::{BridgeToGatewaySystemMsgEvent, DisplaySettingsRejected, DisplaySettingsSet},
};

const EVENT_WAIT: Duration = Duration::from_secs(3);

fn chosen() -> DisplaySettings {
  DisplaySettings {
    mode: BrightnessMode::Auto,
    level: 0.6,
    dim: DimSettings {
      after_s: Some(300),
      level: 0.0,
    },
  }
}

async fn next_change(mut events: impl Stream<Item = BridgeToGatewaySystemMsgEvent> + Unpin) -> DisplaySettings {
  loop {
    let event = tokio::time::timeout(EVENT_WAIT, events.next())
      .await
      .expect("a display settings change")
      .expect("stream open");
    if let BridgeToGatewaySystemMsgEvent::DisplaySettingsChanged(settings) = event {
      return settings;
    }
  }
}

#[tokio::test]
async fn a_companion_reads_and_sets_display_settings_over_the_gateway() {
  let harness = Harness::start().await.expect("harness start");
  let companion = harness.connect_android().await.expect("connect companion");
  let webapp = harness.connect_command_client().await.expect("command client");
  let mut hardware = Box::pin(webapp.hardware().events());

  let initial = companion
    .system()
    .display_settings_get()
    .await
    .expect("display settings get");
  assert_eq!(initial, DisplaySettings::default());

  let reply = companion
    .system()
    .display_settings_set(DisplaySettingsSet { settings: chosen() })
    .await
    .expect("display settings set");
  assert_eq!(reply, chosen(), "the reply is what the device now holds");
  assert_eq!(harness.state().display.settings(), chosen());

  let dim = loop {
    let event = tokio::time::timeout(EVENT_WAIT, hardware.next())
      .await
      .expect("the hub hears the phone's change")
      .expect("stream open");
    if let BridgeToClientHardwareMsgEvent::DimChanged(dim) = event {
      break dim;
    }
  };
  assert_eq!(dim, chosen().dim);

  let announced = webapp.system().version_request().await.expect("version");
  assert_eq!(
    announced.display,
    chosen(),
    "the meta every host reads carries the settings"
  );
}

#[tokio::test]
async fn a_set_the_device_cannot_honor_is_refused_with_its_reason_and_changes_nothing() {
  let harness = Harness::start().await.expect("harness start");
  let companion = harness.connect_android().await.expect("connect companion");

  for (bad, error) in [
    (
      DisplaySettings {
        dim: DimSettings {
          after_s: Some(14),
          ..chosen().dim
        },
        ..chosen()
      },
      HardwareError::DimTimerOutOfRange,
    ),
    (
      DisplaySettings { level: 1.5, ..chosen() },
      HardwareError::LevelOutOfRange,
    ),
  ] {
    let refused = companion
      .system()
      .display_settings_set(DisplaySettingsSet { settings: bad })
      .await;
    assert!(
      matches!(refused, Err(RequestFailure::Domain(DisplaySettingsRejected { error: got })) if got == error),
      "{bad:?} answered {refused:?}"
    );
  }
  assert_eq!(harness.state().display.settings(), DisplaySettings::default());
}

#[tokio::test]
async fn a_change_made_on_the_device_reaches_every_linked_gateway() {
  let harness = Harness::start().await.expect("harness start");
  let first = harness.connect_android().await.expect("first companion");
  let second = harness.connect_android().await.expect("second companion");
  let webapp = harness.connect_command_client().await.expect("command client");
  let heard_first = next_change(Box::pin(first.system().events()));
  let heard_second = next_change(Box::pin(second.system().events()));

  webapp
    .hardware()
    .display_set_level(DisplaySetLevel { level: 0.6 })
    .await
    .expect("display set level");

  let (first, second) = tokio::join!(heard_first, heard_second);
  let expected = DisplaySettings {
    level: 0.6,
    ..DisplaySettings::default()
  };
  assert_eq!(first, expected);
  assert_eq!(second, expected);
}

#[tokio::test]
async fn a_change_from_one_gateway_reaches_the_other() {
  let harness = Harness::start().await.expect("harness start");
  let setter = harness.connect_android().await.expect("setting companion");
  let listener = harness.connect_android().await.expect("listening companion");
  let heard = next_change(Box::pin(listener.system().events()));

  let system = setter.system();
  let (heard, reply) = tokio::join!(
    heard,
    system.display_settings_set(DisplaySettingsSet { settings: chosen() })
  );
  reply.expect("display settings set");
  assert_eq!(heard, chosen());
}

#[tokio::test]
async fn display_settings_set_over_the_gateway_outlive_the_daemon() {
  let harness = Harness::start().await.expect("harness start");
  let companion = harness.connect_android().await.expect("connect companion");
  companion
    .system()
    .display_settings_set(DisplaySettingsSet { settings: chosen() })
    .await
    .expect("display settings set");

  let harness = harness.restart().await.expect("restart");
  let companion = harness.connect_android().await.expect("reconnect companion");
  let stored = companion
    .system()
    .display_settings_get()
    .await
    .expect("display settings get");
  assert_eq!(stored, chosen());
}
