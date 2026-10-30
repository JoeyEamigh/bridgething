use libbridgething::{
  PlayerError,
  client::{
    BridgeToClientPlayerMsg, ClientToBridgePlayerMsgDispatch, PlayUri, PlayerErrorReply, PlayerQueueGet,
    PlayerStateGet, PlayerTargetsGet, QueueUri, SeekTo, SetCrossfade, SetRepeat, SetShuffle, SetSpeed, SkipPrev,
    SkipToIndex, TransferTo,
  },
  gateway::{self, BridgeToGatewayPlayerMsgCommand},
};

use super::{HandlerResult, MsgHandle};
use crate::transport::TransportResult;

pub struct PlayerHandler {
  handle: MsgHandle,
}

impl PlayerHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl ClientToBridgePlayerMsgDispatch for PlayerHandler {
  type Output = HandlerResult;

  async fn play(&self, params: PlayUri) -> HandlerResult {
    let outcome = self.handle.transport.play_uri(params.uri, params.context).await;
    self.answer(outcome).await
  }

  async fn queue(&self, params: QueueUri) -> HandlerResult {
    let outcome = self.handle.transport.queue_uri(params.uri, params.position).await;
    self.answer(outcome).await
  }

  async fn pause(&self) -> HandlerResult {
    let outcome = self.handle.transport.pause().await;
    self.answer(outcome).await
  }

  async fn resume(&self) -> HandlerResult {
    let outcome = self.handle.transport.play().await;
    self.answer(outcome).await
  }

  async fn skip_next(&self) -> HandlerResult {
    let outcome = self.handle.transport.next().await;
    self.answer(outcome).await
  }

  async fn skip_prev(&self, params: SkipPrev) -> HandlerResult {
    let outcome = self.handle.transport.prev(params.allow_seeking).await;
    self.answer(outcome).await
  }

  async fn skip_to_index(&self, params: SkipToIndex) -> HandlerResult {
    let outcome = self.handle.transport.skip_to_index(params.index).await;
    self.answer(outcome).await
  }

  async fn seek_to(&self, params: SeekTo) -> HandlerResult {
    let outcome = self.handle.transport.seek_to(params.position_ms).await;
    self.answer(outcome).await
  }

  async fn set_shuffle(&self, params: SetShuffle) -> HandlerResult {
    let outcome = self.handle.transport.set_shuffle(params.on).await;
    self.answer(outcome).await
  }

  async fn set_repeat(&self, params: SetRepeat) -> HandlerResult {
    let outcome = self.handle.transport.set_repeat(params.mode).await;
    self.answer(outcome).await
  }

  async fn set_speed(&self, params: SetSpeed) -> HandlerResult {
    let outcome = self.handle.transport.set_speed(params.speed).await;
    self.answer(outcome).await
  }

  async fn set_crossfade(&self, params: SetCrossfade) -> HandlerResult {
    let outcome = self
      .handle
      .transport
      .forward(
        "set_crossfade",
        BridgeToGatewayPlayerMsgCommand::SetCrossfade(gateway::SetCrossfade {
          duration_ms: params.duration_ms,
        }),
      )
      .await;
    self.answer(outcome).await
  }

  async fn state_get(&self) -> HandlerResult {
    let reply = self.handle.state.player.state_reply();
    self.handle.respond_to::<PlayerStateGet>(reply).await?;
    Ok(())
  }

  async fn queue_get(&self) -> HandlerResult {
    let reply = self.handle.state.player.queue_reply();
    self.handle.respond_to::<PlayerQueueGet>(reply).await?;
    Ok(())
  }

  async fn targets_get(&self) -> HandlerResult {
    let reply = self.handle.state.playback_targets.current();
    self.handle.respond_to::<PlayerTargetsGet>(reply).await?;
    Ok(())
  }

  async fn transfer_to(&self, params: TransferTo) -> HandlerResult {
    if self.handle.state.capabilities.snapshot().gateway.is_none() {
      return self.respond_player_error(PlayerError::NoGateway).await;
    }
    if !self
      .handle
      .state
      .playback_targets
      .current()
      .targets
      .iter()
      .any(|t| t.id == params.target_id)
    {
      return self
        .respond_player_error(PlayerError::UnknownTarget {
          target_id: params.target_id,
        })
        .await;
    }
    let outcome = self
      .handle
      .transport
      .forward(
        "transfer_to",
        BridgeToGatewayPlayerMsgCommand::TransferTo(gateway::TransferTo {
          target_id: params.target_id,
        }),
      )
      .await;
    self.answer(outcome).await
  }
}

impl PlayerHandler {
  async fn answer(&self, outcome: TransportResult) -> HandlerResult {
    match outcome {
      Ok(()) => Ok(()),
      Err(err) => self.respond_player_error(err.into()).await,
    }
  }

  async fn respond_player_error(&self, error: PlayerError) -> HandlerResult {
    self
      .handle
      .respond(BridgeToClientPlayerMsg::ErrorReply(PlayerErrorReply { error }))
      .await?;
    Ok(())
  }
}
