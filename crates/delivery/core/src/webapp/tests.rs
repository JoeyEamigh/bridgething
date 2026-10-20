use std::{
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_gateway::Gateway;
use libbridgething::{
  WebappError,
  gateway::{
    BridgeToGatewayMsgData, BridgeToGatewayTransferMsg, TransferAck, TransferBody, TransferRef, WebappResourceKind,
    WebappResourceReply,
  },
  wire::WireError,
};
use uuid::Uuid;

use crate::{
  blob::{FsBlobStore, FsSlotIndex},
  bundle::fetch::{ArtifactFetch, DownloadRequest, FetchError, sha256_hex},
  harness::{FakeDevice, linked_gateway},
  seam::{BlobStore, Clock, SlotIndex, SystemClock},
  transfer::{AckSink, MAX_TRANSFER_BYTES, TransferReceiver},
  webapp::{ResourceOrigin, WebappResourceError, WebappResourceService},
};

const PATIENT: Duration = Duration::from_secs(3);

struct DroppedAcks;

impl AckSink for DroppedAcks {
  fn ack(&self, _ack: TransferAck) {}
}

struct Rig {
  gateway: Gateway,
  device: FakeDevice,
  service: WebappResourceService,
  blobs: Arc<dyn BlobStore>,
  receiver: Arc<TransferReceiver>,
  index_path: std::path::PathBuf,
  _cache: tempfile::TempDir,
}

impl Rig {
  fn reopened(&self) -> WebappResourceService {
    WebappResourceService::new(
      self.blobs.clone(),
      Arc::new(FsSlotIndex::new(&self.index_path)) as Arc<dyn SlotIndex>,
      self.receiver.clone(),
    )
  }
}

fn rig() -> Rig {
  rig_with_timeout(Duration::from_secs(30))
}

fn rig_with_timeout(timeout: Duration) -> Rig {
  let (gateway, device) = linked_gateway();
  let receiver = TransferReceiver::new(Arc::new(DroppedAcks), Arc::new(SystemClock) as Arc<dyn Clock>);

  let mut inbound = gateway.events();
  let fed = receiver.clone();
  tokio::spawn(async move {
    while let Ok(msg) = inbound.recv().await {
      match msg.data {
        BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Fragment(fragment)) => fed.on_fragment(fragment),
        BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Abandon(abandon)) => fed.on_abandon(abandon),
        _ => {}
      }
    }
  });

  let cache = tempfile::tempdir().expect("a scratch cache");
  let blobs: Arc<dyn BlobStore> = Arc::new(FsBlobStore::new(cache.path().join("blobs")));
  let index_path = cache.path().join("slots.json");
  let slots: Arc<dyn SlotIndex> = Arc::new(FsSlotIndex::new(&index_path));
  let service = WebappResourceService::with_timeout(blobs.clone(), slots, receiver.clone(), timeout);

  Rig {
    gateway,
    device,
    service,
    blobs,
    receiver,
    index_path,
    _cache: cache,
  }
}

fn ramp(len: usize) -> Vec<u8> {
  (0..len).map(|at| (at % 251) as u8).collect()
}

fn not_available(webapp_id: Uuid) -> WebappError {
  WebappError::ResourceNotAvailable {
    id: webapp_id.to_string(),
  }
}

#[tokio::test]
async fn an_inline_body_lands_in_the_store_and_the_next_fetch_offers_its_digest() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let body = ramp(2048);
  let digest = sha256_hex(&body);

  let fetching = rig
    .service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Icon, None);
  let driving = async {
    let (id, request) = rig.device.next_resource_request().await;
    assert_eq!(request.id, webapp_id);
    assert_eq!(request.kind, WebappResourceKind::Icon);
    assert_eq!(request.have, None, "a first fetch has nothing to offer");
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Icon,
        sha256: digest.clone(),
        mime: Some("image/png".into()),
        body: Some(TransferBody::Inline(body.clone())),
      },
    );
  };

  let (resolved, ()) = tokio::join!(fetching, driving);
  let resolved = resolved.expect("the fetch resolved");
  assert_eq!(resolved.digest, digest);
  assert_eq!(resolved.mime.as_deref(), Some("image/png"));
  assert_eq!(
    rig.blobs.get(&digest).expect("readable"),
    Some(body),
    "the body is what the store holds under its digest"
  );

  let fetching = rig
    .service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Icon, None);
  let driving = async {
    let (id, request) = rig.device.next_resource_request().await;
    assert_eq!(
      request.have.as_deref(),
      Some(digest.as_str()),
      "a second fetch offers what is cached"
    );
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Icon,
        sha256: digest.clone(),
        mime: Some("image/png".into()),
        body: None,
      },
    );
  };

  let (again, ()) = tokio::join!(fetching, driving);
  let again = again.expect("the body-less reply resolved");
  assert_eq!(again.digest, digest, "a body-less reply serves what is cached");
  assert_eq!(again.mime.as_deref(), Some("image/png"));
}

