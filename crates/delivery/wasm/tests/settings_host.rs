#![cfg(target_arch = "wasm32")]

use std::rc::Rc;

use bridgething_delivery::{
  bundle::fetch::sha256_hex,
  harness::{FRAGMENT, FakeDevice, fake_device},
};
use bridgething_delivery_wasm::{WasmByteLink, WasmSession};
use bytes::{Bytes, BytesMut};
use futures::{StreamExt, channel::mpsc};
use libbridgething::{
  BridgeThingMeta, ConfigEntry, LauncherGesture, Priority, WebappError,
  gateway::{
    BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewayWebappMsg, GatewayToBridgeMsgData,
    GatewayToBridgeWebappMsg, WebappBlobDeleteAck, WebappBlobEntry, WebappConfigListReply, WebappDocChanged,
  },
  protocol::{BridgeEndec, Compress, DecodedFrame, encode_frame},
  wire::MsgMeta,
};
use uuid::Uuid;
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use wasm_bindgen_futures::spawn_local;
use wasm_bindgen_test::wasm_bindgen_test;

const PAGE_BYTES: usize = 40_000;

fn frame(msg: &BridgeToGatewayMsg) -> Vec<u8> {
  let mut out = BytesMut::new();
  encode_frame(Priority::Normal, Compress::Never, msg, &mut out).expect("the frame encodes");
  out.to_vec()
}

fn meta() -> BridgeThingMeta {
  BridgeThingMeta {
    bridgething_version: "0.9.0".into(),
    libbridgething_version: "0.9.0".into(),
    app_name: "bridgething".into(),
    nickname: None,
    launcher_gesture: LauncherGesture::default(),
    app_version: "0.9.0".into(),
    daemon_sha256: None,
    wakeword_model_version: None,
    os_name: "linux".into(),
    os_version: "1".into(),
    os_description: String::new(),
    bt_mac: String::new(),
    serial_number: String::new(),
    fcc_id: String::new(),
    ic_id: String::new(),
    model_name: "Car Thing".into(),
    channel: "stable".into(),
    image_variant: "prod".into(),
    image_version: "2026.10".into(),
    image_build_id: String::new(),
    image_build_date: String::new(),
    image_distro: String::new(),
    image_machine: String::new(),
    discord: String::new(),
    credits: String::new(),
  }
}

fn page() -> Vec<u8> {
  (0..PAGE_BYTES).map(|at| (at % 251) as u8).collect()
}

async fn linked() -> (WasmSession, FakeDevice) {
  let (written, mut wire) = mpsc::unbounded::<Vec<u8>>();
  let write = Closure::<dyn FnMut(js_sys::Uint8Array)>::new(move |chunk: js_sys::Uint8Array| {
    let _ = written.unbounded_send(chunk.to_vec());
  });
  let mut link = WasmByteLink::new(write.as_ref().unchecked_ref::<js_sys::Function>().clone());
  write.forget();

  link.push(&frame(&BridgeToGatewayMsg {
    id: Uuid::now_v7(),
    meta: MsgMeta::Event,
    data: BridgeToGatewayMsgData::Version(Box::new(meta())),
  }));
  let session = WasmSession::attach(&mut link, None)
    .await
    .expect("the session attaches");

  let (device, ear, mut said) = fake_device();
  spawn_local(async move {
    let mut endec = BridgeEndec::default();
    let mut pending = Bytes::new();
    while let Some(chunk) = wire.next().await {
      let mut joined = BytesMut::from(&pending[..]);
      joined.extend_from_slice(&chunk);
      let mut buffer = joined.freeze();
      while let Ok(Some(decoded)) = endec.decode_bytes(&mut buffer) {
        if let DecodedFrame::Frame(frame) = decoded {
          ear.hear(frame.priority, frame.msg);
        }
      }
      pending = buffer;
    }
  });
  let link = Rc::new(link);
  spawn_local(async move {
    while let Some(msg) = said.recv().await {
      link.push(&frame(&msg));
    }
    link.close();
  });

  (session, device)
}

