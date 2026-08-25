use bridgething_delivery::ota::{
  event::{OtaPlanStep, OtaStepKind},
  progress::ota_progress,
  run_store::{OtaRun, OtaRunOutcome, OtaRunPhase},
};
use libbridgething::OtaKind;

const EPOCH_MS: u64 = 1_700_000_000_000;

fn image_steps() -> Vec<OtaPlanStep> {
  vec![
    OtaPlanStep {
      id: 0,
      kind: OtaStepKind::Download,
      label: "downloading".into(),
      bytes: 100_000_000,
    },
    OtaPlanStep {
      id: 1,
      kind: OtaStepKind::Apply,
      label: "writing".into(),
      bytes: 100_000_000,
    },
    OtaPlanStep {
      id: 2,
      kind: OtaStepKind::Reboot,
      label: "rebooting".into(),
      bytes: 0,
    },
  ]
}

fn run() -> OtaRun {
  OtaRun {
    run_id: "run-1".into(),
    device_id: "AA:BB:CC:DD:EE:FF".into(),
    identity: None,
    kind: OtaKind::Image,
    phase: OtaRunPhase::Downloading,
    steps: image_steps(),
    step_id: 0,
    started_at_ms: EPOCH_MS,
    phase_started_at_ms: EPOCH_MS,
    stage_received: Some(50_000_000),
    stage_total: Some(100_000_000),
    rate_per_sec: Some(1_000_000.0),
    dwl_percent: None,
    outcome: None,
    error: None,
    release_version: None,
    daemon_version: None,
    image_version: None,
    channel: None,
    root_url: None,
    resumable: false,
    webapp_id: None,
    webapp_name: None,
  }
}

fn lifecycle() -> Vec<OtaRun> {
  let mut frames = Vec::new();

  for pct in (0..=100).step_by(10) {
    let mut frame = run();
    frame.step_id = 0;
    frame.phase = OtaRunPhase::Downloading;
    frame.stage_received = Some(1_000_000 * pct);
    frame.stage_total = Some(100_000_000);
    frames.push(frame);
  }

  for pct in (0..=100).step_by(25) {
    let mut frame = run();
    frame.step_id = 1;
    frame.phase = OtaRunPhase::Writing;
    frame.dwl_percent = Some(pct as u32);
    frame.stage_received = None;
    frame.stage_total = None;
    frames.push(frame);
  }

  let mut rebooting = run();
  rebooting.step_id = 2;
  rebooting.phase = OtaRunPhase::Reboot;
  rebooting.phase_started_at_ms = EPOCH_MS;
  frames.push(rebooting);

  frames
}

fn percents(frames: &[OtaRun]) -> Vec<u32> {
  frames
    .iter()
    .map(|frame| ota_progress(frame, EPOCH_MS).percent)
    .collect()
}

#[test]
fn it_never_runs_backwards_over_the_life_of_an_update() {
  let seen = percents(&lifecycle());
  let drops: Vec<(usize, u32, u32)> = seen
    .windows(2)
    .enumerate()
    .filter(|(_, pair)| pair[1] < pair[0])
    .map(|(at, pair)| (at, pair[0], pair[1]))
    .collect();

  assert!(drops.is_empty(), "the bar went backwards at {drops:?}");
}

#[test]
fn it_stays_within_its_own_bounds() {
  for percent in percents(&lifecycle()) {
    assert!(percent <= 100, "{percent} is not a percentage");
  }
}

#[test]
fn it_does_not_move_when_only_the_transfer_rate_estimate_changes() {
  let mut slow = run();
  slow.rate_per_sec = Some(1_000_000.0);
  let mut fast = run();
  fast.rate_per_sec = Some(5_000_000.0);

  assert_eq!(
    ota_progress(&fast, EPOCH_MS).percent,
    ota_progress(&slow, EPOCH_MS).percent
  );
}