#[tokio::test]
async fn a_streamed_body_reassembles_behind_the_reply_and_lands_in_the_store() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let body = ramp(24 * 1024);
  let digest = sha256_hex(&body);
  let transfer_id = Uuid::now_v7();

  let fetching = rig
    .service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Settings, None);
  let driving = async {
    let (id, request) = rig.device.next_resource_request().await;
    assert_eq!(request.kind, WebappResourceKind::Settings);
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Settings,
        sha256: digest.clone(),
        mime: Some("text/html".into()),
        body: Some(TransferBody::Stream(TransferRef {
          id: transfer_id,
          total_size: body.len() as u32,
          sha256: Some(digest.clone()),
        })),
      },
    );
    rig.device.stream(transfer_id, &body, 0..body.len(), 4 * 1024);
  };

  let (resolved, ()) = tokio::join!(fetching, driving);
  let resolved = resolved.expect("the streamed fetch resolved");
  assert_eq!(resolved.digest, digest);
  assert_eq!(resolved.mime.as_deref(), Some("text/html"));
  assert_eq!(rig.blobs.get(&digest).expect("readable"), Some(body));
}

#[tokio::test]
async fn a_domain_rejection_keeps_its_cause() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();

  let fetching = rig
    .service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Icon, None);
  let driving = async {
    let (id, _) = rig.device.next_resource_request().await;
    rig.device.refuse_webapp(id, not_available(webapp_id));
  };

  let (failed, ()) = tokio::join!(fetching, driving);
  assert_eq!(
    failed.expect_err("a rejected fetch fails"),
    WebappResourceError::Domain(not_available(webapp_id)),
    "the daemon's own error must survive to the caller"
  );
}

#[tokio::test]
async fn a_protocol_refusal_keeps_its_cause() {
  let mut rig = rig();

  let fetching = rig
    .service
    .fetch(&rig.gateway, Uuid::now_v7(), WebappResourceKind::Icon, None);
  let driving = async {
    let (id, _) = rig.device.next_resource_request().await;
    rig.device.refuse(id, WireError::Unsupported);
  };

  let (failed, ()) = tokio::join!(fetching, driving);
  assert_eq!(
    failed.expect_err("a refused fetch fails"),
    WebappResourceError::Wire(WireError::Unsupported)
  );
}

#[tokio::test]
async fn a_body_less_reply_with_nothing_cached_is_a_stale_cache_failure() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();

  let fetching = rig
    .service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Icon, None);
  let driving = async {
    let (id, _) = rig.device.next_resource_request().await;
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Icon,
        sha256: sha256_hex(b"nothing here"),
        mime: Some("image/png".into()),
        body: None,
      },
    );
  };

  let (failed, ()) = tokio::join!(fetching, driving);
  assert_eq!(
    failed.expect_err("a cache-current reply with no cache fails"),
    WebappResourceError::StaleCacheMissing {
      webapp_id,
      kind: WebappResourceKind::Icon
    }
  );
}

#[tokio::test]
async fn a_body_that_does_not_match_the_replys_digest_fails_typed_and_stores_nothing() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let body = vec![1u8; 512];
  let claimed = sha256_hex(&vec![2u8; 512]);

  let fetching = rig
    .service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Icon, None);
  let driving = async {
    let (id, _) = rig.device.next_resource_request().await;
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Icon,
        sha256: claimed.clone(),
        mime: Some("image/png".into()),
        body: Some(TransferBody::Inline(body.clone())),
      },
    );
  };

  let (failed, ()) = tokio::join!(fetching, driving);
  assert_eq!(
    failed.expect_err("a mismatched body fails"),
    WebappResourceError::ShaMismatch {
      expected: claimed,
      got: sha256_hex(&body),
    }
  );
  assert!(
    !rig.blobs.contains(&sha256_hex(&body)),
    "a body the daemon mis-described must not enter the store"
  );
  assert_eq!(
    rig.service.cached(webapp_id, WebappResourceKind::Icon),
    None,
    "and the slot must stay empty"
  );
}

