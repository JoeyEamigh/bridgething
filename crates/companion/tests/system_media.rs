#[path = "rig/media.rs"]
mod media;
#[path = "support/poll.rs"]
mod poll;
#[path = "dispatch/quiet.rs"]
mod quiet;
#[path = "dispatch/settled.rs"]
mod settled;
#[path = "dispatch/support.rs"]
mod support;

use std::{sync::Arc, time::Duration};

use bridgething_companion::{
  api::{CapabilityFlags, HostInfo},
  backend::{MediaArt, MediaControl, MediaQueueEntry, MediaRepeatMode},
  hub::Hub,
  provider::{
    PlayerTransport, Provider, ProviderRegistry,
    system_media::{ASSET_ID_PREFIX, SOURCE_ID, SystemMediaProvider},
  },
};
use libbridgething::{
  ItemKind, ItemRef, PlaybackState, RepeatMode, ShuffleMode,
  gateway::{GatewayToBridgeMsg, GatewayToBridgeMsgData, GatewayToBridgePlayerMsg, QueueSnapshot},
};
use media::{FakeMediaBackend, playing};
use poll::eventually;
use support::Peer;

fn snapshot_of(msg: &GatewayToBridgeMsg) -> Option<libbridgething::PlayerState> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::Snapshot(state)) => Some(state.as_ref().clone()),
    _ => None,
  }
}

fn queue_of(msg: &GatewayToBridgeMsg) -> Option<QueueSnapshot> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::QueueChanged(queue)) => Some(queue.clone()),
    _ => None,
  }
}
fn queue_entry(id: i64, title: &str, subtitle: &str) -> MediaQueueEntry {
  MediaQueueEntry {
    queue_id: id,
    title: Some(title.into()),
    subtitle: Some(subtitle.into()),
    art_token: None,
  }
}

struct Rig {
  hub: Arc<Hub>,
  peer: Peer,
  source: Arc<SystemMediaProvider>,
  backend: Arc<FakeMediaBackend>,
}

async fn boot(owned: Vec<String>) -> Rig {
  let (gateway, peer) = Peer::link();
  let hub = Hub::new(
    Arc::new(gateway),
    HostInfo {
      app_name: "sysmedia-test".into(),
      app_version: "0.0.1".into(),
      os_name: "test".into(),
      os_version: String::new(),
      host_identifier: String::new(),
    },
    CapabilityFlags {
      geo: false,
      notifications: false,
      net_fetch: true,
      net_ws: true,
      audio_tts: false,
      voice_model: false,
    },
    false,
  );
  hub.start();
  let backend = FakeMediaBackend::new();
  let source = SystemMediaProvider::new(backend.clone(), Arc::new(move || owned.clone()));
  hub.attach_system(source.clone()).await.expect("the fallback attached");
  Rig {
    hub,
    peer,
    source,
    backend,
  }
}

#[tokio::test]
async fn picks_the_playing_foreign_session_and_maps_its_metadata() {
  let rig = boot(vec!["com.spotify.client".into()]).await;
  rig
    .backend
    .emit(vec![playing("Video", "Creator", "com.google.android.youtube")]);

  let state = rig.peer.wait("the snapshot", snapshot_of).await;
  let track = state.track.as_ref().unwrap();
  assert_eq!(track.title.as_deref(), Some("Video"));
  assert_eq!(track.artist.as_deref(), Some("Creator"));
  assert_eq!(state.playback.state, PlaybackState::Playing);
  assert_eq!(state.playback.position_ms, 250);
  assert_eq!(
    rig.hub.now_playing().current_source().as_deref(),
    Some(SOURCE_ID),
    "the fallback became the audible source"
  );
}

#[tokio::test]
async fn a_session_owned_by_a_provider_is_never_double_emitted() {
  let rig = boot(vec!["com.spotify.music".into()]).await;
  rig.backend.emit(vec![playing("Song", "Artist", "com.spotify.music")]);

  rig
    .peer
    .quiet("a snapshot for spotify's own session", snapshot_of)
    .await;
}