#[test]
fn an_unmeasurable_rate_does_not_move_it_either() {
  let mut measured = run();
  measured.rate_per_sec = Some(1_000_000.0);
  let mut unknown = run();
  unknown.rate_per_sec = None;

  assert_eq!(
    ota_progress(&unknown, EPOCH_MS).percent,
    ota_progress(&measured, EPOCH_MS).percent
  );
}

#[test]
fn the_eta_still_gets_shorter_when_the_transfer_speeds_up() {
  let mut slow = run();
  slow.rate_per_sec = Some(1_000_000.0);
  let mut fast = run();
  fast.rate_per_sec = Some(8_000_000.0);

  let slow = ota_progress(&slow, EPOCH_MS).eta_seconds.expect("a measured eta");
  let fast = ota_progress(&fast, EPOCH_MS).eta_seconds.expect("a measured eta");

  assert!(fast < slow, "{fast} is not shorter than {slow}");
}

#[test]
fn a_finished_update_reads_as_finished() {
  let mut done = run();
  done.outcome = Some(OtaRunOutcome::Succeeded);

  let progress = ota_progress(&done, EPOCH_MS);

  assert_eq!(progress.percent, 100);
  assert_eq!(progress.eta_seconds, Some(0));
  assert!(progress.step_label.is_none(), "a finished update names no step");
}

#[test]
fn an_unfinished_update_never_reads_as_finished() {
  let mut rebooting = run();
  rebooting.step_id = 2;
  rebooting.phase = OtaRunPhase::Reboot;

  assert_eq!(
    ota_progress(&rebooting, EPOCH_MS + 86_400_000).percent,
    99,
    "a hundred percent is reserved for an update that actually landed"
  );
}

#[test]
fn a_plan_with_no_weight_reports_nothing_rather_than_dividing_by_it() {
  let mut empty = run();
  empty.steps = Vec::new();

  let progress = ota_progress(&empty, EPOCH_MS);

  assert_eq!(progress.percent, 0);
  assert_eq!(progress.step_index, 0);
  assert_eq!(progress.step_count, 0);
  assert!(progress.step_label.is_none());
  assert!(
    progress.eta_seconds.is_none(),
    "with no plan there is nothing to estimate"
  );
}

#[test]
fn the_link_is_weighed_as_far_slower_than_the_phones_own_internet() {
  let mut download = run();
  download.rate_per_sec = None;
  download.stage_received = None;
  download.stage_total = None;
  download.steps = vec![OtaPlanStep {
    id: 0,
    kind: OtaStepKind::Download,
    label: "moving".into(),
    bytes: 10_000_000,
  }];

  let mut stream = download.clone();
  stream.steps[0].kind = OtaStepKind::Stream;

  let pulling = ota_progress(&download, EPOCH_MS).eta_seconds.expect("an estimate");
  let sending = ota_progress(&stream, EPOCH_MS).eta_seconds.expect("an estimate");

  assert!(
    sending > pulling * 4,
    "the same bytes over the link ({sending}s) should dwarf the download ({pulling}s)"
  );
}

#[test]
fn a_zero_byte_transfer_step_floors_at_a_second_even_with_a_measured_rate() {
  let mut empty = run();
  empty.stage_received = None;
  empty.stage_total = None;
  empty.rate_per_sec = Some(8_000_000.0);
  empty.steps = vec![OtaPlanStep {
    id: 0,
    kind: OtaStepKind::Stream,
    label: "moving".into(),
    bytes: 0,
  }];

  assert_eq!(
    ota_progress(&empty, EPOCH_MS).eta_seconds,
    Some(1),
    "a step with nothing to divide by still takes the minimum"
  );
}

#[test]
fn an_apply_step_with_no_byte_count_falls_back_to_the_batch_estimate() {
  let mut weighed = run();
  weighed.stage_received = None;
  weighed.stage_total = None;
  weighed.dwl_percent = Some(0);
  weighed.steps = vec![OtaPlanStep {
    id: 0,
    kind: OtaStepKind::Apply,
    label: "writing".into(),
    bytes: 15_000_000,
  }];

  let mut unweighed = weighed.clone();
  unweighed.steps[0].bytes = 0;

  assert_eq!(
    ota_progress(&weighed, EPOCH_MS).eta_seconds,
    Some(38),
    "a sized apply is divided by the apply throughput"
  );
  assert_eq!(
    ota_progress(&unweighed, EPOCH_MS).eta_seconds,
    Some(15),
    "an apply that names no bytes is a flat batch estimate, not the one-second floor"
  );
}

