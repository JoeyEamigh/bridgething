use std::{
  collections::{HashMap, HashSet},
  path::PathBuf,
  time::Duration,
};

use bridgething_gateway::{Gateway, RequestFailure};
use bridgething_test_harness::{Harness, proptest_cases};
use futures::{Stream, StreamExt, future::join_all};
use libbridgething::{
  WebappError,
  client::BridgeToClientBlobMsgEvent,
  gateway::{
    BridgeToGatewayTransferMsgEvent, BridgeToGatewayWebappMsgEvent, TransferAbandon, TransferFragment, TransferRef,
    WebappBlobDelete, WebappBlobEntry, WebappBlobFailed, WebappBlobList, WebappBlobPut, WebappBlobPutAck,
    WebappBlobStored, WebappSetSlot, WebappSlot, WebappUninstall,
  },
  wire::WireError,
};
use proptest::prelude::*;
use sha2::{Digest, Sha256};
use tokio_util::bytes::Bytes;
use uuid::Uuid;

const EVENT_WAIT: Duration = Duration::from_secs(10);
const SETTLE: Duration = Duration::from_millis(500);
const FRAGMENT_LEN: usize = 4 * 1024;
const BLOB_MAX_BYTES: u32 = 32 * 1024 * 1024;
const QUOTA_BYTES: u32 = 64 * 1024 * 1024;
const BEFORE_EVERY_UPLOAD: &str = "Thu, 01 Jan 1970 00:00:01 GMT";
const AFTER_EVERY_UPLOAD: &str = "Fri, 01 Jan 2100 00:00:00 GMT";

type WebappEvents = std::pin::Pin<Box<dyn Stream<Item = BridgeToGatewayWebappMsgEvent> + Send>>;
type TransferEvents = std::pin::Pin<Box<dyn Stream<Item = BridgeToGatewayTransferMsgEvent> + Send>>;

fn sha_hex(bytes: &[u8]) -> String {
  let mut h = Sha256::new();
  h.update(bytes);
  hex::encode(h.finalize())
}

fn body(len: usize, seed: u8) -> Vec<u8> {
  (0..len)
    .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
    .collect()
}

async fn plant(harness: &Harness, overlay: bool) -> Uuid {
  let id = Uuid::now_v7();
  let dir = harness.state_dir().join("webapps").join(id.simple().to_string());
  std::fs::create_dir_all(&dir).expect("bundle dir");
  std::fs::write(dir.join("index.html"), b"<h1>blobs</h1>").expect("index");
  let overlay_field = if overlay {
    std::fs::write(dir.join("overlay.js"), "/* overlay */").expect("overlay");
    r#","overlay":"overlay.js""#
  } else {
    ""
  };
  std::fs::write(
    dir.join("manifest.json"),
    format!(r#"{{"id":"{id}","name":"blobs","version":"0.1.0","config":[]{overlay_field}}}"#),
  )
  .expect("manifest");
  harness.state().webapps.rescan().await;
  id
}

fn blob_file(harness: &Harness, id: Uuid, sha: &str) -> PathBuf {
  harness
    .state_dir()
    .join("webapp-blobs")
    .join(id.simple().to_string())
    .join(sha)
}

fn put_request(id: Uuid, key: &str, mime: &str, size: u32, sha: &str) -> WebappBlobPut {
  WebappBlobPut {
    id,
    key: key.into(),
    mime: mime.into(),
    transfer: TransferRef {
      id: Uuid::now_v7(),
      total_size: size,
      sha256: Some(sha.into()),
    },
  }
}

async fn send_fragments(companion: &Gateway, transfer_id: Uuid, bytes: &[u8], from: usize, until: usize) {
  let mut offset = from;
  while offset < until {
    let end = (offset + FRAGMENT_LEN).min(until);
    companion
      .transfer()
      .fragment(TransferFragment {
        transfer_id,
        offset: offset as u32,
        bytes: Bytes::copy_from_slice(&bytes[offset..end]),
      })
      .await
      .expect("fragment");
    offset = end;
  }
}

async fn next_terminal(events: &mut WebappEvents, id: Uuid, key: &str) -> Result<WebappBlobStored, WebappBlobFailed> {
  loop {
    let event = tokio::time::timeout(EVENT_WAIT, events.next())
      .await
      .expect("blob terminal event before deadline")
      .expect("webapp event stream open");
    match event {
      BridgeToGatewayWebappMsgEvent::BlobStored(stored) if stored.id == id && stored.key == key => return Ok(stored),
      BridgeToGatewayWebappMsgEvent::BlobFailed(failed) if failed.id == id && failed.key == key => return Err(failed),
      _ => {}
    }
  }
}

async fn upload(
  companion: &Gateway,
  events: &mut WebappEvents,
  id: Uuid,
  key: &str,
  mime: &str,
  bytes: &[u8],
) -> WebappBlobStored {
  let request = put_request(id, key, mime, bytes.len() as u32, &sha_hex(bytes));
  let transfer_id = request.transfer.id;
  let ack = companion.webapp().blob_put(request).await.expect("blob put");
  send_fragments(
    companion,
    transfer_id,
    bytes,
    ack.resume_from_offset as usize,
    bytes.len(),
  )
  .await;
  next_terminal(events, id, key).await.expect("upload stores")
}

fn webapp_events(companion: &Gateway) -> WebappEvents {
  Box::pin(companion.webapp().events())
}

struct Settled {
  outcome: Result<WebappBlobStored, WebappBlobFailed>,
  passed: Vec<WebappBlobFailed>,
}

async fn settle_all(events: &mut WebappEvents, id: Uuid, mut awaited: HashSet<(String, String)>) {
  while !awaited.is_empty() {
    let event = tokio::time::timeout(EVENT_WAIT, events.next())
      .await
      .unwrap_or_else(|_| panic!("no terminal event for {awaited:?}"))
      .expect("webapp event stream open");
    match event {
      BridgeToGatewayWebappMsgEvent::BlobStored(stored) if stored.id == id => {
        awaited.remove(&(stored.key, stored.sha256));
      }
      BridgeToGatewayWebappMsgEvent::BlobFailed(failed) if failed.id == id => {
        awaited.remove(&(failed.key, failed.sha256));
      }
      _ => {}
    }
  }
}

async fn settle(events: &mut WebappEvents, id: Uuid, key: &str, sha: &str) -> Settled {
  let mut passed = Vec::new();
  loop {
    let event = tokio::time::timeout(EVENT_WAIT, events.next())
      .await
      .unwrap_or_else(|_| panic!("no terminal event for {key} {sha}"))
      .expect("webapp event stream open");
    match event {
      BridgeToGatewayWebappMsgEvent::BlobStored(stored)
        if stored.id == id && stored.key == key && stored.sha256 == sha =>
      {
        return Settled {
          outcome: Ok(stored),
          passed,
        };
      }
      BridgeToGatewayWebappMsgEvent::BlobFailed(failed)
        if failed.id == id && failed.key == key && failed.sha256 == sha =>
      {
        return Settled {
          outcome: Err(failed),
          passed,
        };
      }
      BridgeToGatewayWebappMsgEvent::BlobFailed(failed) if failed.id == id => passed.push(failed),
      _ => {}
    }
  }
}

async fn acked_at(transfer_events: &mut TransferEvents, transfer_id: Uuid, received: usize) {
  loop {
    let event = tokio::time::timeout(EVENT_WAIT, transfer_events.next())
      .await
      .unwrap_or_else(|_| panic!("no ack at {received}"))
      .expect("transfer stream open");
    if let BridgeToGatewayTransferMsgEvent::Ack(ack) = event
      && ack.transfer_id == transfer_id
      && ack.received as usize == received
    {
      return;
    }
  }
}

fn transfer_events(companion: &Gateway) -> TransferEvents {
  Box::pin(companion.transfer().events())
}

fn content_names(harness: &Harness, id: Uuid) -> Vec<String> {
  let dir = harness.state_dir().join("webapp-blobs").join(id.simple().to_string());
  let Ok(entries) = std::fs::read_dir(dir) else {
    return Vec::new();
  };
  let mut names: Vec<String> = entries
    .map(|entry| {
      entry
        .expect("blob dir entry")
        .file_name()
        .to_string_lossy()
        .into_owned()
    })
    .collect();
  names.sort();
  names
}

async fn assert_records_match_content(harness: &Harness, companion: &Gateway, id: Uuid) -> Vec<WebappBlobEntry> {
  let listed = companion
    .webapp()
    .blob_list(WebappBlobList { id })
    .await
    .expect("list")
    .entries;
  let mut referenced: Vec<String> = listed.iter().map(|entry| entry.sha256.clone()).collect();
  referenced.sort();
  referenced.dedup();
  assert_eq!(
    content_names(harness, id),
    referenced,
    "the content on disk is exactly what the records reference: {listed:?}"
  );
  for entry in &listed {
    let bytes = std::fs::read(blob_file(harness, id, &entry.sha256)).expect("record content");
    assert_eq!(
      (bytes.len() as u32, sha_hex(&bytes)),
      (entry.size, entry.sha256.clone()),
      "{} holds what its record declares",
      entry.key
    );
  }
  listed
}

fn blob_url(harness: &Harness, path: &str) -> String {
  format!("http://{}{path}", harness.modern_addr())
}

#[tokio::test]
async fn a_single_fragment_blob_stores_and_lists_on_both_surfaces() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  harness.state().set_active_webapp(id).await.expect("activate");
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let bytes = body(1500, 1);
  let sha = sha_hex(&bytes);

  let stored = upload(&companion, &mut events, id, "bg/hero gif", "image/gif", &bytes).await;
  assert_eq!(stored.sha256, sha);
  assert_eq!(stored.size, bytes.len() as u32);
  assert_eq!(
    std::fs::read(blob_file(&harness, id, &sha)).expect("stored file"),
    bytes
  );

  let listed = companion
    .webapp()
    .blob_list(WebappBlobList { id })
    .await
    .expect("gateway blob list");
  assert_eq!(listed.entries.len(), 1);
  assert_eq!(listed.entries[0].key, "bg/hero gif");
  assert_eq!(listed.entries[0].sha256, sha);
  assert_eq!(listed.entries[0].mime, "image/gif");
  assert_eq!(listed.entries[0].size, bytes.len() as u32);

  let client = harness.connect_command_client().await.expect("command client");
  let client_list = client.blob().list().await.expect("client blob list");
  assert_eq!(client_list.entries.len(), 1);
  assert_eq!(client_list.entries[0].sha256, sha);
  assert_eq!(
    client_list.entries[0].url,
    format!("/.bridgething/blob/bg%2Fhero%20gif?v={sha}")
  );
}

