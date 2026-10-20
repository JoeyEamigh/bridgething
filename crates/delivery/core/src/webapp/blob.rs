use std::{
  collections::BTreeMap,
  sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
  },
  time::Duration,
};

use bridgething_gateway::{Gateway, RequestFailure};
use bridgething_sdk_runtime::rt;
use futures::{future::Either, pin_mut};
use libbridgething::{
  WebappError,
  gateway::{TransferAbandon, TransferRef, WebappBlobDelete, WebappBlobEntry, WebappBlobList, WebappBlobPut},
  wire::WireError,
};
use tokio::sync::{Notify, broadcast, oneshot};
use uuid::Uuid;

use super::handles::{BlobHandles, HandleError, HandleLease, HandleStore};
use crate::{
  ota::{
    event::OtaPhaseSnapshot,
    service::{ACK_TIMEOUT_MS, OtaSignal},
    stream::{Artifact, ArtifactStreamer},
  },
  transfer::{SendError, SourceRange},
};

pub const CONFIRM_DEADLINE: Duration = Duration::from_secs(30);
const ATTEMPTS: usize = 2;
const LABEL: &str = "blob";

pub type BlobProgress = Arc<dyn Fn(u64, u64) + Send + Sync>;

pub struct BlobUpload {
  pub webapp: Uuid,
  pub key: String,
  pub mime: String,
  pub artifact: Arc<dyn Artifact>,
  lease: Option<HandleLease>,
}