async fn settle(done: impl Fn() -> bool) {
  for _ in 0..1000 {
    if done() {
      return;
    }
    let _ = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&JsValue::NULL)).await;
  }
}

fn field(value: &JsValue, key: &str) -> JsValue {
  js_sys::Reflect::get(value, &JsValue::from_str(key)).expect("the object answers the key")
}

#[wasm_bindgen_test]
async fn a_settings_page_streamed_over_the_byte_link_reaches_javascript_as_bytes() {
  let (session, mut device) = linked().await;
  let id = Uuid::now_v7().to_string();
  let transfer_id = Uuid::now_v7();

  let fetching = session.webapp_resource(id, "settings".into(), JsValue::NULL);
  let serving = async {
    let (request_id, request) = device.next_resource_request().await;
    device.stream_resource(request_id, &request, &page(), transfer_id);
    device.stream(transfer_id, &page(), 0..PAGE_BYTES, FRAGMENT);
    device.await_ack(transfer_id, PAGE_BYTES as u32).await;
  };
  let (body, ()) = futures::join!(fetching, serving);
  let body = body.expect("the page arrives");

  let bytes: js_sys::Uint8Array = field(&body, "bytes").dyn_into().expect("bytes are a Uint8Array");
  assert_eq!(bytes.to_vec(), page());
  assert_eq!(field(&body, "digest").as_string(), Some(sha256_hex(&page())));
  assert_eq!(field(&body, "mime").as_string().as_deref(), Some("text/html"));
}

#[wasm_bindgen_test]
async fn an_unknown_resource_kind_is_refused_before_anything_crosses_the_link() {
  let (session, _device) = linked().await;

  let refused = session
    .webapp_resource(Uuid::now_v7().to_string(), "wallpaper".into(), JsValue::NULL)
    .await
    .expect_err("there is no wallpaper resource");

  assert_eq!(
    refused.as_string().as_deref(),
    Some("unknown webapp resource kind wallpaper")
  );
}

#[wasm_bindgen_test]
async fn config_entries_reach_javascript_in_the_wire_shape() {
  let (session, mut device) = linked().await;

  let listing = session.webapp_config(Uuid::now_v7().to_string());
  let answering = async {
    let request_id = device
      .next_matching(|msg| match &msg.data {
        GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::ConfigList(_)) => Some(msg.id),
        _ => None,
      })
      .await;
    device.respond(
      request_id,
      BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::ConfigList(WebappConfigListReply {
        entries: vec![ConfigEntry {
          key: "server".into(),
          value: "ws://ha.local".into(),
        }],
      })),
    );
  };
  let (entries, ()) = futures::join!(listing, answering);
  let entries: js_sys::Array = entries.expect("the config lists").dyn_into().expect("an array");

  assert_eq!(entries.length(), 1);
  assert_eq!(field(&entries.get(0), "key").as_string().as_deref(), Some("server"));
  assert_eq!(
    field(&entries.get(0), "value").as_string().as_deref(),
    Some("ws://ha.local")
  );
}

#[wasm_bindgen_test]
async fn a_doc_written_on_the_device_is_the_next_doc_change_with_a_null_for_a_cleared_key() {
  let (session, device) = linked().await;
  let id = Uuid::now_v7();

  device.event(BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::DocChanged(
    WebappDocChanged {
      id,
      key: "dashboard".into(),
      value: None,
    },
  )));
  let change = session.next_doc_change().await.expect("the change arrives");

  assert_eq!(field(&change, "id").as_string(), Some(id.to_string()));
  assert_eq!(field(&change, "key").as_string().as_deref(), Some("dashboard"));
  assert!(field(&change, "value").is_null(), "a cleared key is null, not missing");
}

