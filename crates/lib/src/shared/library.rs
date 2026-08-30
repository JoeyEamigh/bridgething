use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{Album, Artist, Track};

/// `search` and `recommendations` take a list of these to constrain their results.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum ItemKind {
  Track,
  Album,
  Playlist,
  PodcastEpisode,
  Show,
  Artist,
  Station,
}

/// A reference to a library item. Pass it to `favoritesToggle`, or pass its `uri` to `player.play`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct ItemRef {
  pub uri: String,
  pub kind: ItemKind,
  /// Opaque. Null when the source has none.
  pub persistent_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Playlist {
  pub uri: String,
  pub name: String,
  /// Who the source credits, such as an owner or a curator.
  pub owner_name: Option<String>,
  /// Null when the source reports no count.
  pub track_count: Option<u32>,
  pub artwork_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct PodcastEpisode {
  pub uri: String,
  pub name: String,
  pub show_name: Option<String>,
  pub duration_ms: Option<u32>,
  /// Null when the source reports none.
  pub published_at_unix_s: Option<u32>,
  pub artwork_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Show {
  pub uri: String,
  pub name: String,
  pub publisher: Option<String>,
  /// Null when the source reports no count.
  pub episode_count: Option<u32>,
  pub artwork_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct Station {
  pub uri: String,
  pub name: String,
  /// URI the station was built from, such as an artist or a track.
  pub seed: Option<String>,
  pub artwork_id: Option<String>,
}

/// Branch on `type` to read the payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum LibraryItem {
  Track(Track),
  Album(Album),
  Playlist(Playlist),
  PodcastEpisode(PodcastEpisode),
  Show(Show),
  Artist(Artist),
  Station(Station),
}

/// `nodeId` of the recently played folder in a root browse result.
pub const RECENTS_NODE_ID: &str = "recently-played";

/// A folder can be browsed further; an item can be played.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum BrowseEntry {
  Folder(BrowseFolder),
  Item(LibraryItem),
}

/// Pass its `nodeId` to `browse` to descend into it.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct BrowseFolder {
  pub node_id: String,
  pub title: String,
  pub subtitle: Option<String>,
  pub artwork_id: Option<String>,
  /// Null when the source reports no count.
  pub total: Option<u32>,
  /// The first few children, when the source returns them alongside the folder.
  pub preview_children: Option<Vec<BrowseEntry>>,
}

/// Page by raising `offset` while `hasMore` is true.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct BrowseResult {
  pub entries: Vec<BrowseEntry>,
  pub total: Option<u32>,
  pub has_more: bool,
}

/// Ranked best first. Page by raising `offset` while `hasMore` is true.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct SearchResult {
  pub items: Vec<LibraryItem>,
  /// The kinds the search honored. Compare it against the kinds you asked for.
  pub kinds: Vec<ItemKind>,
  pub total: Option<u32>,
  pub has_more: bool,
}

/// Page by raising `offset` while `hasMore` is true.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct RecommendationsResult {
  pub items: Vec<LibraryItem>,
  pub total: Option<u32>,
  pub has_more: bool,
}

/// Mixed kind. Page by raising `offset` while `hasMore` is true.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct FavoritesPage {
  pub items: Vec<LibraryItem>,
  pub total: Option<u32>,
  pub has_more: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum LibraryError {
  /// The uri or node id names nothing in the library.
  NotFound { uri: String },
  /// The music source offers no such operation.
  NotSupported { reason: String },
  /// The signed-in account permits no such operation.
  Unauthorized,
  /// No phone is connected.
  NoGateway,
}
