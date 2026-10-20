use libbridgething::{
  client::{BlobEntry, BlobList, BlobListReply, ClientToBridgeBlobMsgRequestDispatch},
  wire::WireError,
};

use super::{HandlerResult, MsgHandle};
use crate::{net::ClientScope, state::blobs::blob_url};

#[derive(Debug)]
pub struct BlobHandler {
  handle: MsgHandle,
}

impl BlobHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl ClientToBridgeBlobMsgRequestDispatch for BlobHandler {
  type Output = HandlerResult;

  async fn list(&self) -> HandlerResult {
    if self.handle.scope() != ClientScope::ActiveWebapp {
      return Ok(self.handle.respond(WireError::Unsupported).await?);
    }
    let entries = match self.handle.state.active_webapp().await? {
      Some(active) => self
        .handle
        .state
        .webapp_blobs
        .list(active)
        .await?
        .into_iter()
        .map(|(key, meta)| BlobEntry {
          url: blob_url(&key, &meta.sha256),
          key,
          sha256: meta.sha256,
          mime: meta.mime,
          size: meta.size,
        })
        .collect(),
      None => Vec::new(),
    };
    Ok(self.handle.respond_to::<BlobList>(BlobListReply { entries }).await?)
  }
}
