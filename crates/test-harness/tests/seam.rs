use std::time::Duration;

use bridgething::{ClientMode, Iap2TransportCommand};
use bridgething_iap2::{
  HidCommand,
  csm::now_playing::{
    MediaItemAttributes as Iap2MediaItem, NowPlayingUpdate as Iap2NowPlaying, PlaybackAttributes as Iap2Playback,
    PlaybackState as Iap2PlaybackState,
  },
};
use bridgething_test_harness::{
  CommandDriver, FrameObserve, FrameObserver, GatewayDriver, Harness, Iap2OutboundObserve, Iap2Source,
  Iap2SourceDriver, ModernClientDriver, WebappProvision, extract_substring_starting_with,
};
#[cfg(target_os = "linux")]
use bridgething_test_harness::{DeviceHarness, DeviceTier, OverAirTransport};
use libbridgething::{
  CompanionAuthorityScope, GatewayCapabilities, GatewayInfo, GeoAccuracy, GeoError, MediaItem, PhoneCall,
  PhoneCallDirection, PhoneCallStatus, PlayerState, Position,
  client::{GeoGetOnce, GeoWatch, SetShuffle},
  gateway::{
    AuthorityClaim, BridgeToGatewayGeoMsg, BridgeToGatewayMsg, BridgeToGatewayMsgData,
    GeoErrorReply as GatewayGeoErrorReply, SpotifyWakeRequest,
  },
};

fn tap_wake() -> SpotifyWakeRequest {
  SpotifyWakeRequest { allow_play_tap: true }
}

const SNAPSHOT_BARRIER: Duration = Duration::from_secs(10);

const NOW_PLAYING_WAIT: Duration = Duration::from_secs(20);

macro_rules! lift {
  ($scenario:ident, [$($tier:ident),+ $(,)?]) => {
    mod $scenario {
      $( lift!(@tier $tier, $scenario); )+
    }
  };
  (@tier t1, $scenario:ident) => {
    #[tokio::test]
    async fn t1() {
      let tier = super::Harness::start().await.expect("harness start");
      super::$scenario(&tier).await.expect(concat!(stringify!($scenario), " (t1)"));
    }
  };
  (@tier t3_rfcomm, $scenario:ident) => {
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires a booted Car Thing with the test-tap daemon + a host BT radio"]
    async fn t3_rfcomm() {
      let tier = super::DeviceTier::new(
        super::DeviceHarness::from_env().expect("device env (SUPERBIRD_BT_MAC)"),
        super::OverAirTransport::Rfcomm,
      );
      super::$scenario(&tier).await.expect(concat!(stringify!($scenario), " (t3 rfcomm)"));
    }
  };
  (@tier t3_iap2_ea, $scenario:ident) => {
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires a booted Car Thing with the test-tap daemon + a host BT radio"]
    async fn t3_iap2_ea() {
      let tier = super::DeviceTier::new(
        super::DeviceHarness::from_env().expect("device env (SUPERBIRD_BT_MAC)"),
        super::OverAirTransport::Iap2Ea,
      );
      super::$scenario(&tier).await.expect(concat!(stringify!($scenario), " (t3 iap2-ea)"));
    }
  };
  (@tier t3_emulator, $scenario:ident) => {
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires a booted Car Thing with the test-tap daemon + a host BT radio"]
    async fn t3_emulator() {
      let tier = super::DeviceHarness::from_env().expect("device env (SUPERBIRD_BT_MAC)");
      super::$scenario(&tier).await.expect(concat!(stringify!($scenario), " (t3 emulator)"));
    }
  };
}

fn caps() -> GatewayCapabilities {
  GatewayCapabilities {
    gateway: GatewayInfo {
      address: String::new(),
      name: "seam".into(),
      os_name: "android".into(),
      app_name: "seam".into(),
      app_version: "0.0.0".into(),
      adapter_version: "harness".into(),
      lib_version: "0.0.0".into(),
      libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
    },
    ..Default::default()
  }
}

async fn observe_with_registered_client<T>(tier: &T) -> anyhow::Result<FrameObserver>
where
  T: FrameObserve + ModernClientDriver,
{
  let mut frames = tier.frames().await?;
  let mut client = tier.modern_client().await?;
  tokio::spawn(async move { while client.recv().await.is_some() {} });
  let registered = frames
    .wait_for(SNAPSHOT_BARRIER, |f| f.mode == ClientMode::Modern)
    .await;
  anyhow::ensure!(registered.is_some(), "modern client snapshot never egressed");
  Ok(frames)
}

