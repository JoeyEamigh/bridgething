use serde::{Deserialize, Serialize};

/// Stock Spotify webapp operations. Send one as a `legacyStock` message.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", content = "args", rename_all = "camelCase")]
pub enum ClientLegacyStockCommand {
  GetImage {
    id: String,
  },
  GetThumbnailImage {
    id: String,
  },
  GetNextTracks,

  // spotify things
  SpotifyGetChildren {
    parent_id: String,
    limit: usize,
    offset: Option<usize>,
  },
  SpotifyGetPodcast {
    uri: String,
    limit: Option<usize>,
    offset: Option<usize>,
  },
  SpotifyGetSaved {
    id: String,
  },
  SpotifyPlayPodcastTrailer {
    uri: String,
  },
  SpotifyQueueUri {
    uri: String,
  },
  SpotifySetPodcastPlaybackSpeed {
    playback_speed: usize,
  },
  SpotifySetSaved {
    id: Option<String>, // id is same as uri
    uri: Option<String>,
    saved: bool,
  },
  SpotifyPlayUri {
    uri: String,
    feature_identifier: String,
    interaction_id: Option<String>,
    skip_to_uri: Option<String>,
    skip_to_uid: Option<String>,
  },

  SpotifyGetPermissions,
  SpotifyGetPlayerState,
  SpotifyGetSessionState,
  SpotifySummonDj,
  SpotifyGetHome {
    limit: usize,
    limit_overrides: std::collections::HashMap<String, usize>,
  },
  SpotifyGetPresets,
  SpotifySetPreset {
    presets: Vec<crate::stock::StockSetPreset>,
  },
  SpotifyGetTips,
  SpotifyGraphql {
    payload: String,
  },
  SuperbirdPhoneCallImage {
    phone_number: String,
  },
}
