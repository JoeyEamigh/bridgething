use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{MediaType, RepeatMode, ShuffleMode};
use crate::to_slug;

pub const CARTHING_HACKS_LOGO: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD//gATQ3JlYXRlZCB3aXRoIEdJTVD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAEsASwDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDi6KKK+1PkwooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKAC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";

pub const IMAGE_SIZE: usize = 300;
pub const THUMBNAIL_SIZE: usize = 96;

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
/// A track with its album and artist resolved. Library search and browse return these.
pub struct Track {
  pub id: String,
  pub name: String,
  pub album: Album,
  /// Primary credited artist.
  pub artist: Artist,
  /// All credited artists, in order.
  pub artists: Vec<Artist>,
  pub duration_ms: u32,
  /// Opaque artwork asset id. Pass it to `asset.get` for the bytes.
  pub image_id: String,
  pub saved: bool,
}

#[serde_with::serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
/// An image, either an opaque asset id to pass to `asset.get`, or inline bytes.
pub enum Image {
  Id(String),
  Bytes(
    #[serde_as(as = "serde_with::Bytes")]
    #[ts(type = "Uint8Array")]
    Vec<u8>,
  ),
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Album {
  pub id: String,
  pub name: String,
  /// Opaque artwork asset id. Pass it to `asset.get` for the bytes.
  pub artwork_id: Option<String>,
}

impl From<String> for Album {
  fn from(name: String) -> Self {
    Self {
      id: format!("bridgething:album:{}", to_slug(&name)),
      name,
      artwork_id: None,
    }
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Artist {
  pub id: String,
  pub name: String,
  /// Opaque artwork asset id. Pass it to `asset.get` for the bytes.
  pub artwork_id: Option<String>,
}

impl From<String> for Artist {
  fn from(name: String) -> Self {
    Self {
      id: format!("bridgething:artist:{}", to_slug(&name)),
      name,
      artwork_id: None,
    }
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "shared.ts")]
/// The app driving playback on the phone.
pub struct CurrentlyActiveApplication {
  /// Bundle identifier, for example `com.spotify.client`.
  pub id: String,
  pub name: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "shared.ts")]
pub struct PlaybackOptions {
  pub repeat: RepeatMode,
  pub shuffle: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
/// What the current source allows. Disable the control for any flag that is false.
pub struct PlaybackRestrictions {
  pub can_repeat_context: bool,
  pub can_repeat_track: bool,
  pub can_seek: bool,
  pub can_skip_next: bool,
  pub can_skip_prev: bool,
  pub can_toggle_shuffle: bool,
  pub can_like: bool,
  pub can_change_volume: bool,
  pub can_set_output: bool,
}

impl PlaybackRestrictions {
  pub fn all_true() -> Self {
    Self {
      can_repeat_context: true,
      can_repeat_track: true,
      can_seek: true,
      can_skip_next: true,
      can_skip_prev: true,
      can_toggle_shuffle: true,
      can_like: true,
      can_change_volume: true,
      can_set_output: true,
    }
  }

  pub fn all_false() -> Self {
    Self {
      can_repeat_context: false,
      can_repeat_track: false,
      can_seek: false,
      can_skip_next: false,
      can_skip_prev: false,
      can_toggle_shuffle: false,
      can_like: false,
      can_change_volume: false,
      can_set_output: false,
    }
  }
}

impl Default for PlaybackRestrictions {
  fn default() -> Self {
    Self::all_true()
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "shared.ts")]
/// The current track with the tracks before and after it.
pub struct PlaybackQueue {
  pub next: Vec<Track>,
  pub current: Track,
  pub previous: Vec<Track>,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PlaybackState {
  /// No track is loaded.
  #[default]
  Stopped,
  /// A track is loaded and its position is held.
  Paused,
  Playing,
}

/// `positionMs` is the playhead when the state was taken. Advance it locally while `state` is
/// `playing`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Playback {
  pub state: PlaybackState,
  pub position_ms: u32,
  pub position_age_ms: Option<u32>,
  pub shuffle: bool,
  pub shuffle_mode: Option<ShuffleMode>,
  pub repeat: RepeatMode,
  pub queue_index: Option<u32>,
  pub queue_count: Option<u32>,
  pub queue_chapter_index: Option<u32>,
  /// False when the app refuses absolute seeks. Null means no signal yet.
  pub set_elapsed_time_available: Option<bool>,
  pub queue_list_avail: Option<bool>,
  pub apple_music_radio_ad: Option<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PlayerOptions {
  pub speed: f32,
  /// 0 turns crossfade off. Null when the source carries no crossfade setting.
  pub crossfade_ms: Option<u32>,
}

impl Default for PlayerOptions {
  fn default() -> Self {
    Self {
      speed: 1.0,
      crossfade_ms: None,
    }
  }
}

/// `contextUri` is the album, playlist, or show the track plays from. It sets what `skipNext` does.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PlayContext {
  pub context_uri: String,
}

/// Where a `queue` call puts the item.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum QueuePosition {
  #[default]
  Append,
  /// Directly after the current track.
  Next,
  /// At a 0-based slot in the upcoming list.
  Index(u32),
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct QueueItem {
  pub uri: String,
  pub title: Option<String>,
  pub artist: Option<String>,
  pub artist_uri: Option<String>,
  pub album: Option<String>,
  pub album_uri: Option<String>,
  /// Opaque artwork asset id. Pass it to `asset.get` for the bytes.
  pub artwork_id: Option<String>,
  pub duration_ms: Option<u32>,
  /// Opaque. Null when the source has none.
  pub persistent_id: Option<String>,
  #[serde(default, deserialize_with = "bool_absent_or_null_is_false")]
  pub queued: bool,
}

fn bool_absent_or_null_is_false<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
  Ok(Option::<bool>::deserialize(d)?.unwrap_or_default())
}

/// What the current track plays from, such as a playlist, an album, or a show.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PlaybackContext {
  pub uri: String,
  /// Null until the phone resolves the name.
  pub name: Option<String>,
}

/// An endpoint the source does not classify reports `unknown`.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PlaybackTargetKind {
  #[default]
  Unknown,
  Phone,
  Tablet,
  Computer,
  Speaker,
  Tv,
  GameConsole,
  Automobile,
  Wearable,
}

