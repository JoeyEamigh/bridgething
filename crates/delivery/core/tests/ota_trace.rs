use std::{fs, path::PathBuf};

use bridgething_delivery::ota::{
  autopush::{
    AutoPushSchedule, BACKOFF_BASE_MS, BACKOFF_JITTER_MS, BACKOFF_MAX_MS, BACKOFF_SHIFT_CAP, LINK_STABILITY_MS,
    MIN_POLL_INTERVAL_SECONDS, MIN_RESUME_DELAY_MS,
  },
  event::{CANCELLED_REASON, OtaPhaseSnapshot, OtaPlanStep, OtaPollEvent, OtaStepKind},
  run_store::{OtaRun, OtaRunOutcome, OtaRunPhase, OtaRunStore, OtaStoreChange},
};
use libbridgething::{OtaKind, OtaPhase};
use serde_json::{Map, Value, json};
use support::TestClock;

mod support;

fn fixtures_dir() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../lib/fixtures")
}

#[test]
fn emits_ota_trace() {
  let emitted = run_corpus();
  fs::write(
    fixtures_dir().join("ota-trace.rust.json"),
    format!("{}\n", serde_json::to_string_pretty(&emitted).expect("serializes")),
  )
  .expect("trace written");
}

#[test]
fn rust_arm_conforms_to_the_frozen_expectation() {
  let expectation: Value = serde_json::from_str(
    &fs::read_to_string(fixtures_dir().join("ota-trace.expected.json")).expect("expectation readable"),
  )
  .expect("expectation parses");
  let emitted = run_corpus();

  assert_eq!(
    emitted["constants"], expectation["constants"],
    "ota constants moved; reconcile them into the expectation"
  );

  let got = emitted["cases"].as_array().expect("emitted cases");
  let want = expectation["cases"].as_array().expect("expected cases");
  assert_eq!(got.len(), want.len(), "case count");

  for (got_case, want_case) in got.iter().zip(want) {
    let name = want_case["name"].as_str().expect("case name");
    assert_eq!(got_case["name"], want_case["name"], "case order");
    assert_eq!(got_case["component"], want_case["component"], "component of {name}");

    let got_steps = got_case["steps"].as_array().expect("emitted steps");
    let want_steps = want_case["steps"].as_array().expect("expected steps");
    assert_eq!(got_steps.len(), want_steps.len(), "step count in {name}");

    for (at, (got_step, want_step)) in got_steps.iter().zip(want_steps).enumerate() {
      let got_step = got_step.as_object().expect("emitted step is an object");
      let want_step = want_step.as_object().expect("expected step is an object");

      let got_keys: Vec<&String> = got_step.keys().collect();
      let want_keys: Vec<&String> = want_step.keys().collect();
      assert_eq!(got_keys, want_keys, "field set in {name} step {at}");

      for (key, expected) in want_step {
        assert_eq!(got_step.get(key), Some(expected), "{name} step {at} field {key}");
      }
    }
  }
}

fn run_corpus() -> Value {
  let corpus: Value =
    serde_json::from_str(&fs::read_to_string(fixtures_dir().join("ota-trace.json")).expect("corpus readable"))
      .expect("corpus parses");

  let cases: Vec<Value> = corpus["cases"]
    .as_array()
    .expect("corpus cases")
    .iter()
    .map(|case| {
      let component = case["component"].as_str().expect("case component");
      let steps = case["steps"].as_array().expect("case steps");
      let emitted = match component {
        "run_store" => emit_run_store(steps),
        "auto_push" => emit_auto_push(steps),
        other => panic!("unknown case component {other}"),
      };
      json!({ "component": component, "name": case["name"], "steps": emitted })
    })
    .collect();

  json!({
    "impl": "rust",
    "constants": {
      "auto_push_backoff_base_ms": BACKOFF_BASE_MS,
      "auto_push_backoff_max_ms": BACKOFF_MAX_MS,
      "auto_push_backoff_shift_cap": BACKOFF_SHIFT_CAP,
      "auto_push_backoff_jitter": BACKOFF_JITTER_MS,
      "link_stability_ms": LINK_STABILITY_MS,
      "min_resume_delay_ms": MIN_RESUME_DELAY_MS,
      "min_poll_interval_seconds": MIN_POLL_INTERVAL_SECONDS,
      "cancelled_reason": CANCELLED_REASON,
    },
    "cases": cases,
  })
}

// -- run store --

