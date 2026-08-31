use libbridgething::{
  ConfigEntry, ConfigField, DocEntry, WebappError,
  client::{BridgeToClientConfigMsgEvent, BridgeToClientDocMsgEvent, ConfigChanged, DocChanged},
  gateway::{
    BridgeToGatewayWebappMsgEvent, GatewayToBridgeWebappMsgRequestDispatch, GetActiveWebapp, GetWebappSlots,
    ListWebapps, TransferBody, TransferRef, WebappActive, WebappConfigAck, WebappConfigChanged, WebappConfigDelete,
    WebappConfigGet, WebappConfigGetReply, WebappConfigList, WebappConfigListReply, WebappConfigSet, WebappDocAck,
    WebappDocDelete, WebappDocGet, WebappDocGetReply, WebappDocList, WebappDocListReply, WebappDocSet, WebappList,
    WebappResource, WebappResourceKind, WebappResourceReply, WebappSetSlot, WebappSlot, WebappSwitchTo,
    WebappUninstall,
  },
  protocol::Compress,
};
use uuid::Uuid;

use super::{HandlerResult, MsgHandle};
use crate::{chrome::ChromeCommand, state::sha256_hex};

const KIOSK_HOME_URL: &str = "http://127.0.0.1:8891/";
const KIOSK_HUB_URL_BASE: &str = "http://127.0.0.1:8891/_hub/";

const RESOURCE_INLINE_MAX: usize = 16 * 1024;
const DOC_VALUE_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub struct WebappHandler {
  handle: MsgHandle,
}

impl WebappHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl GatewayToBridgeWebappMsgRequestDispatch for WebappHandler {
  type Output = HandlerResult;

  async fn list(&self) -> HandlerResult {
    let webapps = self.handle.state.webapps.list().await;
    self.handle.respond_to::<ListWebapps>(WebappList { webapps }).await;
    Ok(())
  }

  async fn get_active(&self) -> HandlerResult {
    let active = active_payload(&self.handle).await?;
    self.handle.respond_to::<GetActiveWebapp>(active).await;
    Ok(())
  }

  async fn switch_to(&self, params: WebappSwitchTo) -> HandlerResult {
    let WebappSwitchTo { id } = params;
    if self.handle.state.webapps.resolve(id).await.is_none() {
      tracing::debug!(
        "({:?}) webapp {id} not in registry; rescanning disk before refusing",
        &self.handle.address
      );
      self.handle.state.webapps.rescan().await;
    }
    if self.handle.state.webapps.resolve(id).await.is_none() {
      tracing::warn!("({:?}) refusing switch to unknown webapp {id}", &self.handle.address);
      self
        .handle
        .respond_err::<WebappSwitchTo>(WebappError::WebappNotFound { id: id.to_string() })
        .await;
      return Ok(());
    }

    self.handle.state.set_active_webapp(id).await?;
    self.reload_kiosk().await;
    let active = active_payload(&self.handle).await?;
    self.handle.respond_to::<WebappSwitchTo>(active).await;
    self.broadcast_active_changed().await;
    Ok(())
  }

  async fn uninstall(&self, params: WebappUninstall) -> HandlerResult {
    let WebappUninstall { id } = params;
    if self.handle.state.webapps.is_builtin(id).await {
      tracing::warn!("({:?}) refusing uninstall of builtin webapp {id}", &self.handle.address);
      self
        .handle
        .respond_err::<WebappUninstall>(WebappError::CannotUninstallBuiltin { id: id.to_string() })
        .await;
      return Ok(());
    }

    let removed = self.handle.state.webapps.uninstall(id).await?;
    if removed {
      self.handle.state.kv.webapp_purge(id).await?;
    } else {
      tracing::debug!(
        "({:?}) webapp {id} was not installed; nothing to do",
        &self.handle.address
      );
    }

    let released = self.handle.state.release_slots_for(id).await?;
    if released.overlay {
      tracing::info!("uninstalled webapp {id} held the overlay slot; reverting to the builtin overlay");
      self.handle.state.sync_overlay(false).await;
    }
    if released.launcher {
      tracing::info!("uninstalled webapp {id} held the launcher slot; reverting to the builtin hub");
    }

    let active = self.handle.state.active_webapp().await?;
    let mut needs_reload = released.overlay;
    if active == Some(id) {
      match self.handle.state.launcher_webapp().await? {
        Some(fallback) => {
          tracing::info!("active webapp {id} was uninstalled; falling back to {fallback}");
          self.handle.state.set_active_webapp(fallback).await?;
          self.broadcast_active_changed().await;
          needs_reload = true;
        }
        None => tracing::warn!("active webapp {id} was uninstalled and no fallback is available"),
      }
    }
    if needs_reload {
      self.reload_kiosk().await;
    }

    let active = active_payload(&self.handle).await?;
    self.handle.respond_to::<WebappUninstall>(active).await;
    Ok(())
  }

