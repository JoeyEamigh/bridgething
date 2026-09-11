use std::{
  collections::VecDeque,
  path::{Path, PathBuf},
  sync::Arc,
  time::Duration,
};

use libbridgething::{
  BrightnessMode, BrightnessState, HardwareError, HardwareState,
  client::{AmbientLightUpdate, BridgeToClientHardwareMsg, HardwareStateReply},
  wire::MsgMeta,
};
use serde::{Deserialize, Serialize};
use tokio::{
  sync::{RwLock, mpsc, oneshot},
  task::JoinHandle,
};

use crate::net::WireEventBus;

const ALS_PATH: &str = "/sys/bus/iio/devices/iio:device0/in_intensity0_raw";
const BACKLIGHT_DIR: &str = "/sys/class/backlight/backlight";
const ALS_PREFS_FILE: &str = "als.json";

#[derive(Debug, Clone)]
pub struct AlsConfig {
  pub poll_interval: Duration,
  pub raw_at_max: f64,
  pub min_brightness: u32,
  pub dim_knee: u32,
  pub median_window: usize,
  pub ease_pct: f32,
  pub integration_time_s: f64,
  pub gain: u32,
  pub als_path: PathBuf,
  pub backlight_dir: PathBuf,
  pub prefs_path: PathBuf,
}

impl Default for AlsConfig {
  fn default() -> Self {
    Self {
      poll_interval: Duration::from_millis(200),
      raw_at_max: 1500.0,
      min_brightness: 16,
      dim_knee: 3,
      median_window: 11,
      ease_pct: 0.15,
      integration_time_s: 0.100,
      gain: 16,
      als_path: PathBuf::from(ALS_PATH),
      backlight_dir: PathBuf::from(BACKLIGHT_DIR),
      prefs_path: crate::paths::state_dir().join(ALS_PREFS_FILE),
    }
  }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct BrightnessPrefs {
  mode: BrightnessMode,
  level: f32,
}

fn load_prefs(path: &Path) -> Option<BrightnessPrefs> {
  let bytes = std::fs::read(path).ok()?;
  serde_json::from_slice(&bytes).ok()
}

async fn save_prefs(path: &Path, prefs: BrightnessPrefs) {
  if let Some(dir) = path.parent()
    && let Err(err) = tokio::fs::create_dir_all(dir).await
  {
    tracing::warn!(path = %path.display(), "als: cannot create prefs dir: {err}");
    return;
  }
  let body = match serde_json::to_vec(&prefs) {
    Ok(body) => body,
    Err(err) => {
      tracing::warn!("als: cannot serialize brightness prefs: {err}");
      return;
    }
  };
  let tmp = path.with_extension("tmp");
  if let Err(err) = tokio::fs::write(&tmp, body).await {
    tracing::warn!(path = %path.display(), "als: cannot write brightness prefs: {err}");
  } else if let Err(err) = tokio::fs::rename(&tmp, path).await {
    tracing::warn!(path = %path.display(), "als: cannot replace brightness prefs: {err}");
  }
}

#[derive(Debug, thiserror::Error)]
pub enum AlsError {
  #[error("backlight sysfs path does not exist: {0}")]
  BacklightAbsent(PathBuf),
  #[error("io: {0}")]
  Io(#[from] std::io::Error),
  #[error("manager loop has exited")]
  Closed,
}

#[derive(Debug)]
struct Inner {
  config: AlsConfig,
  mode: BrightnessMode,
  manual_level: f32,
  samples: VecDeque<u32>,
  current_ticks: u32,
  max_brightness: u32,
}

impl Inner {
  fn new(config: AlsConfig, max_brightness: u32, current_ticks: u32, prefs: Option<BrightnessPrefs>) -> Self {
    let cap = config.median_window.max(1);
    let (mode, manual_level) = prefs
      .map(|p| (p.mode, p.level.clamp(0.0, 1.0)))
      .unwrap_or((BrightnessMode::Auto, 1.0));
    Self {
      config,
      mode,
      manual_level,
      samples: VecDeque::with_capacity(cap),
      current_ticks,
      max_brightness,
    }
  }

  fn snapshot(&self) -> HardwareState {
    HardwareState {
      brightness: BrightnessState {
        mode: self.mode,
        level: self.manual_level,
        effective_level: self.current_level(),
      },
      ambient_level: self.ambient_level(),
    }
  }

  fn current_level(&self) -> f32 {
    if self.max_brightness == 0 {
      return 0.0;
    }
    self.current_ticks as f32 / self.max_brightness as f32
  }