#[tokio::test]
async fn a_streamed_blob_is_acked_per_drained_fragment() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let mut transfer_events = Box::pin(companion.transfer().events());
  let bytes = body(50 * FRAGMENT_LEN + 123, 2);

  let request = put_request(id, "clip", "video/mp4", bytes.len() as u32, &sha_hex(&bytes));
  let transfer_id = request.transfer.id;
  let ack = companion.webapp().blob_put(request).await.expect("blob put");
  assert_eq!(ack, WebappBlobPutAck { resume_from_offset: 0 });
  send_fragments(&companion, transfer_id, &bytes, 0, bytes.len()).await;

  let mut acked = Vec::new();
  while acked.last().copied() != Some(bytes.len() as u32) {
    let event = tokio::time::timeout(EVENT_WAIT, transfer_events.next())
      .await
      .expect("ack before deadline")
      .expect("transfer stream open");
    if let BridgeToGatewayTransferMsgEvent::Ack(ack) = event
      && ack.transfer_id == transfer_id
    {
      acked.push(ack.received);
    }
  }
  assert_eq!(
    acked.len(),
    bytes.len().div_ceil(FRAGMENT_LEN),
    "one ack per drained fragment"
  );
  assert!(acked.windows(2).all(|w| w[0] < w[1]), "acks advance monotonically");

  let stored = next_terminal(&mut events, id, "clip").await.expect("stored");
  assert_eq!(stored.size, bytes.len() as u32);
}

#[tokio::test]
async fn an_abandoned_upload_resumes_from_its_received_offset() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let bytes = body(20 * FRAGMENT_LEN, 3);
  let sha = sha_hex(&bytes);
  let half = 10 * FRAGMENT_LEN;

  let first = put_request(id, "resume", "application/octet-stream", bytes.len() as u32, &sha);
  let first_transfer = first.transfer.id;
  companion.webapp().blob_put(first).await.expect("first put");
  let mut transfer_events = Box::pin(companion.transfer().events());
  send_fragments(&companion, first_transfer, &bytes, 0, half).await;
  loop {
    let event = tokio::time::timeout(EVENT_WAIT, transfer_events.next())
      .await
      .expect("ack before deadline")
      .expect("transfer stream open");
    if let BridgeToGatewayTransferMsgEvent::Ack(ack) = event
      && ack.received as usize == half
    {
      break;
    }
  }
  companion
    .transfer()
    .abandon(TransferAbandon {
      transfer_id: first_transfer,
      reason: "link lost".into(),
    })
    .await
    .expect("abandon");

  let retry = put_request(id, "resume", "application/octet-stream", bytes.len() as u32, &sha);
  let retry_transfer = retry.transfer.id;
  let ack = companion.webapp().blob_put(retry).await.expect("retry put");
  assert_eq!(
    ack.resume_from_offset as usize, half,
    "a retry resumes the retained partial"
  );
  send_fragments(&companion, retry_transfer, &bytes, half, bytes.len()).await;
  next_terminal(&mut events, id, "resume").await.expect("stored");
  assert_eq!(
    std::fs::read(blob_file(&harness, id, &sha)).expect("stored file"),
    bytes
  );
}

#[tokio::test]
async fn a_digest_mismatch_fails_the_upload_and_discards_the_partial() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let bytes = body(3 * FRAGMENT_LEN, 4);
  let wrong = sha_hex(b"something else");

  let request = put_request(id, "bad", "image/png", bytes.len() as u32, &wrong);
  let transfer_id = request.transfer.id;
  companion.webapp().blob_put(request).await.expect("put");
  send_fragments(&companion, transfer_id, &bytes, 0, bytes.len()).await;
  let failed = next_terminal(&mut events, id, "bad")
    .await
    .expect_err("digest mismatch fails");
  assert_eq!(failed.sha256, wrong);
  assert!(
    matches!(failed.error, WebappError::BlobTransferFailed { .. }),
    "got {:?}",
    failed.error
  );

  let again = companion
    .webapp()
    .blob_put(put_request(id, "bad", "image/png", bytes.len() as u32, &wrong))
    .await
    .expect("put again");
  assert_eq!(again.resume_from_offset, 0, "a failed digest leaves no partial behind");
  let listed = companion.webapp().blob_list(WebappBlobList { id }).await.expect("list");
  assert!(listed.entries.is_empty());
}

#[tokio::test]
async fn puts_outside_the_caps_are_refused_up_front() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let sha = sha_hex(b"x");

  let err = companion
    .webapp()
    .blob_put(put_request(id, "huge", "video/mp4", BLOB_MAX_BYTES + 1, &sha))
    .await
    .expect_err("over the per-blob cap");
  assert!(
    matches!(err, RequestFailure::Domain(WebappError::BlobTooLarge { .. })),
    "got {err:?}"
  );

  let err = companion
    .webapp()
    .blob_put(put_request(id, "", "image/gif", 1, &sha))
    .await
    .expect_err("empty key");
  assert!(
    matches!(err, RequestFailure::Domain(WebappError::InvalidBlobKey { .. })),
    "got {err:?}"
  );

  for dots in [".", ".."] {
    let err = companion
      .webapp()
      .blob_put(put_request(id, dots, "image/gif", 1, &sha))
      .await
      .expect_err("a dot segment is not a fetchable key");
    assert!(
      matches!(err, RequestFailure::Domain(WebappError::InvalidBlobKey { .. })),
      "got {err:?}"
    );
  }

  let mut unhashed = put_request(id, "k", "image/gif", 1, &sha);
  unhashed.transfer.sha256 = None;
  let err = companion.webapp().blob_put(unhashed).await.expect_err("no digest");
  assert!(
    matches!(err, RequestFailure::Domain(WebappError::InvalidBlobPut { .. })),
    "got {err:?}"
  );

  let err = companion
    .webapp()
    .blob_put(put_request(Uuid::now_v7(), "k", "image/gif", 1, &sha))
    .await
    .expect_err("unknown webapp");
  assert!(
    matches!(err, RequestFailure::Domain(WebappError::WebappNotFound { .. })),
    "got {err:?}"
  );
}

