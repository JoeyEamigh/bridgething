use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_client::Client;
use bridgething_gateway::Gateway;
use bridgething_test_harness::Harness;
use libbridgething::{
  BrowseResult, GatewayCapabilities, GatewayInfo, MusicProvider,
  client::{BridgeToClientLibraryMsg, BridgeToClientMsgData, LibraryBrowse},
  gateway::{BridgeToGatewayLibraryMsg, BridgeToGatewayMsgData, BrowseChanged, BrowseReply, LibraryBrowseRequest},
};

const CONVERGE: Duration = Duration::from_secs(5);
const ALBUM_A: &str = "spotify:album:a";
const ALBUM_B: &str = "spotify:album:b";

type Asked = Arc<Mutex<HashMap<(Option<String>, u32), u32>>>;

fn caps() -> GatewayCapabilities {
  GatewayCapabilities {
    gateway: GatewayInfo {
      address: String::new(),
      name: "harness-phone".into(),
      os_name: "android".into(),
      app_name: "harness-phone".into(),
      app_version: "0.0.0".into(),
      adapter_version: "harness".into(),
      lib_version: "0.0.0".into(),
      libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
    },
    music_provider: MusicProvider::Spotify,
    ..Default::default()
  }
}

fn serve_browse(phone: &Gateway) -> Asked {
  let asked: Asked = Arc::default();
  let counted = asked.clone();
  let mut events = phone.events();
  let serving = phone.clone();
  tokio::spawn(async move {
    loop {
      let msg = match events.recv().await {
        Ok(msg) => msg,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
      };
      let BridgeToGatewayMsgData::Library(BridgeToGatewayLibraryMsg::Browse(request)) = &msg.data else {
        continue;
      };
      let times = {
        let mut asked = counted.lock().unwrap();
        let times = asked.entry((request.node_id.clone(), request.offset)).or_default();
        *times += 1;
        *times
      };
      let _ = serving
        .connection()
        .respond_to::<LibraryBrowseRequest>(
          msg.id,
          BrowseReply {
            result: BrowseResult {
              entries: Vec::new(),
              total: Some(times),
              has_more: false,
            },
          },
        )
        .await;
    }
  });
  asked
}

fn page(node_id: Option<&str>, offset: u32) -> LibraryBrowse {
  LibraryBrowse {
    node_id: node_id.map(str::to_owned),
    limit: 10,
    offset,
    sections: None,
    preview: None,
  }
}

async fn browse(client: &Client, node_id: Option<&str>, offset: u32) -> Option<u32> {
  client
    .request(page(node_id, offset))
    .await
    .expect("the browse resolved")
    .result
    .total
}

async fn connected() -> (Harness, Gateway, Asked, Client) {
  let harness = Harness::start().await.expect("harness start");
  let phone = harness.connect_android().await.expect("phone connect");
  let asked = serve_browse(&phone);
  phone.capabilities().announce(caps()).await.expect("phone announce");
  let client = harness.connect_command_client().await.expect("a webapp connects");
  let deadline = tokio::time::Instant::now() + CONVERGE;
  while client.request(page(Some("warmup"), 0)).await.is_err() {
    assert!(
      tokio::time::Instant::now() < deadline,
      "the daemon never picked up the companion"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
  }
  (harness, phone, asked, client)
}

fn asked(asked: &Asked, node_id: Option<&str>, offset: u32) -> u32 {
  asked
    .lock()
    .unwrap()
    .get(&(node_id.map(str::to_owned), offset))
    .copied()
    .unwrap_or_default()
}

fn wire_page(node_id: Option<&str>, offset: u32) -> LibraryBrowseRequest {
  let page = page(node_id, offset);
  LibraryBrowseRequest {
    node_id: page.node_id,
    limit: page.limit,
    offset: page.offset,
    sections: page.sections,
    preview: page.preview,
  }
}

#[tokio::test]
async fn a_changed_page_from_the_phone_drops_exactly_that_page_and_tells_the_webapps() {
  let (_harness, phone, served, client) = connected().await;
  let mut inbound = client.events();
  for _ in 0..2 {
    assert_eq!(browse(&client, Some(ALBUM_A), 0).await, Some(1));
    assert_eq!(browse(&client, Some(ALBUM_A), 10).await, Some(1));
    assert_eq!(browse(&client, Some(ALBUM_B), 0).await, Some(1));
    assert_eq!(browse(&client, None, 0).await, Some(1));
  }

  phone
    .library()
    .browse_changed(BrowseChanged {
      page: wire_page(Some(ALBUM_A), 10),
    })
    .await
    .expect("the change reached the daemon");

  let told = tokio::time::timeout(CONVERGE, async {
    loop {
      let msg = inbound.recv().await.expect("the client link is alive");
      if let BridgeToClientMsgData::Library(BridgeToClientLibraryMsg::BrowseChanged(changed)) = msg.data {
        return changed;
      }
    }
  })
  .await
  .expect("webapps hear which node changed");
  assert_eq!(told.node_id.as_deref(), Some(ALBUM_A));

  assert_eq!(
    browse(&client, Some(ALBUM_A), 10).await,
    Some(2),
    "the named page went back to the phone"
  );
  assert_eq!(browse(&client, Some(ALBUM_A), 0).await, Some(1));
  assert_eq!(browse(&client, Some(ALBUM_B), 0).await, Some(1));
  assert_eq!(browse(&client, None, 0).await, Some(1));
  assert_eq!(asked(&served, Some(ALBUM_A), 10), 2);
  assert_eq!(asked(&served, Some(ALBUM_A), 0), 1);
  assert_eq!(asked(&served, Some(ALBUM_B), 0), 1);
  assert_eq!(asked(&served, None, 0), 1);
}

#[tokio::test]
async fn a_changed_root_from_the_phone_drops_the_cached_home_and_nothing_else() {
  let (_harness, phone, served, client) = connected().await;
  for _ in 0..2 {
    assert_eq!(browse(&client, None, 0).await, Some(1));
    assert_eq!(browse(&client, Some(ALBUM_A), 0).await, Some(1));
  }

  phone
    .library()
    .browse_changed(BrowseChanged {
      page: wire_page(None, 0),
    })
    .await
    .expect("the change reached the daemon");

  let deadline = tokio::time::Instant::now() + CONVERGE;
  while browse(&client, None, 0).await != Some(2) {
    assert!(
      tokio::time::Instant::now() < deadline,
      "the cached home never went back to the phone"
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
  }
  assert_eq!(browse(&client, Some(ALBUM_A), 0).await, Some(1));
  assert_eq!(asked(&served, None, 0), 2);
  assert_eq!(asked(&served, Some(ALBUM_A), 0), 1);
}
