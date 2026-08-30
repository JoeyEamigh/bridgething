use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct LyricLine {
  /// Offset from track start.
  pub start_ms: u32,
  pub text: String,
}

/// `synced` and `plain` are independent; either can be null.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Lyrics {
  pub synced: Option<Vec<LyricLine>>,
  pub plain: Option<String>,
  pub source: String,
}