fn base64_of(bytes: &[u8]) -> String {
  let binary: String = bytes.iter().map(|byte| char::from(*byte)).collect();
  let btoa = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("btoa")).expect("the host has btoa");
  js_sys::Function::from(btoa)
    .call1(&JsValue::NULL, &JsValue::from_str(&binary))
    .expect("btoa encodes")
    .as_string()
    .expect("btoa returns a string")
}

#[wasm_bindgen_test]
async fn a_page_built_handle_streams_over_the_byte_link_and_resolves_with_its_entry() {
  let (session, mut device) = linked().await;
  let id = Uuid::now_v7();
  let body: Vec<u8> = (0..70_000).map(|at| (at % 241) as u8).collect();
  let handle = session.create_blob_handle("image/gif".into()).expect("a handle");
  for chunk in body.chunks(32 * 1024) {
    let base64 = base64_of(chunk);
    session
      .append_blob_handle(handle.clone(), base64)
      .expect("the chunk lands");
  }

  let heard = js_sys::Array::new();
  let tick = Closure::<dyn FnMut(f64, f64)>::new({
    let heard = heard.clone();
    move |sent: f64, total: f64| {
      heard.push(&js_sys::Array::of2(&JsValue::from_f64(sent), &JsValue::from_f64(total)));
    }
  });
  let putting = session.put_webapp_blob(
    id.to_string(),
    "loop".into(),
    handle.clone(),
    Some(tick.as_ref().unchecked_ref::<js_sys::Function>().clone()),
  );
  let storing = async {
    let (request_id, put) = device.await_blob_put().await;
    device.ack_blob_put(request_id, 0);
    let received = device.receive_blob(&put).await;
    device.blob_stored(&put);
    (put.key, received)
  };
  let (stored, landed) = futures::join!(putting, storing);
  let stored = stored.expect("the device stores it");
  let whole = body.len() as f64;
  let ticks = || -> Vec<(f64, f64)> {
    heard
      .iter()
      .map(|pair| {
        let pair = js_sys::Array::from(&pair);
        (pair.get(0).as_f64().unwrap(), pair.get(1).as_f64().unwrap())
      })
      .collect()
  };
  settle(|| ticks().last() == Some(&(whole, whole))).await;
  let seen = ticks();
  assert_eq!(
    seen.first(),
    Some(&(0.0, whole)),
    "the first tick is the start: {seen:?}"
  );
  assert_eq!(
    seen.last(),
    Some(&(whole, whole)),
    "the last tick is the whole file: {seen:?}"
  );
  assert!(seen.windows(2).all(|pair| pair[0].0 <= pair[1].0), "{seen:?}");

  let entry: WebappBlobEntry = serde_wasm_bindgen::from_value(stored).expect("the entry is the wire shape");
  assert_eq!(
    entry,
    WebappBlobEntry {
      key: "loop".into(),
      sha256: sha256_hex(&body),
      mime: "image/gif".into(),
      size: body.len() as u32,
    }
  );
  assert_eq!(landed, ("loop".to_owned(), body));
  assert_eq!(
    token(
      &session
        .append_blob_handle(handle, base64_of(b"x"))
        .expect_err("a stored pick is released")
    ),
    "unknown_handle"
  );
}

fn handle_of(session: &WasmSession, body: &[u8]) -> String {
  let handle = session.create_blob_handle("video/mp4".into()).expect("a handle");
  for chunk in body.chunks(32 * 1024) {
    session
      .append_blob_handle(handle.clone(), base64_of(chunk))
      .expect("the chunk lands");
  }
  handle
}

fn token(rejection: &JsValue) -> String {
  let message = rejection.as_string().expect("a blob verb rejects with a string");
  let (token, rest) = message.split_once(':').expect("a token and a colon lead the message");
  assert!(
    !token.is_empty() && token.chars().all(|letter| letter.is_ascii_lowercase() || letter == '_'),
    "the token is snake_case: {message}"
  );
  assert!(rest.starts_with(' '), "a message follows the token: {message}");
  token.to_owned()
}

