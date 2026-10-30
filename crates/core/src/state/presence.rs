use std::{
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
  time::Duration,
};

use libbridgething::{
  DisplayState, HostPresence, PresenceInfo, PresenceSettings, PresenceSettingsError, PresenceSource,
  client::{BridgeToClientPresenceMsg, PresenceSettingsReply, PresenceSnapshot},
  wire::MsgMeta,
};
use tokio::{sync::Notify, task::JoinHandle, time::Instant};

use crate::{
  als::{AlsManager, Screen},
  net::WireEventBus,
};

const HEARTBEAT_MISSES: u32 = 3;
const LOCAL_WAKE: Duration = Duration::from_secs(30);
const MIN_TIMER_S: u32 = 5;
const MAX_TIMER_S: u32 = 86_400;
const MIN_DIM_LEVEL: f32 = 0.05;

#[derive(Debug)]
struct Inner {
  settings: PresenceSettings,
  host: Option<(HostPresence, Instant)>,
  host_active_at: Option<Instant>,
  input_at: Instant,
  wake_until: Option<Instant>,
  published: PresenceInfo,
  applied: Screen,
}

impl Inner {
  fn new(settings: PresenceSettings, now: Instant) -> Self {
    Self {
      settings,
      host: None,
      host_active_at: None,
      input_at: now,
      wake_until: None,
      published: PresenceInfo::default(),
      applied: Screen::Full,
    }
  }

  fn apply(&mut self, settings: PresenceSettings) {
    self.settings = settings;
    self.wake_until = None;
  }

  fn last_activity(&self) -> Instant {
    self
      .host_active_at
      .map_or(self.input_at, |host| host.max(self.input_at))
  }
}

#[derive(Debug, Clone)]
pub struct PresenceManager {
  inner: Arc<Mutex<Inner>>,
  poke: Arc<Notify>,
  als: AlsManager,
  bus: WireEventBus,
  prefs_path: PathBuf,
}

fn heartbeat_timeout(host: &HostPresence) -> Option<Duration> {
  let beat = host.heartbeat_s.filter(|beat| *beat > 0)?;
  Some(Duration::from_secs(u64::from(beat)) * HEARTBEAT_MISSES)
}

/// The host's last report while it is still being heard from, and when it stops counting.
fn live_host(inner: &Inner, now: Instant) -> Option<(&HostPresence, Option<Instant>)> {
  let (host, seen) = inner.host.as_ref()?;
  let expires = heartbeat_timeout(host).map(|timeout| *seen + timeout);
  match expires {
    Some(at) if now >= at => None,
    _ => Some((host, expires)),
  }
}

fn follow_host(
  inner: &Inner,
  host: &HostPresence,
  expires: Option<Instant>,
  now: Instant,
) -> (PresenceInfo, Option<Instant>) {
  let (mut display, mut source, mut deadline) = (host.display, PresenceSource::Host, expires);
  if display != DisplayState::Active
    && let Some(until) = inner.wake_until
    && now < until
  {
    display = DisplayState::Active;
    source = PresenceSource::LocalWake;
    deadline = Some(deadline.map_or(until, |d| d.min(until)));
  }
  (PresenceInfo { display, source }, deadline)
}

fn idle_timers(inner: &Inner, now: Instant) -> (PresenceInfo, Option<Instant>) {
  let last = inner.last_activity();
  let due = |after: Option<u32>| after.map(|secs| last + Duration::from_secs(u64::from(secs)));
  let (dim_at, sleep_at) = (due(inner.settings.dim_after_s), due(inner.settings.sleep_after_s));
  let (display, deadline) = if sleep_at.is_some_and(|at| now >= at) {
    (DisplayState::Off, None)
  } else if dim_at.is_some_and(|at| now >= at) {
    (DisplayState::Dim, sleep_at)
  } else {
    (DisplayState::Active, [dim_at, sleep_at].into_iter().flatten().min())
  };
  (
    PresenceInfo {
      display,
      source: PresenceSource::Timer,
    },
    deadline,
  )
}

