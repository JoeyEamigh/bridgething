use std::time::Duration;

use bridgething_test_harness::Harness;

const SETTLE: Duration = Duration::from_secs(3);

#[tokio::test]
async fn a_newly_connected_webapp_is_told_the_current_volume() {
  let harness = Harness::start().await.expect("harness start");
  let mut client = harness.connect_modern_client().await.expect("connect modern client");

  let mut seen: Vec<String> = Vec::new();
  let deadline = tokio::time::Instant::now() + SETTLE;
  loop {
    let left = deadline.saturating_duration_since(tokio::time::Instant::now());
    assert!(
      !left.is_zero(),
      "volumeChanged only ever fires on a change, so a webapp that connects mid-session \
       has no volume to draw unless the seed carries one; it saw {seen:?}"
    );
    match tokio::time::timeout(left, client.recv()).await {
      Ok(Some(text)) if text.contains("volumeChanged") => return,
      Ok(Some(text)) => seen.push(text),
      Ok(None) => panic!("the client link closed before the seed arrived"),
      Err(_) => {}
    }
  }
}