#[tokio::test]
async fn the_per_webapp_quota_counts_shared_content_once_and_refuses_instead_of_evicting() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let edge = body(16, 5);
  let seeded = [
    ("a", sha_hex(b"1"), QUOTA_BYTES / 2),
    ("b", sha_hex(b"1"), QUOTA_BYTES / 2),
    ("c", sha_hex(b"2"), QUOTA_BYTES / 2 - edge.len() as u32),
  ];
  for (key, sha, size) in &seeded {
    let record = serde_json::json!({ "sha256": sha, "mime": "video/mp4", "size": size });
    harness
      .state()
      .kv
      .blob_set(id, key, record.to_string())
      .await
      .expect("seed blob record");
  }

  upload(&companion, &mut events, id, "d", "image/gif", &edge).await;

  let ack = companion
    .webapp()
    .blob_put(put_request(id, "e", "image/gif", edge.len() as u32, &sha_hex(&edge)))
    .await
    .expect("a key sharing stored content adds nothing to a full quota");
  assert_eq!(ack.resume_from_offset as usize, edge.len());
  next_terminal(&mut events, id, "e")
    .await
    .expect("shared content stored");

  let one_more = body(1, 6);
  let err = companion
    .webapp()
    .blob_put(put_request(id, "f", "image/gif", 1, &sha_hex(&one_more)))
    .await
    .expect_err("over quota");
  assert!(
    matches!(err, RequestFailure::Domain(WebappError::BlobQuotaExceeded { .. })),
    "got {err:?}"
  );

  upload(&companion, &mut events, id, "c", "image/gif", &one_more).await;
  let listed = companion.webapp().blob_list(WebappBlobList { id }).await.expect("list");
  let mut keys: Vec<&str> = listed.entries.iter().map(|entry| entry.key.as_str()).collect();
  keys.sort();
  assert_eq!(
    keys,
    ["a", "b", "c", "d", "e"],
    "a replacement that shrinks usage fits, and nothing was evicted"
  );
}

#[tokio::test]
async fn identical_content_is_stored_once_and_collected_when_unreferenced() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let shared = body(2 * FRAGMENT_LEN, 6);
  let shared_sha = sha_hex(&shared);
  let other = body(FRAGMENT_LEN, 7);

  upload(&companion, &mut events, id, "a", "image/gif", &shared).await;
  let ack = companion
    .webapp()
    .blob_put(put_request(id, "b", "image/gif", shared.len() as u32, &shared_sha))
    .await
    .expect("put duplicate");
  assert_eq!(
    ack.resume_from_offset as usize,
    shared.len(),
    "content already on disk needs no transfer"
  );
  next_terminal(&mut events, id, "b").await.expect("duplicate stored");

  upload(&companion, &mut events, id, "a", "image/gif", &other).await;
  assert!(
    blob_file(&harness, id, &shared_sha).exists(),
    "b still references the shared content"
  );

  companion
    .webapp()
    .blob_delete(WebappBlobDelete { id, key: "b".into() })
    .await
    .expect("delete b");
  assert!(
    !blob_file(&harness, id, &shared_sha).exists(),
    "the last reference going away collects the file"
  );
  assert!(blob_file(&harness, id, &sha_hex(&other)).exists());
}

#[tokio::test]
async fn deleting_a_blob_removes_it_everywhere_and_tells_the_active_webapp() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  harness.state().set_active_webapp(id).await.expect("activate");
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let bytes = body(100, 8);
  upload(&companion, &mut events, id, "gone", "image/gif", &bytes).await;

  let client = harness.connect_command_client().await.expect("command client");
  let mut blob_events = Box::pin(client.blob().events());
  let ack = companion
    .webapp()
    .blob_delete(WebappBlobDelete { id, key: "gone".into() })
    .await
    .expect("delete");
  assert_eq!(ack.key, "gone");

  let BridgeToClientBlobMsgEvent::Changed(changed) = tokio::time::timeout(EVENT_WAIT, blob_events.next())
    .await
    .expect("blob change event")
    .expect("stream open");
  assert_eq!(changed.key, "gone");
  assert_eq!(changed.sha256, None);
  assert_eq!(changed.url, None);

  assert!(client.blob().list().await.expect("client list").entries.is_empty());
  let status = reqwest::get(blob_url(&harness, "/.bridgething/blob/gone"))
    .await
    .expect("http get")
    .status();
  assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
  assert!(!blob_file(&harness, id, &sha_hex(&bytes)).exists());
}

#[tokio::test]
async fn blob_changes_reach_the_active_webapp_and_never_the_overlay() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, true).await;
  let companion = harness.connect_android().await.expect("connect companion");
  companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Overlay,
      id: Some(id),
    })
    .await
    .expect("overlay slot");
  harness.state().set_active_webapp(id).await.expect("activate");

  let foreground = harness.connect_command_client().await.expect("foreground client");
  let overlay = harness.connect_overlay_command_client().await.expect("overlay client");
  let mut foreground_events = Box::pin(foreground.blob().events());
  let mut overlay_events = Box::pin(overlay.blob().events());

  let mut events = webapp_events(&companion);
  let bytes = body(300, 9);
  let sha = sha_hex(&bytes);
  upload(&companion, &mut events, id, "wallpaper", "image/png", &bytes).await;

  let BridgeToClientBlobMsgEvent::Changed(changed) = tokio::time::timeout(EVENT_WAIT, foreground_events.next())
    .await
    .expect("foreground hears the change")
    .expect("stream open");
  assert_eq!(changed.key, "wallpaper");
  assert_eq!(changed.sha256.as_deref(), Some(sha.as_str()));
  assert_eq!(
    changed.url.as_deref(),
    Some(format!("/.bridgething/blob/wallpaper?v={sha}").as_str())
  );

  let leaked = tokio::time::timeout(SETTLE, overlay_events.next()).await;
  assert!(
    leaked.is_err(),
    "the overlay scope must not hear blob changes: {leaked:?}"
  );
}