async fn gateway_now_playing_reaches_frame_tap<T>(tier: &T) -> anyhow::Result<()>
where
  T: GatewayDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;

  let gateway = tier.gateway().await?;
  gateway.capabilities().announce(caps()).await.expect("announce");
  gateway
    .authority()
    .claim(AuthorityClaim {
      scope: CompanionAuthorityScope::NowPlayingMetadata,
      app_bundle: None,
    })
    .await
    .expect("claim metadata authority");
  gateway
    .player()
    .snapshot(PlayerState {
      track: Some(MediaItem {
        persistent_id: Some("seam-gateway".into()),
        title: Some("Seam Gateway Track".into()),
        artist: Some("Seam".into()),
        ..Default::default()
      }),
      ..Default::default()
    })
    .await
    .expect("push now-playing");

  let observed = frames
    .wait_for(NOW_PLAYING_WAIT, |f| f.json().contains("Seam Gateway Track"))
    .await;
  anyhow::ensure!(observed.is_some(), "gateway now-playing never reached the frame-tap");
  Ok(())
}

async fn iap2_source_now_playing_reaches_frame_tap<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;

  let source = tier.iap2_source().await?;
  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0x5EA3),
        title: Some("Seam iAP2 Track".into()),
        artist: Some("Seam".into()),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;

  let observed = frames
    .wait_for(NOW_PLAYING_WAIT, |f| f.json().contains("Seam iAP2 Track"))
    .await;
  anyhow::ensure!(
    observed.is_some(),
    "iAP2 source now-playing never reached the frame-tap"
  );
  Ok(())
}

async fn single_source_artwork_reaches_frame_tap<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;
  let source = tier.iap2_source().await?;
  let pid = 0x5EA3u64;
  let transfer_id = 9u8;
  let art_id = format!("iap2/art/{pid:016x}/");

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(pid),
        title: Some("Cover Art Track".into()),
        artwork_id: Some(transfer_id),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(pid),
        title: Some("Cover Art Track".into()),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source.push_artwork(transfer_id, vec![0xFF; 1024]).await?;

  let observed = frames.wait_for(NOW_PLAYING_WAIT, |f| f.json().contains(&art_id)).await;
  anyhow::ensure!(
    observed.is_some(),
    "resolved cover-art id {art_id} never reached the frame-tap"
  );
  Ok(())
}

async fn idle_sentinel_never_broadcasts_art_url<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;
  let source = tier.iap2_source().await?;
  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0),
        title: Some(String::new()),
        artwork_id: Some(3),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source.push_artwork(3, vec![0xFF; 64]).await?;

  let leaked = frames
    .wait_for(Duration::from_secs(2), |f| {
      f.json().contains("iap2/art/0000000000000000")
    })
    .await;
  anyhow::ensure!(leaked.is_none(), "idle-sentinel art url leaked to the frame-tap");
  Ok(())
}

async fn non_music_pid_zero_with_title_surfaces<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;
  let source = tier.iap2_source().await?;
  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0),
        title: Some("Big Buck Bunny".into()),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;

  let observed = frames
    .wait_for(NOW_PLAYING_WAIT, |f| f.json().contains("Big Buck Bunny"))
    .await;
  anyhow::ensure!(
    observed.is_some(),
    "non-music pid-0 track with a real title never surfaced"
  );
  Ok(())
}

async fn non_music_artwork_reaches_frame_tap<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;
  let source = tier.iap2_source().await?;
  let transfer_id = 7u8;

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0),
        title: Some("Big Buck Bunny".into()),
        artwork_id: Some(transfer_id),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source.push_artwork(transfer_id, vec![0xFF; 1024]).await?;

  let art_seen = frames
    .wait_for(NOW_PLAYING_WAIT, |f| {
      let j = f.json();
      j.contains("Big Buck Bunny") && j.contains("iap2/art/") && !j.contains("iap2/art/0000000000000000")
    })
    .await;
  anyhow::ensure!(
    art_seen.is_some(),
    "non-music cover art never reached the frame-tap (dropped, or keyed as the idle sentinel)"
  );
  Ok(())
}

async fn transport_routes_to_iap2_and_refuses_unknown_shuffle<T>(tier: &T) -> anyhow::Result<()>
where
  T: CommandDriver + Iap2OutboundObserve,
{
  const PLAY_PAUSE: u8 = 0x01;
  const SHUFFLE: u8 = 0x40;

  let mut outbound = tier.iap2_outbound().await?;
  let client = tier.command_client().await?;

  client
    .player()
    .set_shuffle(SetShuffle { on: true })
    .await
    .expect("set_shuffle");
  client.player().pause().await.expect("pause");

  let pulses = outbound.collect_for(Duration::from_secs(2)).await;
  let is_pulse = |c: &Iap2TransportCommand, bit: u8| matches!(c, Iap2TransportCommand::Hid(HidCommand::Pulse(mask)) if mask & bit != 0);
  anyhow::ensure!(
    pulses.iter().any(|c| is_pulse(c, PLAY_PAUSE)),
    "pause did not route to the iAP2 HID play/pause pulse: {pulses:?}"
  );
  anyhow::ensure!(
    !pulses.iter().any(|c| is_pulse(c, SHUFFLE)),
    "set_shuffle with unknown state must refuse, but a shuffle pulse was emitted: {pulses:?}"
  );
  Ok(())
}

