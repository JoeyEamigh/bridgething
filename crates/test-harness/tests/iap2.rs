use std::time::Duration;

use bridgething::{ClientMode, LeCommand};
use bridgething_iap2::{
  Lsp, SessionEvent,
  csm::now_playing::{MediaItemAttributes, NowPlayingUpdate, PlaybackAttributes, PlaybackState},
};
use bridgething_test_harness::{Harness, extract_substring_starting_with};
use libbridgething::{
  CompanionAuthorityScope, GatewayCapabilities, GatewayInfo, MediaItem, Playback, PlayerState, gateway::AuthorityClaim,
};

const CONVERGE: Duration = Duration::from_secs(3);

fn companion_caps() -> GatewayCapabilities {
  GatewayCapabilities {
    gateway: GatewayInfo {
      address: String::new(),
      name: "harness-companion".into(),
      os_name: "ios".into(),
      app_name: "harness-companion".into(),
      app_version: "0.0.0".into(),
      adapter_version: "harness".into(),
      lib_version: "0.0.0".into(),
      libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
    },
    ..Default::default()
  }
}

#[tokio::test]
async fn iap2_single_source_now_playing() {
  let harness = Harness::start().await.expect("harness start");
  let phone = harness.iap2_peer();

  harness
    .inject_iap2(
      phone,
      SessionEvent::NowPlayingUpdate(NowPlayingUpdate {
        media_item: Some(MediaItemAttributes {
          persistent_id: Some(0x1234),
          title: Some("iAP2 Song".into()),
          artist: Some("iAP2 Artist".into()),
          ..Default::default()
        }),
        playback: None,
      }),
    )
    .await
    .expect("inject iap2 now-playing");

  let converged = harness
    .wait_for(
      |state| state.player.state_reply().state.track.and_then(|t| t.title).as_deref() == Some("iAP2 Song"),
      CONVERGE,
    )
    .await;
  assert!(converged, "merged title never reflected injected iAP2 data");

  let track = harness.state().player.state_reply().state.track.expect("track present");
  assert_eq!(track.artist.as_deref(), Some("iAP2 Artist"));
}

#[tokio::test]
async fn iap2_artwork_resolves_and_appears_in_broadcast() {
  let harness = Harness::start().await.expect("harness start");
  let mut frames = harness.observe_frames();
  let _client = harness.connect_modern_client().await.expect("connect modern client");

  let registered = harness
    .wait_for(|state| state.client_man.client_count() >= 1, CONVERGE)
    .await;
  assert!(registered, "modern client never registered");

  let phone = harness.iap2_peer();
  harness
    .inject_iap2(
      phone,
      SessionEvent::NowPlayingUpdate(NowPlayingUpdate {
        media_item: Some(MediaItemAttributes {
          persistent_id: Some(0x1234),
          title: Some("Art Song".into()),
          artwork_id: Some(7),
          ..Default::default()
        }),
        playback: None,
      }),
    )
    .await
    .expect("inject now-playing");
  harness
    .iap2_artwork(phone, 7, vec![0xFF; 64])
    .await
    .expect("inject artwork");

  let art_prefix = "iap2/art/0000000000001234/";

  let observed = frames
    .wait_for(CONVERGE, |f| {
      f.mode == ClientMode::Modern && f.json().contains(art_prefix)
    })
    .await;
  let frame = observed.expect("no modern broadcast carried the art id");
  let art_id = extract_substring_starting_with(frame.json(), art_prefix).expect("frame had no art id");

  let cached = harness.state().assets.get(&art_id).await.expect("asset get");
  assert!(
    cached.is_some(),
    "iap2 artwork bytes never resolved into the asset cache"
  );
}

