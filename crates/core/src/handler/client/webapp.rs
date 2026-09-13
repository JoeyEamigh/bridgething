use libbridgething::{
  WebappError,
  client::{
    ClientToBridgeWebappMsgDispatch, WebappActivate, WebappActiveReply, WebappCurrent, WebappCurrentReply, WebappIcon,
    WebappIconReply, WebappList, WebappListReply,
  },
  gateway::BridgeToGatewayWebappMsgEvent,
};

use super::{HandlerResult, MsgHandle};
use crate::{chrome::ChromeCommand, handler::gateway::webapp::navigate_url_for_active};

pub struct WebappHandler {
  handle: MsgHandle,
}

impl WebappHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl ClientToBridgeWebappMsgDispatch for WebappHandler {
  type Output = HandlerResult;

  async fn list(&self) -> HandlerResult {
    let webapps = self.handle.state.webapps.list_for_clients().await;
    self
      .handle
      .respond_to::<WebappList>(WebappListReply { webapps })
      .await?;
    Ok(())
  }

  async fn current(&self) -> HandlerResult {
    let id = self.handle.state.active_webapp().await?;
    let name = match id {
      Some(id) => self
        .handle
        .state
        .webapps
        .bundle(id)
        .await
        .map(|b| b.manifest.name.clone()),
      None => None,
    };
    self
      .handle
      .respond_to::<WebappCurrent>(WebappCurrentReply { id, name })
      .await?;
    Ok(())
  }

  async fn activate(&self, params: WebappActivate) -> HandlerResult {
    let WebappActivate { id } = params;
    if self.handle.state.webapps.resolve(id).await.is_none() {
      self.handle.state.webapps.rescan().await;
    }
    if self.handle.state.webapps.resolve(id).await.is_none() {
      self
        .handle
        .respond_err::<WebappActivate>(WebappError::WebappNotFound { id: id.to_string() })
        .await?;
      return Ok(());
    }
    if !self.handle.state.webapps.has_app_entry(id).await {
      tracing::warn!("refusing to activate {id}: the bundle has no app entry to show");
      self
        .handle
        .respond_err::<WebappActivate>(WebappError::MissingIndexHtml)
        .await?;
      return Ok(());
    }

    self.handle.state.set_active_webapp(id).await?;
    let url = navigate_url_for_active(&self.handle.state).await;
    if let Err(e) = self.handle.state.chrome.send(ChromeCommand::Navigate(url)).await {
      tracing::warn!("failed to reload kiosk after webapp activate: {:?}", e);
    }
    let name = self
      .handle
      .state
      .webapps
      .bundle(id)
      .await
      .map(|b| b.manifest.name.clone());
    self
      .handle
      .respond_to::<WebappActivate>(WebappActiveReply { id: Some(id), name })
      .await?;
    self
      .handle
      .bluetooth
      .gateway_man
      .broadcast(BridgeToGatewayWebappMsgEvent::ActiveChanged(
        self.handle.state.active_webapp_changed_event().await,
      ))
      .await;
    Ok(())
  }

  async fn icon(&self, params: WebappIcon) -> HandlerResult {
    let WebappIcon { id } = params;
    match self.handle.state.webapps.read_icon(id).await {
      Some((bytes, mime)) => {
        self
          .handle
          .respond_to::<WebappIcon>(WebappIconReply { bytes, mime })
          .await?;
      }
      None => {
        self
          .handle
          .respond_err::<WebappIcon>(WebappError::ResourceNotAvailable { id: id.to_string() })
          .await?;
      }
    }
    Ok(())
  }
}