#[tokio::test]
async fn the_active_webapp_serves_its_blobs_with_validators_ranges_and_mime() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let other = plant(&harness, false).await;
  harness.state().set_active_webapp(id).await.expect("activate");
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let bytes = body(10_000, 10);
  let sha = sha_hex(&bytes);
  upload(&companion, &mut events, id, "anim", "image/gif", &bytes).await;
  upload(&companion, &mut events, other, "theirs", "image/gif", &bytes).await;

  let http = reqwest::Client::new();
  let plain = http
    .get(blob_url(&harness, "/.bridgething/blob/anim"))
    .send()
    .await
    .expect("get");
  assert_eq!(plain.status(), reqwest::StatusCode::OK);
  let etag = format!("\"{sha}\"");
  assert_eq!(plain.headers()["content-type"], "image/gif");
  assert_eq!(plain.headers()["etag"], etag.as_str());
  assert_eq!(plain.headers()["cache-control"], "no-cache");
  assert_eq!(plain.bytes().await.expect("body").as_ref(), bytes.as_slice());

  let pinned = http
    .get(blob_url(&harness, &format!("/.bridgething/blob/anim?v={sha}")))
    .send()
    .await
    .expect("pinned get");
  assert_eq!(pinned.headers()["cache-control"], "public, max-age=31536000, immutable");

  let revalidated = http
    .get(blob_url(&harness, "/.bridgething/blob/anim"))
    .header("if-none-match", etag.as_str())
    .send()
    .await
    .expect("conditional get");
  assert_eq!(revalidated.status(), reqwest::StatusCode::NOT_MODIFIED);
  assert_eq!(revalidated.headers()["etag"], etag.as_str());

  let ranged = http
    .get(blob_url(&harness, "/.bridgething/blob/anim"))
    .header("range", "bytes=100-199")
    .send()
    .await
    .expect("range get");
  assert_eq!(ranged.status(), reqwest::StatusCode::PARTIAL_CONTENT);
  assert_eq!(ranged.headers()["content-type"], "image/gif");
  assert_eq!(ranged.bytes().await.expect("range body").as_ref(), &bytes[100..200]);

  let stale_range = http
    .get(blob_url(&harness, "/.bridgething/blob/anim"))
    .header("range", "bytes=100-199")
    .header("if-range", "\"stale\"")
    .send()
    .await
    .expect("if-range get");
  assert_eq!(
    stale_range.status(),
    reqwest::StatusCode::OK,
    "a stale If-Range gets the whole body"
  );

  let foreign = http
    .get(blob_url(&harness, "/.bridgething/blob/theirs"))
    .send()
    .await
    .expect("foreign get");
  assert_eq!(
    foreign.status(),
    reqwest::StatusCode::NOT_FOUND,
    "only the active webapp's blobs are served"
  );
}

#[tokio::test]
async fn uninstalling_a_webapp_deletes_its_blobs_and_cancels_its_uploads() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let stored = body(500, 11);
  upload(&companion, &mut events, id, "kept", "image/gif", &stored).await;

  let pending = body(8 * FRAGMENT_LEN, 12);
  let request = put_request(id, "pending", "video/mp4", pending.len() as u32, &sha_hex(&pending));
  let transfer_id = request.transfer.id;
  companion.webapp().blob_put(request).await.expect("put");
  send_fragments(&companion, transfer_id, &pending, 0, 2 * FRAGMENT_LEN).await;

  companion
    .webapp()
    .uninstall(WebappUninstall { id })
    .await
    .expect("uninstall");
  let failed = next_terminal(&mut events, id, "pending")
    .await
    .expect_err("the in-flight upload is cancelled");
  assert!(
    matches!(failed.error, WebappError::BlobTransferFailed { .. }),
    "got {:?}",
    failed.error
  );

  assert!(
    !harness
      .state_dir()
      .join("webapp-blobs")
      .join(id.simple().to_string())
      .exists(),
    "the blob dir goes with the webapp"
  );
  assert!(
    harness.state().kv.blob_list(id).await.expect("kv list").is_empty(),
    "blob records go with the webapp"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn blobs_and_partial_uploads_survive_a_daemon_restart() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  harness.state().set_active_webapp(id).await.expect("activate");
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let stored = body(5000, 13);
  let stored_sha = sha_hex(&stored);
  upload(&companion, &mut events, id, "kept", "image/png", &stored).await;

  let partial = body(12 * FRAGMENT_LEN, 14);
  let partial_sha = sha_hex(&partial);
  let half = 6 * FRAGMENT_LEN;
  let request = put_request(id, "half", "video/mp4", partial.len() as u32, &partial_sha);
  let transfer_id = request.transfer.id;
  companion.webapp().blob_put(request).await.expect("put");
  let mut transfer_events = Box::pin(companion.transfer().events());
  send_fragments(&companion, transfer_id, &partial, 0, half).await;
  loop {
    let event = tokio::time::timeout(EVENT_WAIT, transfer_events.next())
      .await
      .expect("ack before deadline")
      .expect("transfer stream open");
    if let BridgeToGatewayTransferMsgEvent::Ack(ack) = event
      && ack.received as usize == half
    {
      break;
    }
  }
  drop(transfer_events);
  drop(events);
  drop(companion);

  let harness = harness.restart().await.expect("restart");
  let companion = harness.connect_android().await.expect("reconnect companion");
  let listed = companion.webapp().blob_list(WebappBlobList { id }).await.expect("list");
  assert_eq!(listed.entries.len(), 1);
  assert_eq!(listed.entries[0].sha256, stored_sha);

  let served = reqwest::get(blob_url(&harness, "/.bridgething/blob/kept"))
    .await
    .expect("get");
  assert_eq!(served.status(), reqwest::StatusCode::OK);
  assert_eq!(served.bytes().await.expect("body").as_ref(), stored.as_slice());

  let mut events = webapp_events(&companion);
  let retry = put_request(id, "half", "video/mp4", partial.len() as u32, &partial_sha);
  let retry_transfer = retry.transfer.id;
  let ack = companion.webapp().blob_put(retry).await.expect("retry put");
  assert_eq!(
    ack.resume_from_offset as usize, half,
    "the partial survives the restart"
  );
  send_fragments(&companion, retry_transfer, &partial, half, partial.len()).await;
  next_terminal(&mut events, id, "half")
    .await
    .expect("stored after restart");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_session_streams_a_page_built_handle_and_reads_it_back() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let io = harness.connect_android_io().await.expect("a link to the daemon");
  let session = bridgething_host_gateway::session::from_io(io, Default::default())
    .await
    .expect("a delivery session");
  let bytes = body(3 * FRAGMENT_LEN * 64 + 77, 6);
  let handle = session.handles.create("image/gif".into()).expect("a handle");
  for chunk in bytes.chunks(256 * 1024) {
    session.handles.append(&handle, chunk).expect("the chunk lands");
  }

  let stored = tokio::time::timeout(EVENT_WAIT, session.put_blob(id, "loop".into(), &handle, None))
    .await
    .expect("the put settles")
    .expect("the daemon stores it");

  assert_eq!(stored.sha256, sha_hex(&bytes));
  assert_eq!((stored.mime.as_str(), stored.size), ("image/gif", bytes.len() as u32));
  assert_eq!(
    std::fs::read(blob_file(&harness, id, &stored.sha256)).expect("stored file"),
    bytes
  );
  assert_eq!(session.list_blobs(id).await.expect("the listing"), vec![stored]);

  session.delete_blob(id, "loop".into()).await.expect("the delete");
  assert!(session.list_blobs(id).await.expect("the listing").is_empty());
}

#[tokio::test]
async fn deleting_a_key_cancels_the_upload_in_flight_for_it() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let mut acks = transfer_events(&companion);
  let bytes = body(8 * FRAGMENT_LEN, 15);
  let sha = sha_hex(&bytes);

  let request = put_request(id, "doomed", "video/mp4", bytes.len() as u32, &sha);
  let transfer_id = request.transfer.id;
  companion.webapp().blob_put(request).await.expect("put");
  send_fragments(&companion, transfer_id, &bytes, 0, 2 * FRAGMENT_LEN).await;
  acked_at(&mut acks, transfer_id, 2 * FRAGMENT_LEN).await;

  companion
    .webapp()
    .blob_delete(WebappBlobDelete {
      id,
      key: "doomed".into(),
    })
    .await
    .expect("delete");
  let failed = settle(&mut events, id, "doomed", &sha)
    .await
    .outcome
    .expect_err("the delete cancels the upload");
  assert!(
    matches!(failed.error, WebappError::BlobTransferFailed { .. }),
    "got {:?}",
    failed.error
  );

  let again = companion
    .webapp()
    .blob_put(put_request(id, "doomed", "video/mp4", bytes.len() as u32, &sha))
    .await
    .expect("put again");
  assert_eq!(
    again.resume_from_offset, 0,
    "a cancelled upload leaves no partial behind"
  );
  companion
    .webapp()
    .blob_delete(WebappBlobDelete {
      id,
      key: "doomed".into(),
    })
    .await
    .expect("delete again");
  assert!(assert_records_match_content(&harness, &companion, id).await.is_empty());
}