#[tokio::test]
async fn iap2_zero_byte_artwork_does_not_strand_the_retransmit() {
  let harness = Harness::start().await.expect("harness start");
  let phone = harness.iap2_peer();

  harness
    .inject_iap2(
      phone,
      SessionEvent::NowPlayingUpdate(NowPlayingUpdate {
        media_item: Some(MediaItemAttributes {
          persistent_id: Some(0x1234),
          title: Some("Art Song".into()),
          artwork_id: Some(7),
          ..Default::default()
        }),
        playback: None,
      }),
    )
    .await
    .expect("inject now-playing");

  harness
    .iap2_artwork(phone, 7, Vec::new())
    .await
    .expect("inject 0-byte artwork");

  harness
    .iap2_artwork(phone, 7, vec![0xFF; 64])
    .await
    .expect("inject real artwork retransmit");

  let deadline = std::time::Instant::now() + CONVERGE;
  let landed = loop {
    if let Some(art_id) = harness
      .state()
      .player
      .state_reply()
      .state
      .track
      .and_then(|t| t.artwork_id)
      && let Some(asset) = harness.state().assets.get(&art_id).await.expect("asset get")
    {
      break Some(asset);
    }
    if std::time::Instant::now() >= deadline {
      break None;
    }
    tokio::time::sleep(Duration::from_millis(25)).await;
  };

  let asset = landed.expect("real artwork retransmit was dropped after a 0-byte placeholder");
  assert_eq!(
    &asset.bytes[..],
    &[0xFF; 64][..],
    "cached bytes are the real retransmit"
  );
}

#[tokio::test]
async fn companion_snapshot_authoritative_and_iap2_app_change_gate() {
  let harness = Harness::start().await.expect("harness start");
  let phone_addr = harness.iap2_peer();
  let companion = harness.connect_android().await.expect("connect companion");
  companion
    .capabilities()
    .announce(companion_caps())
    .await
    .expect("announce");

  for scope in [
    CompanionAuthorityScope::NowPlayingMetadata,
    CompanionAuthorityScope::NowPlayingPlayback,
  ] {
    companion
      .authority()
      .claim(AuthorityClaim {
        scope,
        app_bundle: Some("com.spotify.client".into()),
      })
      .await
      .expect("claim now-playing authority");
  }

  companion
    .player()
    .snapshot(PlayerState {
      track: Some(MediaItem {
        uri: Some("spotify:track:gold".into()),
        persistent_id: Some("spotify:track:gold".into()),
        title: Some("Spotify Song".into()),
        ..Default::default()
      }),
      playback: Playback {
        state: libbridgething::PlaybackState::Playing,
        ..Default::default()
      },
      ..Default::default()
    })
    .await
    .expect("send player snapshot");

  let companion_owns = harness
    .wait_for(
      |state| state.player.state_reply().state.track.and_then(|t| t.uri).as_deref() == Some("spotify:track:gold"),
      CONVERGE,
    )
    .await;
  assert!(companion_owns, "companion snapshot never reached merged now-playing");
  assert_eq!(
    harness
      .state()
      .player
      .state_reply()
      .state
      .track
      .and_then(|t| t.title)
      .as_deref(),
    Some("Spotify Song"),
    "companion title authoritative"
  );

  harness
    .inject_iap2(
      phone_addr,
      SessionEvent::NowPlayingUpdate(NowPlayingUpdate {
        media_item: Some(MediaItemAttributes {
          persistent_id: Some(0xBEEF),
          title: Some("YouTube Video".into()),
          ..Default::default()
        }),
        playback: Some(PlaybackAttributes {
          state: Some(PlaybackState::Playing),
          app_bundle: Some("com.google.ios.youtube".into()),
          ..Default::default()
        }),
      }),
    )
    .await
    .expect("inject youtube iap2 update");

  let iap2_takes = harness
    .wait_for(
      |state| state.player.state_reply().state.track.and_then(|t| t.title).as_deref() == Some("YouTube Video"),
      CONVERGE,
    )
    .await;
  assert!(
    iap2_takes,
    "diverging iAP2 foreground app did not hand now-playing to iAP2"
  );

  harness
    .inject_iap2(
      phone_addr,
      SessionEvent::NowPlayingUpdate(NowPlayingUpdate {
        media_item: Some(MediaItemAttributes {
          persistent_id: Some(0x1234),
          title: Some("Spotify From iAP2".into()),
          ..Default::default()
        }),
        playback: Some(PlaybackAttributes {
          state: Some(PlaybackState::Playing),
          app_bundle: Some("com.spotify.client".into()),
          ..Default::default()
        }),
      }),
    )
    .await
    .expect("inject spotify-bundle iap2 update");

  let companion_retakes = harness
    .wait_for(
      |state| state.player.state_reply().state.track.and_then(|t| t.uri).as_deref() == Some("spotify:track:gold"),
      CONVERGE,
    )
    .await;
  assert!(
    companion_retakes,
    "companion did not re-take now-playing when iAP2 returned to the spotify bundle"
  );
}

