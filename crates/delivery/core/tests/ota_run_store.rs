use std::sync::Arc;

use bridgething_delivery::ota::{
  event::{CANCELLED_REASON, OtaPhaseSnapshot, OtaPlanStep, OtaPollEvent, OtaStepKind},
  run_store::{OtaRun, OtaRunOutcome, OtaRunPhase, OtaRunStore, OtaStoreChange, RESUMABLE_REASON},
};
use libbridgething::{OtaKind, OtaPhase};
use support::TestClock;

mod support;

const DEVICE: &str = "AA:BB:CC:DD:EE:FF";
const OTHER: &str = "11:22:33:44:55:66";
const EPOCH_MS: u64 = 1_700_000_000_000;

fn at(seconds: u64) -> u64 {
  EPOCH_MS + seconds * 1_000
}

fn store_at(millis: u64) -> (Arc<TestClock>, OtaRunStore) {
  let clock = TestClock::at(millis);
  let store = OtaRunStore::new(clock.clone(), None);
  (clock, store)
}

fn plan_step(id: u32, kind: OtaStepKind) -> OtaPlanStep {
  OtaPlanStep {
    id,
    kind,
    label: "step".into(),
    bytes: 100,
  }
}

fn planned_image() -> OtaPollEvent {
  planned_image_targeting("0.9.0", "2026.06.0")
}

fn planned_image_targeting(daemon: &str, image: &str) -> OtaPollEvent {
  OtaPollEvent::Planned {
    device_id: DEVICE.into(),
    kind: OtaKind::Image,
    release: format!("{daemon}+image.{image}"),
    daemon_version: daemon.into(),
    image_version: image.into(),
    channel: "stable".into(),
    root_url: "https://ota.bridgething.com".into(),
    steps: vec![
      plan_step(0, OtaStepKind::Download),
      plan_step(1, OtaStepKind::Apply),
      plan_step(2, OtaStepKind::Reboot),
    ],
  }
}

fn planned_webapp() -> OtaPollEvent {
  OtaPollEvent::Planned {
    device_id: DEVICE.into(),
    kind: OtaKind::InstalledWebapp,
    release: String::new(),
    daemon_version: String::new(),
    image_version: String::new(),
    channel: "stable".into(),
    root_url: "https://ota.bridgething.com".into(),
    steps: vec![plan_step(0, OtaStepKind::Download)],
  }
}

fn downloading(received: u64, total: u64, rate_per_sec: Option<f64>) -> OtaPhaseSnapshot {
  OtaPhaseSnapshot::Downloading {
    asset: "update.swu".into(),
    received,
    total,
    rate_per_sec,
  }
}

fn applying(phase: OtaPhase, write_percent: u32, dwl_percent: u32, dwl_bytes: u64) -> OtaPhaseSnapshot {
  OtaPhaseSnapshot::Applying {
    phase,
    write_percent,
    dwl_percent,
    dwl_bytes,
  }
}

fn progress(step_id: u32, snapshot: OtaPhaseSnapshot) -> OtaPollEvent {
  OtaPollEvent::Progress {
    device_id: DEVICE.into(),
    kind: OtaKind::Image,
    step_id,
    snapshot,
  }
}

fn updated(kind: OtaKind, version: &str) -> OtaPollEvent {
  OtaPollEvent::Updated {
    device_id: DEVICE.into(),
    kind,
    version: version.into(),
  }
}

fn failed(kind: OtaKind, reason: &str) -> OtaPollEvent {
  OtaPollEvent::Failed {
    device_id: DEVICE.into(),
    kind,
    reason: reason.into(),
  }
}

fn only_run(changes: &[OtaStoreChange]) -> Option<&OtaRun> {
  changes.iter().find_map(|change| match change {
    OtaStoreChange::Run(run) => Some(run.as_ref()),
    _ => None,
  })
}

fn only_available_release(changes: &[OtaStoreChange]) -> Option<Option<&str>> {
  changes.iter().find_map(|change| match change {
    OtaStoreChange::Available(available) => Some(available.release_version.as_deref()),
    _ => None,
  })
}

// -- planned --