#[test]
fn the_step_position_is_named_by_the_plan_not_by_the_step_id() {
  let mut mid = run();
  mid.step_id = 1;

  let progress = ota_progress(&mid, EPOCH_MS);

  assert_eq!(progress.step_index, 1);
  assert_eq!(progress.step_count, 3);
  assert_eq!(progress.step_label.as_deref(), Some("writing"));
}

#[test]
fn a_step_id_outside_the_plan_lands_on_the_step_the_phase_is_describing() {
  let mut stray = run();
  stray.step_id = 99;
  stray.phase = OtaRunPhase::Writing;
  stray.dwl_percent = Some(0);
  stray.stage_received = None;
  stray.stage_total = None;

  let progress = ota_progress(&stray, EPOCH_MS);

  assert_eq!(progress.step_index, 1);
  assert_eq!(progress.step_label.as_deref(), Some("writing"));
  assert!(
    progress.percent >= ota_progress(&run(), EPOCH_MS).percent,
    "an unrecognised step id must not throw the bar back to the start"
  );
}

#[test]
fn a_stray_step_id_with_nothing_to_match_still_names_the_first_step() {
  let mut stray = run();
  stray.step_id = 99;
  stray.phase = OtaRunPhase::Idle;

  assert_eq!(ota_progress(&stray, EPOCH_MS).step_index, 0);
}

#[test]
fn a_transfer_step_the_device_has_moved_past_counts_whole() {
  let mut stuck = run();
  stuck.step_id = 0;
  stuck.phase = OtaRunPhase::Writing;
  stuck.stage_received = None;
  stuck.stage_total = None;

  let mut moving = run();
  moving.stage_received = Some(99_000_000);

  assert!(
    ota_progress(&stuck, EPOCH_MS).percent >= ota_progress(&moving, EPOCH_MS).percent,
    "an applying device means the download behind the cursor is finished, not zero"
  );
}

#[test]
fn a_device_reporting_its_own_work_puts_the_transfer_behind_the_cursor() {
  let mut applying = run();
  applying.step_id = 0;
  applying.phase = OtaRunPhase::Streaming;
  applying.dwl_percent = Some(20);
  applying.stage_received = None;
  applying.stage_total = None;

  assert!(
    ota_progress(&applying, EPOCH_MS).percent > ota_progress(&run(), EPOCH_MS).percent,
    "an applying snapshot clears the staged totals, which must not read as a transfer at zero"
  );
}

#[test]
fn an_image_apply_tracks_the_delta_pull() {
  let mut image = run();
  image.step_id = 1;
  image.phase = OtaRunPhase::Streaming;
  image.dwl_percent = Some(50);
  image.stage_received = None;
  image.stage_total = None;

  let mut none = image.clone();
  none.dwl_percent = Some(0);

  assert!(ota_progress(&image, EPOCH_MS).percent > ota_progress(&none, EPOCH_MS).percent);
}

#[test]
fn a_non_image_apply_fills_in_by_phase_instead_of_jumping_whole() {
  let mut webapp = run();
  webapp.kind = OtaKind::InstalledWebapp;
  webapp.step_id = 1;
  webapp.stage_received = None;
  webapp.stage_total = None;
  webapp.dwl_percent = None;

  let at = |phase| {
    let mut frame = webapp.clone();
    frame.phase = phase;
    ota_progress(&frame, EPOCH_MS).percent
  };

  let verifying = at(OtaRunPhase::Verifying);
  let writing = at(OtaRunPhase::Writing);
  let confirming = at(OtaRunPhase::Confirming);

  assert!(
    verifying < writing && writing < confirming,
    "a batch apply climbs {verifying} to {writing} to {confirming} rather than switching on"
  );
}