fn emit_run_store(steps: &[Value]) -> Vec<Value> {
  let clock = TestClock::at(0);
  let mut store = OtaRunStore::new(clock.clone(), None);
  let mut run_ids: Vec<String> = Vec::new();
  let mut out = Vec::new();

  for step in steps {
    let t_ms = step["t_ms"].as_u64().expect("t_ms");
    clock.set(t_ms);

    let mut row = Map::new();
    row.insert("t_ms".into(), json!(t_ms));

    if let Some(ingest) = step.get("ingest") {
      let changes = store.ingest(parse_event(ingest), None);

      let kinds: Vec<&str> = changes.iter().map(change_kind).collect();
      let run = changes.iter().find_map(|change| match change {
        OtaStoreChange::Run(run) => Some(run.as_ref()),
        _ => None,
      });

      row.insert("ret_changes".into(), json!(kinds));
      row.insert("ret_present".into(), json!(run.is_some()));
      put_run_fields(&mut row, run, &mut run_ids);

      match changes.iter().find_map(|change| match change {
        OtaStoreChange::Available(available) => Some(available),
        _ => None,
      }) {
        Some(available) => {
          row.insert("ret_available_device_id".into(), json!(available.device_id));
          row.insert("ret_available_release_version".into(), json!(available.release_version));
          row.insert("ret_available_daemon_version".into(), json!(available.daemon_version));
          row.insert("ret_available_image_version".into(), json!(available.image_version));
        }
        None => put_nulls(&mut row, &AVAILABLE_KEYS),
      }

      match changes.iter().find_map(|change| match change {
        OtaStoreChange::Poll(poll) => Some(poll),
        _ => None,
      }) {
        Some(poll) => {
          row.insert("ret_poll_last_polled_at".into(), json!(poll.last_polled_at));
          row.insert("ret_poll_error".into(), json!(poll.error));
        }
        None => put_nulls(&mut row, &POLL_KEYS),
      }

      row.insert("ret_open_kind".into(), Value::Null);
    } else {
      let call = step["call"].as_str().expect("call");
      let device_id = step["device_id"].as_str().expect("device_id");
      let mut run: Option<OtaRun> = None;
      let mut open_kind: Option<OtaKind> = None;

      match call {
        "dismiss" => run = store.dismiss(device_id),
        "interrupt" => run = store.interrupt(device_id),
        "note_meta" => {
          run = store.note_meta(
            device_id,
            step["daemon_version"].as_str().expect("daemon_version"),
            step["image_version"].as_str().expect("image_version"),
          )
        }
        "annotate_webapp" => {
          run = store.annotate_webapp(device_id, step["webapp_id"].as_str(), step["webapp_name"].as_str())
        }
        "open_run_kind" => open_kind = store.open_run_kind(device_id),
        other => panic!("unknown call {other}"),
      }

      row.insert("ret_changes".into(), Value::Null);
      row.insert("ret_present".into(), json!(run.is_some()));
      put_run_fields(&mut row, run.as_ref(), &mut run_ids);
      put_nulls(&mut row, &AVAILABLE_KEYS);
      put_nulls(&mut row, &POLL_KEYS);
      row.insert("ret_open_kind".into(), open_kind.map_or(Value::Null, wire_kind));
    }

    let mut runs = store.runs();
    runs.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    let mut offers = store.available();
    offers.sort_by(|a, b| a.device_id.cmp(&b.device_id));
    let poll = store.poll_status();

    row.insert(
      "store_run_device_ids".into(),
      json!(runs.iter().map(|run| &run.device_id).collect::<Vec<_>>()),
    );
    row.insert(
      "store_run_phases".into(),
      json!(runs.iter().map(|run| wire_run_phase(run.phase)).collect::<Vec<_>>()),
    );
    row.insert(
      "store_run_outcomes".into(),
      json!(runs.iter().map(|run| run.outcome.map(wire_outcome)).collect::<Vec<_>>()),
    );
    row.insert(
      "store_available_device_ids".into(),
      json!(offers.iter().map(|offer| &offer.device_id).collect::<Vec<_>>()),
    );
    row.insert("store_poll_last_polled_at".into(), json!(poll.last_polled_at));
    row.insert("store_poll_error".into(), json!(poll.error));

    out.push(Value::Object(row));
  }

  out
}

const RUN_KEYS: [&str; 19] = [
  "ret_device_id",
  "ret_run_id_seq",
  "ret_kind",
  "ret_phase",
  "ret_step_id",
  "ret_step_ids",
  "ret_started_at_ms",
  "ret_phase_started_at_ms",
  "ret_stage_received",
  "ret_stage_total",
  "ret_rate_micros",
  "ret_dwl_percent",
  "ret_outcome",
  "ret_error",
  "ret_release_version",
  "ret_daemon_version",
  "ret_image_version",
  "ret_webapp_id",
  "ret_webapp_name",
];

