use std::time::Duration;

use libbridgething::{
  BrowseResult, LibraryError,
  client::{
    BridgeToClientLibraryMsgEvent, BridgeToClientMsgData, ClientToBridgeLibraryMsgDispatch,
    FavoritesSet as ClientFavoritesSet, FavoritesSetMany as ClientFavoritesSetMany,
    FavoritesToggle as ClientFavoritesToggle, LibraryBrowse, LibraryBrowseReply, LibraryErrorReply,
    LibraryFavoritesContains, LibraryFavoritesContainsReply, LibraryFavoritesList, LibraryFavoritesListReply,
    LibraryRecommendations, LibraryRecommendationsReply, LibraryResolveContext, LibraryResolveContextReply,
    LibrarySearch, LibrarySearchReply,
  },
  gateway::{
    self, BridgeToGatewayLibraryMsgCommand, BrowseReply, FavoritesContainsReply, LibraryBrowseRequest,
    LibraryFavoritesContainsRequest, LibraryFavoritesListRequest, LibraryRecommendationsRequest,
    LibraryResolveContextRequest, LibrarySearchRequest,
  },
  wire::{RequestError, WireRequest},
};

use super::{HandlerResult, MsgHandle};
use crate::{
  bluetooth::{Address, GatewayMan},
  player::{Player, is_synthetic_uri},
  state::{BrowseContentCache, RootBrowseCache},
};

const RECOMMENDATIONS_SEEDS_MAX: usize = 5;
const FAVORITES_CONTAINS_MAX: usize = 50;
const BROWSE_LIMIT_MAX: u32 = 100;
const ROOT_BROWSE_TTL: Duration = Duration::from_secs(300);

pub struct LibraryHandler {
  handle: MsgHandle,
}

impl LibraryHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }

  fn has_gateway(&self) -> bool {
    self.handle.state.capabilities.snapshot().gateway.is_some()
  }

  async fn forward_command(&self, verb: &'static str, cmd: BridgeToGatewayLibraryMsgCommand) -> HandlerResult {
    if let Err(err) = self.handle.transport.forward(verb, cmd).await {
      tracing::debug!(%err, "library {verb} reached no companion");
      let event = BridgeToClientLibraryMsgEvent::ErrorEvent(LibraryErrorReply {
        error: LibraryError::NoGateway,
      });
      self.handle.state.bus.send_event(self.handle.from, event).await?;
    }
    Ok(())
  }

  async fn respond_error<R>(&self, error: LibraryError) -> HandlerResult
  where
    R: WireRequest<Inbound = BridgeToClientMsgData, DomainError = LibraryErrorReply>,
  {
    self
      .handle
      .respond_err::<R>(LibraryErrorReply { error })
      .await
      .map_err(Into::into)
  }

  async fn respond_request_error<R>(&self, verb: &str, err: RequestError<gateway::LibraryErrorReply>) -> HandlerResult
  where
    R: WireRequest<Inbound = BridgeToClientMsgData, DomainError = LibraryErrorReply>,
  {
    let error = match err {
      RequestError::Domain(domain) => domain.error,
      RequestError::Protocol(err) => {
        tracing::warn!(?err, "{verb} protocol error");
        LibraryError::NotSupported {
          reason: format!("{err:?}"),
        }
      }
      RequestError::ResponseMismatch => {
        tracing::error!("{verb} response did not match expected shape");
        LibraryError::NotSupported {
          reason: "response shape mismatch".into(),
        }
      }
    };
    self.respond_error::<R>(error).await
  }
}

impl ClientToBridgeLibraryMsgDispatch for LibraryHandler {
  type Output = HandlerResult;

  async fn browse(&self, params: LibraryBrowse) -> HandlerResult {
    let LibraryBrowse {
      node_id,
      limit,
      offset,
      sections,
      preview,
    } = params;
    if !self.has_gateway() {
      return self.respond_error::<LibraryBrowse>(LibraryError::NoGateway).await;
    }
    let outbound = LibraryBrowseRequest {
      node_id,
      limit: limit.min(BROWSE_LIMIT_MAX),
      offset,
      sections,
      preview,
    };
    match browse_request(
      &self.handle.bluetooth.gateway_man,
      self.handle.state.capabilities.primary_addr(),
      &self.handle.state.root_browse,
      &self.handle.state.browse_content,
      &self.handle.state.player,
      outbound,
    )
    .await
    {
      Ok(reply) => {
        self
          .handle
          .respond_to::<LibraryBrowse>(LibraryBrowseReply { result: reply.result })
          .await?;
      }
      Err(err) => {
        self
          .respond_request_error::<LibraryBrowse>("library.browse", err)
          .await?
      }
    }
    Ok(())
  }

