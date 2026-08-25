use std::time::Duration;

use bridgething::{Address, ClientMode, TappedFrame};
use bridgething_iap2::{SessionEvent, csm::telephony::CallStateUpdate};
use bridgething_test_harness::Harness;
use libbridgething::{Device, DeviceType, LinkKind, PeerIap2Status};
use serde_json::Value;

const CONVERGE: Duration = Duration::from_secs(3);

fn ringing_incoming(call_id: &str) -> CallStateUpdate {
  CallStateUpdate {
    remote_id: Some("+15555550100".into()),
    display_name: Some("Test Caller".into()),
    status: Some(2),
    direction: Some(1),
    call_uuid: Some(call_id.into()),
    ..Default::default()
  }
}

async fn identify_ios_peer(harness: &Harness, mac: Address) {
  let peers = harness.state().peers.clone();
  peers
    .ensure_exists(
      mac,
      Device {
        name: "test-iphone".into(),
        device_type: DeviceType::Ios,
        id: mac.to_string(),
        kind: LinkKind::Bluetooth,
        default: false,
      },
    )
    .await;
  peers.set_iap2(mac, PeerIap2Status::Identified).await;
  let up = harness
    .wait_for(
      |s| s.peers.snapshot().peers.get(&mac).is_some_and(|p| p.has_useful_link()),
      CONVERGE,
    )
    .await;
  assert!(up, "iap2 peer never reached a useful link");
}

fn stock_call_info(frame: &TappedFrame) -> Option<Value> {
  if frame.mode != ClientMode::Stock {
    return None;
  }
  let value: Value = serde_json::from_str(frame.json()).ok()?;
  (value.get("type")?.as_str()? == "phone_call_info").then_some(value)
}

fn modern_phone_event(frame: &TappedFrame, event: &str) -> Option<Value> {
  if frame.mode != ClientMode::Modern {
    return None;
  }
  let value: Value = serde_json::from_str(frame.json()).ok()?;
  let surface = value.get("data")?;
  if surface.get("type")?.as_str()? != "phone" {
    return None;
  }
  let msg = surface.get("data")?;
  (msg.get("event")?.as_str()? == event).then(|| msg.get("data").cloned().unwrap_or(Value::Null))
}

#[tokio::test]
async fn iap2_incoming_call_reaches_stock_with_pascal_case_fields() {
  let harness = Harness::start().await.expect("harness start");
  let mut frames = harness.observe_frames();
  let _client = harness.connect_stock_client().await.expect("connect stock client");

  let registered = harness
    .wait_for(|state| state.client_man.client_count() >= 1, CONVERGE)
    .await;
  assert!(registered, "stock client never registered");

  let phone = harness.iap2_peer();
  identify_ios_peer(&harness, phone).await;
  harness
    .inject_iap2(phone, SessionEvent::CallStateUpdate(ringing_incoming("call-1")))
    .await
    .expect("inject call state");

  let observed = frames
    .wait_for(CONVERGE, |f| stock_call_info(f).is_some())
    .await
    .expect("no stock phone_call_info frame");
  let info = stock_call_info(&observed).expect("frame parses");
  assert_eq!(info["status"].as_str(), Some("Ringing"));
  assert_eq!(info["call_dir"].as_str(), Some("Incoming"));
  assert_eq!(info["display_name"].as_str(), Some("Test Caller"));
  assert_eq!(info["remote_id"].as_str(), Some("(555) 555-0100"));
  assert!(info.get("service").is_none(), "service must not cross to stock");
  assert_eq!(info["call_id"].as_str(), Some("call-1"));
}

