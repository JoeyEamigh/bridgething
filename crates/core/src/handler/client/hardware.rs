use libbridgething::client::{
  BridgeToClientHardwareMsg, ClientToBridgeHardwareMsgDispatch, DisplaySetLevel, DisplaySetMode,
};

use super::{HandlerResult, MsgHandle};
use crate::display::SetResult;

pub struct HardwareHandler {
  handle: MsgHandle,
}

impl HardwareHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }

  fn report(&self, verb: &str, result: SetResult) {
    match result {
      Ok(Ok(())) => {}
      Ok(Err(err)) => tracing::debug!("({}) hardware.{verb} rejected: {err:?}", &self.handle.from),
      Err(err) => tracing::warn!("({}) hardware.{verb} failed: {err}", &self.handle.from),
    }
  }
}

impl ClientToBridgeHardwareMsgDispatch for HardwareHandler {
  type Output = HandlerResult;

  async fn display_set_mode(&self, params: DisplaySetMode) -> HandlerResult {
    self.report("displaySetMode", self.handle.state.display.set_mode(params.mode).await);
    Ok(())
  }

  async fn display_set_level(&self, params: DisplaySetLevel) -> HandlerResult {
    self.report(
      "displaySetLevel",
      self.handle.state.display.set_level(params.level).await,
    );
    Ok(())
  }

  async fn state_get(&self) -> HandlerResult {
    let reply = self.handle.state.display.snapshot_reply();
    self
      .handle
      .respond(BridgeToClientHardwareMsg::StateReply(reply))
      .await?;
    Ok(())
  }
}