  async fn resource(&self, params: WebappResource) -> HandlerResult {
    let WebappResource { id, kind, have } = params;
    let (bytes, mime) = match kind {
      WebappResourceKind::Icon => match self.handle.state.webapps.read_icon(id).await {
        Some((bytes, mime)) => (bytes, mime),
        None => {
          self
            .handle
            .respond_err::<WebappResource>(WebappError::ResourceNotAvailable { id: id.to_string() })
            .await;
          return Ok(());
        }
      },
      WebappResourceKind::Settings => match self.handle.state.webapps.read_settings(id).await {
        Some(bytes) => (bytes, Some("text/html".to_string())),
        None => {
          self
            .handle
            .respond_err::<WebappResource>(WebappError::ResourceNotAvailable { id: id.to_string() })
            .await;
          return Ok(());
        }
      },
      WebappResourceKind::Overlay => match self.handle.state.webapps.read_overlay(id).await {
        Some(bytes) => (bytes, Some("text/javascript".to_string())),
        None => {
          self
            .handle
            .respond_err::<WebappResource>(WebappError::ResourceNotAvailable { id: id.to_string() })
            .await;
          return Ok(());
        }
      },
    };

    let sha256 = sha256_hex(&bytes);
    if have.as_deref() == Some(sha256.as_str()) {
      self
        .handle
        .respond_to::<WebappResource>(WebappResourceReply {
          id,
          kind,
          sha256,
          mime,
          body: None,
        })
        .await;
      return Ok(());
    }

    if bytes.len() <= RESOURCE_INLINE_MAX {
      self
        .handle
        .respond_to::<WebappResource>(WebappResourceReply {
          id,
          kind,
          sha256,
          mime,
          body: Some(TransferBody::Inline(bytes)),
        })
        .await;
      return Ok(());
    }

    let Some(address) = self.handle.address else {
      tracing::warn!("webapp resource stream requested by an addressless peer; refusing");
      self
        .handle
        .respond_err::<WebappResource>(WebappError::Internal {
          reason: "streaming reply needs an addressed peer".into(),
        })
        .await;
      return Ok(());
    };

    let transfer = TransferRef {
      id: self.handle.id,
      total_size: bytes.len() as u32,
      sha256: Some(sha256.clone()),
    };
    self
      .handle
      .respond_to::<WebappResource>(WebappResourceReply {
        id,
        kind,
        sha256,
        mime,
        body: Some(TransferBody::Stream(transfer)),
      })
      .await;

    let outbound = self.handle.state.transfer_outbound.clone();
    let bluetooth = self.handle.bluetooth.clone();
    let transfer_id = self.handle.id;
    tokio::spawn(async move {
      outbound
        .send_stream(&bluetooth, address, transfer_id, bytes.into(), Compress::IfSmaller)
        .await;
    });
    Ok(())
  }

  async fn get_slots(&self) -> HandlerResult {
    let slots = self.handle.state.webapp_slots().await?;
    self.handle.respond_to::<GetWebappSlots>(slots).await;
    Ok(())
  }

