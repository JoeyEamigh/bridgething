use std::time::Duration;

use bridgething_gateway::Gateway;
use bridgething_test_harness::Harness;
use libbridgething::{
  CompanionAuthorityScope, GatewayCapabilities, GatewayInfo, MediaItem, Playback, PlaybackState as WirePlaybackState,
  PlayerState, QueueItem,
  gateway::{AuthorityClaim, QueueSnapshot},
};

const CONVERGE: Duration = Duration::from_secs(5);

fn caps() -> GatewayCapabilities {
  GatewayCapabilities {
    gateway: GatewayInfo {
      address: String::new(),
      name: "harness-companion".into(),
      os_name: "android".into(),
      app_name: "harness-companion".into(),
      app_version: "0.0.0".into(),
      adapter_version: "harness".into(),
      lib_version: "0.0.0".into(),
      libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
    },
    ..Default::default()
  }
}

fn queue_item(uri: &str, title: &str) -> QueueItem {
  QueueItem {
    uri: uri.into(),
    title: Some(title.into()),
    artist: Some("Artist".into()),
    artist_uri: None,
    album: None,
    album_uri: None,
    artwork_id: None,
    duration_ms: Some(200_000),
    persistent_id: None,
    queued: false,
  }
}

fn upcoming_snapshot() -> QueueSnapshot {
  QueueSnapshot {
    order: vec!["spotify:track:u1".into(), "spotify:track:u2".into()],
    items: vec![
      queue_item("spotify:track:u1", "Up One"),
      queue_item("spotify:track:u2", "Up Two"),
    ],
  }
}

fn playing_snapshot() -> PlayerState {
  PlayerState {
    track: Some(MediaItem {
      persistent_id: Some("spotify:track:now".into()),
      title: Some("Now Playing".into()),
      artist: Some("Artist".into()),
      duration_ms: Some(240_000),
      ..MediaItem::default()
    }),
    playback: Playback {
      state: WirePlaybackState::Playing,
      position_ms: 5_000,
      ..Playback::default()
    },
    ..PlayerState::default()
  }
}

async fn claim(phone: &Gateway) {
  for scope in [
    CompanionAuthorityScope::NowPlayingMetadata,
    CompanionAuthorityScope::NowPlayingPlayback,
  ] {
    phone
      .authority()
      .claim(AuthorityClaim {
        scope,
        app_bundle: Some("com.spotify.client".into()),
      })
      .await
      .expect("claim");
  }
}

async fn connect_companion(harness: &Harness) -> Gateway {
  let phone = harness.connect_android().await.expect("companion connect");
  phone.capabilities().announce(caps()).await.expect("announce");
  phone
}

async fn assert_queue_visible(harness: &Harness, label: &str) {
  let visible = harness
    .wait_for(
      |s| {
        let q = s.player.queue_reply();
        q.items.iter().map(|i| i.uri.as_str()).collect::<Vec<_>>() == ["spotify:track:u1", "spotify:track:u2"]
      },
      CONVERGE,
    )
    .await;
  let got: Vec<String> = harness
    .state()
    .player
    .queue_reply()
    .items
    .into_iter()
    .map(|i| i.uri)
    .collect();
  assert!(
    visible,
    "[{label}] the held queue never became visible; queue_reply = {got:?}"
  );
}

#[tokio::test]
async fn connect_order_claim_snapshot_queue_serves_the_queue() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_companion(&harness).await;
  claim(&phone).await;
  phone.player().snapshot(playing_snapshot()).await.expect("snapshot");
  phone.player().queue_changed(upcoming_snapshot()).await.expect("queue");
  assert_queue_visible(&harness, "claim-snapshot-queue").await;
}

#[tokio::test]
async fn connect_order_queue_before_claim_serves_the_queue() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_companion(&harness).await;
  phone.player().queue_changed(upcoming_snapshot()).await.expect("queue");
  claim(&phone).await;
  phone.player().snapshot(playing_snapshot()).await.expect("snapshot");
  assert_queue_visible(&harness, "queue-claim-snapshot").await;
}

#[tokio::test]
async fn connect_order_snapshot_queue_claim_serves_the_queue() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_companion(&harness).await;
  phone.player().snapshot(playing_snapshot()).await.expect("snapshot");
  phone.player().queue_changed(upcoming_snapshot()).await.expect("queue");
  claim(&phone).await;
  assert_queue_visible(&harness, "snapshot-queue-claim").await;
}

#[tokio::test]
async fn reconnect_without_a_queue_resend_serves_the_retained_queue() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_companion(&harness).await;
  claim(&phone).await;
  phone.player().snapshot(playing_snapshot()).await.expect("snapshot");
  phone.player().queue_changed(upcoming_snapshot()).await.expect("queue");
  assert_queue_visible(&harness, "pre-blip").await;

  drop(phone);
  let dropped = harness
    .wait_for(|s| !s.player.companion_playback_authoritative(), CONVERGE)
    .await;
  assert!(dropped, "companion disconnect never dropped authority");

  let phone = connect_companion(&harness).await;
  claim(&phone).await;
  phone.player().snapshot(playing_snapshot()).await.expect("snapshot");
  assert_queue_visible(&harness, "post-reconnect").await;
}

#[tokio::test]
async fn connect_broadcasts_the_queue_to_attached_clients() {
  let harness = Harness::start().await.expect("harness start");
  let _modern = harness.connect_modern_client().await.expect("modern client");
  let registered = harness.wait_for(|s| s.client_man.client_count() >= 1, CONVERGE).await;
  assert!(registered, "modern client never registered");

  let mut frames = harness.observe_frames();
  let phone = connect_companion(&harness).await;
  claim(&phone).await;
  phone.player().snapshot(playing_snapshot()).await.expect("snapshot");
  phone.player().queue_changed(upcoming_snapshot()).await.expect("queue");
  assert_queue_visible(&harness, "broadcast").await;

  let saw_queue = frames
    .collect_for(Duration::from_millis(1_200))
    .await
    .into_iter()
    .map(|f| f.json().to_string())
    .any(|j| j.contains("spotify:track:u1"));
  assert!(saw_queue, "no client frame ever carried the upcoming queue");
}

#[tokio::test]
async fn the_queue_head_s_hero_art_is_warmed_before_anyone_asks() {
  let harness = Harness::start().await.expect("harness start");
  let phone = connect_companion(&harness).await;
  claim(&phone).await;
  let mut inbound = phone.events();
  phone.player().snapshot(playing_snapshot()).await.expect("snapshot");
  let mut upcoming = upcoming_snapshot();
  upcoming.items[0].artwork_id = Some("spotify/img/96/iupone".into());
  phone.player().queue_changed(upcoming).await.expect("queue");

  let asked = tokio::time::timeout(CONVERGE, async {
    loop {
      let Ok(msg) = inbound.recv().await else { return None };
      if let libbridgething::gateway::BridgeToGatewayMsgData::Asset(
        libbridgething::gateway::BridgeToGatewayAssetMsg::Request(req),
      ) = msg.data
      {
        return Some(req.id);
      }
    }
  })
  .await
  .ok()
  .flatten();
  assert_eq!(
    asked.as_deref(),
    Some("spotify/img/248/iupone"),
    "the daemon warms the id the now-playing view will ask for, not the queue's thumbnail id"
  );
}
