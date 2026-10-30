use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How lit the device screen should be. The host maps its own display and idle state onto this.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum DisplayState {
  /// Full brightness, as set by the brightness mode.
  #[default]
  Active,
  /// Lowered to `PresenceSettings.dimLevel`. The host display is dimmed, or an idle timer ran out.
  Dim,
  /// Backlight off. The host display is asleep, or an idle timer ran out.
  Off,
}

/// What a connected host says about its own display, so the device screen can follow it.
///
/// The device acts on `display` only when `PresenceSettings.followHost` is on, and ignores it
/// otherwise. A host counts as connected while its reports keep coming: with `heartbeatS` set, after
/// three missed reports the device treats the host as gone and the idle timers take over, and a
/// report without `heartbeatS` never expires. `idleS` counts as activity for the idle timers whether
/// or not the device follows the host.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct HostPresence {
  pub display: DisplayState,
  /// Seconds since the host user last touched an input device.
  pub idle_s: Option<u32>,
  /// Seconds until the host reports again. The device treats the host as gone after three missed
  /// reports. Omit it for a one-off report that does not expire.
  pub heartbeat_s: Option<u16>,
}

/// Why the device screen is in its current state.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PresenceSource {
  /// No host is managing the screen, so the brightness mode is in charge.
  #[default]
  None,
  /// The host reported this state.
  Host,
  /// A button, dial or touch woke the screen.
  LocalWake,
  /// The idle timers dimmed or slept the screen.
  Timer,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PresenceInfo {
  pub display: DisplayState,
  pub source: PresenceSource,
}

/// What the user wants the screen to do. The default leaves the screen to the brightness mode, as it
/// was before presence existed.
///
/// `followHost` sleeps and wakes the screen with the host display while a host is connected. The idle
/// timers dim and sleep it when it is not following, and also when it is following but no host is
/// connected, so a host that is off or unplugged falls back to the timers. An idle timer counts
/// activity on the device (buttons, dial, touch) and, when a host reports one, on the host.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export, export_to = "shared.ts")]
pub struct PresenceSettings {
  /// Sleep and wake with the host display. A host that dims its display dims this one. With no host
  /// connected the idle timers apply instead.
  pub follow_host: bool,
  /// Idle seconds before the screen dims, 5 to 86400. Null never dims it.
  pub dim_after_s: Option<u32>,
  /// Idle seconds before the screen sleeps, 5 to 86400, and later than `dimAfterS`. Null never
  /// sleeps it.
  pub sleep_after_s: Option<u32>,
  /// How far a dimmed screen drops, `0.05` to `1.0`. With manual brightness it is the highest level
  /// a dimmed screen shows, and a screen already dimmer stays put. With ambient brightness it
  /// scales whatever the light sensor asks for.
  pub dim_level: f32,
}

impl Default for PresenceSettings {
  fn default() -> Self {
    Self {
      follow_host: false,
      dim_after_s: None,
      sleep_after_s: None,
      dim_level: 0.2,
    }
  }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PresenceSettingsError {
  /// `dimAfterS` has to be earlier than `sleepAfterS`.
  DimNotBeforeSleep,
  /// A timer outside 5 to 86400 seconds.
  TimerOutOfRange,
  /// `dimLevel` outside `0.05` to `1.0`.
  DimLevelOutOfRange,
}
