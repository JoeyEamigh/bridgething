use crate::{
  api::ota::{OtaAvailable, OtaPollStatus, OtaRun},
  provider::ResumeTarget,
};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct HostInfo {
  pub app_name: String,
  pub app_version: String,
  pub os_name: String,
  pub os_version: String,
  pub host_identifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct SessionHostInfo {
  pub app_name: String,
  pub app_version: String,
  pub os_name: String,
  pub os_version: String,
  pub host_identifier: String,
  pub lib_version: String,
  pub libbridgething_version: String,
  pub adapter_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct CapabilityFlags {
  pub geo: bool,
  pub notifications: bool,
  pub net_fetch: bool,
  pub net_ws: bool,
  pub audio_tts: bool,
  pub voice_model: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum PeerLinkStatus {
  Connected,
  LinkFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct SessionPeer {
  pub id: String,
  pub name: String,
  pub status: PeerLinkStatus,
  pub link_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum AuthKind {
  Idle,
  Pending,
  Authenticated,
  Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct AuthState {
  pub kind: AuthKind,
  pub user_code: Option<String>,
  pub verification_url: Option<String>,
  pub verification_url_complete: Option<String>,
  pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum ServiceHealthKind {
  Ok,
  RateLimited,
  Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct ServiceHealth {
  pub kind: ServiceHealthKind,
  pub retry_after_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct ProviderInfo {
  pub id: String,
  pub display_name: String,
  pub available: bool,
  pub connected: bool,
  pub auth_state: AuthState,
  pub service_health: ServiceHealth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum RepeatMode {
  Off,
  One,
  All,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct NowPlayingTrack {
  pub id: Option<String>,
  pub title: Option<String>,
  pub artist: Option<String>,
  pub album: Option<String>,
  pub artwork_url: Option<String>,
  pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct NowPlayingPlayback {
  pub playing: bool,
  pub position_ms: u64,
  pub shuffle: bool,
  pub repeat_mode: RepeatMode,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct NowPlaying {
  pub track: Option<NowPlayingTrack>,
  pub playback: NowPlayingPlayback,
  pub app_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum AncsAuthStatus {
  Unknown,
  Probing,
  Authorized,
  Unauthorized,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct AncsAuthStatusEntry {
  pub device_id: String,
  pub status: AncsAuthStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum WebappSource {
  Builtin,
  Installed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum WebappRole {
  Standard,
  Launcher,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum ConfigKind {
  String,
  Number,
  Boolean,
  Enum,
  Secret,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct ConfigField {
  pub kind: ConfigKind,
  pub key: String,
  pub label: String,
  pub pattern: Option<String>,
  pub min_length: Option<u32>,
  pub max_length: Option<u32>,
  pub min: Option<f64>,
  pub max: Option<f64>,
  pub step: Option<f64>,
  pub choices: Vec<String>,
  pub default_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct ExtensionInfo {
  pub permissions: Vec<String>,
  pub api: u32,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct WebappInfo {
  pub id: String,
  pub name: String,
  pub source: WebappSource,
  pub role: WebappRole,
  pub version: String,
  pub provenance: Option<String>,
  pub description: Option<String>,
  pub icon_hash: Option<String>,
  pub settings_hash: Option<String>,
  pub overlay_hash: Option<String>,
  pub config: Vec<ConfigField>,
  pub permissions: Vec<String>,
  pub extension: Option<ExtensionInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct ActiveWebapp {
  pub id: String,
  pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct DeviceWebappsEntry {
  pub device_id: String,
  pub webapps: Vec<WebappInfo>,
  pub active: Option<ActiveWebapp>,
  pub listed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum WebappSlot {
  Launcher,
  Overlay,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct WebappSlots {
  pub launcher: Option<String>,
  pub overlay: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct ConfigEntry {
  pub key: String,
  pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct DocEntry {
  pub key: String,
  pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum WebappResourceKind {
  Icon,
  Settings,
  Overlay,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct WebappResourceFile {
  pub path: String,
  pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct WebappResourceOrigin {
  pub url: String,
  pub sha256: String,
  pub size: u64,
  pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct DeviceMeta {
  pub daemon_version: String,
  pub libbridgething_version: String,
  pub image_version: String,
  pub app_name: String,
  pub os_name: String,
  pub os_version: String,
  pub channel: String,
  pub model_name: String,
  pub serial_number: String,
  pub nickname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct DeviceMetaEntry {
  pub device_id: String,
  pub meta: DeviceMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum ModelPlatform {
  Ios,
  Android,
  Macos,
  Linux,
  Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum VoiceModelStatus {
  Absent,
  Downloading,
  Ready,
  Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct VoiceModelPaths {
  pub nlu_bundle_dir: Option<String>,
  pub asr_weights: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct VoiceModelState {
  pub status: VoiceModelStatus,
  pub received_bytes: u64,
  pub total_bytes: u64,
  pub version: Option<String>,
  pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum VoiceTurnTrigger {
  PushToTalk,
  Assistant,
  WakeWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum VoiceTurnPhase {
  Listening,
  Resolved,
  Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct VoiceTurn {
  pub device_id: String,
  pub stream_id: String,
  pub trigger: VoiceTurnTrigger,
  pub phase: VoiceTurnPhase,
  pub transcript: Option<String>,
  pub intent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct OtaPollConfig {
  pub interval_seconds: u64,
  pub auto_push: bool,
  pub root_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct DeviceAutoResume {
  pub device_id: String,
  pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct DeviceResumeTarget {
  pub device_id: String,
  pub target: ResumeTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct VoiceDebug {
  pub has_model: bool,
  pub armed_bundle: Option<String>,
  pub transfer_allowed: bool,
  pub paths: VoiceModelPaths,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct CompanionDebug {
  pub authority_playback_held: bool,
  pub authority_metadata_held: bool,
  pub authority_volume_held: bool,
  pub authority_app_bundle: Option<String>,
  pub arbitrated_source: Option<String>,
  pub library_source: Option<String>,
  pub last_played_from: Option<String>,
  pub attached_providers: Vec<String>,
  pub attached_schemes: Vec<String>,
  pub linked_devices: Vec<String>,
  pub auto_resume: Vec<DeviceAutoResume>,
  pub resume_targets: Vec<DeviceResumeTarget>,
  pub voice: VoiceDebug,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct SessionSnapshot {
  pub host_info: SessionHostInfo,
  pub providers: Vec<ProviderInfo>,
  pub provider_priority: Vec<String>,
  pub library_provider: Option<String>,
  pub peers: Vec<SessionPeer>,
  pub ancs_auth_statuses: Vec<AncsAuthStatusEntry>,
  pub now_playing: Option<NowPlaying>,
  pub device_meta: Vec<DeviceMetaEntry>,
  pub capability_flags: CapabilityFlags,
  pub voice_model: VoiceModelState,
  pub ota_poll_config: Option<OtaPollConfig>,
  pub webapps: Vec<DeviceWebappsEntry>,
  pub ota_runs: Vec<OtaRun>,
  pub ota_available: Vec<OtaAvailable>,
  pub ota_poll: OtaPollStatus,
}
