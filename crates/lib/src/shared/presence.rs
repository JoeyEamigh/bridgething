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
  /// Capped to a low level. The host is idle but its display is still on.
  Dim,
  /// Backlight off. The host display is asleep, or the host went away.
  Off,
}

/// What a connected host says about its own display, so the device screen can follow it.
///
/// A device with no presence-reporting host leaves the screen to the brightness mode. Once a host
/// reports with `heartbeatS`, the device blanks if that host goes quiet. A report without
/// `heartbeatS` never expires, so a host that is shutting down sends `active` without one to hand
/// the screen back.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct HostPresence {
  pub display: DisplayState,
  /// Seconds since the host user last touched an input device.
  pub idle_s: Option<u32>,
  /// Seconds until the host reports again. The device blanks after three missed reports, and never
  /// sooner than 60 seconds. Omit it for a one-off report that does not expire.
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
  /// The host promised reports and stopped sending them.
  Failsafe,
  /// A button or knob press woke the screen.
  LocalWake,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PresenceInfo {
  pub display: DisplayState,
  pub source: PresenceSource,
}
