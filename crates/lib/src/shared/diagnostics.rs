use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Diagnostics {
  pub disk_used_bytes: u32,
  pub disk_free_bytes: u32,
  pub mem_used_bytes: u32,
  pub mem_avail_bytes: u32,
  pub uptime_s: u32,
  pub soc_temp_c: Option<f32>,
  pub load_avg: [f32; 3],
  pub daemon_version: String,
  pub kernel_version: String,
  pub boot_id: String,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum LogLevel {
  Trace,
  Debug,
  #[default]
  Info,
  Warn,
  Error,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum LogSource {
  #[default]
  Daemon,
  System,
  All,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct LogEntry {
  pub ts_unix_s: u32,
  pub level: LogLevel,
  pub target: String,
  pub message: String,
}
