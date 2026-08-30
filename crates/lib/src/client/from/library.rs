use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ItemKind, ItemRef};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Library,
  request_variant = Browse,
  response = crate::client::LibraryBrowseReply,
  response_variant = BrowseReply,
  error = crate::client::LibraryErrorReply,
  error_variant = ErrorReply,
)]
/// Pages through one folder of the library tree, or the root menu. Root results are held for 5 minutes.
pub struct LibraryBrowse {
  /// A `nodeId` from an earlier result. Null browses the root.
  pub node_id: Option<String>,
  /// Capped at 100.
  pub limit: u32,
  pub offset: u32,
  /// Root only. Null returns every folder.
  #[serde(default)]
  pub sections: Option<u32>,
  /// Root only. Preview children per folder; `0` returns ids and titles only.
  #[serde(default)]
  pub preview: Option<u32>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Library,
  request_variant = Search,
  response = crate::client::LibrarySearchReply,
  response_variant = SearchReply,
  error = crate::client::LibraryErrorReply,
  error_variant = ErrorReply,
)]
pub struct LibrarySearch {
  pub query: String,
  /// Null searches every kind.
  pub kinds: Option<Vec<ItemKind>>,
  /// Capped at 100.
  pub limit: u32,
  pub offset: u32,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Library,
  request_variant = Recommendations,
  response = crate::client::LibraryRecommendationsReply,
  response_variant = RecommendationsReply,
  error = crate::client::LibraryErrorReply,
  error_variant = ErrorReply,
)]
pub struct LibraryRecommendations {
  /// Only the first 5 are used.
  pub seeds: Vec<ItemRef>,
  /// Null lets the companion app choose from the seeds.
  pub kind: Option<ItemKind>,
  /// Capped at 100.
  pub limit: u32,
  pub offset: u32,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Library,
  request_variant = ResolveContext,
  response = crate::client::LibraryResolveContextReply,
  response_variant = ResolveContextReply,
  error = crate::client::LibraryErrorReply,
  error_variant = ErrorReply,
)]
/// Resolves a context uri, such as `PlayerState.context.uri`, into a name and an artwork id.
pub struct LibraryResolveContext {
  pub uri: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Library,
  request_variant = FavoritesList,
  response = crate::client::LibraryFavoritesListReply,
  response_variant = FavoritesListReply,
  error = crate::client::LibraryErrorReply,
  error_variant = ErrorReply,
)]
/// Pages the user's saved items, mixed across kinds.
pub struct LibraryFavoritesList {
  /// Capped at 100.
  pub limit: u32,
  pub offset: u32,
}

/// Checks which uris the user has saved. The reply's `liked` lines up with `uris`.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireRequest)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[wire_request(
  direction = ClientToBridge,
  surface = Library,
  request_variant = FavoritesContains,
  response = crate::client::LibraryFavoritesContainsReply,
  response_variant = FavoritesContainsReply,
  error = crate::client::LibraryErrorReply,
  error_variant = ErrorReply,
)]
pub struct LibraryFavoritesContains {
  /// Only the first 50 are used.
  pub uris: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct FavoritesToggle {
  pub item: ItemRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct FavoritesSet {
  pub item: ItemRef,
  pub liked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct FavoritesSetMany {
  pub entries: Vec<FavoritesSet>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::ClientToBridgeMsgData)]
/// Browses, searches, and edits the music library on the connected phone.
pub enum ClientToBridgeLibraryMsg {
  #[bridge_request]
  Browse(LibraryBrowse),
  #[bridge_request]
  Search(LibrarySearch),
  #[bridge_request]
  Recommendations(LibraryRecommendations),
  #[bridge_request]
  ResolveContext(LibraryResolveContext),
  #[bridge_request]
  FavoritesList(LibraryFavoritesList),
  #[bridge_request]
  FavoritesContains(LibraryFavoritesContains),
  #[bridge_command]
  FavoritesToggle(FavoritesToggle),
  #[bridge_command]
  FavoritesSet(FavoritesSet),
  #[bridge_command]
  FavoritesSetMany(FavoritesSetMany),
}
