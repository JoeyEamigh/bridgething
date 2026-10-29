use libbridgething::{HostPresence, gateway::GatewayToBridgePresenceMsgEventDispatch};

use super::{HandlerResult, MsgHandle};

pub struct PresenceHandler {
  handle: MsgHandle,
}

impl PresenceHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl GatewayToBridgePresenceMsgEventDispatch for PresenceHandler {
  type Output = HandlerResult;

  async fn update(&self, params: HostPresence) -> HandlerResult {
    self.handle.state.presence.update(params);
    Ok(())
  }
}
