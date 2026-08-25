use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::Arc,
};

use libbridgething::{OtaKind, OtaPhase};
use uuid::Uuid;

use crate::{
  ota::event::{CANCELLED_REASON, OtaPhaseSnapshot, OtaPlanStep, OtaPollEvent},
  seam::Clock,
};

const INTERRUPTED_REASON: &str = "the device disconnected mid-update";
pub const RESUMABLE_REASON: &str = "the device disconnected mid-update; it will pick up where it left off";

pub const RUNS_FILE: &str = "ota/runs.json";
const SCHEMA_VERSION: u32 = 1;
const COUNTER_PERSIST_INTERVAL_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OtaRunOutcome {
  Succeeded,
  Failed,
  Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OtaRunPhase {
  Idle,
  Downloading,
  Streaming,
  Verifying,
  Writing,
  Confirming,
  Reboot,
  Completed,
  Failed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtaRun {
  pub run_id: String,
  pub device_id: String,
  #[serde(default)]
  pub identity: Option<String>,
  pub kind: OtaKind,
  pub phase: OtaRunPhase,
  pub steps: Vec<OtaPlanStep>,
  pub step_id: u32,
  pub started_at_ms: u64,
  pub phase_started_at_ms: u64,
  pub stage_received: Option<u64>,
  pub stage_total: Option<u64>,
  #[serde(skip)]
  pub rate_per_sec: Option<f64>,
  pub dwl_percent: Option<u32>,
  pub outcome: Option<OtaRunOutcome>,
  pub error: Option<String>,
  pub release_version: Option<String>,
  pub daemon_version: Option<String>,
  pub image_version: Option<String>,
  pub channel: Option<String>,
  pub root_url: Option<String>,
  pub resumable: bool,
  pub webapp_id: Option<String>,
  pub webapp_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaResume {
  pub channel: String,
  pub root_url: String,
  pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaAvailable {
  pub device_id: String,
  pub release_version: Option<String>,
  pub daemon_version: Option<String>,
  pub image_version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OtaPollStatus {
  pub last_polled_at: Option<String>,
  pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OtaStoreChange {
  Run(Box<OtaRun>),
  Available(OtaAvailable),
  Poll(OtaPollStatus),
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedRuns {
  version: u32,
  runs: Vec<OtaRun>,
}

pub struct OtaRunStore {
  clock: Arc<dyn Clock>,
  file: Option<PathBuf>,
  wrote_at_ms: u64,
  runs: BTreeMap<String, OtaRun>,
  available: BTreeMap<String, OtaAvailable>,
  poll: OtaPollStatus,
}

impl OtaRunStore {
  pub fn new(clock: Arc<dyn Clock>, data_dir: Option<PathBuf>) -> Self {
    let file = data_dir.map(|dir| dir.join(RUNS_FILE));
    let runs = file.as_deref().map(load).unwrap_or_default();

    Self {
      clock,
      file,
      wrote_at_ms: 0,
      runs,
      available: BTreeMap::new(),
      poll: OtaPollStatus::default(),
    }
  }

  fn persist(&mut self) {
    let Some(file) = self.file.clone() else { return };
    let held = PersistedRuns {
      version: SCHEMA_VERSION,
      runs: self.runs.values().cloned().collect(),
    };
    self.wrote_at_ms = self.clock.unix_millis();
    if let Err(e) = write(&file, &held) {
      tracing::warn!(path = %file.display(), %e, "could not persist the ota runs; a relaunch will not resume");
    }
  }

  fn identities(&self) -> Vec<(String, OtaRunPhase, Option<OtaRunOutcome>, bool)> {
    self
      .runs
      .values()
      .map(|run| (run.run_id.clone(), run.phase, run.outcome, run.resumable))
      .collect()
  }

  pub fn runs(&self) -> Vec<&OtaRun> {
    self.runs.values().collect()
  }

  pub fn run(&self, device_id: &str) -> Option<&OtaRun> {
    self.runs.get(device_id)
  }

  pub fn available(&self) -> Vec<&OtaAvailable> {
    self.available.values().collect()
  }

  pub fn poll_status(&self) -> &OtaPollStatus {
    &self.poll
  }

  pub fn open_run_kind(&self, device_id: &str) -> Option<OtaKind> {
    self
      .runs
      .get(device_id)
      .filter(|run| run.outcome.is_none())
      .map(|run| run.kind)
  }

  pub fn dismiss(&mut self, device_id: &str) -> Option<OtaRun> {
    self.runs.get(device_id)?.outcome?;
    let mut cleared = self.runs.remove(device_id)?;
    cleared.phase = OtaRunPhase::Idle;
    self.persist();
    Some(cleared)
  }

  pub fn clear_available(&mut self, device_id: &str) -> Option<OtaStoreChange> {
    self.available.remove(device_id)?;
    Some(OtaStoreChange::Available(OtaAvailable {
      device_id: device_id.to_owned(),
      release_version: None,
      daemon_version: None,
      image_version: None,
    }))
  }

  pub fn interrupt(&mut self, device_id: &str) -> Option<OtaRun> {
    let run = self.runs.get_mut(device_id)?;
    if run.outcome.is_some() || matches!(run.phase, OtaRunPhase::Reboot | OtaRunPhase::Confirming) {
      return None;
    }
    strand(run);
    let interrupted = run.clone();
    self.persist();
    Some(interrupted)
  }

  pub fn take_resume(&mut self, device_id: &str) -> Option<OtaResume> {
    let run = self.runs.get_mut(device_id)?;
    if !run.resumable {
      return None;
    }
    run.resumable = false;
    let resume = resume_of(run);
    self.persist();
    resume
  }

  pub fn note_meta(&mut self, device_id: &str, daemon_version: &str, image_version: &str) -> Option<OtaRun> {
    let run = self.runs.get(device_id)?;
    let daemon_ok = run.daemon_version.as_deref().is_none_or(|want| want == daemon_version);
    let image_ok = run.image_version.as_deref().is_none_or(|want| want == image_version);
    if !daemon_ok || !image_ok {
      return None;
    }
    let targeted = run.daemon_version.is_some() || run.image_version.is_some();
    if !targeted && run.outcome != Some(OtaRunOutcome::Succeeded) {
      return None;
    }
    let mut cleared = self.runs.remove(device_id)?;
    cleared.phase = OtaRunPhase::Idle;
    cleared.outcome = Some(OtaRunOutcome::Succeeded);
    cleared.error = None;
    cleared.resumable = false;
    self.persist();
    Some(cleared)
  }

  pub fn annotate_webapp(
    &mut self,
    device_id: &str,
    webapp_id: Option<&str>,
    webapp_name: Option<&str>,
  ) -> Option<OtaRun> {
    let run = self.runs.get_mut(device_id)?;
    run.webapp_id = webapp_id.map(str::to_owned);
    run.webapp_name = webapp_name.map(str::to_owned);
    let annotated = run.clone();
    self.persist();
    Some(annotated)
  }

  pub fn ingest(&mut self, event: OtaPollEvent, identity: Option<&str>) -> Vec<OtaStoreChange> {
    let mut changes = self.ingest_inner(event);
    let Some(identity) = identity else { return changes };
    let mut moved = false;
    for change in &mut changes {
      let OtaStoreChange::Run(run) = change else { continue };
      if run.identity.as_deref() == Some(identity) {
        continue;
      }
      run.identity = Some(identity.to_owned());
      if let Some(held) = self.runs.get_mut(&run.device_id) {
        held.identity = Some(identity.to_owned());
        moved = true;
      }
    }
    if moved {
      self.persist();
    }
    changes
  }

  fn ingest_inner(&mut self, event: OtaPollEvent) -> Vec<OtaStoreChange> {
    let before = self.identities();
    let changes = self.reduce(event);
    if !changes.iter().any(|change| matches!(change, OtaStoreChange::Run(_))) {
      return changes;
    }

    let counters_only = before == self.identities();
    if counters_only && self.clock.unix_millis().saturating_sub(self.wrote_at_ms) < COUNTER_PERSIST_INTERVAL_MS {
      return changes;
    }
    self.persist();
    changes
  }

  fn reduce(&mut self, event: OtaPollEvent) -> Vec<OtaStoreChange> {
    let now = self.clock.unix_millis();

    match event {
      OtaPollEvent::ManifestPolled { updated_at } => {
        self.poll = OtaPollStatus {
          last_polled_at: Some(updated_at),
          error: None,
        };
        vec![OtaStoreChange::Poll(self.poll.clone())]
      }

      OtaPollEvent::ManifestPollFailed { reason } => {
        self.poll.error = Some(reason);
        vec![OtaStoreChange::Poll(self.poll.clone())]
      }

      OtaPollEvent::UpdateAvailable {
        device_id,
        release,
        daemon_version,
        image_version,
      } => {
        let entry = OtaAvailable {
          device_id: device_id.clone(),
          release_version: Some(release),
          daemon_version: Some(daemon_version),
          image_version: Some(image_version),
        };
        self.available.insert(device_id, entry.clone());
        vec![OtaStoreChange::Available(entry)]
      }

      OtaPollEvent::Planned {
        device_id,
        kind,
        release,
        daemon_version,
        image_version,
        channel,
        root_url,
        steps,
      } => {
        let run = OtaRun {
          run_id: Uuid::now_v7().to_string(),
          device_id: device_id.clone(),
          identity: None,
          kind,
          phase: OtaRunPhase::Idle,
          step_id: steps.first().map_or(0, |step| step.id),
          steps,
          started_at_ms: now,
          phase_started_at_ms: now,
          stage_received: None,
          stage_total: None,
          rate_per_sec: None,
          dwl_percent: None,
          outcome: None,
          error: None,
          release_version: unset_if_empty(release),
          daemon_version: unset_if_empty(daemon_version),
          image_version: unset_if_empty(image_version),
          channel: unset_if_empty(channel),
          root_url: unset_if_empty(root_url),
          resumable: false,
          webapp_id: None,
          webapp_name: None,
        };
        self.runs.insert(device_id, run.clone());
        vec![OtaStoreChange::Run(Box::new(run))]
      }

      OtaPollEvent::Progress {
        device_id,
        step_id,
        snapshot,
        ..
      } => {
        let Some(run) = self.runs.get_mut(&device_id) else {
          return Vec::new();
        };
        let before = run.phase;
        if run.steps.is_empty() || run.steps.iter().any(|step| step.id == step_id) {
          run.step_id = step_id;
        }
        apply_snapshot(snapshot, run);
        if run.phase != before {
          run.phase_started_at_ms = now;
        }
        vec![OtaStoreChange::Run(Box::new(run.clone()))]
      }

      OtaPollEvent::Updated { device_id, version, .. } => {
        let Some(run) = self.runs.get_mut(&device_id) else {
          return Vec::new();
        };
        run.phase = OtaRunPhase::Completed;
        run.outcome = Some(OtaRunOutcome::Succeeded);
        run.error = None;
        run.resumable = false;
        run.stage_received = None;
        run.stage_total = None;
        run.rate_per_sec = None;
        run.dwl_percent = None;
        if run.release_version.is_none() {
          run.release_version = unset_if_empty(version);
        }
        let run = run.clone();
        self.available.remove(&device_id);

        vec![
          OtaStoreChange::Run(Box::new(run)),
          OtaStoreChange::Available(OtaAvailable {
            device_id,
            release_version: None,
            daemon_version: None,
            image_version: None,
          }),
        ]
      }

      OtaPollEvent::Failed {
        device_id,
        kind,
        reason,
      } => {
        let outcome = if reason == CANCELLED_REASON {
          OtaRunOutcome::Cancelled
        } else {
          OtaRunOutcome::Failed
        };
        let run = self.runs.entry(device_id.clone()).or_insert_with(|| OtaRun {
          run_id: Uuid::now_v7().to_string(),
          device_id,
          identity: None,
          kind,
          phase: OtaRunPhase::Failed,
          steps: Vec::new(),
          step_id: 0,
          started_at_ms: now,
          phase_started_at_ms: now,
          stage_received: None,
          stage_total: None,
          rate_per_sec: None,
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
        });
        if run.resumable {
          return Vec::new();
        }
        run.phase = OtaRunPhase::Failed;
        run.outcome = Some(outcome);
        run.error = Some(reason);
        run.stage_received = None;
        run.stage_total = None;
        run.rate_per_sec = None;
        vec![OtaStoreChange::Run(Box::new(run.clone()))]
      }
    }
  }
}

fn load(file: &Path) -> BTreeMap<String, OtaRun> {
  let body = match std::fs::read_to_string(file) {
    Ok(body) => body,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BTreeMap::new(),
    Err(e) => {
      tracing::warn!(path = %file.display(), %e, "could not read the persisted ota runs");
      return BTreeMap::new();
    }
  };
  let held: PersistedRuns = match serde_json::from_str(&body) {
    Ok(held) => held,
    Err(e) => {
      tracing::warn!(path = %file.display(), %e, "the persisted ota runs are unreadable; starting empty");
      return BTreeMap::new();
    }
  };
  if held.version != SCHEMA_VERSION {
    return BTreeMap::new();
  }

  held
    .runs
    .into_iter()
    .map(|mut run| {
      strand(&mut run);
      (run.device_id.clone(), run)
    })
    .collect()
}

fn strand(run: &mut OtaRun) {
  if run.outcome.is_some() || matches!(run.phase, OtaRunPhase::Reboot | OtaRunPhase::Confirming) {
    return;
  }
  run.resumable = resume_of(run).is_some();
  run.phase = OtaRunPhase::Failed;
  run.outcome = Some(OtaRunOutcome::Failed);
  run.error = Some(
    if run.resumable {
      RESUMABLE_REASON
    } else {
      INTERRUPTED_REASON
    }
    .to_owned(),
  );
}

fn write(file: &Path, held: &PersistedRuns) -> std::io::Result<()> {
  if let Some(dir) = file.parent() {
    std::fs::create_dir_all(dir)?;
  }
  let body = serde_json::to_vec(held).map_err(std::io::Error::other)?;
  let staging = file.with_extension("json.tmp");
  std::fs::write(&staging, &body)?;
  std::fs::rename(&staging, file)
}

fn resume_of(run: &OtaRun) -> Option<OtaResume> {
  Some(OtaResume {
    channel: run.channel.clone()?,
    root_url: run.root_url.clone()?,
    version: run.release_version.clone()?,
  })
}

fn unset_if_empty(value: String) -> Option<String> {
  (!value.is_empty()).then_some(value)
}

fn apply_snapshot(snapshot: OtaPhaseSnapshot, run: &mut OtaRun) {
  match snapshot {
    OtaPhaseSnapshot::Idle => run.phase = OtaRunPhase::Idle,

    OtaPhaseSnapshot::Downloading {
      received,
      total,
      rate_per_sec,
      ..
    } => {
      run.phase = OtaRunPhase::Downloading;
      run.stage_received = Some(received);
      run.stage_total = Some(total);
      run.rate_per_sec = rate_per_sec;
      run.dwl_percent = None;
    }

    OtaPhaseSnapshot::Streaming {
      sent,
      total,
      rate_per_sec,
      ..
    } => {
      run.phase = OtaRunPhase::Streaming;
      run.stage_received = Some(sent);
      run.stage_total = Some(total);
      run.rate_per_sec = rate_per_sec;
      run.dwl_percent = None;
    }

    OtaPhaseSnapshot::Applying {
      phase,
      dwl_percent,
      dwl_bytes,
      ..
    } => {
      run.phase = match phase {
        OtaPhase::Streaming => OtaRunPhase::Streaming,
        OtaPhase::Verifying => OtaRunPhase::Verifying,
        OtaPhase::Writing => OtaRunPhase::Writing,
        OtaPhase::Confirming => OtaRunPhase::Confirming,
        OtaPhase::Reboot => OtaRunPhase::Reboot,
      };
      run.dwl_percent = Some(dwl_percent);
      run.stage_received = (dwl_percent < 100 && dwl_bytes > 0).then_some(dwl_bytes);
      run.stage_total = None;
    }

    OtaPhaseSnapshot::Staged => {
      run.phase = OtaRunPhase::Writing;
      run.stage_received = None;
      run.stage_total = None;
    }

    OtaPhaseSnapshot::Completed => run.phase = OtaRunPhase::Completed,

    OtaPhaseSnapshot::Failed { reason } => {
      run.phase = OtaRunPhase::Failed;
      run.error = Some(reason);
    }
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use libbridgething::OtaKind;
  use tempfile::TempDir;

  use super::{OtaPollEvent, OtaRunOutcome, OtaRunPhase, OtaRunStore, RESUMABLE_REASON, RUNS_FILE};
  use crate::ota::{
    event::{OtaPhaseSnapshot, OtaPlanStep, OtaStepKind},
    harness::TestClock,
    service::bandaid_plan,
  };

  const DEVICE: &str = "AA:BB:CC:DD:EE:FF";
  const CHANNEL: &str = "stable";
  const ROOT: &str = "https://ota.test";
  const RELEASE: &str = "0.9.1+image.1.0.0";

  struct Home {
    dir: TempDir,
  }

  impl Home {
    fn new() -> Self {
      Self {
        dir: TempDir::new().expect("a scratch directory"),
      }
    }

    fn path(&self) -> PathBuf {
      self.dir.path().to_path_buf()
    }

    fn open(&self) -> OtaRunStore {
      OtaRunStore::new(TestClock::new(), Some(self.path()))
    }

    fn raw(&self) -> String {
      std::fs::read_to_string(self.dir.path().join(RUNS_FILE)).expect("the store wrote its runs")
    }

    fn persisted_phase(&self) -> OtaRunPhase {
      let held: serde_json::Value = serde_json::from_str(&self.raw()).expect("the file is json");
      serde_json::from_value(held["runs"][0]["phase"].clone()).expect("a run carries its phase")
    }

    fn overwrite(&self, body: &str) {
      let file = self.dir.path().join(RUNS_FILE);
      std::fs::create_dir_all(file.parent().expect("the runs file is under a directory"))
        .expect("the scratch directory is writable");
      std::fs::write(file, body).expect("the scratch directory is writable");
    }
  }

  fn planned() -> OtaPollEvent {
    OtaPollEvent::Planned {
      device_id: DEVICE.into(),
      kind: OtaKind::Daemon,
      release: RELEASE.into(),
      daemon_version: "0.9.1".into(),
      image_version: "1.0.0".into(),
      channel: CHANNEL.into(),
      root_url: ROOT.into(),
      steps: bandaid_plan(&[("daemon".into(), 40 * 1024)]),
    }
  }

  fn streaming(sent: u64) -> OtaPollEvent {
    OtaPollEvent::Progress {
      device_id: DEVICE.into(),
      kind: OtaKind::Daemon,
      step_id: 1,
      snapshot: OtaPhaseSnapshot::Streaming {
        asset: "daemon".into(),
        sent,
        total: 40 * 1024,
        rate_per_sec: Some(150_000.0),
        eta_seconds: Some(4.0),
      },
    }
  }

  #[tokio::test]
  async fn a_run_survives_the_process_that_planned_it() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    store.ingest(streaming(8 * 1024), None);
    let before = store.run(DEVICE).cloned().expect("the planned run");

    let reopened = home.open();

    let after = reopened.run(DEVICE).expect("the run outlived its process");
    assert_eq!(after.run_id, before.run_id);
    assert_eq!(after.channel.as_deref(), Some(CHANNEL));
    assert_eq!(after.root_url.as_deref(), Some(ROOT));
    assert_eq!(after.release_version.as_deref(), Some(RELEASE));
    assert_eq!(after.kind, OtaKind::Daemon);
    assert_eq!(after.steps, bandaid_plan(&[("daemon".into(), 40 * 1024)]));
    assert_eq!(
      after.stage_received,
      Some(8 * 1024),
      "how far it got is what the bar has to keep showing"
    );
  }

  #[tokio::test]
  async fn a_run_the_process_died_mid_drive_under_loads_interrupted_and_resumable() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    store.ingest(streaming(8 * 1024), None);
    assert_eq!(store.run(DEVICE).expect("the run").phase, OtaRunPhase::Streaming);

    let reopened = home.open();

    let run = reopened.run(DEVICE).expect("the run");
    assert_eq!(run.phase, OtaRunPhase::Failed);
    assert_eq!(run.outcome, Some(OtaRunOutcome::Failed));
    assert!(run.resumable, "the drive that owned it is gone, so it wants re-driving");
    assert_eq!(run.error.as_deref(), Some(RESUMABLE_REASON));
  }

  #[tokio::test]
  async fn a_run_the_process_died_under_mid_reboot_is_left_for_the_device_to_answer() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    store.ingest(
      OtaPollEvent::Progress {
        device_id: DEVICE.into(),
        kind: OtaKind::Daemon,
        step_id: 3,
        snapshot: OtaPhaseSnapshot::Applying {
          phase: libbridgething::OtaPhase::Reboot,
          write_percent: 100,
          dwl_percent: 100,
          dwl_bytes: 0,
        },
      },
      None,
    );

    let reopened = home.open();

    let run = reopened.run(DEVICE).expect("the run");
    assert_eq!(run.phase, OtaRunPhase::Reboot);
    assert!(
      run.outcome.is_none() && !run.resumable,
      "that run asked the device to go away; re-driving it would push an update that already landed"
    );
  }

  #[tokio::test]
  async fn a_resume_that_was_handed_over_is_not_handed_over_twice_across_a_restart() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    store.interrupt(DEVICE);
    store
      .take_resume(DEVICE)
      .expect("the interrupted run carries its parameters");

    let mut reopened = home.open();

    assert!(
      reopened.take_resume(DEVICE).is_none(),
      "a resume already in flight when the process died must not start a second one"
    );
  }

  #[tokio::test]
  async fn a_run_that_reached_a_successful_terminal_drops_its_interrupt_marker() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    store.interrupt(DEVICE);
    assert!(store.run(DEVICE).expect("the run").resumable);

    store.ingest(
      OtaPollEvent::Updated {
        device_id: DEVICE.into(),
        kind: OtaKind::Daemon,
        version: RELEASE.into(),
      },
      None,
    );

    assert!(
      !store.run(DEVICE).expect("the run").resumable,
      "the update landed, so the older interrupt marker must not survive to re-drive it"
    );
    assert!(store.take_resume(DEVICE).is_none(), "and nothing may still pick it up");
    assert!(
      !home.open().run(DEVICE).expect("the run").resumable,
      "the cleared marker has to reach disk, or the next launch re-drives it"
    );
  }

  #[tokio::test]
  async fn a_moving_byte_counter_does_not_rewrite_the_file() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    store.ingest(streaming(8 * 1024), None);
    let settled = home.raw();

    store.ingest(streaming(16 * 1024), None);

    assert_eq!(
      home.raw(),
      settled,
      "a stream ticks four times a second for hours; the file is not where that belongs"
    );
  }

  #[tokio::test]
  async fn a_phase_the_run_moves_into_reaches_the_file_at_once() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    store.ingest(
      OtaPollEvent::Progress {
        device_id: DEVICE.into(),
        kind: OtaKind::Daemon,
        step_id: 0,
        snapshot: OtaPhaseSnapshot::Downloading {
          asset: "daemon".into(),
          received: 1_024,
          total: 40 * 1024,
          rate_per_sec: None,
        },
      },
      None,
    );

    store.ingest(streaming(0), None);

    assert_eq!(
      home.persisted_phase(),
      OtaRunPhase::Streaming,
      "where a run got to is not a counter, and a relaunch reads it back to decide what to do"
    );
  }

  #[tokio::test]
  async fn a_dismissed_run_stays_dismissed() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    store.interrupt(DEVICE);

    store.dismiss(DEVICE).expect("a settled run can be dismissed");

    assert!(
      home.open().run(DEVICE).is_none(),
      "a run the user waved away must not come back on the next launch"
    );
  }

  #[tokio::test]
  async fn a_measured_rate_does_not_cross_a_restart() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    store.ingest(streaming(8 * 1024), None);
    assert_eq!(store.run(DEVICE).expect("the run").rate_per_sec, Some(150_000.0));

    let reopened = home.open();

    assert_eq!(
      reopened.run(DEVICE).expect("the run").rate_per_sec,
      None,
      "the rate measures a link this process never held, and a stale one prints a fictional eta"
    );
  }

  #[tokio::test]
  async fn an_unreadable_store_starts_empty_rather_than_failing() {
    let home = Home::new();
    home.overwrite("{ this is not json");

    let store = home.open();

    assert!(store.runs().is_empty());
  }

  #[tokio::test]
  async fn a_store_from_another_schema_is_ignored() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    let body = home.raw().replace("\"version\":1", "\"version\":99");
    home.overwrite(&body);

    let reopened = home.open();

    assert!(reopened.runs().is_empty());
  }

  #[tokio::test]
  async fn a_missing_store_starts_empty() {
    let home = Home::new();

    let store = home.open();

    assert!(store.runs().is_empty());
  }

  #[tokio::test]
  async fn a_store_with_nowhere_to_write_keeps_working_in_memory() {
    let home = Home::new();
    let mut store = OtaRunStore::new(TestClock::new(), None);

    store.ingest(planned(), None);

    assert!(store.run(DEVICE).is_some(), "the reducer surface still reduces");
    assert!(!home.path().join(RUNS_FILE).exists(), "and it wrote nothing anywhere");
  }

  #[tokio::test]
  async fn an_annotation_reaches_the_file() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);

    store.annotate_webapp(DEVICE, Some("hub"), Some("Hub"));

    let reopened = home.open();
    assert_eq!(reopened.run(DEVICE).expect("the run").webapp_id.as_deref(), Some("hub"));
  }

  #[test]
  fn a_plan_step_round_trips_through_the_file_format() {
    let step = OtaPlanStep {
      id: 3,
      kind: OtaStepKind::Stream,
      label: "daemon".into(),
      bytes: 512,
    };

    let body = serde_json::to_string(&step).expect("a plan step is serializable");

    assert_eq!(
      serde_json::from_str::<OtaPlanStep>(&body).expect("and readable back"),
      step
    );
  }

  #[tokio::test]
  async fn a_run_carries_the_device_it_was_driving_across_a_launch() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), Some("8558R481Q61R"));

    assert_eq!(
      store.run(DEVICE).expect("the run").identity.as_deref(),
      Some("8558R481Q61R"),
      "the live run knows which device is behind the address it is driving"
    );
    assert_eq!(
      home.open().run(DEVICE).expect("the run").identity.as_deref(),
      Some("8558R481Q61R"),
      "and it survives the relaunch, which is the only time the address can have changed hands"
    );
  }

  #[tokio::test]
  async fn a_run_from_before_identities_existed_reads_as_unknown_rather_than_wrong() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);

    assert!(
      home.open().run(DEVICE).expect("the run").identity.is_none(),
      "an unstamped run claims nothing, so a resume falls back to trusting the address"
    );
  }

  #[tokio::test]
  async fn a_clock_is_only_needed_for_fresh_runs() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(planned(), None);
    let planned_at = store.run(DEVICE).expect("the run").started_at_ms;

    let reopened = OtaRunStore::new(TestClock::new(), Some(home.path()));

    assert_eq!(
      reopened.run(DEVICE).expect("the run").started_at_ms,
      planned_at,
      "the timestamps a run was born with are its own, not the launch's"
    );
  }

  #[tokio::test]
  async fn the_available_set_is_not_carried_across_a_launch() {
    let home = Home::new();
    let mut store = home.open();
    store.ingest(
      OtaPollEvent::UpdateAvailable {
        device_id: DEVICE.into(),
        release: RELEASE.into(),
        daemon_version: "0.9.1".into(),
        image_version: "1.0.0".into(),
      },
      None,
    );
    store.ingest(planned(), None);
    assert_eq!(store.available().len(), 1);

    let reopened = home.open();

    assert!(reopened.run(DEVICE).is_some(), "the run itself did survive");
    assert!(
      reopened.available().is_empty(),
      "a device may have been updated by something else while the app was gone; the next poll re-derives it"
    );
  }
}