#[tokio::test]
async fn content_that_no_longer_matches_its_digest_is_uploaded_again_instead_of_deduplicated() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  harness.state().set_active_webapp(id).await.expect("activate");
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let client = harness.connect_command_client().await.expect("command client");
  let mut blob_events = Box::pin(client.blob().events());

  let truncated = body(3 * FRAGMENT_LEN, 16);
  let truncated_sha = sha_hex(&truncated);
  upload(&companion, &mut events, id, "torn", "image/gif", &truncated).await;
  std::fs::write(
    blob_file(&harness, id, &truncated_sha),
    &truncated[..truncated.len() / 2],
  )
  .expect("truncate the stored content");

  let rotted = body(3 * FRAGMENT_LEN, 17);
  let rotted_sha = sha_hex(&rotted);
  upload(&companion, &mut events, id, "rotted", "image/gif", &rotted).await;
  let mut flipped = rotted.clone();
  flipped[FRAGMENT_LEN] ^= 0xFF;
  std::fs::write(blob_file(&harness, id, &rotted_sha), &flipped).expect("corrupt the stored content");
  for _ in 0..2 {
    let BridgeToClientBlobMsgEvent::Changed(stored) = tokio::time::timeout(EVENT_WAIT, blob_events.next())
      .await
      .expect("the uploads reach the webapp")
      .expect("stream open");
    assert!(stored.sha256.is_some());
  }

  let request = put_request(id, "torn", "image/gif", truncated.len() as u32, &truncated_sha);
  let transfer_id = request.transfer.id;
  let ack = companion.webapp().blob_put(request).await.expect("put the same bytes");
  assert_eq!(ack.resume_from_offset, 0, "truncated content is not taken as stored");
  let BridgeToClientBlobMsgEvent::Changed(dropped) = tokio::time::timeout(EVENT_WAIT, blob_events.next())
    .await
    .expect("the webapp hears its blob is gone")
    .expect("stream open");
  assert_eq!(
    (dropped.key.as_str(), dropped.sha256, dropped.url),
    ("torn", None, None)
  );
  send_fragments(&companion, transfer_id, &truncated, 0, truncated.len()).await;
  settle(&mut events, id, "torn", &truncated_sha)
    .await
    .outcome
    .expect("stored again");

  let request = put_request(id, "copy", "image/gif", rotted.len() as u32, &rotted_sha);
  let transfer_id = request.transfer.id;
  let ack = companion.webapp().blob_put(request).await.expect("put the same bytes");
  assert_eq!(
    ack.resume_from_offset, 0,
    "content of the right size and the wrong bytes is not taken as stored"
  );
  let BridgeToClientBlobMsgEvent::Changed(restored) = tokio::time::timeout(EVENT_WAIT, blob_events.next())
    .await
    .expect("the webapp hears the reupload")
    .expect("stream open");
  assert_eq!(
    (restored.key.as_str(), restored.sha256.as_deref()),
    ("torn", Some(truncated_sha.as_str()))
  );
  let BridgeToClientBlobMsgEvent::Changed(dropped) = tokio::time::timeout(EVENT_WAIT, blob_events.next())
    .await
    .expect("the webapp hears its blob is gone")
    .expect("stream open");
  assert_eq!(
    (dropped.key.as_str(), dropped.sha256, dropped.url),
    ("rotted", None, None)
  );
  send_fragments(&companion, transfer_id, &rotted, 0, rotted.len()).await;
  settle(&mut events, id, "copy", &rotted_sha)
    .await
    .outcome
    .expect("stored again");

  let listed = assert_records_match_content(&harness, &companion, id).await;
  let keys: Vec<&str> = listed.iter().map(|entry| entry.key.as_str()).collect();
  assert_eq!(keys, ["copy", "torn"], "the record of the corrupt content went with it");
}

