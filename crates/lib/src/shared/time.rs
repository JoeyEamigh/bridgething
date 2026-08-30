use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The device has no battery-backed clock, so the phone is the time authority. Read the zone from
/// `tzIana`; when it is null, use `utcOffsetMinutes` plus `dstOffsetMinutes`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct TimeInfo {
  /// IANA zone identifier, for example `America/Denver`.
  pub tz_iana: Option<String>,
  pub locale: Option<String>,
  pub wall_clock_unix_s: Option<u32>,
  pub utc_offset_minutes: Option<i16>,
  pub dst_offset_minutes: Option<i8>,
}