const AVAILABLE_KEYS: [&str; 4] = [
  "ret_available_device_id",
  "ret_available_release_version",
  "ret_available_daemon_version",
  "ret_available_image_version",
];

const POLL_KEYS: [&str; 2] = ["ret_poll_last_polled_at", "ret_poll_error"];

fn put_nulls(row: &mut Map<String, Value>, keys: &[&str]) {
  for key in keys {
    row.insert((*key).into(), Value::Null);
  }
}

fn put_run_fields(row: &mut Map<String, Value>, run: Option<&OtaRun>, run_ids: &mut Vec<String>) {
  let Some(run) = run else {
    put_nulls(row, &RUN_KEYS);
    return;
  };

  row.insert("ret_device_id".into(), json!(run.device_id));
  row.insert("ret_run_id_seq".into(), json!(run_id_seq(run_ids, &run.run_id)));
  row.insert("ret_kind".into(), wire_kind(run.kind));
  row.insert("ret_phase".into(), json!(wire_run_phase(run.phase)));
  row.insert("ret_step_id".into(), json!(run.step_id));
  row.insert(
    "ret_step_ids".into(),
    json!(run.steps.iter().map(|step| step.id).collect::<Vec<_>>()),
  );
  row.insert("ret_started_at_ms".into(), json!(run.started_at_ms));
  row.insert("ret_phase_started_at_ms".into(), json!(run.phase_started_at_ms));
  row.insert("ret_stage_received".into(), json!(run.stage_received));
  row.insert("ret_stage_total".into(), json!(run.stage_total));
  row.insert(
    "ret_rate_micros".into(),
    json!(run.rate_per_sec.map(|rate| (rate * 1e6).round() as i64)),
  );
  row.insert("ret_dwl_percent".into(), json!(run.dwl_percent));
  row.insert("ret_outcome".into(), json!(run.outcome.map(wire_outcome)));
  row.insert("ret_error".into(), json!(run.error));
  row.insert("ret_release_version".into(), json!(run.release_version));
  row.insert("ret_daemon_version".into(), json!(run.daemon_version));
  row.insert("ret_image_version".into(), json!(run.image_version));
  row.insert("ret_webapp_id".into(), json!(run.webapp_id));
  row.insert("ret_webapp_name".into(), json!(run.webapp_name));
}

fn run_id_seq(run_ids: &mut Vec<String>, run_id: &str) -> usize {
  match run_ids.iter().position(|seen| seen == run_id) {
    Some(at) => at,
    None => {
      run_ids.push(run_id.to_string());
      run_ids.len() - 1
    }
  }
}

fn change_kind(change: &OtaStoreChange) -> &'static str {
  match change {
    OtaStoreChange::Run(_) => "run",
    OtaStoreChange::Available(_) => "available",
    OtaStoreChange::Poll(_) => "poll",
  }
}

fn wire_kind(kind: OtaKind) -> Value {
  serde_json::to_value(kind).expect("an ota kind serializes to its wire string")
}

fn wire_run_phase(phase: OtaRunPhase) -> &'static str {
  match phase {
    OtaRunPhase::Idle => "idle",
    OtaRunPhase::Downloading => "downloading",
    OtaRunPhase::Streaming => "streaming",
    OtaRunPhase::Verifying => "verifying",
    OtaRunPhase::Writing => "writing",
    OtaRunPhase::Confirming => "confirming",
    OtaRunPhase::Reboot => "reboot",
    OtaRunPhase::Completed => "completed",
    OtaRunPhase::Failed => "failed",
  }
}

fn wire_outcome(outcome: OtaRunOutcome) -> &'static str {
  match outcome {
    OtaRunOutcome::Succeeded => "succeeded",
    OtaRunOutcome::Failed => "failed",
    OtaRunOutcome::Cancelled => "cancelled",
  }
}

// -- corpus event construction --