#[test]
fn planned_opens_a_run_carrying_its_plan() {
  let (_clock, mut store) = store_at(at(0));
  let changes = store.ingest(planned_image(), None);
  let run = only_run(&changes).expect("a plan opens a run");

  assert_eq!(run.device_id, DEVICE);
  assert_eq!(run.kind, OtaKind::Image);
  assert_eq!(run.phase, OtaRunPhase::Idle);
  assert_eq!(run.steps.len(), 3);
  assert_eq!(run.step_id, 0);
  assert_eq!(run.started_at_ms, at(0));
  assert_eq!(run.phase_started_at_ms, at(0));
  assert_eq!(run.daemon_version.as_deref(), Some("0.9.0"));
  assert_eq!(run.image_version.as_deref(), Some("2026.06.0"));
  assert_eq!(run.release_version.as_deref(), Some("0.9.0+image.2026.06.0"));
  assert!(run.outcome.is_none(), "a run that has only been planned has not ended");
  assert_eq!(store.runs().len(), 1);
}

#[test]
fn planned_with_no_versions_leaves_them_unset_rather_than_empty() {
  let (_clock, mut store) = store_at(at(0));
  let changes = store.ingest(planned_webapp(), None);
  let run = only_run(&changes).expect("a plan opens a run");

  assert!(run.daemon_version.is_none());
  assert!(run.image_version.is_none());
  assert!(run.release_version.is_none());
}

#[test]
fn a_second_plan_replaces_the_first_for_the_same_device() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);
  let first = store.runs()[0].run_id.clone();

  clock.set(at(1));
  store.ingest(planned_webapp(), None);

  assert_eq!(store.runs().len(), 1, "the store keys one run per device");
  assert_ne!(store.runs()[0].run_id, first);
  assert_eq!(store.runs()[0].kind, OtaKind::InstalledWebapp);
}

// -- progress --

#[test]
fn progress_without_a_plan_is_ignored() {
  let (_clock, mut store) = store_at(at(0));
  let changes = store.ingest(progress(0, OtaPhaseSnapshot::Staged), None);

  assert!(changes.is_empty());
  assert!(store.runs().is_empty(), "progress cannot conjure a run nobody planned");
}

#[test]
fn download_progress_carries_bytes_and_rate() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  let changes = store.ingest(progress(0, downloading(40, 100, Some(20.0))), None);
  let run = only_run(&changes).expect("progress reports the run");

  assert_eq!(run.phase, OtaRunPhase::Downloading);
  assert_eq!(run.stage_received, Some(40));
  assert_eq!(run.stage_total, Some(100));
  assert_eq!(run.rate_per_sec, Some(20.0));
  assert_eq!(run.step_id, 0);
}

#[test]
fn phase_started_at_moves_only_when_the_phase_does() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(10));
  let first = store.ingest(progress(0, downloading(1, 100, None)), None);
  assert_eq!(only_run(&first).expect("run").phase_started_at_ms, at(10));

  clock.set(at(20));
  let same = store.ingest(progress(0, downloading(2, 100, None)), None);
  assert_eq!(
    only_run(&same).expect("run").phase_started_at_ms,
    at(10),
    "more of the same phase is not a new phase"
  );

  clock.set(at(30));
  let next = store.ingest(progress(1, OtaPhaseSnapshot::Staged), None);
  assert_eq!(only_run(&next).expect("run").phase_started_at_ms, at(30));
}

#[test]
fn applying_reports_delta_bytes_only_while_the_delta_pull_is_measurable() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  let pulling = store.ingest(progress(1, applying(OtaPhase::Writing, 10, 50, 4096)), None);
  let pulling = only_run(&pulling).expect("run");
  assert_eq!(pulling.phase, OtaRunPhase::Writing);
  assert_eq!(pulling.dwl_percent, Some(50));
  assert_eq!(pulling.stage_received, Some(4096));

  clock.set(at(2));
  let writing = store.ingest(progress(1, applying(OtaPhase::Writing, 60, 100, 8192)), None);
  assert!(
    only_run(&writing).expect("run").stage_received.is_none(),
    "past the delta pull there are no reported bytes, and a frozen number reads as a stall"
  );
}

#[test]
fn a_zero_byte_delta_pull_reports_no_bytes_either() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  let changes = store.ingest(progress(1, applying(OtaPhase::Writing, 10, 50, 0)), None);

  assert!(only_run(&changes).expect("run").stage_received.is_none());
}

#[test]
fn an_applying_tick_clears_the_stage_total_but_not_the_rate() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(0, downloading(90, 100, Some(250.0))), None);

  clock.set(at(2));
  let changes = store.ingest(progress(1, applying(OtaPhase::Writing, 0, 100, 0)), None);
  let run = only_run(&changes).expect("run");

  assert!(run.stage_total.is_none(), "the apply phase has no byte total to show");
  assert_eq!(
    run.rate_per_sec,
    Some(250.0),
    "the last measured transfer rate still feeds the eta"
  );
}

