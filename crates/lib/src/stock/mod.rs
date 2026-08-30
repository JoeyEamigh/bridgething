use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "stock.ts")]
pub struct StockSetPreset {
  /// Payload version. Send 1.
  pub version: usize,
  pub context_uri: String,
  /// Slot 1 to 4. The daemon ignores others.
  pub slot_index: usize,
  /// `tactile` or `voice`.
  pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "stock.ts")]
pub struct StockPreset {
  pub context_uri: String,
  pub image_url: Option<String>,
  /// Slot 1 to 4.
  pub slot_index: usize,
  pub name: Option<String>,
  pub description: Option<String>,
}
