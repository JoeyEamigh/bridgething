use std::time::Duration;

use bridgething_test_harness::Harness;
use futures::StreamExt;
use libbridgething::{
  client::{DocGet as ClientDocGet, DocSet as ClientDocSet},
  gateway::{
    BridgeToGatewayTransferMsgEvent, BridgeToGatewayWebappMsgEvent, TransferAck, TransferBody, WebappConfigDelete,
    WebappConfigSet, WebappDocGet, WebappDocList, WebappDocSet, WebappResource, WebappResourceKind,
  },
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const EVENT_WAIT: Duration = Duration::from_secs(5);
const FRAGMENT_LEN: usize = 4 * 1024;
const WINDOW: usize = 64 * 1024;

fn sha_hex(bytes: &[u8]) -> String {
  let mut h = Sha256::new();
  h.update(bytes);
  hex::encode(h.finalize())
}

async fn plant_bundle(harness: &Harness, id: Uuid, settings_len: usize) -> (Vec<u8>, Vec<u8>) {
  let dir = harness.state_dir().join("webapps").join(id.simple().to_string());
  std::fs::create_dir_all(&dir).expect("bundle dir");
  std::fs::write(dir.join("index.html"), b"<h1>hi</h1>").expect("index");
  let icon = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec();
  std::fs::write(dir.join("icon.svg"), &icon).expect("icon");
  let settings: Vec<u8> = (0..settings_len).map(|i| (i % 251) as u8).collect();
  let config = r#""config":[{"type":"string","data":{"key":"zip","label":"Zip"}}]"#;
  let manifest = if settings_len > 0 {
    std::fs::write(dir.join("settings.html"), &settings).expect("settings");
    format!(
      r#"{{"id":"{id}","name":"planted","version":"0.1.0","icon":"icon.svg","settings":"settings.html",{config}}}"#
    )
  } else {
    format!(r#"{{"id":"{id}","name":"planted","version":"0.1.0","icon":"icon.svg",{config}}}"#)
  };
  std::fs::write(dir.join("manifest.json"), manifest).expect("manifest");
  harness.state().webapps.rescan().await;
  (icon, settings)
}

#[tokio::test]
async fn gateway_list_carries_hashes_not_bytes() {
  let harness = Harness::start().await.expect("harness start");
  let id = Uuid::now_v7();
  let (icon, settings) = plant_bundle(&harness, id, 1024).await;

  let companion = harness.connect_android().await.expect("connect companion");
  let list = companion.webapp().list().await.expect("list");
  let info = list.webapps.iter().find(|w| w.id == id).expect("planted app listed");
  assert_eq!(info.icon_hash.as_deref(), Some(sha_hex(&icon).as_str()));
  assert_eq!(info.settings_hash.as_deref(), Some(sha_hex(&settings).as_str()));
}

#[tokio::test]
async fn small_resource_rides_inline_and_have_match_elides_the_body() {
  let harness = Harness::start().await.expect("harness start");
  let id = Uuid::now_v7();
  let (icon, _) = plant_bundle(&harness, id, 0).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let reply = companion
    .webapp()
    .resource(WebappResource {
      id,
      kind: WebappResourceKind::Icon,
      have: None,
    })
    .await
    .expect("resource");
  assert_eq!(reply.sha256, sha_hex(&icon));
  assert_eq!(reply.mime.as_deref(), Some("image/svg+xml"));
  match reply.body {
    Some(TransferBody::Inline(bytes)) => assert_eq!(bytes, icon),
    other => panic!("small icon must ride inline, got {other:?}"),
  }

  let cached = companion
    .webapp()
    .resource(WebappResource {
      id,
      kind: WebappResourceKind::Icon,
      have: Some(sha_hex(&icon)),
    })
    .await
    .expect("resource with have");
  assert!(cached.body.is_none(), "matching have must elide the body");
  assert_eq!(cached.sha256, sha_hex(&icon));
  assert_eq!(cached.mime.as_deref(), Some("image/svg+xml"));
}

#[tokio::test]
async fn missing_resource_is_a_domain_error() {
  let harness = Harness::start().await.expect("harness start");
  let id = Uuid::now_v7();
  plant_bundle(&harness, id, 0).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let err = companion
    .webapp()
    .resource(WebappResource {
      id,
      kind: WebappResourceKind::Settings,
      have: None,
    })
    .await
    .expect_err("no settings page declared");
  assert!(
    format!("{err:?}").contains("ResourceNotAvailable"),
    "expected ResourceNotAvailable, got {err:?}"
  );
}

#[tokio::test]
async fn large_settings_page_streams_under_ack_window() {
  let harness = Harness::start().await.expect("harness start");
  let id = Uuid::now_v7();
  let (_, settings) = plant_bundle(&harness, id, 3 * WINDOW + 12_345).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let mut transfer_events = Box::pin(companion.transfer().events());

  let reply = companion
    .webapp()
    .resource(WebappResource {
      id,
      kind: WebappResourceKind::Settings,
      have: None,
    })
    .await
    .expect("resource");
  assert_eq!(reply.mime.as_deref(), Some("text/html"));
  let transfer = match reply.body {
    Some(TransferBody::Stream(t)) => t,
    other => panic!("large settings page must stream, got {other:?}"),
  };
  assert_eq!(transfer.total_size as usize, settings.len());
  assert_eq!(transfer.sha256.as_deref(), Some(reply.sha256.as_str()));

  let mut buf: Vec<u8> = Vec::with_capacity(settings.len());
  let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
  while buf.len() < settings.len() {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    assert!(!remaining.is_zero(), "stream stalled at {} bytes", buf.len());
    let event = tokio::time::timeout(remaining, transfer_events.next())
      .await
      .expect("fragment before deadline")
      .expect("stream open");
    match event {
      BridgeToGatewayTransferMsgEvent::Fragment(f) => {
        assert_eq!(f.transfer_id, transfer.id);
        assert_eq!(f.offset as usize, buf.len(), "fragments must arrive offset-ordered");
        assert!(f.bytes.len() <= FRAGMENT_LEN, "fragment exceeds preemption budget");
        buf.extend_from_slice(&f.bytes);
        companion
          .transfer()
          .ack(TransferAck {
            transfer_id: transfer.id,
            received: buf.len() as u32,
          })
          .await
          .expect("ack");
      }
      BridgeToGatewayTransferMsgEvent::Abandon(a) => panic!("sender abandoned: {}", a.reason),
      BridgeToGatewayTransferMsgEvent::Ack(_) => {}
    }
  }
  assert_eq!(buf, settings);
  assert_eq!(sha_hex(&buf), reply.sha256);
}

#[tokio::test]
async fn gateway_doc_write_reaches_the_active_webapp_live() {
  let harness = Harness::start().await.expect("harness start");
  let id = Uuid::now_v7();
  plant_bundle(&harness, id, 0).await;
  harness.state().set_active_webapp(id).await.expect("activate");

  let companion = harness.connect_android().await.expect("connect companion");
  let client = harness.connect_command_client().await.expect("command client");
  let mut doc_events = Box::pin(client.doc().events());

  let ack = companion
    .webapp()
    .doc_set(WebappDocSet {
      id,
      key: "dashboard".into(),
      value: r#"{"tiles":[1,2,3]}"#.into(),
    })
    .await
    .expect("doc set");
  assert_eq!(ack.value.as_deref(), Some(r#"{"tiles":[1,2,3]}"#));

  let changed = tokio::time::timeout(EVENT_WAIT, doc_events.next())
    .await
    .expect("doc change event")
    .expect("stream open");
  let libbridgething::client::BridgeToClientDocMsgEvent::Changed(changed) = changed;
  assert_eq!(changed.key, "dashboard");
  assert_eq!(changed.value.as_deref(), Some(r#"{"tiles":[1,2,3]}"#));

  let read_back = client
    .doc()
    .get(ClientDocGet {
      key: "dashboard".into(),
    })
    .await
    .expect("client doc get");
  assert_eq!(read_back.value.as_deref(), Some(r#"{"tiles":[1,2,3]}"#));
}

#[tokio::test]
async fn a_config_write_is_announced_to_every_gateway_whether_or_not_the_webapp_is_active() {
  let harness = Harness::start().await.expect("harness start");
  let id = Uuid::now_v7();
  plant_bundle(&harness, id, 0).await;

  let writer = harness.connect_android().await.expect("connect writer");
  let listener = harness.connect_android().await.expect("connect listener");
  let mut webapp_events = Box::pin(listener.webapp().events());

  writer
    .webapp()
    .config_set(WebappConfigSet {
      id,
      key: "zip".into(),
      value: "10001".into(),
    })
    .await
    .expect("config set");

  let changed = loop {
    let event = tokio::time::timeout(EVENT_WAIT, webapp_events.next())
      .await
      .expect("gateway config change event")
      .expect("stream open");
    if let BridgeToGatewayWebappMsgEvent::ConfigChanged(c) = event {
      break c;
    }
  };
  assert_eq!(changed.id, id);
  assert_eq!(changed.key, "zip");
  assert_eq!(changed.value.as_deref(), Some("10001"));

  writer
    .webapp()
    .config_delete(WebappConfigDelete { id, key: "zip".into() })
    .await
    .expect("config delete");

  let cleared = loop {
    let event = tokio::time::timeout(EVENT_WAIT, webapp_events.next())
      .await
      .expect("gateway config reset event")
      .expect("stream open");
    if let BridgeToGatewayWebappMsgEvent::ConfigChanged(c) = event {
      break c;
    }
  };
  assert_eq!(cleared.key, "zip");
  assert_eq!(cleared.value, None, "a reset with no default clears the key");
}

#[tokio::test]
async fn webapp_doc_write_reaches_the_companion_live() {
  let harness = Harness::start().await.expect("harness start");
  let id = Uuid::now_v7();
  plant_bundle(&harness, id, 0).await;
  harness.state().set_active_webapp(id).await.expect("activate");

  let companion = harness.connect_android().await.expect("connect companion");
  let client = harness.connect_command_client().await.expect("command client");
  let mut webapp_events = Box::pin(companion.webapp().events());

  client
    .doc()
    .set(ClientDocSet {
      key: "state".into(),
      value: "migrated".into(),
    })
    .await
    .expect("client doc set");

  let changed = loop {
    let event = tokio::time::timeout(EVENT_WAIT, webapp_events.next())
      .await
      .expect("gateway doc change event")
      .expect("stream open");
    if let BridgeToGatewayWebappMsgEvent::DocChanged(c) = event {
      break c;
    }
  };
  assert_eq!(changed.id, id);
  assert_eq!(changed.key, "state");
  assert_eq!(changed.value.as_deref(), Some("migrated"));

  let listed = companion
    .webapp()
    .doc_list(WebappDocList { id })
    .await
    .expect("doc list");
  assert_eq!(listed.entries.len(), 1);
  assert_eq!(listed.entries[0].key, "state");

  let got = companion
    .webapp()
    .doc_get(WebappDocGet {
      id,
      key: "state".into(),
    })
    .await
    .expect("doc get");
  assert_eq!(got.value.as_deref(), Some("migrated"));
}

#[tokio::test]
async fn oversized_doc_value_is_refused_end_to_end() {
  let harness = Harness::start().await.expect("harness start");
  let id = Uuid::now_v7();
  plant_bundle(&harness, id, 0).await;
  harness.state().set_active_webapp(id).await.expect("activate");

  let companion = harness.connect_android().await.expect("connect companion");
  let client = harness.connect_command_client().await.expect("command client");
  let oversized = "x".repeat(256 * 1024 + 1);

  let gateway_err = companion
    .webapp()
    .doc_set(WebappDocSet {
      id,
      key: "big".into(),
      value: oversized.clone(),
    })
    .await
    .expect_err("oversized value must be refused");
  assert!(format!("{gateway_err:?}").contains("InvalidDocValue"));

  let client_err = client
    .doc()
    .set(ClientDocSet {
      key: "big".into(),
      value: oversized,
    })
    .await
    .expect_err("oversized value must be refused");
  assert!(format!("{client_err:?}").contains("InvalidDocValue"));

  let got = companion
    .webapp()
    .doc_get(WebappDocGet { id, key: "big".into() })
    .await
    .expect("doc get");
  assert!(got.value.is_none(), "refused writes must not land");
}