#[test]
fn the_idle_snapshot_rewinds_the_phase_without_clearing_the_numbers() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(0, downloading(40, 100, Some(20.0))), None);

  clock.set(at(2));
  let changes = store.ingest(progress(0, OtaPhaseSnapshot::Idle), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(run.phase, OtaRunPhase::Idle);
  assert_eq!(run.stage_received, Some(40));
  assert_eq!(run.stage_total, Some(100));
  assert_eq!(run.phase_started_at_ms, at(2));
}

#[test]
fn a_streaming_snapshot_replaces_the_download_numbers() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(0, downloading(100, 100, Some(20.0))), None);

  clock.set(at(2));
  let changes = store.ingest(
    progress(
      1,
      OtaPhaseSnapshot::Streaming {
        asset: "update.swu".into(),
        sent: 25,
        total: 200,
        rate_per_sec: Some(5.0),
        eta_seconds: Some(35.0),
      },
    ),
    None,
  );
  let run = only_run(&changes).expect("run");

  assert_eq!(run.phase, OtaRunPhase::Streaming);
  assert_eq!(run.stage_received, Some(25));
  assert_eq!(run.stage_total, Some(200));
  assert_eq!(run.rate_per_sec, Some(5.0));
}

#[test]
fn every_applying_phase_maps_to_its_own_run_phase() {
  let cases = [
    (OtaPhase::Streaming, OtaRunPhase::Streaming),
    (OtaPhase::Verifying, OtaRunPhase::Verifying),
    (OtaPhase::Writing, OtaRunPhase::Writing),
    (OtaPhase::Confirming, OtaRunPhase::Confirming),
    (OtaPhase::Reboot, OtaRunPhase::Reboot),
  ];

  for (wire, expected) in cases {
    let (clock, mut store) = store_at(at(0));
    store.ingest(planned_image(), None);
    clock.set(at(1));
    let changes = store.ingest(progress(1, applying(wire, 0, 0, 0)), None);

    assert_eq!(only_run(&changes).expect("run").phase, expected, "{wire:?}");
  }
}

#[test]
fn the_staged_snapshot_reads_as_writing_with_no_numbers() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(0, downloading(40, 100, Some(20.0))), None);

  clock.set(at(2));
  let changes = store.ingest(progress(1, OtaPhaseSnapshot::Staged), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(run.phase, OtaRunPhase::Writing);
  assert!(run.stage_received.is_none());
  assert!(run.stage_total.is_none());
}

#[test]
fn progress_for_a_step_outside_the_plan_keeps_the_last_understood_position() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(1, applying(OtaPhase::Writing, 50, 50, 0)), None);

  clock.set(at(2));
  let changes = store.ingest(progress(99, applying(OtaPhase::Writing, 60, 60, 0)), None);

  assert_eq!(
    only_run(&changes).expect("run").step_id,
    1,
    "a step id that does not index this plan must not rewind the run to its start"
  );
}

#[test]
fn an_empty_plan_accepts_any_step_id() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(failed(OtaKind::Image, "gateway not attached"), None);

  clock.set(at(1));
  let changes = store.ingest(progress(7, OtaPhaseSnapshot::Staged), None);

  assert_eq!(
    only_run(&changes).expect("run").step_id,
    7,
    "with no plan to index there is nothing to disagree with"
  );
}

#[test]
fn a_progress_tick_does_not_relabel_the_run_kind() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_webapp(), None);

  clock.set(at(1));
  let changes = store.ingest(progress(0, OtaPhaseSnapshot::Staged), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(
    run.kind,
    OtaKind::InstalledWebapp,
    "the plan names what is installing, so a tick carrying another kind cannot relabel the card"
  );
  assert_eq!(run.phase, OtaRunPhase::Writing, "the tick's progress still applies");
  assert_eq!(run.phase_started_at_ms, at(1));
  assert_eq!(
    store.open_run_kind(DEVICE),
    Some(OtaKind::InstalledWebapp),
    "the backstop terminal is reported for the kind that was planned"
  );
}

#[test]
fn a_failed_snapshot_is_not_a_failed_outcome() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  let changes = store.ingest(
    progress(
      1,
      OtaPhaseSnapshot::Failed {
        reason: "swupdate said no".into(),
      },
    ),
    None,
  );
  let run = only_run(&changes).expect("run");

  assert_eq!(run.phase, OtaRunPhase::Failed);
  assert_eq!(run.error.as_deref(), Some("swupdate said no"));
  assert!(
    run.outcome.is_none(),
    "a phase report is not a terminal event, so the run stays open"
  );
  assert_eq!(store.open_run_kind(DEVICE), Some(OtaKind::Image));
}