  fn ambient_level(&self) -> u8 {
    if self.max_brightness == 0 {
      return 0;
    }
    ((self.current_ticks * 100) / self.max_brightness).min(100) as u8
  }

  fn push_sample(&mut self, raw: u32) {
    if self.samples.len() == self.config.median_window {
      self.samples.pop_front();
    }
    self.samples.push_back(raw);
  }

  fn median(&self) -> Option<u32> {
    if self.samples.len() < self.config.median_window {
      return None;
    }
    let mut sorted: Vec<u32> = self.samples.iter().copied().collect();
    sorted.sort_unstable();
    Some(sorted[sorted.len() / 2])
  }

  fn target_for_raw(&self, raw: u32) -> u32 {
    if self.max_brightness == 0 {
      return 0;
    }
    let min_t = self.config.min_brightness.min(self.max_brightness);
    if raw <= self.config.dim_knee {
      return min_t;
    }
    let log_max = (1.0 + self.config.raw_at_max).log10();
    let ratio = ((1.0 + raw as f64).log10() / log_max).clamp(0.0, 1.0);
    let span = (self.max_brightness - min_t) as f64;
    min_t + (span * ratio).round() as u32
  }

  fn level_to_ticks(&self, level: f32) -> u32 {
    let level = level.clamp(0.0, 1.0);
    (level * self.max_brightness as f32 + 0.5) as u32
  }
}

fn ease_step(current: u32, target: u32, ease_pct: f32) -> u32 {
  if current == target {
    return current;
  }
  let diff = target as i32 - current as i32;
  let mag = diff.unsigned_abs();
  let mut step = ((mag as f32) * ease_pct).round() as u32;
  if step == 0 {
    step = 1;
  }
  if step > mag {
    step = mag;
  }
  if diff > 0 { current + step } else { current - step }
}

#[derive(Debug)]
enum Cmd {
  SetMode(BrightnessMode, oneshot::Sender<Result<(), AlsError>>),
  SetLevel(f32, oneshot::Sender<Result<Result<(), HardwareError>, AlsError>>),
}

#[derive(Debug, Clone)]
pub struct AlsManager {
  inner: Arc<RwLock<Inner>>,
  tx: mpsc::Sender<Cmd>,
}

impl AlsManager {
  pub async fn init(bus: WireEventBus, config: AlsConfig) -> Result<AlsManagerInit, AlsError> {
    apply_chip_config(&config).await;

    let max_brightness = read_max_brightness(&config.backlight_dir).await.unwrap_or_else(|_| {
      tracing::warn!(
        "als: unable to read {}/max_brightness; deferring backlight policy until paths exist",
        config.backlight_dir.display(),
      );
      0
    });
    let initial_ticks = read_actual_brightness(&config.backlight_dir)
      .await
      .unwrap_or(max_brightness)
      .min(max_brightness);
    if max_brightness > 0 {
      tracing::info!(
        "als: initialized (max={max_brightness}, min={}, raw_at_max={}, knee={}, window={}, ease={:.2}, gain={}, integ={}s, current={initial_ticks})",
        config.min_brightness,
        config.raw_at_max,
        config.dim_knee,
        config.median_window,
        config.ease_pct,
        config.gain,
        config.integration_time_s,
      );
    }

    let prefs = load_prefs(&config.prefs_path);
    let inner = Arc::new(RwLock::new(Inner::new(config, max_brightness, initial_ticks, prefs)));
    let restore = {
      let guard = inner.read().await;
      (guard.mode == BrightnessMode::Manual && guard.max_brightness > 0).then(|| {
        (
          guard.level_to_ticks(guard.manual_level),
          guard.config.backlight_dir.clone(),
        )
      })
    };
    if let Some((ticks, dir)) = restore {
      match write_brightness(&dir, ticks).await {
        Ok(()) => {
          inner.write().await.current_ticks = ticks;
        }
        Err(err) => {
          tracing::warn!(dir = %dir.display(), "als: cannot restore manual brightness: {err}");
        }
      }
    }
    let (tx, rx) = mpsc::channel(16);
    Ok(AlsManagerInit {
      manager: Self {
        inner: inner.clone(),
        tx,
      },
      rx,
      inner,
      bus,
    })
  }

  pub async fn snapshot(&self) -> HardwareState {
    self.inner.read().await.snapshot()
  }

  pub async fn snapshot_reply(&self) -> HardwareStateReply {
    HardwareStateReply {
      state: self.snapshot().await,
    }
  }

