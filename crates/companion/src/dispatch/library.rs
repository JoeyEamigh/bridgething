use std::{collections::HashMap, future::Future, sync::Arc};

use bridgething_gateway::{HandlerError, LibraryHandler, OutboundLink, OutboundLinkExt, Reply};
use futures::StreamExt;
use libbridgething::{
  BrowseEntry, LibraryError, LibraryItem, Priority,
  gateway::{
    BrowseChanged, BrowseReply, ContextResolveReply, FavoritesContainsReply, FavoritesListReply, FavoritesSet,
    FavoritesSetMany, FavoritesToggle, GatewayToBridgeLibraryMsgEvent, LibraryBrowseRequest, LibraryErrorReply,
    LibraryFavoritesContainsRequest, LibraryFavoritesListRequest, LibraryRecommendationsRequest, LibrarySearchRequest,
    RecommendationsReply, SearchReply,
  },
  protocol::Compress,
  wire::WireError,
};

use crate::{
  cache::response::ResponseCache,
  provider::{Provider, ProviderError, ProviderRegistry},
};

const WARM_CONCURRENCY: usize = 4;

fn is_root(node_id: Option<&str>) -> bool {
  matches!(node_id, None | Some("") | Some("root"))
}

fn item_art(item: &LibraryItem) -> Option<&str> {
  match item {
    LibraryItem::Track(track) => Some(track.image_id.as_str()),
    LibraryItem::Playlist(playlist) => playlist.artwork_id.as_deref(),
    LibraryItem::PodcastEpisode(episode) => episode.artwork_id.as_deref(),
    LibraryItem::Show(show) => show.artwork_id.as_deref(),
    LibraryItem::Station(station) => station.artwork_id.as_deref(),
    LibraryItem::Album(album) => album.artwork_id.as_deref(),
    LibraryItem::Artist(artist) => artist.artwork_id.as_deref(),
  }
}

fn entry_art<'a>(entries: &'a [BrowseEntry], out: &mut Vec<&'a str>) {
  for entry in entries {
    match entry {
      BrowseEntry::Folder(folder) => {
        out.extend(folder.artwork_id.as_deref());
        if let Some(children) = &folder.preview_children {
          entry_art(children, out);
        }
      }
      BrowseEntry::Item(item) => out.extend(item_art(item)),
    }
  }
}

fn warm<'a>(provider: &Arc<dyn Provider>, ids: impl IntoIterator<Item = &'a str>) {
  let mut ids: Vec<String> = ids.into_iter().filter(|id| !id.is_empty()).map(str::to_owned).collect();
  ids.sort();
  ids.dedup();
  if ids.is_empty() {
    return;
  }
  let provider = provider.clone();
  tokio::spawn(async move {
    futures::stream::iter(ids)
      .for_each_concurrent(WARM_CONCURRENCY, |id| {
        let provider = provider.clone();
        async move {
          if let Err(reason) = provider.asset(&id).await {
            tracing::debug!(%id, %reason, "art warm missed");
          }
        }
      })
      .await;
  });
}

pub struct LibraryDispatcher {
  providers: Arc<dyn ProviderRegistry>,
  link: Arc<dyn OutboundLink>,
  cache: ResponseCache,
}

impl LibraryDispatcher {
  pub fn new(providers: Arc<dyn ProviderRegistry>, link: Arc<dyn OutboundLink>, cache: ResponseCache) -> Self {
    Self { providers, link, cache }
  }

  async fn read<K, T, F, Fut, C>(
    &self,
    kind: &str,
    key: K,
    fetch: F,
    art: fn(&T) -> Vec<&str>,
    changed: C,
  ) -> Result<T, HandlerError<LibraryErrorReply>>
  where
    K: serde::Serialize + Send + 'static,
    T: serde::Serialize + serde::de::DeserializeOwned + Send + 'static,
    F: FnOnce(Arc<dyn Provider>, K) -> Fut,
    Fut: Future<Output = Result<T, ProviderError>> + Send + 'static,
    C: Future<Output = ()> + Send + 'static,
  {
    let provider = self.active()?;
    let scope = provider.name().to_owned();
    let cache_key = (kind, &key);
    let cache_key =
      serde_json::to_value(cache_key).map_err(|error| refusal(ProviderError::Failed(error.to_string())))?;
    let pending = fetch(provider.clone(), key);
    let live = async move {
      let value = pending.await?;
      warm(&provider, art(&value));
      Ok(value)
    };
    self
      .cache
      .revalidate(&scope, &cache_key, live, changed)
      .await
      .map_err(refusal)
  }

  fn announce(&self, page: LibraryBrowseRequest) -> impl Future<Output = ()> + Send + 'static {
    let link = self.link.clone();
    async move {
      if let Err(failure) = link
        .event(GatewayToBridgeLibraryMsgEvent::BrowseChanged(BrowseChanged { page }))
        .await
      {
        tracing::warn!(?failure, "the browse change did not reach the peer");
      }
    }
  }

  fn active(&self) -> Result<Arc<dyn Provider>, HandlerError<LibraryErrorReply>> {
    self.providers.library().ok_or_else(|| {
      HandlerError::Domain(LibraryErrorReply {
        error: LibraryError::NotSupported {
          reason: "no active music provider".into(),
        },
      })
    })
  }

  fn owner(&self, uri: &str) -> Option<Arc<dyn Provider>> {
    self.providers.for_uri(uri).or_else(|| self.providers.library())
  }

  async fn report(&self, error: LibraryError) {
    if let Err(failure) = self
      .link
      .event(GatewayToBridgeLibraryMsgEvent::ErrorEvent(LibraryErrorReply { error }))
      .await
    {
      tracing::warn!(?failure, "the library error event did not reach the peer");
    }
  }

  async fn report_write(&self, error: ProviderError) {
    match refusal(error) {
      HandlerError::Domain(reply) => self.report(reply.error).await,
      HandlerError::Wire(_) => {
        self
          .report(LibraryError::NotSupported {
            reason: "unimplemented".into(),
          })
          .await
      }
    }
  }
}

