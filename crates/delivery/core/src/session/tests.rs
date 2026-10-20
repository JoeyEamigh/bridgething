use std::{sync::Arc, time::Duration};

use bridgething_gateway::transport::FramedConnector;
use libbridgething::gateway::{
  BridgeToGatewayMsgData, BridgeToGatewayWebappMsg, WebappBlobPut, WebappDocChanged, WebappResourceKind,
};
use uuid::Uuid;

use super::{DeliverySession, SessionDeps, gateway_info};
use crate::{
  blob::{MemoryBlobStore, MemorySlotIndex},
  bundle::fetch::sha256_hex,
  harness::{FRAGMENT, FakeDevice, linked_io},
  ota::harness::{FakeFetch, meta},
  seam::SystemClock,
  transfer::{MIN_WINDOW_BYTES, TransferReceiveError},
  webapp::{WebappResourceError, WebappResourceService, blob::BlobError, handles::HandleStore},
};

const PATIENT: Duration = Duration::from_secs(3);

async fn linked() -> (DeliverySession, FakeDevice) {
  let (companion_io, device) = linked_io();
  device.event(BridgeToGatewayMsgData::Version(Box::new(meta(
    "0.9.0", "2026.10", "stable",
  ))));

  let cache = tempfile::tempdir().expect("a scratch cache");
  let session = DeliverySession::spawn(
    FramedConnector::new(companion_io),
    SessionDeps {
      device_id: "browser".into(),
      clock: Arc::new(SystemClock),
      fetch: FakeFetch::new(),
      cache_dir: cache.path().to_path_buf(),
      data_dir: None,
      info: gateway_info("browser", "browser", "0.0.0"),
    },
  )
  .await
  .expect("the session announces");
  (session, device)
}

fn held(session: &DeliverySession, len: usize) -> String {
  let handle = session.handles.create("video/mp4".into()).expect("a handle");
  session.handles.append(&handle, &ramp(len)).expect("the bytes land");
  handle
}

async fn begun(device: &mut FakeDevice) -> WebappBlobPut {
  let (request_id, put) = device.await_blob_put().await;
  device.ack_blob_put(request_id, 0);
  put
}

fn resources(session: &DeliverySession) -> WebappResourceService {
  WebappResourceService::new(
    Arc::new(MemoryBlobStore::new()),
    Arc::new(MemorySlotIndex::new()),
    session.receiver().clone(),
  )
}

fn ramp(len: usize) -> Vec<u8> {
  (0..len).map(|at| (at % 251) as u8).collect()
}

#[tokio::test]
async fn a_doc_written_on_the_device_reaches_every_doc_subscriber() {
  let (session, device) = linked().await;
  let mut first = session.doc_changes();
  let mut second = session.doc_changes();
  let change = WebappDocChanged {
    id: Uuid::now_v7(),
    key: "dashboard".into(),
    value: Some("[\"light.kitchen\"]".into()),
  };

  device.event(BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::DocChanged(
    change.clone(),
  )));

  for subscriber in [&mut first, &mut second] {
    let heard = tokio::time::timeout(PATIENT, subscriber.recv())
      .await
      .expect("the change arrives")
      .expect("the feed is open");
    assert_eq!(heard, change);
  }
}

#[tokio::test]
async fn a_settings_page_streamed_off_the_device_is_collected_and_acked_back() {
  let (session, mut device) = linked().await;
  let service = resources(&session);
  let webapp_id = Uuid::now_v7();
  let page = ramp(40_000);
  let transfer_id = Uuid::now_v7();

  let gateway = session.gateway.clone();
  let fetching = service.fetch(&gateway, webapp_id, WebappResourceKind::Settings, None);
  let driving = async {
    let (request_id, request) = device.next_resource_request().await;
    assert_eq!(request.id, webapp_id);
    device.stream_resource(request_id, &request, &page, transfer_id);
    device.stream(transfer_id, &page, 0..page.len(), FRAGMENT);
    device.await_ack(transfer_id, page.len() as u32).await;
  };

  let (resource, ()) = tokio::join!(fetching, driving);
  let resource = resource.expect("the streamed page lands");
  assert_eq!(resource.digest, sha256_hex(&page));
  assert_eq!(resource.mime.as_deref(), Some("text/html"));
}