  async fn set_slot(&self, params: WebappSetSlot) -> HandlerResult {
    let WebappSetSlot { slot, id } = params;

    if let Some(id) = id {
      if self.handle.state.webapps.resolve(id).await.is_none() {
        self.handle.state.webapps.rescan().await;
      }
      if self.handle.state.webapps.resolve(id).await.is_none() {
        self
          .handle
          .respond_err::<WebappSetSlot>(WebappError::WebappNotFound { id: id.to_string() })
          .await;
        return Ok(());
      }
      let eligible = match slot {
        WebappSlot::Launcher => self.handle.state.webapps.is_launcher(id).await,
        WebappSlot::Overlay => self.handle.state.webapps.provides_overlay(id).await,
      };
      if !eligible {
        let err = match slot {
          WebappSlot::Launcher => WebappError::NotALauncher { id: id.to_string() },
          WebappSlot::Overlay => WebappError::NoOverlay { id: id.to_string() },
        };
        tracing::warn!(
          "({:?}) refusing {slot:?} slot for {id}: ineligible",
          &self.handle.address
        );
        self.handle.respond_err::<WebappSetSlot>(err).await;
        return Ok(());
      }
    }

    match slot {
      WebappSlot::Launcher => {
        let previous = self.handle.state.launcher_webapp().await?;
        self.handle.state.set_launcher_slot(id).await?;
        let active = self.handle.state.active_webapp().await?;
        if active.is_some()
          && active == previous
          && let Some(next) = self.handle.state.launcher_webapp().await?
        {
          self.handle.state.set_active_webapp(next).await?;
          self.broadcast_active_changed().await;
        }
        self.reload_kiosk().await;
      }
      WebappSlot::Overlay => {
        self.handle.state.set_overlay_slot(id).await?;
        self.handle.state.sync_overlay(false).await;
        self.reload_kiosk().await;
      }
    }

    let slots = self.handle.state.webapp_slots().await?;
    self.handle.respond_to::<WebappSetSlot>(slots).await;
    Ok(())
  }

  async fn config_get(&self, params: WebappConfigGet) -> HandlerResult {
    let WebappConfigGet { id, key } = params;
    if self.handle.state.webapps.bundle(id).await.is_none() {
      self
        .handle
        .respond_err::<WebappConfigGet>(WebappError::WebappNotFound { id: id.to_string() })
        .await;
      return Ok(());
    }
    let value = self.handle.state.kv.config_get(id, &key).await?;
    self
      .handle
      .respond_to::<WebappConfigGet>(WebappConfigGetReply { key, value })
      .await;
    Ok(())
  }

  async fn config_list(&self, params: WebappConfigList) -> HandlerResult {
    let WebappConfigList { id } = params;
    if self.handle.state.webapps.bundle(id).await.is_none() {
      self
        .handle
        .respond_err::<WebappConfigList>(WebappError::WebappNotFound { id: id.to_string() })
        .await;
      return Ok(());
    }
    let entries = self
      .handle
      .state
      .kv
      .config_list(id)
      .await?
      .into_iter()
      .map(|(key, value)| ConfigEntry { key, value })
      .collect();
    self
      .handle
      .respond_to::<WebappConfigList>(WebappConfigListReply { entries })
      .await;
    Ok(())
  }

  async fn config_set(&self, params: WebappConfigSet) -> HandlerResult {
    let WebappConfigSet { id, key, value } = params;
    let manifest = match self.handle.state.webapps.manifest(id).await {
      Some(m) => m,
      None => {
        self
          .handle
          .respond_err::<WebappConfigSet>(WebappError::WebappNotFound { id: id.to_string() })
          .await;
        return Ok(());
      }
    };
    let field = match manifest.config.iter().find(|f| f.key() == key) {
      Some(f) => f,
      None => {
        self
          .handle
          .respond_err::<WebappConfigSet>(WebappError::UnknownConfigKey { key })
          .await;
        return Ok(());
      }
    };
    if let Err(reason) = validate_value(field, &value) {
      self
        .handle
        .respond_err::<WebappConfigSet>(WebappError::InvalidConfigValue { key, reason })
        .await;
      return Ok(());
    }

    self.handle.state.kv.config_set(id, &key, value.clone()).await?;
    self.broadcast_config_change(id, &key, Some(value.clone())).await;
    self
      .handle
      .respond_to::<WebappConfigSet>(WebappConfigAck {
        key,
        value: Some(value),
      })
      .await;
    Ok(())
  }

