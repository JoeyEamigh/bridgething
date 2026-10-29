use libbridgething::client::{BridgeToClientPresenceMsg, ClientToBridgePresenceMsgRequestDispatch};

use super::{HandlerResult, MsgHandle};

pub struct PresenceHandler {
  handle: MsgHandle,
}

impl PresenceHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl ClientToBridgePresenceMsgRequestDispatch for PresenceHandler {
  type Output = HandlerResult;

  async fn get(&self) -> HandlerResult {
    let snapshot = self.handle.state.presence.snapshot();
    self
      .handle
      .respond(BridgeToClientPresenceMsg::Snapshot(snapshot))
      .await?;
    Ok(())
  }
}
