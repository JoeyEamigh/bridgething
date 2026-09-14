use libbridgething::{
  client::{
    BridgeToClientInputMsg, ClientToBridgeInputMsgDispatch, LauncherGestureChanged, LauncherGestureGet,
    LauncherGestureReply, LauncherGestureSet,
  },
  wire::MsgMeta,
};

use super::{HandlerResult, MsgHandle};

pub struct InputHandler {
  handle: MsgHandle,
}

impl InputHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl ClientToBridgeInputMsgDispatch for InputHandler {
  type Output = HandlerResult;

  async fn set_gesture(&self, params: LauncherGestureSet) -> HandlerResult {
    self.handle.state.input.set_gesture(params.gesture).await;
    let event = BridgeToClientInputMsg::GestureChanged(LauncherGestureChanged {
      gesture: params.gesture,
    });
    if let Err(errors) = self.handle.state.bus.broadcast(event, MsgMeta::Event).await {
      tracing::trace!("input broadcast had {} ws error(s)", errors.len());
    }
    Ok(())
  }

  async fn get_gesture(&self) -> HandlerResult {
    let gesture = self.handle.state.input.gesture().await;
    Ok(
      self
        .handle
        .respond_to::<LauncherGestureGet>(LauncherGestureReply { gesture })
        .await?,
    )
  }
}