#[tokio::test]
async fn iap2_playback_only_deltas_reach_webapp_without_companion() {
  let harness = Harness::start().await.expect("harness start");
  let mut frames = harness.observe_frames();
  let _client = harness.connect_modern_client().await.expect("connect modern client");
  let registered = harness
    .wait_for(|state| state.client_man.client_count() >= 1, CONVERGE)
    .await;
  assert!(registered, "modern client never registered");

  let phone = harness.iap2_peer();

  harness
    .inject_iap2(
      phone,
      SessionEvent::NowPlayingUpdate(NowPlayingUpdate {
        media_item: Some(MediaItemAttributes {
          persistent_id: Some(0x1234),
          title: Some("Video".into()),
          duration_ms: Some(200_000),
          ..Default::default()
        }),
        playback: Some(PlaybackAttributes {
          state: Some(PlaybackState::Paused),
          position_ms: Some(0),
          ..Default::default()
        }),
      }),
    )
    .await
    .expect("inject metadata");
  let started = frames
    .wait_for(CONVERGE, |f| f.mode == ClientMode::Modern && f.json().contains("Video"))
    .await;
  assert!(started.is_some(), "metadata never reached a modern frame");

  harness
    .inject_iap2(
      phone,
      SessionEvent::NowPlayingUpdate(NowPlayingUpdate {
        media_item: None,
        playback: Some(PlaybackAttributes {
          state: Some(PlaybackState::Playing),
          position_ms: Some(30_000),
          ..Default::default()
        }),
      }),
    )
    .await
    .expect("inject play delta");
  let playing = frames
    .wait_for(CONVERGE, |f| {
      f.mode == ClientMode::Modern && f.json().contains("\"state\":\"playing\"")
    })
    .await;
  assert!(
    playing.is_some(),
    "iAP2 play-state delta never reached the webapp (transport looks dead)"
  );

  harness
    .inject_iap2(
      phone,
      SessionEvent::NowPlayingUpdate(NowPlayingUpdate {
        media_item: None,
        playback: Some(PlaybackAttributes {
          state: Some(PlaybackState::Paused),
          position_ms: Some(45_000),
          ..Default::default()
        }),
      }),
    )
    .await
    .expect("inject pause delta");
  let paused = frames
    .wait_for(CONVERGE, |f| {
      f.mode == ClientMode::Modern && f.json().contains("\"state\":\"paused\"")
    })
    .await;
  assert!(paused.is_some(), "iAP2 pause delta never reached the webapp");
}

#[tokio::test]
async fn an_iap2_restart_in_place_leaves_the_le_link_alone() {
  let harness = Harness::start().await.expect("harness start");
  let phone = harness.iap2_peer();
  let mut le = harness.observe_le();

  harness
    .inject_iap2(phone, SessionEvent::LinkEstablished(Lsp::accessory_default()))
    .await
    .expect("inject link established");
  assert!(
    le.wait_for(CONVERGE, |cmd| *cmd == LeCommand::Attach { address: phone })
      .await
      .is_some(),
    "an established link attaches the LE session"
  );

  for _ in 0..3 {
    harness
      .inject_iap2(phone, SessionEvent::LinkRestarting("peer RST".into()))
      .await
      .expect("inject link restarting");
    harness
      .inject_iap2(phone, SessionEvent::LinkEstablished(Lsp::accessory_default()))
      .await
      .expect("inject link established");
  }
  let during_restarts = le.collect_for(Duration::from_millis(500)).await;
  assert!(
    !during_restarts
      .iter()
      .any(|cmd| matches!(cmd, LeCommand::Detach { .. })),
    "an in-place iAP2 restart must never release the LE session: {during_restarts:?}"
  );

  harness
    .inject_iap2(phone, SessionEvent::LinkDown("restart failed".into()))
    .await
    .expect("inject link down");
  assert!(
    le.wait_for(CONVERGE, |cmd| *cmd == LeCommand::Detach { address: phone })
      .await
      .is_some(),
    "a real link down releases the LE session"
  );
}
