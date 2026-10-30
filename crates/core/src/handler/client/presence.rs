use libbridgething::{
  PresenceSettingsError,
  client::{
    BridgeToClientPresenceMsg, ClientToBridgePresenceMsgRequestDispatch, PresenceSettingsErrorReply,
    PresenceSettingsReply, PresenceSettingsSet,
  },
};

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

  async fn settings_get(&self) -> HandlerResult {
    let settings = self.handle.state.presence.settings();
    self
      .handle
      .respond(BridgeToClientPresenceMsg::SettingsReply(PresenceSettingsReply {
        settings,
      }))
      .await?;
    Ok(())
  }

  async fn settings_set(&self, params: PresenceSettingsSet) -> HandlerResult {
    match self.handle.state.presence.set_settings(params.settings).await {
      Ok(()) => {
        let settings = self.handle.state.presence.settings();
        self
          .handle
          .respond(BridgeToClientPresenceMsg::SettingsReply(PresenceSettingsReply {
            settings,
          }))
          .await?;
      }
      Err(error) => {
        tracing::debug!("({}) presence.settingsSet rejected: {error:?}", &self.handle.from);
        self.respond_error(error).await?;
      }
    }
    Ok(())
  }
}

impl PresenceHandler {
  async fn respond_error(&self, error: PresenceSettingsError) -> HandlerResult {
    self
      .handle
      .respond_err::<PresenceSettingsSet>(PresenceSettingsErrorReply { error })
      .await
      .map_err(Into::into)
  }
}
