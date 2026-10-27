use libbridgething::OtaKind;

use crate::ota::{
  event::{OtaPlanStep, OtaStepKind},
  run_store::{OtaRun, OtaRunOutcome, OtaRunPhase},
};

const REBOOT_SECONDS: f64 = 45.0;
const REBOOT_SETTLE_FACTOR: f64 = 2.0;
const BATCH_APPLY_SECONDS: f64 = 15.0;
const MIN_STEP_SECONDS: f64 = 1.0;
/// the phone pulling artifacts off the update host over its own wifi or cellular, not the link
const DOWNLOAD_BYTES_PER_SEC: f64 = 3_000_000.0;
/// the bt link, which sustains roughly 150 KB/s once the transfer window has opened up
const STREAM_BYTES_PER_SEC: f64 = 150_000.0;
/// artifact bytes per second while the device writes the whole slot to emmc at roughly 1 MiB/s
const APPLY_BYTES_PER_SEC: f64 = 400_000.0;
/// a bandaid apply reports nothing but its phase, so its step fills in by phase instead of jumping whole
const BATCH_VERIFY_FRACTION: f64 = 0.3;
const BATCH_WRITE_FRACTION: f64 = 0.7;
const BATCH_CONFIRM_FRACTION: f64 = 0.9;
/// a hundred percent means the update landed, so an unfinished run stops short of it
const UNFINISHED_CEILING: f64 = 99.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaRunProgress {
  pub percent: u32,
  pub step_index: usize,
  pub step_count: usize,
  pub step_label: Option<String>,
  pub eta_seconds: Option<u64>,
}

pub fn ota_progress(run: &OtaRun, now_ms: u64) -> OtaRunProgress {
  let weights: Vec<f64> = run.steps.iter().map(step_weight).collect();
  let total_weight: f64 = weights.iter().sum();
  let index = resolve_index(run);

  if run.outcome == Some(OtaRunOutcome::Succeeded) {
    return OtaRunProgress {
      percent: 100,
      step_index: index,
      step_count: run.steps.len(),
      step_label: None,
      eta_seconds: Some(0),
    };
  }

  if total_weight <= 0.0 {
    return OtaRunProgress {
      percent: 0,
      step_index: index,
      step_count: run.steps.len(),
      step_label: None,
      eta_seconds: None,
    };
  }

  let mut done = 0.0;
  let mut remaining = 0.0;
  for (at, step) in run.steps.iter().enumerate() {
    if at < index {
      done += weights[at];
      continue;
    }
    let fraction = if at == index {
      step_fraction(step, run, now_ms)
    } else {
      0.0
    };
    done += weights[at] * fraction;
    remaining += step_seconds(step, run) * (1.0 - fraction);
  }

  let ceiling = if run.outcome.is_none() {
    UNFINISHED_CEILING
  } else {
    100.0
  };

  OtaRunProgress {
    percent: ((done / total_weight) * 100.0).round().min(ceiling) as u32,
    step_index: index,
    step_count: run.steps.len(),
    step_label: run.steps.get(index).map(|step| step.label.clone()),
    eta_seconds: Some(remaining.round() as u64),
  }
}

/// an id the plan does not carry would otherwise collapse to the first step and walk the bar backwards.
fn resolve_index(run: &OtaRun) -> usize {
  if let Some(at) = run.steps.iter().position(|step| step.id == run.step_id) {
    return at;
  }
  if run.phase == OtaRunPhase::Completed {
    return run.steps.len().saturating_sub(1);
  }
  phase_step_kind(run.phase)
    .and_then(|kind| run.steps.iter().position(|step| step.kind == kind))
    .unwrap_or(0)
}

