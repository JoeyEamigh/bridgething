use std::sync::Arc;

use bridgething_delivery::ota::service::OtaService;
use bridgething_gateway::{HandlerError, Reply};
use libbridgething::{
  BridgeThingMeta, OtaError, OtaFinished, OtaProgress, WebappInfo,
  gateway::{OtaAssetRange, OtaAssetRangeAbandon, OtaAssetRangeRejected, OtaAssetRangeReply, TransferAck},
};
use uuid::Uuid;

use crate::dispatch::OtaInbound;

pub struct OtaLink {
  service: Arc<OtaService>,
  device_id: String,
}

impl OtaLink {
  pub fn new(service: Arc<OtaService>, device_id: &str) -> Arc<Self> {
    Arc::new(Self {
      service,
      device_id: device_id.to_owned(),
    })
  }
}

#[async_trait::async_trait]
impl OtaInbound for OtaLink {
  async fn asset_range(
    &self,
    id: Uuid,
    request: OtaAssetRange,
  ) -> Result<Reply<OtaAssetRangeReply>, HandlerError<OtaAssetRangeRejected>> {
    self.service.asset_range(&self.device_id, id, request).await
  }

  fn asset_range_abandon(&self, payload: OtaAssetRangeAbandon) {
    self.service.asset_range_abandon(&self.device_id, payload);
  }

  fn progress(&self, payload: OtaProgress) {
    self.service.progress(&self.device_id, payload);
  }

  fn error(&self, payload: OtaError) {
    self.service.error(&self.device_id, payload);
  }

  fn finished(&self, payload: OtaFinished) {
    self.service.finished(&self.device_id, payload);
  }

  fn nickname_changed(&self, nickname: Option<String>) -> Option<BridgeThingMeta> {
    self.service.nickname_changed(&self.device_id, nickname)
  }

  fn launcher_gesture_changed(&self, gesture: libbridgething::LauncherGesture) -> Option<BridgeThingMeta> {
    self.service.launcher_gesture_changed(&self.device_id, gesture)
  }

  fn device_meta(&self, meta: BridgeThingMeta) {
    self.service.device_meta(&self.device_id, meta);
  }

  fn transfer_ack(&self, ack: TransferAck) {
    self
      .service
      .transfer_ack(&self.device_id, ack.transfer_id, u64::from(ack.received));
  }

  fn webapp_installed(&self, info: WebappInfo) {
    self.service.webapp_installed(&self.device_id, info);
  }
}
