use std::{collections::HashSet, time::Duration};

use bridgething::{Address, State};
use bridgething_gateway::Gateway;
use bridgething_iap2::{
  SessionEvent,
  csm::now_playing::{
    MediaItemAttributes, NowPlayingUpdate as Iap2NowPlaying, PlaybackAttributes, PlaybackState, RepeatMode, ShuffleMode,
  },
};
use bridgething_test_harness::{
  Harness,
  model::{Model, ModelEvent, Projection},
  proptest_cases,
};
use libbridgething::{
  CompanionAuthorityScope, GatewayCapabilities, GatewayInfo, MediaItem, Playback, PlayerOptions, PlayerState,
  gateway::{AuthorityClaim, AuthorityRelease},
};
use proptest::prelude::*;

const CONVERGE: Duration = Duration::from_millis(500);
const STABLE_WINDOW: Duration = Duration::from_millis(20);

fn daemon_projection(state: &State) -> Projection {
  Projection::from_daemon(
    &state.player.state_reply(),
    &state.player.queue_reply(),
    state.player.current_artwork_id(),
  )
}

fn daemon_authority(state: &State) -> HashSet<CompanionAuthorityScope> {
  state.authority.live_scopes().into_iter().collect()
}

fn projection_matches(
  state: &State,
  expected: &Projection,
  expected_authority: &HashSet<CompanionAuthorityScope>,
) -> bool {
  daemon_projection(state) == *expected && daemon_authority(state) == *expected_authority
}

async fn converge<T>(
  harness: &Harness,
  player_watch: &mut tokio::sync::watch::Receiver<T>,
  expected: &Projection,
  expected_authority: &HashSet<CompanionAuthorityScope>,
) -> bool {
  let deadline = tokio::time::Instant::now() + CONVERGE;
  loop {
    if projection_matches(harness.state(), expected, expected_authority) {
      player_watch.mark_unchanged();
      match tokio::time::timeout(STABLE_WINDOW, player_watch.changed()).await {
        Err(_) | Ok(Err(_)) => {
          if projection_matches(harness.state(), expected, expected_authority) {
            return true;
          }
        }
        Ok(Ok(())) => {}
      }
    } else {
      let _ = tokio::time::timeout(Duration::from_millis(10), player_watch.changed()).await;
    }
    if tokio::time::Instant::now() >= deadline {
      return projection_matches(harness.state(), expected, expected_authority);
    }
  }
}

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

async fn apply_to_daemon(harness: &Harness, addr: Address, phone: &Gateway, event: &ModelEvent) -> anyhow::Result<()> {
  match event {
    ModelEvent::Iap2NowPlaying(update) => {
      harness
        .inject_iap2(addr, SessionEvent::NowPlayingUpdate(update.clone()))
        .await
    }
    ModelEvent::Iap2Artwork { transfer_id, bytes_len } => {
      harness.iap2_artwork(addr, *transfer_id, vec![0u8; *bytes_len]).await
    }
    ModelEvent::CompanionSnapshot(snap) => phone
      .player()
      .snapshot(snap.clone())
      .await
      .map_err(|e| anyhow::anyhow!("companion snapshot: {e}")),
    ModelEvent::AuthorityClaim(scope) => phone
      .authority()
      .claim(AuthorityClaim {
        scope: *scope,
        app_bundle: None,
      })
      .await
      .map_err(|e| anyhow::anyhow!("authority claim: {e}")),
    ModelEvent::AuthorityRelease(scope) => phone
      .authority()
      .release(AuthorityRelease { scope: *scope })
      .await
      .map_err(|e| anyhow::anyhow!("authority release: {e}")),
  }
}

fn pid_strategy() -> impl Strategy<Value = Option<u64>> {
  prop_oneof![
    Just(None),
    Just(Some(0u64)),
    Just(Some(1)),
    Just(Some(2)),
    Just(Some(3))
  ]
}