async fn cold_spotify_wake_holds_play_until_spotify_claims<T>(tier: &T) -> anyhow::Result<()>
where
  T: GatewayDriver + Iap2SourceDriver + Iap2OutboundObserve,
{
  const PLAY_PAUSE: u8 = 0x01;
  let is_pulse = |c: &Iap2TransportCommand| matches!(c, Iap2TransportCommand::Hid(HidCommand::Pulse(mask)) if mask & PLAY_PAUSE != 0);

  let mut outbound = tier.iap2_outbound().await?;
  let gateway = tier.gateway().await?;
  gateway.capabilities().announce(caps()).await.expect("announce");
  let source = tier.iap2_source().await?;

  gateway.player().request_spotify_wake(tap_wake()).await.expect("wake");

  let launch = outbound
    .wait_for(Duration::from_secs(5), |c| {
      matches!(c, Iap2TransportCommand::RequestAppLaunch(_))
    })
    .await;
  anyhow::ensure!(launch.is_some(), "wake never requested the spotify app launch");

  let premature = outbound.collect_for(Duration::from_millis(2500)).await;
  anyhow::ensure!(
    !premature.iter().any(is_pulse),
    "play tapped before spotify claimed now-playing: {premature:?}"
  );

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: None,
      playback: Some(Iap2Playback {
        state: Some(Iap2PlaybackState::Paused),
        app_bundle: Some("com.spotify.client".into()),
        ..Default::default()
      }),
    })
    .await?;

  let tapped = outbound.wait_for(Duration::from_secs(5), |c| is_pulse(c)).await;
  anyhow::ensure!(tapped.is_some(), "play never tapped after spotify claimed now-playing");
  Ok(())
}

async fn duplicate_spotify_wakes_tap_play_exactly_once<T>(tier: &T) -> anyhow::Result<()>
where
  T: GatewayDriver + Iap2SourceDriver + Iap2OutboundObserve,
{
  const PLAY_PAUSE: u8 = 0x01;
  let is_pulse = |c: &Iap2TransportCommand| matches!(c, Iap2TransportCommand::Hid(HidCommand::Pulse(mask)) if mask & PLAY_PAUSE != 0);

  let mut outbound = tier.iap2_outbound().await?;
  let gateway = tier.gateway().await?;
  gateway.capabilities().announce(caps()).await.expect("announce");
  let source = tier.iap2_source().await?;

  gateway.player().request_spotify_wake(tap_wake()).await.expect("wake 1");
  gateway.player().request_spotify_wake(tap_wake()).await.expect("wake 2");
  let launch = outbound
    .wait_for(Duration::from_secs(5), |c| {
      matches!(c, Iap2TransportCommand::RequestAppLaunch(_))
    })
    .await;
  anyhow::ensure!(launch.is_some(), "wake never requested the spotify app launch");

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: None,
      playback: Some(Iap2Playback {
        state: Some(Iap2PlaybackState::Paused),
        app_bundle: Some("com.spotify.client".into()),
        ..Default::default()
      }),
    })
    .await?;

  let tapped = outbound.wait_for(Duration::from_secs(5), |c| is_pulse(c)).await;
  anyhow::ensure!(tapped.is_some(), "play never tapped after spotify claimed now-playing");
  let extra = outbound.collect_for(Duration::from_millis(2000)).await;
  anyhow::ensure!(
    !extra.iter().any(is_pulse),
    "duplicate wake tapped play a second time: {extra:?}"
  );
  Ok(())
}

async fn cold_spotify_wake_withholds_play_when_another_app_claims<T>(tier: &T) -> anyhow::Result<()>
where
  T: GatewayDriver + Iap2SourceDriver + Iap2OutboundObserve,
{
  const PLAY_PAUSE: u8 = 0x01;
  let is_pulse = |c: &Iap2TransportCommand| matches!(c, Iap2TransportCommand::Hid(HidCommand::Pulse(mask)) if mask & PLAY_PAUSE != 0);

  let mut outbound = tier.iap2_outbound().await?;
  let gateway = tier.gateway().await?;
  gateway.capabilities().announce(caps()).await.expect("announce");
  let source = tier.iap2_source().await?;

  gateway.player().request_spotify_wake(tap_wake()).await.expect("wake");
  let launch = outbound
    .wait_for(Duration::from_secs(5), |c| {
      matches!(c, Iap2TransportCommand::RequestAppLaunch(_))
    })
    .await;
  anyhow::ensure!(launch.is_some(), "wake never requested the spotify app launch");

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: None,
      playback: Some(Iap2Playback {
        state: Some(Iap2PlaybackState::Paused),
        app_bundle: Some("com.apple.podcasts".into()),
        ..Default::default()
      }),
    })
    .await?;

  let pulses = outbound.collect_for(Duration::from_millis(2500)).await;
  anyhow::ensure!(
    !pulses.iter().any(is_pulse),
    "play tapped after a non-spotify app claimed now-playing: {pulses:?}"
  );
  Ok(())
}

