use std::time::Duration;

use bridgething_gateway::Gateway;
use bridgething_iap2::csm::now_playing::{
  MediaItemAttributes as Iap2MediaItem, NowPlayingUpdate as Iap2NowPlaying, PlaybackAttributes, PlaybackState,
};
use bridgething_test_harness::{Harness, Iap2Source, Iap2SourceDriver, MockWsClient};
use libbridgething::{
  CompanionAuthorityScope, GatewayCapabilities, GatewayInfo, MediaItem, Playback, PlaybackState as WirePlaybackState,
  PlayerState, QueueItem,
  gateway::{AuthorityClaim, QueueSnapshot},
};

const CONVERGE: Duration = Duration::from_secs(5);
const SILENCE_WINDOW: Duration = Duration::from_millis(1200);

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

fn spotify_snapshot(position_ms: u32) -> PlayerState {
  PlayerState {
    track: Some(MediaItem {
      persistent_id: Some("spotify:track:x".into()),
      title: Some("Companion Song".into()),
      artist: Some("Companion Artist".into()),
      album: Some("Companion Album".into()),
      duration_ms: Some(240_000),
      ..MediaItem::default()
    }),
    playback: Playback {
      state: WirePlaybackState::Playing,
      position_ms,
      ..Playback::default()
    },
    ..PlayerState::default()
  }
}

fn iap2_spotify_tick(position_ms: Option<u32>) -> Iap2NowPlaying {
  Iap2NowPlaying {
    media_item: Some(Iap2MediaItem {
      persistent_id: Some(0x1234),
      title: Some("Companion Song".into()),
      ..Default::default()
    }),
    playback: Some(PlaybackAttributes {
      state: Some(PlaybackState::Playing),
      position_ms,
      app_bundle: Some("com.spotify.client".into()),
      ..Default::default()
    }),
  }
}

fn is_player_frame(json: &str) -> bool {
  json.contains("\"type\":\"player\"")
}

fn queue_item(uri: &str, title: &str, artist: &str) -> QueueItem {
  QueueItem {
    uri: uri.into(),
    title: Some(title.into()),
    artist: Some(artist.into()),
    artist_uri: None,
    album: None,
    album_uri: None,
    artwork_id: None,
    duration_ms: Some(200_000),
    persistent_id: None,
    queued: false,
  }
}

async fn companion_owned_harness() -> (Harness, Gateway, MockWsClient) {
  let harness = Harness::start().await.expect("harness start");
  let modern = harness.connect_modern_client().await.expect("modern client");
  let registered = harness.wait_for(|s| s.client_man.client_count() >= 1, CONVERGE).await;
  assert!(registered, "modern client never registered");

  let phone = harness.connect_android().await.expect("companion connect");
  phone.capabilities().announce(caps()).await.expect("announce");
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
  phone
    .player()
    .snapshot(spotify_snapshot(10_000))
    .await
    .expect("snapshot");
  let converged = harness
    .wait_for(
      |s| {
        s.player.state_reply().state.track.and_then(|t| t.title).as_deref() == Some("Companion Song")
          && s.player.companion_playback_authoritative()
      },
      CONVERGE,
    )
    .await;
  assert!(converged, "companion never took now-playing authority");
  (harness, phone, modern)
}

#[tokio::test]
async fn iap2_chatter_during_companion_playback_is_broadcast_silent() {
  let (harness, _phone, _modern) = companion_owned_harness().await;
  let source = harness.iap2_source().await.expect("iap2 source");

  tokio::time::sleep(Duration::from_millis(2_500)).await;
  let before = harness.state().player.state_reply().state.playback.position_ms;

  let mut frames = harness.observe_frames();
  for _ in 0..4 {
    source
      .push_now_playing(iap2_spotify_tick(None))
      .await
      .expect("push chatter");
    tokio::time::sleep(Duration::from_millis(60)).await;
  }

  let player_frames: Vec<String> = frames
    .collect_for(SILENCE_WINDOW)
    .await
    .into_iter()
    .map(|f| f.json().to_string())
    .filter(|j| is_player_frame(j))
    .collect();
  assert!(
    player_frames.is_empty(),
    "staged iap2 chatter leaked {} player broadcast(s): {:?}",
    player_frames.len(),
    player_frames.first()
  );

  let after = harness.state().player.state_reply().state.playback.position_ms;
  assert!(
    after >= before,
    "playhead snapped backward on staged iap2 chatter: {after} < {before}"
  );
}