#[tokio::test]
async fn a_link_that_drops_mid_stream_fails_the_fetch_instead_of_waiting_out_the_timeout() {
  let (session, mut device) = linked().await;
  let service = resources(&session);
  let webapp_id = Uuid::now_v7();
  let page = ramp(40_000);
  let transfer_id = Uuid::now_v7();

  let gateway = session.gateway.clone();
  let fetching = service.fetch(&gateway, webapp_id, WebappResourceKind::Settings, None);
  let driving = async {
    let (request_id, request) = device.next_resource_request().await;
    device.stream_resource(request_id, &request, &page, transfer_id);
    device.stream(transfer_id, &page, 0..page.len() / 2, FRAGMENT);
    tokio::time::sleep(Duration::from_millis(50)).await;
    device.hang_up();
  };

  let (outcome, ()) = tokio::time::timeout(PATIENT, async { tokio::join!(fetching, driving) })
    .await
    .expect("a dead link settles the fetch promptly");
  assert!(
    matches!(
      outcome,
      Err(WebappResourceError::Transfer(TransferReceiveError::Stopped { transfer_id: id })) if id == transfer_id
    ),
    "{outcome:?}"
  );
}

#[tokio::test]
async fn a_link_that_drops_with_the_window_full_fails_the_put_as_link_lost_instead_of_waiting_out_the_ack_stall() {
  let (session, mut device) = linked().await;
  let handle = held(&session, 8 * MIN_WINDOW_BYTES as usize);

  let putting = session.put_blob(Uuid::now_v7(), "clip".into(), &handle, None);
  let driving = async {
    let put = begun(&mut device).await;
    let mut unacked = 0;
    while unacked < MIN_WINDOW_BYTES {
      unacked += device.next_fragment(put.transfer.id).await.bytes.len() as u64;
    }
    device.hang_up();
  };

  let (outcome, ()) = tokio::time::timeout(PATIENT, async { tokio::join!(putting, driving) })
    .await
    .expect("a dead link settles the put promptly");
  assert_eq!(outcome, Err(BlobError::LinkLost));
  assert!(
    session.handles.resolve(&handle).is_ok(),
    "the handle survives for the retry"
  );
}

#[tokio::test]
async fn a_link_that_drops_before_the_device_confirms_fails_the_put_as_link_lost_instead_of_waiting_out_the_deadline() {
  let (session, mut device) = linked().await;
  let handle = held(&session, 16 * 1024);

  let putting = session.put_blob(Uuid::now_v7(), "clip".into(), &handle, None);
  let driving = async {
    let put = begun(&mut device).await;
    device.receive_blob(&put).await;
    device.hang_up();
  };

  let (outcome, ()) = tokio::time::timeout(PATIENT, async { tokio::join!(putting, driving) })
    .await
    .expect("a dead link settles the put promptly");
  assert_eq!(outcome, Err(BlobError::LinkLost));
}

#[tokio::test]
async fn a_put_the_device_stores_releases_its_handle() {
  let (session, mut device) = linked().await;
  let handle = held(&session, 16 * 1024);
  let bytes = Arc::downgrade(&session.handles.store().artifact(&handle).expect("the pick is held"));

  let putting = session.put_blob(Uuid::now_v7(), "clip".into(), &handle, None);
  let driving = async {
    let put = begun(&mut device).await;
    device.receive_blob(&put).await;
    device.blob_stored(&put);
  };

  let (stored, ()) = tokio::time::timeout(PATIENT, async { tokio::join!(putting, driving) })
    .await
    .expect("the put settles");
  stored.expect("the device stored the blob");
  assert!(
    session.handles.resolve(&handle).is_err(),
    "a stored pick takes no new puts"
  );
  assert!(
    session.handles.store().artifact(&handle).is_err(),
    "the store no longer holds the pick"
  );
  assert!(bytes.upgrade().is_none(), "nothing keeps the stored pick's bytes alive");
}

#[tokio::test]
async fn once_the_session_reports_closed_every_blob_verb_is_not_connected() {
  let (session, mut device) = linked().await;
  let handle = held(&session, 1024);
  let webapp = Uuid::now_v7();

  device.hang_up();
  tokio::time::timeout(PATIENT, session.closed())
    .await
    .expect("the session notices the link closing");

  assert_eq!(
    session.put_blob(webapp, "clip".into(), &handle, None).await,
    Err(BlobError::NotConnected)
  );
  assert_eq!(
    session.delete_blob(webapp, "clip".into()).await,
    Err(BlobError::NotConnected)
  );
  assert_eq!(session.list_blobs(webapp).await, Err(BlobError::NotConnected));
}