  async fn search(&self, params: LibrarySearch) -> HandlerResult {
    let LibrarySearch {
      query,
      kinds,
      limit,
      offset,
    } = params;
    if !self.has_gateway() {
      return self.respond_error::<LibrarySearch>(LibraryError::NoGateway).await;
    }
    let outbound = LibrarySearchRequest {
      query,
      kinds,
      limit: limit.min(BROWSE_LIMIT_MAX),
      offset,
    };
    let primary = self.handle.state.capabilities.primary_addr();
    match self.handle.bluetooth.gateway_man.request(primary, outbound).await {
      Ok(reply) => {
        self
          .handle
          .respond_to::<LibrarySearch>(LibrarySearchReply { result: reply.result })
          .await?;
      }
      Err(err) => {
        self
          .respond_request_error::<LibrarySearch>("library.search", err)
          .await?
      }
    }
    Ok(())
  }

  async fn recommendations(&self, params: LibraryRecommendations) -> HandlerResult {
    let req = params;
    if !self.has_gateway() {
      return self
        .respond_error::<LibraryRecommendations>(LibraryError::NoGateway)
        .await;
    }
    let mut seeds = req.seeds;
    seeds.truncate(RECOMMENDATIONS_SEEDS_MAX);
    let outbound = LibraryRecommendationsRequest {
      seeds,
      kind: req.kind,
      limit: req.limit.min(BROWSE_LIMIT_MAX),
      offset: req.offset,
    };
    let primary = self.handle.state.capabilities.primary_addr();
    match self.handle.bluetooth.gateway_man.request(primary, outbound).await {
      Ok(reply) => {
        self
          .handle
          .respond_to::<LibraryRecommendations>(LibraryRecommendationsReply { result: reply.result })
          .await?;
      }
      Err(err) => {
        self
          .respond_request_error::<LibraryRecommendations>("library.recommendations", err)
          .await?
      }
    }
    Ok(())
  }

  async fn resolve_context(&self, params: LibraryResolveContext) -> HandlerResult {
    let LibraryResolveContext { uri } = params;
    if !self.has_gateway() {
      return self
        .respond_error::<LibraryResolveContext>(LibraryError::NoGateway)
        .await;
    }
    if is_synthetic_uri(&uri) {
      return self
        .handle
        .respond_to::<LibraryResolveContext>(LibraryResolveContextReply {
          name: None,
          artwork_id: None,
          subtitle: None,
        })
        .await
        .map_err(Into::into);
    }
    match self
      .handle
      .bluetooth
      .gateway_man
      .request(
        self.handle.state.capabilities.primary_addr(),
        LibraryResolveContextRequest { uri },
      )
      .await
    {
      Ok(reply) => {
        self
          .handle
          .respond_to::<LibraryResolveContext>(LibraryResolveContextReply {
            name: reply.name,
            artwork_id: reply.artwork_id,
            subtitle: reply.subtitle,
          })
          .await?;
      }
      Err(err) => {
        self
          .respond_request_error::<LibraryResolveContext>("library.resolveContext", err)
          .await?
      }
    }
    Ok(())
  }

  async fn favorites_list(&self, params: LibraryFavoritesList) -> HandlerResult {
    let LibraryFavoritesList { limit, offset } = params;
    if !self.has_gateway() {
      return self
        .respond_error::<LibraryFavoritesList>(LibraryError::NoGateway)
        .await;
    }
    let outbound = LibraryFavoritesListRequest {
      limit: limit.min(BROWSE_LIMIT_MAX),
      offset,
    };
    let primary = self.handle.state.capabilities.primary_addr();
    match self.handle.bluetooth.gateway_man.request(primary, outbound).await {
      Ok(reply) => {
        self
          .handle
          .respond_to::<LibraryFavoritesList>(LibraryFavoritesListReply { page: reply.page })
          .await?;
      }
      Err(err) => {
        self
          .respond_request_error::<LibraryFavoritesList>("library.favoritesList", err)
          .await?
      }
    }
    Ok(())
  }