fn evaluate(inner: &Inner, now: Instant) -> (PresenceInfo, Option<Instant>) {
  let settings = &inner.settings;
  if settings.follow_host
    && let Some((host, expires)) = live_host(inner, now)
  {
    return follow_host(inner, host, expires, now);
  }
  if settings.dim_after_s.is_some() || settings.sleep_after_s.is_some() {
    idle_timers(inner, now)
  } else {
    (PresenceInfo::default(), None)
  }
}

fn screen_for(display: DisplayState, settings: &PresenceSettings) -> Screen {
  match display {
    DisplayState::Active => Screen::Full,
    DisplayState::Dim => Screen::Dim(settings.dim_level),
    DisplayState::Off => Screen::Off,
  }
}

fn validate(settings: &PresenceSettings) -> Result<(), PresenceSettingsError> {
  let timers = [settings.dim_after_s, settings.sleep_after_s];
  if timers
    .into_iter()
    .flatten()
    .any(|secs| !(MIN_TIMER_S..=MAX_TIMER_S).contains(&secs))
  {
    return Err(PresenceSettingsError::TimerOutOfRange);
  }
  if let (Some(dim), Some(sleep)) = (settings.dim_after_s, settings.sleep_after_s)
    && dim >= sleep
  {
    return Err(PresenceSettingsError::DimNotBeforeSleep);
  }
  if !(MIN_DIM_LEVEL..=1.0).contains(&settings.dim_level) {
    return Err(PresenceSettingsError::DimLevelOutOfRange);
  }
  Ok(())
}

fn load_settings(path: &Path) -> PresenceSettings {
  std::fs::read(path)
    .ok()
    .and_then(|bytes| serde_json::from_slice::<PresenceSettings>(&bytes).ok())
    .filter(|settings| validate(settings).is_ok())
    .unwrap_or_default()
}

async fn save_settings(path: &Path, settings: &PresenceSettings) {
  if let Some(dir) = path.parent()
    && let Err(err) = tokio::fs::create_dir_all(dir).await
  {
    tracing::warn!(path = %path.display(), "presence: cannot create settings dir: {err}");
    return;
  }
  let body = match serde_json::to_vec(settings) {
    Ok(body) => body,
    Err(err) => {
      tracing::warn!("presence: cannot serialize settings: {err}");
      return;
    }
  };
  let tmp = path.with_extension("tmp");
  if let Err(err) = tokio::fs::write(&tmp, body).await {
    tracing::warn!(path = %tmp.display(), "presence: cannot write settings: {err}");
    return;
  }
  if let Err(err) = tokio::fs::rename(&tmp, path).await {
    tracing::warn!(path = %path.display(), "presence: cannot replace settings: {err}");
  }
}

impl PresenceManager {
  pub fn init(bus: WireEventBus, als: AlsManager, prefs_path: PathBuf) -> (Self, JoinHandle<()>) {
    let settings = load_settings(&prefs_path);
    let manager = Self {
      inner: Arc::new(Mutex::new(Inner::new(settings, Instant::now()))),
      poke: Arc::new(Notify::new()),
      als,
      bus,
      prefs_path,
    };
    let handle = tokio::spawn(manager.clone().run());
    (manager, handle)
  }

  pub fn snapshot(&self) -> PresenceSnapshot {
    PresenceSnapshot {
      presence: self.inner.lock().unwrap().published,
    }
  }

  pub fn settings(&self) -> PresenceSettings {
    self.inner.lock().unwrap().settings
  }

  pub async fn set_settings(&self, settings: PresenceSettings) -> Result<(), PresenceSettingsError> {
    validate(&settings)?;
    {
      let mut guard = self.inner.lock().unwrap();
      if guard.settings == settings {
        return Ok(());
      }
      guard.apply(settings);
    }
    save_settings(&self.prefs_path, &settings).await;
    self.poke.notify_one();
    let event = BridgeToClientPresenceMsg::SettingsChanged(PresenceSettingsReply { settings });
    if let Err(errors) = self.bus.broadcast(event, MsgMeta::Event).await {
      tracing::debug!(clients = errors.len(), "presence: settings broadcast reached no client");
    }
    Ok(())
  }