#[tokio::test]
async fn a_stream_that_never_arrives_times_the_fetch_out_rather_than_hanging_it() {
  let mut rig = rig_with_timeout(Duration::from_millis(300));
  let webapp_id = Uuid::now_v7();
  let transfer_id = Uuid::now_v7();

  let fetching = rig
    .service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Settings, None);
  let driving = async {
    let (id, _) = rig.device.next_resource_request().await;
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Settings,
        sha256: sha256_hex(b"never sent"),
        mime: Some("text/html".into()),
        body: Some(TransferBody::Stream(TransferRef {
          id: transfer_id,
          total_size: 32 * 1024,
          sha256: None,
        })),
      },
    );
  };

  let (failed, ()) = tokio::time::timeout(PATIENT, async { tokio::join!(fetching, driving) })
    .await
    .expect("a silent stream must not park the fetch");
  assert!(
    matches!(
      failed.expect_err("a silent stream fails"),
      WebappResourceError::Transfer(crate::transfer::TransferReceiveError::TimedOut { .. })
    ),
    "the fetch surfaces the transfer timeout"
  );
}

#[tokio::test]
async fn a_replaced_resource_drops_the_digest_it_displaced() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let first = ramp(1024);
  let second = ramp(2048);
  let (first_digest, second_digest) = (sha256_hex(&first), sha256_hex(&second));
  let seen = Arc::new(Mutex::new(Vec::new()));

  for (body, digest) in [(&first, &first_digest), (&second, &second_digest)] {
    let fetching = rig
      .service
      .fetch(&rig.gateway, webapp_id, WebappResourceKind::Overlay, None);
    let seen = seen.clone();
    let driving = async {
      let (id, request) = rig.device.next_resource_request().await;
      seen.lock().unwrap().push(request.have.clone());
      rig.device.resource(
        id,
        WebappResourceReply {
          id: webapp_id,
          kind: WebappResourceKind::Overlay,
          sha256: digest.clone(),
          mime: Some("text/html".into()),
          body: Some(TransferBody::Inline(body.clone())),
        },
      );
    };
    let (resolved, ()) = tokio::join!(fetching, driving);
    assert_eq!(&resolved.expect("the fetch resolved").digest, digest);
  }

  assert_eq!(
    seen.lock().unwrap().clone(),
    vec![None, Some(first_digest.clone())],
    "the second fetch offers what the first cached"
  );
  assert!(
    !rig.blobs.contains(&first_digest),
    "the superseded body must not be left behind"
  );
  assert!(rig.blobs.contains(&second_digest));
}

async fn fetch_once(
  device: &mut FakeDevice,
  gateway: &Gateway,
  service: &WebappResourceService,
  webapp_id: Uuid,
  kind: WebappResourceKind,
  body: &[u8],
  mime: &str,
) -> (Option<String>, String) {
  let digest = sha256_hex(body);
  let fetching = service.fetch(gateway, webapp_id, kind, None);
  let offered = Arc::new(Mutex::new(None));
  let seen = offered.clone();
  let driving = async {
    let (id, request) = device.next_resource_request().await;
    *seen.lock().unwrap() = request.have.clone();
    device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind,
        sha256: digest.clone(),
        mime: Some(mime.to_owned()),
        body: Some(TransferBody::Inline(body.to_vec())),
      },
    );
  };
  let (resolved, ()) = tokio::join!(fetching, driving);
  assert_eq!(resolved.expect("the fetch resolved").digest, digest);
  let offered = offered.lock().unwrap().clone();
  (offered, digest)
}

