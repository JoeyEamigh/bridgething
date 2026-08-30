//! Partial updates to what the phone is playing. A field left unset keeps its prior value.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum RepeatMode {
  #[default]
  Off,
  All,
  One,
}

/// A phone that does not separate track from album shuffle reports `songs` while shuffle is on.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum ShuffleMode {
  #[default]
  Off,
  Songs,
  Albums,
}

impl ShuffleMode {
  pub fn is_on(self) -> bool {
    !matches!(self, Self::Off)
  }
}

/// An item can carry more than one kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum MediaType {
  Music,
  Podcast,
  AudioBook,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct NowPlayingUpdate {
  pub media_item: Option<MediaItemUpdate>,
  pub playback: Option<PlaybackUpdate>,
}

impl NowPlayingUpdate {
  pub fn is_empty(&self) -> bool {
    let media_empty = self.media_item.as_ref().is_none_or(MediaItemUpdate::is_empty);
    let playback_empty = self.playback.as_ref().is_none_or(PlaybackUpdate::is_empty);
    media_empty && playback_empty
  }
}

/// Attributes that change when the track changes.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct MediaItemUpdate {
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
  pub is_resident_on_device: Option<bool>,
  pub chapter_count: Option<u16>,
}

impl MediaItemUpdate {
  pub fn is_empty(&self) -> bool {
    self.persistent_id.is_none()
      && self.title.is_none()
      && self.album.is_none()
      && self.album_uri.is_none()
      && self.album_artist.is_none()
      && self.artist.is_none()
      && self.artist_uri.is_none()
      && self.liked.is_none()
      && self.artwork_id.is_none()
      && self.duration_ms.is_none()
      && self.media_types.is_none()
      && self.track_number.is_none()
      && self.track_count.is_none()
      && self.is_like_supported.is_none()
      && self.is_ban_supported.is_none()
      && self.is_banned.is_none()
      && self.is_resident_on_device.is_none()
      && self.chapter_count.is_none()
  }
}

/// Attributes that change without the track changing.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PlaybackUpdate {
  pub playing: Option<bool>,
  pub position_ms: Option<u32>,
  pub shuffle: Option<bool>,
  pub shuffle_mode: Option<ShuffleMode>,
  pub repeat: Option<RepeatMode>,
  /// For example `com.spotify.client`. Null on Android.
  pub app_bundle: Option<String>,
  pub app_display_name: Option<String>,
  pub queue_index: Option<u32>,
  pub queue_count: Option<u32>,
  pub queue_chapter_index: Option<u32>,
  pub playback_speed: Option<f32>,
  /// False when the app refuses absolute seeks. Null means no signal yet.
  pub set_elapsed_time_available: Option<bool>,
  pub queue_list_avail: Option<bool>,
  pub apple_music_radio_ad: Option<bool>,
  pub apple_music_radio_station_name: Option<String>,
}

impl PlaybackUpdate {
  pub fn is_empty(&self) -> bool {
    self.playing.is_none()
      && self.position_ms.is_none()
      && self.shuffle.is_none()
      && self.shuffle_mode.is_none()
      && self.repeat.is_none()
      && self.app_bundle.is_none()
      && self.app_display_name.is_none()
      && self.queue_index.is_none()
      && self.queue_count.is_none()
      && self.queue_chapter_index.is_none()
      && self.playback_speed.is_none()
      && self.set_elapsed_time_available.is_none()
      && self.queue_list_avail.is_none()
      && self.apple_music_radio_ad.is_none()
      && self.apple_music_radio_station_name.is_none()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_is_empty() {
    let update = NowPlayingUpdate::default();
    assert!(update.is_empty());
  }

  #[test]
  fn populated_media_item_is_not_empty() {
    let update = NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        title: Some("Song".into()),
        ..Default::default()
      }),
      playback: None,
    };
    assert!(!update.is_empty());
  }

  #[test]
  fn empty_inner_groups_count_as_empty() {
    let update = NowPlayingUpdate {
      media_item: Some(MediaItemUpdate::default()),
      playback: Some(PlaybackUpdate::default()),
    };
    assert!(update.is_empty());
  }

  #[test]
  fn json_serialization_skips_none_fields() {
    let update = NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        title: Some("Song".into()),
        ..Default::default()
      }),
      playback: None,
    };
    let json = serde_json::to_string(&update).unwrap();
    assert!(json.contains("\"title\":\"Song\""));
    assert!(!json.contains("artist"));
    assert!(!json.contains("playback"));
  }
}
