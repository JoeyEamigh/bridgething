use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum LauncherGesture {
  /// Hold M to jump to the launcher.
  LongPress,
  /// Press M five times within 1.5 seconds to jump to the launcher.
  #[default]
  FivePress,
}