#[tokio::test]
async fn a_service_built_over_a_written_index_offers_what_the_last_run_cached() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let body = ramp(1536);

  let (offered, digest) = fetch_once(
    &mut rig.device,
    &rig.gateway,
    &rig.service,
    webapp_id,
    WebappResourceKind::Icon,
    &body,
    "image/png",
  )
  .await;
  assert_eq!(offered, None, "a first fetch has nothing to offer");

  let cold = rig.reopened();
  assert_eq!(
    cold.cached(webapp_id, WebappResourceKind::Icon),
    Some(crate::seam::CachedResource {
      digest: digest.clone(),
      mime: Some("image/png".into()),
    }),
    "a cold start reads its slots off disk"
  );

  let (offered, again) = fetch_once(
    &mut rig.device,
    &rig.gateway,
    &cold,
    webapp_id,
    WebappResourceKind::Icon,
    &body,
    "image/png",
  )
  .await;
  assert_eq!(offered.as_deref(), Some(digest.as_str()), "and offers them as `have`");
  assert_eq!(again, digest);
}

#[tokio::test]
async fn a_digest_two_slots_share_survives_one_of_them_being_displaced() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let shared = ramp(1024);
  let replacement = ramp(2048);
  let (shared_digest, replacement_digest) = (sha256_hex(&shared), sha256_hex(&replacement));

  fetch_once(
    &mut rig.device,
    &rig.gateway,
    &rig.service,
    webapp_id,
    WebappResourceKind::Icon,
    &shared,
    "image/png",
  )
  .await;
  fetch_once(
    &mut rig.device,
    &rig.gateway,
    &rig.service,
    webapp_id,
    WebappResourceKind::Overlay,
    &shared,
    "image/png",
  )
  .await;
  assert!(rig.blobs.contains(&shared_digest));

  fetch_once(
    &mut rig.device,
    &rig.gateway,
    &rig.service,
    webapp_id,
    WebappResourceKind::Icon,
    &replacement,
    "image/png",
  )
  .await;

  assert!(
    rig.blobs.contains(&shared_digest),
    "the overlay still names that digest, so displacing the icon must not remove it"
  );
  assert!(rig.blobs.contains(&replacement_digest));
  assert_eq!(
    rig
      .service
      .cached(webapp_id, WebappResourceKind::Overlay)
      .map(|held| held.digest),
    Some(shared_digest),
    "and the overlay still resolves"
  );
}

#[tokio::test]
async fn an_index_that_does_not_parse_costs_a_refetch_and_nothing_else() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let body = ramp(768);

  let (_, digest) = fetch_once(
    &mut rig.device,
    &rig.gateway,
    &rig.service,
    webapp_id,
    WebappResourceKind::Settings,
    &body,
    "text/html",
  )
  .await;
  std::fs::write(&rig.index_path, b"\x00 not json at all").expect("the index is corrupted");

  let after = rig.reopened();
  assert_eq!(
    after.cached(webapp_id, WebappResourceKind::Settings),
    None,
    "an unreadable index reports no slots rather than failing"
  );

  let (offered, again) = fetch_once(
    &mut rig.device,
    &rig.gateway,
    &after,
    webapp_id,
    WebappResourceKind::Settings,
    &body,
    "text/html",
  )
  .await;
  assert_eq!(
    offered, None,
    "so the fetch offers nothing and the body comes down again"
  );
  assert_eq!(again, digest);
  assert_eq!(
    rig
      .reopened()
      .cached(webapp_id, WebappResourceKind::Settings)
      .map(|held| held.digest),
    Some(digest),
    "and the index is writable again from there"
  );
}