  pub async fn set_mode(&self, mode: BrightnessMode) -> Result<(), AlsError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .tx
      .send(Cmd::SetMode(mode, reply_tx))
      .await
      .map_err(|_| AlsError::Closed)?;
    reply_rx.await.map_err(|_| AlsError::Closed)?
  }

  pub async fn set_level(&self, level: f32) -> Result<Result<(), HardwareError>, AlsError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .tx
      .send(Cmd::SetLevel(level, reply_tx))
      .await
      .map_err(|_| AlsError::Closed)?;
    reply_rx.await.map_err(|_| AlsError::Closed)?
  }
}

pub struct AlsManagerInit {
  pub manager: AlsManager,
  rx: mpsc::Receiver<Cmd>,
  inner: Arc<RwLock<Inner>>,
  bus: WireEventBus,
}

impl AlsManagerInit {
  pub fn spawn(self) -> (AlsManager, JoinHandle<()>) {
    let manager = self.manager.clone();
    let handle = tokio::spawn(run_loop(self.rx, self.inner, self.bus));
    (manager, handle)
  }
}

async fn run_loop(mut rx: mpsc::Receiver<Cmd>, inner: Arc<RwLock<Inner>>, bus: WireEventBus) {
  let mut interval = {
    let guard = inner.read().await;
    tokio::time::interval(guard.config.poll_interval)
  };
  interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

  loop {
    tokio::select! {
      cmd = rx.recv() => {
        let Some(cmd) = cmd else { break; };
        handle_cmd(cmd, &inner, &bus).await;
      }
      _ = interval.tick() => {
        if let Err(err) = poll_once(&inner, &bus).await {
          tracing::trace!("als poll failed: {err}");
        }
      }
    }
  }
  tracing::debug!("als manager loop exiting");
}

async fn handle_cmd(cmd: Cmd, inner: &Arc<RwLock<Inner>>, bus: &WireEventBus) {
  match cmd {
    Cmd::SetMode(mode, reply) => {
      let (write_ticks, dir) = {
        let guard = inner.read().await;
        if guard.mode == mode {
          let _ = reply.send(Ok(()));
          return;
        }
        let ticks = if mode == BrightnessMode::Manual {
          guard.level_to_ticks(guard.manual_level)
        } else {
          guard
            .median()
            .map(|m| guard.target_for_raw(m))
            .unwrap_or(guard.current_ticks)
        };
        (ticks, guard.config.backlight_dir.clone())
      };
      if let Err(err) = write_brightness(&dir, write_ticks).await {
        tracing::error!(dir = %dir.display(), ticks = write_ticks, "als: backlight write failed: {err}");
        let _ = reply.send(Err(err));
        return;
      }
      let (prefs_path, prefs, brightness) = {
        let mut guard = inner.write().await;
        guard.mode = mode;
        guard.current_ticks = write_ticks;
        let prefs = BrightnessPrefs {
          mode: guard.mode,
          level: guard.manual_level,
        };
        (guard.config.prefs_path.clone(), prefs, guard.snapshot().brightness)
      };
      save_prefs(&prefs_path, prefs).await;
      let _ = reply.send(Ok(()));
      broadcast(bus, BridgeToClientHardwareMsg::BrightnessChanged(brightness)).await;
    }
    Cmd::SetLevel(level, reply) => {
      if !(0.0..=1.0).contains(&level) {
        let _ = reply.send(Ok(Err(HardwareError::LevelOutOfRange)));
        return;
      }
      let (mismatch, write_ticks, dir) = {
        let guard = inner.read().await;
        let mismatch = guard.mode != BrightnessMode::Manual;
        let ticks = (!mismatch).then(|| guard.level_to_ticks(level));
        (mismatch, ticks, guard.config.backlight_dir.clone())
      };
      if let Some(ticks) = write_ticks
        && let Err(err) = write_brightness(&dir, ticks).await
      {
        tracing::error!(dir = %dir.display(), ticks, "als: backlight write failed: {err}");
        let _ = reply.send(Err(err));
        return;
      }
      let (prefs_path, prefs, brightness) = {
        let mut guard = inner.write().await;
        guard.manual_level = level;
        if let Some(ticks) = write_ticks {
          guard.current_ticks = ticks;
        }
        let prefs = BrightnessPrefs {
          mode: guard.mode,
          level: guard.manual_level,
        };
        (guard.config.prefs_path.clone(), prefs, guard.snapshot().brightness)
      };
      save_prefs(&prefs_path, prefs).await;
      let outcome = if mismatch {
        Err(HardwareError::ModeMismatch)
      } else {
        Ok(())
      };
      let _ = reply.send(Ok(outcome));
      broadcast(bus, BridgeToClientHardwareMsg::BrightnessChanged(brightness)).await;
    }
  }
}