  async fn config_delete(&self, params: WebappConfigDelete) -> HandlerResult {
    let WebappConfigDelete { id, key } = params;
    let manifest = match self.handle.state.webapps.manifest(id).await {
      Some(m) => m,
      None => {
        self
          .handle
          .respond_err::<WebappConfigDelete>(WebappError::WebappNotFound { id: id.to_string() })
          .await;
        return Ok(());
      }
    };
    let field = match manifest.config.iter().find(|f| f.key() == key) {
      Some(f) => f,
      None => {
        self
          .handle
          .respond_err::<WebappConfigDelete>(WebappError::UnknownConfigKey { key })
          .await;
        return Ok(());
      }
    };

    let restored = field.default_as_storage();
    match restored.clone() {
      Some(default) => {
        self.handle.state.kv.config_set(id, &key, default).await?;
      }
      None => {
        self.handle.state.kv.config_delete(id, &key).await?;
      }
    }
    self.broadcast_config_change(id, &key, restored.clone()).await;
    self
      .handle
      .respond_to::<WebappConfigDelete>(WebappConfigAck { key, value: restored })
      .await;
    Ok(())
  }

  async fn doc_get(&self, params: WebappDocGet) -> HandlerResult {
    let WebappDocGet { id, key } = params;
    if self.handle.state.webapps.bundle(id).await.is_none() {
      self
        .handle
        .respond_err::<WebappDocGet>(WebappError::WebappNotFound { id: id.to_string() })
        .await;
      return Ok(());
    }
    let value = self.handle.state.kv.doc_get(id, &key).await?;
    self
      .handle
      .respond_to::<WebappDocGet>(WebappDocGetReply { key, value })
      .await;
    Ok(())
  }

  async fn doc_list(&self, params: WebappDocList) -> HandlerResult {
    let WebappDocList { id } = params;
    if self.handle.state.webapps.bundle(id).await.is_none() {
      self
        .handle
        .respond_err::<WebappDocList>(WebappError::WebappNotFound { id: id.to_string() })
        .await;
      return Ok(());
    }
    let entries = self
      .handle
      .state
      .kv
      .doc_list(id)
      .await?
      .into_iter()
      .map(|(key, value)| DocEntry { key, value })
      .collect();
    self
      .handle
      .respond_to::<WebappDocList>(WebappDocListReply { entries })
      .await;
    Ok(())
  }

  async fn doc_set(&self, params: WebappDocSet) -> HandlerResult {
    let WebappDocSet { id, key, value } = params;
    if self.handle.state.webapps.bundle(id).await.is_none() {
      self
        .handle
        .respond_err::<WebappDocSet>(WebappError::WebappNotFound { id: id.to_string() })
        .await;
      return Ok(());
    }
    if value.len() > DOC_VALUE_MAX_BYTES {
      self
        .handle
        .respond_err::<WebappDocSet>(WebappError::InvalidDocValue {
          key,
          reason: format!("value exceeds {DOC_VALUE_MAX_BYTES} bytes"),
        })
        .await;
      return Ok(());
    }

    self.handle.state.kv.doc_set(id, &key, value.clone()).await?;
    self.broadcast_doc_change_to_client(id, &key, Some(value.clone())).await;
    self
      .handle
      .respond_to::<WebappDocSet>(WebappDocAck {
        key,
        value: Some(value),
      })
      .await;
    Ok(())
  }

  async fn doc_delete(&self, params: WebappDocDelete) -> HandlerResult {
    let WebappDocDelete { id, key } = params;
    if self.handle.state.webapps.bundle(id).await.is_none() {
      self
        .handle
        .respond_err::<WebappDocDelete>(WebappError::WebappNotFound { id: id.to_string() })
        .await;
      return Ok(());
    }
    self.handle.state.kv.doc_delete(id, &key).await?;
    self.broadcast_doc_change_to_client(id, &key, None).await;
    self
      .handle
      .respond_to::<WebappDocDelete>(WebappDocAck { key, value: None })
      .await;
    Ok(())
  }
}

