use libbridgething::{
  DocEntry, WebappError,
  client::{
    ClientToBridgeDocMsgRequestDispatch, DocAck, DocDelete, DocGet, DocGetReply, DocList, DocListReply, DocSet,
  },
  gateway::{BridgeToGatewayWebappMsgEvent, WebappDocChanged},
};
use uuid::Uuid;

use super::{HandlerResult, MsgHandle};

const DOC_VALUE_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub struct DocHandler {
  handle: MsgHandle,
}

impl DocHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }

  async fn broadcast_doc_change_to_gateway(&self, id: Uuid, key: &str, value: Option<String>) {
    let event = BridgeToGatewayWebappMsgEvent::DocChanged(WebappDocChanged {
      id,
      key: key.to_string(),
      value,
    });
    self.handle.bluetooth.gateway_man.broadcast(event).await;
  }
}

impl ClientToBridgeDocMsgRequestDispatch for DocHandler {
  type Output = HandlerResult;

  async fn get(&self, params: DocGet) -> HandlerResult {
    let app_id = self.handle.scoped_app_id().await?;
    let DocGet { key } = params;
    let value = self.handle.state.kv.doc_get(app_id, &key).await?;
    Ok(self.handle.respond_to::<DocGet>(DocGetReply { key, value }).await?)
  }

  async fn list(&self) -> HandlerResult {
    let app_id = self.handle.scoped_app_id().await?;
    let entries = self
      .handle
      .state
      .kv
      .doc_list(app_id)
      .await?
      .into_iter()
      .map(|(key, value)| DocEntry { key, value })
      .collect();
    Ok(self.handle.respond_to::<DocList>(DocListReply { entries }).await?)
  }

  async fn set(&self, params: DocSet) -> HandlerResult {
    let app_id = self.handle.scoped_app_id().await?;
    let DocSet { key, value } = params;
    if value.len() > DOC_VALUE_MAX_BYTES {
      return Ok(
        self
          .handle
          .respond_err::<DocSet>(WebappError::InvalidDocValue {
            key,
            reason: format!("value exceeds {DOC_VALUE_MAX_BYTES} bytes"),
          })
          .await?,
      );
    }
    self.handle.state.kv.doc_set(app_id, &key, value.clone()).await?;
    self
      .broadcast_doc_change_to_gateway(app_id, &key, Some(value.clone()))
      .await;
    Ok(
      self
        .handle
        .respond_to::<DocSet>(DocAck {
          key,
          value: Some(value),
        })
        .await?,
    )
  }

  async fn delete(&self, params: DocDelete) -> HandlerResult {
    let app_id = self.handle.scoped_app_id().await?;
    let DocDelete { key } = params;
    self.handle.state.kv.doc_delete(app_id, &key).await?;
    self.broadcast_doc_change_to_gateway(app_id, &key, None).await;
    Ok(self.handle.respond_to::<DocDelete>(DocAck { key, value: None }).await?)
  }
}