#[tokio::test]
async fn youtube_flip_is_one_hard_cut_not_a_burst() {
  let (harness, _phone, _modern) = companion_owned_harness().await;
  let source = harness.iap2_source().await.expect("iap2 source");

  let mut frames = harness.observe_frames();
  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0x5678),
        title: Some("YouTube Video".into()),
        duration_ms: Some(600_000),
        ..Default::default()
      }),
      playback: Some(PlaybackAttributes {
        state: Some(PlaybackState::Playing),
        position_ms: Some(5_000),
        app_bundle: Some("com.google.ios.youtube".into()),
        ..Default::default()
      }),
    })
    .await
    .expect("push youtube");

  let cut_frames: Vec<String> = frames
    .collect_for(SILENCE_WINDOW)
    .await
    .into_iter()
    .map(|f| f.json().to_string())
    .filter(|j| is_player_frame(j))
    .collect();
  assert!(
    cut_frames.iter().any(|j| j.contains("YouTube Video")),
    "the hard cut never broadcast the iap2 view"
  );
  assert!(
    cut_frames.len() <= 2,
    "an ownership flip must be one snapshot + queue pair, got {}: {:?}",
    cut_frames.len(),
    cut_frames
  );

  let mut frames = harness.observe_frames();
  for pos in [5_600u32, 6_200, 6_800] {
    tokio::time::sleep(Duration::from_millis(400)).await;
    source
      .push_now_playing(Iap2NowPlaying {
        media_item: None,
        playback: Some(PlaybackAttributes {
          position_ms: Some(pos),
          ..Default::default()
        }),
      })
      .await
      .expect("push tick");
  }
  let tick_frames: Vec<String> = frames
    .collect_for(SILENCE_WINDOW)
    .await
    .into_iter()
    .map(|f| f.json().to_string())
    .filter(|j| is_player_frame(j))
    .collect();
  assert!(
    tick_frames.is_empty(),
    "iap2 position ticks leaked {} broadcast(s)",
    tick_frames.len()
  );
}

#[tokio::test]
async fn context_switch_survives_a_stale_iap2_echo() {
  let (harness, phone, _modern) = companion_owned_harness().await;
  let source = harness.iap2_source().await.expect("iap2 source");

  phone
    .player()
    .queue_changed(QueueSnapshot {
      order: vec!["spotify:track:x".into(), "spotify:track:y".into()],
      items: vec![
        queue_item("spotify:track:x", "Companion Song", "Companion Artist"),
        queue_item("spotify:track:y", "Track Y", "Artist Y"),
      ],
    })
    .await
    .expect("queue");
  source
    .push_now_playing(iap2_spotify_tick(None))
    .await
    .expect("iap2 same-song tick");

  phone
    .player()
    .snapshot(PlayerState {
      track: Some(MediaItem {
        persistent_id: Some("spotify:track:t".into()),
        title: Some("Fresh Pick".into()),
        artist: Some("Fresh Artist".into()),
        duration_ms: Some(210_000),
        ..MediaItem::default()
      }),
      playback: Playback {
        state: WirePlaybackState::Playing,
        position_ms: 0,
        ..Playback::default()
      },
      ..PlayerState::default()
    })
    .await
    .expect("snapshot");
  let switched = harness
    .wait_for(
      |s| s.player.state_reply().state.track.and_then(|t| t.title).as_deref() == Some("Fresh Pick"),
      CONVERGE,
    )
    .await;
  assert!(switched, "the dealer-confirmed context switch never applied");

  source
    .push_now_playing(iap2_spotify_tick(None))
    .await
    .expect("stale echo");
  tokio::time::sleep(Duration::from_millis(400)).await;
  let track = harness.state().player.state_reply().state.track;
  assert_eq!(
    track.as_ref().and_then(|t| t.persistent_id.as_deref()),
    Some("spotify:track:t"),
    "a stale iap2 echo reverted the card to the previous song"
  );
}
