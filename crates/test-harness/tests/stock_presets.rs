use std::time::{Duration, Instant};

use base64::Engine;
use bridgething::ChromeCommand;
use bridgething_delivery::webapp::STOCK_WEBAPP_ID;
use bridgething_gateway::Gateway;
use bridgething_test_harness::{Harness, MockWsClient};
use libbridgething::{
  AssetRetention, GatewayCapabilities, GatewayInfo,
  gateway::{
    AssetGotReply, AssetNotFoundReply, AssetRequest, BridgeToGatewayAssetMsg, BridgeToGatewayLibraryMsg,
    BridgeToGatewayMsgData, ContextResolveReply, GatewayToBridgeAssetMsg, GatewayToBridgeLibraryMsg,
    GatewayToBridgeMsgData, TransferBody,
  },
};
use serde_json::Value;

const PLAYLIST: &str = "spotify:playlist:roadtrip";
const ART: &str = "gw/img/300/roadtrip";
const HERO_ART: &str = "gw/img/248/roadtrip";
const REPLY: Duration = Duration::from_secs(3);

async fn announce(gateway: &Gateway) {
  let caps = GatewayCapabilities {
    gateway: GatewayInfo {
      address: String::new(),
      name: "preset-resolver".into(),
      os_name: "ios".into(),
      app_name: "preset-resolver".into(),
      app_version: "0.0.0".into(),
      adapter_version: "harness".into(),
      lib_version: "0.0.0".into(),
      libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
    },
    ..Default::default()
  };
  gateway.capabilities().announce(caps).await.expect("announce");
}

struct Companion {
  resolves: bool,
  art: Option<Vec<u8>>,
}

fn serve(gateway: Gateway, companion: Companion) -> tokio::task::JoinHandle<()> {
  let mut events = gateway.events();
  tokio::spawn(async move {
    while let Ok(msg) = events.recv().await {
      match &msg.data {
        BridgeToGatewayMsgData::Library(BridgeToGatewayLibraryMsg::ResolveContext(req)) if companion.resolves => {
          let reply = if req.uri == PLAYLIST {
            ContextResolveReply {
              name: Some("Road Trip".into()),
              artwork_id: Some(ART.into()),
              subtitle: None,
            }
          } else {
            ContextResolveReply {
              name: Some(format!("name of {}", req.uri)),
              artwork_id: Some(format!("gw/img/300/{}", req.uri.replace(':', "-"))),
              subtitle: None,
            }
          };
          let reply = GatewayToBridgeMsgData::Library(GatewayToBridgeLibraryMsg::ContextResolveReply(reply));
          gateway
            .connection()
            .respond(msg.id, reply)
            .await
            .expect("respond resolve");
        }
        BridgeToGatewayMsgData::Asset(BridgeToGatewayAssetMsg::Request(req)) => match &companion.art {
          Some(bytes) => {
            let reply = GatewayToBridgeMsgData::Asset(GatewayToBridgeAssetMsg::Got(AssetGotReply {
              id: req.id.clone(),
              mime: Some("image/jpeg".into()),
              body: TransferBody::Inline(bytes.clone()),
            }));
            gateway
              .connection()
              .respond(msg.id, reply)
              .await
              .expect("respond asset");
          }
          None => {
            gateway
              .connection()
              .respond_err::<AssetRequest>(msg.id, AssetNotFoundReply { id: req.id.clone() })
              .await
              .expect("respond not found");
          }
        },
        _ => {}
      }
    }
  })
}

async fn connect(harness: &Harness, companion: Companion) -> (Gateway, tokio::task::JoinHandle<()>) {
  let gateway = harness.connect_android().await.expect("connect companion");
  announce(&gateway).await;
  let responder = serve(gateway.clone(), companion);
  let ready = harness
    .wait_for(|state| state.gateway_info().is_some(), Duration::from_secs(5))
    .await;
  assert!(ready, "the companion never registered");
  (gateway, responder)
}

