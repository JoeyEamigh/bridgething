use std::time::{Duration, Instant};

const PROGRESS_INTERVAL: Duration = Duration::from_secs(10);
const PLAYED_FLOOR_MS: u32 = 30_000;
const PLAYED_CEILING_MS: u32 = 240_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportedState {
  Playing,
  Paused,
  Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayReport {
  Started {
    uri: String,
  },
  Progress {
    uri: String,
    state: ReportedState,
    position_ms: u32,
    duration_ms: Option<u32>,
  },
  Played {
    uri: String,
  },
}

pub trait PlayReporter: Send + Sync {
  fn report(&self, report: PlayReport);
}

fn crossed(position_ms: u32, duration_ms: Option<u32>) -> bool {
  let Some(duration) = duration_ms.filter(|duration| *duration >= PLAYED_FLOOR_MS) else {
    return false;
  };
  position_ms >= (duration / 2).min(PLAYED_CEILING_MS)
}

#[derive(Debug, Default)]
pub(super) struct Ledger {
  started: bool,
  played: bool,
  stopped: bool,
  state: Option<ReportedState>,
  progressed_at: Option<Instant>,
}

impl Ledger {
  pub(super) fn observe(
    &mut self,
    uri: &str,
    state: ReportedState,
    position_ms: u32,
    duration_ms: Option<u32>,
    now: Instant,
  ) -> Vec<PlayReport> {
    let mut reports = Vec::new();
    if self.stopped {
      return reports;
    }
    if state == ReportedState::Playing && !self.started {
      self.started = true;
      reports.push(PlayReport::Started { uri: uri.to_owned() });
    }
    if !self.started {
      return reports;
    }
    let due = self.state != Some(state)
      || self
        .progressed_at
        .is_none_or(|at| now.duration_since(at) >= PROGRESS_INTERVAL);
    if due {
      self.state = Some(state);
      self.progressed_at = Some(now);
      reports.push(PlayReport::Progress {
        uri: uri.to_owned(),
        state,
        position_ms,
        duration_ms,
      });
    }
    if !self.played && crossed(position_ms, duration_ms) {
      self.played = true;
      reports.push(PlayReport::Played { uri: uri.to_owned() });
    }
    reports
  }

  pub(super) fn stop(
    &mut self,
    uri: &str,
    position_ms: u32,
    duration_ms: Option<u32>,
    finished: bool,
  ) -> Vec<PlayReport> {
    let mut reports = Vec::new();
    if !self.started || self.stopped {
      return reports;
    }
    self.stopped = true;
    if !self.played && (finished || crossed(position_ms, duration_ms)) {
      self.played = true;
      reports.push(PlayReport::Played { uri: uri.to_owned() });
    }
    reports.push(PlayReport::Progress {
      uri: uri.to_owned(),
      state: ReportedState::Stopped,
      position_ms,
      duration_ms,
    });
    reports
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const URI: &str = "plex:m:track:1";
  const MINUTES_3: Option<u32> = Some(180_000);

  fn kinds(reports: &[PlayReport]) -> Vec<&'static str> {
    reports
      .iter()
      .map(|report| match report {
        PlayReport::Started { .. } => "started",
        PlayReport::Progress {
          state: ReportedState::Playing,
          ..
        } => "playing",
        PlayReport::Progress {
          state: ReportedState::Paused,
          ..
        } => "paused",
        PlayReport::Progress {
          state: ReportedState::Stopped,
          ..
        } => "stopped",
        PlayReport::Played { .. } => "played",
      })
      .collect()
  }

  #[test]
  fn nothing_is_reported_until_the_track_actually_plays() {
    let mut ledger = Ledger::default();
    let now = Instant::now();
    assert!(ledger.observe(URI, ReportedState::Paused, 0, MINUTES_3, now).is_empty());
    assert!(ledger.stop(URI, 0, MINUTES_3, false).is_empty());
  }

  #[test]
  fn progress_repeats_on_state_changes_and_every_interval_only() {
    let mut ledger = Ledger::default();
    let start = Instant::now();
    assert_eq!(
      kinds(&ledger.observe(URI, ReportedState::Playing, 0, MINUTES_3, start)),
      ["started", "playing"]
    );
    assert!(
      ledger
        .observe(
          URI,
          ReportedState::Playing,
          2_000,
          MINUTES_3,
          start + Duration::from_secs(2)
        )
        .is_empty()
    );
    assert_eq!(
      kinds(&ledger.observe(
        URI,
        ReportedState::Paused,
        3_000,
        MINUTES_3,
        start + Duration::from_secs(3)
      )),
      ["paused"]
    );
    assert_eq!(
      kinds(&ledger.observe(
        URI,
        ReportedState::Playing,
        3_000,
        MINUTES_3,
        start + Duration::from_secs(4)
      )),
      ["playing"]
    );
    assert_eq!(
      kinds(&ledger.observe(
        URI,
        ReportedState::Playing,
        13_000,
        MINUTES_3,
        start + Duration::from_secs(14)
      )),
      ["playing"]
    );
  }

  #[test]
  fn a_track_counts_as_played_at_half_or_four_minutes_once() {
    let mut ledger = Ledger::default();
    let start = Instant::now();
    ledger.observe(URI, ReportedState::Playing, 0, MINUTES_3, start);
    assert_eq!(
      kinds(&ledger.observe(
        URI,
        ReportedState::Playing,
        90_000,
        MINUTES_3,
        start + Duration::from_secs(90)
      )),
      ["playing", "played"]
    );
    assert_eq!(kinds(&ledger.stop(URI, 100_000, MINUTES_3, false)), ["stopped"]);

    let mut long = Ledger::default();
    long.observe(URI, ReportedState::Playing, 0, Some(1_200_000), start);
    assert_eq!(
      kinds(&long.observe(
        URI,
        ReportedState::Playing,
        240_000,
        Some(1_200_000),
        start + Duration::from_secs(1)
      )),
      ["played"]
    );
  }

  #[test]
  fn a_skip_before_the_threshold_stops_without_a_play() {
    let mut ledger = Ledger::default();
    ledger.observe(URI, ReportedState::Playing, 0, MINUTES_3, Instant::now());
    assert_eq!(kinds(&ledger.stop(URI, 20_000, MINUTES_3, false)), ["stopped"]);
    assert!(ledger.stop(URI, 20_000, MINUTES_3, false).is_empty());
    assert!(
      ledger
        .observe(URI, ReportedState::Playing, 30_000, MINUTES_3, Instant::now())
        .is_empty()
    );
  }

  #[test]
  fn finishing_a_track_plays_it_even_when_short() {
    let mut ledger = Ledger::default();
    ledger.observe(URI, ReportedState::Playing, 0, Some(20_000), Instant::now());
    assert_eq!(
      kinds(&ledger.stop(URI, 20_000, Some(20_000), true)),
      ["played", "stopped"]
    );
  }
}
