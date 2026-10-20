use std::{collections::BTreeSet, time::Duration};

use bridgething_gateway::Gateway;
use bridgething_test_harness::{Harness, MockWsClient};
use futures::StreamExt;
use libbridgething::{client::DocSet, gateway::BridgeToGatewayWebappMsgEvent};
use uuid::Uuid;

const PATIENT: Duration = Duration::from_secs(10);
const SETTLED: Duration = Duration::from_millis(300);
const DOC_BYTES: usize = 250 * 1024;
const PAGE_READS: usize = 128;
const DRAIN_DEADLINE: Duration = Duration::from_secs(2);
const GET_REPLY: &str = r#""event":"get""#;

const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

fn incompressible(seed: u64, len: usize) -> String {
  let mut state = seed | 1;
  (0..len)
    .map(|_| {
      state ^= state << 13;
      state ^= state >> 7;
      state ^= state << 17;
      char::from(ALPHABET[(state % ALPHABET.len() as u64) as usize])
    })
    .collect()
}

fn request(id: Uuid, event: &str, data: &str) -> String {
  format!(
    r#"{{"id":"{id}","meta":{{"kind":"request"}},"data":{{"type":"doc","data":{{"event":"{event}","data":{data}}}}}}}"#
  )
}

async fn page_behind_on_reads(harness: &Harness) -> (MockWsClient, usize) {
  let mut page = harness.connect_modern_client().await.expect("modern client");
  let stored = Uuid::now_v7();
  page
    .send_text(request(
      stored,
      "set",
      &format!(r#"{{"key":"big","value":"{}"}}"#, incompressible(1, DOC_BYTES)),
    ))
    .await
    .expect("doc set");
  let acked = tokio::time::timeout(PATIENT, async {
    while let Some(text) = page.recv().await {
      if text.contains(&stored.to_string()) {
        return text;
      }
    }
    panic!("the page's socket closed before the set was answered");
  })
  .await
  .expect("the set is answered");
  assert!(
    acked.contains(r#""event":"ack""#),
    "got {}",
    &acked[..acked.len().min(200)]
  );
  let mut frames = harness.observe_frames();
  for _ in 0..PAGE_READS {
    page
      .send_text(request(Uuid::now_v7(), "get", r#"{"key":"big"}"#))
      .await
      .expect("doc get");
  }
  let mut sent = 0;
  loop {
    let batch = frames.collect_for(SETTLED).await;
    let replies = batch.iter().filter(|frame| frame.json().contains(GET_REPLY)).count();
    if replies == 0 && sent > 0 {
      break;
    }
    sent += replies;
  }
  assert!(
    sent < PAGE_READS,
    "the page's socket took every reply; nothing is left queued to drain"
  );
  (page, sent)
}

#[tokio::test]
async fn shutdown_waits_for_the_frames_queued_to_a_page_and_then_closes_it() {
  let harness = Harness::start().await.expect("harness start");
  let (mut page, sent_before) = page_behind_on_reads(&harness).await;
  let mut frames = harness.observe_frames();

  harness.stop_daemon();
  tokio::time::sleep(SETTLED).await;

  assert!(
    !harness.daemon_exited(),
    "the daemon waits while a page has frames it has not taken"
  );
  let (texts, closed) = tokio::time::timeout(PATIENT, page.recv_until_closed())
    .await
    .expect("the page's socket closes once it is drained");
  assert_eq!(closed, Some((1000, "bye".to_string())));
  let received = texts.iter().filter(|text| text.contains(GET_REPLY)).count();
  let sent_during = frames
    .collect_for(SETTLED)
    .await
    .iter()
    .filter(|frame| frame.json().contains(GET_REPLY))
    .count();
  assert!(sent_during > 0, "the drain wrote replies that were still queued");
  assert_eq!(
    received,
    sent_before + sent_during,
    "every reply the daemon sent reached the page before the socket closed"
  );
  assert!(harness.wait_for(|_| harness.daemon_exited(), PATIENT).await);
}

#[tokio::test]
async fn a_page_that_never_reads_holds_shutdown_only_until_the_deadline() {
  let harness = Harness::start().await.expect("harness start");
  let (_page, _) = page_behind_on_reads(&harness).await;

  let stopping = tokio::time::Instant::now();
  harness.stop_daemon();

  assert!(
    harness
      .wait_for(|_| harness.daemon_exited(), DRAIN_DEADLINE + PATIENT)
      .await
  );
  let waited = stopping.elapsed();
  assert!(
    waited >= DRAIN_DEADLINE,
    "the daemon gave the page its drain window, exited after {waited:?}"
  );
  assert!(
    waited < DRAIN_DEADLINE + Duration::from_secs(1),
    "a stalled page holds the exit no longer than the drain deadline, took {waited:?}"
  );
}

#[tokio::test]
async fn shutdown_delivers_every_change_the_page_was_told_reached_the_companion() {
  let harness = Harness::start().await.expect("harness start");
  let companion_link = harness.connect_android_io().await.expect("companion link");
  let page = harness.connect_command_client().await.expect("command client");
  let mut acked = BTreeSet::new();
  for n in 0..400u64 {
    let key = format!("k{n:03}");
    let docs = page.doc();
    let set = docs.set(DocSet {
      key: key.clone(),
      value: incompressible(n + 2, DOC_BYTES),
    });
    match tokio::time::timeout(SETTLED, set).await {
      Ok(answer) => {
        answer.expect("doc set");
        acked.insert(key);
      }
      Err(_) => break,
    }
  }
  assert!(
    acked.len() < 400,
    "the companion never fell behind; nothing is left queued to drain"
  );

  harness.stop_daemon();
  tokio::time::sleep(SETTLED).await;
  assert!(
    !harness.daemon_exited(),
    "the daemon waits while the companion has frames it has not taken"
  );

  let companion = Gateway::from_io(companion_link);
  let mut changes = Box::pin(companion.webapp().events());
  let mut delivered = BTreeSet::new();
  let caught_up = tokio::time::timeout(PATIENT, async {
    while !acked.is_subset(&delivered) {
      if let Some(BridgeToGatewayWebappMsgEvent::DocChanged(changed)) = changes.next().await {
        delivered.insert(changed.key);
      }
    }
  })
  .await;

  assert!(
    caught_up.is_ok(),
    "{} acknowledged changes never reached the companion",
    acked.difference(&delivered).count()
  );
  assert!(harness.wait_for(|_| harness.daemon_exited(), PATIENT).await);
}