#[tokio::test]
async fn pausing_the_only_session_keeps_it_rather_than_dropping_the_source() {
  let rig = boot(Vec::new()).await;
  rig
    .backend
    .emit(vec![playing("Video", "Creator", "com.google.android.youtube")]);
  rig.peer.wait("the first snapshot", snapshot_of).await;

  let mut paused = playing("Video", "Creator", "com.google.android.youtube");
  paused.playing = false;
  rig.backend.emit(vec![paused]);

  assert!(
    eventually(|| rig
      .peer
      .latest(snapshot_of)
      .is_some_and(|state| state.playback.state == PlaybackState::Paused))
    .await,
    "a pause is a state the source reports, not a reason to stop being a source"
  );
  assert_eq!(
    rig.hub.now_playing().current_source().as_deref(),
    Some(SOURCE_ID),
    "a paused session with a track is exactly what resume, skip and seek are for; dropping it here hands \
     every one of those verbs to whatever stale source is left in the table"
  );
}

#[tokio::test]
async fn a_session_that_leaves_the_roster_clears_the_source() {
  let rig = boot(Vec::new()).await;
  rig
    .backend
    .emit(vec![playing("Video", "Creator", "com.google.android.youtube")]);
  rig.peer.wait("the first snapshot", snapshot_of).await;

  rig.backend.emit(Vec::new());

  assert!(
    eventually(|| rig.hub.now_playing().current_source().is_none()).await,
    "the session actually going away is the one thing that does clear the source"
  );
}

#[tokio::test]
async fn an_unchanged_snapshot_does_not_resubmit() {
  let rig = boot(Vec::new()).await;
  let session = playing("Video", "Creator", "com.google.android.youtube");
  rig.backend.emit(vec![session.clone()]);
  rig.peer.wait("the first snapshot", snapshot_of).await;
  rig.backend.emit(vec![session.clone()]);
  rig.backend.emit(vec![session]);

  assert_eq!(
    rig.peer.settled_count(snapshot_of).await,
    1,
    "an unchanged audible snapshot must not re-push over the link"
  );
}

#[tokio::test]
async fn a_fresher_read_of_the_same_position_does_not_resubmit() {
  let rig = boot(Vec::new()).await;
  let mut session = playing("Video", "Creator", "com.google.android.youtube");
  session.position_age_ms = Some(10);
  rig.backend.emit(vec![session.clone()]);
  rig.peer.wait("the first snapshot", snapshot_of).await;
  session.position_age_ms = Some(900);
  rig.backend.emit(vec![session]);

  assert_eq!(rig.peer.settled_count(snapshot_of).await, 1);
}

#[tokio::test]
async fn the_art_token_becomes_an_asset_id_the_audible_session_serves() {
  let rig = boot(Vec::new()).await;
  let mut session = playing("Video", "Creator", "com.google.android.youtube");
  session.art_token = Some("bdeadbeef".into());
  rig.backend.art.lock().unwrap().insert(
    ("com.google.android.youtube".into(), "bdeadbeef".into()),
    MediaArt {
      bytes: vec![1, 2, 3],
      mime: "image/jpeg".into(),
    },
  );
  rig.backend.emit(vec![session]);

  let state = rig.peer.wait("the snapshot", snapshot_of).await;
  let artwork_id = state.track.as_ref().unwrap().artwork_id.clone().unwrap();
  assert_eq!(artwork_id, format!("{ASSET_ID_PREFIX}bdeadbeef"));

  let served = rig.source.asset(&artwork_id).await.unwrap().unwrap();
  assert_eq!(served.mime.as_deref(), Some("image/jpeg"));
  assert_eq!(served.bytes, vec![1, 2, 3]);
  assert!(
    rig
      .source
      .asset(&format!("{ASSET_ID_PREFIX}gone"))
      .await
      .unwrap()
      .is_none(),
    "an unresolvable token serves nothing"
  );
  assert!(
    rig.source.asset("img:not-ours").await.unwrap().is_none(),
    "a non-system id is never served here"
  );
}