#[test]
fn a_completed_snapshot_is_not_a_succeeded_outcome() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  let changes = store.ingest(progress(2, OtaPhaseSnapshot::Completed), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(run.phase, OtaRunPhase::Completed);
  assert!(run.outcome.is_none());
  assert!(store.dismiss(DEVICE).is_none(), "and so it is not dismissable yet");
}

#[test]
fn progress_after_a_terminal_outcome_still_moves_the_phase() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(failed(OtaKind::Image, "write failed"), None);

  clock.set(at(2));
  let changes = store.ingest(progress(0, downloading(10, 100, None)), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(run.phase, OtaRunPhase::Downloading);
  assert_eq!(run.outcome, Some(OtaRunOutcome::Failed), "the outcome still stands");
}

// -- terminal events --

#[test]
fn updated_ends_the_run_and_clears_the_available_update() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(
    OtaPollEvent::UpdateAvailable {
      device_id: DEVICE.into(),
      release: "r".into(),
      daemon_version: "0.9.0".into(),
      image_version: "2026.06.0".into(),
    },
    None,
  );

  clock.set(at(1));
  store.ingest(planned_image(), None);

  clock.set(at(2));
  store.ingest(progress(0, downloading(5, 100, Some(3.0))), None);
  assert_eq!(
    store.available().len(),
    1,
    "an update was on offer before it was installed"
  );

  clock.set(at(3));
  let changes = store.ingest(updated(OtaKind::Image, "2026.06.0"), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(run.phase, OtaRunPhase::Completed);
  assert_eq!(run.outcome, Some(OtaRunOutcome::Succeeded));
  assert!(
    run.stage_received.is_none(),
    "leftover progress would render behind a finished bar"
  );
  assert!(run.stage_total.is_none());
  assert!(run.rate_per_sec.is_none());
  assert!(run.dwl_percent.is_none());
  assert!(run.error.is_none());
  assert!(
    store.available().is_empty(),
    "the update is no longer available; it is installed"
  );

  let cleared = only_available_release(&changes).expect("the cleared offer is announced, not silently dropped");
  assert!(
    cleared.is_none(),
    "listeners are told the offer is gone, not left holding the old one"
  );
}

#[test]
fn clearing_retracts_the_offer_and_announces_the_retraction() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(
    OtaPollEvent::UpdateAvailable {
      device_id: DEVICE.into(),
      release: "r".into(),
      daemon_version: "0.9.0".into(),
      image_version: "2026.06.0".into(),
    },
    None,
  );

  clock.set(at(1));
  let cleared = store.clear_available(DEVICE).expect("the retraction is announced");

  assert!(store.available().is_empty());
  assert_eq!(
    only_available_release(std::slice::from_ref(&cleared)),
    Some(None),
    "listeners are told the offer is gone, not left holding the old one"
  );
}

#[test]
fn clearing_an_offer_that_was_never_made_announces_nothing() {
  let (_clock, mut store) = store_at(at(0));

  assert!(
    store.clear_available(DEVICE).is_none(),
    "a poll that finds every device current must not broadcast a retraction per device per interval"
  );
}

#[test]
fn updated_with_no_version_keeps_the_planned_release() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  let changes = store.ingest(updated(OtaKind::Image, ""), None);

  assert_eq!(
    only_run(&changes).expect("run").release_version.as_deref(),
    Some("0.9.0+image.2026.06.0")
  );
}

#[test]
fn updated_without_a_plan_is_ignored() {
  let (_clock, mut store) = store_at(at(0));
  let changes = store.ingest(updated(OtaKind::Image, "2026.06.0"), None);

  assert!(changes.is_empty());
  assert!(store.runs().is_empty());
}

#[test]
fn failed_records_the_reason_and_clears_the_numbers() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(0, downloading(40, 100, Some(20.0))), None);

  clock.set(at(2));
  let changes = store.ingest(failed(OtaKind::Image, "write failed"), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(run.phase, OtaRunPhase::Failed);
  assert_eq!(run.outcome, Some(OtaRunOutcome::Failed));
  assert_eq!(run.error.as_deref(), Some("write failed"));
  assert!(run.stage_received.is_none());
  assert!(run.stage_total.is_none());
  assert!(run.rate_per_sec.is_none());
}

#[test]
fn cancellation_is_its_own_outcome() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  let changes = store.ingest(failed(OtaKind::Image, CANCELLED_REASON), None);

  assert_eq!(
    only_run(&changes).expect("run").outcome,
    Some(OtaRunOutcome::Cancelled),
    "a user stopping an update is not a failure to report"
  );
}

