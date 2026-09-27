use libbridgething::{
  ConfigEntry,
  client::{ClientToBridgeConfigMsgRequestDispatch, ConfigGet, ConfigGetReply, ConfigList, ConfigListReply},
};

use super::{HandlerResult, MsgHandle};

pub struct ConfigHandler {
  handle: MsgHandle,
}

impl ConfigHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl ClientToBridgeConfigMsgRequestDispatch for ConfigHandler {
  type Output = HandlerResult;

  async fn get(&self, params: ConfigGet) -> HandlerResult {
    let app_id = self.handle.scoped_app_id().await?;
    let ConfigGet { key } = params;
    let value = self.handle.state.kv.config_get(app_id, &key).await?;
    Ok(
      self
        .handle
        .respond_to::<ConfigGet>(ConfigGetReply { key, value })
        .await?,
    )
  }

  async fn list(&self) -> HandlerResult {
    let app_id = self.handle.scoped_app_id().await?;
    let entries = self
      .handle
      .state
      .kv
      .config_list(app_id)
      .await?
      .into_iter()
      .map(|(key, value)| ConfigEntry { key, value })
      .collect();
    Ok(
      self
        .handle
        .respond_to::<ConfigList>(ConfigListReply { entries })
        .await?,
    )
  }
}