#[tokio::test]
async fn iap2_call_lifecycle_started_updated_ended() {
  let harness = Harness::start().await.expect("harness start");
  let mut frames = harness.observe_frames();
  let _client = harness.connect_modern_client().await.expect("connect modern client");

  let registered = harness
    .wait_for(|state| state.client_man.client_count() >= 1, CONVERGE)
    .await;
  assert!(registered, "modern client never registered");

  let phone = harness.iap2_peer();
  harness
    .inject_iap2(phone, SessionEvent::CallStateUpdate(ringing_incoming("call-2")))
    .await
    .expect("inject ringing");

  let started = frames
    .wait_for(CONVERGE, |f| modern_phone_event(f, "callStarted").is_some())
    .await;
  assert!(started.is_some(), "no callStarted broadcast");

  harness
    .inject_iap2(
      phone,
      SessionEvent::CallStateUpdate(CallStateUpdate {
        status: Some(4),
        call_uuid: Some("call-2".into()),
        ..Default::default()
      }),
    )
    .await
    .expect("inject active");

  let updated = frames
    .wait_for(CONVERGE, |f| {
      modern_phone_event(f, "callUpdated").is_some_and(|data| {
        data["status"].as_str() == Some("active") && data["displayName"].as_str() == Some("Test Caller")
      })
    })
    .await;
  assert!(updated.is_some(), "no callUpdated broadcast with merged active status");

  harness
    .inject_iap2(
      phone,
      SessionEvent::CallStateUpdate(CallStateUpdate {
        status: Some(0),
        disconnect_reason: Some(0),
        call_uuid: Some("call-2".into()),
        ..Default::default()
      }),
    )
    .await
    .expect("inject disconnect");

  let ended = frames
    .wait_for(CONVERGE, |f| modern_phone_event(f, "callEnded").is_some())
    .await;
  assert!(ended.is_some(), "no callEnded broadcast");

  let deadline = tokio::time::Instant::now() + CONVERGE;
  loop {
    if harness.state().telephony.snapshot().await.active_calls.is_empty() {
      break;
    }
    assert!(
      tokio::time::Instant::now() < deadline,
      "disconnected call not evicted from state"
    );
    tokio::time::sleep(Duration::from_millis(25)).await;
  }
}

#[tokio::test]
async fn iap2_uuid_less_delta_advances_the_single_tracked_call() {
  let harness = Harness::start().await.expect("harness start");
  let mut frames = harness.observe_frames();
  let _client = harness.connect_modern_client().await.expect("connect modern client");

  let registered = harness
    .wait_for(|state| state.client_man.client_count() >= 1, CONVERGE)
    .await;
  assert!(registered, "modern client never registered");

  let phone = harness.iap2_peer();
  harness
    .inject_iap2(phone, SessionEvent::CallStateUpdate(ringing_incoming("call-4")))
    .await
    .expect("inject ringing");

  let started = frames
    .wait_for(CONVERGE, |f| modern_phone_event(f, "callStarted").is_some())
    .await;
  assert!(started.is_some(), "no callStarted broadcast");

  harness
    .inject_iap2(
      phone,
      SessionEvent::CallStateUpdate(CallStateUpdate {
        status: Some(4),
        ..Default::default()
      }),
    )
    .await
    .expect("inject uuid-less active delta");

  let updated = frames
    .wait_for(CONVERGE, |f| {
      modern_phone_event(f, "callUpdated")
        .is_some_and(|data| data["callId"].as_str() == Some("call-4") && data["status"].as_str() == Some("active"))
    })
    .await;
  assert!(updated.is_some(), "uuid-less delta did not advance the tracked call");
}

#[tokio::test]
async fn iap2_unanswered_ring_ends_as_missed() {
  let harness = Harness::start().await.expect("harness start");
  let mut frames = harness.observe_frames();
  let _client = harness.connect_modern_client().await.expect("connect modern client");

  let registered = harness
    .wait_for(|state| state.client_man.client_count() >= 1, CONVERGE)
    .await;
  assert!(registered, "modern client never registered");

  let phone = harness.iap2_peer();
  harness
    .inject_iap2(phone, SessionEvent::CallStateUpdate(ringing_incoming("call-3")))
    .await
    .expect("inject ringing");
  harness
    .inject_iap2(
      phone,
      SessionEvent::CallStateUpdate(CallStateUpdate {
        status: Some(0),
        disconnect_reason: Some(0),
        call_uuid: Some("call-3".into()),
        ..Default::default()
      }),
    )
    .await
    .expect("inject disconnect");

  let ended = frames
    .wait_for(CONVERGE, |f| {
      modern_phone_event(f, "callEnded").is_some_and(|data| data["reason"]["type"].as_str() == Some("missed"))
    })
    .await;
  assert!(ended.is_some(), "unanswered ring did not end as missed");
}
