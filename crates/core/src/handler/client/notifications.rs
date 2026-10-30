use libbridgething::{
  NotificationsError,
  client::{
    BridgeToClientNotificationsMsgEvent, ClientToBridgeNotificationsMsgDispatch,
    NotificationInvoke as ClientNotificationInvoke, NotificationsErrorReply,
  },
  gateway::{self, BridgeToGatewayNotificationsMsgCommand},
};

use super::{HandlerResult, MsgHandle};

pub struct NotificationsHandler {
  handle: MsgHandle,
}

impl NotificationsHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }

  async fn invoke(
    &self,
    params: ClientNotificationInvoke,
    action: fn(gateway::NotificationInvoke) -> BridgeToGatewayNotificationsMsgCommand,
  ) -> HandlerResult {
    match self.handle.state.notification_origins.origin_of(&params.id) {
      Some(origin) => {
        let cmd = action(gateway::NotificationInvoke { id: params.id });
        self.handle.bluetooth.gateway_man.send_command(origin, cmd).await;
      }
      None => {
        tracing::debug!(id = %params.id, "notification action for a notification no connected companion posted");
        let event = BridgeToClientNotificationsMsgEvent::ErrorEvent(NotificationsErrorReply {
          error: NotificationsError::NoTarget,
        });
        if let Err(err) = self.handle.state.bus.send_event(self.handle.from, event).await {
          tracing::warn!(?err, "failed to report notification action failure to webapp");
        }
      }
    }
    Ok(())
  }
}

impl ClientToBridgeNotificationsMsgDispatch for NotificationsHandler {
  type Output = HandlerResult;

  async fn invoke_positive(&self, params: ClientNotificationInvoke) -> HandlerResult {
    if self.handle.bluetooth.le.try_invoke_positive(&params.id).await {
      return Ok(());
    }
    self
      .invoke(params, BridgeToGatewayNotificationsMsgCommand::InvokePositive)
      .await
  }

  async fn invoke_negative(&self, params: ClientNotificationInvoke) -> HandlerResult {
    if self.handle.bluetooth.le.try_invoke_negative(&params.id).await {
      return Ok(());
    }
    self
      .invoke(params, BridgeToGatewayNotificationsMsgCommand::InvokeNegative)
      .await
  }
}