async fn call(stock: &mut MockWsClient, msg_id: u64, method: &str, args: Value, timeout: Duration) -> Value {
  let req = serde_json::json!({ "msgId": msg_id, "method": method, "args": args, "userAction": true });
  stock.send_text(req.to_string()).await.expect("send stock request");
  let deadline = tokio::time::Instant::now() + timeout;
  loop {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    assert!(
      !remaining.is_zero(),
      "{method} ({msg_id}) got no reply within {timeout:?}"
    );
    let text = match tokio::time::timeout(remaining, stock.recv()).await {
      Ok(Some(t)) => t,
      Ok(None) => panic!("stock socket closed awaiting {method}"),
      Err(_) => panic!("{method} ({msg_id}) got no reply within {timeout:?}"),
    };
    let Ok(val) = serde_json::from_str::<Value>(&text) else {
      continue;
    };
    if val.get("msgId").and_then(Value::as_u64) == Some(msg_id) {
      return val;
    }
  }
}

async fn set_preset(stock: &mut MockWsClient, msg_id: u64, slot: u8, uri: &str, timeout: Duration) -> Vec<Value> {
  let args = serde_json::json!({
    "presets": [{ "version": 1, "context_uri": uri, "slot_index": slot, "source": "tactile" }]
  });
  let reply = call(stock, msg_id, "com.spotify.superbird.presets.set_preset", args, timeout).await;
  reply["payload"]["result"]
    .as_array()
    .unwrap_or_else(|| panic!("set_preset reply has no result list: {reply}"))
    .clone()
}

async fn graphql_presets(stock: &mut MockWsClient, msg_id: u64) -> Vec<Value> {
  let payload = r#"query{presets(serial:"000000000000"){... on Presets{presets{context_uri:contextUri name image_url:imageUrl slot_index:slotIndex}}}}"#;
  let reply = call(
    stock,
    msg_id,
    "com.spotify.superbird.graphql",
    serde_json::json!({ "payload": payload }),
    REPLY,
  )
  .await;
  reply["payload"]["data"]["presets"]["presets"]
    .as_array()
    .unwrap_or_else(|| panic!("graphql presets reply has no preset list: {reply}"))
    .clone()
}

fn slot(presets: &[Value], index: u64) -> &Value {
  presets
    .iter()
    .find(|p| p["slot_index"].as_u64() == Some(index))
    .unwrap_or_else(|| panic!("slot {index} missing from {presets:?}"))
}

async fn stock_image(stock: &mut MockWsClient, msg_id: u64, id: &str) -> Option<String> {
  let reply = call(
    stock,
    msg_id,
    "com.spotify.get_image",
    serde_json::json!({ "id": id }),
    Duration::from_secs(5),
  )
  .await;
  reply["payload"]["image_data"]
    .as_str()
    .filter(|data| !data.is_empty())
    .map(str::to_string)
}

async fn is_disk_pinned(harness: &Harness, id: &str) -> bool {
  harness.state().assets.is_disk_pinned(id).await.expect("asset cache")
}

async fn wait_pinned(harness: &Harness, id: &str) {
  let deadline = Instant::now() + Duration::from_secs(5);
  while !is_disk_pinned(harness, id).await {
    assert!(Instant::now() < deadline, "{id} never became a disk pin");
    tokio::time::sleep(Duration::from_millis(20)).await;
  }
}

fn art_bytes() -> Vec<u8> {
  (0u8..=255).cycle().take(4096).collect()
}

#[tokio::test]
async fn a_preset_saved_without_a_companion_gets_its_metadata_on_the_next_load() {
  let harness = Harness::start().await.expect("harness start");
  let mut stock = harness.connect_stock_client().await.expect("stock client");

  let saved = set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;
  assert!(
    slot(&saved, 1)["name"].is_null(),
    "nothing could resolve the context yet: {saved:?}"
  );

  let (_companion, _responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: Some(art_bytes()),
    },
  )
  .await;

  let loaded = graphql_presets(&mut stock, 2).await;
  let preset = slot(&loaded, 1);
  assert_eq!(
    preset["name"], "Road Trip",
    "stock loads presets over graphql, so that load has to repair the slot or stock shows it as unavailable forever"
  );
  assert_eq!(preset["image_url"], ART);
}

