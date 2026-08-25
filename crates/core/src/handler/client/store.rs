use libbridgething::client::{ClientToBridgeStoreMsgRequestDispatch, KVDelete, KVGet, KVPut, StorageResponse};
use uuid::Uuid;

use super::{HandlerResult, MsgHandle};
use crate::{chrome::ChromeCommand, state::BROWSER_WEBAPP_ID, stock::StockSetupSend};

const BROWSER_NAVIGATE_KEY: &str = "@cdp/navigate";

#[derive(Debug)]
pub struct StorageHandler {
  handle: MsgHandle,
}

impl StorageHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }

  async fn active_app_id(&self) -> Result<Uuid, crate::handler::HandlerError> {
    Ok(self.handle.state.active_webapp().await?.unwrap_or(Uuid::nil()))
  }
}

impl ClientToBridgeStoreMsgRequestDispatch for StorageHandler {
  type Output = HandlerResult;

  async fn get(&self, params: KVGet) -> HandlerResult {
    let app_id = self.active_app_id().await?;
    let KVGet { key } = params;
    tracing::debug!("({}) getting value for key: {}", &self.handle.from, &key);

    let mut value = self.handle.state.kv.data_get(app_id, &key).await?;

    // handle for stock firmware
    if &key == "onboarding_status" {
      tracing::trace!(
        "({}) sending setup status to make stock firmware happy",
        &self.handle.from
      );

      let finished = self.handle.state.devices.last().await?.is_some();
      let status = if finished {
        StockSetupSend::finished()
      } else {
        StockSetupSend::unfinished()
      };

      self.handle.send_stock(status).await?;

      if finished {
        value = Some("finished".to_string());
      }
    }

    Ok(self.handle.respond_to::<KVGet>(StorageResponse { key, value }).await?)
  }

  async fn put(&self, params: KVPut) -> HandlerResult {
    let app_id = self.active_app_id().await?;
    let KVPut { key, value } = params;

    if app_id == BROWSER_WEBAPP_ID && key == BROWSER_NAVIGATE_KEY {
      tracing::info!("({}) browser webapp cdp-navigate to {}", &self.handle.from, &value);
      if let Err(e) = self
        .handle
        .state
        .chrome
        .send(ChromeCommand::NavigateExternal(value.clone()))
        .await
      {
        tracing::warn!("({}) browser navigate dispatch failed: {:?}", &self.handle.from, e);
      }
      return Ok(
        self
          .handle
          .respond_to::<KVPut>(StorageResponse {
            key,
            value: Some(value),
          })
          .await?,
      );
    }

    tracing::debug!("({}) putting key: {}, value: {}", &self.handle.from, &key, &value);
    self.handle.state.kv.data_set(app_id, &key, value.clone()).await?;

    Ok(
      self
        .handle
        .respond_to::<KVPut>(StorageResponse {
          key,
          value: Some(value),
        })
        .await?,
    )
  }

  async fn delete(&self, params: KVDelete) -> HandlerResult {
    let app_id = self.active_app_id().await?;
    let KVDelete { key } = params;
    tracing::debug!("({}) deleting value for key: {}", &self.handle.from, key);
    self.handle.state.kv.data_delete(app_id, &key).await?;

    Ok(
      self
        .handle
        .respond_to::<KVDelete>(StorageResponse { key, value: None })
        .await?,
    )
  }
}