fn phase_step_kind(phase: OtaRunPhase) -> Option<OtaStepKind> {
  match phase {
    OtaRunPhase::Downloading => Some(OtaStepKind::Download),
    OtaRunPhase::Streaming => Some(OtaStepKind::Stream),
    OtaRunPhase::Verifying | OtaRunPhase::Writing | OtaRunPhase::Confirming => Some(OtaStepKind::Apply),
    OtaRunPhase::Reboot => Some(OtaStepKind::Reboot),
    OtaRunPhase::Idle | OtaRunPhase::Completed | OtaRunPhase::Failed => None,
  }
}

fn step_weight(step: &OtaPlanStep) -> f64 {
  match step.kind {
    OtaStepKind::Download => sized_seconds(step.bytes, DOWNLOAD_BYTES_PER_SEC, MIN_STEP_SECONDS),
    OtaStepKind::Stream => sized_seconds(step.bytes, STREAM_BYTES_PER_SEC, MIN_STEP_SECONDS),
    OtaStepKind::Apply => sized_seconds(step.bytes, APPLY_BYTES_PER_SEC, BATCH_APPLY_SECONDS),
    OtaStepKind::Reboot => REBOOT_SECONDS,
  }
}

fn sized_seconds(bytes: u64, per_sec: f64, unsized_seconds: f64) -> f64 {
  if bytes == 0 {
    return unsized_seconds;
  }
  (bytes as f64 / per_sec).max(MIN_STEP_SECONDS)
}

fn step_seconds(step: &OtaPlanStep, run: &OtaRun) -> f64 {
  match step.kind {
    OtaStepKind::Download | OtaStepKind::Stream => match run.rate_per_sec {
      Some(rate) if rate > 0.0 && step.bytes > 0 => (step.bytes as f64 / rate).max(MIN_STEP_SECONDS),
      _ => step_weight(step),
    },
    _ => step_weight(step),
  }
}

fn step_fraction(step: &OtaPlanStep, run: &OtaRun, now_ms: u64) -> f64 {
  match step.kind {
    OtaStepKind::Reboot => reboot_fraction(run, now_ms),
    OtaStepKind::Apply => apply_fraction(run),
    OtaStepKind::Download | OtaStepKind::Stream => transfer_fraction(run),
  }
}

/// the device is gone and cannot report, so the bar coasts on the clock and settles rather than crawling forever.
fn reboot_fraction(run: &OtaRun, now_ms: u64) -> f64 {
  let elapsed = now_ms.saturating_sub(run.phase_started_at_ms) as f64 / 1_000.0;
  let settled = 1.0 - (-REBOOT_SETTLE_FACTOR).exp();
  ((1.0 - (-elapsed / REBOOT_SECONDS).exp()) / settled).min(1.0)
}

fn apply_fraction(run: &OtaRun) -> f64 {
  if run.kind == OtaKind::Image {
    return match run.phase {
      OtaRunPhase::Confirming | OtaRunPhase::Reboot | OtaRunPhase::Completed => 1.0,
      _ => (f64::from(run.write_percent.unwrap_or(0)) / 100.0).min(1.0),
    };
  }
  match run.phase {
    OtaRunPhase::Verifying => BATCH_VERIFY_FRACTION,
    OtaRunPhase::Writing => BATCH_WRITE_FRACTION,
    OtaRunPhase::Confirming => BATCH_CONFIRM_FRACTION,
    OtaRunPhase::Reboot | OtaRunPhase::Completed => 1.0,
    _ => 0.0,
  }
}

/// the staged byte counts track whatever is moving now, so anything the device is reporting on is already behind us.
fn transfer_fraction(run: &OtaRun) -> f64 {
  if run.write_percent.is_some() {
    return 1.0;
  }
  match run.phase {
    OtaRunPhase::Verifying
    | OtaRunPhase::Writing
    | OtaRunPhase::Confirming
    | OtaRunPhase::Reboot
    | OtaRunPhase::Completed => 1.0,
    _ => match run.stage_total {
      Some(total) if total > 0 => (run.stage_received.unwrap_or(0) as f64 / total as f64).min(1.0),
      _ => 0.0,
    },
  }
}