#[test]
fn an_image_apply_is_whole_once_the_device_is_confirming_or_rebooting() {
  let mut image = run();
  image.step_id = 1;
  image.phase = OtaRunPhase::Confirming;
  image.dwl_percent = Some(0);
  image.stage_received = None;
  image.stage_total = None;

  let mut rebooting = image.clone();
  rebooting.phase = OtaRunPhase::Reboot;

  let mut writing = image.clone();
  writing.phase = OtaRunPhase::Writing;

  let confirming_percent = ota_progress(&image, EPOCH_MS).percent;
  assert!(confirming_percent > ota_progress(&writing, EPOCH_MS).percent);
  assert_eq!(ota_progress(&rebooting, EPOCH_MS).percent, confirming_percent);
}

#[test]
fn the_reboot_step_fills_in_from_the_moment_the_phase_started() {
  let mut rebooting = run();
  rebooting.step_id = 2;
  rebooting.phase = OtaRunPhase::Reboot;
  rebooting.phase_started_at_ms = EPOCH_MS;

  let start = ota_progress(&rebooting, EPOCH_MS).percent;
  let later = ota_progress(&rebooting, EPOCH_MS + 45_000).percent;
  let much_later = ota_progress(&rebooting, EPOCH_MS + 600_000).percent;

  assert!(later > start, "the reboot bar moves on the clock alone");
  assert!(much_later > later);
  assert!(much_later <= 100);
}

#[test]
fn the_reboot_step_settles_rather_than_creeping_forever() {
  let mut rebooting = run();
  rebooting.step_id = 2;
  rebooting.phase = OtaRunPhase::Reboot;
  rebooting.phase_started_at_ms = EPOCH_MS;

  assert_eq!(
    ota_progress(&rebooting, EPOCH_MS + 90_000).percent,
    ota_progress(&rebooting, EPOCH_MS + 3_600_000).percent,
    "the reboot step is spent by twice its nominal wait"
  );
}

#[test]
fn a_clock_behind_the_phase_start_does_not_run_the_reboot_bar_backwards() {
  let mut rebooting = run();
  rebooting.step_id = 2;
  rebooting.phase = OtaRunPhase::Reboot;
  rebooting.phase_started_at_ms = EPOCH_MS;

  assert_eq!(
    ota_progress(&rebooting, EPOCH_MS - 10_000).percent,
    ota_progress(&rebooting, EPOCH_MS).percent
  );
}

#[test]
fn the_image_plan_spends_most_of_the_bar_where_it_spends_the_wall_clock() {
  let mut planned = run();
  planned.steps = vec![
    OtaPlanStep {
      id: 0,
      kind: OtaStepKind::Download,
      label: "update.swu".into(),
      bytes: 865_792,
    },
    OtaPlanStep {
      id: 1,
      kind: OtaStepKind::Download,
      label: "system.img.zck".into(),
      bytes: 195_242_214,
    },
    OtaPlanStep {
      id: 2,
      kind: OtaStepKind::Download,
      label: "boot.vfat.zck".into(),
      bytes: 9_986_415,
    },
    OtaPlanStep {
      id: 3,
      kind: OtaStepKind::Stream,
      label: "update.swu".into(),
      bytes: 865_792,
    },
    OtaPlanStep {
      id: 4,
      kind: OtaStepKind::Apply,
      label: "installing image".into(),
      bytes: 195_242_214,
    },
    OtaPlanStep {
      id: 5,
      kind: OtaStepKind::Reboot,
      label: "reboot".into(),
      bytes: 0,
    },
  ];
  planned.stage_received = None;
  planned.stage_total = None;

  let downloaded = {
    let mut frame = planned.clone();
    frame.step_id = 4;
    frame.phase = OtaRunPhase::Writing;
    frame.dwl_percent = Some(0);
    ota_progress(&frame, EPOCH_MS).percent
  };

  assert!(
    (5..=20).contains(&downloaded),
    "every artifact downloaded is a small slice of an image update, not {downloaded} percent"
  );
}