async fn poll_once(inner: &Arc<RwLock<Inner>>, bus: &WireEventBus) -> Result<(), AlsError> {
  let als_path = inner.read().await.config.als_path.clone();
  let sample = match read_raw(&als_path).await {
    Ok(v) => v,
    Err(_) => return Ok(()),
  };
  if inner.read().await.max_brightness == 0 {
    let dir = inner.read().await.config.backlight_dir.clone();
    let max = read_max_brightness(&dir).await?;
    let actual = read_actual_brightness(&dir).await.unwrap_or(max).min(max);
    let restore_ticks = {
      let mut guard = inner.write().await;
      guard.max_brightness = max;
      guard.current_ticks = actual;
      (guard.mode == BrightnessMode::Manual).then(|| guard.level_to_ticks(guard.manual_level))
    };
    if let Some(ticks) = restore_ticks
      && write_brightness(&dir, ticks).await.is_ok()
    {
      inner.write().await.current_ticks = ticks;
    }
  }

  let (ticks_to_write, dir, prev_level) = {
    let mut guard = inner.write().await;
    guard.push_sample(sample);
    let prev_level = guard.ambient_level();

    let mut ticks_to_write: Option<u32> = None;
    if guard.mode == BrightnessMode::Auto
      && let Some(m) = guard.median()
    {
      let target = guard.target_for_raw(m);
      let next = ease_step(guard.current_ticks, target, guard.config.ease_pct);
      if next != guard.current_ticks {
        ticks_to_write = Some(next);
      }
    }

    (ticks_to_write, guard.config.backlight_dir.clone(), prev_level)
  };

  let Some(ticks) = ticks_to_write else {
    return Ok(());
  };
  if let Err(err) = write_brightness(&dir, ticks).await {
    tracing::error!(dir = %dir.display(), ticks, "als: backlight write failed: {err}");
    return Ok(());
  }

  let (brightness, level) = {
    let mut guard = inner.write().await;
    guard.current_ticks = ticks;
    (guard.snapshot().brightness, guard.ambient_level())
  };
  if level != prev_level {
    broadcast(
      bus,
      BridgeToClientHardwareMsg::AmbientLightUpdate(AmbientLightUpdate { ambient_level: level }),
    )
    .await;
  }
  broadcast(bus, BridgeToClientHardwareMsg::BrightnessChanged(brightness)).await;
  Ok(())
}

async fn apply_chip_config(config: &AlsConfig) {
  let calibscale = config.als_path.with_file_name("in_intensity0_calibscale");
  let integ = config.als_path.with_file_name("in_intensity0_integration_time");
  if let Err(err) = tokio::fs::write(&integ, format!("{:.6}\n", config.integration_time_s)).await {
    tracing::warn!(
      "als: failed to write integration_time={} to {}: {err}",
      config.integration_time_s,
      integ.display(),
    );
  }
  if let Err(err) = tokio::fs::write(&calibscale, format!("{}\n", config.gain)).await {
    tracing::warn!(
      "als: failed to write gain={} to {}: {err}",
      config.gain,
      calibscale.display(),
    );
  }
}

async fn read_raw(path: &Path) -> Result<u32, AlsError> {
  let bytes = tokio::fs::read(path).await?;
  Ok(parse_uint(&bytes))
}

async fn read_max_brightness(dir: &Path) -> Result<u32, AlsError> {
  let path = dir.join("max_brightness");
  if !tokio::fs::try_exists(&path).await? {
    return Err(AlsError::BacklightAbsent(dir.to_path_buf()));
  }
  let bytes = tokio::fs::read(&path).await?;
  Ok(parse_uint(&bytes))
}

async fn read_actual_brightness(dir: &Path) -> Result<u32, AlsError> {
  let bytes = tokio::fs::read(dir.join("actual_brightness")).await?;
  Ok(parse_uint(&bytes))
}

async fn write_brightness(dir: &Path, ticks: u32) -> Result<(), AlsError> {
  let path = dir.join("brightness");
  tokio::fs::write(path, format!("{ticks}\n")).await?;
  Ok(())
}

fn parse_uint(bytes: &[u8]) -> u32 {
  let s = std::str::from_utf8(bytes).unwrap_or("0").trim();
  s.parse().unwrap_or(0)
}