  pub fn update(&self, host: HostPresence) {
    tracing::debug!(?host, "presence: host update");
    let now = Instant::now();
    let mut guard = self.inner.lock().unwrap();
    if let Some(idle_s) = host.idle_s {
      guard.host_active_at = now.checked_sub(Duration::from_secs(u64::from(idle_s)));
    }
    guard.host = Some((host, now));
    drop(guard);
    self.poke.notify_one();
  }

  pub fn touch(&self) {
    let now = Instant::now();
    let mut guard = self.inner.lock().unwrap();
    guard.input_at = now;
    if guard.settings.follow_host {
      guard.wake_until = Some(now + LOCAL_WAKE);
    }
    let lit = guard.published.display == DisplayState::Active;
    drop(guard);
    if !lit {
      self.poke.notify_one();
    }
  }

  async fn run(self) {
    loop {
      let deadline = self.reevaluate().await;
      match deadline {
        Some(at) => {
          tokio::select! {
            () = self.poke.notified() => {}
            () = tokio::time::sleep_until(at) => {}
          }
        }
        None => self.poke.notified().await,
      }
    }
  }

  async fn reevaluate(&self) -> Option<Instant> {
    let (info, screen, deadline, info_changed, screen_changed) = {
      let mut guard = self.inner.lock().unwrap();
      let (info, deadline) = evaluate(&guard, Instant::now());
      let screen = screen_for(info.display, &guard.settings);
      let info_changed = info != guard.published;
      let screen_changed = screen != guard.applied;
      guard.published = info;
      guard.applied = screen;
      (info, screen, deadline, info_changed, screen_changed)
    };
    if info_changed {
      tracing::info!(display = ?info.display, source = ?info.source, "presence: screen state changed");
    }
    if screen_changed && let Err(err) = self.als.set_screen(screen).await {
      tracing::warn!(?err, "presence: could not set the backlight");
    }
    if info_changed {
      let event = BridgeToClientPresenceMsg::Changed(PresenceSnapshot { presence: info });
      if let Err(errors) = self.bus.broadcast(event, MsgMeta::Event).await {
        tracing::debug!(clients = errors.len(), "presence: broadcast reached no client");
      }
    }
    deadline
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
  }

  fn timers(dim: Option<u32>, sleep: Option<u32>) -> PresenceSettings {
    PresenceSettings {
      dim_after_s: dim,
      sleep_after_s: sleep,
      ..PresenceSettings::default()
    }
  }

  fn following() -> PresenceSettings {
    PresenceSettings {
      follow_host: true,
      ..PresenceSettings::default()
    }
  }

  fn following_with(dim: Option<u32>, sleep: Option<u32>) -> PresenceSettings {
    PresenceSettings {
      follow_host: true,
      ..timers(dim, sleep)
    }
  }

  fn with_host(settings: PresenceSettings, display: DisplayState, heartbeat_s: Option<u16>, t0: Instant) -> Inner {
    let mut inner = Inner::new(settings, t0);
    inner.host = Some((
      HostPresence {
        display,
        idle_s: None,
        heartbeat_s,
      },
      t0,
    ));
    inner
  }

  fn state(inner: &Inner, at: Instant) -> (DisplayState, PresenceSource) {
    let (info, _) = evaluate(inner, at);
    (info.display, info.source)
  }

  #[test]
  fn the_defaults_leave_the_screen_to_the_brightness_mode() {
    let t0 = Instant::now();
    let mut inner = with_host(PresenceSettings::default(), DisplayState::Off, Some(10), t0);
    inner.host_active_at = Some(t0);
    let (info, deadline) = evaluate(&inner, t0 + secs(86_400));
    assert_eq!(
      (info.display, info.source),
      (DisplayState::Active, PresenceSource::None)
    );
    assert!(deadline.is_none(), "nothing to wait for when nothing is managed");
  }

