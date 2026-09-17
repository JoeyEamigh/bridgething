use std::sync::{Arc, Mutex};

use bridgething_companion::dispatch::{
  OtaInbound,
  system::{DeviceLogSink, SystemDispatcher},
};
use bridgething_gateway::{HandlerError, Reply, SystemHandler};
use libbridgething::{
  BridgeThingMeta, LogEntry, LogLevel, OtaError, OtaFinished, OtaProgress, RangeSpec, WebappInfo,
  gateway::{
    DeviceNicknameReply, KeepalivePing, OtaAssetRange, OtaAssetRangeAbandon, OtaAssetRangeRejected, OtaAssetRangeReply,
    TransferAck, TransferBody,
  },
};
use uuid::Uuid;

#[derive(Default)]
struct RecordedLogs(Mutex<Vec<LogEntry>>);

impl DeviceLogSink for RecordedLogs {
  fn on_entry(&self, entry: LogEntry) {
    self.0.lock().unwrap().push(entry);
  }
}

#[derive(Default)]
struct RecordedOta {
  progress: Mutex<Vec<OtaProgress>>,
  nicknames: Mutex<Vec<Option<String>>>,
  gestures: Mutex<Vec<libbridgething::LauncherGesture>>,
  ranges: Mutex<Vec<(Uuid, OtaAssetRange)>>,
  abandoned: Mutex<Vec<Uuid>>,
  metas: Mutex<Vec<BridgeThingMeta>>,
  acks: Mutex<Vec<TransferAck>>,
  installed: Mutex<Vec<WebappInfo>>,
}

fn served(total_size: u32) -> OtaAssetRangeReply {
  OtaAssetRangeReply {
    total_size,
    parts: vec![],
    body: TransferBody::Inline(vec![]),
  }
}

#[async_trait::async_trait]
impl OtaInbound for RecordedOta {
  async fn asset_range(
    &self,
    id: Uuid,
    request: OtaAssetRange,
  ) -> Result<Reply<OtaAssetRangeReply>, HandlerError<OtaAssetRangeRejected>> {
    let total_size = request.ranges.len() as u32;
    self.ranges.lock().unwrap().push((id, request));
    Ok(Reply::new(served(total_size)))
  }

  fn device_meta(&self, meta: BridgeThingMeta) {
    self.metas.lock().unwrap().push(meta);
  }

  fn transfer_ack(&self, ack: TransferAck) {
    self.acks.lock().unwrap().push(ack);
  }

  fn webapp_installed(&self, info: WebappInfo) {
    self.installed.lock().unwrap().push(info);
  }

  fn asset_range_abandon(&self, payload: OtaAssetRangeAbandon) {
    self.abandoned.lock().unwrap().push(payload.request_id);
  }

  fn progress(&self, payload: OtaProgress) {
    self.progress.lock().unwrap().push(payload);
  }

  fn error(&self, _payload: OtaError) {}

  fn finished(&self, _payload: OtaFinished) {}

  fn nickname_changed(&self, nickname: Option<String>) -> Option<BridgeThingMeta> {
    self.nicknames.lock().unwrap().push(nickname);
    None
  }

  fn launcher_gesture_changed(&self, gesture: libbridgething::LauncherGesture) -> Option<BridgeThingMeta> {
    self.gestures.lock().unwrap().push(gesture);
    None
  }
}

fn dispatcher() -> (SystemDispatcher, Arc<RecordedOta>, Arc<RecordedLogs>) {
  let ota = Arc::new(RecordedOta::default());
  let logs = Arc::new(RecordedLogs::default());
  (SystemDispatcher::new(ota.clone(), logs.clone()), ota, logs)
}

#[tokio::test]
async fn a_keepalive_is_answered_on_the_sequence_it_arrived_on() {
  let (dispatch, _ota, _logs) = dispatcher();

  let ack = dispatch.keepalive(KeepalivePing { seq: 41 }).await.expect("answered");
  assert_eq!(ack.response.seq, 41);
}

#[tokio::test]
async fn a_device_log_entry_reaches_the_ring() {
  let (dispatch, _ota, logs) = dispatcher();

  dispatch
    .log_entry(LogEntry {
      ts_unix_s: 1,
      level: LogLevel::Warn,
      target: "bridgething::ota".into(),
      message: "staged".into(),
    })
    .await
    .expect("an event never refuses");

  let held = logs.0.lock().unwrap();
  assert_eq!(held.len(), 1);
  assert_eq!(held[0].message, "staged");
}

#[tokio::test]
async fn a_nickname_change_reaches_the_update_service() {
  let (dispatch, ota, _logs) = dispatcher();

  dispatch
    .device_nickname_changed(DeviceNicknameReply {
      nickname: Some("garage thing".into()),
    })
    .await
    .expect("an event never refuses");

  assert_eq!(
    ota.nicknames.lock().unwrap().clone(),
    vec![Some("garage thing".to_owned())]
  );
}

#[tokio::test]
async fn a_gesture_change_reaches_the_update_service() {
  let (dispatch, ota, _logs) = dispatcher();

  dispatch
    .launcher_gesture_changed(libbridgething::gateway::LauncherGestureReply {
      gesture: libbridgething::LauncherGesture::LongPress,
    })
    .await
    .expect("an event never refuses");

  assert_eq!(
    ota.gestures.lock().unwrap().clone(),
    vec![libbridgething::LauncherGesture::LongPress]
  );
}

#[tokio::test]
async fn a_range_request_is_served_by_the_update_service_and_answered_on_its_transfer() {
  let (dispatch, ota, _logs) = dispatcher();

  let id = Uuid::now_v7();
  let reply = dispatch
    .ota_asset_range(
      id,
      OtaAssetRange {
        update_id: "2026.05.0".into(),
        asset: "daemon".into(),
        ranges: vec![RangeSpec { start: 0, length: 16 }],
      },
    )
    .await
    .expect("the range was served");

  assert_eq!(reply.response.total_size, 1);
  let served = ota.ranges.lock().unwrap();
  assert_eq!(served[0].1.asset, "daemon");
  assert_eq!(
    served[0].0, id,
    "the request's own envelope id is what the reply's transfer must name"
  );
}

#[tokio::test]
async fn an_abandoned_range_reaches_the_update_service() {
  let (dispatch, ota, _logs) = dispatcher();
  let request_id = Uuid::now_v7();

  dispatch
    .ota_asset_range_abandon(OtaAssetRangeAbandon { request_id })
    .await
    .expect("an event never refuses");

  assert_eq!(ota.abandoned.lock().unwrap().clone(), vec![request_id]);
}
