use std::sync::Arc;

use bridgething_gateway::WebappHandler;
use libbridgething::{
  WebappInfo,
  gateway::{WebappActiveChanged, WebappConfigChanged, WebappDocChanged},
  wire::WireError,
};

use crate::provider::ProviderRegistry;

pub const DEFAULT_HERO_PX: u32 = 248;
pub const DEFAULT_THUMB_PX: u32 = 96;

pub trait WebappObserver: Send + Sync {
  fn doc_changed(&self, device_id: &str, changed: WebappDocChanged);
  fn installed(&self, device_id: &str, info: WebappInfo);
  fn active_changed(&self, device_id: &str, changed: WebappActiveChanged);
}

pub struct WebappDispatcher {
  providers: Arc<dyn ProviderRegistry>,
  observer: Arc<dyn WebappObserver>,
  device_id: String,
}

impl WebappDispatcher {
  pub fn new(providers: Arc<dyn ProviderRegistry>, observer: Arc<dyn WebappObserver>, device_id: &str) -> Self {
    Self {
      providers,
      observer,
      device_id: device_id.to_owned(),
    }
  }
}

impl WebappHandler for WebappDispatcher {
  async fn doc_changed(&self, payload: WebappDocChanged) -> Result<(), WireError> {
    self.observer.doc_changed(&self.device_id, payload);
    Ok(())
  }

  async fn config_changed(&self, _payload: WebappConfigChanged) -> Result<(), WireError> {
    Ok(())
  }

  async fn webapp_installed(&self, payload: WebappInfo) -> Result<(), WireError> {
    self.observer.installed(&self.device_id, payload);
    Ok(())
  }

  async fn active_changed(&self, payload: WebappActiveChanged) -> Result<(), WireError> {
    let (hero_px, thumb_px) = payload
      .art
      .as_ref()
      .map(|art| (art.hero_px, art.thumb_px))
      .unwrap_or((DEFAULT_HERO_PX, DEFAULT_THUMB_PX));
    for provider in self.providers.all() {
      provider.set_art_profile(hero_px, thumb_px).await;
    }
    self.observer.active_changed(&self.device_id, payload);
    Ok(())
  }
}