  #[test]
  fn following_with_no_host_and_no_timers_leaves_the_screen_alone() {
    let t0 = Instant::now();
    let (info, deadline) = evaluate(&Inner::new(following(), t0), t0 + secs(86_400));
    assert_eq!(
      (info.display, info.source),
      (DisplayState::Active, PresenceSource::None)
    );
    assert!(deadline.is_none());
  }

  #[test]
  fn following_with_no_host_falls_back_to_the_timers() {
    let t0 = Instant::now();
    let inner = Inner::new(following_with(Some(30), Some(120)), t0);
    assert_eq!(
      state(&inner, t0 + secs(29)),
      (DisplayState::Active, PresenceSource::Timer)
    );
    assert_eq!(state(&inner, t0 + secs(30)), (DisplayState::Dim, PresenceSource::Timer));
    assert_eq!(
      state(&inner, t0 + secs(120)),
      (DisplayState::Off, PresenceSource::Timer)
    );
  }

  #[test]
  fn following_sleeps_dims_and_wakes_with_the_host_display() {
    let t0 = Instant::now();
    let mut inner = with_host(following(), DisplayState::Off, Some(10), t0);
    assert_eq!(state(&inner, t0 + secs(5)), (DisplayState::Off, PresenceSource::Host));
    inner.host.as_mut().unwrap().0.display = DisplayState::Active;
    assert_eq!(
      state(&inner, t0 + secs(5)),
      (DisplayState::Active, PresenceSource::Host)
    );
    inner.host.as_mut().unwrap().0.display = DisplayState::Dim;
    assert_eq!(state(&inner, t0 + secs(5)), (DisplayState::Dim, PresenceSource::Host));
  }

  #[test]
  fn a_host_that_is_heard_from_overrides_the_timers() {
    let t0 = Instant::now();
    let lit = with_host(following_with(Some(30), Some(120)), DisplayState::Active, None, t0);
    assert_eq!(
      state(&lit, t0 + secs(500)),
      (DisplayState::Active, PresenceSource::Host)
    );
    let dark = with_host(following_with(None, Some(86_400)), DisplayState::Off, None, t0);
    assert_eq!(state(&dark, t0 + secs(5)), (DisplayState::Off, PresenceSource::Host));
  }

  #[test]
  fn a_report_without_a_heartbeat_never_expires() {
    let t0 = Instant::now();
    let inner = with_host(following(), DisplayState::Dim, None, t0);
    let (info, deadline) = evaluate(&inner, t0 + secs(86_400));
    assert_eq!((info.display, info.source), (DisplayState::Dim, PresenceSource::Host));
    assert!(deadline.is_none());
  }

  #[test]
  fn a_missed_heartbeat_hands_the_screen_to_the_timers() {
    let t0 = Instant::now();
    let inner = with_host(following_with(None, Some(120)), DisplayState::Off, Some(5), t0);
    let (info, deadline) = evaluate(&inner, t0 + secs(14));
    assert_eq!((info.display, info.source), (DisplayState::Off, PresenceSource::Host));
    assert_eq!(
      deadline,
      Some(t0 + secs(15)),
      "three missed beats of the host's own interval"
    );
    let (info, deadline) = evaluate(&inner, t0 + secs(15));
    assert_eq!(
      (info.display, info.source),
      (DisplayState::Active, PresenceSource::Timer)
    );
    assert_eq!(deadline, Some(t0 + secs(120)));
    assert_eq!(
      state(&inner, t0 + secs(120)),
      (DisplayState::Off, PresenceSource::Timer)
    );
  }

  #[test]
  fn a_missed_heartbeat_with_no_timers_leaves_the_screen_lit() {
    let t0 = Instant::now();
    let inner = with_host(following(), DisplayState::Off, Some(5), t0);
    assert_eq!(
      state(&inner, t0 + secs(15)),
      (DisplayState::Active, PresenceSource::None)
    );
  }

