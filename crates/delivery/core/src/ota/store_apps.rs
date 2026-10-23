use std::{sync::Arc, time::Duration};

use libbridgething::{ExtensionInfo, OtaKind, WebappInfo, WebappRole, WebappSource};
use uuid::Uuid;

use crate::{
  bundle::fetch::sha256_hex,
  ota::{
    autopush::LINK_STABILITY_MS,
    event::OtaPollEvent,
    harness::{
      DEVICE, FakeDevice, FakeFetch, ManifestFixture, Spool, TestClock, digest_of, linked_gateway, meta, pattern,
      route_into,
    },
    poll::OtaPollConfig,
    service::{OtaService, OtaServiceDeps},
  },
};

const ROOT: &str = "https://ota.test";
const CHANNEL: &str = "stable";
const SOURCE: &str = "https://apps.example/catalog.json";
const OTHER_SOURCE: &str = "https://other.example/catalog.json";
const BUNDLE: &str = "https://apps.example/r/weather/1.1.0.zip";
const WEATHER: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001);
const QUIET: Duration = Duration::from_millis(500);

struct Rig {
  service: Arc<OtaService>,
  fetch: Arc<FakeFetch>,
  _spool: Spool,
  device: FakeDevice,
}

async fn rig() -> Rig {
  let fetch = FakeFetch::new();
  let spool = Spool::new();
  let service = OtaService::new(OtaServiceDeps {
    clock: TestClock::new(),
    fetch: fetch.clone(),
    cache_dir: spool.path().to_path_buf(),
    data_dir: Some(spool.path().join("state")),
  });
  let (gateway, device) = linked_gateway();
  service.adopt(DEVICE, gateway.clone()).await;
  route_into(&gateway, &service, DEVICE);
  device.announce_meta(meta("0.9.0", "2026.06.0", CHANNEL));
  tokio::time::advance(Duration::from_millis(LINK_STABILITY_MS + 1_000)).await;
  Rig {
    service,
    fetch,
    _spool: spool,
    device,
  }
}

fn config(auto_push: bool) -> OtaPollConfig {
  OtaPollConfig {
    root_url: ROOT.into(),
    interval_seconds: 86_400,
    auto_push,
  }
}

fn publish_current_firmware(fetch: &FakeFetch) {
  fetch.serve_text(
    &format!("{ROOT}/manifest.json"),
    ManifestFixture::new(CHANNEL, "0.9.0+image.2026.06.0").json(),
  );
}

fn publish_catalog(fetch: &FakeFetch, url: &str, extension: bool) -> Vec<u8> {
  let bundle = pattern(8 * 1024);
  fetch.serve_artifact(BUNDLE, bundle.clone());
  let digest = digest_of(&bundle);
  let extension = if extension {
    r#","extension":{"desktop":true,"permissions":[]}"#
  } else {
    ""
  };
  fetch.serve_text(
    url,
    format!(
      r#"{{"schema":"catalog.v1","updated_at":"2026-08-01T00:00:00Z","repo":{{"name":"example","description":"x","homepage":null,"icon":null}},"recommended_sources":[],"apps":[{{"id":"{WEATHER}","name":"weather","description":"x","author":"x","icon":null,"homepage":null,"source":null,"versions":[{{"version":"1.1.0","released_at":"2026-08-01T00:00:00Z","download":{{"url":"{BUNDLE}","size":{},"sha256":"{}"}},"permissions":[],"min_libbridgething_version":"0.1.0","changelog":null{extension}}},{{"version":"1.0.0","released_at":"2026-07-01T00:00:00Z","download":{{"url":"https://apps.example/r/weather/1.0.0.zip","size":1,"sha256":"{}"}},"permissions":[],"min_libbridgething_version":"0.1.0","changelog":null}}]}}]}}"#,
      digest.size,
      digest.sha256,
      "0".repeat(64),
    ),
  );
  bundle
}