#[tokio::test]
async fn preset_save_does_not_stall_on_a_companion_that_never_answers() {
  let harness = Harness::start().await.expect("harness start");
  let (_companion, _responder) = connect(
    &harness,
    Companion {
      resolves: false,
      art: None,
    },
  )
  .await;
  let mut stock = harness.connect_stock_client().await.expect("stock client");

  let started = Instant::now();
  let saved = set_preset(&mut stock, 1, 1, PLAYLIST, Duration::from_secs(15)).await;
  assert!(
    started.elapsed() < Duration::from_secs(10),
    "a suspended phone must not hold the save for the full gateway request timeout: took {:?}",
    started.elapsed()
  );
  assert_eq!(slot(&saved, 1)["context_uri"], PLAYLIST, "the slot still saves");
}

#[tokio::test]
async fn preset_art_fetched_from_the_companion_is_pinned_under_the_id_stock_requests() {
  let harness = Harness::start().await.expect("harness start");
  let (companion, responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: Some(art_bytes()),
    },
  )
  .await;
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;

  wait_pinned(&harness, HERO_ART).await;

  responder.abort();
  drop(companion);
  drop(stock);
  let harness = harness.restart().await.expect("restart");
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  let image = stock_image(&mut stock, 2, ART).await;
  assert_eq!(
    image,
    Some(base64::engine::general_purpose::STANDARD.encode(art_bytes())),
    "preset art has to render after a reboot with no phone attached"
  );
}

#[tokio::test]
async fn preset_art_already_cached_in_memory_is_promoted_to_a_disk_pin() {
  let harness = Harness::start().await.expect("harness start");
  harness
    .state()
    .assets
    .insert(
      HERO_ART.into(),
      art_bytes().into(),
      Some("image/jpeg".into()),
      AssetRetention::Lru,
    )
    .await
    .expect("seed memory cache");
  let (companion, responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: None,
    },
  )
  .await;
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;

  wait_pinned(&harness, HERO_ART).await;

  responder.abort();
  drop(companion);
  drop(stock);
  let harness = harness.restart().await.expect("restart");
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  assert!(
    stock_image(&mut stock, 2, ART).await.is_some(),
    "art the cache already held in memory has to land on disk, or a reboot loses it"
  );
}

#[tokio::test]
async fn resaving_the_same_context_keeps_metadata_when_the_companion_is_gone() {
  let harness = Harness::start().await.expect("harness start");
  let (companion, responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: Some(art_bytes()),
    },
  )
  .await;
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;

  responder.abort();
  drop(companion);
  let gone = harness
    .wait_for(|state| state.gateway_info().is_none(), Duration::from_secs(5))
    .await;
  assert!(gone, "the companion never dropped");

  let saved = set_preset(&mut stock, 2, 1, PLAYLIST, REPLY).await;
  let preset = slot(&saved, 1);
  assert_eq!(preset["name"], "Road Trip", "re-saving a slot must never degrade it");
  assert_eq!(preset["image_url"], ART);
}

#[tokio::test]
async fn overwriting_a_preset_releases_its_old_art_pin() {
  let harness = Harness::start().await.expect("harness start");
  let (_companion, _responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: Some(art_bytes()),
    },
  )
  .await;
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;
  wait_pinned(&harness, HERO_ART).await;

  set_preset(&mut stock, 2, 1, "spotify:album:other", REPLY).await;
  wait_pinned(&harness, "gw/img/248/spotify-album-other").await;

  assert!(
    !is_disk_pinned(&harness, HERO_ART).await,
    "art no preset references any more has to become evictable again"
  );
}

