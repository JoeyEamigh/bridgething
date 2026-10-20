use bridgething_macros::BridgeEnum;
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
  BrowseResult, FavoritesPage, LibraryError, RecommendationsResult, SearchResult, gateway::LibraryBrowseRequest,
};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct BrowseReply {
  pub result: BrowseResult,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct ContextResolveReply {
  pub name: Option<String>,
  pub artwork_id: Option<String>,
  pub subtitle: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct SearchReply {
  pub result: SearchResult,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct RecommendationsReply {
  pub result: RecommendationsResult,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct FavoritesListReply {
  pub page: FavoritesPage,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct FavoritesContainsReply {
  pub liked: Vec<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct LibraryErrorReply {
  pub error: LibraryError,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct FavoriteChanged {
  pub uri: String,
  pub liked: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub enum LibraryScope {
  Saved,
  Playlists,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct LibraryChanged {
  pub scope: LibraryScope,
}

/// A browse page the companion answered from its own cache came back different once the network caught
/// up. `page` is the request exactly as the daemon sent it.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct BrowseChanged {
  pub page: LibraryBrowseRequest,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[bridge_enum(into = crate::gateway::GatewayToBridgeMsgData)]
pub enum GatewayToBridgeLibraryMsg {
  #[bridge_response]
  BrowseReply(BrowseReply),
  #[bridge_response]
  ContextResolveReply(ContextResolveReply),
  #[bridge_response]
  SearchReply(SearchReply),
  #[bridge_response]
  RecommendationsReply(RecommendationsReply),
  #[bridge_response]
  FavoritesListReply(FavoritesListReply),
  #[bridge_response]
  FavoritesContainsReply(FavoritesContainsReply),
  #[bridge_response]
  ErrorReply(LibraryErrorReply),
  #[bridge_event]
  FavoriteChanged(FavoriteChanged),
  #[bridge_event]
  LibraryChanged(LibraryChanged),
  #[bridge_event]
  BrowseChanged(BrowseChanged),
  #[bridge_event]
  ErrorEvent(LibraryErrorReply),
}