/// `transferTo` moves playback here. Check `available.playbackTargets` in `Capabilities` first.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PlaybackTarget {
  /// Opaque endpoint id. Pass it to `transferTo`.
  pub id: String,
  pub name: String,
  pub kind: PlaybackTargetKind,
  /// True when this endpoint is the one playing.
  pub is_active: bool,
  /// Null when the endpoint reports no volume.
  pub volume_percent: Option<u32>,
}

/// `onSnapshot` delivers it, and the SDK merges later deltas into it.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PlayerState {
  pub track: Option<MediaItem>,
  pub playback: Playback,
  pub queue: Vec<QueueItem>,
  pub options: PlayerOptions,
  pub context: Option<PlaybackContext>,
  #[serde(default)]
  pub target: Option<PlaybackTarget>,
}

/// A field stays null until the phone reports it.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct MediaItem {
  pub uri: Option<String>,
  pub persistent_id: Option<String>,
  pub title: Option<String>,
  pub album: Option<String>,
  pub album_uri: Option<String>,
  pub album_artist: Option<String>,
  pub artist: Option<String>,
  pub artist_uri: Option<String>,
  pub liked: Option<bool>,
  /// Opaque artwork asset id. Pass it to `asset.get` for the bytes.
  pub artwork_id: Option<String>,
  pub duration_ms: Option<u32>,
  pub media_types: Option<Vec<MediaType>>,
  pub track_number: Option<u16>,
  pub track_count: Option<u16>,
  pub is_like_supported: Option<bool>,
  pub is_ban_supported: Option<bool>,
  pub is_banned: Option<bool>,
  pub chapter_count: Option<u16>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum PlayerError {
  /// No connected phone claims the uri's scheme.
  SchemeUnclaimed { scheme: String },
  /// The phone accepted the command and then failed to carry it out.
  PlayFailed { reason: String },
  /// No phone is connected.
  NoGateway,
  /// `skipToIndex` named an index outside the queue.
  NotInQueue { index: u32 },
  /// `transferTo` named an endpoint outside the current target list.
  UnknownTarget { target_id: String },
}

#[cfg(test)]
mod tests {
  use super::QueueItem;

  #[test]
  fn queue_item_without_queued_field_decodes() {
    // phones ship on their own cadence; an additive wire field must decode when absent
    let legacy = rmp_serde::to_vec_named(&serde_json::json!({
      "uri": "spotify:track:abc",
      "title": "Song",
    }))
    .expect("encode legacy map");
    let item: QueueItem = rmp_serde::from_slice(&legacy).expect("decode without queued");
    assert!(!item.queued);
  }

  #[test]
  fn queue_item_with_null_queued_decodes() {
    // the kotlin msgpack encoder emits explicit nil for a defaulted nullable field
    let nulled = rmp_serde::to_vec_named(&serde_json::json!({
      "uri": "spotify:track:abc",
      "queued": null,
    }))
    .expect("encode nulled map");
    let item: QueueItem = rmp_serde::from_slice(&nulled).expect("decode with null queued");
    assert!(!item.queued);

    let set = rmp_serde::to_vec_named(&serde_json::json!({
      "uri": "spotify:track:abc",
      "queued": true,
    }))
    .expect("encode set map");
    let item: QueueItem = rmp_serde::from_slice(&set).expect("decode with queued set");
    assert!(item.queued);
  }
}