#[test]
fn a_reason_that_merely_contains_cancelled_is_a_failure() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  let changes = store.ingest(failed(OtaKind::Image, "the daemon cancelled the write"), None);

  assert_eq!(only_run(&changes).expect("run").outcome, Some(OtaRunOutcome::Failed));
}

#[test]
fn a_failure_with_no_plan_still_opens_a_run_to_report_it() {
  let (_clock, mut store) = store_at(at(0));
  let changes = store.ingest(failed(OtaKind::Daemon, "gateway not attached"), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(run.outcome, Some(OtaRunOutcome::Failed));
  assert_eq!(run.kind, OtaKind::Daemon);
  assert!(run.steps.is_empty());
  assert_eq!(run.started_at_ms, at(0));
}

#[test]
fn a_second_failure_overwrites_the_first_reason() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(failed(OtaKind::Image, "write failed"), None);

  clock.set(at(2));
  let changes = store.ingest(failed(OtaKind::Image, CANCELLED_REASON), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(run.error.as_deref(), Some(CANCELLED_REASON));
  assert_eq!(run.outcome, Some(OtaRunOutcome::Cancelled));
}

#[test]
fn a_fresh_transfer_drops_what_the_device_said_about_the_last_one() {
  let (_clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);
  store.ingest(progress(1, applying(OtaPhase::Writing, 100, 40, 4_096)), None);
  assert!(store.runs()[0].dwl_percent.is_some());

  store.ingest(progress(0, downloading(10, 100, None)), None);

  assert!(
    store.runs()[0].dwl_percent.is_none(),
    "the next piece in a batch starts from nothing, not from the previous piece's apply"
  );
}

// -- interrupt --

#[test]
fn a_link_that_dies_mid_download_ends_the_run() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(0, downloading(40, 100, Some(20.0))), None);

  clock.set(at(2));
  let interrupted = store.interrupt(DEVICE).expect("an open run is interruptible");

  assert_eq!(interrupted.outcome, Some(OtaRunOutcome::Failed));
  assert_eq!(interrupted.phase, OtaRunPhase::Failed);
  assert!(interrupted.resumable, "a planned run keeps what it takes to re-drive");
  assert_eq!(interrupted.error.as_deref(), Some(RESUMABLE_REASON));
  assert_eq!(
    interrupted.phase_started_at_ms,
    at(1),
    "the interrupt reports the run, it does not restart its clock"
  );
  assert!(store.dismiss(DEVICE).is_some(), "so the card can be cleared");
}

#[test]
fn an_interrupted_run_hands_its_drive_parameters_over_exactly_once() {
  let (_clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);
  store.ingest(progress(0, downloading(40, 100, None)), None);
  store.interrupt(DEVICE).expect("an open run is interruptible");

  let resume = store.take_resume(DEVICE).expect("the parameters it was driven with");
  assert_eq!(resume.channel, "stable");
  assert_eq!(resume.root_url, "https://ota.bridgething.com");
  assert_eq!(resume.version, "0.9.0+image.2026.06.0");

  assert!(
    store.take_resume(DEVICE).is_none(),
    "a second reconnect must not start the same run twice"
  );
}

#[test]
fn a_run_with_nothing_to_re_drive_reads_as_a_plain_failure() {
  let (_clock, mut store) = store_at(at(0));
  store.ingest(
    OtaPollEvent::Failed {
      device_id: DEVICE.into(),
      kind: OtaKind::Daemon,
      reason: "boom".into(),
    },
    None,
  );

  assert!(store.take_resume(DEVICE).is_none());
}

#[test]
fn a_link_that_dies_while_the_device_reboots_leaves_the_run_alone() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(2, applying(OtaPhase::Reboot, 100, 100, 0)), None);

  assert!(
    store.interrupt(DEVICE).is_none(),
    "the run is what asked the device to go away, so its disconnect is not a failure"
  );
  assert!(store.runs()[0].outcome.is_none());
}

#[test]
fn a_link_that_dies_while_the_device_confirms_leaves_the_run_alone() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(1, applying(OtaPhase::Confirming, 100, 100, 0)), None);

  assert!(store.interrupt(DEVICE).is_none());
  assert!(store.runs()[0].outcome.is_none());
}

#[test]
fn interrupt_leaves_a_finished_run_alone() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(updated(OtaKind::Image, "2026.06.0"), None);

  assert!(store.interrupt(DEVICE).is_none());
  assert_eq!(store.runs()[0].outcome, Some(OtaRunOutcome::Succeeded));
}