async fn launch_only_spotify_wake_never_taps_play<T>(tier: &T) -> anyhow::Result<()>
where
  T: GatewayDriver + Iap2SourceDriver + Iap2OutboundObserve,
{
  const PLAY_PAUSE: u8 = 0x01;
  let is_pulse = |c: &Iap2TransportCommand| matches!(c, Iap2TransportCommand::Hid(HidCommand::Pulse(mask)) if mask & PLAY_PAUSE != 0);

  let mut outbound = tier.iap2_outbound().await?;
  let gateway = tier.gateway().await?;
  gateway.capabilities().announce(caps()).await.expect("announce");
  let source = tier.iap2_source().await?;

  gateway
    .player()
    .request_spotify_wake(SpotifyWakeRequest { allow_play_tap: false })
    .await
    .expect("wake");
  let launch = outbound
    .wait_for(Duration::from_secs(5), |c| {
      matches!(c, Iap2TransportCommand::RequestAppLaunch(_))
    })
    .await;
  anyhow::ensure!(
    launch.is_some(),
    "launch-only wake never requested the spotify app launch"
  );

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: None,
      playback: Some(Iap2Playback {
        state: Some(Iap2PlaybackState::Paused),
        app_bundle: Some("com.spotify.client".into()),
        ..Default::default()
      }),
    })
    .await?;

  let pulses = outbound.collect_for(Duration::from_millis(2500)).await;
  anyhow::ensure!(
    !pulses.iter().any(is_pulse),
    "a launch-only wake tapped play; the parked session could be a remote speaker: {pulses:?}"
  );
  Ok(())
}

async fn launch_only_wake_launches_instead_of_tapping_a_paused_spotify<T>(tier: &T) -> anyhow::Result<()>
where
  T: GatewayDriver + Iap2SourceDriver + Iap2OutboundObserve,
{
  const PLAY_PAUSE: u8 = 0x01;
  let is_pulse = |c: &Iap2TransportCommand| matches!(c, Iap2TransportCommand::Hid(HidCommand::Pulse(mask)) if mask & PLAY_PAUSE != 0);

  let mut outbound = tier.iap2_outbound().await?;
  let gateway = tier.gateway().await?;
  gateway.capabilities().announce(caps()).await.expect("announce");
  let source = tier.iap2_source().await?;

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: None,
      playback: Some(Iap2Playback {
        state: Some(Iap2PlaybackState::Paused),
        app_bundle: Some("com.spotify.client".into()),
        ..Default::default()
      }),
    })
    .await?;

  gateway
    .player()
    .request_spotify_wake(SpotifyWakeRequest { allow_play_tap: false })
    .await
    .expect("wake");

  let launch = outbound
    .wait_for(Duration::from_secs(5), |c| {
      matches!(c, Iap2TransportCommand::RequestAppLaunch(_))
    })
    .await;
  anyhow::ensure!(
    launch.is_some(),
    "a launch-only wake of a paused spotify must launch, not tap"
  );
  let pulses = outbound.collect_for(Duration::from_millis(2500)).await;
  anyhow::ensure!(
    !pulses.iter().any(is_pulse),
    "a launch-only wake tapped a paused spotify; that resumes wherever the session is parked: {pulses:?}"
  );
  Ok(())
}

async fn incoming_call_surfaces_to_webapp<T>(tier: &T) -> anyhow::Result<()>
where
  T: GatewayDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;
  let gateway = tier.gateway().await?;
  gateway.capabilities().announce(caps()).await.expect("announce");
  gateway
    .phone()
    .call_started(PhoneCall {
      call_id: "seam-call-1".into(),
      remote_id: "+15550000001".into(),
      display_name: "Ada Lovelace".into(),
      status: PhoneCallStatus::Ringing,
      direction: PhoneCallDirection::Incoming,
      started_at_unix_s: None,
      label: None,
      address_book_id: None,
      service: None,
      is_conferenced: None,
      conference_group: None,
    })
    .await
    .expect("call_started");

  let observed = frames
    .wait_for(NOW_PLAYING_WAIT, |f| f.json().contains("Ada Lovelace"))
    .await;
  anyhow::ensure!(
    observed.is_some(),
    "incoming call never reached the webapp frame stream"
  );
  Ok(())
}

lift!(gateway_now_playing_reaches_frame_tap, [t1, t3_rfcomm, t3_iap2_ea]);
lift!(incoming_call_surfaces_to_webapp, [t1, t3_rfcomm, t3_iap2_ea]);

