use std::{net::SocketAddr, sync::Arc};

use libbridgething::{
  CompanionAuthorityScope,
  client::{BridgeToClientAudioMsgEvent, VolumeChanged},
};
use tokio::sync::RwLock;

use crate::{authority::AuthorityRegistry, net::WireEventBus};

const PLACEHOLDER: VolumeChanged = VolumeChanged {
  level: 0.5,
  muted: false,
};

#[derive(Debug, Clone)]
pub struct AudioManager {
  inner: Arc<RwLock<Inner>>,
  authority: AuthorityRegistry,
  bus: WireEventBus,
}

#[derive(Debug, Default)]
struct Inner {
  companion: Option<VolumeChanged>,
  ams: Option<VolumeChanged>,
}

impl AudioManager {
  pub fn new(authority: AuthorityRegistry, bus: WireEventBus) -> Self {
    Self {
      inner: Arc::new(RwLock::new(Inner::default())),
      authority,
      bus,
    }
  }

  pub async fn current(&self) -> VolumeChanged {
    let inner = self.inner.read().await;
    if self.authority.is_authoritative(CompanionAuthorityScope::Volume)
      && let Some(v) = inner.companion
    {
      return v;
    }
    if let Some(v) = inner.ams {
      return v;
    }
    PLACEHOLDER
  }

  pub async fn apply_companion(&self, vol: VolumeChanged) -> Result<(), AudioError> {
    self.inner.write().await.companion = Some(vol);
    self.broadcast_current().await
  }

  pub async fn apply_ams(&self, vol: VolumeChanged) -> Result<(), AudioError> {
    self.inner.write().await.ams = Some(vol);
    self.broadcast_current().await
  }

  pub async fn send_current_to(&self, to: SocketAddr) -> Result<(), AudioError> {
    let v = self.current().await;
    self
      .bus
      .send_event(to, BridgeToClientAudioMsgEvent::VolumeChanged(v))
      .await?;
    Ok(())
  }

  pub async fn broadcast_current(&self) -> Result<(), AudioError> {
    let v = self.current().await;
    self
      .bus
      .broadcast_event(BridgeToClientAudioMsgEvent::VolumeChanged(v))
      .await?;
    Ok(())
  }
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
  #[error(transparent)]
  WS(#[from] crate::net::WSError),
}

crate::impl_broadcast_failure_from!(AudioError);