#[tokio::test]
async fn the_queue_maps_to_the_upcoming_window_after_the_active_item() {
  let rig = boot(Vec::new()).await;
  let mut session = playing("Current", "B", "com.google.android.youtube");
  let mut with_art = queue_entry(12, "Next", "C");
  with_art.art_token = Some("u123".into());
  session.queue = vec![
    queue_entry(10, "Played", "A"),
    queue_entry(11, "Current", "B"),
    with_art,
    queue_entry(13, "Later", "D"),
  ];
  session.active_queue_id = Some(11);
  rig.backend.emit(vec![session.clone()]);

  let queue = rig.peer.wait("the queue push", queue_of).await;
  let titles: Vec<&str> = queue.items.iter().filter_map(|item| item.title.as_deref()).collect();
  assert_eq!(titles, vec!["Next", "Later"]);
  assert_eq!(
    queue.order,
    queue.items.iter().map(|i| i.uri.clone()).collect::<Vec<_>>()
  );
  assert_eq!(
    queue.items[0].artwork_id.as_deref(),
    Some(&format!("{ASSET_ID_PREFIX}u123")[..])
  );
  assert_eq!(queue.items[0].artist.as_deref(), Some("C"));

  rig.backend.emit(vec![session]);
  assert_eq!(
    rig.peer.settled_count(queue_of).await,
    1,
    "an unchanged queue must not re-push over the link"
  );
}

#[tokio::test]
async fn skip_to_index_routes_to_the_queue_id_of_the_upcoming_entry() {
  let rig = boot(Vec::new()).await;
  let mut session = playing("Current", "B", "com.google.android.youtube");
  session.queue = vec![
    queue_entry(11, "Current", "B"),
    queue_entry(12, "Next", "C"),
    queue_entry(13, "Later", "D"),
  ];
  session.active_queue_id = Some(11);
  rig.backend.emit(vec![session]);
  rig.peer.wait("the queue push", queue_of).await;

  rig.source.skip_to_index(1).await.unwrap();
  rig
    .backend
    .wait_control(MediaControl::SkipToQueueItem { queue_id: 13 })
    .await;
  rig.source.skip_to_index(9).await.unwrap();
  tokio::time::sleep(Duration::from_millis(50)).await;
  assert_eq!(rig.backend.controls().len(), 1, "an out-of-range index is dropped");
}

#[tokio::test]
async fn full_fat_session_state_maps_onto_the_wire_snapshot() {
  let rig = boot(Vec::new()).await;
  let mut session = playing("Video", "Creator", "com.google.android.youtube");
  session.shuffle = Some(true);
  session.repeat = Some(MediaRepeatMode::All);
  session.speed = Some(1.5);
  session.position_age_ms = Some(40);
  session.liked = Some(true);
  session.like_supported = true;
  session.queue_title = Some("My Mix".into());
  rig.backend.emit(vec![session]);

  let state = rig.peer.wait("the snapshot", snapshot_of).await;
  assert!(state.playback.shuffle);
  assert_eq!(state.playback.shuffle_mode, Some(ShuffleMode::Songs));
  assert_eq!(state.playback.repeat, RepeatMode::All);
  assert_eq!(state.playback.position_age_ms, Some(40));
  assert_eq!(state.options.speed, 1.5);
  let track = state.track.as_ref().unwrap();
  assert_eq!(track.liked, Some(true));
  assert_eq!(track.is_like_supported, Some(true));
  assert_eq!(state.context.as_ref().unwrap().name.as_deref(), Some("My Mix"));
}

#[tokio::test]
async fn unknown_compat_state_degrades_to_the_neutral_card() {
  let rig = boot(Vec::new()).await;
  rig
    .backend
    .emit(vec![playing("Video", "Creator", "com.google.android.youtube")]);

  let state = rig.peer.wait("the snapshot", snapshot_of).await;
  assert!(!state.playback.shuffle);
  assert_eq!(state.playback.shuffle_mode, None);
  assert_eq!(state.playback.repeat, RepeatMode::Off);
  assert_eq!(state.options.speed, 1.0);
  let track = state.track.as_ref().unwrap();
  assert_eq!(track.liked, None);
  assert_eq!(track.is_like_supported, None);
  assert!(state.context.is_none());
}

