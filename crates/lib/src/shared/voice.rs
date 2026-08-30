use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NluAlternate {
  pub intent: String,
  pub slots: Option<NluSlots>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NluSlots {
  pub target: Option<String>,
  pub target_type: Option<NluTargetType>,
  pub playlist: Option<String>,
  pub genre: Option<String>,
  pub mood: Option<String>,
  pub era: Option<String>,
  pub popularity_filter: Option<NluPopularityFilter>,
  pub position: Option<u32>,
  pub count: Option<u32>,
  pub scope: Option<NluScope>,
  pub enabled: Option<bool>,
  pub mute: Option<bool>,
  pub repeat_mode: Option<NluRepeatMode>,
  pub seconds: Option<i32>,
  pub speed: Option<NluPlaybackSpeed>,
  pub direction: Option<NluDirection>,
  pub amount: Option<NluAmount>,
  pub level: Option<u32>,
  pub preset: Option<String>,
  pub view: Option<NluView>,
  pub phone_action: Option<NluPhoneAction>,
  pub webapp_name: Option<String>,
  pub uri: Option<String>,
  pub context_uri: Option<String>,
}

impl NluSlots {
  pub fn has_catalog_slots(&self) -> bool {
    self.target.is_some()
      || self.target_type.is_some()
      || self.genre.is_some()
      || self.mood.is_some()
      || self.era.is_some()
      || self.popularity_filter.is_some()
      || self.position.is_some()
      || self.uri.is_some()
      || self.context_uri.is_some()
  }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluTargetType {
  Artist,
  Track,
  Album,
  Playlist,
  Podcast,
  Episode,
  Station,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluPopularityFilter {
  Top5,
  Top10,
  Popular,
  Recent,
  New,
  First,
  Random,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluScope {
  PreviousTrack,
  Restart,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluAmount {
  Small,
  Medium,
  Large,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluRepeatMode {
  Off,
  All,
  One,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluPlaybackSpeed {
  #[serde(rename = "1")]
  One,
  #[serde(rename = "1.2")]
  OnePointTwo,
  #[serde(rename = "1.5")]
  OnePointFive,
  #[serde(rename = "2")]
  Two,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluDirection {
  Up,
  Down,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluView {
  NowPlaying,
  Artist,
  Album,
  Playlist,
  Playlists,
  Library,
  Songs,
  Presets,
  Queue,
  SavedEpisodes,
  NewEpisodes,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluPhoneAction {
  Answer,
  Decline,
  End,
  Hold,
  Unhold,
  Swap,
  Merge,
  Mute,
  Unmute,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NluResolvedIntent {
  pub intent: String,
  #[serde(default)]
  pub slots: NluSlots,
  pub transcript: String,
  pub alternates: Option<Vec<NluAlternate>>,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum VoiceCaptureReason {
  #[default]
  PushToTalk,
  Assistant,
  WakeWord,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum NluStage {
  FastPath,
  Model,
  RejectedNoIntent,
  RejectedClarify,
  NoModel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum VoiceDispatchErrorCode {
  WebappNotFound,
  NotDispatchable,
  Unsupported,
  PlaybackFailed,
  BadSlots,
  Internal,
}

/// Where a recognized voice intent went.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum VoiceDispatchTarget {
  Playback,
  /// A device action, such as pairing mode or saving a preset.
  Device,
  Phone,
  /// The active webapp, which renders the intent.
  Display,
  WebappSwitch,
}