fn title_strategy() -> impl Strategy<Value = Option<String>> {
  prop_oneof![
    Just(None),
    Just(Some(String::new())),
    Just(Some("Alpha".to_string())),
    Just(Some("Beta".to_string())),
  ]
}

fn name_strategy() -> impl Strategy<Value = Option<String>> {
  prop_oneof![Just(None), Just(Some("X".to_string())), Just(Some("Y".to_string()))]
}

fn opt_transfer_id() -> impl Strategy<Value = Option<u8>> {
  prop_oneof![Just(None), (1u8..=3).prop_map(Some)]
}

fn opt_duration() -> impl Strategy<Value = Option<u32>> {
  prop_oneof![Just(None), Just(Some(120_000u32)), Just(Some(240_000))]
}

fn opt_bool() -> impl Strategy<Value = Option<bool>> {
  prop_oneof![Just(None), Just(Some(true)), Just(Some(false))]
}

fn media_strategy() -> impl Strategy<Value = Option<MediaItemAttributes>> {
  prop_oneof![
    1 => Just(None),
    6 => (
      pid_strategy(),
      title_strategy(),
      opt_transfer_id(),
      opt_duration(),
      opt_bool(),
      name_strategy(),
      name_strategy(),
    )
      .prop_map(|(persistent_id, title, artwork_id, duration_ms, liked, album, artist)| {
        Some(MediaItemAttributes {
          persistent_id,
          title,
          artwork_id,
          duration_ms,
          liked,
          album,
          artist,
          ..Default::default()
        })
      }),
  ]
}

fn opt_state() -> impl Strategy<Value = Option<PlaybackState>> {
  prop_oneof![
    Just(None),
    Just(Some(PlaybackState::Playing)),
    Just(Some(PlaybackState::Paused)),
  ]
}

fn opt_shuffle() -> impl Strategy<Value = Option<ShuffleMode>> {
  prop_oneof![Just(None), Just(Some(ShuffleMode::Off)), Just(Some(ShuffleMode::Songs)),]
}

fn opt_repeat() -> impl Strategy<Value = Option<RepeatMode>> {
  prop_oneof![
    Just(None),
    Just(Some(RepeatMode::Off)),
    Just(Some(RepeatMode::Track)),
    Just(Some(RepeatMode::All)),
  ]
}

fn playback_strategy() -> impl Strategy<Value = Option<PlaybackAttributes>> {
  prop_oneof![
    1 => Just(None),
    4 => (opt_state(), opt_shuffle(), opt_repeat(), opt_bool()).prop_map(
      |(state, shuffle_mode, repeat, set_elapsed_time_available)| {
        Some(PlaybackAttributes {
          state,
          shuffle_mode,
          repeat,
          set_elapsed_time_available,
          ..Default::default()
        })
      }
    ),
  ]
}

fn companion_pid() -> impl Strategy<Value = Option<String>> {
  prop_oneof![
    Just(Some("spotify:1".to_string())),
    Just(Some("spotify:2".to_string())),
    Just(Some("spotify:3".to_string())),
  ]
}

fn companion_snapshot_strategy() -> impl Strategy<Value = PlayerState> {
  let track = prop_oneof![
    1 => Just(None),
    5 => (companion_pid(), title_strategy(), opt_duration(), opt_bool(), name_strategy()).prop_map(
      |(persistent_id, title, duration_ms, liked, album)| {
        Some(MediaItem {
          persistent_id,
          title,
          duration_ms,
          liked,
          album,
          ..Default::default()
        })
      }
    ),
  ];
  let playback = (opt_state(), opt_shuffle(), opt_repeat(), opt_bool()).prop_map(
    |(state, shuffle_mode, repeat, set_elapsed_time_available)| Playback {
      state: match state {
        Some(PlaybackState::Playing) => libbridgething::PlaybackState::Playing,
        _ => libbridgething::PlaybackState::Paused,
      },
      shuffle: shuffle_mode.is_some_and(|m| m.is_on()),
      shuffle_mode: shuffle_mode.map(translate_shuffle),
      repeat: repeat.map(translate_repeat).unwrap_or(libbridgething::RepeatMode::Off),
      set_elapsed_time_available,
      ..Default::default()
    },
  );
  (track, playback).prop_map(|(track, playback)| PlayerState {
    track,
    playback,
    options: PlayerOptions {
      speed: 1.0,
      crossfade_ms: None,
    },
    ..Default::default()
  })
}