struct HostedPage {
  bytes: Vec<u8>,
  hits: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl ArtifactFetch for HostedPage {
  async fn bytes(&self, _url: &str, limit: usize) -> Result<Vec<u8>, FetchError> {
    *self.hits.lock().unwrap() += 1;
    if self.bytes.len() > limit {
      return Err(FetchError::TooLarge { limit });
    }
    Ok(self.bytes.clone())
  }

  async fn download(&self, _request: DownloadRequest) -> Result<std::path::PathBuf, FetchError> {
    Err(FetchError::Transport(
      "a hosted resource is never spooled to disk".into(),
    ))
  }
}

fn hosting(rig: &Rig, bytes: Vec<u8>) -> (WebappResourceService, Arc<Mutex<usize>>) {
  let hits = Arc::new(Mutex::new(0));
  let service = rig.reopened().with_fetch(Arc::new(HostedPage {
    bytes,
    hits: hits.clone(),
  }));
  (service, hits)
}

fn origin_for(bytes: &[u8]) -> ResourceOrigin {
  ResourceOrigin {
    url: "https://apps.example.com/s/page.html".into(),
    sha256: sha256_hex(bytes),
    size: bytes.len() as u64,
    mime: Some("text/html".into()),
  }
}

#[tokio::test]
async fn a_hosted_page_that_matches_the_declared_digest_is_served_without_asking_the_device() {
  let rig = rig();
  let webapp_id = Uuid::now_v7();
  let body = ramp(40_000);
  let (service, hits) = hosting(&rig, body.clone());

  let resource = service
    .fetch(
      &rig.gateway,
      webapp_id,
      WebappResourceKind::Settings,
      Some(&origin_for(&body)),
    )
    .await
    .expect("bytes matching the declared digest are taken as the page");

  assert_eq!(resource.digest, sha256_hex(&body));
  assert_eq!(*hits.lock().unwrap(), 1, "one download, no round trip to the device");
  assert_eq!(rig.blobs.get(&resource.digest).expect("stored").unwrap(), body);
}

#[tokio::test]
async fn a_second_open_reads_the_cache_without_the_network_or_the_device() {
  let rig = rig();
  let webapp_id = Uuid::now_v7();
  let body = ramp(40_000);
  let origin = origin_for(&body);
  let (service, hits) = hosting(&rig, body.clone());

  service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Settings, Some(&origin))
    .await
    .expect("first open");
  let again = service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Settings, Some(&origin))
    .await
    .expect("second open");

  assert_eq!(again.digest, sha256_hex(&body));
  assert_eq!(
    *hits.lock().unwrap(),
    1,
    "the cached digest already matches what the device reports"
  );
}

#[tokio::test]
async fn an_overlay_ignores_a_hosted_origin_and_takes_the_bytes_off_the_device() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let installed = ramp(2048);
  let hosted = ramp(4096);
  let (service, hits) = hosting(&rig, hosted.clone());
  let origin = origin_for(&hosted);

  let fetching = service.fetch(&rig.gateway, webapp_id, WebappResourceKind::Overlay, Some(&origin));
  let driving = async {
    let (id, request) = rig.device.next_resource_request().await;
    assert_eq!(request.kind, WebappResourceKind::Overlay);
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Overlay,
        sha256: sha256_hex(&installed),
        mime: Some("text/javascript".into()),
        body: Some(TransferBody::Inline(installed.clone())),
      },
    );
  };
  let (resource, ()) = tokio::join!(fetching, driving);

  assert_eq!(
    resource.expect("the overlay comes off the device").digest,
    sha256_hex(&installed),
    "injected javascript is never taken from a catalog url"
  );
  assert_eq!(*hits.lock().unwrap(), 0, "and the hosted url was never opened");
}

#[tokio::test]
async fn an_origin_the_device_never_vouched_for_is_still_served_to_the_caller_that_supplied_it() {
  let rig = rig();
  let webapp_id = Uuid::now_v7();
  let installed = ramp(2048);
  let elsewhere = ramp(4096);
  let origin = origin_for(&elsewhere);
  let (service, hits) = hosting(&rig, elsewhere.clone());

  let resource = service
    .fetch(&rig.gateway, webapp_id, WebappResourceKind::Settings, Some(&origin))
    .await
    .expect("the service trusts the digest it was handed");

  assert_eq!(resource.digest, sha256_hex(&elsewhere));
  assert_ne!(
    resource.digest,
    sha256_hex(&installed),
    "bytes the device never reported are served when a caller vouches for them"
  );
  assert_eq!(
    *hits.lock().unwrap(),
    1,
    "and the device is never consulted to contradict it"
  );
}