#[tokio::test]
async fn a_put_declaring_the_wrong_size_for_stored_content_is_refused_and_harms_nothing() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let bytes = body(2 * FRAGMENT_LEN, 18);
  let sha = sha_hex(&bytes);
  upload(&companion, &mut events, id, "kept", "image/gif", &bytes).await;

  let err = companion
    .webapp()
    .blob_put(put_request(id, "liar", "image/gif", bytes.len() as u32 - 1, &sha))
    .await
    .expect_err("the stored content has another size");
  assert!(
    matches!(err, RequestFailure::Domain(WebappError::InvalidBlobPut { .. })),
    "got {err:?}"
  );
  let listed = assert_records_match_content(&harness, &companion, id).await;
  assert_eq!(listed.len(), 1);
  assert_eq!(listed[0].key, "kept");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_collects_orphaned_content_and_drops_records_whose_content_is_gone() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  harness.state().set_active_webapp(id).await.expect("activate");
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let kept = body(5000, 19);
  let lost = body(6000, 20);
  let torn = body(7000, 21);
  upload(&companion, &mut events, id, "kept", "image/png", &kept).await;
  upload(&companion, &mut events, id, "lost", "image/png", &lost).await;
  upload(&companion, &mut events, id, "torn", "image/png", &torn).await;
  harness
    .state()
    .kv
    .blob_set(id, "unreadable", "not a record".into())
    .await
    .expect("seed an unreadable record");
  drop(events);
  drop(companion);

  let blobs_root = harness.state_dir().join("webapp-blobs");
  let orphan = sha_hex(b"orphan");
  let staged = format!(".{orphan}.{}.staged", Uuid::now_v7().simple());
  let departed = blobs_root.join(Uuid::now_v7().simple().to_string());
  std::fs::write(blob_file(&harness, id, &orphan), b"content no record points at").expect("orphan");
  std::fs::write(blobs_root.join(id.simple().to_string()).join(&staged), b"half a copy").expect("staged");
  std::fs::create_dir_all(&departed).expect("departed webapp dir");
  std::fs::write(departed.join(&orphan), b"content of a webapp that is gone").expect("departed content");
  std::fs::write(blobs_root.join("notes.txt"), b"not the store's").expect("foreign file");
  std::fs::remove_file(blob_file(&harness, id, &sha_hex(&lost))).expect("lose content");
  std::fs::write(blob_file(&harness, id, &sha_hex(&torn)), &torn[..torn.len() / 2]).expect("tear content");

  let harness = harness.restart().await.expect("restart");
  let companion = harness.connect_android().await.expect("reconnect companion");

  let listed = assert_records_match_content(&harness, &companion, id).await;
  assert_eq!(listed.len(), 1, "got {listed:?}");
  assert_eq!(
    (listed[0].key.as_str(), listed[0].sha256.clone()),
    ("kept", sha_hex(&kept))
  );
  assert!(
    harness
      .state()
      .kv
      .blob_get(id, "unreadable")
      .await
      .expect("kv get")
      .is_none(),
    "a record nothing can read is dropped"
  );
  assert!(!departed.exists(), "content of a webapp with no records is collected");
  assert!(
    blobs_root.join("notes.txt").exists(),
    "names the store never wrote are left alone"
  );

  let client = harness.connect_command_client().await.expect("command client");
  let client_list = client.blob().list().await.expect("client blob list");
  assert_eq!(client_list.entries.len(), 1);
  assert_eq!(client_list.entries[0].key, "kept");
  for gone in ["lost", "torn"] {
    let status = reqwest::get(blob_url(&harness, &format!("/.bridgething/blob/{gone}")))
      .await
      .expect("http get")
      .status();
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "{gone} is no longer served");
  }
  let served = reqwest::get(blob_url(&harness, "/.bridgething/blob/kept"))
    .await
    .expect("get");
  assert_eq!(served.bytes().await.expect("body").as_ref(), kept.as_slice());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_put_racing_the_delete_of_its_content_never_records_content_that_is_gone() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let first = harness.connect_android().await.expect("connect first companion");
  let second = harness.connect_android().await.expect("connect second companion");
  let mut events = webapp_events(&first);

  for round in 0..32u8 {
    let bytes = body(2 * FRAGMENT_LEN, round);
    let sha = sha_hex(&bytes);
    upload(&first, &mut events, id, "source", "image/gif", &bytes).await;

    let request = put_request(id, "copy", "image/gif", bytes.len() as u32, &sha);
    let transfer_id = request.transfer.id;
    let put = async {
      tokio::time::sleep(Duration::from_micros(40 * u64::from(round % 8))).await;
      first.webapp().blob_put(request).await
    };
    let delete = async {
      tokio::time::sleep(Duration::from_micros(40 * u64::from(7 - round % 8))).await;
      second
        .webapp()
        .blob_delete(WebappBlobDelete {
          id,
          key: "source".into(),
        })
        .await
    };
    let (ack, deleted) = tokio::join!(put, delete);
    deleted.expect("delete the source");
    let ack = ack.expect("put the copy");
    send_fragments(
      &first,
      transfer_id,
      &bytes,
      ack.resume_from_offset as usize,
      bytes.len(),
    )
    .await;
    settle(&mut events, id, "copy", &sha)
      .await
      .outcome
      .expect("the copy stores");

    let listed = assert_records_match_content(&harness, &first, id).await;
    assert_eq!(listed.len(), 1, "round {round}: {listed:?}");
    assert_eq!(listed[0].key, "copy");
    first
      .webapp()
      .blob_delete(WebappBlobDelete { id, key: "copy".into() })
      .await
      .expect("delete the copy");
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_completing_as_its_webapp_is_uninstalled_leaves_nothing_behind() {
  let harness = Harness::start().await.expect("harness start");
  let uploader = harness.connect_android().await.expect("connect uploader");
  let remover = harness.connect_android().await.expect("connect remover");
  let mut events = webapp_events(&uploader);
  let mut acks = transfer_events(&uploader);

  for round in 0..32u8 {
    let id = plant(&harness, false).await;
    let bytes = body(3 * FRAGMENT_LEN, round);
    let sha = sha_hex(&bytes);
    let request = put_request(id, "last", "image/gif", bytes.len() as u32, &sha);
    let transfer_id = request.transfer.id;
    uploader.webapp().blob_put(request).await.expect("put");
    send_fragments(&uploader, transfer_id, &bytes, 0, 2 * FRAGMENT_LEN).await;
    acked_at(&mut acks, transfer_id, 2 * FRAGMENT_LEN).await;

    let finish = send_fragments(&uploader, transfer_id, &bytes, 2 * FRAGMENT_LEN, bytes.len());
    let uninstall = async {
      tokio::time::sleep(Duration::from_micros(250 * u64::from(round))).await;
      remover.webapp().uninstall(WebappUninstall { id }).await
    };
    let ((), uninstalled) = tokio::join!(finish, uninstall);
    uninstalled.expect("uninstall");
    let _ = settle(&mut events, id, "last", &sha).await;

    assert!(
      harness.state().kv.blob_list(id).await.expect("kv list").is_empty(),
      "round {round}: a record outlived its webapp"
    );
    assert!(
      !harness
        .state_dir()
        .join("webapp-blobs")
        .join(id.simple().to_string())
        .exists(),
      "round {round}: content outlived its webapp"
    );
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_companions_putting_the_same_bytes_under_one_key_at_once_store_them_once() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let first = harness.connect_android().await.expect("connect first companion");
  let second = harness.connect_android().await.expect("connect second companion");

  for round in 0..16u8 {
    let mut first_events = webapp_events(&first);
    let mut second_events = webapp_events(&second);
    let bytes = body(4 * FRAGMENT_LEN, round);
    let sha = sha_hex(&bytes);
    let first_request = put_request(id, "bg", "image/gif", bytes.len() as u32, &sha);
    let second_request = put_request(id, "bg", "image/gif", bytes.len() as u32, &sha);
    let (first_transfer, second_transfer) = (first_request.transfer.id, second_request.transfer.id);

    let (first_ack, second_ack) = tokio::join!(async { first.webapp().blob_put(first_request).await }, async {
      second.webapp().blob_put(second_request).await
    });
    let first_ack = first_ack.expect("first put");
    let second_ack = second_ack.expect("second put");
    tokio::join!(
      send_fragments(
        &first,
        first_transfer,
        &bytes,
        first_ack.resume_from_offset as usize,
        bytes.len()
      ),
      send_fragments(
        &second,
        second_transfer,
        &bytes,
        second_ack.resume_from_offset as usize,
        bytes.len()
      ),
    );

    let outcomes = [
      settle(&mut first_events, id, "bg", &sha).await.outcome,
      settle(&mut second_events, id, "bg", &sha).await.outcome,
    ];
    assert_eq!(
      outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
      1,
      "round {round}: one upload wins and the one it replaced is told: {outcomes:?}"
    );
    let listed = assert_records_match_content(&harness, &first, id).await;
    assert_eq!(listed.len(), 1, "round {round}: {listed:?}");
    assert_eq!(listed[0].sha256, sha);
    first
      .webapp()
      .blob_delete(WebappBlobDelete { id, key: "bg".into() })
      .await
      .expect("delete");
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delete_cancels_whichever_of_two_overlapping_puts_survived() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let first = harness.connect_android().await.expect("connect first companion");
  let second = harness.connect_android().await.expect("connect second companion");

  for round in 0..16u8 {
    let mut first_events = webapp_events(&first);
    let mut second_events = webapp_events(&second);
    let (ours, theirs) = (body(4 * FRAGMENT_LEN, round), body(4 * FRAGMENT_LEN, round + 100));
    let (our_sha, their_sha) = (sha_hex(&ours), sha_hex(&theirs));
    let first_request = put_request(id, "bg", "image/gif", ours.len() as u32, &our_sha);
    let second_request = put_request(id, "bg", "image/gif", theirs.len() as u32, &their_sha);
    let (first_transfer, second_transfer) = (first_request.transfer.id, second_request.transfer.id);

    let (first_ack, second_ack) = tokio::join!(async { first.webapp().blob_put(first_request).await }, async {
      second.webapp().blob_put(second_request).await
    });
    first_ack.expect("first put");
    second_ack.expect("second put");
    first
      .webapp()
      .blob_delete(WebappBlobDelete { id, key: "bg".into() })
      .await
      .expect("delete");

    settle(&mut first_events, id, "bg", &our_sha)
      .await
      .outcome
      .expect_err("the first upload is cancelled");
    settle(&mut second_events, id, "bg", &their_sha)
      .await
      .outcome
      .expect_err("the second upload is cancelled");

    tokio::join!(
      send_fragments(&first, first_transfer, &ours, 0, ours.len()),
      send_fragments(&second, second_transfer, &theirs, 0, theirs.len()),
    );
    let mut probe_events = webapp_events(&first);
    let probe = body(16, round);
    upload(&first, &mut probe_events, id, "probe", "image/gif", &probe).await;
    let listed = assert_records_match_content(&harness, &first, id).await;
    assert_eq!(
      listed.len(),
      1,
      "round {round}: a cancelled upload stored anyway: {listed:?}"
    );
    first
      .webapp()
      .blob_delete(WebappBlobDelete {
        id,
        key: "probe".into(),
      })
      .await
      .expect("delete the probe");
  }
}

#[tokio::test]
async fn a_put_the_device_cannot_begin_leaves_the_upload_in_flight_running() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let first = harness.connect_android().await.expect("connect first companion");
  let second = harness.connect_android().await.expect("connect second companion");
  let mut acks = transfer_events(&first);
  let running = body(6 * FRAGMENT_LEN, 22);
  let request = put_request(id, "bg", "video/mp4", running.len() as u32, &sha_hex(&running));
  let transfer_id = request.transfer.id;
  first.webapp().blob_put(request).await.expect("put");
  send_fragments(&first, transfer_id, &running, 0, 2 * FRAGMENT_LEN).await;
  acked_at(&mut acks, transfer_id, 2 * FRAGMENT_LEN).await;

  std::fs::remove_dir_all(harness.state_dir().join("blob-transfers")).expect("break the transfer scratch dir");
  let newer = body(FRAGMENT_LEN, 23);
  let err = second
    .webapp()
    .blob_put(put_request(id, "bg", "image/gif", newer.len() as u32, &sha_hex(&newer)))
    .await
    .expect_err("the device cannot open a transfer");
  assert!(
    matches!(err, RequestFailure::Domain(WebappError::BlobTransferFailed { .. })),
    "got {err:?}"
  );

  send_fragments(&first, transfer_id, &running, 2 * FRAGMENT_LEN, 3 * FRAGMENT_LEN).await;
  acked_at(&mut acks, transfer_id, 3 * FRAGMENT_LEN).await;
}

#[tokio::test]
async fn a_put_from_another_companion_fails_the_upload_it_replaces() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let first = harness.connect_android().await.expect("connect first companion");
  let second = harness.connect_android().await.expect("connect second companion");
  let mut first_events = webapp_events(&first);
  let mut second_events = webapp_events(&second);
  let mut acks = transfer_events(&first);
  let older = body(6 * FRAGMENT_LEN, 24);
  let older_sha = sha_hex(&older);
  let request = put_request(id, "bg", "video/mp4", older.len() as u32, &older_sha);
  let transfer_id = request.transfer.id;
  first.webapp().blob_put(request).await.expect("put");
  send_fragments(&first, transfer_id, &older, 0, 2 * FRAGMENT_LEN).await;
  acked_at(&mut acks, transfer_id, 2 * FRAGMENT_LEN).await;

  let newer = body(2 * FRAGMENT_LEN, 25);
  let newer_sha = sha_hex(&newer);
  let request = put_request(id, "bg", "image/gif", newer.len() as u32, &newer_sha);
  let newer_transfer = request.transfer.id;
  second.webapp().blob_put(request).await.expect("newer put");
  let replaced = settle(&mut first_events, id, "bg", &older_sha)
    .await
    .outcome
    .expect_err("the replaced upload is told");
  assert!(
    matches!(replaced.error, WebappError::BlobTransferFailed { .. }),
    "got {:?}",
    replaced.error
  );

  send_fragments(&second, newer_transfer, &newer, 0, newer.len()).await;
  let settled = settle(&mut second_events, id, "bg", &newer_sha).await;
  settled.outcome.expect("the newer put stores");
  assert!(
    settled.passed.is_empty(),
    "the replacing companion hears nothing about the upload it replaced: {:?}",
    settled.passed
  );
  let listed = assert_records_match_content(&harness, &second, id).await;
  assert_eq!(listed.len(), 1);
  assert_eq!(listed[0].sha256, newer_sha);
}

#[tokio::test]
async fn a_companion_replacing_its_own_upload_hears_nothing_about_the_older_one() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let mut acks = transfer_events(&companion);
  let older = body(6 * FRAGMENT_LEN, 26);
  let request = put_request(id, "bg", "video/mp4", older.len() as u32, &sha_hex(&older));
  let older_transfer = request.transfer.id;
  companion.webapp().blob_put(request).await.expect("put");
  send_fragments(&companion, older_transfer, &older, 0, 2 * FRAGMENT_LEN).await;
  acked_at(&mut acks, older_transfer, 2 * FRAGMENT_LEN).await;

  let newer = body(2 * FRAGMENT_LEN, 27);
  let newer_sha = sha_hex(&newer);
  let request = put_request(id, "bg", "image/gif", newer.len() as u32, &newer_sha);
  let newer_transfer = request.transfer.id;
  let ack = companion.webapp().blob_put(request).await.expect("newer put");
  send_fragments(
    &companion,
    newer_transfer,
    &newer,
    ack.resume_from_offset as usize,
    newer.len(),
  )
  .await;
  let settled = settle(&mut events, id, "bg", &newer_sha).await;
  settled.outcome.expect("the newer put stores");
  assert!(
    settled.passed.is_empty(),
    "a companion resolves its own replaced upload itself: {:?}",
    settled.passed
  );

  let listed = assert_records_match_content(&harness, &companion, id).await;
  assert_eq!(listed.len(), 1);
  assert_eq!(listed[0].sha256, newer_sha);
}