#[wasm_bindgen_test]
async fn a_blob_verb_the_host_or_the_device_refuses_rejects_with_a_leading_token() {
  let (session, mut device) = linked().await;
  let id = Uuid::now_v7().to_string();
  let handle = handle_of(&session, b"small");

  let unknown = session
    .put_webapp_blob(id.clone(), "clip".into(), "no-such-handle".into(), None)
    .await
    .expect_err("the handle is not live");
  assert_eq!(token(&unknown), "unknown_handle");

  let unknown = session
    .append_blob_handle("no-such-handle".into(), base64_of(b"x"))
    .expect_err("the handle is not live");
  assert_eq!(token(&unknown), "unknown_handle");

  let garbled = session
    .append_blob_handle(handle.clone(), "not base64!".into())
    .expect_err("the chunk does not decode");
  assert_eq!(token(&garbled), "invalid");

  let misaddressed = session
    .put_webapp_blob("not-a-uuid".into(), "clip".into(), handle, None)
    .await
    .expect_err("the webapp id does not parse");
  assert_eq!(token(&misaddressed), "invalid");

  let deleting = session.delete_webapp_blob(id.clone(), "no/slashes".into());
  let refusing = async {
    let (request_id, key) = next_delete(&mut device).await;
    device.refuse_webapp(
      request_id,
      WebappError::InvalidBlobKey {
        key,
        reason: "keys are one path segment".into(),
      },
    );
  };
  let (refused, ()) = futures::join!(deleting, refusing);
  assert_eq!(token(&refused.expect_err("the device refuses the key")), "invalid_key");

  let deleting = session.delete_webapp_blob(id, "clip".into());
  let acking = async {
    let (request_id, key) = next_delete(&mut device).await;
    device.respond(
      request_id,
      BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::BlobDeleteAck(WebappBlobDeleteAck { key })),
    );
  };
  let (deleted, ()) = futures::join!(deleting, acking);
  deleted.expect("a well formed delete is acked");
}

async fn next_delete(device: &mut FakeDevice) -> (Uuid, String) {
  device
    .next_matching(|msg| match &msg.data {
      GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::BlobDelete(delete)) => {
        Some((msg.id, delete.key.clone()))
      }
      _ => None,
    })
    .await
}

#[wasm_bindgen_test]
async fn a_link_that_closes_mid_upload_rejects_the_put_as_link_lost_and_later_verbs_as_not_connected() {
  let (session, mut device) = linked().await;
  let id = Uuid::now_v7().to_string();
  let body: Vec<u8> = (0..512 * 1024).map(|at| (at % 239) as u8).collect();
  let handle = handle_of(&session, &body);

  let putting = session.put_webapp_blob(id.clone(), "clip".into(), handle.clone(), None);
  let dropping = async {
    let (request_id, put) = device.await_blob_put().await;
    device.ack_blob_put(request_id, 0);
    device.next_fragment(put.transfer.id).await;
    device.hang_up();
  };
  let (dropped, ()) = futures::join!(putting, dropping);
  assert_eq!(token(&dropped.expect_err("a dead link stores nothing")), "link_lost");

  session.closed().await;
  let put = session
    .put_webapp_blob(id.clone(), "clip".into(), handle, None)
    .await
    .expect_err("nothing is connected");
  assert_eq!(token(&put), "not_connected");
  let deleted = session
    .delete_webapp_blob(id.clone(), "clip".into())
    .await
    .expect_err("nothing is connected");
  assert_eq!(token(&deleted), "not_connected");
  let listed = session.webapp_blobs(id).await.expect_err("nothing is connected");
  assert_eq!(token(&listed), "not_connected");
  assert!(
    session
      .next_doc_change()
      .await
      .expect("the doc feed ends cleanly")
      .is_null(),
    "a closed link ends the doc feed"
  );
}
