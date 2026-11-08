use std::{collections::HashMap, hash::Hash, time::Duration};

use libbridgething::{DimSettings, HardwareError, gateway::HostState};
use tokio::time::Instant;

pub const LOCAL_WAKE: Duration = Duration::from_secs(30);
const AFTER_S: std::ops::RangeInclusive<u32> = 15..=86_400;

pub fn validate(dim: &DimSettings) -> Result<(), HardwareError> {
  if dim.after_s.is_some_and(|s| !AFTER_S.contains(&s)) {
    return Err(HardwareError::DimTimerOutOfRange);
  }
  if !(0.0..=1.0).contains(&dim.level) {
    return Err(HardwareError::LevelOutOfRange);
  }
  Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
  pub dimmed: bool,
  pub recheck_at: Option<Instant>,
}

#[derive(Debug)]
pub struct DimState<K> {
  settings: DimSettings,
  hosts: HashMap<K, Option<HostState>>,
  held: bool,
  idle_since: Instant,
  last_input: Option<Instant>,
}

impl<K: Eq + Hash + Copy> DimState<K> {
  pub fn new(settings: DimSettings, now: Instant) -> Self {
    Self {
      settings,
      hosts: HashMap::new(),
      held: false,
      idle_since: now,
      last_input: None,
    }
  }

  pub fn settings(&self) -> DimSettings {
    self.settings
  }

  pub fn set_settings(&mut self, settings: DimSettings) {
    self.settings = settings;
  }