fn scope_strategy() -> impl Strategy<Value = CompanionAuthorityScope> {
  prop_oneof![
    Just(CompanionAuthorityScope::NowPlayingMetadata),
    Just(CompanionAuthorityScope::NowPlayingPlayback),
  ]
}

fn translate_shuffle(mode: ShuffleMode) -> libbridgething::ShuffleMode {
  match mode {
    ShuffleMode::Off => libbridgething::ShuffleMode::Off,
    ShuffleMode::Songs => libbridgething::ShuffleMode::Songs,
    ShuffleMode::Albums => libbridgething::ShuffleMode::Albums,
  }
}

fn translate_repeat(mode: RepeatMode) -> libbridgething::RepeatMode {
  match mode {
    RepeatMode::Off => libbridgething::RepeatMode::Off,
    RepeatMode::Track => libbridgething::RepeatMode::One,
    RepeatMode::All => libbridgething::RepeatMode::All,
  }
}

fn event_strategy() -> impl Strategy<Value = ModelEvent> {
  prop_oneof![
    4 => (media_strategy(), playback_strategy())
      .prop_map(|(media_item, playback)| ModelEvent::Iap2NowPlaying(Iap2NowPlaying { media_item, playback })),
    2 => (1u8..=3).prop_map(|transfer_id| ModelEvent::Iap2Artwork { transfer_id, bytes_len: 64 }),
    3 => companion_snapshot_strategy().prop_map(ModelEvent::CompanionSnapshot),
    2 => scope_strategy().prop_map(ModelEvent::AuthorityClaim),
    1 => scope_strategy().prop_map(ModelEvent::AuthorityRelease),
  ]
}

fn events_strategy() -> impl Strategy<Value = Vec<ModelEvent>> {
  prop::collection::vec(event_strategy(), 1..=18)
}

fn chaos_iap2_pid() -> impl Strategy<Value = Option<u64>> {
  prop_oneof![Just(None), Just(Some(0u64)), Just(Some(1)), Just(Some(2))]
}

fn chaos_iap2_title() -> impl Strategy<Value = Option<String>> {
  prop_oneof![
    Just(None),
    Just(Some(String::new())),
    Just(Some("Real".to_string())),
    Just(Some("Beta".to_string())),
  ]
}

fn chaos_event_strategy() -> impl Strategy<Value = ModelEvent> {
  let iap2_media = prop_oneof![
    1 => Just(None),
    5 => (chaos_iap2_pid(), chaos_iap2_title(), opt_transfer_id(), opt_duration()).prop_map(
      |(persistent_id, title, artwork_id, duration_ms)| Some(MediaItemAttributes {
        persistent_id,
        title,
        artwork_id,
        duration_ms,
        ..Default::default()
      })
    ),
  ];
  prop_oneof![
    4 => (iap2_media, playback_strategy())
      .prop_map(|(media_item, playback)| ModelEvent::Iap2NowPlaying(Iap2NowPlaying { media_item, playback })),
    2 => (0u8..=3).prop_map(|transfer_id| ModelEvent::Iap2Artwork { transfer_id, bytes_len: 64 }),
    3 => companion_snapshot_strategy().prop_map(ModelEvent::CompanionSnapshot),
    2 => scope_strategy().prop_map(ModelEvent::AuthorityClaim),
    1 => scope_strategy().prop_map(ModelEvent::AuthorityRelease),
  ]
}

fn chaos_events_strategy() -> impl Strategy<Value = Vec<ModelEvent>> {
  prop::collection::vec(chaos_event_strategy(), 4..=24)
}