async fn geo_position_reaches_watching_webapp<T>(tier: &T) -> anyhow::Result<()>
where
  T: CommandDriver + GatewayDriver + FrameObserve + WebappProvision,
{
  let mut frames = tier.frames().await?;
  let gateway = tier.gateway().await?;
  let mut announce = caps();
  announce.available.geo = true;
  gateway.capabilities().announce(announce).await.expect("announce");
  tier.activate_webapp_declaring(&["geo"]).await?;
  let client = tier.command_client().await?;

  client
    .geo()
    .watch(GeoWatch {
      accuracy: GeoAccuracy::Fine,
      min_interval_ms: 1000,
    })
    .await
    .expect("geo watch");
  gateway
    .geo()
    .position(Position {
      lat: 12.25,
      lon: -71.5,
      alt_m: None,
      accuracy_m: 5.0,
      speed_mps: None,
      heading_deg: None,
      ts_unix_s: 1_700_000_000,
    })
    .await
    .expect("position");

  let observed = frames.wait_for(NOW_PLAYING_WAIT, |f| f.json().contains("12.25")).await;
  anyhow::ensure!(
    observed.is_some(),
    "companion geo position never reached the watching webapp"
  );
  Ok(())
}

lift!(transport_routes_to_iap2_and_refuses_unknown_shuffle, [t1]);
lift!(cold_spotify_wake_holds_play_until_spotify_claims, [t1]);
lift!(cold_spotify_wake_withholds_play_when_another_app_claims, [t1]);
lift!(duplicate_spotify_wakes_tap_play_exactly_once, [t1]);
lift!(launch_only_spotify_wake_never_taps_play, [t1]);
lift!(launch_only_wake_launches_instead_of_tapping_a_paused_spotify, [t1]);

lift!(geo_position_reaches_watching_webapp, [t1]);

async fn geo_watch_failure_reaches_watching_webapp<T>(tier: &T) -> anyhow::Result<()>
where
  T: CommandDriver + GatewayDriver + FrameObserve + WebappProvision,
{
  let mut frames = tier.frames().await?;
  let gateway = tier.gateway().await?;
  let mut announce = caps();
  announce.available.geo = true;
  gateway.capabilities().announce(announce).await.expect("announce");
  tier.activate_webapp_declaring(&["geo"]).await?;
  let client = tier.command_client().await?;

  client
    .geo()
    .watch(GeoWatch {
      accuracy: GeoAccuracy::Fine,
      min_interval_ms: 1000,
    })
    .await
    .expect("geo watch");
  gateway
    .geo()
    .error_event(GatewayGeoErrorReply {
      error: GeoError::PermissionDenied,
    })
    .await
    .expect("error event");

  let observed = frames
    .wait_for(NOW_PLAYING_WAIT, |f| {
      let json = f.json();
      json.contains("errorEvent") && json.contains("permissionDenied")
    })
    .await;
  anyhow::ensure!(
    observed.is_some(),
    "companion geo watch failure never reached the watching webapp"
  );
  Ok(())
}

lift!(geo_watch_failure_reaches_watching_webapp, [t1]);

async fn abrupt_webapp_disconnect_releases_geo_watch<T>(tier: &T) -> anyhow::Result<()>
where
  T: CommandDriver + GatewayDriver + WebappProvision,
{
  let gateway = tier.gateway().await?;
  let mut events = gateway.events();
  let mut announce = caps();
  announce.available.geo = true;
  gateway.capabilities().announce(announce).await.expect("announce");
  tier.activate_webapp_declaring(&["geo"]).await?;

  let client = tier.command_client().await?;
  client
    .geo()
    .watch(GeoWatch {
      accuracy: GeoAccuracy::Fine,
      min_interval_ms: 1000,
    })
    .await
    .expect("geo watch");

  wait_for_geo(&mut events, |msg| matches!(msg, BridgeToGatewayGeoMsg::Watch(_)))
    .await
    .ok_or_else(|| anyhow::anyhow!("the watch never reached the companion"))?;

  drop(client);

  wait_for_geo(&mut events, |msg| matches!(msg, BridgeToGatewayGeoMsg::Unwatch))
    .await
    .ok_or_else(|| anyhow::anyhow!("watch was never released after the webapp vanished"))?;
  Ok(())
}

async fn geo_is_refused_for_a_webapp_that_never_declared_it<T>(tier: &T) -> anyhow::Result<()>
where
  T: CommandDriver + GatewayDriver + WebappProvision,
{
  let gateway = tier.gateway().await?;
  let mut events = gateway.events();
  let mut announce = caps();
  announce.available.geo = true;
  gateway.capabilities().announce(announce).await.expect("announce");
  tier.activate_webapp_declaring(&["net.fetch"]).await?;

  let client = tier.command_client().await?;
  let refused = client
    .geo()
    .get_once(GeoGetOnce {
      accuracy: GeoAccuracy::Fine,
      max_age_s: None,
    })
    .await;
  match refused {
    Err(bridgething_client::RequestFailure::Domain(reply)) => anyhow::ensure!(
      reply.error == GeoError::NotDeclared,
      "expected notDeclared, got {:?}",
      reply.error
    ),
    other => anyhow::bail!("undeclared geo should be refused, got {other:?}"),
  }
  anyhow::ensure!(
    wait_for_geo(&mut events, |msg| matches!(msg, BridgeToGatewayGeoMsg::GetOnce(_)))
      .await
      .is_none(),
    "an undeclared request still reached the phone"
  );
  Ok(())
}