#[test]
fn interrupt_and_dismiss_and_annotate_on_an_unknown_device_are_no_ops() {
  let (_clock, mut store) = store_at(at(0));

  assert!(store.interrupt(DEVICE).is_none());
  assert!(store.dismiss(DEVICE).is_none());
  assert!(store.annotate_webapp(DEVICE, Some("abc"), Some("Weather")).is_none());
  assert!(store.note_meta(DEVICE, "0.9.0", "2026.06.0").is_none());
  assert!(store.open_run_kind(DEVICE).is_none());
  assert!(store.runs().is_empty());
}

// -- dismiss --

#[test]
fn dismiss_clears_a_finished_run() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(updated(OtaKind::Image, "2026.06.0"), None);

  let dismissed = store.dismiss(DEVICE).expect("a finished run is dismissable");
  assert_eq!(dismissed.phase, OtaRunPhase::Idle, "the card is told to go quiet");
  assert!(store.runs().is_empty());
}

#[test]
fn dismiss_refuses_a_run_still_in_flight() {
  let (_clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  assert!(
    store.dismiss(DEVICE).is_none(),
    "dismissing a card must not make the update it describes invisible"
  );
  assert_eq!(store.runs().len(), 1);
}

#[test]
fn an_abandoned_run_is_reported_and_then_dismissable() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);
  assert_eq!(
    store.open_run_kind(DEVICE),
    Some(OtaKind::Image),
    "a planned run has not ended"
  );
  assert!(
    store.dismiss(DEVICE).is_none(),
    "and cannot be dismissed while it is open"
  );

  clock.set(at(1));
  store.ingest(failed(OtaKind::Image, "abandoned"), None);

  assert!(store.open_run_kind(DEVICE).is_none(), "the run has ended");
  assert!(store.dismiss(DEVICE).is_some(), "so the card can be cleared");
  assert!(store.runs().is_empty());
}

#[test]
fn open_run_kind_ignores_a_finished_run() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(updated(OtaKind::Image, "2026.06.0"), None);

  assert!(
    store.open_run_kind(DEVICE).is_none(),
    "a run that reported a result needs no backstop terminal"
  );
}

#[test]
fn dismiss_does_not_clear_the_available_offer() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(
    OtaPollEvent::UpdateAvailable {
      device_id: DEVICE.into(),
      release: "r".into(),
      daemon_version: "0.9.1".into(),
      image_version: "2026.07.0".into(),
    },
    None,
  );

  clock.set(at(1));
  store.ingest(planned_image(), None);

  clock.set(at(2));
  store.ingest(failed(OtaKind::Image, "write failed"), None);

  assert!(store.dismiss(DEVICE).is_some());
  assert_eq!(
    store.available().len(),
    1,
    "the update on offer survives the card being cleared"
  );
}

#[test]
fn a_later_offer_replaces_an_earlier_one_for_the_same_device() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(
    OtaPollEvent::UpdateAvailable {
      device_id: DEVICE.into(),
      release: "r1".into(),
      daemon_version: "0.9.0".into(),
      image_version: "2026.06.0".into(),
    },
    None,
  );

  clock.set(at(1));
  store.ingest(
    OtaPollEvent::UpdateAvailable {
      device_id: DEVICE.into(),
      release: "r2".into(),
      daemon_version: "0.9.1".into(),
      image_version: "2026.07.0".into(),
    },
    None,
  );

  assert_eq!(store.available().len(), 1);
  assert_eq!(store.available()[0].release_version.as_deref(), Some("r2"));
}

#[test]
fn an_offer_with_no_versions_reports_them_empty_rather_than_unset() {
  let (_clock, mut store) = store_at(at(0));
  store.ingest(
    OtaPollEvent::UpdateAvailable {
      device_id: DEVICE.into(),
      release: String::new(),
      daemon_version: String::new(),
      image_version: String::new(),
    },
    None,
  );

  let offer = store.available()[0].clone();

  assert_eq!(
    (
      offer.release_version.as_deref(),
      offer.daemon_version.as_deref(),
      offer.image_version.as_deref()
    ),
    (Some(""), Some(""), Some("")),
    "an offer carries what the feed said; only a plan turns an empty version into an unset one"
  );
}

// -- note_meta --

#[test]
fn meta_on_the_target_version_clears_the_run() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(updated(OtaKind::Image, "2026.06.0"), None);

  let cleared = store
    .note_meta(DEVICE, "0.9.0", "2026.06.0")
    .expect("the run is confirmed");
  assert_eq!(cleared.phase, OtaRunPhase::Idle);
  assert_eq!(cleared.outcome, Some(OtaRunOutcome::Succeeded));
  assert!(store.runs().is_empty());
}