#[tokio::test]
async fn a_hosted_page_that_is_not_the_installed_one_falls_back_to_the_device() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let installed = ramp(2048);
  let impostor = ramp(3072);
  let mut origin = origin_for(&installed);
  origin.size = impostor.len() as u64;
  let (service, hits) = hosting(&rig, impostor);

  let fetching = service.fetch(&rig.gateway, webapp_id, WebappResourceKind::Settings, Some(&origin));
  let driving = async {
    let (id, _) = rig.device.next_resource_request().await;
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Settings,
        sha256: sha256_hex(&installed),
        mime: Some("text/html".into()),
        body: Some(TransferBody::Inline(installed.clone())),
      },
    );
  };
  let (resource, ()) = tokio::join!(fetching, driving);

  assert_eq!(
    resource.expect("the link still has the real page").digest,
    sha256_hex(&installed)
  );
  assert_eq!(*hits.lock().unwrap(), 1, "the hosted copy was tried once and refused");
}

#[tokio::test]
async fn a_hosted_page_shorter_than_the_catalog_declares_falls_back_to_the_device() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let installed = ramp(2048);
  let truncated = installed[..1024].to_vec();
  let origin = origin_for(&installed);
  let (service, hits) = hosting(&rig, truncated);

  let fetching = service.fetch(&rig.gateway, webapp_id, WebappResourceKind::Settings, Some(&origin));
  let driving = async {
    let (id, _) = rig.device.next_resource_request().await;
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Settings,
        sha256: sha256_hex(&installed),
        mime: Some("text/html".into()),
        body: Some(TransferBody::Inline(installed.clone())),
      },
    );
  };
  let (resource, ()) = tokio::join!(fetching, driving);

  assert_eq!(
    resource.expect("the link has the whole page").digest,
    sha256_hex(&installed)
  );
  assert_eq!(*hits.lock().unwrap(), 1, "the short copy was read once and refused");
}

struct StuckOrigin;

#[async_trait::async_trait]
impl ArtifactFetch for StuckOrigin {
  async fn bytes(&self, _url: &str, _limit: usize) -> Result<Vec<u8>, FetchError> {
    std::future::pending().await
  }

  async fn download(&self, _request: DownloadRequest) -> Result<std::path::PathBuf, FetchError> {
    Err(FetchError::Transport(
      "a hosted resource is never spooled to disk".into(),
    ))
  }
}

#[tokio::test]
async fn a_hosted_page_that_never_answers_falls_back_to_the_device() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let installed = ramp(2048);
  let origin = origin_for(&installed);
  let service = WebappResourceService::with_timeout(
    rig.blobs.clone(),
    Arc::new(FsSlotIndex::new(&rig.index_path)) as Arc<dyn SlotIndex>,
    rig.receiver.clone(),
    Duration::from_millis(200),
  )
  .with_fetch(Arc::new(StuckOrigin));

  let fetching = service.fetch(&rig.gateway, webapp_id, WebappResourceKind::Settings, Some(&origin));
  let driving = async {
    let (id, _) = rig.device.next_resource_request().await;
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Settings,
        sha256: sha256_hex(&installed),
        mime: Some("text/html".into()),
        body: Some(TransferBody::Inline(installed.clone())),
      },
    );
  };
  let (resource, ()) = tokio::time::timeout(PATIENT, async { tokio::join!(fetching, driving) })
    .await
    .expect("a hosted url that hangs must not hang the page");

  assert_eq!(
    resource.expect("the link still has the page").digest,
    sha256_hex(&installed)
  );
}

#[tokio::test]
async fn a_hosted_page_declared_past_the_resource_cap_is_never_fetched() {
  let mut rig = rig();
  let webapp_id = Uuid::now_v7();
  let installed = ramp(2048);
  let mut origin = origin_for(&installed);
  origin.size = MAX_TRANSFER_BYTES + 1;
  let (service, hits) = hosting(&rig, installed.clone());

  let fetching = service.fetch(&rig.gateway, webapp_id, WebappResourceKind::Settings, Some(&origin));
  let driving = async {
    let (id, _) = rig.device.next_resource_request().await;
    rig.device.resource(
      id,
      WebappResourceReply {
        id: webapp_id,
        kind: WebappResourceKind::Settings,
        sha256: sha256_hex(&installed),
        mime: Some("text/html".into()),
        body: Some(TransferBody::Inline(installed.clone())),
      },
    );
  };
  let (resource, ()) = tokio::join!(fetching, driving);

  assert_eq!(resource.expect("the link has the page").digest, sha256_hex(&installed));
  assert_eq!(
    *hits.lock().unwrap(),
    0,
    "a size past the cap is refused before the url is opened"
  );
}
