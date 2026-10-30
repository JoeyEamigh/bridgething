use libbridgething::{
  client::{
    AudioErrorReply, BridgeToClientAudioMsgEvent, ClientToBridgeAudioMsgCommandDispatch, Earcon, SetMute, SetVolume,
    Tts, TtsCancel,
  },
  gateway::{self, BridgeToGatewayAudioMsgCommand},
};

use super::{HandlerResult, MsgHandle};
use crate::transport::TransportResult;

pub struct AudioHandler {
  handle: MsgHandle,
}

impl AudioHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }

  async fn answer(&self, outcome: TransportResult) -> HandlerResult {
    let Err(err) = outcome else {
      return Ok(());
    };
    let event = BridgeToClientAudioMsgEvent::ErrorEvent(AudioErrorReply { error: err.into() });
    self.handle.state.bus.send_event(self.handle.from, event).await?;
    Ok(())
  }

  async fn forward(&self, verb: &'static str, cmd: BridgeToGatewayAudioMsgCommand) -> HandlerResult {
    let outcome = self.handle.transport.forward(verb, cmd).await;
    self.answer(outcome).await
  }
}

impl ClientToBridgeAudioMsgCommandDispatch for AudioHandler {
  type Output = HandlerResult;

  async fn volume_up(&self) -> HandlerResult {
    let outcome = self.handle.transport.volume_up().await;
    self.answer(outcome).await
  }

  async fn volume_down(&self) -> HandlerResult {
    let outcome = self.handle.transport.volume_down().await;
    self.answer(outcome).await
  }

  async fn set_volume(&self, params: SetVolume) -> HandlerResult {
    let outcome = self.handle.transport.set_volume(params.level).await;
    self.handle.state.audio.broadcast_current().await?;
    self.answer(outcome).await
  }

  async fn mute_toggle(&self) -> HandlerResult {
    let outcome = self.handle.transport.mute_toggle().await;
    self.answer(outcome).await
  }

  async fn set_mute(&self, params: SetMute) -> HandlerResult {
    let outcome = self.handle.transport.set_mute(params.muted).await;
    self.handle.state.audio.broadcast_current().await?;
    self.answer(outcome).await
  }

  async fn tts(&self, params: Tts) -> HandlerResult {
    self
      .forward(
        "tts",
        BridgeToGatewayAudioMsgCommand::Tts(gateway::Tts {
          id: params.id,
          text: params.text,
          voice: params.voice,
        }),
      )
      .await
  }

  async fn tts_cancel(&self, params: TtsCancel) -> HandlerResult {
    self
      .forward(
        "tts_cancel",
        BridgeToGatewayAudioMsgCommand::TtsCancel(gateway::TtsCancel { id: params.id }),
      )
      .await
  }

  async fn tts_cancel_all(&self) -> HandlerResult {
    self
      .forward("tts_cancel_all", BridgeToGatewayAudioMsgCommand::TtsCancelAll)
      .await
  }

  async fn earcon(&self, params: Earcon) -> HandlerResult {
    self
      .forward(
        "earcon",
        BridgeToGatewayAudioMsgCommand::Earcon(gateway::Earcon { name: params.name }),
      )
      .await
  }
}
