use libbridgething::BridgeThingMeta;

use crate::ota::manifest::{OtaCompositeVersion, OtaDiscoverManifest, OtaManifestRelease};

pub const DEFAULT_OTA_ROOT_URL: &str = "https://ota.bridgething.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaPollConfig {
  pub root_url: String,
  pub interval_seconds: u64,
  pub auto_push: bool,
}

impl Default for OtaPollConfig {
  fn default() -> Self {
    Self {
      root_url: DEFAULT_OTA_ROOT_URL.into(),
      interval_seconds: 3_600,
      auto_push: true,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drift {
  pub image: bool,
  pub daemon: bool,
}

impl Drift {
  pub fn any(&self) -> bool {
    self.image || self.daemon
  }
}

pub fn drift(meta: &BridgeThingMeta, latest: &OtaCompositeVersion) -> Drift {
  Drift {
    image: meta.image_version != latest.image,
    daemon: meta.app_version != latest.daemon,
  }
}

pub fn wakeword_drift(
  meta: &BridgeThingMeta,
  release: Option<&OtaManifestRelease>,
  daemon_after: &str,
) -> Option<String> {
  let wakeword = release?.wakeword.as_ref()?;
  let have = meta.wakeword_model_version.as_deref()?;
  if have == wakeword.model {
    return None;
  }
  if wakeword.trained_against(&wakeword.model) != daemon_after {
    return None;
  }
  Some(wakeword.model.clone())
}

pub fn pushable_release<'a>(
  manifest: &'a OtaDiscoverManifest,
  channel: &str,
) -> Option<(OtaCompositeVersion, Option<&'a OtaManifestRelease>)> {
  let channel = manifest.channels.get(channel)?;
  let latest = OtaCompositeVersion::parse(&channel.latest)?;
  let release = manifest.releases.get(&channel.latest);
  if release.is_some_and(|release| release.yanked.is_some() || release.deprecated) {
    return None;
  }
  Some((latest, release))
}

#[cfg(test)]
mod tests {
  use std::{sync::Arc, time::Duration};

  use libbridgething::{OtaKind, OtaPhase, WebappInfo, WebappRole, WebappSource};
  use tokio::sync::broadcast;
  use uuid::Uuid;

  use super::OtaPollConfig;
  use crate::{
    ota::{
      autopush::{LINK_STABILITY_MS, MIN_POLL_INTERVAL_SECONDS},
      event::{OtaPollEvent, OtaStepKind},
      harness::{
        DEVICE, FakeDevice, FakeFetch, ManifestFixture, OTHER_DEVICE, Spool, TestClock, digest_of, linked_gateway,
        meta, pattern, route_into, sha256_hex,
      },
      manifest::OtaArtifactUrls,
      service::{OtaService, OtaServiceDeps},
    },
    webapp::BROWSER_WEBAPP_ID,
  };

  const ROOT: &str = "https://ota.test";
  const CHANNEL: &str = "stable";
  const OTHER_CHANNEL: &str = "beta";
  const LATEST: &str = "0.9.0+image.2026.06.0";

  struct Rig {
    service: Arc<OtaService>,
    fetch: Arc<FakeFetch>,
    _spool: Spool,
    device: FakeDevice,
    events: broadcast::Receiver<OtaPollEvent>,
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
    let events = service.events();
    let (gateway, device) = linked_gateway();
    service.adopt(DEVICE, gateway.clone()).await;
    route_into(&gateway, &service, DEVICE);
    Rig {
      service,
      fetch,
      _spool: spool,
      device,
      events,
    }
  }

  fn config(auto_push: bool) -> OtaPollConfig {
    OtaPollConfig {
      root_url: ROOT.into(),
      interval_seconds: 86_400,
      auto_push,
    }
  }

  async fn next_event(
    events: &mut broadcast::Receiver<OtaPollEvent>,
    pick: impl Fn(&OtaPollEvent) -> bool,
  ) -> OtaPollEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
      loop {
        let event = events.recv().await.expect("the feed stayed open");
        if pick(&event) {
          return event;
        }
      }
    })
    .await
    .expect("the poll feed went quiet while an event was expected")
  }

  async fn no_event(
    events: &mut broadcast::Receiver<OtaPollEvent>,
    window: Duration,
    pick: impl Fn(&OtaPollEvent) -> bool,
  ) -> bool {
    tokio::time::timeout(window, async {
      loop {
        let event = events.recv().await.expect("the feed stayed open");
        if pick(&event) {
          return;
        }
      }
    })
    .await
    .is_err()
  }

  fn serve_release(fetch: &FakeFetch, variant: &str) -> ManifestFixture {
    let daemon = pattern(4 * 1024);
    let swu = pattern(6 * 1024);
    let zck = pattern(2 * 1024);
    let boot = pattern(1_024);
    let urls = OtaArtifactUrls::build(ROOT, CHANNEL, "0.9.0", "2026.06.0", variant);

    fetch.serve_artifact(&urls.daemon_binary, daemon.clone());
    fetch.serve_artifact(&urls.image_swu, swu.clone());
    fetch.serve_artifact(&urls.image_zck, zck.clone());
    fetch.serve_artifact(&urls.image_boot_zck, boot.clone());

    let mut fixture = ManifestFixture::new(CHANNEL, LATEST);
    fixture.daemon = Some(digest_of(&daemon));
    fixture.image_swu = Some(digest_of(&swu));
    fixture.image_zck = Some(digest_of(&zck));
    fixture.image_boot_zck = Some(digest_of(&boot));
    fixture
  }

  fn publish(fetch: &FakeFetch, fixture: &ManifestFixture) {
    fetch.serve_text(&format!("{ROOT}/manifest.json"), fixture.json());
  }

  async fn hold_link_open() {
    tokio::time::advance(Duration::from_millis(LINK_STABILITY_MS + 1_000)).await;
  }

  fn builtin_info(id: Uuid, name: &str, version: &str) -> WebappInfo {
    WebappInfo {
      id,
      name: name.to_owned(),
      source: WebappSource::Builtin,
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
      provenance: None,
    }
  }

  #[tokio::test(start_paused = true)]
  async fn a_poll_reports_when_the_manifest_was_last_updated() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));

    rig.service.set_poll_config(Some(config(false))).await;

    let event = next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::ManifestPolled { .. })
    })
    .await;
    assert!(matches!(
      event,
      OtaPollEvent::ManifestPolled { ref updated_at } if updated_at == "2026-08-03T00:00:00Z"
    ));
  }

  #[tokio::test(start_paused = true)]
  async fn a_failed_poll_keeps_the_last_good_timestamp() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.service.set_poll_config(Some(config(false))).await;
    next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::ManifestPolled { .. })
    })
    .await;

    rig.fetch.fail_with("dns went away");
    rig.service.poll_now().await;

    next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::ManifestPollFailed { .. })
    })
    .await;
    let status = rig.service.retained_poll_status().await;
    assert_eq!(status.last_polled_at.as_deref(), Some("2026-08-03T00:00:00Z"));
    assert!(status.error.is_some(), "the failure is still reported alongside it");
  }

  #[tokio::test(start_paused = true)]
  async fn an_adopted_link_records_the_device_it_announces() {
    let rig = rig().await;

    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));

    let recorded = tokio::time::timeout(Duration::from_secs(3), async {
      loop {
        if let Some(recorded) = rig.service.meta(DEVICE).await {
          return recorded;
        }
        tokio::task::yield_now().await;
      }
    })
    .await
    .expect("an announce reaches the service");
    assert_eq!(recorded.app_version, "0.8.0");
  }

  #[tokio::test(start_paused = true)]
  async fn a_released_link_stops_being_reconciled() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    hold_link_open().await;

    rig.service.release(DEVICE).await;
    rig.service.set_poll_config(Some(config(true))).await;

    assert!(
      no_event(&mut rig.events, Duration::from_millis(500), |event| matches!(
        event,
        OtaPollEvent::UpdateAvailable { .. }
      ))
      .await,
      "a device that is gone is not offered an update"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn two_devices_are_reconciled_independently() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    let (other_gateway, other_device) = linked_gateway();
    rig.service.adopt(OTHER_DEVICE, other_gateway.clone()).await;
    route_into(&other_gateway, &rig.service, OTHER_DEVICE);

    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    other_device.announce_meta(meta("0.9.0", "2026.06.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(false))).await;

    let event = next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::UpdateAvailable { .. })
    })
    .await;
    assert!(
      matches!(event, OtaPollEvent::UpdateAvailable { ref device_id, .. } if device_id == DEVICE),
      "only the drifted device is offered an update, got {event:?}"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_device_on_the_latest_release_is_offered_nothing() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.9.0", "2026.06.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    assert!(
      no_event(&mut rig.events, Duration::from_millis(500), |event| matches!(
        event,
        OtaPollEvent::UpdateAvailable { .. }
      ))
      .await
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_yanked_release_is_not_offered() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    fixture.yanked = Some("bricks the boot slot".into());
    publish(&rig.fetch, &fixture);
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    assert!(
      no_event(&mut rig.events, Duration::from_millis(500), |event| matches!(
        event,
        OtaPollEvent::UpdateAvailable { .. }
      ))
      .await
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_deprecated_release_is_not_offered() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    fixture.deprecated = true;
    publish(&rig.fetch, &fixture);
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    assert!(
      no_event(&mut rig.events, Duration::from_millis(500), |event| matches!(
        event,
        OtaPollEvent::UpdateAvailable { .. }
      ))
      .await
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_device_on_a_channel_the_manifest_does_not_carry_is_skipped() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", "nightly"));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    assert!(
      no_event(&mut rig.events, Duration::from_millis(500), |event| matches!(
        event,
        OtaPollEvent::UpdateAvailable { .. }
      ))
      .await
    );
  }

  #[tokio::test(start_paused = true)]
  async fn auto_push_off_offers_the_update_and_pushes_nothing() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(false))).await;

    next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::UpdateAvailable { .. })
    })
    .await;
    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "an update the host did not ask for must not be pushed"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_link_that_has_not_held_long_enough_is_not_pushed_to() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    tokio::time::advance(Duration::from_millis(LINK_STABILITY_MS / 2)).await;

    rig.service.set_poll_config(Some(config(true))).await;

    next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::UpdateAvailable { .. })
    })
    .await;
    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "a link that just came up does not get handed a multi-megabyte update"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn an_image_change_runs_the_image_and_never_the_daemon_bandaid() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    let planned = next_event(&mut rig.events, |event| matches!(event, OtaPollEvent::Planned { .. })).await;
    let OtaPollEvent::Planned { kind, ref steps, .. } = planned else {
      unreachable!()
    };
    assert_eq!(kind, OtaKind::Image);
    assert_eq!(
      steps.iter().map(|step| step.kind).collect::<Vec<_>>(),
      vec![
        OtaStepKind::Download,
        OtaStepKind::Download,
        OtaStepKind::Download,
        OtaStepKind::Stream,
        OtaStepKind::Apply,
        OtaStepKind::Reboot,
      ],
      "the plan is announced before a byte moves"
    );

    let (_, begin) = rig.device.await_ota_begin().await;
    assert_eq!(begin.kind, OtaKind::Image);
    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "no standalone daemon bandaid push while the image is changing"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_daemon_only_delta_runs_the_bandaid_and_never_an_image() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.06.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    let (_, begin) = rig.device.await_ota_begin().await;
    assert_eq!(
      begin.kind,
      OtaKind::Daemon,
      "a daemon-only delta runs the daemon bandaid"
    );
    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "a daemon-only delta must not start an image ota"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_daemon_push_whose_compressed_artifact_fails_falls_back_to_the_plain_binary() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    let daemon = pattern(5 * 1024);
    rig.fetch.serve_artifact(
      &OtaArtifactUrls::build(ROOT, CHANNEL, "0.9.0", "2026.06.0", "prod").daemon_binary,
      daemon.clone(),
    );
    fixture.daemon = Some(digest_of(&daemon));
    fixture.daemon_zst = Some(digest_of(&pattern(1_024)));
    publish(&rig.fetch, &fixture);
    rig.device.announce_meta(meta("0.8.0", "2026.06.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    let (_, begin) = rig.device.await_ota_begin().await;
    assert_eq!(begin.kind, OtaKind::Daemon);
    assert!(
      begin.patch.is_none(),
      "the fallback tells the device to install what it receives as-is"
    );
    assert_eq!(
      begin.update_id,
      sha256_hex(&daemon),
      "installed as-is, so the artifact has to be the runnable binary and never the compressed one"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn every_builtin_webapp_the_release_names_is_offered_when_it_drifts() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    let bundle = pattern(2 * 1024);
    fixture.builtin_webapps.insert("browser".into(), "0.5.0".into());
    rig.fetch.serve_artifact(
      &OtaArtifactUrls::builtin_webapp(ROOT, CHANNEL, "browser", "0.5.0"),
      bundle.clone(),
    );
    publish(&rig.fetch, &fixture);
    rig.device.announce_meta(meta("0.9.0", "2026.06.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;
    rig
      .device
      .answer_webapp_list(vec![builtin_info(BROWSER_WEBAPP_ID, "browser", "0.4.0")])
      .await;

    let planned = next_event(&mut rig.events, |event| matches!(event, OtaPollEvent::Planned { .. })).await;
    let OtaPollEvent::Planned { kind, ref steps, .. } = planned else {
      unreachable!()
    };
    assert_eq!(kind, OtaKind::BuiltinWebapp);
    assert!(
      steps.iter().any(|step| step.label == "webapp: browser"),
      "a builtin the release publishes but the poller never checks can only change on a full image ota, got {steps:?}"
    );

    let (_, begin) = rig.device.await_ota_begin().await;
    assert_eq!(begin.update_id, sha256_hex(&bundle));
  }

  #[tokio::test(start_paused = true)]
  async fn a_device_that_turns_out_to_be_current_has_its_offer_retracted() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(false))).await;
    next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::UpdateAvailable { .. })
    })
    .await;
    assert_eq!(rig.service.retained_available().await.len(), 1);

    rig.device.announce_meta(meta("0.9.0", "2026.06.0", CHANNEL));
    hold_link_open().await;
    rig.service.poll_now().await;

    assert!(
      rig.service.retained_available().await.is_empty(),
      "a device that got current out of band would otherwise wear an update badge forever"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_webapp_list_that_comes_back_empty_does_not_retract_a_held_offer() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    fixture.builtin_webapps.insert("browser".into(), "0.5.0".into());
    rig.fetch.serve_artifact(
      &OtaArtifactUrls::builtin_webapp(ROOT, CHANNEL, "browser", "0.5.0"),
      pattern(2 * 1024),
    );
    publish(&rig.fetch, &fixture);
    rig.device.announce_meta(meta("0.9.0", "2026.06.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(false))).await;
    rig
      .device
      .answer_webapp_list(vec![builtin_info(BROWSER_WEBAPP_ID, "browser", "0.4.0")])
      .await;
    next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::UpdateAvailable { .. })
    })
    .await;
    assert_eq!(rig.service.retained_available().await.len(), 1);

    let poll = tokio::spawn({
      let service = rig.service.clone();
      async move { service.poll_now().await }
    });
    rig.device.answer_webapp_list(Vec::new()).await;
    poll.await.expect("the poll finishes");

    assert_eq!(
      rig.service.retained_available().await.len(),
      1,
      "a device that never said what it has is not a device that said it is current"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn an_unknown_webapp_list_is_rechecked_without_waiting_out_the_interval() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    fixture.builtin_webapps.insert("browser".into(), "0.5.0".into());
    rig.fetch.serve_artifact(
      &OtaArtifactUrls::builtin_webapp(ROOT, CHANNEL, "browser", "0.5.0"),
      pattern(2 * 1024),
    );
    publish(&rig.fetch, &fixture);
    rig.device.announce_meta(meta("0.9.0", "2026.06.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(false))).await;
    rig.device.answer_webapp_list(Vec::new()).await;
    for _ in 0..64 {
      tokio::task::yield_now().await;
    }

    tokio::time::advance(Duration::from_secs(MIN_POLL_INTERVAL_SECONDS + 1)).await;
    rig
      .device
      .answer_webapp_list(vec![builtin_info(BROWSER_WEBAPP_ID, "browser", "0.4.0")])
      .await;

    next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::UpdateAvailable { .. })
    })
    .await;
    assert_eq!(
      rig.service.retained_available().await.len(),
      1,
      "the bump has to surface on the recheck, not an interval later"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_manifest_that_could_not_be_read_is_retried_without_waiting_out_the_interval() {
    let mut rig = rig().await;
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(false))).await;
    next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::ManifestPollFailed { .. })
    })
    .await;
    for _ in 0..64 {
      tokio::task::yield_now().await;
    }

    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    tokio::time::advance(Duration::from_secs(MIN_POLL_INTERVAL_SECONDS + 1)).await;

    next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::UpdateAvailable { .. })
    })
    .await;
    assert_eq!(
      rig.service.retained_available().await.len(),
      1,
      "a manifest that was unreadable once cannot cost a whole interval of silence"
    );
  }

  const TRAINED_AGAINST: &str = "0.9.0";

  fn serve_wakeword(fetch: &FakeFetch, fixture: &mut ManifestFixture, model: &str) -> Vec<u8> {
    serve_wakeword_trained_against(fetch, fixture, model, TRAINED_AGAINST)
  }

  fn serve_wakeword_trained_against(
    fetch: &FakeFetch,
    fixture: &mut ManifestFixture,
    model: &str,
    runtime: &str,
  ) -> Vec<u8> {
    let body = pattern(3 * 1024);
    fetch.serve_artifact(&OtaArtifactUrls::wakeword_model(ROOT, CHANNEL, model), body.clone());
    fixture.wakeword_model = Some(model.into());
    fixture.wakeword_runtime = Some(runtime.into());
    fixture.wakeword_model_digest = Some(digest_of(&body));
    body
  }

  fn meta_with_wakeword(daemon: &str, image: &str, wakeword: Option<&str>) -> libbridgething::BridgeThingMeta {
    let mut announced = meta(daemon, image, CHANNEL);
    announced.wakeword_model_version = wakeword.map(str::to_owned);
    announced
  }

  #[tokio::test(start_paused = true)]
  async fn a_stale_wake_word_model_is_pushed_on_its_own() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    let body = serve_wakeword(&rig.fetch, &mut fixture, "1.2.0");
    publish(&rig.fetch, &fixture);
    rig
      .device
      .announce_meta(meta_with_wakeword("0.9.0", "2026.06.0", Some("1.0.0")));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    let (_, begin) = rig.device.await_ota_begin().await;
    assert_eq!(begin.kind, OtaKind::WakewordModel);
    assert_eq!(
      begin.update_id,
      crate::ota::harness::sha256_hex(&body),
      "the artifact pushed is the one the manifest declared"
    );
    assert_eq!(
      begin.version.as_deref(),
      Some("1.2.0"),
      "a model container carries no version of its own, so the push has to name it"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_current_wake_word_model_is_left_alone() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    serve_wakeword(&rig.fetch, &mut fixture, "1.2.0");
    publish(&rig.fetch, &fixture);
    rig
      .device
      .announce_meta(meta_with_wakeword("0.9.0", "2026.06.0", Some("1.2.0")));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "nothing drifted, so nothing is pushed"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_device_that_reports_no_wake_word_version_is_not_guessed_at() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    serve_wakeword(&rig.fetch, &mut fixture, "1.2.0");
    publish(&rig.fetch, &fixture);
    rig.device.announce_meta(meta_with_wakeword("0.9.0", "2026.06.0", None));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "an unknown version is not a stale one"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_model_trained_against_a_different_daemon_is_refused() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    serve_wakeword_trained_against(&rig.fetch, &mut fixture, "1.2.0", "0.4.0");
    publish(&rig.fetch, &fixture);
    rig
      .device
      .announce_meta(meta_with_wakeword("0.9.0", "2026.06.0", Some("1.0.0")));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "a model trained against another embedding graph silently never fires, so it is not offered"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_model_is_checked_against_the_daemon_the_batch_will_leave_behind() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    serve_wakeword(&rig.fetch, &mut fixture, "1.2.0");
    publish(&rig.fetch, &fixture);
    rig
      .device
      .announce_meta(meta_with_wakeword("0.8.0", "2026.06.0", Some("1.0.0")));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    let planned = next_event(&mut rig.events, |event| matches!(event, OtaPollEvent::Planned { .. })).await;
    let OtaPollEvent::Planned { ref steps, .. } = planned else {
      unreachable!()
    };
    assert!(
      steps.iter().any(|step| step.label == "wake word model"),
      "the daemon in the same batch is the one the model has to match, not the one being replaced"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_drifted_daemon_and_wake_word_model_ride_the_same_batch() {
    let mut rig = rig().await;
    let mut fixture = serve_release(&rig.fetch, "prod");
    serve_wakeword(&rig.fetch, &mut fixture, "1.2.0");
    publish(&rig.fetch, &fixture);
    rig
      .device
      .announce_meta(meta_with_wakeword("0.8.0", "2026.06.0", Some("1.0.0")));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    let planned = next_event(&mut rig.events, |event| matches!(event, OtaPollEvent::Planned { .. })).await;
    let OtaPollEvent::Planned { kind, ref steps, .. } = planned else {
      unreachable!()
    };
    assert_eq!(
      kind,
      OtaKind::Daemon,
      "a batch with a daemon in it reads as a daemon run"
    );
    assert_eq!(
      steps
        .iter()
        .filter(|step| step.kind == OtaStepKind::Download)
        .map(|step| step.label.clone())
        .collect::<Vec<_>>(),
      vec!["daemon".to_string(), "wake word model".to_string()]
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_pushed_artifact_is_the_one_the_manifest_declared() {
    let mut rig = rig().await;
    let fixture = serve_release(&rig.fetch, "prod");
    let want = fixture.daemon.clone().expect("the fixture declares a daemon").sha256;
    publish(&rig.fetch, &fixture);
    rig.device.announce_meta(meta("0.8.0", "2026.06.0", CHANNEL));
    hold_link_open().await;

    rig.service.set_poll_config(Some(config(true))).await;

    let (_, begin) = rig.device.await_ota_begin().await;
    assert_eq!(
      begin.update_id, want,
      "the update is named by the digest the manifest declared"
    );
    assert_eq!(rig.fetch.downloads(), 1, "and it was pulled exactly once");
  }

  #[tokio::test(start_paused = true)]
  async fn a_device_with_an_update_in_flight_is_skipped_by_the_next_poll() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.06.0", CHANNEL));
    hold_link_open().await;
    rig.service.set_poll_config(Some(config(true))).await;
    rig.device.await_ota_begin().await;

    rig.service.poll_now().await;

    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "a second poll must not start a second update on a device already taking one"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_failed_push_arms_the_backoff_before_the_next_attempt() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.06.0", CHANNEL));
    hold_link_open().await;
    rig.service.set_poll_config(Some(config(true))).await;

    let (request_id, _) = rig.device.await_ota_begin().await;
    rig.device.reject_begin(request_id, "slot busy");
    next_event(&mut rig.events, |event| matches!(event, OtaPollEvent::Failed { .. })).await;

    rig.service.poll_now().await;
    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "a failure has to cost something, or a broken device is retried on every poll"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_failed_push_reports_its_own_reason_exactly_once() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.06.0", CHANNEL));
    hold_link_open().await;
    rig.service.set_poll_config(Some(config(true))).await;

    let (request_id, _) = rig.device.await_ota_begin().await;
    rig.device.reject_begin(request_id, "slot busy");

    let failed = next_event(&mut rig.events, |event| matches!(event, OtaPollEvent::Failed { .. })).await;
    let OtaPollEvent::Failed { ref reason, .. } = failed else {
      unreachable!()
    };
    assert!(reason.contains("slot busy"), "got {reason}");
    assert!(
      no_event(&mut rig.events, Duration::from_millis(500), |event| matches!(
        event,
        OtaPollEvent::Failed { .. }
      ))
      .await,
      "the in-flight backstop must not report a second failure over the real one"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_device_that_comes_back_on_the_target_image_closes_its_run() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    hold_link_open().await;
    rig.service.set_poll_config(Some(config(true))).await;
    rig.device.await_ota_begin().await;

    rig.device.announce_meta(meta("0.9.0", "2026.06.0", CHANNEL));

    let updated = next_event(&mut rig.events, |event| matches!(event, OtaPollEvent::Updated { .. })).await;
    assert!(
      matches!(updated, OtaPollEvent::Updated { kind, ref version, .. } if kind == OtaKind::Image && version == "2026.06.0"),
      "a device announcing the version it was being updated to is the update succeeding, got {updated:?}"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn apply_version_with_an_image_change_runs_the_image_only() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    await_meta(&rig).await;

    let service = rig.service.clone();
    tokio::spawn(async move { service.apply_version(DEVICE, CHANNEL, LATEST, ROOT).await });

    let (_, begin) = rig.device.await_ota_begin().await;
    assert_eq!(begin.kind, OtaKind::Image, "an image change runs the image ota");
    assert!(rig.device.no_ota_begin(Duration::from_millis(500)).await);
  }

  #[tokio::test(start_paused = true)]
  async fn apply_version_with_a_daemon_only_delta_runs_the_bandaid() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.06.0", CHANNEL));
    await_meta(&rig).await;

    let service = rig.service.clone();
    tokio::spawn(async move { service.apply_version(DEVICE, CHANNEL, LATEST, ROOT).await });

    let (_, begin) = rig.device.await_ota_begin().await;
    assert_eq!(begin.kind, OtaKind::Daemon);
    assert!(rig.device.no_ota_begin(Duration::from_millis(500)).await);
  }

  #[tokio::test(start_paused = true)]
  async fn apply_version_ignores_a_link_that_is_still_settling() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.06.0", CHANNEL));
    await_meta(&rig).await;

    let service = rig.service.clone();
    tokio::spawn(async move { service.apply_version(DEVICE, CHANNEL, LATEST, ROOT).await });

    rig.device.await_ota_begin().await;
  }

  #[tokio::test(start_paused = true)]
  async fn apply_version_spools_under_the_channel_the_host_asked_for() {
    let mut rig = rig().await;
    let swu = pattern(6 * 1024);
    let zck = pattern(2 * 1024);
    let boot = pattern(1_024);
    let daemon = pattern(4 * 1024);
    let urls = OtaArtifactUrls::build(ROOT, OTHER_CHANNEL, "0.9.0", "2026.06.0", "prod");
    rig.fetch.serve_artifact(&urls.image_swu, swu.clone());
    rig.fetch.serve_artifact(&urls.image_zck, zck.clone());
    rig.fetch.serve_artifact(&urls.image_boot_zck, boot.clone());
    rig.fetch.serve_artifact(&urls.daemon_binary, daemon.clone());
    let mut fixture = ManifestFixture::new(CHANNEL, LATEST);
    fixture.daemon = Some(digest_of(&daemon));
    fixture.image_swu = Some(digest_of(&swu));
    fixture.image_zck = Some(digest_of(&zck));
    fixture.image_boot_zck = Some(digest_of(&boot));
    publish(&rig.fetch, &fixture);
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    await_meta(&rig).await;

    let service = rig.service.clone();
    tokio::spawn(async move { service.apply_version(DEVICE, OTHER_CHANNEL, LATEST, ROOT).await });
    rig.device.await_ota_begin().await;

    let spooled: Vec<String> = std::fs::read_dir(rig._spool.path())
      .expect("the spool directory")
      .map(|entry| entry.expect("a spool entry").file_name().to_string_lossy().into_owned())
      .collect();
    assert!(
      spooled.iter().any(|name| name.starts_with("image-beta-")),
      "a hand-driven apply spools under the channel it was asked for, not the one the device is on, got {spooled:?}"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn apply_version_refuses_something_that_is_not_a_composite_version() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    await_meta(&rig).await;

    rig.service.apply_version(DEVICE, CHANNEL, "0.9.0", ROOT).await;

    let failed = next_event(&mut rig.events, |event| matches!(event, OtaPollEvent::Failed { .. })).await;
    assert!(
      matches!(failed, OtaPollEvent::Failed { ref reason, .. } if reason.contains("is not a composite version")),
      "got {failed:?}"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn apply_version_refuses_a_device_it_has_never_heard_from() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));

    rig.service.apply_version(DEVICE, CHANNEL, LATEST, ROOT).await;

    let failed = next_event(&mut rig.events, |event| matches!(event, OtaPollEvent::Failed { .. })).await;
    assert!(
      matches!(failed, OtaPollEvent::Failed { ref reason, .. } if reason == "device meta not yet known"),
      "got {failed:?}"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn apply_version_does_nothing_while_an_update_is_already_running() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.06.0", CHANNEL));
    await_meta(&rig).await;
    let service = rig.service.clone();
    tokio::spawn(async move { service.apply_version(DEVICE, CHANNEL, LATEST, ROOT).await });
    rig.device.await_ota_begin().await;

    rig.service.apply_version(DEVICE, CHANNEL, LATEST, ROOT).await;

    assert!(
      rig.device.no_ota_begin(Duration::from_millis(500)).await,
      "asking twice does not start two updates"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn the_image_variant_the_device_reports_picks_the_artifact() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "dev"));
    rig.device.announce_meta(crate::ota::harness::meta_with_variant(
      "0.8.0",
      "2026.05.0",
      CHANNEL,
      "dev",
      None,
    ));
    await_meta(&rig).await;

    let service = rig.service.clone();
    tokio::spawn(async move { service.apply_version(DEVICE, CHANNEL, LATEST, ROOT).await });
    rig.device.await_ota_begin().await;

    let asked = rig.fetch.urls();
    assert!(
      asked.iter().any(|url| url.contains("bridgething-dev-image.swu")),
      "a dev device must not be handed the prod image, asked for {asked:?}"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn an_update_the_daemon_finishes_reports_a_reboot_step() {
    let mut rig = rig().await;
    publish(&rig.fetch, &serve_release(&rig.fetch, "prod"));
    rig.device.announce_meta(meta("0.8.0", "2026.05.0", CHANNEL));
    await_meta(&rig).await;
    let service = rig.service.clone();
    tokio::spawn(async move { service.apply_version(DEVICE, CHANNEL, LATEST, ROOT).await });

    let (request_id, begin) = rig.device.await_ota_begin().await;
    rig.device.ack_begin(request_id, begin.transfer.total_size);
    rig.device.progress_full(OtaPhase::Reboot, 100, 100, 0);

    let progress = next_event(&mut rig.events, |event| {
      matches!(event, OtaPollEvent::Progress { step_id: 5, .. })
    })
    .await;
    assert!(
      matches!(progress, OtaPollEvent::Progress { kind, .. } if kind == OtaKind::Image),
      "a reboot tick lands on the reboot step of the image plan, got {progress:?}"
    );
  }

  async fn await_meta(rig: &Rig) {
    tokio::time::timeout(Duration::from_secs(3), async {
      loop {
        if rig.service.meta(DEVICE).await.is_some() {
          return;
        }
        tokio::task::yield_now().await;
      }
    })
    .await
    .expect("the device announce reaches the service");
  }
}