  async fn favorites_contains(&self, params: LibraryFavoritesContains) -> HandlerResult {
    let LibraryFavoritesContains { uris } = params;
    if !self.has_gateway() {
      return self
        .respond_error::<LibraryFavoritesContains>(LibraryError::NoGateway)
        .await;
    }
    if uris.is_empty() {
      self
        .handle
        .respond_to::<LibraryFavoritesContains>(LibraryFavoritesContainsReply { liked: Vec::new() })
        .await?;
      return Ok(());
    }
    let mut uris = uris;
    uris.truncate(FAVORITES_CONTAINS_MAX);
    let outbound = LibraryFavoritesContainsRequest { uris };
    match favorites_contains_request(
      &self.handle.bluetooth.gateway_man,
      self.handle.state.capabilities.primary_addr(),
      outbound,
    )
    .await
    {
      Ok(reply) => {
        self
          .handle
          .respond_to::<LibraryFavoritesContains>(LibraryFavoritesContainsReply { liked: reply.liked })
          .await?;
      }
      Err(err) => {
        self
          .respond_request_error::<LibraryFavoritesContains>("library.favoritesContains", err)
          .await?
      }
    }
    Ok(())
  }

  async fn favorites_toggle(&self, params: ClientFavoritesToggle) -> HandlerResult {
    self
      .forward_command(
        "favorites_toggle",
        BridgeToGatewayLibraryMsgCommand::FavoritesToggle(gateway::FavoritesToggle { item: params.item }),
      )
      .await
  }

  async fn favorites_set(&self, params: ClientFavoritesSet) -> HandlerResult {
    self
      .forward_command(
        "favorites_set",
        BridgeToGatewayLibraryMsgCommand::FavoritesSet(gateway::FavoritesSet {
          item: params.item,
          liked: params.liked,
        }),
      )
      .await
  }

  async fn favorites_set_many(&self, params: ClientFavoritesSetMany) -> HandlerResult {
    let entries = params
      .entries
      .into_iter()
      .map(|e| gateway::FavoritesSet {
        item: e.item,
        liked: e.liked,
      })
      .collect();
    self
      .forward_command(
        "favorites_set_many",
        BridgeToGatewayLibraryMsgCommand::FavoritesSetMany(gateway::FavoritesSetMany { entries }),
      )
      .await
  }
}

pub(super) async fn browse_request(
  gateway_man: &GatewayMan,
  primary: Option<Address>,
  root_cache: &RootBrowseCache,
  content_cache: &BrowseContentCache,
  player: &Player,
  req: LibraryBrowseRequest,
) -> Result<BrowseReply, RequestError<gateway::LibraryErrorReply>> {
  if req.node_id.as_deref().is_some_and(is_synthetic_uri) {
    return Ok(BrowseReply {
      result: BrowseResult {
        entries: Vec::new(),
        total: None,
        has_more: false,
      },
    });
  }
  let Some(node_id) = req.node_id.clone() else {
    let shape = (req.sections, req.preview);
    let result = root_cache
      .get_or_fetch(shape, player.root_browse_gen(), ROOT_BROWSE_TTL, || async {
        gateway_man.request(primary, req).await.map(|reply| reply.result)
      })
      .await?;
    return Ok(BrowseReply { result });
  };
  let offset = req.offset;
  let limit = req.limit;
  let result = content_cache
    .get_or_fetch(&node_id, offset, limit, player.root_browse_gen(), || async move {
      gateway_man.request(primary, req).await.map(|reply| reply.result)
    })
    .await?;
  Ok(BrowseReply { result })
}

pub(super) async fn favorites_contains_request(
  gateway_man: &GatewayMan,
  primary: Option<Address>,
  req: LibraryFavoritesContainsRequest,
) -> Result<FavoritesContainsReply, RequestError<gateway::LibraryErrorReply>> {
  if !req.uris.is_empty() && req.uris.iter().all(|uri| is_synthetic_uri(uri)) {
    return Ok(FavoritesContainsReply {
      liked: vec![false; req.uris.len()],
    });
  }
  gateway_man.request(primary, req).await
}