lift!(geo_is_refused_for_a_webapp_that_never_declared_it, [t1]);

async fn a_request_missing_a_required_field_is_nacked_as_malformed<T>(tier: &T) -> anyhow::Result<()>
where
  T: ModernClientDriver,
{
  let mut client = tier.modern_client().await?;
  let request_id = uuid::Uuid::now_v7();
  client
    .send_text(format!(
      r#"{{"id":"{request_id}","meta":{{"kind":"request"}},"data":{{"type":"geo","data":{{"event":"getOnce","data":{{}}}}}}}}"#
    ))
    .await?;

  let reply = tokio::time::timeout(SNAPSHOT_BARRIER, async {
    while let Some(text) = client.recv().await {
      if text.contains(&request_id.to_string()) {
        return Some(text);
      }
    }
    None
  })
  .await
  .ok()
  .flatten()
  .ok_or_else(|| anyhow::anyhow!("the malformed request was never answered"))?;

  anyhow::ensure!(
    reply.contains("\"malformed\""),
    "a validation failure must not read as `unsupported`: {reply}"
  );
  anyhow::ensure!(
    reply.contains("accuracy"),
    "the nack should name the field that was missing: {reply}"
  );
  Ok(())
}

lift!(a_request_missing_a_required_field_is_nacked_as_malformed, [t1]);

async fn a_tolerant_get_once_is_served_from_the_held_fix<T>(tier: &T) -> anyhow::Result<()>
where
  T: CommandDriver + GatewayDriver + WebappProvision,
{
  let gateway = tier.gateway().await?;
  let mut events = gateway.events();
  let mut announce = caps();
  announce.available.geo = true;
  gateway.capabilities().announce(announce).await.expect("announce");
  tier.activate_webapp_declaring(&["geo"]).await?;

  let client = tier.command_client().await?;
  client
    .geo()
    .watch(GeoWatch {
      accuracy: GeoAccuracy::Fine,
      min_interval_ms: 1000,
    })
    .await
    .expect("geo watch");
  wait_for_geo(&mut events, |msg| matches!(msg, BridgeToGatewayGeoMsg::Watch(_)))
    .await
    .ok_or_else(|| anyhow::anyhow!("the watch never reached the companion"))?;

  gateway
    .geo()
    .position(Position {
      lat: 41.5,
      lon: -8.25,
      alt_m: None,
      accuracy_m: 6.0,
      speed_mps: None,
      heading_deg: None,
      ts_unix_s: 1_700_000_000,
    })
    .await
    .expect("position");

  let reply = client
    .geo()
    .get_once(GeoGetOnce {
      accuracy: GeoAccuracy::Fine,
      max_age_s: Some(300),
    })
    .await
    .expect("geo getOnce");
  anyhow::ensure!(
    (reply.position.lat - 41.5).abs() < f64::EPSILON,
    "getOnce answered with something other than the held fix: {:?}",
    reply.position
  );
  anyhow::ensure!(
    wait_for_geo(&mut events, |msg| matches!(msg, BridgeToGatewayGeoMsg::GetOnce(_)))
      .await
      .is_none(),
    "a tolerant getOnce woke the phone anyway"
  );
  Ok(())
}

lift!(a_tolerant_get_once_is_served_from_the_held_fix, [t1]);

async fn wait_for_geo<F>(events: &mut tokio::sync::broadcast::Receiver<BridgeToGatewayMsg>, pred: F) -> Option<()>
where
  F: Fn(&BridgeToGatewayGeoMsg) -> bool,
{
  tokio::time::timeout(SNAPSHOT_BARRIER, async {
    while let Ok(msg) = events.recv().await {
      if let BridgeToGatewayMsgData::Geo(geo) = &msg.data
        && pred(geo)
      {
        return Some(());
      }
    }
    None
  })
  .await
  .ok()
  .flatten()
}

lift!(abrupt_webapp_disconnect_releases_geo_watch, [t1]);

