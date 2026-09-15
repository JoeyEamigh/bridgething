use libbridgething::{
  client::{BridgeToClientInputMsg, LauncherGestureChanged},
  gateway::{
    BridgeToGatewayInputMsg, BridgeToGatewayMsg, GatewayToBridgeInputMsgCommandDispatch,
    GatewayToBridgeInputMsgRequestDispatch, InputGestureChanged, InputGestureReply, InputGetGesture, InputSetGesture,
  },
  wire::MsgMeta,
};
use uuid::Uuid;

use super::{HandlerResult, MsgHandle};
use crate::bluetooth::OutboundGatewayMessage;

pub struct InputHandler {
  handle: MsgHandle,
}

impl InputHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }

  async fn announce(&self, gesture: libbridgething::LauncherGesture) {
    let client_event = BridgeToClientInputMsg::GestureChanged(LauncherGestureChanged { gesture });
    if let Err(errors) = self.handle.state.bus.broadcast(client_event, MsgMeta::Event).await {
      tracing::trace!("input broadcast had {} ws error(s)", errors.len());
    }
    self
      .handle
      .bluetooth
      .gateway_man
      .send_all(OutboundGatewayMessage::new(
        None,
        BridgeToGatewayMsg {
          id: Uuid::now_v7(),
          meta: MsgMeta::Event,
          data: BridgeToGatewayInputMsg::GestureChanged(InputGestureChanged { gesture }).into(),
        },
      ))
      .await;
  }
}

impl GatewayToBridgeInputMsgCommandDispatch for InputHandler {
  type Output = HandlerResult;

  async fn set_gesture(&self, params: InputSetGesture) -> HandlerResult {
    self.handle.state.input.set_gesture(params.gesture).await;
    self.announce(params.gesture).await;
    Ok(())
  }
}

impl GatewayToBridgeInputMsgRequestDispatch for InputHandler {
  type Output = HandlerResult;

  async fn get_gesture(&self) -> HandlerResult {
    let gesture = self.handle.state.input.gesture().await;
    self
      .handle
      .respond_to::<InputGetGesture>(InputGestureReply { gesture })
      .await;
    Ok(())
  }
}