#[tokio::test]
async fn an_upload_refused_for_quota_at_completion_is_kept_for_the_retry() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let mut acks = transfer_events(&companion);
  let bytes = body(3 * FRAGMENT_LEN, 28);
  let sha = sha_hex(&bytes);
  let seed = |key: &'static str, size: u32| {
    let record = serde_json::json!({ "sha256": sha_hex(key.as_bytes()), "mime": "video/mp4", "size": size });
    let kv = harness.state().kv.clone();
    async move {
      kv.blob_set(id, key, record.to_string())
        .await
        .expect("seed blob record")
    }
  };
  seed("big", QUOTA_BYTES - bytes.len() as u32).await;

  let request = put_request(id, "clip", "video/mp4", bytes.len() as u32, &sha);
  let transfer_id = request.transfer.id;
  companion.webapp().blob_put(request).await.expect("put");
  send_fragments(&companion, transfer_id, &bytes, 0, 2 * FRAGMENT_LEN).await;
  acked_at(&mut acks, transfer_id, 2 * FRAGMENT_LEN).await;
  seed("late", 1).await;
  send_fragments(&companion, transfer_id, &bytes, 2 * FRAGMENT_LEN, bytes.len()).await;
  let refused = settle(&mut events, id, "clip", &sha)
    .await
    .outcome
    .expect_err("the quota filled up mid-upload");
  assert!(
    matches!(refused.error, WebappError::BlobQuotaExceeded { .. }),
    "got {:?}",
    refused.error
  );

  companion
    .webapp()
    .blob_delete(WebappBlobDelete { id, key: "late".into() })
    .await
    .expect("free the space");
  let retry = companion
    .webapp()
    .blob_put(put_request(id, "clip", "video/mp4", bytes.len() as u32, &sha))
    .await
    .expect("retry");
  assert_eq!(
    retry.resume_from_offset as usize,
    bytes.len(),
    "the verified bytes were kept, so the retry sends nothing"
  );
  settle(&mut events, id, "clip", &sha)
    .await
    .outcome
    .expect("stored on the retry");
  assert_eq!(
    std::fs::read(blob_file(&harness, id, &sha)).expect("stored file"),
    bytes
  );
}

const KEYS: [&str; 2] = ["a", "b"];

#[derive(Debug, Clone)]
enum Action {
  Fresh(usize),
  Copy(usize),
  Delete,
}

#[derive(Debug, Clone)]
struct Step {
  companion: usize,
  key: usize,
  action: Action,
}

fn wave() -> impl Strategy<Value = Vec<Step>> {
  let action = prop_oneof![
    (1..=3 * FRAGMENT_LEN).prop_map(Action::Fresh),
    (0..KEYS.len()).prop_map(Action::Copy),
    Just(Action::Delete),
  ];
  let step = (0..2usize, 0..KEYS.len(), action).prop_map(|(companion, key, action)| Step { companion, key, action });
  prop::collection::vec(step, 2..=4).prop_map(|steps| {
    let mut taken = HashSet::new();
    steps
      .into_iter()
      .filter(|step| taken.insert((step.companion, step.key)))
      .collect()
  })
}

enum Planned {
  Put { bytes: Vec<u8>, sha: String },
  Delete,
}

