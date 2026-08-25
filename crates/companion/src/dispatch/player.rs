use std::sync::Arc;

use bridgething_gateway::{HandlerError, OutboundLink, OutboundLinkExt, PlayerHandler, Reply};
use libbridgething::{
  PlayerError,
  gateway::{
    GatewayToBridgePlayerMsgEvent, PlayUri, PlayerErrorReply, PlayerSnapshotAck, PlayerSnapshotRequest, QueueUri,
    SeekTo, SetCrossfade, SetRepeat, SetShuffle, SetSpeed, SkipToIndex, TransferTo,
  },
  wire::WireError,
};

use crate::{
  hub::Hub,
  provider::{PlayerTransport, ProviderError, ProviderRegistry},
};

pub struct PlayerDispatcher {
  hub: Arc<Hub>,
  link: Arc<dyn OutboundLink>,
}

impl PlayerDispatcher {
  pub fn new(hub: Arc<Hub>, link: Arc<dyn OutboundLink>) -> Self {
    Self { hub, link }
  }

  async fn report(&self, error: PlayerError) {
    if let Err(failure) = self
      .link
      .event(GatewayToBridgePlayerMsgEvent::ErrorEvent(PlayerErrorReply { error }))
      .await
    {
      tracing::warn!(?failure, "the player error event did not reach the peer");
    }
  }

  async fn failed(&self, verb: &str, error: ProviderError) {
    tracing::warn!(%verb, %error, "player verb failed");
    self
      .report(PlayerError::PlayFailed {
        reason: error.to_string(),
      })
      .await;
  }

  fn scheme(uri: &str) -> String {
    uri.split(':').next().unwrap_or(uri).to_owned()
  }

  fn transport(&self, verb: &str) -> Option<Arc<dyn PlayerTransport>> {
    if let Some(audible) = self.hub.now_playing().current_transport() {
      tracing::debug!(%verb, source = ?self.hub.now_playing().current_source(), "verb routed to the audible source");
      return Some(audible);
    }
    let library = self.hub.library();
    tracing::debug!(
      %verb,
      fallback = ?library.as_ref().map(|provider| provider.name().to_owned()),
      "nothing is audible; verb falls back to the library provider"
    );
    library.map(|provider| provider as Arc<dyn PlayerTransport>)
  }
}

macro_rules! on_transport {
  ($self:ident, $verb:literal, $call:expr) => {{
    let Some(transport) = $self.transport($verb) else {
      $self
        .report(PlayerError::PlayFailed {
          reason: "no active transport".into(),
        })
        .await;
      return Ok(());
    };
    #[allow(clippy::redundant_closure_call)]
    if let Err(error) = ($call)(transport).await {
      $self.failed($verb, error).await;
    }
    Ok(())
  }};
}

impl PlayerHandler for PlayerDispatcher {
  async fn play(&self, payload: PlayUri) -> Result<(), WireError> {
    let Some(provider) = self.hub.for_uri(&payload.uri) else {
      tracing::warn!(uri = %payload.uri, "play dropped: no provider claims the scheme");
      self
        .report(PlayerError::SchemeUnclaimed {
          scheme: Self::scheme(&payload.uri),
        })
        .await;
      return Ok(());
    };
    self.hub.mark_played_from(provider.name());
    if let Err(error) = PlayerTransport::play(provider.as_ref(), payload).await {
      self.failed("play", error).await;
    }
    Ok(())
  }

  async fn queue(&self, payload: QueueUri) -> Result<(), WireError> {
    let Some(provider) = self.hub.for_uri(&payload.uri) else {
      tracing::warn!(uri = %payload.uri, "queue dropped: no provider claims the scheme");
      self
        .report(PlayerError::SchemeUnclaimed {
          scheme: Self::scheme(&payload.uri),
        })
        .await;
      return Ok(());
    };
    if let Err(error) = PlayerTransport::queue(provider.as_ref(), payload).await {
      self.failed("queue", error).await;
    }
    Ok(())
  }

  async fn pause(&self) -> Result<(), WireError> {
    on_transport!(
      self,
      "pause",
      |t: Arc<dyn PlayerTransport>| async move { t.pause().await }
    )
  }

  async fn resume(&self) -> Result<(), WireError> {
    on_transport!(
      self,
      "resume",
      |t: Arc<dyn PlayerTransport>| async move { t.resume().await }
    )
  }

  async fn skip_next(&self) -> Result<(), WireError> {
    on_transport!(self, "skipNext", |t: Arc<dyn PlayerTransport>| async move {
      t.skip_next().await
    })
  }

  async fn skip_prev(&self) -> Result<(), WireError> {
    on_transport!(self, "skipPrev", |t: Arc<dyn PlayerTransport>| async move {
      t.skip_prev().await
    })
  }

  async fn skip_to_index(&self, payload: SkipToIndex) -> Result<(), WireError> {
    on_transport!(self, "skipToIndex", |t: Arc<dyn PlayerTransport>| async move {
      t.skip_to_index(payload.index).await
    })
  }

  async fn seek_to(&self, payload: SeekTo) -> Result<(), WireError> {
    on_transport!(self, "seekTo", |t: Arc<dyn PlayerTransport>| async move {
      t.seek_to(payload.position_ms).await
    })
  }

  async fn set_shuffle(&self, payload: SetShuffle) -> Result<(), WireError> {
    on_transport!(self, "setShuffle", |t: Arc<dyn PlayerTransport>| async move {
      t.set_shuffle(payload.on).await
    })
  }

  async fn set_repeat(&self, payload: SetRepeat) -> Result<(), WireError> {
    on_transport!(self, "setRepeat", |t: Arc<dyn PlayerTransport>| async move {
      t.set_repeat(payload.mode).await
    })
  }

  async fn set_speed(&self, payload: SetSpeed) -> Result<(), WireError> {
    on_transport!(self, "setSpeed", |t: Arc<dyn PlayerTransport>| async move {
      t.set_speed(payload.speed).await
    })
  }

  async fn set_crossfade(&self, payload: SetCrossfade) -> Result<(), WireError> {
    on_transport!(self, "setCrossfade", |t: Arc<dyn PlayerTransport>| async move {
      t.set_crossfade(payload.duration_ms).await
    })
  }

  async fn transfer_to(&self, payload: TransferTo) -> Result<(), WireError> {
    let Some(provider) = self.hub.audible().or_else(|| self.hub.library()) else {
      self
        .report(PlayerError::UnknownTarget {
          target_id: payload.target_id,
        })
        .await;
      return Ok(());
    };
    if let Err(error) = provider.transfer_to(&payload.target_id).await {
      self.failed("transferTo", error).await;
    }
    Ok(())
  }

  async fn snapshot_request(
    &self,
    _request: PlayerSnapshotRequest,
  ) -> Result<Reply<PlayerSnapshotAck>, HandlerError<PlayerErrorReply>> {
    self.hub.now_playing().resync().await;
    Ok(Reply::new(PlayerSnapshotAck {}))
  }
}
