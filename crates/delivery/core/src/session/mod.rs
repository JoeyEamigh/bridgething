mod handlers;
#[cfg(test)]
mod tests;

use std::{sync::Arc, time::Duration};

use bridgething_gateway::{
  Gateway, GatewayProtocol,
  routing::{Routing, spawn_routing},
};
use bridgething_sdk_runtime::{Connector, rt};
pub use handlers::DeliveryHandlers;
use libbridgething::{
  GatewayCapabilities, GatewayInfo,
  gateway::{
    BridgeToGatewayMsg, GatewayToBridgeCapabilitiesMsgEvent, GatewayToBridgeTransferMsgEvent, TransferAck,
    WebappBlobEntry, WebappDocChanged,
  },
};
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

use crate::{
  bundle::fetch::ArtifactFetch,
  ota::service::{OtaService, OtaServiceDeps},
  seam::Clock,
  transfer::{AckSink, TransferReceiver},
  webapp::{
    blob::{BlobError, BlobProgress, BlobUpload},
    handles::{BlobHandles, MemoryHandleStore},
  },
};

pub const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
  #[error("announce failed: {0}")]
  Announce(String),
}

pub struct SessionDeps {
  pub device_id: String,
  pub clock: Arc<dyn Clock>,
  pub fetch: Arc<dyn ArtifactFetch>,
  pub cache_dir: std::path::PathBuf,
  pub data_dir: Option<std::path::PathBuf>,
  pub info: GatewayInfo,
}

pub fn gateway_info(name: &str, os_name: &str, app_version: &str) -> GatewayInfo {
  GatewayInfo {
    address: String::new(),
    name: name.into(),
    os_name: os_name.into(),
    app_name: name.into(),
    app_version: app_version.into(),
    adapter_version: String::new(),
    lib_version: env!("CARGO_PKG_VERSION").to_string(),
    libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
  }
}

pub struct DeliverySession {
  pub gateway: Gateway,
  pub ota: Arc<OtaService>,
  pub handlers: Arc<DeliveryHandlers>,
  pub handles: BlobHandles<MemoryHandleStore>,
  device_id: String,
  routing: Routing,
  released: watch::Receiver<bool>,
}

impl DeliverySession {
  pub async fn spawn<C: Connector<GatewayProtocol>>(connector: C, deps: SessionDeps) -> Result<Self, SessionError> {
    let (gateway, inbound) = Gateway::spawn_subscribed(connector);
    Self::adopt(gateway, inbound, deps).await
  }

  #[cfg(any(target_arch = "wasm32", feature = "native-ws"))]
  pub async fn connect(url: &str, deps: SessionDeps) -> Result<Self, SessionError> {
    let (gateway, inbound) = Gateway::connect_subscribed(url)
      .await
      .map_err(|e| SessionError::Announce(format!("ws connect {url}: {e:?}")))?;
    Self::adopt(gateway, inbound, deps).await
  }

  async fn adopt(
    gateway: Gateway,
    inbound: broadcast::Receiver<BridgeToGatewayMsg>,
    deps: SessionDeps,
  ) -> Result<Self, SessionError> {
    let ota = OtaService::new(OtaServiceDeps {
      clock: deps.clock.clone(),
      fetch: deps.fetch,
      cache_dir: deps.cache_dir,
      data_dir: deps.data_dir,
    });
    ota.adopt(&deps.device_id, gateway.clone()).await;

    let receiver = TransferReceiver::new(Arc::new(GatewayAcks(gateway.clone())), deps.clock.clone());
    let closed = gateway.connection().closed();
    let stopping = Arc::downgrade(&receiver);
    let lost = (Arc::downgrade(&ota), deps.device_id.clone());
    let (release, released) = watch::channel(false);
    rt::spawn(async move {
      closed.await;
      if let Some(stopping) = stopping.upgrade() {
        stopping.stop();
      }
      if let Some(ota) = lost.0.upgrade() {
        ota.release(&lost.1).await;
      }
      let _ = release.send(true);
    });

    #[cfg(not(target_arch = "wasm32"))]
    let handlers = DeliveryHandlers::new(
      deps.device_id.clone(),
      ota.clone(),
      receiver,
      crate::serve::tunnel::TunnelDispatcher::new(Arc::new(gateway.clone()), deps.clock.clone()),
    );
    #[cfg(target_arch = "wasm32")]
    let handlers = DeliveryHandlers::new(deps.device_id.clone(), ota.clone(), receiver);

    let notifier = handlers.clone();
    let mut announced = std::pin::pin!(notifier.announced.notified());
    announced.as_mut().enable();

    let routing = spawn_routing(gateway.clone(), handlers.clone(), inbound);

    announce(&gateway, deps.info).await?;
    if rt::timeout(ANNOUNCE_TIMEOUT, announced).await.is_err() {
      tracing::warn!("daemon did not announce a version; continuing anyway");
    }

    let handles = BlobHandles::new(MemoryHandleStore::default());
    rt::spawn(handles.sweeper());

    Ok(Self {
      gateway,
      ota,
      handlers,
      handles,
      device_id: deps.device_id,
      routing,
      released,
    })
  }

  pub fn device_id(&self) -> &str {
    &self.device_id
  }

  pub fn receiver(&self) -> &Arc<TransferReceiver> {
    &self.handlers.receiver
  }

  pub fn doc_changes(&self) -> broadcast::Receiver<WebappDocChanged> {
    self.handlers.docs.subscribe()
  }

  pub async fn put_blob(
    &self,
    webapp: Uuid,
    key: String,
    handle: &str,
    progress: Option<BlobProgress>,
  ) -> Result<WebappBlobEntry, BlobError> {
    let upload = BlobUpload::from_handle(&self.handles, webapp, key, handle)?;
    self.ota.put_blob(&self.device_id, upload, progress).await
  }

  pub async fn delete_blob(&self, webapp: Uuid, key: String) -> Result<(), BlobError> {
    self.ota.delete_blob(&self.device_id, webapp, key).await
  }

  pub async fn list_blobs(&self, webapp: Uuid) -> Result<Vec<WebappBlobEntry>, BlobError> {
    self.ota.list_blobs(&self.device_id, webapp).await
  }

  pub async fn closed(&self) {
    self.routing.closed().await;
    let mut released = self.released.clone();
    let _ = released.wait_for(|released| *released).await;
  }
}

struct GatewayAcks(Gateway);

impl AckSink for GatewayAcks {
  fn ack(&self, ack: TransferAck) {
    let gateway = self.0.clone();
    rt::spawn(async move {
      if let Err(error) = gateway.event(GatewayToBridgeTransferMsgEvent::Ack(ack)).await {
        tracing::debug!(%error, "a transfer receipt ack did not reach the device");
      }
    });
  }
}

async fn announce(gateway: &Gateway, info: GatewayInfo) -> Result<(), SessionError> {
  let caps = GatewayCapabilities {
    gateway: info,
    ..Default::default()
  };
  gateway
    .event(GatewayToBridgeCapabilitiesMsgEvent::Announce(caps))
    .await
    .map_err(|e| SessionError::Announce(e.to_string()))
}