fn parse_event(ingest: &Value) -> OtaPollEvent {
  let device_id = || ingest["device_id"].as_str().expect("device_id").to_string();
  let kind = || parse_kind(&ingest["kind"]);

  match ingest["event"].as_str().expect("event") {
    "manifest_polled" => OtaPollEvent::ManifestPolled {
      updated_at: ingest["updated_at"].as_str().expect("updated_at").into(),
    },
    "manifest_poll_failed" => OtaPollEvent::ManifestPollFailed {
      reason: ingest["reason"].as_str().expect("reason").into(),
    },
    "update_available" => OtaPollEvent::UpdateAvailable {
      device_id: device_id(),
      release: ingest["release"].as_str().expect("release").into(),
      daemon_version: ingest["daemon_version"].as_str().expect("daemon_version").into(),
      image_version: ingest["image_version"].as_str().expect("image_version").into(),
    },
    "planned" => OtaPollEvent::Planned {
      device_id: device_id(),
      kind: kind(),
      release: ingest["release"].as_str().expect("release").into(),
      daemon_version: ingest["daemon_version"].as_str().expect("daemon_version").into(),
      image_version: ingest["image_version"].as_str().expect("image_version").into(),
      channel: ingest["channel"].as_str().unwrap_or("stable").into(),
      root_url: ingest["root_url"]
        .as_str()
        .unwrap_or("https://ota.bridgething.com")
        .into(),
      steps: ingest["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(parse_plan_step)
        .collect(),
    },
    "progress" => OtaPollEvent::Progress {
      device_id: device_id(),
      kind: kind(),
      step_id: ingest["step_id"].as_u64().expect("step_id") as u32,
      snapshot: parse_snapshot(&ingest["snapshot"]),
    },
    "updated" => OtaPollEvent::Updated {
      device_id: device_id(),
      kind: kind(),
      version: ingest["version"].as_str().expect("version").into(),
    },
    "failed" => OtaPollEvent::Failed {
      device_id: device_id(),
      kind: kind(),
      reason: ingest["reason"].as_str().expect("reason").into(),
    },
    other => panic!("unknown ingest event {other}"),
  }
}

fn parse_plan_step(step: &Value) -> OtaPlanStep {
  OtaPlanStep {
    id: step["id"].as_u64().expect("step id") as u32,
    kind: match step["kind"].as_str().expect("step kind") {
      "download" => OtaStepKind::Download,
      "stream" => OtaStepKind::Stream,
      "apply" => OtaStepKind::Apply,
      "reboot" => OtaStepKind::Reboot,
      other => panic!("unknown plan step kind {other}"),
    },
    label: step["label"].as_str().expect("step label").into(),
    bytes: step["bytes"].as_u64().expect("step bytes"),
  }
}

fn parse_snapshot(snapshot: &Value) -> OtaPhaseSnapshot {
  let micros = |key: &str| snapshot[key].as_i64().map(|value| value as f64 / 1e6);

  match snapshot["phase"].as_str().expect("snapshot phase") {
    "idle" => OtaPhaseSnapshot::Idle,
    "staged" => OtaPhaseSnapshot::Staged,
    "completed" => OtaPhaseSnapshot::Completed,
    "failed" => OtaPhaseSnapshot::Failed {
      reason: snapshot["reason"].as_str().expect("reason").into(),
    },
    "downloading" => OtaPhaseSnapshot::Downloading {
      asset: snapshot["asset"].as_str().expect("asset").into(),
      received: snapshot["received"].as_u64().expect("received"),
      total: snapshot["total"].as_u64().expect("total"),
      rate_per_sec: micros("rate_micros"),
    },
    "streaming" => OtaPhaseSnapshot::Streaming {
      asset: snapshot["asset"].as_str().expect("asset").into(),
      sent: snapshot["sent"].as_u64().expect("sent"),
      total: snapshot["total"].as_u64().expect("total"),
      rate_per_sec: micros("rate_micros"),
      eta_seconds: micros("eta_micros"),
    },
    "applying" => OtaPhaseSnapshot::Applying {
      phase: parse_phase(&snapshot["ota_phase"]),
      write_percent: snapshot["write_percent"].as_u64().expect("write_percent") as u32,
      dwl_percent: snapshot["dwl_percent"].as_u64().expect("dwl_percent") as u32,
      dwl_bytes: snapshot["dwl_bytes"].as_u64().expect("dwl_bytes"),
    },
    other => panic!("unknown snapshot phase {other}"),
  }
}

fn parse_kind(value: &Value) -> OtaKind {
  serde_json::from_value(value.clone()).expect("an ota kind")
}

fn parse_phase(value: &Value) -> OtaPhase {
  serde_json::from_value(value.clone()).expect("an ota phase")
}

// -- auto push --

fn emit_auto_push(steps: &[Value]) -> Vec<Value> {
  let mut schedule = AutoPushSchedule::new();
  let mut out = Vec::new();

  for step in steps {
    let t_ms = step["t_ms"].as_u64().expect("t_ms");
    let mut raw_delay = Value::Null;
    let mut delay = Value::Null;

    match step.get("note").and_then(Value::as_str) {
      Some("failure") => {
        let backoff = schedule.record_failure(t_ms);
        raw_delay = json!(backoff.raw_ms);
        delay = json!(backoff.delay_ms);
      }
      Some("success") => schedule.record_success(),
      Some(other) => panic!("unknown note {other}"),
      None => {}
    }

    match step.get("link").and_then(Value::as_str) {
      Some("open") => schedule.link_opened(t_ms),
      Some("close") => schedule.link_closed(),
      Some(other) => panic!("unknown link {other}"),
      None => {}
    }

    let (wake_deadline, wake_sleep) = match step.get("interval_seconds").and_then(Value::as_f64) {
      Some(interval) => {
        let deadline = schedule.wake_deadline_ms(t_ms, interval as u64);
        (json!(deadline), json!(deadline.saturating_sub(t_ms)))
      }
      None => (Value::Null, Value::Null),
    };

    out.push(json!({
      "t_ms": t_ms,
      "backoff_failures": schedule.failures(),
      "backoff_raw_delay_ms": raw_delay,
      "backoff_delay_ms": delay,
      "backoff_next_at_ms": schedule.next_at_ms(),
      "link_stable": schedule.link_stable(t_ms),
      "auto_push_ready": schedule.ready(t_ms),
      "wake_deadline_ms": wake_deadline,
      "wake_sleep_ms": wake_sleep,
    }));
  }

  out
}

#[test]
fn the_backoff_ladder_clamps_its_exponent_before_its_ceiling() {
  let mut schedule = AutoPushSchedule::new();
  let ladder: Vec<(u64, u64)> = (0..8)
    .map(|_| {
      let backoff = schedule.record_failure(0);
      (backoff.raw_ms, backoff.delay_ms)
    })
    .collect();

  assert_eq!(
    ladder,
    vec![
      (120_000, 120_000),
      (240_000, 240_000),
      (480_000, 480_000),
      (960_000, 900_000),
      (1_920_000, 900_000),
      (3_840_000, 900_000),
      (3_840_000, 900_000),
      (3_840_000, 900_000),
    ],
    "the shift stops at {BACKOFF_SHIFT_CAP} and the ceiling holds at {BACKOFF_MAX_MS}ms"
  );
  assert_eq!(BACKOFF_JITTER_MS, 0, "the schedule is deterministic");
}

#[test]
fn a_success_disarms_the_schedule() {
  let mut schedule = AutoPushSchedule::new();
  schedule.record_failure(1_000);
  schedule.record_failure(2_000);
  assert_eq!(schedule.failures(), 2);
  assert_eq!(schedule.next_at_ms(), Some(242_000));

  schedule.record_success();

  assert_eq!(schedule.failures(), 0);
  assert!(schedule.next_at_ms().is_none());
}

#[test]
fn readiness_needs_a_stable_link_and_an_elapsed_deadline_together() {
  let mut schedule = AutoPushSchedule::new();
  assert!(!schedule.ready(0), "a link that was never open is not stable");

  schedule.link_opened(1_000);
  assert!(!schedule.link_stable(1_000 + LINK_STABILITY_MS - 1));
  assert!(schedule.link_stable(1_000 + LINK_STABILITY_MS));
  assert!(schedule.ready(1_000 + LINK_STABILITY_MS));

  schedule.record_failure(122_000);
  assert!(!schedule.ready(241_999), "the backoff still holds it");
  assert!(schedule.ready(242_000));

  schedule.link_closed();
  assert!(!schedule.ready(242_000), "a closed link is never ready");
}

#[test]
fn the_wake_deadline_floors_the_interval_and_is_pulled_in_by_whatever_comes_sooner() {
  let mut schedule = AutoPushSchedule::new();

  assert_eq!(
    schedule.wake_deadline_ms(0, 0),
    MIN_POLL_INTERVAL_SECONDS * 1_000,
    "a poll cadence below the floor is raised to it"
  );
  assert_eq!(schedule.wake_deadline_ms(0, 3_600), 3_600_000);

  schedule.record_failure(2_000);
  assert_eq!(
    schedule.wake_deadline_ms(121_000, 3_600),
    121_000 + MIN_RESUME_DELAY_MS,
    "an already-due backoff still waits out the resume floor"
  );

  schedule.link_opened(1_000);
  assert_eq!(
    schedule.wake_deadline_ms(1_000, 3_600),
    1_000 + LINK_STABILITY_MS,
    "a link that is about to become stable is the soonest thing worth waking for"
  );
}