fn weather(version: &str, provenance: &str) -> WebappInfo {
  WebappInfo {
    id: WEATHER,
    name: "weather".to_owned(),
    source: WebappSource::Installed,
    role: WebappRole::Standard,
    version: version.to_owned(),
    description: None,
    icon_hash: None,
    settings_hash: None,
    overlay_hash: None,
    config: Vec::new(),
    permissions: Vec::new(),
    renders_voice_display: false,
    art: None,
    provenance: Some(provenance.to_owned()),
    extension: None,
  }
}

#[tokio::test(start_paused = true)]
async fn a_store_app_behind_its_subscribed_source_is_updated_without_anyone_watching() {
  let mut rig = rig().await;
  publish_current_firmware(&rig.fetch);
  let bundle = publish_catalog(&rig.fetch, SOURCE, false);
  rig.service.set_catalog_sources(vec![SOURCE.to_owned()]);

  rig.service.set_poll_config(Some(config(true)));
  rig.device.answer_webapp_list(vec![weather("1.0.0", SOURCE)]).await;

  let (_, begin) = rig.device.await_ota_begin().await;
  assert_eq!(begin.kind, OtaKind::InstalledWebapp);
  assert_eq!(
    begin.provenance.as_deref(),
    Some(SOURCE),
    "the update keeps the app pinned to its source"
  );
  assert_eq!(
    begin.update_id,
    sha256_hex(&bundle),
    "the bundle pushed is the one the catalog declared"
  );
}

#[tokio::test(start_paused = true)]
async fn a_store_app_is_updated_even_when_the_firmware_manifest_is_unreachable() {
  let mut rig = rig().await;
  publish_catalog(&rig.fetch, SOURCE, false);
  rig.service.set_catalog_sources(vec![SOURCE.to_owned()]);

  rig.service.set_poll_config(Some(config(true)));
  rig.device.answer_webapp_list(vec![weather("1.0.0", SOURCE)]).await;

  let (_, begin) = rig.device.await_ota_begin().await;
  assert_eq!(begin.kind, OtaKind::InstalledWebapp);
}

#[tokio::test(start_paused = true)]
async fn auto_install_off_leaves_store_apps_alone() {
  let mut rig = rig().await;
  publish_current_firmware(&rig.fetch);
  publish_catalog(&rig.fetch, SOURCE, false);
  rig.service.set_catalog_sources(vec![SOURCE.to_owned()]);

  rig.service.set_poll_config(Some(config(false)));

  assert!(rig.device.no_ota_begin(QUIET).await);
  assert!(
    !rig.fetch.urls().iter().any(|url| url == SOURCE),
    "a host that installs nothing on its own has no reason to read catalogs"
  );
}

#[tokio::test(start_paused = true)]
async fn an_app_from_a_source_the_host_dropped_is_not_updated() {
  let mut rig = rig().await;
  publish_current_firmware(&rig.fetch);
  publish_catalog(&rig.fetch, SOURCE, false);
  publish_catalog(&rig.fetch, OTHER_SOURCE, false);
  rig.service.set_catalog_sources(vec![OTHER_SOURCE.to_owned()]);

  rig.service.set_poll_config(Some(config(true)));
  rig.device.answer_webapp_list(vec![weather("1.0.0", SOURCE)]).await;

  assert!(
    rig.device.no_ota_begin(QUIET).await,
    "a subscription is what buys update checks, and another source listing the same app never updates it"
  );
}

#[tokio::test(start_paused = true)]
async fn an_app_that_ships_a_native_extension_is_never_updated_on_its_own() {
  let mut rig = rig().await;
  publish_current_firmware(&rig.fetch);
  publish_catalog(&rig.fetch, SOURCE, true);
  rig.service.set_catalog_sources(vec![SOURCE.to_owned()]);

  rig.service.set_poll_config(Some(config(true)));
  rig.device.answer_webapp_list(vec![weather("1.0.0", SOURCE)]).await;

  assert!(
    rig.device.no_ota_begin(QUIET).await,
    "moving the webapp version under a desktop's extracted extension needs the store's consent dialog"
  );
}