impl WebappHandler {
  async fn broadcast_config_change(&self, id: Uuid, key: &str, value: Option<String>) {
    self
      .handle
      .bluetooth
      .gateway_man
      .broadcast(BridgeToGatewayWebappMsgEvent::ConfigChanged(WebappConfigChanged {
        id,
        key: key.to_string(),
        value: value.clone(),
      }))
      .await;
    let active = match self.handle.state.active_webapp().await {
      Ok(Some(active)) => active,
      _ => return,
    };
    if active != id {
      return;
    }
    let event = BridgeToClientConfigMsgEvent::Changed(ConfigChanged {
      key: key.to_string(),
      value,
    });
    if let Err(errs) = self.handle.state.bus.broadcast_event(event).await {
      tracing::debug!("config-change broadcast: {} non-fatal errors", errs.len());
    }
  }

  async fn broadcast_doc_change_to_client(&self, id: Uuid, key: &str, value: Option<String>) {
    let active = match self.handle.state.active_webapp().await {
      Ok(Some(active)) => active,
      _ => return,
    };
    if active != id {
      return;
    }
    let event = BridgeToClientDocMsgEvent::Changed(DocChanged {
      key: key.to_string(),
      value,
    });
    if let Err(errs) = self.handle.state.bus.broadcast_event(event).await {
      tracing::debug!("doc-change broadcast: {} non-fatal errors", errs.len());
    }
  }

  async fn reload_kiosk(&self) {
    let url = navigate_url_for_active(&self.handle.state).await;
    if let Err(e) = self.handle.state.chrome.send(ChromeCommand::Navigate(url)).await {
      tracing::warn!("failed to reload kiosk after webapp switch: {:?}", e);
    }
  }

  async fn broadcast_active_changed(&self) {
    self
      .handle
      .bluetooth
      .gateway_man
      .broadcast(BridgeToGatewayWebappMsgEvent::ActiveChanged(
        self.handle.state.active_webapp_changed_event().await,
      ))
      .await;
  }
}

pub async fn navigate_url_for_active(state: &crate::state::State) -> String {
  let Ok(Some(active)) = state.active_webapp().await else {
    return KIOSK_HOME_URL.to_string();
  };
  if state.launcher_webapp().await.ok().flatten() != Some(active) {
    return KIOSK_HOME_URL.to_string();
  }
  match state.webapps.bundle_hash(active).await {
    Some(hash) => format!("{KIOSK_HUB_URL_BASE}{hash}/"),
    None => KIOSK_HOME_URL.to_string(),
  }
}

async fn active_payload(handle: &MsgHandle) -> Result<WebappActive, crate::state::StateError> {
  let id = handle.state.active_webapp().await?;
  let name = match id {
    Some(id) => handle.state.webapps.bundle(id).await.map(|b| b.manifest.name.clone()),
    None => None,
  };
  Ok(WebappActive { id, name })
}

fn validate_value(field: &ConfigField, value: &str) -> Result<(), String> {
  match field {
    ConfigField::String(f) | ConfigField::Secret(f) => {
      let len = value.chars().count() as u32;
      if let Some(min) = f.min_length
        && len < min
      {
        return Err(format!("value shorter than min_length {min}"));
      }
      if let Some(max) = f.max_length
        && len > max
      {
        return Err(format!("value longer than max_length {max}"));
      }
    }
    ConfigField::Number(f) => {
      let n = value
        .parse::<f64>()
        .map_err(|_| format!("not a valid number: {value}"))?;
      if let Some(min) = f.min
        && n < min
      {
        return Err(format!("value below min {min}"));
      }
      if let Some(max) = f.max
        && n > max
      {
        return Err(format!("value above max {max}"));
      }
    }
    ConfigField::Boolean(_) => {
      if !matches!(value, "true" | "false") {
        return Err(format!("expected true/false, got {value}"));
      }
    }
    ConfigField::Enum(f) => {
      if !f.choices.iter().any(|c| c == value) {
        return Err(format!("not in choices: {value}"));
      }
    }
  }
  Ok(())
}
