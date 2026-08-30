use libbridgething::{
  ForwardRouted,
  gateway::{ExtensionsRunning, GatewayToBridgeForwardMsgEventDispatch},
};

use super::{HandlerResult, MsgHandle};

pub struct ForwardHandler {
  handle: MsgHandle,
}

impl ForwardHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl GatewayToBridgeForwardMsgEventDispatch for ForwardHandler {
  type Output = HandlerResult;

  async fn routed(&self, params: ForwardRouted) -> HandlerResult {
    let active = self.handle.state.active_webapp().await?;
    if active != Some(params.webapp) {
      tracing::debug!(
        webapp = %params.webapp,
        ?active,
        "dropping a gateway forward addressed to a webapp that is not active"
      );
      return Ok(());
    }
    if let Err(errs) = self.handle.state.bus.broadcast_event(params.message).await {
      for err in errs {
        tracing::warn!(?err, "a client missed a forward");
      }
    }
    Ok(())
  }

  async fn extensions_running(&self, params: ExtensionsRunning) -> HandlerResult {
    let Some(address) = self.handle.address else {
      tracing::debug!("ignoring an extensions-running report from an unaddressed gateway");
      return Ok(());
    };
    tracing::debug!(
      ?address,
      count = params.webapps.len(),
      "gateway reported running extensions"
    );
    self.handle.state.note_extensions_running(address, params.webapps).await;
    Ok(())
  }
}