async fn broadcast(bus: &WireEventBus, event: BridgeToClientHardwareMsg) {
  if let Err(errors) = bus.broadcast(event, MsgMeta::Event).await {
    tracing::trace!("als broadcast had {} ws error(s)", errors.len());
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&root);
    root
  }

  async fn manager_at(root: &Path) -> (AlsManager, JoinHandle<()>) {
    let (client_man, _listener) = crate::net::create_client_manager();
    let config = AlsConfig {
      als_path: root.join("in_intensity0_raw"),
      backlight_dir: root.join("backlight"),
      prefs_path: root.join("als.json"),
      ..Default::default()
    };
    let (manager, loop_handle) = AlsManager::init(WireEventBus::new(client_man), config)
      .await
      .expect("als init tolerates absent sysfs")
      .spawn();
    (manager, loop_handle)
  }

  #[tokio::test]
  async fn a_backlight_write_that_never_landed_is_reported_rather_than_announced() {
    let root = scratch("als-test-absent-backlight");
    let (manager, _rig) = manager_at(&root).await;

    let mode = manager.set_mode(BrightnessMode::Manual).await;
    assert!(
      matches!(mode, Err(AlsError::Io(_))),
      "a mode switch whose backlight write failed cannot report success: {mode:?}"
    );
    assert_eq!(
      manager.snapshot().await.brightness.mode,
      BrightnessMode::Auto,
      "the daemon entered a mode the panel never took"
    );
  }

  #[tokio::test]
  async fn a_failed_backlight_write_leaves_the_prior_brightness_in_place() {
    let root = scratch("als-test-write-failure");
    let backlight = root.join("backlight");
    std::fs::create_dir_all(&backlight).expect("scratch backlight");
    std::fs::write(backlight.join("max_brightness"), "255\n").expect("max_brightness");
    std::fs::write(backlight.join("actual_brightness"), "128\n").expect("actual_brightness");
    std::fs::write(backlight.join("brightness"), "128\n").expect("brightness");

    let (manager, _rig) = manager_at(&root).await;
    manager
      .set_mode(BrightnessMode::Manual)
      .await
      .expect("a writable backlight takes the mode switch");
    manager
      .set_level(0.5)
      .await
      .expect("a writable backlight takes the level")
      .expect("manual mode accepts a level");
    let settled = manager.snapshot().await.brightness;

    std::fs::remove_file(backlight.join("brightness")).expect("unlink brightness");
    std::fs::create_dir(backlight.join("brightness")).expect("shadow brightness");

    let failed = manager.set_level(0.9).await;
    assert!(
      matches!(failed, Err(AlsError::Io(_))),
      "a level change whose backlight write failed cannot report success: {failed:?}"
    );
    let after = manager.snapshot().await.brightness;
    assert_eq!(
      after.level, settled.level,
      "the daemon reports a brightness the panel never took"
    );
    assert_eq!(after.effective_level, settled.effective_level);
  }

  #[tokio::test]
  async fn manual_brightness_is_restored_after_a_restart() {
    let root = scratch("als-test-persist");
    let backlight = root.join("backlight");
    std::fs::create_dir_all(&backlight).expect("scratch backlight");
    std::fs::write(backlight.join("max_brightness"), "255\n").expect("max_brightness");
    std::fs::write(backlight.join("actual_brightness"), "255\n").expect("actual_brightness");
    std::fs::write(backlight.join("brightness"), "255\n").expect("brightness");

    let (manager, loop_handle) = manager_at(&root).await;
    manager.set_mode(BrightnessMode::Manual).await.expect("mode switch");
    manager
      .set_level(0.42)
      .await
      .expect("level write")
      .expect("manual mode accepts a level");
    let prefs_body = std::fs::read_to_string(root.join("als.json")).expect("prefs were persisted");
    assert!(
      prefs_body.contains("\"manual\""),
      "prefs record manual mode: {prefs_body}"
    );
    drop(manager);
    loop_handle.abort();

    std::fs::write(backlight.join("actual_brightness"), "255\n").expect("actual_brightness");

    let (manager2, _rig2) = manager_at(&root).await;
    let state = manager2.snapshot().await.brightness;
    assert_eq!(state.mode, BrightnessMode::Manual, "manual mode survives a reboot");
    assert!(
      (state.level - 0.42).abs() < f32::EPSILON,
      "manual level survives a reboot: {}",
      state.level
    );
    let panel: String = std::fs::read_to_string(backlight.join("brightness"))
      .expect("panel brightness")
      .trim()
      .to_string();
    assert_eq!(panel, "107", "the panel itself is restored to the manual level");
  }
}