#[tokio::test(start_paused = true)]
async fn an_installed_bundle_with_an_extension_is_never_updated_on_its_own() {
  let mut rig = rig().await;
  publish_current_firmware(&rig.fetch);
  publish_catalog(&rig.fetch, SOURCE, false);
  rig.service.set_catalog_sources(vec![SOURCE.to_owned()]);
  let mut installed = weather("1.0.0", SOURCE);
  installed.extension = Some(ExtensionInfo {
    permissions: Vec::new(),
    api: 1,
  });

  rig.service.set_poll_config(Some(config(true)));
  rig.device.answer_webapp_list(vec![installed]).await;

  assert!(rig.device.no_ota_begin(QUIET).await);
}

#[tokio::test(start_paused = true)]
async fn a_failed_store_update_is_not_retried_on_every_poll() {
  let mut rig = rig().await;
  publish_current_firmware(&rig.fetch);
  publish_catalog(&rig.fetch, SOURCE, false);
  rig.service.set_catalog_sources(vec![SOURCE.to_owned()]);
  let mut events = rig.service.events();
  rig.service.set_poll_config(Some(config(true)));
  rig.device.answer_webapp_list(vec![weather("1.0.0", SOURCE)]).await;
  let (request_id, _) = rig.device.await_ota_begin().await;
  rig.device.reject_begin(request_id, "no room");
  tokio::time::timeout(Duration::from_secs(5), async {
    while !matches!(
      events.recv().await,
      Ok(OtaPollEvent::Failed {
        kind: OtaKind::InstalledWebapp,
        ..
      })
    ) {}
  })
  .await
  .expect("the rejected install fails its run");

  let polled = tokio::spawn({
    let service = rig.service.clone();
    async move { service.poll_now().await }
  });
  rig.device.answer_webapp_list(vec![weather("1.0.0", SOURCE)]).await;
  polled.await.expect("the poll finishes");

  assert!(
    rig.device.no_ota_begin(QUIET).await,
    "a version that failed once waits for a new version, not for the next poll"
  );
}

#[tokio::test(start_paused = true)]
async fn a_firmware_update_holds_the_device_and_store_apps_wait() {
  let mut rig = rig().await;
  let daemon = pattern(4 * 1024);
  rig.fetch.serve_artifact(
    &crate::ota::manifest::OtaArtifactUrls::build(ROOT, CHANNEL, "1.0.0", "2026.06.0", "prod").daemon_binary,
    daemon.clone(),
  );
  let mut fixture = ManifestFixture::new(CHANNEL, "1.0.0+image.2026.06.0");
  fixture.daemon = Some(digest_of(&daemon));
  rig.fetch.serve_text(&format!("{ROOT}/manifest.json"), fixture.json());
  publish_catalog(&rig.fetch, SOURCE, false);
  rig.service.set_catalog_sources(vec![SOURCE.to_owned()]);

  rig.service.set_poll_config(Some(config(true)));

  let (_, begin) = rig.device.await_ota_begin().await;
  assert_eq!(begin.kind, OtaKind::Daemon);
  assert!(
    rig.device.no_ota_begin(QUIET).await,
    "a store app must not ride into a device that is taking firmware"
  );
}

#[tokio::test(start_paused = true)]
async fn a_catalog_that_is_not_v1_updates_nothing() {
  let mut rig = rig().await;
  publish_current_firmware(&rig.fetch);
  rig
    .fetch
    .serve_text(SOURCE, r#"{"schema":"catalog.v2","apps":[]}"#.to_owned());
  rig.service.set_catalog_sources(vec![SOURCE.to_owned()]);

  rig.service.set_poll_config(Some(config(true)));

  assert!(rig.device.no_ota_begin(QUIET).await);
}
