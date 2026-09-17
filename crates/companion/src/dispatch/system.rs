use std::sync::Arc;

use bridgething_gateway::{HandlerError, Reply, SystemHandler};
use libbridgething::{
  LogEntry, OtaError, OtaFinished, OtaProgress,
  gateway::{
    DeviceNicknameReply, KeepaliveAck, KeepalivePing, LauncherGestureReply, OtaAssetRange, OtaAssetRangeAbandon,
    OtaAssetRangeRejected, OtaAssetRangeReply,
  },
  wire::WireError,
};
use uuid::Uuid;

use crate::dispatch::OtaInbound;

pub trait DeviceLogSink: Send + Sync {
  fn on_entry(&self, entry: LogEntry);
}

pub struct SystemDispatcher {
  ota: Arc<dyn OtaInbound>,
  logs: Arc<dyn DeviceLogSink>,
}

impl SystemDispatcher {
  pub fn new(ota: Arc<dyn OtaInbound>, logs: Arc<dyn DeviceLogSink>) -> Self {
    Self { ota, logs }
  }
}

impl SystemHandler for SystemDispatcher {
  async fn ota_asset_range(
    &self,
    id: Uuid,
    request: OtaAssetRange,
  ) -> Result<Reply<OtaAssetRangeReply>, HandlerError<OtaAssetRangeRejected>> {
    self.ota.asset_range(id, request).await
  }

  async fn keepalive(
    &self,
    request: KeepalivePing,
  ) -> Result<Reply<KeepaliveAck>, HandlerError<::core::convert::Infallible>> {
    Ok(KeepaliveAck { seq: request.seq }.into())
  }

  async fn ota_progress(&self, payload: OtaProgress) -> Result<(), WireError> {
    self.ota.progress(payload);
    Ok(())
  }

  async fn ota_error(&self, payload: OtaError) -> Result<(), WireError> {
    self.ota.error(payload);
    Ok(())
  }

  async fn ota_finished(&self, payload: OtaFinished) -> Result<(), WireError> {
    self.ota.finished(payload);
    Ok(())
  }

  async fn ota_asset_range_abandon(&self, payload: OtaAssetRangeAbandon) -> Result<(), WireError> {
    self.ota.asset_range_abandon(payload);
    Ok(())
  }

  async fn device_nickname_changed(&self, payload: DeviceNicknameReply) -> Result<(), WireError> {
    self.ota.nickname_changed(payload.nickname);
    Ok(())
  }

  async fn launcher_gesture_changed(&self, _payload: LauncherGestureReply) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }

  async fn log_entry(&self, payload: LogEntry) -> Result<(), WireError> {
    self.logs.on_entry(payload);
    Ok(())
  }
}
