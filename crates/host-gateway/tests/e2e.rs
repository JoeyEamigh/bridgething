use std::{
  io::Write,
  path::{Path, PathBuf},
  time::Duration,
};

use bridgething_delivery::session::DeliverySession;
use bridgething_host_gateway::{
  chaos::ChaosConfig,
  host_state, install,
  ota::{self, PushRequest, PushShape},
  session, webapp,
};
use bridgething_test_harness::Harness;
use libbridgething::{OtaKind, gateway::HostState};
use uuid::Uuid;

const ARTIFACT_BYTES: usize = 512 * 1024;

const DEADLINE: Duration = Duration::from_secs(60);

async fn session(harness: &Harness) -> DeliverySession {
  conditioned_session(harness, ChaosConfig::default()).await
}

async fn conditioned_session(harness: &Harness, chaos: ChaosConfig) -> DeliverySession {
  let io = harness.connect_android_io().await.expect("a link to the daemon");
  session::from_io(io, chaos)
    .await
    .expect("the session announces and adopts")
}

fn write_artifact(dir: &Path, name: &str, len: usize) -> PathBuf {
  let path = dir.join(name);
  let body: Vec<u8> = (0..len).map(|at| (at % 251) as u8).collect();
  std::fs::write(&path, body).expect("the artifact spools");
  path
}

fn write_bundle(dir: &Path, id: Uuid) -> PathBuf {
  let path = dir.join("bundle.zip");
  let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).expect("create the bundle"));
  let opts = zip::write::SimpleFileOptions::default();
  zip.start_file("index.html", opts).expect("start index.html");
  zip
    .write_all(b"<!doctype html><title>e2e</title>")
    .expect("write index");
  zip.start_file("manifest.json", opts).expect("start manifest.json");
  zip
    .write_all(format!(r#"{{"id":"{id}","name":"e2e","version":"0.1.0","config":[],"permissions":[]}}"#).as_bytes())
    .expect("write manifest");
  zip.finish().expect("finish the bundle");
  path
}

#[tokio::test(flavor = "multi_thread")]
async fn a_push_completes_against_the_harness_daemon() {
  let harness = Harness::start().await.expect("the headless daemon boots");
  let session = session(&harness).await;
  let spool = tempfile::tempdir().expect("a scratch directory");
  let artifact = write_artifact(spool.path(), "daemon", ARTIFACT_BYTES);

  tokio::time::timeout(
    DEADLINE,
    ota::push(
      &session,
      PushRequest {
        kind: OtaKind::Daemon,
        artifact,
        shape: PushShape::Whole,
        update_url_base: None,
        zcks: Default::default(),
        version: None,
      },
    ),
  )
  .await
  .expect("the push ended rather than parking")
  .expect("the push completed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_compressed_push_completes_against_the_harness_daemon() {
  let harness = Harness::start().await.expect("the headless daemon boots");
  let session = session(&harness).await;
  let spool = tempfile::tempdir().expect("a scratch directory");
  let artifact = write_artifact(spool.path(), "daemon", ARTIFACT_BYTES);

  tokio::time::timeout(
    DEADLINE,
    ota::push(
      &session,
      PushRequest {
        kind: OtaKind::Daemon,
        artifact,
        shape: PushShape::Compressed,
        update_url_base: None,
        zcks: Default::default(),
        version: None,
      },
    ),
  )
  .await
  .expect("the push ended rather than parking")
  .expect("a zstd-wrapped artifact is applied the same way a released one is");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_install_lands_the_bundle_and_it_can_then_be_switched_to() {
  let harness = Harness::start().await.expect("the headless daemon boots");
  let session = session(&harness).await;
  let spool = tempfile::tempdir().expect("a scratch directory");
  let id = Uuid::now_v7();
  let bundle = write_bundle(spool.path(), id);

  tokio::time::timeout(
    DEADLINE,
    install::install(&session, &bundle, Some("https://apps.bridgething.test/catalog.json")),
  )
  .await
  .expect("the install ended rather than parking")
  .expect("the daemon installed the bundle");

  let installed = harness.state().webapps.list().await;
  let entry = installed
    .iter()
    .find(|info| info.id == id)
    .expect("the daemon's own registry holds the bundle it just installed");
  assert_eq!(
    entry.provenance.as_deref(),
    Some("https://apps.bridgething.test/catalog.json"),
    "the provenance the install carried reached the registry"
  );

  tokio::time::timeout(DEADLINE, webapp::switch(&session, id))
    .await
    .expect("the switch ended rather than parking")
    .expect("a freshly installed bundle can be activated");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_dies_mid_push_fails_the_push_rather_than_parking_it() {
  let harness = Harness::start().await.expect("the headless daemon boots");
  let session = conditioned_session(
    &harness,
    ChaosConfig {
      inject_disconnect: Some(Duration::from_millis(150)),
      ..ChaosConfig::default()
    },
  )
  .await;
  let spool = tempfile::tempdir().expect("a scratch directory");
  let artifact = write_artifact(spool.path(), "daemon", 64 * 1024 * 1024);

  let outcome = tokio::time::timeout(
    DEADLINE,
    ota::push(
      &session,
      PushRequest {
        kind: OtaKind::Daemon,
        artifact,
        shape: PushShape::Whole,
        update_url_base: None,
        zcks: Default::default(),
        version: None,
      },
    ),
  )
  .await
  .expect("a dead link ends the push rather than parking on the watchdog");

  assert!(
    outcome.is_err(),
    "a push over a link that died is not a completed update"
  );
}

#[tokio::test]
async fn a_following_host_dims_the_device_at_link_up_and_its_departure_leaves_it_dimmed() {
  let harness = Harness::start().await.expect("harness start");

  let (gateway, inbound) = harness
    .connect_android_subscribed()
    .await
    .expect("a link to the daemon");
  let away = HostState {
    away: Some(true),
    idle: false,
  };
  let serving = tokio::spawn(async move { host_state::serve(&gateway, inbound, away).await });
  assert!(
    harness
      .wait_for(|state| state.display.snapshot().brightness.dimmed, DEADLINE)
      .await,
    "a following host reports without being asked"
  );

  serving.abort();
  let _ = serving.await;
  assert!(
    harness
      .wait_for(
        |state| state
          .peers
          .snapshot()
          .peers
          .values()
          .all(|peer| peer.companion == libbridgething::PeerCompanionStatus::None),
        DEADLINE
      )
      .await,
    "the link went with the tool"
  );
  assert!(harness.state().display.snapshot().brightness.dimmed);
}