impl BlobUpload {
  pub fn from_handle<S: HandleStore>(
    handles: &BlobHandles<S>,
    webapp: Uuid,
    key: String,
    handle: &str,
  ) -> Result<Self, HandleError> {
    let (held, artifact, lease) = handles.lease(handle, webapp, &key)?;
    Ok(Self {
      webapp,
      key,
      mime: held.mime,
      artifact,
      lease: Some(lease),
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlobError {
  #[error("not_connected: no link to the device")]
  NotConnected,
  #[error("invalid: {0}")]
  Source(String),
  #[error(transparent)]
  Handle(#[from] HandleError),
  #[error("{}", refusal(.0))]
  Refused(WebappError),
  #[error("transfer_failed: {0}")]
  Stream(String),
  #[error("link_lost: the link to the device dropped")]
  LinkLost,
  #[error("superseded: a newer upload of the same key replaced this one")]
  Superseded,
  #[error("unconfirmed: the device never confirmed the blob")]
  Unconfirmed,
  #[error("unsupported: the device does not take webapp blobs")]
  Unsupported,
  #[error("protocol: {0}")]
  Protocol(String),
}

impl From<RequestFailure<WebappError>> for BlobError {
  fn from(failure: RequestFailure<WebappError>) -> Self {
    match failure {
      RequestFailure::Domain(error) => Self::Refused(error),
      RequestFailure::Disconnected => Self::LinkLost,
      RequestFailure::Timeout => Self::Protocol("the device did not answer in time".into()),
      RequestFailure::ResponseMismatch => Self::Protocol("the reply did not match the request".into()),
      RequestFailure::Protocol(WireError::Unsupported | WireError::Unimplemented) => Self::Unsupported,
      RequestFailure::Protocol(error) => Self::Protocol(format!("{error:?}")),
    }
  }
}

impl From<SendError> for BlobError {
  fn from(error: SendError) -> Self {
    match error {
      SendError::SinkClosed { .. } => Self::LinkLost,
      other => Self::Stream(other.to_string()),
    }
  }
}

fn refusal(error: &WebappError) -> String {
  match error {
    WebappError::BlobTooLarge { key, max_bytes } => format!("too_large: {key} is over the {max_bytes} byte cap"),
    WebappError::BlobQuotaExceeded { key, max_bytes } => {
      format!("quota_exceeded: {key} would take the webapp over its {max_bytes} byte quota")
    }
    WebappError::BlobStorageFull { key, needed_bytes } => {
      format!("storage_full: the device has no room for the {needed_bytes} bytes {key} needs")
    }
    WebappError::InvalidBlobKey { key, reason } => format!("invalid_key: {key}: {reason}"),
    WebappError::InvalidBlobPut { key, reason } => format!("invalid: {key}: {reason}"),
    WebappError::BlobTransferFailed { key, reason } => format!("transfer_failed: {key}: {reason}"),
    WebappError::WebappNotFound { id } => format!("not_found: no webapp {id} on the device"),
    other => format!("refused: {other:?}"),
  }
}

pub(crate) type BlobSlot = (String, Uuid, String);

#[derive(Default)]
struct Accepted {
  order: u64,
  superseded: Option<Arc<Notify>>,
}

struct Turn {
  claims: usize,
  accepted: Arc<tokio::sync::Mutex<Accepted>>,
}

#[derive(Default)]
pub(crate) struct BlobPuts {
  orders: AtomicU64,
  turns: Mutex<BTreeMap<BlobSlot, Turn>>,
}

impl BlobPuts {
  pub(crate) fn claim(self: &Arc<Self>, slot: BlobSlot) -> Arc<BlobClaim> {
    let mut turns = self.turns.lock().unwrap();
    let order = self.orders.fetch_add(1, Ordering::SeqCst) + 1;
    let turn = turns.entry(slot.clone()).or_insert_with(|| Turn {
      claims: 0,
      accepted: Arc::default(),
    });
    turn.claims += 1;
    Arc::new(BlobClaim {
      puts: self.clone(),
      slot,
      order,
      accepted: turn.accepted.clone(),
      superseded: Arc::new(Notify::new()),
    })
  }
}

pub(crate) struct BlobClaim {
  puts: Arc<BlobPuts>,
  slot: BlobSlot,
  order: u64,
  accepted: Arc<tokio::sync::Mutex<Accepted>>,
  superseded: Arc<Notify>,
}

impl Drop for BlobClaim {
  fn drop(&mut self) {
    let mut turns = self.puts.turns.lock().unwrap();
    if let Some(turn) = turns.get_mut(&self.slot) {
      turn.claims -= 1;
      if turn.claims == 0 {
        turns.remove(&self.slot);
      }
    }
  }
}

#[derive(Clone)]
pub(crate) struct BlobLane {
  pub gateway: Gateway,
  pub streamer: ArtifactStreamer,
  pub signals: broadcast::Sender<OtaSignal>,
}

impl BlobLane {
  pub(crate) async fn delete(&self, webapp: Uuid, key: String) -> Result<(), BlobError> {
    self
      .gateway
      .webapp()
      .blob_delete(WebappBlobDelete { id: webapp, key })
      .await?;
    Ok(())
  }

  pub(crate) async fn list(&self, webapp: Uuid) -> Result<Vec<WebappBlobEntry>, BlobError> {
    Ok(
      self
        .gateway
        .webapp()
        .blob_list(WebappBlobList { id: webapp })
        .await?
        .entries,
    )
  }
}

enum Terminal {
  Stored(WebappBlobEntry),
  Failed(WebappError),
  LinkLost,
}

struct Abandon {
  gateway: Gateway,
  streamer: ArtifactStreamer,
  transfer_id: Uuid,
  armed: bool,
}

impl Abandon {
  fn armed(lane: &BlobLane, transfer_id: Uuid) -> Self {
    Self {
      gateway: lane.gateway.clone(),
      streamer: lane.streamer.clone(),
      transfer_id,
      armed: true,
    }
  }

  fn disarm(mut self) {
    self.armed = false;
  }
}

impl Drop for Abandon {
  fn drop(&mut self) {
    self.streamer.acks().finish(self.transfer_id);
    if !self.armed {
      return;
    }
    let gateway = self.gateway.clone();
    let transfer_id = self.transfer_id;
    rt::spawn(async move {
      let _ = gateway
        .transfer()
        .abandon(TransferAbandon {
          transfer_id,
          reason: "blob upload ended".into(),
        })
        .await;
    });
  }
}

struct Begun {
  resume: u64,
  guard: Abandon,
}

pub(crate) async fn upload(
  lane: BlobLane,
  mut upload: BlobUpload,
  progress: Option<BlobProgress>,
  claim: Arc<BlobClaim>,
) -> Result<WebappBlobEntry, BlobError> {
  let lease = upload.lease.take();
  let stored = send(lane, upload, progress, claim).await?;
  if let Some(lease) = lease {
    lease.stored();
  }
  Ok(stored)
}

async fn send(
  lane: BlobLane,
  upload: BlobUpload,
  progress: Option<BlobProgress>,
  claim: Arc<BlobClaim>,
) -> Result<WebappBlobEntry, BlobError> {
  let report = move |sent: u64, total: u64| {
    if let Some(progress) = &progress {
      progress(sent, total);
    }
  };
  let attempts = async {
    let total = upload.artifact.size().map_err(BlobError::Source)?;
    if total == 0 {
      return Err(BlobError::Source("the file is empty".into()));
    }
    let total_size =
      u32::try_from(total).map_err(|_| BlobError::Source(format!("{total} bytes is too large to send")))?;
    let sha256 = {
      let artifact = upload.artifact.clone();
      rt::spawn_blocking(move || artifact.sha256())
        .await
        .map_err(BlobError::Source)?
    };
    for _ in 0..ATTEMPTS {
      if let Some(outcome) = attempt(&lane, &claim, &upload, total_size, &sha256, &report).await {
        return outcome;
      }
    }
    Err(BlobError::Unconfirmed)
  };
  let superseded = claim.superseded.notified();
  pin_mut!(attempts, superseded);
  match futures::future::select(attempts, superseded).await {
    Either::Left((outcome, _)) => outcome,
    Either::Right(((), _)) => Err(BlobError::Superseded),
  }
}

async fn begin(lane: &BlobLane, claim: &Arc<BlobClaim>, put: WebappBlobPut) -> Result<Begun, BlobError> {
  let (answered, answer) = oneshot::channel();
  rt::spawn(exchange(lane.clone(), claim.clone(), put, answered));
  answer.await.unwrap_or_else(|_| {
    Err(BlobError::Protocol(
      "the upload ended before the device answered".into(),
    ))
  })
}

async fn exchange(
  lane: BlobLane,
  claim: Arc<BlobClaim>,
  put: WebappBlobPut,
  mut answered: oneshot::Sender<Result<Begun, BlobError>>,
) {
  let mut accepted = {
    let turn = claim.accepted.lock();
    let unwanted = answered.closed();
    pin_mut!(turn, unwanted);
    match futures::future::select(turn, unwanted).await {
      Either::Left((accepted, _)) => accepted,
      Either::Right(((), _)) => return,
    }
  };
  if accepted.order > claim.order {
    let _ = answered.send(Err(BlobError::Superseded));
    return;
  }
  let transfer_id = put.transfer.id;
  let begun = match lane.gateway.webapp().blob_put(put).await {
    Ok(ack) => {
      if accepted.order < claim.order {
        let displaced = std::mem::replace(
          &mut *accepted,
          Accepted {
            order: claim.order,
            superseded: Some(claim.superseded.clone()),
          },
        );
        if let Some(displaced) = displaced.superseded {
          displaced.notify_one();
        }
      }
      Ok(Begun {
        resume: u64::from(ack.resume_from_offset),
        guard: Abandon::armed(&lane, transfer_id),
      })
    }
    Err(failure) => {
      if failure == RequestFailure::Timeout {
        drop(Abandon::armed(&lane, transfer_id));
      }
      Err(failure.into())
    }
  };
  drop(accepted);
  let _ = answered.send(begun);
}

async fn attempt(
  lane: &BlobLane,
  claim: &Arc<BlobClaim>,
  upload: &BlobUpload,
  total_size: u32,
  sha256: &str,
  report: &(dyn Fn(u64, u64) + Send + Sync),
) -> Option<Result<WebappBlobEntry, BlobError>> {
  let mut signals = lane.signals.subscribe();
  let transfer_id = Uuid::now_v7();
  let total = u64::from(total_size);
  let put = WebappBlobPut {
    id: upload.webapp,
    key: upload.key.clone(),
    mime: upload.mime.clone(),
    transfer: TransferRef {
      id: transfer_id,
      total_size,
      sha256: Some(sha256.to_owned()),
    },
  };
  let Begun { resume, guard } = match begin(lane, claim, put).await {
    Ok(begun) => begun,
    Err(error) => return Some(Err(error)),
  };
  report(resume, total);

  let ranges = [SourceRange {
    start: 0,
    length: total,
  }];
  let ticks = |snapshot: OtaPhaseSnapshot| {
    if let OtaPhaseSnapshot::Streaming { sent, total, .. } = snapshot {
      report(sent, total);
    }
  };
  let ack_timeout = Duration::from_millis(ACK_TIMEOUT_MS);
  let delivering = async {
    lane
      .streamer
      .stream(
        transfer_id,
        upload.artifact.as_ref(),
        LABEL,
        &ranges,
        resume,
        ack_timeout,
        &ticks,
      )
      .await?;
    lane
      .streamer
      .acks()
      .await_window(transfer_id, total, 1, ack_timeout)
      .await
      .map_err(SendError::from)
  };
  let terminal = terminal(&mut signals, upload, sha256);
  pin_mut!(delivering, terminal);

  let settled = match futures::future::select(terminal, delivering).await {
    Either::Left((settled, _)) => settled,
    Either::Right((Err(error), _)) => return Some(Err(error.into())),
    Either::Right((Ok(()), terminal)) => match rt::timeout(CONFIRM_DEADLINE, terminal).await {
      Ok(settled) => settled,
      Err(_) => {
        guard.disarm();
        return None;
      }
    },
  };
  guard.disarm();
  Some(match settled {
    Terminal::Stored(stored) => Ok(stored),
    Terminal::Failed(error) => Err(BlobError::Refused(error)),
    Terminal::LinkLost => Err(BlobError::LinkLost),
  })
}

async fn terminal(signals: &mut broadcast::Receiver<OtaSignal>, upload: &BlobUpload, sha256: &str) -> Terminal {
  loop {
    match signals.recv().await {
      Ok(OtaSignal::BlobStored(stored))
        if stored.id == upload.webapp && stored.key == upload.key && stored.sha256 == sha256 =>
      {
        return Terminal::Stored(WebappBlobEntry {
          key: stored.key,
          sha256: stored.sha256,
          mime: upload.mime.clone(),
          size: stored.size,
        });
      }
      Ok(OtaSignal::BlobFailed(failed))
        if failed.id == upload.webapp && failed.key == upload.key && failed.sha256 == sha256 =>
      {
        return Terminal::Failed(failed.error);
      }
      Ok(OtaSignal::LinkLost) | Err(broadcast::error::RecvError::Closed) => return Terminal::LinkLost,
      Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
    }
  }
}

#[cfg(test)]
mod tests {
  use std::sync::mpsc as gate;

  use libbridgething::gateway::{GatewayToBridgeMsgData, GatewayToBridgeWebappMsg};
  use tokio::{sync::mpsc, task::JoinHandle, time::Instant};

  use super::*;
  use crate::{
    bundle::fetch::sha256_hex,
    ota::{
      harness::{DEVICE, FakeDevice, FakeFetch, Spool, TestClock, linked_gateway, pattern, route_into},
      service::{OtaService, OtaServiceDeps},
    },
    transfer::{BytesSource, FRAGMENT_BYTES, FragmentSource, MIN_WINDOW_BYTES},
  };

  const QUIET: Duration = Duration::from_millis(200);
  const PATIENT: Duration = Duration::from_secs(3);
  const PARKED_BYTES: usize = 4 * MIN_WINDOW_BYTES as usize;
  const UNACKED_BYTES: usize = MIN_WINDOW_BYTES as usize - FRAGMENT_BYTES;

  type Put = JoinHandle<Result<WebappBlobEntry, BlobError>>;

  struct Rig {
    service: Arc<OtaService>,
    device: FakeDevice,
    webapp: Uuid,
    _cache: Spool,
  }

  async fn rig() -> Rig {
    let cache = Spool::new();
    let service = OtaService::new(OtaServiceDeps {
      clock: TestClock::new(),
      fetch: FakeFetch::new(),
      cache_dir: cache.path().to_path_buf(),
      data_dir: None,
    });
    let (gateway, device) = linked_gateway();
    service.adopt(DEVICE, gateway.clone()).await;
    route_into(&gateway, &service, DEVICE);
    Rig {
      service,
      device,
      webapp: Uuid::now_v7(),
      _cache: cache,
    }
  }

  impl Rig {
    fn put(&self, key: &str, body: Vec<u8>) -> Put {
      self.put_artifact(key, Arc::new(BytesSource::new(body)), None)
    }

    fn put_artifact(&self, key: &str, artifact: Arc<dyn Artifact>, progress: Option<BlobProgress>) -> Put {
      let service = self.service.clone();
      let upload = BlobUpload {
        webapp: self.webapp,
        key: key.into(),
        mime: "video/mp4".into(),
        artifact,
        lease: None,
      };
      tokio::spawn(async move { service.put_blob(DEVICE, upload, progress).await })
    }

    async fn accept(&mut self, resume: u32) -> WebappBlobPut {
      let (request_id, put) = self.device.await_blob_put().await;
      self.device.ack_blob_put(request_id, resume);
      put
    }

    async fn receive(&mut self, put: &WebappBlobPut, from: u32, acking: bool) -> Vec<u8> {
      let mut received = Vec::new();
      let mut at = from;
      while at < put.transfer.total_size {
        let fragment = self.device.next_fragment(put.transfer.id).await;
        assert_eq!(fragment.offset, at, "fragments arrive contiguous from the resume point");
        at += fragment.bytes.len() as u32;
        received.extend_from_slice(&fragment.bytes);
        if acking {
          self.device.ack(put.transfer.id, at);
        }
      }
      received
    }

    async fn parked(&mut self, key: &str) -> (Put, WebappBlobPut) {
      let put = self.put(key, pattern(PARKED_BYTES));
      let begun = self.accept(0).await;
      let first = self.device.next_fragment(begun.transfer.id).await;
      assert_eq!(first.offset, 0);
      (put, begun)
    }

    async fn finish_parked(&mut self, begun: &WebappBlobPut) {
      let mut at = FRAGMENT_BYTES as u32;
      self.device.ack(begun.transfer.id, at);
      while at < begun.transfer.total_size {
        at += self.device.next_fragment(begun.transfer.id).await.bytes.len() as u32;
        self.device.ack(begun.transfer.id, at);
      }
      self.device.blob_stored(begun);
    }
  }

  async fn settled(put: Put) -> Result<WebappBlobEntry, BlobError> {
    tokio::time::timeout(PATIENT, put)
      .await
      .expect("the put settles")
      .expect("the put task")
  }

  fn token(error: &BlobError) -> String {
    error.to_string().split(':').next().unwrap_or_default().to_owned()
  }

  #[tokio::test]
  async fn a_put_streams_from_the_offset_the_device_holds_and_resolves_with_its_entry() {
    let mut rig = rig().await;
    let body = pattern(96 * 1024);
    let resume = 2 * FRAGMENT_BYTES as u32;
    let (ticks, mut heard) = mpsc::unbounded_channel();
    let progress: BlobProgress = Arc::new(move |sent, total| {
      let _ = ticks.send((sent, total));
    });
    let put = rig.put_artifact("clip", Arc::new(BytesSource::new(body.clone())), Some(progress));

    let begun = rig.accept(resume).await;
    let received = rig.receive(&begun, resume, true).await;
    rig.device.blob_stored(&begun);

    assert_eq!(received, body[resume as usize..]);
    assert_eq!(
      settled(put).await,
      Ok(WebappBlobEntry {
        key: "clip".into(),
        sha256: sha256_hex(&body),
        mime: "video/mp4".into(),
        size: body.len() as u32,
      })
    );
    let total = body.len() as u64;
    let mut seen = Vec::new();
    while let Ok(tick) = heard.try_recv() {
      seen.push(tick);
    }
    assert_eq!(
      seen.first(),
      Some(&(u64::from(resume), total)),
      "progress starts where the device does"
    );
    assert_eq!(seen.last(), Some(&(total, total)), "progress ends on the whole file");
    assert!(
      rig.device.no_abandon(begun.transfer.id, QUIET).await,
      "a stored blob is never abandoned"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_stream_the_device_never_confirms_is_begun_again_after_the_deadline() {
    let mut rig = rig().await;
    let body = pattern(UNACKED_BYTES);
    let put = rig.put("clip", body.clone());
    let first = rig.accept(0).await;
    rig.receive(&first, 0, true).await;
    let acked = Instant::now();

    let (request_id, second) = rig.device.await_blob_put_within(CONFIRM_DEADLINE * 2).await;

    assert!(
      acked.elapsed() >= CONFIRM_DEADLINE,
      "the second begin waits out the confirm deadline, came after {:?}",
      acked.elapsed()
    );
    assert_ne!(second.transfer.id, first.transfer.id);
    assert_eq!(second.transfer.sha256, first.transfer.sha256);
    rig.device.ack_blob_put(request_id, first.transfer.total_size);
    rig.device.blob_stored(&second);
    assert_eq!(put.await.unwrap().map(|stored| stored.sha256), Ok(sha256_hex(&body)));
  }

  #[tokio::test(start_paused = true)]
  async fn a_blob_the_device_never_confirms_fails_unconfirmed_after_two_attempts() {
    let mut rig = rig().await;
    let put = rig.put("clip", pattern(UNACKED_BYTES));
    let first = rig.accept(0).await;
    rig.receive(&first, 0, true).await;
    let acked = Instant::now();
    let (request_id, _) = rig.device.await_blob_put_within(CONFIRM_DEADLINE * 2).await;
    rig.device.ack_blob_put(request_id, first.transfer.total_size);

    let failed = put.await.unwrap().expect_err("nothing ever confirmed the blob");

    assert_eq!(failed, BlobError::Unconfirmed);
    assert_eq!(token(&failed), "unconfirmed");
    assert!(
      acked.elapsed() >= CONFIRM_DEADLINE * 2,
      "each attempt waits its own confirm deadline"
    );
    assert!(
      rig.device.no_blob_put(CONFIRM_DEADLINE * 2).await,
      "two attempts are the limit"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn the_confirm_deadline_runs_from_the_last_ack_not_the_last_fragment() {
    let mut rig = rig().await;
    let body = pattern(UNACKED_BYTES);
    let put = rig.put("clip", body.clone());
    let begun = rig.accept(0).await;
    rig.receive(&begun, 0, false).await;

    tokio::time::sleep(Duration::from_secs(10)).await;
    rig.device.ack(begun.transfer.id, begun.transfer.total_size);
    tokio::time::sleep(CONFIRM_DEADLINE - Duration::from_secs(5)).await;

    assert!(
      rig.device.no_blob_put(Duration::from_millis(1)).await,
      "the deadline has not run out for a blob whose last byte was acked 25 s ago"
    );
    rig.device.blob_stored(&begun);
    assert_eq!(put.await.unwrap().map(|stored| stored.sha256), Ok(sha256_hex(&body)));
  }

  #[tokio::test(start_paused = true)]
  async fn a_final_window_the_device_never_acks_stalls_the_put() {
    let mut rig = rig().await;
    let put = rig.put("clip", pattern(UNACKED_BYTES));
    let begun = rig.accept(0).await;
    rig.receive(&begun, 0, false).await;

    let failed = put.await.unwrap().expect_err("the last window was never acked");

    assert_eq!(token(&failed), "transfer_failed");
    rig
      .device
      .await_abandon_within(CONFIRM_DEADLINE, begun.transfer.id)
      .await;
  }

  #[tokio::test]
  async fn a_failure_the_device_announces_after_the_stream_fails_the_put_with_its_token() {
    let mut rig = rig().await;
    let put = rig.put("clip", pattern(UNACKED_BYTES));
    let begun = rig.accept(0).await;
    rig.receive(&begun, 0, true).await;

    rig.device.blob_failed(
      &begun,
      WebappError::BlobTransferFailed {
        key: "clip".into(),
        reason: "sha256 mismatch".into(),
      },
    );

    let failed = settled(put).await.expect_err("the device refused the bytes");
    assert_eq!(failed.to_string(), "transfer_failed: clip: sha256 mismatch");
  }

  #[tokio::test]
  async fn a_terminal_for_another_digest_of_the_key_does_not_settle_the_put() {
    let mut rig = rig().await;
    let body = pattern(UNACKED_BYTES);
    let put = rig.put("clip", body.clone());
    let begun = rig.accept(0).await;
    rig.receive(&begun, 0, true).await;

    let mut other = begun.clone();
    other.transfer.sha256 = Some(sha256_hex(b"some other upload of this key"));
    rig.device.blob_stored(&other);
    rig.device.blob_failed(
      &other,
      WebappError::BlobTransferFailed {
        key: "clip".into(),
        reason: "not this upload".into(),
      },
    );
    tokio::time::sleep(QUIET).await;
    assert!(!put.is_finished(), "only the terminal for this digest settles the put");

    rig.device.blob_stored(&begun);
    assert_eq!(settled(put).await.map(|stored| stored.sha256), Ok(sha256_hex(&body)));
  }

  #[tokio::test]
  async fn a_newer_put_takes_over_only_once_the_device_accepts_it() {
    let mut rig = rig().await;
    let (older, first) = rig.parked("bg").await;
    let small = pattern(8 * 1024);
    let newer = rig.put("bg", small.clone());

    let (request_id, second) = rig.device.await_blob_put().await;
    assert!(
      !older.is_finished(),
      "the upload in flight keeps running until the device takes the newer one"
    );
    rig.device.ack_blob_put(request_id, 0);

    assert_eq!(settled(older).await, Err(BlobError::Superseded));
    rig.device.await_abandon(first.transfer.id).await;
    rig.receive(&second, 0, true).await;
    rig.device.blob_stored(&second);
    assert_eq!(settled(newer).await.map(|stored| stored.sha256), Ok(sha256_hex(&small)));
  }

  #[tokio::test]
  async fn a_newer_put_the_device_refuses_leaves_the_upload_in_flight_alone() {
    let mut rig = rig().await;
    let (older, first) = rig.parked("bg").await;
    let newer = rig.put("bg", pattern(8 * 1024));

    let (request_id, _) = rig.device.await_blob_put().await;
    rig.device.refuse_webapp(
      request_id,
      WebappError::BlobQuotaExceeded {
        key: "bg".into(),
        max_bytes: 1024,
      },
    );

    let refused = settled(newer).await.expect_err("the device refused the newer put");
    assert_eq!(token(&refused), "quota_exceeded");
    rig.finish_parked(&first).await;
    assert_eq!(
      settled(older).await.map(|stored| stored.sha256),
      Ok(sha256_hex(&pattern(PARKED_BYTES))),
      "a replacement that never started must not cost the upload it would have replaced"
    );
  }

  #[tokio::test]
  async fn an_empty_newer_put_never_reaches_the_device_and_leaves_the_upload_in_flight_alone() {
    let mut rig = rig().await;
    let (older, first) = rig.parked("bg").await;

    let empty = settled(rig.put("bg", Vec::new()))
      .await
      .expect_err("an empty file is refused");

    assert_eq!(empty.to_string(), "invalid: the file is empty");
    assert!(rig.device.no_blob_put(QUIET).await);
    rig.finish_parked(&first).await;
    assert_eq!(
      settled(older).await.map(|stored| stored.sha256),
      Ok(sha256_hex(&pattern(PARKED_BYTES)))
    );
  }

  struct SlowToHash {
    bytes: BytesSource,
    hashing: mpsc::UnboundedSender<()>,
    release: Mutex<gate::Receiver<()>>,
  }

  impl FragmentSource for SlowToHash {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
      self.bytes.read_at(offset, buf)
    }
  }

  impl Artifact for SlowToHash {
    fn size(&self) -> Result<u64, String> {
      self.bytes.size()
    }

    fn sha256(&self) -> Result<String, String> {
      let _ = self.hashing.send(());
      let _ = self.release.lock().unwrap().recv();
      self.bytes.sha256()
    }
  }

  #[tokio::test]
  async fn an_older_put_that_reaches_the_device_last_gives_way_without_beginning() {
    let mut rig = rig().await;
    let (hashing, mut started) = mpsc::unbounded_channel();
    let (release, gate) = gate::channel();
    let older = rig.put_artifact(
      "bg",
      Arc::new(SlowToHash {
        bytes: BytesSource::new(pattern(64 * 1024)),
        hashing,
        release: Mutex::new(gate),
      }),
      None,
    );
    started.recv().await.expect("the older put is hashing");

    let small = pattern(8 * 1024);
    let newer = rig.put("bg", small.clone());
    let begun = rig.accept(0).await;
    rig.receive(&begun, 0, true).await;
    rig.device.blob_stored(&begun);
    assert_eq!(settled(newer).await.map(|stored| stored.sha256), Ok(sha256_hex(&small)));

    release.send(()).expect("the older put is still hashing");

    assert_eq!(settled(older).await, Err(BlobError::Superseded));
    assert!(
      rig.device.no_blob_put(QUIET).await,
      "the call the page made first must not overwrite the one it made last"
    );
  }

  #[tokio::test]
  async fn a_put_dropped_before_the_device_answers_abandons_the_transfer_it_opened() {
    let mut rig = rig().await;
    let put = rig.put("clip", pattern(UNACKED_BYTES));
    let (request_id, begun) = rig.device.await_blob_put().await;

    put.abort();
    assert!(put.await.unwrap_err().is_cancelled());
    rig.device.ack_blob_put(request_id, 0);

    rig.device.await_abandon(begun.transfer.id).await;
  }

  #[tokio::test(start_paused = true)]
  async fn a_begin_the_device_never_answers_fails_the_put_and_abandons_the_transfer() {
    let mut rig = rig().await;
    let put = rig.put("clip", pattern(UNACKED_BYTES));
    let (_, begun) = rig.device.await_blob_put().await;

    let failed = put.await.unwrap().expect_err("the device never answered");

    assert_eq!(token(&failed), "protocol");
    rig
      .device
      .await_abandon_within(CONFIRM_DEADLINE, begun.transfer.id)
      .await;
  }

  #[tokio::test]
  async fn a_released_link_fails_the_upload_as_link_lost_and_later_verbs_as_not_connected() {
    let mut rig = rig().await;
    let (put, _) = rig.parked("clip").await;

    rig.service.release(DEVICE).await;

    assert_eq!(settled(put).await, Err(BlobError::LinkLost));
    assert_eq!(
      settled(rig.put("clip", pattern(1024))).await,
      Err(BlobError::NotConnected)
    );
    assert_eq!(
      rig.service.delete_blob(DEVICE, rig.webapp, "clip".into()).await,
      Err(BlobError::NotConnected)
    );
    assert_eq!(
      rig.service.list_blobs(DEVICE, rig.webapp).await,
      Err(BlobError::NotConnected)
    );
  }

  #[tokio::test]
  async fn a_refused_delete_or_list_carries_the_same_tokens_a_put_does() {
    let mut rig = rig().await;
    let webapp = rig.webapp;

    let service = rig.service.clone();
    let deleting = tokio::spawn(async move { service.delete_blob(DEVICE, webapp, "no/slashes".into()).await });
    let request_id = rig
      .device
      .next_matching(|msg| match &msg.data {
        GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::BlobDelete(_)) => Some(msg.id),
        _ => None,
      })
      .await;
    rig.device.refuse_webapp(
      request_id,
      WebappError::InvalidBlobKey {
        key: "no/slashes".into(),
        reason: "keys are one path segment".into(),
      },
    );
    let refused = deleting.await.unwrap().expect_err("the key is refused");
    assert_eq!(
      refused.to_string(),
      "invalid_key: no/slashes: keys are one path segment"
    );

    let service = rig.service.clone();
    let listing = tokio::spawn(async move { service.list_blobs(DEVICE, webapp).await });
    let request_id = rig
      .device
      .next_matching(|msg| match &msg.data {
        GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::BlobList(_)) => Some(msg.id),
        _ => None,
      })
      .await;
    rig
      .device
      .refuse_webapp(request_id, WebappError::WebappNotFound { id: webapp.to_string() });
    let refused = listing.await.unwrap().expect_err("the webapp is gone");
    assert_eq!(token(&refused), "not_found");
  }

  #[test]
  fn every_blob_failure_leads_with_a_snake_case_token() {
    let key = || "clip".to_owned();
    let failures = [
      BlobError::NotConnected,
      BlobError::Source("the file is empty".into()),
      BlobError::Handle(HandleError::Unknown("gone".into())),
      BlobError::Handle(HandleError::TooLarge),
      BlobError::Handle(HandleError::Busy { budget: 1 }),
      BlobError::Handle(HandleError::Encoding("bad padding".into())),
      BlobError::Handle(HandleError::Io("disk".into())),
      BlobError::Refused(WebappError::BlobTooLarge {
        key: key(),
        max_bytes: 1,
      }),
      BlobError::Refused(WebappError::BlobQuotaExceeded {
        key: key(),
        max_bytes: 1,
      }),
      BlobError::Refused(WebappError::BlobStorageFull {
        key: key(),
        needed_bytes: 1,
      }),
      BlobError::Refused(WebappError::InvalidBlobKey {
        key: key(),
        reason: "bad".into(),
      }),
      BlobError::Refused(WebappError::InvalidBlobPut {
        key: key(),
        reason: "bad".into(),
      }),
      BlobError::Refused(WebappError::BlobTransferFailed {
        key: key(),
        reason: "bad".into(),
      }),
      BlobError::Refused(WebappError::WebappNotFound { id: "gone".into() }),
      BlobError::Refused(WebappError::InvalidManifest { reason: "bad".into() }),
      BlobError::Stream("stalled".into()),
      BlobError::LinkLost,
      BlobError::Superseded,
      BlobError::Unconfirmed,
      BlobError::Unsupported,
      BlobError::Protocol("timeout".into()),
    ];
    let tokens: Vec<String> = failures.iter().map(token).collect();

    assert_eq!(
      tokens,
      [
        "not_connected",
        "invalid",
        "unknown_handle",
        "too_large",
        "busy",
        "invalid",
        "io",
        "too_large",
        "quota_exceeded",
        "storage_full",
        "invalid_key",
        "invalid",
        "transfer_failed",
        "not_found",
        "refused",
        "transfer_failed",
        "link_lost",
        "superseded",
        "unconfirmed",
        "unsupported",
        "protocol",
      ]
    );
    for failure in &failures {
      assert!(
        failure.to_string().contains(": "),
        "a token is followed by a colon and a message: {failure}"
      );
    }
  }

  #[test]
  fn a_link_that_closes_under_a_request_or_a_stream_is_link_lost() {
    assert_eq!(
      BlobError::from(RequestFailure::<WebappError>::Disconnected),
      BlobError::LinkLost
    );
    assert_eq!(
      BlobError::from(SendError::SinkClosed { offset: 4096 }),
      BlobError::LinkLost
    );
    assert_eq!(
      token(&BlobError::from(SendError::UnexpectedEof { offset: 1, total: 2 })),
      "transfer_failed"
    );
    assert_eq!(
      token(&BlobError::from(RequestFailure::<WebappError>::Timeout)),
      "protocol"
    );
  }

  #[test]
  fn a_device_that_predates_blobs_is_unsupported() {
    for refusal in [WireError::Unsupported, WireError::Unimplemented] {
      assert_eq!(
        token(&BlobError::from(RequestFailure::<WebappError>::Protocol(refusal))),
        "unsupported"
      );
    }
    assert_eq!(
      token(&BlobError::from(RequestFailure::<WebappError>::Protocol(
        WireError::Malformed { reason: "bad".into() }
      ))),
      "protocol"
    );
  }

  #[test]
  fn a_slot_is_forgotten_once_no_put_claims_it() {
    let puts = Arc::new(BlobPuts::default());
    let slot = || (DEVICE.to_owned(), Uuid::nil(), "bg".to_owned());

    let first = puts.claim(slot());
    let second = puts.claim(slot());
    assert!(
      second.order > first.order,
      "claims are ordered by the call that made them"
    );
    assert!(Arc::ptr_eq(&first.accepted, &second.accepted));

    drop(first);
    assert_eq!(puts.turns.lock().unwrap().len(), 1);
    drop(second);
    assert!(puts.turns.lock().unwrap().is_empty());
  }
}
