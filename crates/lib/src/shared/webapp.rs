use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::{ExtensionInfo, ExtensionManifest};

pub const WEBAPP_PROVENANCE_MAX_LEN: usize = 2048;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum WebappError {
  WebappNotFound {
    id: String,
  },
  CannotUninstallBuiltin {
    id: String,
  },
  /// The manifest id is one the device reserves. Give the webapp a different uuid.
  IdReserved {
    id: String,
  },
  /// The extracted bundle is over the 1 GiB cap.
  ExtractedTooLarge {
    max_bytes: u32,
  },
  /// The provenance string is over 2048 bytes.
  ProvenanceTooLong {
    max_bytes: u32,
  },
  ZipMalformed {
    reason: String,
  },
  /// Put an `index.html` at the root of the bundle.
  MissingIndexHtml,
  /// `manifest.json` is missing, unparseable, or failed validation. `reason` says which.
  InvalidManifest {
    reason: String,
  },
  /// The manifest must declare the resource and the file must exist in the bundle.
  ResourceNotAvailable {
    id: String,
  },
  /// A bundle must declare `role` as `launcher` to take the launcher slot.
  NotALauncher {
    id: String,
  },
  /// A bundle must declare `overlay` to take the overlay slot.
  NoOverlay {
    id: String,
  },
  /// The manifest declares no config field with this key.
  UnknownConfigKey {
    key: String,
  },
  /// The value failed the field's declared constraints. `reason` says which.
  InvalidConfigValue {
    key: String,
    reason: String,
  },
  /// The doc value is over 256 KiB.
  InvalidDocValue {
    key: String,
    reason: String,
  },
  /// An unexpected failure.
  Internal {
    reason: String,
  },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum WebappSource {
  Builtin,
  Installed,
}

/// `launcher` makes the bundle eligible for the launcher slot and keeps it out of `webapp.list`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum WebappRole {
  #[default]
  Standard,
  Launcher,
}

/// Each overlay defaults to on. Set one false to draw that UI yourself; the events are unchanged.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct OverlayProfile {
  #[serde(default = "overlay_surface_default")]
  pub notifications: bool,
  /// Incoming / active call banner.
  #[serde(default = "overlay_surface_default")]
  pub call: bool,
  /// Bluetooth pairing PIN modal.
  #[serde(default = "overlay_surface_default")]
  pub pairing: bool,
  /// Banner shown while a paired phone has no live link.
  #[serde(default = "overlay_surface_default")]
  pub connection: bool,
  /// Transient volume level indicator.
  #[serde(default = "overlay_surface_default")]
  pub volume: bool,
  /// Voice turn indicator for listening, recognizing, and the outcome.
  #[serde(default = "overlay_surface_default")]
  pub voice: bool,
}

fn overlay_surface_default() -> bool {
  true
}

impl Default for OverlayProfile {
  fn default() -> Self {
    Self {
      notifications: true,
      call: true,
      pairing: true,
      connection: true,
      volume: true,
      voice: true,
    }
  }
}

impl OverlayProfile {
  pub fn any_enabled(&self) -> bool {
    self.notifications || self.call || self.pairing || self.connection || self.volume || self.voice
  }
}

/// A manifest that omits it gets 248 for `heroPx` and 96 for `thumbPx`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct ArtProfile {
  pub hero_px: u32,
  pub thumb_px: u32,
}

impl Default for ArtProfile {
  fn default() -> Self {
    Self {
      hero_px: 248,
      thumb_px: 96,
    }
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct WebappInfo {
  #[ts(type = "string")]
  pub id: Uuid,
  pub name: String,
  pub source: WebappSource,
  pub role: WebappRole,
  pub version: String,
  pub description: Option<String>,
  pub icon_hash: Option<String>,
  pub settings_hash: Option<String>,
  pub overlay_hash: Option<String>,
  pub config: Vec<ConfigField>,
  pub permissions: Vec<String>,
  #[serde(default)]
  pub renders_voice_display: bool,
  pub art: Option<ArtProfile>,
  pub provenance: Option<String>,
  #[serde(default)]
  pub extension: Option<ExtensionInfo>,
}

/// The `manifest.json` at the root of a webapp bundle.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct WebappManifest {
  #[ts(type = "string")]
  pub id: Uuid,
  pub name: String,
  pub version: String,
  pub description: Option<String>,
  /// Path in the bundle. The file must be 64 KiB or under.
  pub icon: Option<String>,
  /// Path in the bundle. The file must be 1 MiB or under.
  pub settings: Option<String>,
  /// Path in the bundle. The file must be 512 KiB or under.
  pub overlay: Option<String>,
  #[serde(default)]
  pub role: WebappRole,
  #[serde(default)]
  pub config: Vec<ConfigField>,
  /// The device recognizes `geo` and `net.proxy`.
  #[serde(default)]
  pub permissions: Vec<String>,
  #[serde(default)]
  pub renders_voice_display: bool,
  #[serde(default)]
  pub art: Option<ArtProfile>,
  #[serde(default)]
  pub overlays: OverlayProfile,
  #[serde(default)]
  pub extension: Option<ExtensionManifest>,
}

/// One setting the user can tune. In `manifest.json` it reads
/// `{"type":"string","data":{"key":"zip","label":"ZIP code"}}`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum ConfigField {
  String(StringField),
  Number(NumberField),
  Boolean(BoolField),
  Enum(EnumField),
  Secret(StringField),
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct StringField {
  pub key: String,
  pub label: String,
  pub pattern: Option<String>,
  /// In characters.
  pub min_length: Option<u32>,
  /// In characters.
  pub max_length: Option<u32>,
  pub default: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NumberField {
  pub key: String,
  pub label: String,
  pub min: Option<f64>,
  pub max: Option<f64>,
  pub step: Option<f64>,
  pub default: Option<f64>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct BoolField {
  pub key: String,
  pub label: String,
  pub default: Option<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct EnumField {
  pub key: String,
  pub label: String,
  pub choices: Vec<String>,
  pub default: Option<String>,
}

impl ConfigField {
  pub fn key(&self) -> &str {
    match self {
      ConfigField::String(f) | ConfigField::Secret(f) => &f.key,
      ConfigField::Number(f) => &f.key,
      ConfigField::Boolean(f) => &f.key,
      ConfigField::Enum(f) => &f.key,
    }
  }

  pub fn default_as_storage(&self) -> Option<String> {
    match self {
      ConfigField::String(f) | ConfigField::Secret(f) => f.default.clone(),
      ConfigField::Enum(f) => f.default.clone(),
      ConfigField::Number(f) => f.default.map(|n| n.to_string()),
      ConfigField::Boolean(f) => f.default.map(|b| b.to_string()),
    }
  }
}

/// `value` is always a string. Parse it by the field's kind; a boolean is `"true"` or `"false"`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct ConfigEntry {
  pub key: String,
  pub value: String,
}

/// Both the webapp and the companion app write it, and the last write wins. Values are strings.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct DocEntry {
  pub key: String,
  pub value: String,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_manifest_overlays_object_defaults_omitted_surfaces_on() {
    let manifest: WebappManifest = serde_json::from_str(
      r#"{"id":"00000000-0000-0000-0000-000000000001","name":"partial","version":"0.1.0",
          "overlays":{"notifications":false,"call":false,"pairing":false,"connection":false,"volume":false}}"#,
    )
    .expect("manifest parses");
    assert!(manifest.overlays.voice, "an undeclared surface stays on");
    assert!(
      manifest.overlays.any_enabled(),
      "one surface on means the overlay still injects"
    );
  }
}
