#![cfg(feature = "chrome")]

use std::time::Duration;

use bridgething_test_harness::Harness;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use libbridgething::OverlayProfile;
use uuid::Uuid;

const HOST_PROBE: &str = "!!document.querySelector('bridgething-overlay')";
const HOST_COUNT: &str = "document.querySelectorAll('bridgething-overlay').length";

fn free_port() -> u16 {
  std::net::TcpListener::bind("127.0.0.1:0")
    .expect("free port probe")
    .local_addr()
    .expect("probe addr")
    .port()
}

fn all_surfaces_off() -> OverlayProfile {
  OverlayProfile {
    notifications: false,
    call: false,
    pairing: false,
    connection: false,
    volume: false,
    voice: false,
  }
}

async fn plant(harness: &Harness, id: Uuid, overlays: Option<OverlayProfile>) {
  let dir = harness.state_dir().join("webapps").join(id.simple().to_string());
  std::fs::create_dir_all(&dir).expect("bundle dir");
  std::fs::write(dir.join("index.html"), b"<html><body><h1>app</h1></body></html>").expect("index");
  let overlays_field = overlays
    .map(|o| format!(r#","overlays":{}"#, serde_json::to_string(&o).expect("overlay json")))
    .unwrap_or_default();
  let manifest = format!(r#"{{"id":"{id}","name":"planted","version":"0.1.0"{overlays_field}}}"#);
  std::fs::write(dir.join("manifest.json"), manifest).expect("manifest");
  harness.state().webapps.rescan().await;
}

async fn first_page(browser: &Browser) -> Page {
  for _ in 0..40 {
    if let Ok(pages) = browser.pages().await
      && let Some(page) = pages.into_iter().next()
    {
      return page;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
  }
  browser.new_page("about:blank").await.expect("initial page")
}

async fn overlay_mounted(page: &Page) -> bool {
  matches!(
    page.evaluate(HOST_PROBE).await.map(|r| r.into_value::<bool>()),
    Ok(Ok(true))
  )
}

async fn settle(page: &Page, url: &str, want_mounted: bool) -> bool {
  for _ in 0..200 {
    if page.goto(url).await.is_err() {
      tokio::time::sleep(Duration::from_millis(250)).await;
      continue;
    }
    let _ = page.wait_for_navigation().await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    if overlay_mounted(page).await == want_mounted {
      return true;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
  }
  false
}

#[tokio::test]
async fn t2_overlay_injection_follows_the_active_manifest() {
  let port = free_port();
  unsafe { std::env::set_var("BRIDGETHING_CHROME_PORT", port.to_string()) };

  let config = match BrowserConfig::builder()
    .no_sandbox()
    .window_size(800, 480)
    .port(port)
    .arg("--disable-dev-shm-usage")
    .build()
  {
    Ok(c) => c,
    Err(err) => {
      eprintln!("skipping overlay T2: chromium config unavailable ({err})");
      return;
    }
  };
  let (browser, mut events) = match Browser::launch(config).await {
    Ok(pair) => pair,
    Err(err) => {
      eprintln!("skipping overlay T2: chromium unavailable ({err})");
      return;
    }
  };
  let handler = tokio::spawn(async move {
    while let Some(event) = events.next().await {
      if event.is_err() {
        break;
      }
    }
  });

  let page = first_page(&browser).await;

  let harness = Harness::start().await.expect("harness start");
  let url = format!("http://{}/", harness.modern_addr());

  let on_id = Uuid::now_v7();
  let off_id = Uuid::now_v7();
  plant(&harness, on_id, None).await;
  plant(&harness, off_id, Some(all_surfaces_off())).await;

  harness.state().set_active_webapp(on_id).await.expect("activate on-app");
  assert!(
    settle(&page, &url, true).await,
    "overlay host never mounted for a default-manifest webapp"
  );

  harness.state().sync_overlay(true).await;
  tokio::time::sleep(Duration::from_secs(1)).await;
  let count: i64 = page
    .evaluate(HOST_COUNT)
    .await
    .expect("count eval")
    .into_value()
    .expect("count value");
  assert_eq!(count, 1, "run_immediately re-install must not double-mount");

  harness
    .state()
    .set_active_webapp(off_id)
    .await
    .expect("activate off-app");
  assert!(
    settle(&page, &url, false).await,
    "overlay host still mounts for an all-off manifest"
  );

  handler.abort();
}