#[test]
fn meta_on_the_wrong_version_leaves_the_run_alone() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(updated(OtaKind::Image, "2026.06.0"), None);

  assert!(store.note_meta(DEVICE, "0.9.0", "2026.05.0").is_none());
  assert_eq!(
    store.runs().len(),
    1,
    "the device came back on the old image; the run did not land"
  );
}

#[test]
fn meta_on_the_target_version_rescues_a_run_that_timed_out_rebooting() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(90));
  store.ingest(failed(OtaKind::Image, "ota stalled: no progress within 60s"), None);
  assert_eq!(store.runs()[0].outcome, Some(OtaRunOutcome::Failed));

  let cleared = store
    .note_meta(DEVICE, "0.9.0", "2026.06.0")
    .expect("the run is confirmed");
  assert_eq!(
    cleared.outcome,
    Some(OtaRunOutcome::Succeeded),
    "the version the device came back on outranks the guess"
  );
  assert!(cleared.error.is_none());
  assert!(store.runs().is_empty());
}

#[test]
fn meta_does_not_rescue_a_run_that_failed_before_reaching_the_device() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(failed(OtaKind::Image, "bundle download failed"), None);

  assert!(
    store.note_meta(DEVICE, "0.8.0", "2026.05.0").is_none(),
    "the device is still on the old versions, so nothing confirms this run"
  );
  assert_eq!(store.runs().len(), 1);
}

#[test]
fn meta_clears_a_targeted_run_that_is_still_in_flight() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(progress(0, downloading(40, 100, Some(20.0))), None);

  let cleared = store
    .note_meta(DEVICE, "0.9.0", "2026.06.0")
    .expect("the device is already on the target versions");
  assert_eq!(cleared.outcome, Some(OtaRunOutcome::Succeeded));
  assert!(store.runs().is_empty());
}

#[test]
fn a_daemon_only_run_is_confirmed_by_the_daemon_version_alone() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(
    OtaPollEvent::Planned {
      device_id: DEVICE.into(),
      kind: OtaKind::Daemon,
      release: "0.9.1".into(),
      daemon_version: "0.9.1".into(),
      image_version: String::new(),
      channel: "stable".into(),
      root_url: "https://ota.bridgething.com".into(),
      steps: vec![plan_step(0, OtaStepKind::Download)],
    },
    None,
  );

  clock.set(at(1));
  let cleared = store
    .note_meta(DEVICE, "0.9.1", "any-image-at-all")
    .expect("nothing was targeted about the image");
  assert_eq!(cleared.outcome, Some(OtaRunOutcome::Succeeded));
  assert!(store.runs().is_empty());
}

#[test]
fn meta_leaves_a_webapp_run_alone_until_it_says_it_succeeded() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_webapp(), None);

  assert!(
    store.note_meta(DEVICE, "0.9.0", "2026.06.0").is_none(),
    "a webapp install targets no version, so device meta confirms nothing about it"
  );
  assert_eq!(store.runs().len(), 1);

  clock.set(at(1));
  store.ingest(updated(OtaKind::InstalledWebapp, "1.4.0"), None);

  assert!(store.note_meta(DEVICE, "0.9.0", "2026.06.0").is_some());
  assert!(store.runs().is_empty());
}

// -- annotate_webapp --

#[test]
fn annotate_names_the_app_being_installed() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_webapp(), None);

  clock.set(at(1));
  let run = store
    .annotate_webapp(DEVICE, Some("abc"), Some("Weather"))
    .expect("the run is annotated");

  assert_eq!(run.webapp_name.as_deref(), Some("Weather"));
  assert_eq!(store.runs()[0].webapp_id.as_deref(), Some("abc"));
}

#[test]
fn annotate_with_nothing_clears_the_name() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_webapp(), None);
  store.annotate_webapp(DEVICE, Some("abc"), Some("Weather"));

  clock.set(at(1));
  let run = store.annotate_webapp(DEVICE, None, None).expect("the run is annotated");

  assert!(run.webapp_id.is_none());
  assert!(run.webapp_name.is_none());
}

#[test]
fn annotate_without_a_run_is_a_no_op() {
  let (_clock, mut store) = store_at(at(0));

  assert!(store.annotate_webapp(DEVICE, Some("abc"), Some("Weather")).is_none());
}