#[tokio::test]
async fn transport_verbs_and_setters_delegate_to_the_audible_session() {
  let rig = boot(Vec::new()).await;
  rig
    .backend
    .emit(vec![playing("Video", "Creator", "com.google.android.youtube")]);
  rig.peer.wait("the snapshot", snapshot_of).await;

  rig.source.pause().await.unwrap();
  rig.source.resume().await.unwrap();
  rig.source.skip_next().await.unwrap();
  rig.source.skip_prev().await.unwrap();
  rig.source.seek_to(5000).await.unwrap();
  rig.source.set_shuffle(true).await.unwrap();
  rig.source.set_repeat(RepeatMode::One).await.unwrap();
  rig.source.set_speed(1.5).await.unwrap();

  rig.backend.wait_control(MediaControl::SetSpeed { speed: 1.5 }).await;
  let cmds: Vec<MediaControl> = rig.backend.controls().iter().map(|(_, cmd)| *cmd).collect();
  assert_eq!(
    cmds,
    vec![
      MediaControl::Pause,
      MediaControl::Play,
      MediaControl::SkipNext,
      MediaControl::SkipPrev,
      MediaControl::SeekTo { position_ms: 5000 },
      MediaControl::SetShuffle { on: true },
      MediaControl::SetRepeat {
        mode: MediaRepeatMode::One
      },
      MediaControl::SetSpeed { speed: 1.5 },
    ]
  );
  let controls = rig.backend.controls();
  assert!(controls.iter().all(|(p, _)| p == "com.google.android.youtube"));
}

#[tokio::test]
async fn a_like_for_the_system_uri_routes_to_the_session_rating() {
  let rig = boot(Vec::new()).await;
  let mut session = playing("Video", "Creator", "com.google.android.youtube");
  session.liked = Some(false);
  session.like_supported = true;
  rig.backend.emit(vec![session.clone()]);
  let state = rig.peer.wait("the snapshot", snapshot_of).await;
  let uri = state.track.as_ref().unwrap().uri.clone().unwrap();

  let owner = rig.hub.for_uri(&uri).expect("the system source claims system uris");
  owner
    .favorites_set(
      ItemRef {
        uri: uri.clone(),
        kind: ItemKind::Track,
        persistent_id: None,
      },
      true,
    )
    .await
    .unwrap();
  rig.backend.wait_control(MediaControl::SetLiked { liked: true }).await;

  session.liked = Some(true);
  rig.backend.emit(vec![session]);
  rig
    .peer
    .wait("the liked snapshot", |msg| {
      snapshot_of(msg).filter(|state| state.track.as_ref().is_some_and(|track| track.liked == Some(true)))
    })
    .await;
  owner
    .favorites_toggle(ItemRef {
      uri,
      kind: ItemKind::Track,
      persistent_id: None,
    })
    .await
    .unwrap();
  rig.backend.wait_control(MediaControl::SetLiked { liked: false }).await;
}

#[tokio::test]
async fn the_fallback_claims_no_scheme_and_never_serves_the_library() {
  let rig = boot(Vec::new()).await;
  rig
    .backend
    .emit(vec![playing("Video", "Creator", "com.google.android.youtube")]);
  rig.peer.wait("the snapshot", snapshot_of).await;

  assert!(
    rig.hub.attached_schemes().is_empty(),
    "system is not an announced scheme"
  );
  assert!(
    rig.hub.library().is_none(),
    "the fallback cannot become the library pick"
  );
}

#[tokio::test]
async fn revoked_access_reports_no_sessions() {
  let rig = boot(Vec::new()).await;
  *rig.backend.granted.lock().unwrap() = false;
  rig
    .backend
    .emit(vec![playing("Video", "Creator", "com.google.android.youtube")]);

  rig.peer.quiet("a snapshot without access", snapshot_of).await;
}