  #[test]
  fn a_slow_heartbeat_stretches_how_long_the_host_counts() {
    let t0 = Instant::now();
    let inner = with_host(following_with(None, Some(30)), DisplayState::Active, Some(40), t0);
    assert_eq!(
      state(&inner, t0 + secs(119)),
      (DisplayState::Active, PresenceSource::Host)
    );
    assert_eq!(
      state(&inner, t0 + secs(120)),
      (DisplayState::Off, PresenceSource::Timer)
    );
  }

  #[test]
  fn a_missed_heartbeat_does_not_matter_to_a_screen_that_is_not_following() {
    let t0 = Instant::now();
    let inner = with_host(timers(None, Some(3_600)), DisplayState::Off, Some(10), t0);
    assert_eq!(
      state(&inner, t0 + secs(600)),
      (DisplayState::Active, PresenceSource::Timer)
    );
  }

  #[test]
  fn a_press_wakes_a_screen_that_follows_the_host_until_the_window_lapses() {
    let t0 = Instant::now();
    let mut inner = with_host(following(), DisplayState::Off, None, t0);
    inner.wake_until = Some(t0 + LOCAL_WAKE);
    let (info, deadline) = evaluate(&inner, t0 + secs(5));
    assert_eq!(
      (info.display, info.source),
      (DisplayState::Active, PresenceSource::LocalWake)
    );
    assert_eq!(deadline, Some(t0 + LOCAL_WAKE));
    assert_eq!(
      state(&inner, t0 + LOCAL_WAKE + secs(1)),
      (DisplayState::Off, PresenceSource::Host)
    );
  }

  #[test]
  fn idle_timers_dim_and_then_sleep() {
    let t0 = Instant::now();
    let inner = Inner::new(timers(Some(30), Some(120)), t0);
    let (info, deadline) = evaluate(&inner, t0 + secs(29));
    assert_eq!(
      (info.display, info.source),
      (DisplayState::Active, PresenceSource::Timer)
    );
    assert_eq!(deadline, Some(t0 + secs(30)));
    let (info, deadline) = evaluate(&inner, t0 + secs(30));
    assert_eq!(info.display, DisplayState::Dim);
    assert_eq!(deadline, Some(t0 + secs(120)));
    let (info, deadline) = evaluate(&inner, t0 + secs(120));
    assert_eq!(info.display, DisplayState::Off);
    assert!(deadline.is_none());
  }

  #[test]
  fn either_timer_works_alone() {
    let t0 = Instant::now();
    let dim_only = Inner::new(timers(Some(30), None), t0);
    assert_eq!(state(&dim_only, t0 + secs(86_000)).0, DisplayState::Dim);
    let sleep_only = Inner::new(timers(None, Some(30)), t0);
    assert_eq!(state(&sleep_only, t0 + secs(29)).0, DisplayState::Active);
    assert_eq!(state(&sleep_only, t0 + secs(30)).0, DisplayState::Off);
  }

  #[test]
  fn input_on_the_device_restarts_the_idle_clock() {
    let t0 = Instant::now();
    let mut inner = Inner::new(timers(Some(30), Some(120)), t0);
    assert_eq!(state(&inner, t0 + secs(200)).0, DisplayState::Off);
    inner.input_at = t0 + secs(200);
    assert_eq!(state(&inner, t0 + secs(210)).0, DisplayState::Active);
    assert_eq!(state(&inner, t0 + secs(230)).0, DisplayState::Dim);
  }

  #[test]
  fn activity_on_the_host_keeps_the_screen_lit() {
    let t0 = Instant::now();
    let mut inner = Inner::new(timers(Some(30), Some(120)), t0);
    inner.host_active_at = Some(t0 + secs(100));
    assert_eq!(state(&inner, t0 + secs(110)).0, DisplayState::Active);
    assert_eq!(state(&inner, t0 + secs(130)).0, DisplayState::Dim);
    inner.input_at = t0 + secs(150);
    assert_eq!(
      state(&inner, t0 + secs(160)).0,
      DisplayState::Active,
      "the later of the two counts"
    );
  }

