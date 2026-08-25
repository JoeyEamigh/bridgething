use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Device {
  pub name: String,
  #[serde(rename = "type")]
  pub device_type: DeviceType,
  pub id: String,
  pub kind: LinkKind,
  pub default: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum LinkKind {
  #[default]
  Bluetooth,
  Network,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
#[derive(Default)]
pub enum DeviceType {
  Android,
  #[serde(rename = "iOS")]
  Ios,
  Windows,
  MacOS,
  Linux,
  #[default]
  Unknown,
}
