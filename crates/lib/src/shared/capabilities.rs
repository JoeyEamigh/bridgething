use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::CompanionAuthorityScope;

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct GatewayInfo {
  pub address: String,
  pub name: String,
  pub os_name: String,
  pub app_name: String,
  pub app_version: String,
  pub adapter_version: String,
  pub lib_version: String,
  pub libbridgething_version: String,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NetworkKind {
  #[default]
  Unknown,
  Wifi,
  Cellular,
  Ethernet,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NetworkInfo {
  pub kind: NetworkKind,
  pub metered: bool,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct SurfaceAvailability {
  pub geo: bool,
  pub notifications: bool,
  pub net_fetch: bool,
  pub net_ws: bool,
  pub audio_tts: bool,
  pub lyrics: bool,
  #[serde(default)]
  pub playback_targets: bool,
  #[serde(default)]
  pub forward: bool,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum MusicProvider {
  #[default]
  None,
  Spotify,
  AppleMusic,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct VoiceDescriptor {
  pub id: String,
  pub name: String,
  pub locale: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct AudioCapabilities {
  pub earcons: Vec<String>,
  pub voices: Vec<VoiceDescriptor>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct GatewayCapabilities {
  pub gateway: GatewayInfo,
  pub uri_schemes: Vec<String>,
  pub network: NetworkInfo,
  pub available: SurfaceAvailability,
  pub audio: AudioCapabilities,
  pub music_provider: MusicProvider,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Capabilities {
  pub gateway: Option<GatewayInfo>,
  pub available: SurfaceAvailability,
  pub authority: Vec<CompanionAuthorityScope>,
  pub uri_schemes: Vec<String>,
  pub network: NetworkInfo,
  pub audio: AudioCapabilities,
  pub music_provider: MusicProvider,
}