  #[test]
  fn a_host_that_went_quiet_still_counts_its_last_activity() {
    let t0 = Instant::now();
    let mut inner = with_host(following_with(Some(60), Some(300)), DisplayState::Active, Some(5), t0);
    inner.host_active_at = Some(t0 + secs(100));
    assert_eq!(
      state(&inner, t0 + secs(120)),
      (DisplayState::Active, PresenceSource::Timer)
    );
    assert_eq!(
      state(&inner, t0 + secs(160)),
      (DisplayState::Dim, PresenceSource::Timer)
    );
  }

  #[test]
  fn a_dimmed_screen_uses_the_chosen_dim_level() {
    let settings = PresenceSettings {
      dim_level: 0.4,
      ..PresenceSettings::default()
    };
    assert_eq!(screen_for(DisplayState::Active, &settings), Screen::Full);
    assert_eq!(screen_for(DisplayState::Dim, &settings), Screen::Dim(0.4));
    assert_eq!(screen_for(DisplayState::Off, &settings), Screen::Off);
  }

  #[test]
  fn changing_the_settings_ends_a_wake_window() {
    let t0 = Instant::now();
    let mut inner = Inner::new(following(), t0);
    inner.wake_until = Some(t0 + LOCAL_WAKE);
    inner.apply(following_with(Some(30), None));
    assert!(inner.wake_until.is_none());
  }

  #[test]
  fn following_the_host_together_with_timers_is_allowed() {
    assert_eq!(validate(&following_with(Some(30), Some(120))), Ok(()));
  }

  #[test]
  fn dimming_has_to_start_before_sleeping() {
    assert_eq!(
      validate(&timers(Some(60), Some(60))),
      Err(PresenceSettingsError::DimNotBeforeSleep)
    );
    assert_eq!(
      validate(&timers(Some(90), Some(60))),
      Err(PresenceSettingsError::DimNotBeforeSleep)
    );
    assert_eq!(validate(&timers(Some(59), Some(60))), Ok(()));
  }

  #[test]
  fn timers_and_the_dim_level_have_bounds() {
    assert_eq!(
      validate(&timers(Some(4), None)),
      Err(PresenceSettingsError::TimerOutOfRange)
    );
    assert_eq!(
      validate(&timers(None, Some(86_401))),
      Err(PresenceSettingsError::TimerOutOfRange)
    );
    assert_eq!(validate(&timers(Some(5), Some(86_400))), Ok(()));
    for level in [0.0, 0.04, 1.01, f32::NAN] {
      let settings = PresenceSettings {
        dim_level: level,
        ..PresenceSettings::default()
      };
      assert_eq!(
        validate(&settings),
        Err(PresenceSettingsError::DimLevelOutOfRange),
        "{level}"
      );
    }
    let lowest = PresenceSettings {
      dim_level: 0.05,
      ..PresenceSettings::default()
    };
    assert_eq!(validate(&lowest), Ok(()));
  }

  #[test]
  fn settings_survive_a_restart_and_a_bad_file_falls_back_to_the_defaults() {
    let dir = std::env::temp_dir().join("presence-test-settings");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("presence.json");
    assert_eq!(load_settings(&path), PresenceSettings::default());

    let chosen = PresenceSettings {
      follow_host: true,
      dim_after_s: Some(120),
      sleep_after_s: Some(900),
      dim_level: 0.35,
    };
    std::fs::write(&path, serde_json::to_vec(&chosen).unwrap()).unwrap();
    assert_eq!(load_settings(&path), chosen);

    std::fs::write(&path, b"{\"dimAfterS\":3}").unwrap();
    assert_eq!(
      load_settings(&path),
      PresenceSettings::default(),
      "a file the daemon would have refused is not trusted"
    );
    std::fs::write(&path, b"not json").unwrap();
    assert_eq!(load_settings(&path), PresenceSettings::default());
  }
}