#[tokio::test]
async fn art_shared_by_two_presets_stays_pinned_when_one_is_overwritten() {
  let harness = Harness::start().await.expect("harness start");
  let (_companion, _responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: Some(art_bytes()),
    },
  )
  .await;
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;
  set_preset(&mut stock, 2, 2, PLAYLIST, REPLY).await;
  wait_pinned(&harness, HERO_ART).await;

  set_preset(&mut stock, 3, 1, "spotify:album:other", REPLY).await;
  wait_pinned(&harness, "gw/img/248/spotify-album-other").await;

  assert!(is_disk_pinned(&harness, HERO_ART).await, "slot 2 still shows this art");
}

async fn reloaded_within(chrome: &mut tokio::sync::broadcast::Receiver<ChromeCommand>, window: Duration) -> bool {
  let deadline = tokio::time::Instant::now() + window;
  loop {
    match tokio::time::timeout_at(deadline, chrome.recv()).await {
      Ok(Ok(ChromeCommand::Reload)) => return true,
      Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
      Ok(Err(_)) | Err(_) => return false,
    }
  }
}

async fn stock_harness() -> Harness {
  let harness = Harness::start_with_stub_stock_webapp().await.expect("harness start");
  assert_eq!(
    harness.state().active_webapp().await.expect("active webapp"),
    Some(STOCK_WEBAPP_ID),
    "these tests need stock to be the webapp on screen"
  );
  harness
}

#[tokio::test]
async fn a_companion_arriving_reloads_stock_once_it_repairs_an_unavailable_preset() {
  let harness = stock_harness().await;
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;
  let mut chrome = harness.state().chrome.tap();

  let (_companion, _responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: Some(art_bytes()),
    },
  )
  .await;

  assert!(
    reloaded_within(&mut chrome, Duration::from_secs(5)).await,
    "stock only reads presets when it loads, so the repaired slot never shows without a reload"
  );
  let loaded = graphql_presets(&mut stock, 2).await;
  assert_eq!(slot(&loaded, 1)["name"], "Road Trip");
}

#[tokio::test]
async fn a_companion_arriving_leaves_stock_alone_when_no_preset_is_unavailable() {
  let harness = stock_harness().await;
  let (companion, responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: Some(art_bytes()),
    },
  )
  .await;
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;
  responder.abort();
  drop(companion);
  let gone = harness
    .wait_for(|state| state.gateway_info().is_none(), Duration::from_secs(5))
    .await;
  assert!(gone, "the companion never dropped");
  let mut chrome = harness.state().chrome.tap();

  let (_companion, _responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: Some(art_bytes()),
    },
  )
  .await;

  assert!(
    !reloaded_within(&mut chrome, Duration::from_secs(2)).await,
    "a reload costs the user their screen, so it only happens when it fixes something"
  );
}

#[tokio::test]
async fn a_companion_that_cannot_resolve_does_not_reload_stock() {
  let harness = stock_harness().await;
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;
  let mut chrome = harness.state().chrome.tap();

  let (_companion, _responder) = connect(
    &harness,
    Companion {
      resolves: false,
      art: None,
    },
  )
  .await;

  assert!(
    !reloaded_within(&mut chrome, Duration::from_secs(8)).await,
    "nothing was repaired, so a reload would show the same unavailable slots"
  );
}

#[tokio::test]
async fn a_repair_while_another_webapp_is_on_screen_does_not_reload() {
  let harness = Harness::start().await.expect("harness start");
  assert_ne!(
    harness.state().active_webapp().await.expect("active webapp"),
    Some(STOCK_WEBAPP_ID)
  );
  let mut stock = harness.connect_stock_client().await.expect("stock client");
  set_preset(&mut stock, 1, 1, PLAYLIST, REPLY).await;
  let mut chrome = harness.state().chrome.tap();

  let (_companion, _responder) = connect(
    &harness,
    Companion {
      resolves: true,
      art: Some(art_bytes()),
    },
  )
  .await;

  assert!(
    !reloaded_within(&mut chrome, Duration::from_secs(2)).await,
    "reloading would reload whatever webapp is on screen instead of stock"
  );
}
