use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum BrightnessMode {
  /// The device sets the backlight from its ambient light sensor.
  #[default]
  Auto,
  /// `setLevel` sets the backlight.
  Manual,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct BrightnessState {
  pub mode: BrightnessMode,
  /// 0.0 to 1.0, as last set by `setLevel`.
  pub level: f32,
  /// 0.0 to 1.0. Follows `level` in `manual` mode and the light sensor in `auto`.
  pub effective_level: f32,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct HardwareState {
  pub brightness: BrightnessState,
  /// 0 to 100.
  pub ambient_level: u8,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum HardwareError {
  /// `setLevel` takes a level from 0.0 to 1.0.
  LevelOutOfRange,
  /// `setLevel` needs `manual` mode. Set the mode first.
  ModeMismatch,
}