lift!(iap2_source_now_playing_reaches_frame_tap, [t1, t3_emulator]);
lift!(single_source_artwork_reaches_frame_tap, [t1, t3_emulator]);
lift!(idle_sentinel_never_broadcasts_art_url, [t1, t3_emulator]);
lift!(non_music_pid_zero_with_title_surfaces, [t1, t3_emulator]);
lift!(non_music_artwork_reaches_frame_tap, [t1, t3_emulator]);
lift!(spotify_pid_none_two_tracks_get_distinct_art_keys, [t1, t3_emulator]);
lift!(pid_less_track_change_after_pid_track_is_a_new_track, [t1, t3_emulator]);
lift!(re_sent_artwork_for_the_same_track_keeps_one_art_key, [t1, t3_emulator]);
lift!(idle_shaped_duration_delta_does_not_drop_pending_art, [t1, t3_emulator]);
lift!(position_resets_across_track_change, [t1, t3_emulator]);

async fn spotify_pid_none_two_tracks_get_distinct_art_keys<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;
  let source = tier.iap2_source().await?;
  let transfer_id = 9u8;

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: None,
        title: Some("Sanguirush".into()),
        artist: Some("The Destruction Of The Cult Of The Sun".into()),
        artwork_id: Some(transfer_id),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source.push_artwork(transfer_id, vec![0xAA; 1024]).await?;

  let track_a = frames
    .wait_for(NOW_PLAYING_WAIT, |f| {
      let j = f.json();
      j.contains("Sanguirush") && j.contains("iap2/art/")
    })
    .await;
  let track_a = track_a.ok_or_else(|| anyhow::anyhow!("track A art never reached the frame-tap"))?;
  let art_a = extract_substring_starting_with(track_a.json(), "iap2/art/")
    .ok_or_else(|| anyhow::anyhow!("track A frame had no iap2/art/ url"))?;

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: None,
        title: Some("Counterweight".into()),
        artist: Some("Other Artist".into()),
        artwork_id: Some(transfer_id),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source.push_artwork(transfer_id, vec![0xBB; 1024]).await?;

  let track_b = frames
    .wait_for(NOW_PLAYING_WAIT, |f| {
      let j = f.json();
      j.contains("Counterweight") && j.contains("iap2/art/")
    })
    .await;
  let track_b = track_b.ok_or_else(|| anyhow::anyhow!("track B art never reached the frame-tap"))?;
  let art_b = extract_substring_starting_with(track_b.json(), "iap2/art/")
    .ok_or_else(|| anyhow::anyhow!("track B frame had no iap2/art/ url"))?;

  anyhow::ensure!(
    art_a != art_b,
    "two distinct pid=None tracks shared one art key ({art_a}) - the daemon collapsed them to the same nonmusic slot"
  );
  Ok(())
}

async fn pid_less_track_change_after_pid_track_is_a_new_track<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;
  let source = tier.iap2_source().await?;
  let pid = 0x51DEu64;
  let pid_track_id = format!("iap2:track:{pid:016x}");

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(pid),
        title: Some("Side of Town".into()),
        artist: Some("Capital Soiree".into()),
        duration_ms: Some(177_280),
        artwork_id: Some(40),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source.push_artwork(40, vec![0xAA; 1024]).await?;
  let frame_a = frames
    .wait_for(NOW_PLAYING_WAIT, |f| {
      let j = f.json();
      j.contains("Side of Town") && j.contains("iap2/art/")
    })
    .await
    .ok_or_else(|| anyhow::anyhow!("the pid track's art never reached the frame-tap"))?;
  let art_a = extract_substring_starting_with(frame_a.json(), "iap2/art/")
    .ok_or_else(|| anyhow::anyhow!("the pid track's frame had no iap2/art/ url"))?;

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: None,
        title: Some("Sanguirush".into()),
        artist: Some("The Destruction Of The Cult Of The Sun".into()),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        artwork_id: Some(41),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source.push_artwork(41, vec![0xBB; 1024]).await?;

  let frame_b = frames
    .wait_for(NOW_PLAYING_WAIT, |f| {
      let j = f.json();
      j.contains("Sanguirush") && j.contains("iap2/art/")
    })
    .await
    .ok_or_else(|| anyhow::anyhow!("the pid-less track's art never reached the frame-tap"))?;
  let art_b = extract_substring_starting_with(frame_b.json(), "iap2/art/")
    .ok_or_else(|| anyhow::anyhow!("the pid-less track's frame had no iap2/art/ url"))?;
  anyhow::ensure!(
    !frame_b.json().contains(&pid_track_id),
    "a pid-less track change kept the previous track's pid identity ({pid_track_id}) - the accumulator never resets and the new track wears the old track's state"
  );
  anyhow::ensure!(
    art_b != art_a,
    "the first artful frame for the pid-less track still carried the previous track's art ({art_a}) - stale art flashes on every track change"
  );
  Ok(())
}