fn refusal(error: ProviderError) -> HandlerError<LibraryErrorReply> {
  let error = match error {
    ProviderError::NotImplemented => return HandlerError::Wire(WireError::Unimplemented),
    ProviderError::NotAuthenticated => LibraryError::Unauthorized,
    ProviderError::Detached => LibraryError::NotSupported {
      reason: "the music provider detached".into(),
    },
    ProviderError::Failed(reason) => LibraryError::NotSupported { reason },
  };
  HandlerError::Domain(LibraryErrorReply { error })
}

impl LibraryHandler for LibraryDispatcher {
  async fn browse(&self, request: LibraryBrowseRequest) -> Result<Reply<BrowseReply>, HandlerError<LibraryErrorReply>> {
    let root = is_root(request.node_id.as_deref());
    let changed = self.announce(request.clone());
    let result = self
      .read(
        "browse",
        request,
        |provider, request| async move { provider.browse(request).await },
        |result| {
          let mut ids = Vec::new();
          entry_art(&result.entries, &mut ids);
          ids
        },
        changed,
      )
      .await?;
    let reply = Reply::new(BrowseReply { result });
    Ok(if root {
      reply.lane(Priority::Bulk).compressed(Compress::Always)
    } else {
      reply
    })
  }

  async fn resolve_context(
    &self,
    request: libbridgething::gateway::LibraryResolveContextRequest,
  ) -> Result<Reply<ContextResolveReply>, HandlerError<LibraryErrorReply>> {
    let resolved = self
      .read(
        "context",
        request.uri,
        |provider, uri| async move { provider.resolve_context(&uri).await },
        |resolved| resolved.artwork_id.as_deref().into_iter().collect(),
        async {},
      )
      .await?;
    Ok(resolved.into())
  }

  async fn search(&self, request: LibrarySearchRequest) -> Result<Reply<SearchReply>, HandlerError<LibraryErrorReply>> {
    let result = self
      .read(
        "search",
        request,
        |provider, request| async move { provider.search(request).await },
        |result| result.items.iter().filter_map(item_art).collect(),
        async {},
      )
      .await?;
    Ok(SearchReply { result }.into())
  }

  async fn recommendations(
    &self,
    request: LibraryRecommendationsRequest,
  ) -> Result<Reply<RecommendationsReply>, HandlerError<LibraryErrorReply>> {
    let result = self
      .read(
        "recommendations",
        request,
        |provider, request| async move { provider.recommendations(request).await },
        |result| result.items.iter().filter_map(item_art).collect(),
        async {},
      )
      .await?;
    Ok(RecommendationsReply { result }.into())
  }

  async fn favorites_list(
    &self,
    request: LibraryFavoritesListRequest,
  ) -> Result<Reply<FavoritesListReply>, HandlerError<LibraryErrorReply>> {
    let page = self
      .read(
        "favorites",
        request,
        |provider, request| async move { provider.favorites_list(request).await },
        |page| page.items.iter().filter_map(item_art).collect(),
        async {},
      )
      .await?;
    Ok(FavoritesListReply { page }.into())
  }

  async fn favorites_contains(
    &self,
    request: LibraryFavoritesContainsRequest,
  ) -> Result<Reply<FavoritesContainsReply>, HandlerError<LibraryErrorReply>> {
    let liked = self.active()?.favorites_contains(request).await.map_err(refusal)?;
    Ok(FavoritesContainsReply { liked }.into())
  }

  async fn favorites_toggle(&self, payload: FavoritesToggle) -> Result<(), WireError> {
    let Some(provider) = self.owner(&payload.item.uri) else {
      self.report(LibraryError::NoGateway).await;
      return Ok(());
    };
    if let Err(error) = provider.favorites_toggle(payload.item).await {
      self.report_write(error).await;
    }
    Ok(())
  }

  async fn favorites_set(&self, payload: FavoritesSet) -> Result<(), WireError> {
    let Some(provider) = self.owner(&payload.item.uri) else {
      self.report(LibraryError::NoGateway).await;
      return Ok(());
    };
    if let Err(error) = provider.favorites_set(payload.item, payload.liked).await {
      self.report_write(error).await;
    }
    Ok(())
  }

  async fn favorites_set_many(&self, payload: FavoritesSetMany) -> Result<(), WireError> {
    let mut grouped: HashMap<String, (Arc<dyn Provider>, Vec<FavoritesSet>)> = HashMap::new();
    for entry in payload.entries {
      let Some(provider) = self.owner(&entry.item.uri) else {
        self.report(LibraryError::NoGateway).await;
        continue;
      };
      grouped
        .entry(provider.name().to_owned())
        .or_insert_with(|| (provider, Vec::new()))
        .1
        .push(entry);
    }

    for (provider, entries) in grouped.into_values() {
      if let Err(error) = provider.favorites_set_many(entries).await {
        self.report_write(error).await;
      }
    }
    Ok(())
  }
}