#[test]
fn a_webapp_run_carries_its_name_and_its_installed_version_separately() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_webapp(), None);
  store.annotate_webapp(DEVICE, Some("abc"), Some("Weather"));

  clock.set(at(1));
  let changes = store.ingest(updated(OtaKind::InstalledWebapp, "1.4.0"), None);
  let run = only_run(&changes).expect("run");

  assert_eq!(
    run.webapp_name.as_deref(),
    Some("Weather"),
    "the app's name identifies it"
  );
  assert_eq!(
    run.release_version.as_deref(),
    Some("1.4.0"),
    "and the version field holds a version, not the name again"
  );
}

// -- poll status --

#[test]
fn poll_failure_keeps_the_last_good_timestamp() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(
    OtaPollEvent::ManifestPolled {
      updated_at: "2026-06-01T00:00:00Z".into(),
    },
    None,
  );

  clock.set(at(1));
  store.ingest(
    OtaPollEvent::ManifestPollFailed {
      reason: "offline".into(),
    },
    None,
  );

  assert_eq!(
    store.poll_status().last_polled_at.as_deref(),
    Some("2026-06-01T00:00:00Z")
  );
  assert_eq!(store.poll_status().error.as_deref(), Some("offline"));
}

#[test]
fn a_successful_poll_clears_the_previous_error() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(
    OtaPollEvent::ManifestPollFailed {
      reason: "offline".into(),
    },
    None,
  );

  clock.set(at(1));
  store.ingest(
    OtaPollEvent::ManifestPolled {
      updated_at: "2026-06-02T00:00:00Z".into(),
    },
    None,
  );

  assert!(store.poll_status().error.is_none());
  assert_eq!(
    store.poll_status().last_polled_at.as_deref(),
    Some("2026-06-02T00:00:00Z")
  );
}

#[test]
fn a_poll_change_is_the_only_thing_a_poll_event_reports() {
  let (_clock, mut store) = store_at(at(0));
  let changes = store.ingest(
    OtaPollEvent::ManifestPolled {
      updated_at: "2026-06-01T00:00:00Z".into(),
    },
    None,
  );

  assert_eq!(changes.len(), 1);
  assert!(matches!(changes[0], OtaStoreChange::Poll(_)));
}

// -- isolation --

#[test]
fn runs_on_different_devices_do_not_interfere() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);

  clock.set(at(1));
  store.ingest(
    OtaPollEvent::Planned {
      device_id: OTHER.into(),
      kind: OtaKind::Daemon,
      release: "0.9.1".into(),
      daemon_version: "0.9.1".into(),
      image_version: String::new(),
      channel: "stable".into(),
      root_url: "https://ota.bridgething.com".into(),
      steps: vec![plan_step(0, OtaStepKind::Download)],
    },
    None,
  );

  clock.set(at(2));
  store.ingest(
    OtaPollEvent::Failed {
      device_id: OTHER.into(),
      kind: OtaKind::Daemon,
      reason: "nope".into(),
    },
    None,
  );

  assert_eq!(store.runs().len(), 2);
  let this = store
    .runs()
    .into_iter()
    .find(|run| run.device_id == DEVICE)
    .expect("run");
  let other = store
    .runs()
    .into_iter()
    .find(|run| run.device_id == OTHER)
    .expect("run");
  assert!(this.outcome.is_none());
  assert_eq!(other.outcome, Some(OtaRunOutcome::Failed));
}

#[test]
fn runs_and_offers_are_reported_in_device_id_order() {
  let (clock, mut store) = store_at(at(0));
  store.ingest(planned_image(), None);
  store.ingest(
    OtaPollEvent::UpdateAvailable {
      device_id: DEVICE.into(),
      release: "r".into(),
      daemon_version: "0.9.0".into(),
      image_version: "2026.06.0".into(),
    },
    None,
  );

  clock.set(at(1));
  store.ingest(
    OtaPollEvent::Planned {
      device_id: OTHER.into(),
      kind: OtaKind::Daemon,
      release: "0.9.1".into(),
      daemon_version: "0.9.1".into(),
      image_version: String::new(),
      channel: "stable".into(),
      root_url: "https://ota.bridgething.com".into(),
      steps: vec![plan_step(0, OtaStepKind::Download)],
    },
    None,
  );
  store.ingest(
    OtaPollEvent::UpdateAvailable {
      device_id: OTHER.into(),
      release: "r".into(),
      daemon_version: "0.9.1".into(),
      image_version: String::new(),
    },
    None,
  );

  let runs: Vec<&str> = store.runs().iter().map(|run| run.device_id.as_str()).collect();
  let offers: Vec<&str> = store
    .available()
    .iter()
    .map(|available| available.device_id.as_str())
    .collect();

  assert_eq!(runs, vec![OTHER, DEVICE], "insertion order must not leak to a caller");
  assert_eq!(offers, vec![OTHER, DEVICE]);
}