async fn quiesce<T>(player_watch: &mut tokio::sync::watch::Receiver<T>) {
  let deadline = tokio::time::Instant::now() + CONVERGE;
  loop {
    player_watch.mark_unchanged();
    match tokio::time::timeout(STABLE_WINDOW, player_watch.changed()).await {
      Err(_) | Ok(Err(_)) => return,
      Ok(Ok(())) => {}
    }
    if tokio::time::Instant::now() >= deadline {
      return;
    }
  }
}

fn liveness_npu() -> Iap2NowPlaying {
  Iap2NowPlaying {
    media_item: Some(MediaItemAttributes {
      persistent_id: Some(0xABCD),
      title: Some("liveness".to_string()),
      ..Default::default()
    }),
    playback: None,
  }
}

proptest! {
  #![proptest_config(ProptestConfig { cases: proptest_cases(128), ..ProptestConfig::default() })]

  #[test]
  fn merge_matches_model(events in events_strategy()) {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .worker_threads(2)
      .enable_all()
      .build()
      .unwrap();

    rt.block_on(async move {
      let harness = Harness::start().await.expect("harness start");
      let addr = harness.iap2_peer();
      let phone = harness.connect_android().await.expect("connect companion");
      phone.capabilities().announce(caps()).await.expect("announce");
      let mut player_watch = harness.state().player.snapshot_watch();
      let mut model = Model::new();

      for (i, event) in events.iter().enumerate() {
        apply_to_daemon(&harness, addr, &phone, event).await.expect("inject");
        model.apply(event);
        let expected = model.project();
        let expected_authority = model.authority_scopes();

        let converged = converge(&harness, &mut player_watch, &expected, &expected_authority).await;

        prop_assert!(
          converged,
          "divergence after event {i} ({event:?})\n  expected (model): {expected:#?}\n  actual (daemon):  {:#?}\n  expected authority: {expected_authority:?}\n  actual authority:   {:?}\n  full sequence: {events:#?}",
          daemon_projection(harness.state()),
          daemon_authority(harness.state()),
        );

        let known = model.known_asset_ids();
        if let Some(id) = &expected.wire_artwork_id {
          prop_assert!(known.contains(id), "wire artwork id {id} not in known assets {known:?}");
        }
        if let Some(id) = &expected.wire_artwork_id {
          prop_assert!(!id.contains("0000000000000000"), "idle-sentinel art url surfaced: {id}");
        }
      }

      Ok(())
    })?;
  }
}

proptest! {
  #![proptest_config(ProptestConfig { cases: proptest_cases(64), ..ProptestConfig::default() })]

  #[test]
  fn chaos_burst_holds_safety_invariants(events in chaos_events_strategy()) {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .worker_threads(2)
      .enable_all()
      .build()
      .unwrap();

    rt.block_on(async move {
      let harness = Harness::start().await.expect("harness start");
      let addr = harness.iap2_peer();
      let phone = harness.connect_android().await.expect("connect companion");
      phone.capabilities().announce(caps()).await.expect("announce");
      let mut player_watch = harness.state().player.snapshot_watch();

      for event in &events {
        apply_to_daemon(&harness, addr, &phone, event).await.expect("inject");
      }
      quiesce(&mut player_watch).await;

      let proj = daemon_projection(harness.state());
      for id in [&proj.wire_artwork_id, &proj.current_artwork_id].into_iter().flatten() {
        prop_assert!(
          !id.contains("0000000000000000"),
          "idle-sentinel art url surfaced under chaos: {id}\n  sequence: {events:#?}"
        );
      }

      player_watch.mark_unchanged();
      harness
        .inject_iap2(addr, SessionEvent::NowPlayingUpdate(liveness_npu()))
        .await
        .expect("liveness inject");
      let alive = tokio::time::timeout(CONVERGE, player_watch.changed()).await.is_ok();
      prop_assert!(alive, "player actor did not process an event after the burst (deadlock/panic?)\n  sequence: {events:#?}");

      Ok(())
    })?;
  }
}
