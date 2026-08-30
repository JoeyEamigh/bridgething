//! Priority lane for outbound frames. Writers drain Normal first, then Bulk, then Background. Bulk
//! carries large payloads a user waits on; Background carries updates and prefetch.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum Priority {
  #[default]
  Normal,
  Bulk,
  Background,
}

impl Priority {
  pub const fn as_byte(self) -> u8 {
    match self {
      Self::Normal => 0x00,
      Self::Bulk => 0x01,
      Self::Background => 0x02,
    }
  }

  pub const fn from_byte(byte: u8) -> Self {
    match byte {
      0x01 => Self::Bulk,
      0x02 => Self::Background,
      _ => Self::Normal,
    }
  }
}