async fn re_sent_artwork_for_the_same_track_keeps_one_art_key<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;
  let source = tier.iap2_source().await?;
  let art_bytes = vec![0xCC; 1024];

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0xA57u64),
        title: Some("Coil".into()),
        artist: Some("Saint Blonde".into()),
        artwork_id: Some(129),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source.push_artwork(129, art_bytes.clone()).await?;
  let resolved = frames
    .wait_for(NOW_PLAYING_WAIT, |f| {
      let j = f.json();
      j.contains("Coil") && j.contains("iap2/art/")
    })
    .await
    .ok_or_else(|| anyhow::anyhow!("cover art never reached the frame-tap"))?;
  let art_key = extract_substring_starting_with(resolved.json(), "iap2/art/")
    .ok_or_else(|| anyhow::anyhow!("resolved frame had no iap2/art/ url"))?;

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: None,
        title: Some("Coil".into()),
        artist: Some("Saint Blonde".into()),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  for transfer_id in [130u8, 131u8] {
    source
      .push_now_playing(Iap2NowPlaying {
        media_item: Some(Iap2MediaItem {
          artwork_id: Some(transfer_id),
          ..Default::default()
        }),
        playback: None,
      })
      .await?;
    source.push_artwork(transfer_id, art_bytes.clone()).await?;
  }

  let rekeyed = frames
    .wait_for(Duration::from_secs(3), |f| {
      let j = f.json();
      extract_substring_starting_with(j, "iap2/art/").is_some_and(|id| id != art_key)
    })
    .await;
  anyhow::ensure!(
    rekeyed.is_none(),
    "a re-sent artwork transfer for the same track minted a new art key (was {art_key}, saw {:?}) - every re-key flashes the webapp back to placeholder",
    rekeyed.and_then(|f| extract_substring_starting_with(f.json(), "iap2/art/"))
  );
  Ok(())
}

async fn idle_shaped_duration_delta_does_not_drop_pending_art<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  let mut frames = observe_with_registered_client(tier).await?;
  let source = tier.iap2_source().await?;
  let transfer_id = 50u8;

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0),
        title: Some("Big Buck Bunny".into()),
        artwork_id: Some(transfer_id),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0),
        title: Some(String::new()),
        duration_ms: Some(596_000),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;
  source.push_artwork(transfer_id, vec![0xDD; 1024]).await?;

  let art = frames
    .wait_for(NOW_PLAYING_WAIT, |f| {
      let j = f.json();
      j.contains("Big Buck Bunny") && j.contains("iap2/art/") && !j.contains("iap2/art/0000000000000000")
    })
    .await;
  anyhow::ensure!(
    art.is_some(),
    "the idle-shaped duration delta dropped the pending artwork - the track's art never resolved"
  );
  Ok(())
}

async fn position_resets_across_track_change<T>(tier: &T) -> anyhow::Result<()>
where
  T: Iap2SourceDriver + FrameObserve + ModernClientDriver,
{
  use bridgething_iap2::csm::now_playing::{PlaybackAttributes, PlaybackState};

  let mut frames = observe_with_registered_client(tier).await?;
  let source = tier.iap2_source().await?;

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0x1111),
        title: Some("ThreeMinuteTrack".into()),
        duration_ms: Some(240_000),
        ..Default::default()
      }),
      playback: Some(PlaybackAttributes {
        state: Some(PlaybackState::Playing),
        position_ms: Some(180_000),
        ..Default::default()
      }),
    })
    .await?;
  let _ = frames
    .wait_for(NOW_PLAYING_WAIT, |f| f.json().contains("ThreeMinuteTrack"))
    .await
    .ok_or_else(|| anyhow::anyhow!("track A never reached the frame-tap"))?;

  source
    .push_now_playing(Iap2NowPlaying {
      media_item: Some(Iap2MediaItem {
        persistent_id: Some(0x2222),
        title: Some("FreshTrack".into()),
        duration_ms: Some(200_000),
        ..Default::default()
      }),
      playback: None,
    })
    .await?;

  let frame_b = frames
    .wait_for(NOW_PLAYING_WAIT, |f| f.json().contains("FreshTrack"))
    .await
    .ok_or_else(|| anyhow::anyhow!("track B never reached the frame-tap"))?;
  let position = position_ms_from_frame_json(frame_b.json())
    .ok_or_else(|| anyhow::anyhow!("could not parse position_ms from track B frame: {}", frame_b.json()))?;
  anyhow::ensure!(
    position < 10_000,
    "new track surfaced at {position} ms (carried over from the prior track at 180000 ms)"
  );
  Ok(())
}

fn position_ms_from_frame_json(json: &str) -> Option<u64> {
  let mut largest: Option<u64> = None;
  for key in ["\"positionMs\"", "\"position_ms\""] {
    for (idx, _) in json.match_indices(key) {
      let tail = &json[idx + key.len()..];
      let Some(after_colon) = tail.find(':') else {
        continue;
      };
      let after = tail[after_colon + 1..].trim_start();
      let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
      if end == 0 {
        continue;
      }
      if let Ok(value) = after[..end].parse::<u64>() {
        largest = Some(largest.map_or(value, |prev| prev.max(value)));
      }
    }
  }
  largest
}
