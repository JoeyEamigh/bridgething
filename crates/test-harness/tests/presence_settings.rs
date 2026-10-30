use std::time::Duration;

use bridgething_client::RequestFailure;
use bridgething_test_harness::Harness;
use libbridgething::{
  DisplayState, HostPresence, PresenceSettings, PresenceSettingsError, PresenceSource,
  client::{PresenceSettingsErrorReply, PresenceSettingsSet},
};

const SETTLE: Duration = Duration::from_secs(5);

fn timers(dim: u32, sleep: u32) -> PresenceSettings {
  PresenceSettings {
    dim_after_s: Some(dim),
    sleep_after_s: Some(sleep),
    dim_level: 0.35,
    ..PresenceSettings::default()
  }
}

fn host_off() -> HostPresence {
  HostPresence {
    display: DisplayState::Off,
    idle_s: None,
    heartbeat_s: None,
  }
}

#[tokio::test]
async fn a_fresh_device_never_dims_or_sleeps_and_does_not_follow_the_host() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");

  let reply = client.presence().settings_get().await.expect("settings get");
  assert_eq!(reply.settings, PresenceSettings::default());
  assert!(!reply.settings.follow_host);
  assert_eq!(reply.settings.dim_after_s, None);
  assert_eq!(reply.settings.sleep_after_s, None);
}

#[tokio::test]
async fn timers_are_kept_across_a_restart() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");
  let reply = client
    .presence()
    .settings_set(PresenceSettingsSet {
      settings: timers(120, 900),
    })
    .await
    .expect("settings set");
  assert_eq!(reply.settings, timers(120, 900));

  let harness = harness.restart().await.expect("restart");
  let reader = harness.connect_command_client().await.expect("second client");
  let stored = reader.presence().settings_get().await.expect("settings get");
  assert_eq!(stored.settings, timers(120, 900));
}

#[tokio::test]
async fn a_request_the_device_cannot_honor_is_refused_with_the_reason_and_changes_nothing() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");

  let backwards = client
    .presence()
    .settings_set(PresenceSettingsSet {
      settings: timers(600, 60),
    })
    .await
    .expect_err("dimming cannot come after sleeping");
  assert!(
    matches!(
      backwards,
      RequestFailure::Domain(PresenceSettingsErrorReply {
        error: PresenceSettingsError::DimNotBeforeSleep
      })
    ),
    "got {backwards:?}"
  );
  let unchanged = client.presence().settings_get().await.expect("settings get");
  assert_eq!(unchanged.settings, PresenceSettings::default());
}

#[tokio::test]
async fn following_the_host_and_timers_can_be_set_together() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");

  let both = PresenceSettings {
    follow_host: true,
    ..timers(60, 600)
  };
  let reply = client
    .presence()
    .settings_set(PresenceSettingsSet { settings: both })
    .await
    .expect("the timers are what the device falls back to without a host");
  assert_eq!(reply.settings, both);
}

#[tokio::test]
async fn a_host_that_goes_quiet_hands_the_screen_to_the_timers() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");
  let gateway = harness.connect_android().await.expect("gateway");

  client
    .presence()
    .settings_set(PresenceSettingsSet {
      settings: PresenceSettings {
        follow_host: true,
        sleep_after_s: Some(5),
        ..PresenceSettings::default()
      },
    })
    .await
    .expect("settings set");
  gateway
    .presence()
    .update(HostPresence {
      display: DisplayState::Active,
      idle_s: Some(0),
      heartbeat_s: Some(1),
    })
    .await
    .expect("presence update");
  assert!(
    harness
      .wait_for(
        |state| state.presence.snapshot().presence.source == PresenceSource::Host,
        SETTLE
      )
      .await,
    "the device heard the host"
  );

  assert!(
    harness
      .wait_for(
        |state| {
          let info = state.presence.snapshot().presence;
          (info.display, info.source) == (DisplayState::Off, PresenceSource::Timer)
        },
        Duration::from_secs(15)
      )
      .await,
    "three missed beats later the host is gone and the sleep timer decides"
  );
}

#[tokio::test]
async fn a_host_saying_its_display_is_off_is_ignored_until_the_device_is_told_to_follow_it() {
  let harness = Harness::start().await.expect("harness start");
  let client = harness.connect_command_client().await.expect("command client");
  let gateway = harness.connect_android().await.expect("gateway");

  gateway.presence().update(host_off()).await.expect("presence update");
  tokio::time::sleep(Duration::from_millis(300)).await;
  let ignored = harness.state().presence.snapshot().presence;
  assert_eq!(
    (ignored.display, ignored.source),
    (DisplayState::Active, PresenceSource::None),
    "a device that was not asked to follow the host leaves its screen alone"
  );

  client
    .presence()
    .settings_set(PresenceSettingsSet {
      settings: PresenceSettings {
        follow_host: true,
        ..PresenceSettings::default()
      },
    })
    .await
    .expect("settings set");
  assert!(
    harness
      .wait_for(
        |state| state.presence.snapshot().presence.display == DisplayState::Off,
        SETTLE
      )
      .await,
    "the host display is off and the device was told to follow it"
  );

  client
    .presence()
    .settings_set(PresenceSettingsSet {
      settings: PresenceSettings::default(),
    })
    .await
    .expect("settings set");
  assert!(
    harness
      .wait_for(
        |state| state.presence.snapshot().presence.display == DisplayState::Active,
        SETTLE
      )
      .await,
    "turning following off gives the screen back"
  );
}
