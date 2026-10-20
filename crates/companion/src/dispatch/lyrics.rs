use std::sync::Arc;

use bridgething_gateway::{HandlerError, LyricsHandler, Reply};
use libbridgething::{
  Lyrics,
  gateway::{LyricsErrorReply, LyricsReply, LyricsRequest, TrackIdentity},
};

use crate::{
  cache::response::ResponseCache,
  provider::{ProviderError, ProviderRegistry},
};

const SCOPE: &str = "lyrics";

#[async_trait::async_trait]
pub trait LyricsResolver: Send + Sync {
  async fn lyrics(&self, track: &TrackIdentity) -> Option<Lyrics>;
}

pub struct LyricsDispatcher {
  providers: Arc<dyn ProviderRegistry>,
  resolver: Arc<dyn LyricsResolver>,
  cache: ResponseCache,
}

impl LyricsDispatcher {
  pub fn new(providers: Arc<dyn ProviderRegistry>, resolver: Arc<dyn LyricsResolver>, cache: ResponseCache) -> Self {
    Self {
      providers,
      resolver,
      cache,
    }
  }

  async fn lookup(&self, track: &TrackIdentity) -> Result<Option<Lyrics>, ProviderError> {
    let provider = self.providers.audible().or_else(|| self.providers.library());
    if let Some(provider) = provider {
      match provider.lyrics(track).await {
        Ok(Some(lyrics)) => return Ok(Some(lyrics)),
        Ok(None) | Err(ProviderError::NotImplemented) => {}
        Err(reason) => return Err(reason),
      }
    }
    Ok(self.resolver.lyrics(track).await)
  }
}

impl LyricsHandler for LyricsDispatcher {
  async fn get(&self, request: LyricsRequest) -> Result<Reply<LyricsReply>, HandlerError<LyricsErrorReply>> {
    let lyrics = self
      .cache
      .immutable(SCOPE, &request.track, self.lookup(&request.track))
      .await
      .map_err(|reason| {
        HandlerError::Domain(LyricsErrorReply {
          message: reason.to_string(),
        })
      })?;
    Ok(LyricsReply { lyrics }.into())
  }
}
