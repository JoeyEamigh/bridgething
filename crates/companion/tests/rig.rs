#[path = "rig/backends.rs"]
mod backends;
#[path = "rig/extension_host.rs"]
mod extension_host;
#[path = "rig/fakes.rs"]
mod fakes;
#[path = "rig/install.rs"]
mod install;
#[path = "rig/log_sink.rs"]
mod log_sink;
#[path = "rig/secrets.rs"]
mod secrets;
#[path = "rig/support.rs"]
mod support;

use std::{
  io::Write,
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::{
  api::{PeerLinkStatus, SessionEvent},
  backend::{ExtensionHost, ExtensionMessage, VolumeBackend, VolumeInbox, VolumeLevel},
  provider::Provider,
};
use bridgething_delivery::ota::{event::OtaPhaseSnapshot, stream::FileSource};
use bridgething_gateway::route;
use libbridgething::{CompanionAuthorityScope, MediaItem, Playback, PlaybackState, PlayerState};
use serde::Serialize;

use crate::{
  extension_host::{FakeExtensionHost, HostCall},
  fakes::FakeSource,
  install::{BlockingSink, RecordingSink, Serving},
  support::{Rig, WireEntry},
};

#[derive(Default)]
pub struct FakeVolume {
  inbox: Mutex<Option<Arc<VolumeInbox>>>,
  level: Mutex<VolumeLevel>,
}

impl FakeVolume {
  pub fn resting_at(level: f32) -> Arc<Self> {
    Arc::new(Self {
      inbox: Mutex::new(None),
      level: Mutex::new(VolumeLevel { level, muted: false }),
    })
  }

  pub fn moved_to(&self, level: f32) {
    *self.level.lock().unwrap() = VolumeLevel { level, muted: false };
    let inbox = self.inbox.lock().unwrap().clone();
    if let Some(inbox) = inbox {
      inbox.on_changed(level, false);
    }
  }
}

impl VolumeBackend for FakeVolume {
  fn start(&self, inbox: Arc<VolumeInbox>) {
    *self.inbox.lock().unwrap() = Some(inbox);
  }

  fn stop(&self) {
    self.inbox.lock().unwrap().take();
  }

  fn snapshot(&self) -> VolumeLevel {
    *self.level.lock().unwrap()
  }

  fn set_volume(&self, level: f32) {
    self.moved_to(level);
  }

  fn set_mute(&self, muted: bool) {
    self.level.lock().unwrap().muted = muted;
  }

  fn volume_up(&self) {}
  fn volume_down(&self) {}
  fn mute_toggle(&self) {}
}

const ARTIFACT_BYTES: usize = 512 * 1024;

const DRIVE_DEADLINE: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_secs(5);

fn playing(title: &str) -> PlayerState {
  PlayerState {
    track: Some(MediaItem {
      title: Some(title.into()),
      artist: Some("the rig".into()),
      ..MediaItem::default()
    }),
    playback: Playback {
      state: PlaybackState::Playing,
      ..Playback::default()
    },
    ..PlayerState::default()
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_announces_itself_and_the_daemon_learns_what_the_host_can_do() {
  let rig = Rig::start().await;

  assert!(
    rig
      .await_event(SETTLE, |event| matches!(
        event,
        SessionEvent::PeerConnected { peer } if peer.status == PeerLinkStatus::Connected
      ))
      .await,
    "the host is told a peer connected"
  );

  assert!(
    rig
      .harness
      .wait_for(
        |state| state
          .capabilities
          .snapshot()
          .gateway
          .is_some_and(|info| info.app_name == "rig"),
        SETTLE,
      )
      .await,
    "the daemon adopted the announce and knows who the host is"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_inbound_request_surface_answers_within_the_deadline() {
  let rig = Rig::start().await;
  rig.settle().await;

  let peer = rig.session.peer_for(rig.device_id()).expect("the link is up");
  let gateway = rig.gateway();
  for probe in support::probes() {
    let name = probe.name;
    let answered = tokio::time::timeout(SETTLE, route(&peer, probe.msg, gateway.connection())).await;
    assert!(
      answered.is_ok(),
      "{name} left the device waiting: an inbound request surface that never returns is a hang, \
       and refusing is an answer"
    );
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_providers_now_playing_reaches_both_the_device_and_the_host() {
  let rig = Rig::start().await;
  let source: Arc<FakeSource> = FakeSource::new("rig");
  rig
    .session
    .add_provider(source.clone() as Arc<dyn Provider>)
    .await
    .expect("the provider attaches");
  rig.settle().await;

  source.submit(playing("the litmus track"));

  assert!(
    rig
      .await_event(SETTLE, |event| matches!(
        event,
        SessionEvent::NowPlayingChanged { now_playing: Some(now) }
          if now.track.as_ref().and_then(|track| track.title.as_deref()) == Some("the litmus track")
      ))
      .await,
    "the host hears the hub's arbitrated voice"
  );

  assert!(
    rig
      .harness
      .wait_for(
        |state| state
          .player
          .state_reply()
          .state
          .track
          .and_then(|track| track.title)
          .as_deref()
          == Some("the litmus track"),
        SETTLE,
      )
      .await,
    "and so does the daemon"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_update_drive_completes_against_the_real_daemon() {
  let rig = Rig::start().await;
  rig.settle().await;
  let artifact = rig.write_artifact("daemon", ARTIFACT_BYTES);

  let terminal = tokio::time::timeout(
    DRIVE_DEADLINE,
    rig
      .session
      .ota()
      .push_daemon(rig.device_id(), Arc::new(FileSource::open(artifact.clone())), None),
  )
  .await
  .expect("the drive ended rather than parking on the watchdog");

  assert_eq!(
    terminal,
    OtaPhaseSnapshot::Completed,
    "the daemon staged the piece, took the activate and reported reboot"
  );

  let said = rig.said();
  assert_eq!(
    said.acked.last().copied(),
    Some(ARTIFACT_BYTES as u32),
    "the daemon acked every byte of the artifact, got {:?}",
    said.acked
  );
  assert!(
    said.acked.len() > 1,
    "a paced push is acked as it lands, not once at the end, got {:?}",
    said.acked
  );
  assert!(
    said.acked.windows(2).all(|pair| pair[0] <= pair[1]),
    "acks are cumulative and never rewind, got {:?}",
    said.acked
  );
  assert!(
    said.phases.contains(&libbridgething::OtaPhase::Writing),
    "the daemon reported the apply, got {:?}",
    said.phases
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_push_of_the_same_artifact_resumes_rather_than_restarting() {
  let rig = Rig::start().await;
  rig.settle().await;
  let artifact = rig.write_artifact("daemon", ARTIFACT_BYTES);

  assert_eq!(
    tokio::time::timeout(
      DRIVE_DEADLINE,
      rig
        .session
        .ota()
        .push_daemon(rig.device_id(), Arc::new(FileSource::open(artifact.clone())), None)
    )
    .await
    .expect("the first drive ended"),
    OtaPhaseSnapshot::Completed
  );
  let first_acks = rig.said().acked.len();

  assert_eq!(
    tokio::time::timeout(
      DRIVE_DEADLINE,
      rig
        .session
        .ota()
        .push_daemon(rig.device_id(), Arc::new(FileSource::open(artifact.clone())), None)
    )
    .await
    .expect("the second drive ended"),
    OtaPhaseSnapshot::Completed,
    "the same artifact pushed twice is not a stuck update"
  );

  let said = rig.said();
  assert!(
    said.acked.len() > first_acks,
    "the second drive is a real transfer of its own, not a replay of the first"
  );
  assert_eq!(
    said.acked.last().copied(),
    Some(ARTIFACT_BYTES as u32),
    "and it also ends on the whole artifact"
  );
}

const TRANSCRIPT_SCENARIO: &[&str] = &[
  "the link comes up and both sides announce",
  "a provider attaches and publishes a track the daemon adopts",
  "every inbound request surface is routed once, in probe order, each driven to its reply",
];

const TRANSCRIPT_NORMALIZED: &[&str] = &[
  "request ids are sequence numbers in first-seen order; a reply carries the number of the request it answers",
  "commands and events carry no id at all: theirs is a fresh uuid per run and correlates nothing",
  "payloads are not recorded, so no uuid, clock reading or daemon-side identifier enters the fixture",
  "within a direction nothing is reordered: every step is driven to its wire effect before the next \
   one starts, and the two concurrent request arms (asset.request, system.otaAssetRange) are awaited \
   on the wire like the inline ones",
  "the two directions are recorded as two lists rather than one interleaving: a lane is ordered, but \
   nothing orders a frame the daemon composed against one the session composed at the same moment, \
   and the daemon's ancs echo of an announce lands on either side of the connect sequence's \
   time.snapshot depending on how loaded the box is",
  "system.deviceNicknameChanged is dropped: the daemon's nickname observer broadcasts once when it \
   spawns, to whoever is connected at that instant, so whether the rig's link exists yet is a race \
   between daemon bring-up and the first gateway connection and has nothing to do with the session",
  "notifications.ancsAuthStateChanged is dropped: the daemon echoes one per capabilities announce, \
   composed on its own task, so its place among the replies the daemon writes at the same moment \
   (the connect-time webapp shelf fetch) is scheduler order, not protocol order",
];

const DROPPED: &[&str] = &["system.deviceNicknameChanged", "notifications.ancsAuthStateChanged"];

#[derive(Debug, Serialize)]
struct TranscriptSnapshot {
  scenario: Vec<String>,
  normalized: Vec<String>,
  #[serde(rename = "toHost")]
  to_host: Vec<WireEntry>,
  #[serde(rename = "toDevice")]
  to_device: Vec<WireEntry>,
}

fn transcript_fixture() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/session-transcript.snap.json")
}

async fn scripted_session() -> Vec<WireEntry> {
  let rig = Rig::recording().await;

  assert!(
    rig
      .await_event(SETTLE, |event| matches!(event, SessionEvent::PeerConnected { .. }))
      .await,
    "the link came up"
  );
  assert!(
    rig
      .harness
      .wait_for(
        |state| state
          .capabilities
          .snapshot()
          .gateway
          .is_some_and(|info| info.app_name == "rig"),
        SETTLE,
      )
      .await,
    "the daemon adopted the announce"
  );

  let source: Arc<FakeSource> = FakeSource::new("rig");
  rig
    .session
    .add_provider(source.clone() as Arc<dyn Provider>)
    .await
    .expect("the provider attaches");
  source.submit(playing("the transcript track"));
  assert!(
    rig
      .harness
      .wait_for(
        |state| state
          .player
          .state_reply()
          .state
          .track
          .and_then(|track| track.title)
          .as_deref()
          == Some("the transcript track"),
        SETTLE,
      )
      .await,
    "the daemon adopted the track"
  );

  let peer = rig.session.peer_for(rig.device_id()).expect("the link is up");
  let gateway = rig.gateway();
  for probe in support::probes() {
    let want = rig.replies() + 1;
    route(&peer, probe.msg, gateway.connection())
      .await
      .expect("the routing path accepted the request");
    assert!(
      rig.await_replies(SETTLE, want).await,
      "{} never put its reply on the wire",
      probe.name
    );
  }

  rig
    .transcript()
    .into_iter()
    .filter(|entry| !DROPPED.contains(&entry.msg.as_str()))
    .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_scripted_session_puts_the_same_frames_on_the_wire_every_time() {
  let first = scripted_session().await;
  let second = scripted_session().await;

  let render = |frames: &[WireEntry]| {
    let of = |dir: &str| frames.iter().filter(|entry| entry.dir == dir).cloned().collect();
    serde_json::to_string_pretty(&TranscriptSnapshot {
      scenario: TRANSCRIPT_SCENARIO.iter().map(|line| (*line).to_owned()).collect(),
      normalized: TRANSCRIPT_NORMALIZED.iter().map(|line| (*line).to_owned()).collect(),
      to_host: of(support::TO_HOST),
      to_device: of(support::TO_DEVICE),
    })
    .expect("the transcript renders")
      + "\n"
  };

  let rendered = render(&first);
  assert_eq!(
    rendered,
    render(&second),
    "two runs of one script disagreed on what crossed the link, which is a race in the assembly, \
     not a fixture to be relaxed"
  );

  let fixture = transcript_fixture();
  if std::env::var("UPDATE_TRANSCRIPT").is_ok() {
    std::fs::create_dir_all(fixture.parent().expect("the fixture has a directory")).expect("the fixture dir exists");
    std::fs::write(&fixture, &rendered).expect("the fixture writes");
    return;
  }

  let held = std::fs::read_to_string(&fixture).expect("the committed transcript exists; UPDATE_TRANSCRIPT=1 writes it");
  assert_eq!(
    held, rendered,
    "the session's wire conversation moved; re-read the diff before running with UPDATE_TRANSCRIPT=1"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_snapshot_is_the_authority_and_survives_the_events_that_hinted_at_it() {
  let rig = Rig::start().await;
  rig.settle().await;

  let snapshot = rig.session.snapshot().await;
  assert_eq!(snapshot.peers.len(), 1, "the live link is a peer");
  assert_eq!(snapshot.peers[0].status, PeerLinkStatus::Connected);
  assert_eq!(snapshot.host_info.app_name, "rig");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_the_device_confirms_comes_back_as_fresh_device_meta() {
  let rig = Rig::start().await;
  rig.settle().await;

  assert!(
    rig
      .await_event(SETTLE, |event| matches!(event, SessionEvent::DeviceMetaChanged { .. }))
      .await,
    "the daemon's announce seeded the first meta"
  );

  rig
    .gateway()
    .system()
    .device_set_nickname(libbridgething::gateway::DeviceSetNickname {
      nickname: "garage thing".into(),
    })
    .await
    .expect("the daemon accepts the rename");

  assert!(
    rig
      .await_event(SETTLE, |event| matches!(
        event,
        SessionEvent::DeviceMetaChanged { meta, .. } if meta.nickname.as_deref() == Some("garage thing")
      ))
      .await,
    "the broadcast the daemon answers with lands back on the host as device meta, or every screen \
     keeps showing the old name"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_devices_webapp_shelf_arrives_with_the_connection() {
  let rig = Rig::start().await;
  rig.settle().await;

  assert!(
    rig
      .await_event(SETTLE, |event| matches!(event, SessionEvent::WebappsChanged { .. }))
      .await,
    "connecting fetches the device's installed webapps; without the seed the shelf only ever \
     hears install deltas and a device with apps reads as empty"
  );
}

async fn plant_webapp(rig: &Rig, name: &str) -> uuid::Uuid {
  let id = uuid::Uuid::now_v7();
  let dir = rig.harness.state_dir().join("webapps").join(id.simple().to_string());
  std::fs::create_dir_all(&dir).expect("bundle dir");
  std::fs::write(dir.join("index.html"), b"<h1>planted</h1>").expect("index");
  std::fs::write(
    dir.join("manifest.json"),
    format!(r#"{{"id":"{id}","name":"{name}","version":"0.1.0"}}"#),
  )
  .expect("manifest");
  rig.harness.state().webapps.rescan().await;
  id
}

async fn plant_configured_webapp(rig: &Rig, name: &str, key: &str) -> uuid::Uuid {
  let id = uuid::Uuid::now_v7();
  let dir = rig.harness.state_dir().join("webapps").join(id.simple().to_string());
  std::fs::create_dir_all(&dir).expect("bundle dir");
  std::fs::write(dir.join("index.html"), b"<h1>planted</h1>").expect("index");
  std::fs::write(
    dir.join("manifest.json"),
    format!(
      r#"{{"id":"{id}","name":"{name}","version":"0.1.0","config":[{{"type":"string","data":{{"key":"{key}","label":"{key}"}}}}]}}"#
    ),
  )
  .expect("manifest");
  rig.harness.state().webapps.rescan().await;
  id
}

async fn rig_with_host() -> (Rig, Arc<FakeExtensionHost>) {
  let host = Arc::new(FakeExtensionHost::default());
  let rig = Rig::with_extension_host(host.clone() as Arc<dyn ExtensionHost>).await;
  rig.settle().await;
  (rig, host)
}

async fn activate(rig: &Rig, id: uuid::Uuid) {
  rig
    .gateway()
    .webapp()
    .switch_to(libbridgething::gateway::WebappSwitchTo { id })
    .await
    .expect("the daemon switches the active webapp");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_learns_about_the_device_and_which_webapp_is_active_on_it() {
  let (rig, host) = rig_with_host().await;

  assert!(
    host
      .await_call(
        SETTLE,
        |call| matches!(call, HostCall::DeviceConnected { device, .. } if device == "rig-device")
      )
      .await,
    "the host is told the link came up, so an extension can push the moment a device appears"
  );

  let app = plant_webapp(&rig, "extended").await;
  activate(&rig, app).await;

  assert!(
    host
      .await_call(SETTLE, |call| matches!(
        call,
        HostCall::DeviceActive { webapp, active: true, .. } if *webapp == app.to_string()
      ))
      .await,
    "an active-webapp change on the device reaches the host"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_running_set_a_host_reports_is_what_makes_forward_available_on_the_daemon() {
  let (rig, host) = rig_with_host().await;
  let app = plant_webapp(&rig, "extended").await;
  activate(&rig, app).await;

  assert!(
    !rig.harness.state().capabilities.snapshot().available.forward,
    "nothing is running yet"
  );

  host.inbox(SETTLE).await.running_changed(vec![app.to_string()]);

  assert!(
    rig
      .harness
      .wait_for(|state| state.capabilities.snapshot().available.forward, SETTLE)
      .await,
    "the host's running set crosses the link as extensionsRunning and the daemon derives forward from it"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webapp_forward_travels_the_whole_way_to_the_host() {
  let (rig, host) = rig_with_host().await;
  let app = plant_webapp(&rig, "extended").await;
  activate(&rig, app).await;

  let client = rig
    .harness
    .connect_command_client()
    .await
    .expect("a webapp connects to the daemon");
  client
    .event(libbridgething::ForwardMessage::Text("hello, extension".into()))
    .await
    .expect("the webapp forwards");

  assert!(
    host
      .await_call(SETTLE, |call| matches!(
        call,
        HostCall::Delivered { webapp, message: ExtensionMessage::Text { text }, .. }
          if *webapp == app.to_string() && text == "hello, extension"
      ))
      .await,
    "the daemon stamped the active webapp and the companion routed it into the host"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_send_reaches_the_webapp_that_is_active_on_the_device() {
  let (rig, host) = rig_with_host().await;
  let app = plant_webapp(&rig, "extended").await;
  activate(&rig, app).await;

  let client = rig
    .harness
    .connect_command_client()
    .await
    .expect("a webapp connects to the daemon");
  let mut inbound = client.events();

  host.inbox(SETTLE).await.send_to_device(
    None,
    app.to_string(),
    ExtensionMessage::Json {
      json: r#"{"kind":"tick"}"#.into(),
    },
  );

  let delivered = tokio::time::timeout(SETTLE, async {
    loop {
      let msg = inbound.recv().await.expect("the client link is alive");
      if let libbridgething::client::BridgeToClientMsgData::Forward(message) = msg.data {
        return message;
      }
    }
  })
  .await
  .expect("an unaddressed host send fans out to every device where the webapp is active");

  assert_eq!(
    delivered,
    libbridgething::ForwardMessage::Json(serde_json::json!({ "kind": "tick" })),
    "json survives the stdio-shaped string hop and lands as json on the webapp leg"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_config_write_from_any_companion_is_reported_to_the_host() {
  let (rig, host) = rig_with_host().await;
  let app = plant_configured_webapp(&rig, "extended", "zip").await;

  rig
    .gateway()
    .webapp()
    .config_set(libbridgething::gateway::WebappConfigSet {
      id: app,
      key: "zip".into(),
      value: "94110".into(),
    })
    .await
    .expect("the daemon takes the write");

  assert!(
    host
      .await_call(SETTLE, |call| matches!(
        call,
        HostCall::ConfigChanged { webapp, key, value: Some(value), .. }
          if *webapp == app.to_string() && key == "zip" && value == "94110"
      ))
      .await,
    "the daemon announces every settings write to every gateway, so a phone's write reaches the desktop's extension"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_host_the_forward_surface_still_answers_unsupported() {
  let rig = Rig::start().await;
  rig.settle().await;
  let peer = rig.session.peer_for(rig.device_id()).expect("the link is up");

  let refused = bridgething_gateway::ForwardHandler::routed(
    peer.as_ref(),
    libbridgething::ForwardRouted {
      webapp: uuid::Uuid::now_v7(),
      message: libbridgething::ForwardMessage::Text("nobody home".into()),
    },
  )
  .await;

  assert_eq!(
    refused,
    Err(libbridgething::wire::WireError::Unsupported),
    "a companion with no extension host must keep refusing forwards, not swallow them"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_is_told_the_device_left_and_then_that_it_should_stop() {
  let (rig, host) = rig_with_host().await;
  assert!(
    host
      .await_call(SETTLE, |call| matches!(call, HostCall::DeviceConnected { .. }))
      .await,
    "the link came up first"
  );

  rig.session.stop().await;

  assert!(
    host
      .await_call(
        SETTLE,
        |call| matches!(call, HostCall::DeviceDisconnected { device } if device == "rig-device")
      )
      .await,
    "an extension has to hear the device leave or it keeps pushing into a dead link"
  );
  assert!(
    host.calls().iter().any(|call| matches!(call, HostCall::Stopped)),
    "session teardown has to await the host stopping, or the sidecars outlive the session, got {:?}",
    host.calls()
  );

  let calls = host.calls();
  let position = |wanted: fn(&HostCall) -> bool| calls.iter().position(wanted);
  assert!(
    position(|call| matches!(call, HostCall::DeviceConnected { .. }))
      < position(|call| matches!(call, HostCall::DeviceDisconnected { .. })),
    "the host sees connect before disconnect, got {calls:?}"
  );
  assert!(
    position(|call| matches!(call, HostCall::DeviceDisconnected { .. }))
      < position(|call| matches!(call, HostCall::Stopped)),
    "and the device leaves before the host is told to halt, got {calls:?}"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_extensions_that_just_started_hear_the_device_connect_again() {
  let (rig, host) = rig_with_host().await;
  let first = plant_webapp(&rig, "first").await;
  let second = plant_webapp(&rig, "second").await;
  activate(&rig, first).await;

  let inbox = host.inbox(SETTLE).await;
  inbox.running_changed(vec![first.to_string()]);
  assert!(
    host
      .await_call(SETTLE, |call| matches!(
        call,
        HostCall::DeviceConnected { webapps: ids, .. } if ids == &vec![first.to_string()]
      ))
      .await,
    "the extension that just started missed the link-up announce and has to be told"
  );

  inbox.running_changed(vec![first.to_string(), second.to_string()]);
  assert!(
    host
      .await_call(SETTLE, |call| matches!(
        call,
        HostCall::DeviceConnected { webapps: ids, .. } if ids == &vec![second.to_string()]
      ))
      .await,
    "the second extension starting is announced to the second extension"
  );

  inbox.running_changed(vec![second.to_string()]);
  assert!(
    rig
      .harness
      .wait_for(|state| !state.capabilities.snapshot().available.forward, SETTLE)
      .await,
    "the daemon saw the first extension leave the running set"
  );

  let announced: Vec<_> = host
    .calls()
    .into_iter()
    .filter(|call| matches!(call, HostCall::DeviceConnected { webapps, .. } if webapps.contains(&first.to_string())))
    .collect();
  assert_eq!(
    announced.len(),
    1,
    "the link-up announce read no settings for an extension that was not running yet, so the announce \
     when it started is the only connect naming it, got {announced:?}"
  );
}

fn webapp_zip(id: uuid::Uuid, name: &str) -> Vec<u8> {
  let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
  let options = zip::write::SimpleFileOptions::default();
  for (entry, body) in [
    ("index.html", "<h1>installed</h1>".to_owned()),
    (
      "manifest.json",
      format!(r#"{{"id":"{id}","name":"{name}","version":"0.1.0","config":[],"permissions":[]}}"#),
    ),
  ] {
    zip.start_file(entry, options).expect("an entry starts");
    zip.write_all(body.as_bytes()).expect("an entry writes");
  }
  zip.finish().expect("the archive closes").into_inner()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_sink_is_handed_the_artifact_while_it_is_still_on_disk() {
  const URL: &str = "https://apps.bridgething.test/installable.zip";
  let id = uuid::Uuid::now_v7();
  let http = Serving::new();
  http.stage(URL, webapp_zip(id, "sinkable"));

  let rig = Rig::with_http(http).await;
  let sink = RecordingSink::new();
  let installed = rig
    .companion
    .install_webapp_from_url(
      rig.device_id().to_owned(),
      URL.to_owned(),
      None,
      None,
      Some(sink.clone()),
    )
    .await
    .expect("the device installs the bundle");
  assert_eq!(installed.id, id.to_string());

  let calls = sink.calls();
  let [(bundle, on_disk)] = calls.as_slice() else {
    panic!("the sink is handed the artifact exactly once, got {calls:?}");
  };
  assert!(
    *on_disk,
    "a sink that unpacks the bundle needs it to still exist when it is called"
  );
  assert!(
    !Path::new(bundle).exists(),
    "and the download is cleaned up once the install returns"
  );
}

#[tokio::test(flavor = "current_thread")]
async fn the_bundle_sink_runs_off_the_worker_that_is_driving_the_link() {
  const URL: &str = "https://apps.bridgething.test/blocking.zip";
  let id = uuid::Uuid::now_v7();
  let http = Serving::new();
  http.stage(URL, webapp_zip(id, "blocking"));

  let rig = Rig::with_http(http).await;
  let (sink, gate) = BlockingSink::new();
  let opener = tokio::spawn(async move {
    let deadline = tokio::time::Instant::now() + SETTLE;
    while tokio::time::Instant::now() < deadline {
      if gate.entered.try_recv().is_ok() {
        let _ = gate.release.send(());
        return;
      }
      tokio::time::sleep(Duration::from_millis(1)).await;
    }
  });

  rig
    .companion
    .install_webapp_from_url(
      rig.device_id().to_owned(),
      URL.to_owned(),
      None,
      None,
      Some(sink.clone()),
    )
    .await
    .expect("the device installs the bundle");

  assert_eq!(
    sink.freed(),
    vec![true],
    "a sink unpacks the artifact, so running it inline wedges every link this runtime is driving until it returns"
  );
  opener.await.expect("the releasing task finished");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_install_that_fails_never_reaches_the_bundle_sink() {
  const URL: &str = "https://apps.bridgething.test/not-a-bundle.zip";
  let http = Serving::new();
  http.stage(URL, b"this is not an archive".to_vec());

  let rig = Rig::with_http(http).await;
  let sink = RecordingSink::new();
  let refused = rig
    .companion
    .install_webapp_from_url(
      rig.device_id().to_owned(),
      URL.to_owned(),
      None,
      None,
      Some(sink.clone()),
    )
    .await;

  assert!(refused.is_err(), "the device cannot install that, got {refused:?}");
  assert!(
    sink.calls().is_empty(),
    "a host must not adopt an extension from a bundle the device rejected, got {:?}",
    sink.calls()
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_that_owns_its_volume_takes_the_scope_and_pushes_its_level_to_the_device() {
  let volume = FakeVolume::resting_at(0.4);
  let rig = Rig::with_volume(volume.clone() as Arc<dyn VolumeBackend>).await;
  let source: Arc<FakeSource> = FakeSource::new("rig");
  rig
    .session
    .add_provider(source.clone() as Arc<dyn Provider>)
    .await
    .expect("the provider attaches");
  rig.settle().await;

  source.submit(playing("the litmus track"));

  assert!(
    rig
      .harness
      .wait_for(
        |state| state.authority.is_authoritative(CompanionAuthorityScope::Volume),
        SETTLE,
      )
      .await,
    "a host with its own mixer claims volume for whatever is audible, even though the source does not own it"
  );

  volume.moved_to(0.8);

  let deadline = tokio::time::Instant::now() + SETTLE;
  loop {
    let announced = rig
      .transcript()
      .into_iter()
      .any(|entry| entry.dir == "toDevice" && entry.msg.contains("volumeChanged"));
    if announced {
      break;
    }
    assert!(
      tokio::time::Instant::now() < deadline,
      "the host mixer moved and the device was never told: {:?}",
      rig.transcript()
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
  }
}
