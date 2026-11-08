use std::sync::Arc;

use bridgething_delivery::ota::service::MetaChange;
use bridgething_gateway::{HandlerError, Reply, SystemHandler};
use libbridgething::{
  DisplaySettings, LogEntry, OtaError, OtaFinished, OtaProgress,
  gateway::{
    CompanionLaunchReply, DeviceNicknameReply, HostWatch, KeepaliveAck, KeepalivePing, LauncherGestureReply,
    OtaAssetRange, OtaAssetRangeAbandon, OtaAssetRangeRejected, OtaAssetRangeReply,
  },
  wire::WireError,
};
use uuid::Uuid;

use crate::dispatch::OtaInbound;

pub trait DeviceLogSink: Send + Sync {
  fn on_entry(&self, entry: LogEntry);
}

pub trait HostWatchSink: Send + Sync {
  fn watch(&self, watch: HostWatch);
}

pub struct SystemDispatcher {
  ota: Arc<dyn OtaInbound>,
  logs: Arc<dyn DeviceLogSink>,
  host: Option<Arc<dyn HostWatchSink>>,
}

impl SystemDispatcher {
  pub fn new(ota: Arc<dyn OtaInbound>, logs: Arc<dyn DeviceLogSink>, host: Option<Arc<dyn HostWatchSink>>) -> Self {
    Self { ota, logs, host }
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
    self.ota.meta_changed(MetaChange::Nickname(payload.nickname));
    Ok(())
  }

  async fn launcher_gesture_changed(&self, payload: LauncherGestureReply) -> Result<(), WireError> {
    self.ota.meta_changed(MetaChange::LauncherGesture(payload.gesture));
    Ok(())
  }

  async fn companion_launch_changed(&self, payload: CompanionLaunchReply) -> Result<(), WireError> {
    self.ota.meta_changed(MetaChange::CompanionLaunch(payload.enabled));
    Ok(())
  }

  async fn display_settings_changed(&self, payload: DisplaySettings) -> Result<(), WireError> {
    self.ota.meta_changed(MetaChange::Display(payload));
    Ok(())
  }

  async fn log_entry(&self, payload: LogEntry) -> Result<(), WireError> {
    self.logs.on_entry(payload);
    Ok(())
  }

  async fn host_watch(&self, payload: HostWatch) -> Result<(), WireError> {
    if let Some(host) = &self.host {
      host.watch(payload);
    }
    Ok(())
  }
}
