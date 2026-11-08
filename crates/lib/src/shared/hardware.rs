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
  #[serde(default)]
  pub dimmed: bool,
}

/// When the screen dims. A connected computer can also ask for a dim with `hostState.away`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export, export_to = "shared.ts")]
pub struct DimSettings {
  /// Seconds without input on the device or a connected computer, 15 to 86400. Null never dims.
  pub after_s: Option<u32>,
  /// `0.0` (dark) to `1.0`. Caps a manual level and scales an auto level.
  pub level: f32,
}

impl Default for DimSettings {
  fn default() -> Self {
    Self {
      after_s: None,
      level: 0.2,
    }
  }
}

/// The display settings a user chooses. Nothing the light sensor moves.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export, export_to = "shared.ts")]
pub struct DisplaySettings {
  pub mode: BrightnessMode,
  /// `0.0` to `1.0`. The backlight in `manual` mode.
  pub level: f32,
  pub dim: DimSettings,
}

impl Default for DisplaySettings {
  fn default() -> Self {
    Self {
      mode: BrightnessMode::Auto,
      level: 1.0,
      dim: DimSettings::default(),
    }
  }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct HardwareState {
  pub brightness: BrightnessState,
  #[serde(default)]
  pub dim: DimSettings,
  /// 0 to 100.
  pub ambient_level: u8,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum HardwareError {
  /// A level outside 0.0 to 1.0.
  LevelOutOfRange,
  /// `setLevel` needs `manual` mode. Set the mode first.
  ModeMismatch,
  /// `afterS` takes 15 to 86400 seconds.
  DimTimerOutOfRange,
}
