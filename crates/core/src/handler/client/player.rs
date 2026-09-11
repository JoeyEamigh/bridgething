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
use crate::{bluetooth::Address, capabilities::is_valid_scheme};

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
    let PlayUri { uri, context } = params;
    let claimant = match self.claimant_of(&uri).await {
      Ok(claimant) => claimant,
      Err(rejected) => return rejected,
    };
    self
      .handle
      .bluetooth
      .gateway_man
      .send_command(
        claimant,
        BridgeToGatewayPlayerMsgCommand::Play(gateway::PlayUri { uri, context }),
      )
      .await;
    Ok(())
  }

  async fn queue(&self, params: QueueUri) -> HandlerResult {
    let QueueUri { uri, position } = params;
    let claimant = match self.claimant_of(&uri).await {
      Ok(claimant) => claimant,
      Err(rejected) => return rejected,
    };
    self
      .handle
      .bluetooth
      .gateway_man
      .send_command(
        claimant,
        BridgeToGatewayPlayerMsgCommand::Queue(gateway::QueueUri { uri, position }),
      )
      .await;
    Ok(())
  }

  async fn pause(&self) -> HandlerResult {
    self.handle.transport.pause().await;
    Ok(())
  }

  async fn resume(&self) -> HandlerResult {
    self.handle.transport.play().await;
    Ok(())
  }

  async fn skip_next(&self) -> HandlerResult {
    self.handle.transport.next().await;
    Ok(())
  }

  async fn skip_prev(&self, params: SkipPrev) -> HandlerResult {
    self.handle.transport.prev(params.allow_seeking).await;
    Ok(())
  }

  async fn skip_to_index(&self, params: SkipToIndex) -> HandlerResult {
    self.handle.transport.skip_to_index(params.index).await;
    Ok(())
  }

  async fn seek_to(&self, params: SeekTo) -> HandlerResult {
    self.handle.transport.seek_to(params.position_ms).await;
    Ok(())
  }

  async fn set_shuffle(&self, params: SetShuffle) -> HandlerResult {
    self.handle.transport.set_shuffle(params.on).await;
    Ok(())
  }

  async fn set_repeat(&self, params: SetRepeat) -> HandlerResult {
    self.handle.transport.set_repeat(params.mode).await;
    Ok(())
  }

  async fn set_speed(&self, params: SetSpeed) -> HandlerResult {
    self
      .forward_command(BridgeToGatewayPlayerMsgCommand::SetSpeed(gateway::SetSpeed {
        speed: params.speed,
      }))
      .await
  }

  async fn set_crossfade(&self, params: SetCrossfade) -> HandlerResult {
    self
      .forward_command(BridgeToGatewayPlayerMsgCommand::SetCrossfade(gateway::SetCrossfade {
        duration_ms: params.duration_ms,
      }))
      .await
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
    self
      .forward_command(BridgeToGatewayPlayerMsgCommand::TransferTo(gateway::TransferTo {
        target_id: params.target_id,
      }))
      .await
  }
}

impl PlayerHandler {
  async fn claimant_of(&self, uri: &str) -> Result<Address, HandlerResult> {
    let capabilities = &self.handle.state.capabilities;
    let Some(scheme) = scheme_of(uri) else {
      return Err(
        self
          .respond_player_error(PlayerError::SchemeUnclaimed {
            scheme: uri.to_string(),
          })
          .await,
      );
    };
    if capabilities.snapshot().gateway.is_none() {
      return Err(self.respond_player_error(PlayerError::NoGateway).await);
    }
    match capabilities.companion_for_scheme(&scheme) {
      Some(claimant) => Ok(claimant),
      None => Err(self.respond_player_error(PlayerError::SchemeUnclaimed { scheme }).await),
    }
  }

  async fn forward_command<C>(&self, cmd: C) -> HandlerResult
  where
    C: libbridgething::wire::WireCommand<libbridgething::gateway::BridgeToGatewayMsgData>,
  {
    match self.handle.state.capabilities.primary_addr() {
      Some(primary) => self.handle.bluetooth.gateway_man.send_command(primary, cmd).await,
      None => return self.respond_player_error(PlayerError::NoGateway).await,
    }
    Ok(())
  }

  async fn respond_player_error(&self, error: PlayerError) -> HandlerResult {
    self
      .handle
      .respond(BridgeToClientPlayerMsg::ErrorReply(PlayerErrorReply { error }))
      .await?;
    Ok(())
  }
}

fn scheme_of(uri: &str) -> Option<String> {
  let (head, _) = uri.split_once(':')?;
  let scheme = head.to_ascii_lowercase();
  is_valid_scheme(&scheme).then_some(scheme)
}

#[cfg(test)]
mod tests {
  use super::{is_valid_scheme, scheme_of};

  #[test]
  fn announce_and_play_share_one_grammar() {
    for raw in [
      "spotify",
      "Spotify",
      "apple-music",
      "ok+v.2",
      "1bad",
      "",
      "bad space",
      "x",
    ] {
      assert_eq!(
        is_valid_scheme(raw),
        scheme_of(&format!("{raw}:track:abc")).is_some(),
        "announce-time and play-time scheme grammars disagree on {raw:?}"
      );
    }
  }

  #[test]
  fn parses_simple_scheme() {
    assert_eq!(scheme_of("spotify:track:abc"), Some("spotify".into()));
  }

  #[test]
  fn lowercases() {
    assert_eq!(scheme_of("Spotify:track:abc"), Some("spotify".into()));
  }

  #[test]
  fn rejects_no_colon() {
    assert_eq!(scheme_of("spotifytrackabc"), None);
  }

  #[test]
  fn rejects_leading_digit() {
    assert_eq!(scheme_of("4spotify:track:abc"), None);
  }

  #[test]
  fn allows_compound_chars() {
    assert_eq!(scheme_of("apple-music:track:abc"), Some("apple-music".into()));
  }
}