async fn run_waves(waves: Vec<Vec<Step>>) {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  let companions = [
    harness.connect_android().await.expect("connect first companion"),
    harness.connect_android().await.expect("connect second companion"),
  ];
  let mut known: HashMap<String, Vec<u8>> = HashMap::new();
  let mut put_under: HashMap<&str, HashSet<String>> = HashMap::new();
  let mut minted = 0u64;

  for (index, steps) in waves.iter().enumerate() {
    let stored: HashMap<String, String> = companions[0]
      .webapp()
      .blob_list(WebappBlobList { id })
      .await
      .expect("list")
      .entries
      .into_iter()
      .map(|entry| (entry.key, entry.sha256))
      .collect();
    let planned: Vec<Planned> = steps
      .iter()
      .map(|step| {
        let bytes = match &step.action {
          Action::Delete => return Planned::Delete,
          Action::Copy(of) => stored.get(KEYS[*of]).map(|sha| known[sha].clone()),
          Action::Fresh(_) => None,
        };
        let bytes = bytes.unwrap_or_else(|| {
          minted += 1;
          let len = match step.action {
            Action::Fresh(len) => len,
            _ => FRAGMENT_LEN,
          };
          let mut fresh = minted.to_le_bytes().to_vec();
          fresh.extend(body(len, minted as u8));
          fresh
        });
        let sha = sha_hex(&bytes);
        known.insert(sha.clone(), bytes.clone());
        put_under.entry(KEYS[step.key]).or_default().insert(sha.clone());
        Planned::Put { bytes, sha }
      })
      .collect();

    let mut events = [webapp_events(&companions[0]), webapp_events(&companions[1])];
    join_all(steps.iter().zip(&planned).map(|(step, planned)| {
      let companion = &companions[step.companion];
      let key = KEYS[step.key];
      async move {
        match planned {
          Planned::Delete => {
            companion
              .webapp()
              .blob_delete(WebappBlobDelete { id, key: key.into() })
              .await
              .expect("delete");
          }
          Planned::Put { bytes, sha } => {
            let request = put_request(id, key, "image/gif", bytes.len() as u32, sha);
            let transfer_id = request.transfer.id;
            let ack = companion.webapp().blob_put(request).await.expect("put");
            send_fragments(
              companion,
              transfer_id,
              bytes,
              ack.resume_from_offset as usize,
              bytes.len(),
            )
            .await;
          }
        }
      }
    }))
    .await;

    for (companion, events) in events.iter_mut().enumerate() {
      let awaited: HashSet<(String, String)> = steps
        .iter()
        .zip(&planned)
        .filter(|(step, _)| step.companion == companion)
        .filter_map(|(step, planned)| match planned {
          Planned::Put { sha, .. } => Some((KEYS[step.key].to_string(), sha.clone())),
          Planned::Delete => None,
        })
        .collect();
      settle_all(events, id, awaited).await;
    }
    for companion in &companions {
      companion
        .webapp()
        .blob_list(WebappBlobList { id })
        .await
        .expect("flush");
    }

    let listed = assert_records_match_content(&harness, &companions[0], id).await;
    for entry in &listed {
      assert!(
        put_under[entry.key.as_str()].contains(&entry.sha256),
        "wave {index}: {} holds content nobody put there: {listed:?}",
        entry.key
      );
    }
  }

  for key in KEYS {
    companions[0]
      .webapp()
      .blob_delete(WebappBlobDelete { id, key: key.into() })
      .await
      .expect("delete");
  }
  assert!(
    assert_records_match_content(&harness, &companions[0], id)
      .await
      .is_empty()
  );
}

proptest! {
  #![proptest_config(ProptestConfig { cases: proptest_cases(48), ..ProptestConfig::default() })]

  #[test]
  fn overlapping_puts_and_deletes_keep_records_and_content_in_step(waves in prop::collection::vec(wave(), 1..=5)) {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .worker_threads(2)
      .enable_all()
      .build()
      .unwrap();
    rt.block_on(run_waves(waves));
  }
}

#[tokio::test]
async fn a_stale_validator_is_not_rescued_by_if_modified_since() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  harness.state().set_active_webapp(id).await.expect("activate");
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let earlier = body(4000, 29);
  let later = body(4000, 30);
  upload(&companion, &mut events, id, "earlier", "image/gif", &earlier).await;
  upload(&companion, &mut events, id, "current", "image/gif", &later).await;

  let http = reqwest::Client::new();
  let cached = http
    .get(blob_url(&harness, "/.bridgething/blob/current"))
    .send()
    .await
    .expect("get");
  let cached_etag = cached.headers()["etag"].clone();
  assert!(
    cached.headers().get("last-modified").is_none(),
    "the digest is the only validator the route hands out"
  );

  let ack = companion
    .webapp()
    .blob_put(put_request(
      id,
      "current",
      "image/gif",
      earlier.len() as u32,
      &sha_hex(&earlier),
    ))
    .await
    .expect("point the key at the earlier content");
  assert_eq!(ack.resume_from_offset as usize, earlier.len());
  settle(&mut events, id, "current", &sha_hex(&earlier))
    .await
    .outcome
    .expect("stored");

  let revalidated = http
    .get(blob_url(&harness, "/.bridgething/blob/current"))
    .header("if-none-match", cached_etag)
    .header("if-modified-since", AFTER_EVERY_UPLOAD)
    .send()
    .await
    .expect("conditional get");
  assert_eq!(
    revalidated.status(),
    reqwest::StatusCode::OK,
    "content older than the cached copy is still a change"
  );
  assert_eq!(revalidated.bytes().await.expect("body").as_ref(), earlier.as_slice());
}

#[tokio::test]
async fn time_validators_never_decide_what_the_blob_route_answers() {
  let harness = Harness::start().await.expect("harness start");
  let id = plant(&harness, false).await;
  harness.state().set_active_webapp(id).await.expect("activate");
  let companion = harness.connect_android().await.expect("connect companion");
  let mut events = webapp_events(&companion);
  let bytes = body(4000, 31);
  upload(&companion, &mut events, id, "clip", "video/mp4", &bytes).await;

  let http = reqwest::Client::new();
  let unchanged_since = http
    .get(blob_url(&harness, "/.bridgething/blob/clip"))
    .header("if-modified-since", AFTER_EVERY_UPLOAD)
    .send()
    .await
    .expect("if-modified-since get");
  assert_eq!(
    unchanged_since.status(),
    reqwest::StatusCode::OK,
    "only a matching digest earns a 304"
  );
  assert_eq!(unchanged_since.bytes().await.expect("body").as_ref(), bytes.as_slice());

  let changed_since = http
    .get(blob_url(&harness, "/.bridgething/blob/clip"))
    .header("if-unmodified-since", BEFORE_EVERY_UPLOAD)
    .send()
    .await
    .expect("if-unmodified-since get");
  assert_eq!(
    changed_since.status(),
    reqwest::StatusCode::OK,
    "a write time is not a precondition the route knows about"
  );

  let dated_range = http
    .get(blob_url(&harness, "/.bridgething/blob/clip"))
    .header("range", "bytes=0-9")
    .header("if-range", AFTER_EVERY_UPLOAD)
    .send()
    .await
    .expect("dated if-range get");
  assert_eq!(
    dated_range.status(),
    reqwest::StatusCode::OK,
    "a date is never a current validator, so the whole body ships"
  );
  assert_eq!(dated_range.bytes().await.expect("body").as_ref(), bytes.as_slice());
}

#[tokio::test]
async fn the_overlay_scope_has_no_blob_surface() {
  let harness = Harness::start().await.expect("harness start");
  let overlay_app = plant(&harness, true).await;
  let active_app = plant(&harness, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Overlay,
      id: Some(overlay_app),
    })
    .await
    .expect("overlay slot");
  harness.state().set_active_webapp(active_app).await.expect("activate");

  let mut events = webapp_events(&companion);
  let theirs = body(700, 32);
  let ours = body(900, 33);
  upload(&companion, &mut events, overlay_app, "background", "image/png", &theirs).await;
  upload(&companion, &mut events, active_app, "background", "image/jpeg", &ours).await;

  let overlay = harness.connect_overlay_command_client().await.expect("overlay client");
  let refused = overlay.blob().list().await;
  assert!(
    matches!(refused, Err(RequestFailure::Protocol(WireError::Unsupported))),
    "the overlay scope must not be handed urls the route resolves against another webapp: {refused:?}"
  );

  let foreground = harness.connect_command_client().await.expect("foreground client");
  let listed = foreground.blob().list().await.expect("foreground list").entries;
  assert_eq!(listed.len(), 1);
  assert_eq!(listed[0].sha256, sha_hex(&ours));
  let served = reqwest::get(blob_url(&harness, &listed[0].url))
    .await
    .expect("listed url");
  assert_eq!(served.headers()["content-type"], "image/jpeg");
  assert_eq!(served.bytes().await.expect("body").as_ref(), ours.as_slice());
}
