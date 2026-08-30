use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{BrowseResult, FavoritesPage, LibraryError, RecommendationsResult, SearchResult};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LibraryBrowseReply {
  pub result: BrowseResult,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LibrarySearchReply {
  pub result: SearchResult,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LibraryRecommendationsReply {
  pub result: RecommendationsResult,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// Fields are null when the companion app cannot name the uri.
pub struct LibraryResolveContextReply {
  pub name: Option<String>,
  pub artwork_id: Option<String>,
  pub subtitle: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LibraryFavoritesListReply {
  pub page: FavoritesPage,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LibraryFavoritesContainsReply {
  /// Lines up with the `uris` you sent.
  pub liked: Vec<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct LibraryErrorReply {
  pub error: LibraryError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// Fires for changes made through this client and for changes made in the phone's own music app.
pub struct FavoriteChanged {
  pub uri: String,
  pub liked: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// The music library on the connected phone. `browse`, `search`, and `favoritesList` read it,
/// `favoritesToggle` and `favoritesSet` edit saved state, and `onFavoriteChanged` reports every change.
pub enum BridgeToClientLibraryMsg {
  #[bridge_response]
  BrowseReply(LibraryBrowseReply),
  #[bridge_response]
  SearchReply(LibrarySearchReply),
  #[bridge_response]
  RecommendationsReply(LibraryRecommendationsReply),
  #[bridge_response]
  ResolveContextReply(LibraryResolveContextReply),
  #[bridge_response]
  FavoritesListReply(LibraryFavoritesListReply),
  #[bridge_response]
  FavoritesContainsReply(LibraryFavoritesContainsReply),
  #[bridge_response]
  ErrorReply(LibraryErrorReply),
  #[bridge_event]
  FavoriteChanged(FavoriteChanged),
  #[bridge_event]
  /// A `favoritesToggle`, `favoritesSet`, or `favoritesSetMany` command failed.
  ErrorEvent(LibraryErrorReply),
}