  pub fn linked(&self) -> impl Iterator<Item = K> + '_ {
    self.hosts.keys().copied()
  }

  pub fn link_up(&mut self, host: K) -> bool {
    if self.hosts.contains_key(&host) {
      return false;
    }
    self.hosts.insert(host, None);
    true
  }

  pub fn link_lost(&mut self, host: K) {
    let Some(last) = self.hosts.remove(&host) else {
      return;
    };
    if last.and_then(|state| state.away) == Some(true) && self.votes().all(|away| away) {
      self.held = true;
    }
  }

  pub fn report(&mut self, host: K, state: HostState) -> bool {
    let Some(slot) = self.hosts.get_mut(&host) else {
      return false;
    };
    *slot = Some(state);
    if state.away != Some(true) {
      self.held = false;
    }
    true
  }

  pub fn input(&mut self, now: Instant) {
    self.idle_since = now;
    self.last_input = Some(now);
  }

  fn reporting(&self) -> impl Iterator<Item = &HostState> {
    self.hosts.values().flatten()
  }

  fn votes(&self) -> impl Iterator<Item = bool> + '_ {
    self.reporting().filter_map(|state| state.away)
  }

  fn following(&self) -> bool {
    self.held || self.votes().next().is_some()
  }

  pub fn decide(&self, now: Instant) -> Verdict {
    let wake_ends = self.last_input.map(|at| at + LOCAL_WAKE);
    let away = wake_ends.is_none_or(|ends| now >= ends)
      && (self.held || (self.votes().next().is_some() && self.votes().all(|away| away)));
    let timer_due = self
      .settings
      .after_s
      .map(|s| self.idle_since + Duration::from_secs(u64::from(s)));
    let timed = timer_due.is_some_and(|at| now >= at) && self.reporting().all(|h| h.idle);
    let recheck_at = [wake_ends.filter(|_| self.following()), timer_due]
      .into_iter()
      .flatten()
      .filter(|at| *at > now)
      .min();
    Verdict {
      dimmed: away || timed,
      recheck_at,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
  }

  fn settings(after_s: Option<u32>) -> DimSettings {
    DimSettings { after_s, level: 0.2 }
  }

  fn host(away: Option<bool>, idle: bool) -> HostState {
    HostState { away, idle }
  }

  const AWAY: Option<bool> = Some(true);
  const BACK: Option<bool> = Some(false);
  const SILENT: Option<bool> = None;

  fn dimmed(state: &DimState<u8>, at: Instant) -> bool {
    state.decide(at).dimmed
  }

  fn quiet(after_s: Option<u32>) -> (DimState<u8>, Instant) {
    let t0 = Instant::now();
    (DimState::new(settings(after_s), t0), t0 + secs(1))
  }

  fn held() -> (DimState<u8>, Instant) {
    let (mut state, t) = quiet(None);
    state.link_up(1);
    state.report(1, host(AWAY, false));
    state.link_lost(1);
    (state, t)
  }

  #[test]
  fn a_device_that_just_started_follows_an_away_host_at_once() {
    let (mut state, t) = quiet(None);
    state.link_up(1);
    state.report(1, host(AWAY, false));
    assert!(dimmed(&state, t), "starting up is not device input");
  }

  #[test]
  fn the_timer_counts_from_startup_before_any_input() {
    let t0 = Instant::now();
    let state: DimState<u8> = DimState::new(settings(Some(15)), t0);
    assert!(!dimmed(&state, t0 + secs(14)));
    assert!(dimmed(&state, t0 + secs(15)));
  }

  #[test]
  fn the_defaults_never_dim() {
    let (mut state, t) = quiet(None);
    state.link_up(1);
    state.report(1, host(SILENT, true));
    let verdict = state.decide(t + secs(86_400));
    assert!(!verdict.dimmed);
    assert_eq!(verdict.recheck_at, None, "nothing to wait for");
  }

  #[test]
  fn the_screen_dims_when_every_host_that_votes_is_away() {
    let (mut state, t) = quiet(None);
    state.link_up(1);
    state.link_up(2);
    assert!(!dimmed(&state, t), "no host has reported");
    state.report(1, host(AWAY, false));
    assert!(dimmed(&state, t), "a silent gateway does not count");
    state.report(2, host(BACK, false));
    assert!(!dimmed(&state, t), "one following host in use keeps the screen lit");
    state.report(2, host(SILENT, false));
    assert!(dimmed(&state, t), "a host that does not follow has no vote");
    state.report(2, host(AWAY, false));
    assert!(dimmed(&state, t));
  }

  #[test]
  fn a_host_that_does_not_follow_never_dims_the_screen() {
    let (mut state, t) = quiet(None);
    state.link_up(1);
    state.report(1, host(SILENT, false));
    assert!(!dimmed(&state, t));
    state.report(1, host(AWAY, false));
    assert!(dimmed(&state, t), "following starts with the report that carries away");
  }

  #[test]
  fn a_report_from_a_gateway_with_no_link_is_dropped() {
    let (mut state, t) = quiet(None);
    assert!(!state.report(9, host(AWAY, true)));
    assert!(!dimmed(&state, t));
  }

  #[test]
  fn device_input_while_away_undims_for_the_wake_window_then_redims() {
    let (mut state, t) = quiet(None);
    state.link_up(1);
    state.report(1, host(AWAY, false));
    assert!(dimmed(&state, t));
    state.input(t + secs(5));
    let verdict = state.decide(t + secs(6));
    assert!(!verdict.dimmed);
    assert_eq!(verdict.recheck_at, Some(t + secs(5) + LOCAL_WAKE));
    state.input(t + secs(20));
    assert!(
      !dimmed(&state, t + secs(5) + LOCAL_WAKE),
      "the window runs from the last input"
    );
    assert!(dimmed(&state, t + secs(20) + LOCAL_WAKE));
  }

  #[test]
  fn a_host_that_vanishes_while_away_holds_the_dim() {
    let (state, t) = held();
    assert!(
      dimmed(&state, t),
      "a sleeping computer that drops its link does not light the screen"
    );
    assert!(dimmed(&state, t + secs(86_400)));
  }

  #[test]
  fn device_input_lights_a_held_screen_only_for_the_wake_window() {
    let (mut state, t) = held();
    state.input(t + secs(1));
    let verdict = state.decide(t + secs(1));
    assert!(!verdict.dimmed, "a tap lights the screen");
    assert_eq!(verdict.recheck_at, Some(t + secs(1) + LOCAL_WAKE));
    assert!(
      dimmed(&state, t + secs(1) + LOCAL_WAKE),
      "the computer is still asleep, so the screen dims again"
    );
    state.input(t + secs(100));
    assert!(!dimmed(&state, t + secs(101)));
    assert!(
      dimmed(&state, t + secs(100) + LOCAL_WAKE),
      "every tap gets its own window"
    );
    assert!(
      dimmed(&state, t + secs(86_400)),
      "and the hold lasts until a host says otherwise"
    );
  }

  #[test]
  fn a_host_returning_releases_a_hold_that_device_input_only_suspended() {
    let (mut state, t) = held();
    state.input(t + secs(1));
    state.link_up(1);
    state.report(1, host(BACK, false));
    assert!(!dimmed(&state, t + secs(1) + LOCAL_WAKE));
    assert!(!dimmed(&state, t + secs(86_400)));
  }

  #[test]
  fn a_host_that_comes_back_not_following_releases_a_hold() {
    let (mut state, t) = held();
    state.link_up(1);
    assert!(dimmed(&state, t), "a fresh link that has not reported keeps the hold");
    state.report(1, host(SILENT, false));
    assert!(
      !dimmed(&state, t + secs(86_400)),
      "a computer that is back and does not want dimming lights the screen"
    );
  }

  #[test]
  fn any_host_coming_back_active_releases_a_held_dim() {
    let (mut state, t) = held();
    state.link_up(2);
    assert!(dimmed(&state, t), "a fresh link that has not reported keeps the hold");
    state.report(2, host(AWAY, false));
    assert!(dimmed(&state, t));
    state.report(2, host(BACK, false));
    assert!(!dimmed(&state, t));
    state.report(2, host(AWAY, false));
    state.link_lost(2);
    state.link_up(1);
    state.report(1, host(BACK, false));
    assert!(!dimmed(&state, t));
  }

  #[test]
  fn a_vanished_away_host_does_not_hold_while_another_host_is_in_use() {
    let (mut state, t) = quiet(None);
    state.link_up(1);
    state.link_up(2);
    state.report(1, host(AWAY, false));
    state.report(2, host(BACK, false));
    state.link_lost(1);
    assert!(!dimmed(&state, t));
    state.link_lost(2);
    assert!(!dimmed(&state, t), "the host that was in use left lit");
  }

  #[test]
  fn a_host_that_never_followed_leaves_no_hold_behind() {
    let (mut state, t) = quiet(None);
    state.link_up(1);
    state.report(1, host(SILENT, false));
    state.link_lost(1);
    assert!(!dimmed(&state, t));
  }

  #[test]
  fn a_host_that_vanishes_while_active_leaves_the_screen_to_the_timer() {
    let (mut state, t) = quiet(Some(60));
    state.link_up(1);
    state.report(1, host(BACK, false));
    state.link_lost(1);
    assert!(!dimmed(&state, t));
    let start = t - secs(1);
    assert!(!dimmed(&state, start + secs(59)));
    assert!(dimmed(&state, start + secs(60)));
  }

  #[test]
  fn changing_the_settings_leaves_a_hold_alone() {
    let (mut state, t) = held();
    state.set_settings(settings(Some(60)));
    assert!(dimmed(&state, t));
    state.set_settings(settings(None));
    assert!(dimmed(&state, t + secs(86_400)));
  }

  #[test]
  fn the_timer_dims_after_the_last_device_input() {
    let t0 = Instant::now();
    let mut state: DimState<u8> = DimState::new(settings(Some(15)), t0);
    let verdict = state.decide(t0 + secs(14));
    assert!(!verdict.dimmed);
    assert_eq!(verdict.recheck_at, Some(t0 + secs(15)));
    assert!(dimmed(&state, t0 + secs(15)));
    state.input(t0 + secs(20));
    assert!(!dimmed(&state, t0 + secs(21)));
    assert!(dimmed(&state, t0 + secs(35)));
    assert_eq!(state.decide(t0 + secs(35)).recheck_at, None);
  }

  #[test]
  fn a_host_in_use_keeps_the_timer_from_dimming() {
    let t0 = Instant::now();
    let mut state: DimState<u8> = DimState::new(settings(Some(15)), t0);
    state.link_up(1);
    state.link_up(2);
    state.report(1, host(SILENT, false));
    assert!(!dimmed(&state, t0 + secs(3_600)));
    state.report(1, host(SILENT, true));
    assert!(
      dimmed(&state, t0 + secs(3_600)),
      "a silent gateway does not block the timer"
    );
    state.report(2, host(SILENT, false));
    assert!(!dimmed(&state, t0 + secs(3_600)));
    state.link_lost(2);
    assert!(
      dimmed(&state, t0 + secs(3_600)),
      "a dropped host's activity goes with it"
    );
  }

  #[test]
  fn following_and_the_timer_combine() {
    let t0 = Instant::now();
    let mut state: DimState<u8> = DimState::new(settings(Some(15)), t0);
    state.input(t0);
    state.link_up(1);
    state.report(1, host(BACK, true));
    assert!(
      dimmed(&state, t0 + secs(15)),
      "an idle computer still in front of the user times out"
    );
    state.report(1, host(AWAY, false));
    assert!(
      dimmed(&state, t0 + LOCAL_WAKE),
      "away dims even while the host reports input"
    );
    assert!(
      !dimmed(&state, t0 + secs(15)),
      "the timer waits on the active host, following waits on the window"
    );
  }

  #[test]
  fn recheck_points_at_the_next_moment_the_verdict_can_change() {
    let t0 = Instant::now();
    let mut state: DimState<u8> = DimState::new(settings(Some(60)), t0);
    state.input(t0);
    assert_eq!(
      state.decide(t0).recheck_at,
      Some(t0 + secs(60)),
      "no host follows, so the wake window cannot change anything"
    );
    state.link_up(1);
    state.report(1, host(BACK, true));
    assert_eq!(state.decide(t0).recheck_at, Some(t0 + LOCAL_WAKE));
    assert_eq!(state.decide(t0 + LOCAL_WAKE).recheck_at, Some(t0 + secs(60)));
    assert_eq!(state.decide(t0 + secs(60)).recheck_at, None);
  }

  #[test]
  fn validation_bounds_the_timer_and_the_level() {
    assert_eq!(validate(&settings(None)), Ok(()));
    assert_eq!(validate(&settings(Some(15))), Ok(()));
    assert_eq!(validate(&settings(Some(86_400))), Ok(()));
    for after_s in [0, 14, 86_401] {
      assert_eq!(
        validate(&settings(Some(after_s))),
        Err(HardwareError::DimTimerOutOfRange),
        "{after_s}"
      );
    }
    for level in [0.0, 1.0] {
      assert_eq!(
        validate(&DimSettings {
          level,
          ..settings(None)
        }),
        Ok(())
      );
    }
    for level in [-0.01, 1.01, f32::NAN] {
      assert_eq!(
        validate(&DimSettings {
          level,
          ..settings(None)
        }),
        Err(HardwareError::LevelOutOfRange),
        "{level}"
      );
    }
  }
}
