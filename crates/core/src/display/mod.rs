mod brightness;
mod dim;
mod sysfs;

use std::{path::PathBuf, time::Duration};

use brightness::{Curve, Sensor, dimmed_ticks, ease_step, level_to_ticks};
use dim::DimState;
use libbridgething::{
  BrightnessMode, BrightnessState, DimSettings, DisplaySettings, HardwareError, HardwareState,
  client::{AmbientLightUpdate, BridgeToClientHardwareMsg, HardwareStateReply},
  gateway::{BridgeToGatewaySystemMsgCommand, BridgeToGatewaySystemMsgEvent, HostState, HostWatch},
  wire::MsgMeta,
};
use tokio::{
  sync::{mpsc, oneshot, watch},
  task::JoinHandle,
  time::Instant,
};

use crate::{
  bluetooth::{Address, GatewayMan},
  deadline::past_deadline,
  net::WireEventBus,
  state::storage::DisplaySettingsStore,
};

const ALS_PATH: &str = "/sys/bus/iio/devices/iio:device0/in_intensity0_raw";
const BACKLIGHT_DIR: &str = "/sys/class/backlight/backlight";

#[derive(Debug, Clone)]
pub struct DisplayConfig {
  pub poll_interval: Duration,
  pub curve: Curve,
  pub ease_pct: f32,
  pub integration_time_s: f64,
  pub gain: u32,
  pub als_path: PathBuf,
  pub backlight_dir: PathBuf,
}

impl Default for DisplayConfig {
  fn default() -> Self {
    Self {
      poll_interval: Duration::from_millis(200),
      curve: Curve {
        raw_at_max: 1500.0,
        min_brightness: 16,
        dim_knee: 3,
        median_window: 11,
      },
      ease_pct: 0.15,
      integration_time_s: 0.100,
      gain: 16,
      als_path: PathBuf::from(ALS_PATH),
      backlight_dir: PathBuf::from(BACKLIGHT_DIR),
    }
  }
}

#[derive(Debug, thiserror::Error)]
pub enum DisplayError {
  #[error("backlight sysfs path does not exist: {0}")]
  BacklightAbsent(PathBuf),
  #[error("io: {0}")]
  Io(#[from] std::io::Error),
  #[error("display actor has exited")]
  Closed,
}

type Reply<T> = oneshot::Sender<T>;

pub type SetResult = Result<Result<(), HardwareError>, DisplayError>;

#[derive(Debug, Clone, Copy)]
enum Patch {
  Mode(BrightnessMode),
  Level(f32),
  All(DisplaySettings),
}

impl Patch {
  fn onto(self, settings: DisplaySettings) -> DisplaySettings {
    match self {
      Patch::Mode(mode) => DisplaySettings { mode, ..settings },
      Patch::Level(level) => DisplaySettings { level, ..settings },
      Patch::All(all) => all,
    }
  }
}

fn validate(settings: &DisplaySettings) -> Result<(), HardwareError> {
  if !(0.0..=1.0).contains(&settings.level) {
    return Err(HardwareError::LevelOutOfRange);
  }
  dim::validate(&settings.dim)
}

#[derive(Debug)]
enum Cmd {
  Set(Patch, Reply<SetResult>),
  Input,
  LinkUp(Address),
  LinkLost(Address),
  HostState(Address, HostState, Reply<bool>),
}

#[derive(Debug, Clone)]
pub struct DisplayManager {
  tx: mpsc::Sender<Cmd>,
  state_rx: watch::Receiver<HardwareState>,
}

pub struct DisplayInit {
  manager: DisplayManager,
  actor: Actor,
  rx: mpsc::Receiver<Cmd>,
}

impl DisplayInit {
  pub fn spawn(self) -> (DisplayManager, JoinHandle<()>) {
    let handle = tokio::spawn(self.actor.run(self.rx));
    (self.manager, handle)
  }
}

impl DisplayManager {
  pub async fn init(
    bus: WireEventBus,
    gateways: GatewayMan,
    store: DisplaySettingsStore,
    config: DisplayConfig,
  ) -> DisplayInit {
    sysfs::apply_chip_config(&config).await;
    let max = sysfs::read_max_brightness(&config.backlight_dir)
      .await
      .unwrap_or_else(|_| {
        tracing::warn!(
          "display: unable to read {}/max_brightness; deferring backlight policy until paths exist",
          config.backlight_dir.display(),
        );
        0
      });
    let current = sysfs::read_actual_brightness(&config.backlight_dir)
      .await
      .unwrap_or(max)
      .min(max);
    if max > 0 {
      tracing::info!(
        "display: initialized (max={max}, curve={:?}, ease={:.2}, gain={}, integ={}s, current={current})",
        config.curve,
        config.ease_pct,
        config.gain,
        config.integration_time_s,
      );
    }

    let prefs = match store.load().await {
      Ok(prefs) => prefs.unwrap_or_default(),
      Err(err) => {
        tracing::warn!("display: cannot load settings, using defaults: {err}");
        DisplaySettings::default()
      }
    };
    let dim = if dim::validate(&prefs.dim).is_ok() {
      prefs.dim
    } else {
      DimSettings::default()
    };

    let mut actor = Actor {
      sensor: Sensor::new(config.curve),
      config,
      max,
      current,
      normal: current,
      mode: prefs.mode,
      manual_level: prefs.level.clamp(0.0, 1.0),
      dim: DimState::new(dim, Instant::now()),
      dimmed: false,
      store,
      bus,
      gateways,
      state_tx: watch::Sender::new(HardwareState::default()),
    };
    if actor.mode == BrightnessMode::Manual && actor.max > 0 {
      let ticks = level_to_ticks(actor.manual_level, actor.max);
      match sysfs::write_brightness(&actor.config.backlight_dir, ticks).await {
        Ok(()) => {
          actor.current = ticks;
          actor.normal = ticks;
        }
        Err(err) => tracing::warn!("display: cannot restore manual brightness: {err}"),
      }
    }
    actor.publish();

    let (tx, rx) = mpsc::channel(16);
    DisplayInit {
      manager: Self {
        tx,
        state_rx: actor.state_tx.subscribe(),
      },
      actor,
      rx,
    }
  }

  pub fn snapshot(&self) -> HardwareState {
    *self.state_rx.borrow()
  }

  pub fn snapshot_reply(&self) -> HardwareStateReply {
    HardwareStateReply { state: self.snapshot() }
  }

  pub fn settings(&self) -> DisplaySettings {
    let state = self.snapshot();
    DisplaySettings {
      mode: state.brightness.mode,
      level: state.brightness.level,
      dim: state.dim,
    }
  }

  async fn ask<T>(&self, cmd: impl FnOnce(Reply<T>) -> Cmd) -> Result<T, DisplayError> {
    let (reply, rx) = oneshot::channel();
    self.tx.send(cmd(reply)).await.map_err(|_| DisplayError::Closed)?;
    rx.await.map_err(|_| DisplayError::Closed)
  }

  async fn tell(&self, cmd: Cmd) {
    if self.tx.send(cmd).await.is_err() {
      tracing::warn!("display: actor gone; command dropped");
    }
  }

  async fn set(&self, patch: Patch) -> SetResult {
    self.ask(|reply| Cmd::Set(patch, reply)).await?
  }

  pub async fn set_mode(&self, mode: BrightnessMode) -> SetResult {
    self.set(Patch::Mode(mode)).await
  }

  pub async fn set_level(&self, level: f32) -> SetResult {
    self.set(Patch::Level(level)).await
  }

  pub async fn set_settings(&self, settings: DisplaySettings) -> SetResult {
    self.set(Patch::All(settings)).await
  }

  pub async fn note_input(&self) {
    self.tell(Cmd::Input).await;
  }

  pub async fn link_up(&self, gateway: Address) {
    self.tell(Cmd::LinkUp(gateway)).await;
  }

  pub async fn link_lost(&self, gateway: Address) {
    self.tell(Cmd::LinkLost(gateway)).await;
  }

  pub async fn host_state(&self, gateway: Address, state: HostState) -> Result<bool, DisplayError> {
    self.ask(|reply| Cmd::HostState(gateway, state, reply)).await
  }
}

fn host_watch(dim: &DimSettings) -> HostWatch {
  HostWatch {
    idle_after_s: dim.after_s,
  }
}

struct Actor {
  config: DisplayConfig,
  sensor: Sensor,
  max: u32,
  current: u32,
  normal: u32,
  mode: BrightnessMode,
  manual_level: f32,
  dim: DimState<Address>,
  dimmed: bool,
  store: DisplaySettingsStore,
  bus: WireEventBus,
  gateways: GatewayMan,
  state_tx: watch::Sender<HardwareState>,
}

impl Actor {
  async fn run(mut self, mut rx: mpsc::Receiver<Cmd>) {
    let mut poll = tokio::time::interval(self.config.poll_interval);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
      let recheck = self.dim.decide(Instant::now()).recheck_at;
      tokio::select! {
        cmd = rx.recv() => {
          let Some(cmd) = cmd else { break };
          self.handle(cmd).await;
        }
        _ = poll.tick() => self.poll().await,
        () = past_deadline(recheck) => {}
      }
      self.reconcile().await;
    }
    tracing::debug!("display actor exiting");
  }

  async fn handle(&mut self, cmd: Cmd) {
    match cmd {
      Cmd::Set(patch, reply) => {
        let _ = reply.send(self.set(patch).await);
      }
      Cmd::Input => self.dim.input(Instant::now()),
      Cmd::LinkUp(gateway) => {
        let watch = host_watch(&self.dim.settings());
        if self.dim.link_up(gateway) && watch.idle_after_s.is_some() {
          self.send_watch(gateway, watch).await;
        }
      }
      Cmd::LinkLost(gateway) => self.dim.link_lost(gateway),
      Cmd::HostState(gateway, state, reply) => {
        let applied = self.dim.report(gateway, state);
        if applied {
          tracing::debug!(%gateway, ?state, "display: host state");
          self.reconcile().await;
        } else {
          tracing::trace!(%gateway, "display: host state from a gateway with no link; dropped");
        }
        let _ = reply.send(applied);
      }
    }
  }

  fn settings(&self) -> DisplaySettings {
    DisplaySettings {
      mode: self.mode,
      level: self.manual_level,
      dim: self.dim.settings(),
    }
  }

  async fn set(&mut self, patch: Patch) -> SetResult {
    let prior = self.settings();
    let next = patch.onto(prior);
    if let Err(err) = validate(&next) {
      return Ok(Err(err));
    }
    if next != prior {
      let relit = next.mode != prior.mode || (next.mode == BrightnessMode::Manual && next.level != prior.level);
      if relit {
        let normal = self.normal_at(next.mode, next.level);
        let ticks = if self.dimmed {
          dimmed_ticks(next.mode, normal, next.dim.level, self.max)
        } else {
          normal
        };
        self.write(ticks).await?;
        self.normal = normal;
      }
      self.mode = next.mode;
      self.manual_level = next.level;
      self.dim.set_settings(next.dim);
      self.persist().await;
      tracing::debug!(?next, "display: settings changed");
      self.announce(prior, next).await;
    }
    Ok(match patch {
      Patch::Level(_) if next.mode != BrightnessMode::Manual => Err(HardwareError::ModeMismatch),
      _ => Ok(()),
    })
  }

  async fn announce(&self, prior: DisplaySettings, next: DisplaySettings) {
    self.publish();
    if (next.mode, next.level) != (prior.mode, prior.level) {
      self.announce_brightness().await;
    }
    if next.dim != prior.dim {
      self.broadcast(BridgeToClientHardwareMsg::DimChanged(next.dim)).await;
      let watch = host_watch(&next.dim);
      if watch != host_watch(&prior.dim) {
        let linked: Vec<Address> = self.dim.linked().collect();
        for gateway in linked {
          self.send_watch(gateway, watch).await;
        }
      }
    }
    self
      .gateways
      .broadcast(BridgeToGatewaySystemMsgEvent::DisplaySettingsChanged(next))
      .await;
  }

  async fn send_watch(&self, gateway: Address, watch: HostWatch) {
    tracing::debug!(%gateway, ?watch, "display: asking a gateway for its host state");
    self
      .gateways
      .send_command(gateway, BridgeToGatewaySystemMsgCommand::HostWatch(watch))
      .await;
  }

  async fn poll(&mut self) {
    if let Ok(sample) = sysfs::read_raw(&self.config.als_path).await {
      let prior = self.sensor.ambient_level();
      self.sensor.push(sample);
      let ambient_level = self.sensor.ambient_level();
      if ambient_level != prior {
        self.publish();
        self
          .broadcast(BridgeToClientHardwareMsg::AmbientLightUpdate(AmbientLightUpdate {
            ambient_level,
          }))
          .await;
      }
    }
    if self.max == 0 && !self.discover_backlight().await {
      return;
    }
    self.normal = self.normal_for(self.mode);
    let next = ease_step(self.current, self.shown(self.mode, self.normal), self.config.ease_pct);
    if next == self.current {
      return;
    }
    if let Err(err) = self.write(next).await {
      tracing::error!(ticks = next, "display: backlight write failed: {err}");
      return;
    }
    self.announce_brightness().await;
  }

  async fn discover_backlight(&mut self) -> bool {
    let dir = self.config.backlight_dir.clone();
    let Ok(max) = sysfs::read_max_brightness(&dir).await else {
      return false;
    };
    self.max = max;
    self.current = sysfs::read_actual_brightness(&dir).await.unwrap_or(max).min(max);
    if self.mode == BrightnessMode::Manual {
      let ticks = level_to_ticks(self.manual_level, max);
      if self.write(ticks).await.is_ok() {
        self.normal = ticks;
      }
    }
    max > 0
  }

  async fn reconcile(&mut self) {
    let dimmed = self.dim.decide(Instant::now()).dimmed;
    if dimmed == self.dimmed {
      return;
    }
    self.dimmed = dimmed;
    tracing::debug!(dimmed, "display: dim state changed");
    if !dimmed && self.max > 0 {
      let ticks = self.shown(self.mode, self.normal_for(self.mode));
      if let Err(err) = self.write(ticks).await {
        tracing::error!(ticks, "display: backlight write failed: {err}");
      }
    }
    self.announce_brightness().await;
  }

  fn normal_for(&self, mode: BrightnessMode) -> u32 {
    self.normal_at(mode, self.manual_level)
  }

  fn normal_at(&self, mode: BrightnessMode, level: f32) -> u32 {
    match mode {
      BrightnessMode::Manual => level_to_ticks(level, self.max),
      BrightnessMode::Auto => self.sensor.target(self.max).unwrap_or(self.normal),
    }
  }

  fn shown(&self, mode: BrightnessMode, normal: u32) -> u32 {
    if self.dimmed {
      dimmed_ticks(mode, normal, self.dim.settings().level, self.max)
    } else {
      normal
    }
  }

  async fn write(&mut self, ticks: u32) -> Result<(), DisplayError> {
    sysfs::write_brightness(&self.config.backlight_dir, ticks).await?;
    self.current = ticks;
    Ok(())
  }

  fn snapshot(&self) -> HardwareState {
    HardwareState {
      brightness: BrightnessState {
        mode: self.mode,
        level: self.manual_level,
        effective_level: if self.max == 0 {
          0.0
        } else {
          self.current as f32 / self.max as f32
        },
        dimmed: self.dimmed,
      },
      dim: self.dim.settings(),
      ambient_level: self.sensor.ambient_level(),
    }
  }

  fn publish(&self) {
    self.state_tx.send_replace(self.snapshot());
  }

  async fn announce_brightness(&self) {
    self.publish();
    let brightness = self.snapshot().brightness;
    self
      .broadcast(BridgeToClientHardwareMsg::BrightnessChanged(brightness))
      .await;
  }

  async fn broadcast(&self, event: BridgeToClientHardwareMsg) {
    if let Err(errors) = self.bus.broadcast(event, MsgMeta::Event).await {
      tracing::trace!("display broadcast had {} ws error(s)", errors.len());
    }
  }

  async fn persist(&self) {
    if let Err(err) = self.store.save(&self.settings()).await {
      tracing::warn!("display: cannot persist settings: {err}");
    }
  }
}

#[cfg(test)]
mod tests {
  use std::path::Path;

  use libbridgething::gateway::{BridgeToGatewayMsgData, BridgeToGatewaySystemMsg};
  use sea_orm::DatabaseConnection;

  use super::*;
  use crate::bluetooth::GatewaySendRx;

  const HOST: Address = Address([0xFE, 0xED, 0, 0, 0, 1]);
  const OTHER: Address = Address([0xFE, 0xED, 0, 0, 0, 2]);

  struct Rig {
    display: DisplayManager,
    sent: GatewaySendRx,
    _actor: JoinHandle<()>,
  }

  fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch root");
    root
  }

  fn backlight_at(root: &Path, ticks: u32) -> PathBuf {
    let backlight = root.join("backlight");
    std::fs::create_dir_all(&backlight).expect("scratch backlight");
    std::fs::write(backlight.join("max_brightness"), "255\n").expect("max_brightness");
    std::fs::write(backlight.join("actual_brightness"), format!("{ticks}\n")).expect("actual_brightness");
    std::fs::write(backlight.join("brightness"), format!("{ticks}\n")).expect("brightness");
    backlight
  }

  fn panel(backlight: &Path) -> u32 {
    loop {
      let body = std::fs::read_to_string(backlight.join("brightness")).expect("panel brightness");
      if let Ok(ticks) = body.trim().parse() {
        return ticks;
      }
      std::thread::yield_now();
    }
  }

  async fn settles_at(backlight: &Path, ticks: u32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
      if panel(backlight) == ticks {
        return true;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panel(backlight) == ticks
  }

  async fn rig_with(root: &Path, db: DatabaseConnection, ease_pct: f32) -> Rig {
    let (client_man, _listener) = crate::net::create_client_manager();
    let (gateways, sent) = GatewayMan::capturing();
    let config = DisplayConfig {
      poll_interval: Duration::from_millis(10),
      ease_pct,
      als_path: root.join("in_intensity0_raw"),
      backlight_dir: root.join("backlight"),
      ..Default::default()
    };
    let (display, actor) = DisplayManager::init(
      WireEventBus::new(client_man),
      gateways,
      DisplaySettingsStore::new(db),
      config,
    )
    .await
    .spawn();
    Rig {
      display,
      sent,
      _actor: actor,
    }
  }

  async fn rig(root: &Path) -> Rig {
    rig_with(root, crate::db::open(None).await.expect("db"), 0.15).await
  }

  async fn set_dim(display: &DisplayManager, dim: DimSettings) -> SetResult {
    display
      .set_settings(DisplaySettings {
        dim,
        ..display.settings()
      })
      .await
  }

  fn dimming_to(level: f32) -> DimSettings {
    DimSettings { after_s: None, level }
  }

  fn timed(after_s: u32) -> DimSettings {
    DimSettings {
      after_s: Some(after_s),
      ..DimSettings::default()
    }
  }

  async fn host_away(display: &DisplayManager, away: bool) {
    display.link_up(HOST).await;
    let applied = display
      .host_state(
        HOST,
        HostState {
          away: Some(away),
          idle: false,
        },
      )
      .await
      .expect("the display actor is up");
    assert!(applied, "a linked host's report is taken in");
  }

  async fn manual(display: &DisplayManager, level: f32) {
    display
      .set_mode(BrightnessMode::Manual)
      .await
      .expect("mode switch")
      .expect("valid");
    display
      .set_level(level)
      .await
      .expect("level write")
      .expect("manual level");
  }

  async fn dimmed(display: &DisplayManager, want: bool) -> bool {
    for _ in 0..400 {
      if display.snapshot().brightness.dimmed == want {
        return true;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
  }

  async fn ambient_reaches(display: &DisplayManager, level: u8) -> bool {
    for _ in 0..400 {
      if display.snapshot().ambient_level == level {
        return true;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
  }

  fn watches(sent: &mut GatewaySendRx) -> Vec<(Option<Address>, HostWatch)> {
    let mut seen = Vec::new();
    while let Ok(out) = sent.try_recv() {
      if let BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::HostWatch(watch)) = &out.msg.data {
        seen.push((out.address, *watch));
      }
    }
    seen
  }

  fn announced(sent: &mut GatewaySendRx) -> Vec<(Option<Address>, DisplaySettings)> {
    let mut seen = Vec::new();
    while let Ok(out) = sent.try_recv() {
      if let BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::DisplaySettingsChanged(settings)) = &out.msg.data
      {
        seen.push((out.address, *settings));
      }
    }
    seen
  }

  #[tokio::test]
  async fn every_settings_change_is_announced_to_all_gateways_and_nothing_else_is() {
    let root = scratch("display-test-announce");
    let _backlight = backlight_at(&root, 255);
    std::fs::write(root.join("in_intensity0_raw"), "1500\n").expect("sensor at full scale");
    let mut rig = rig(&root).await;
    assert!(ambient_reaches(&rig.display, 100).await);
    assert!(announced(&mut rig.sent).is_empty(), "the light sensor is not a setting");

    manual(&rig.display, 0.5).await;
    let after_level = DisplaySettings {
      mode: BrightnessMode::Manual,
      level: 0.5,
      dim: DimSettings::default(),
    };
    assert_eq!(
      announced(&mut rig.sent),
      vec![
        (
          None,
          DisplaySettings {
            mode: BrightnessMode::Manual,
            level: 1.0,
            dim: DimSettings::default(),
          }
        ),
        (None, after_level),
      ]
    );

    set_dim(&rig.display, dimming_to(0.0))
      .await
      .expect("actor")
      .expect("valid");
    let after_dim = DisplaySettings {
      dim: dimming_to(0.0),
      ..after_level
    };
    assert_eq!(announced(&mut rig.sent), vec![(None, after_dim)]);

    let all = DisplaySettings {
      mode: BrightnessMode::Auto,
      level: 0.3,
      dim: DimSettings {
        after_s: Some(60),
        ..dimming_to(0.4)
      },
    };
    rig.display.set_settings(all).await.expect("actor").expect("valid");
    assert_eq!(announced(&mut rig.sent), vec![(None, all)]);
    assert_eq!(rig.display.settings(), all);

    rig.display.set_settings(all).await.expect("actor").expect("unchanged");
    assert!(
      announced(&mut rig.sent).is_empty(),
      "an unchanged set announces nothing"
    );
  }

  #[tokio::test]
  async fn a_settings_set_the_device_cannot_honor_changes_nothing_at_all() {
    let root = scratch("display-test-invalid-settings");
    let backlight = backlight_at(&root, 255);
    let mut rig = rig(&root).await;
    let bad = DisplaySettings {
      mode: BrightnessMode::Manual,
      level: 0.1,
      dim: DimSettings {
        after_s: Some(5),
        ..DimSettings::default()
      },
    };
    assert_eq!(
      rig.display.set_settings(bad).await.expect("actor"),
      Err(HardwareError::DimTimerOutOfRange)
    );
    assert_eq!(rig.display.settings(), DisplaySettings::default());
    assert_eq!(
      panel(&backlight),
      255,
      "the valid half of a refused set never reaches the panel"
    );
    assert!(announced(&mut rig.sent).is_empty());
  }

  #[tokio::test]
  async fn a_level_set_in_auto_mode_is_kept_and_reported_as_a_mode_mismatch() {
    let root = scratch("display-test-auto-level");
    let _backlight = backlight_at(&root, 255);
    let rig = rig(&root).await;
    assert_eq!(
      rig.display.set_level(0.4).await.expect("actor"),
      Err(HardwareError::ModeMismatch)
    );
    assert!((rig.display.settings().level - 0.4).abs() < f32::EPSILON);
  }

  #[tokio::test]
  async fn a_backlight_write_that_never_landed_is_reported_rather_than_announced() {
    let root = scratch("display-test-absent-backlight");
    let rig = rig(&root).await;
    let mode = rig.display.set_mode(BrightnessMode::Manual).await;
    assert!(
      matches!(mode, Err(DisplayError::Io(_))),
      "a mode switch whose backlight write failed cannot report success: {mode:?}"
    );
    assert_eq!(rig.display.snapshot().brightness.mode, BrightnessMode::Auto);
  }

  #[tokio::test]
  async fn a_failed_backlight_write_leaves_the_prior_brightness_in_place() {
    let root = scratch("display-test-write-failure");
    let backlight = backlight_at(&root, 128);
    let rig = rig(&root).await;
    manual(&rig.display, 0.5).await;
    let settled = rig.display.snapshot().brightness;

    std::fs::remove_file(backlight.join("brightness")).expect("unlink brightness");
    std::fs::create_dir(backlight.join("brightness")).expect("shadow brightness");

    let failed = rig.display.set_level(0.9).await;
    assert!(matches!(failed, Err(DisplayError::Io(_))), "{failed:?}");
    let after = rig.display.snapshot().brightness;
    assert_eq!(
      after.level, settled.level,
      "the daemon reports a brightness the panel never took"
    );
    assert_eq!(after.effective_level, settled.effective_level);
  }

  #[tokio::test]
  async fn brightness_and_dim_settings_survive_a_restart() {
    let root = scratch("display-test-persist");
    let backlight = backlight_at(&root, 255);
    let db_path = root.join("bridgething.db");
    let chosen = DimSettings {
      after_s: Some(600),
      level: 0.0,
    };
    {
      let rig = rig_with(&root, crate::db::open(Some(&db_path)).await.expect("db"), 0.15).await;
      manual(&rig.display, 0.42).await;
      set_dim(&rig.display, chosen).await.expect("actor").expect("valid");
    }
    std::fs::write(backlight.join("actual_brightness"), "255\n").expect("actual_brightness");
    std::fs::write(backlight.join("brightness"), "255\n").expect("brightness");

    let rig = rig_with(&root, crate::db::open(Some(&db_path)).await.expect("db"), 0.15).await;
    let state = rig.display.snapshot();
    assert_eq!(state.brightness.mode, BrightnessMode::Manual);
    assert!((state.brightness.level - 0.42).abs() < f32::EPSILON);
    assert_eq!(state.dim, chosen);
    assert_eq!(
      panel(&backlight),
      107,
      "the panel itself is restored to the manual level"
    );
  }

  #[tokio::test]
  async fn a_setting_the_device_cannot_honor_changes_nothing() {
    let root = scratch("display-test-invalid-dim");
    let rig = rig(&root).await;
    for (bad, error) in [
      (
        DimSettings {
          after_s: Some(14),
          ..DimSettings::default()
        },
        HardwareError::DimTimerOutOfRange,
      ),
      (
        DimSettings {
          level: 1.5,
          ..DimSettings::default()
        },
        HardwareError::LevelOutOfRange,
      ),
    ] {
      assert_eq!(set_dim(&rig.display, bad).await.expect("actor"), Err(error));
    }
    assert_eq!(rig.display.snapshot().dim, DimSettings::default());
  }

  #[tokio::test]
  async fn a_dimmed_manual_screen_is_capped_and_snaps_back_when_undimmed() {
    let root = scratch("display-test-dim-manual");
    let backlight = backlight_at(&root, 255);
    let rig = rig_with(&root, crate::db::open(None).await.expect("db"), 0.001).await;
    manual(&rig.display, 0.8).await;
    assert_eq!(panel(&backlight), 204);

    set_dim(&rig.display, dimming_to(0.25))
      .await
      .expect("actor")
      .expect("valid");
    host_away(&rig.display, true).await;
    assert!(dimmed(&rig.display, true).await);
    assert!(
      settles_at(&backlight, 64, Duration::from_secs(5)).await,
      "dims to the cap: {}",
      panel(&backlight)
    );

    rig
      .display
      .set_level(0.1)
      .await
      .expect("level write")
      .expect("manual level");
    assert_eq!(panel(&backlight), 26, "a level under the cap shows as chosen");
    manual(&rig.display, 0.8).await;
    assert_eq!(panel(&backlight), 64, "a level over the cap is held at it");

    let applied = rig
      .display
      .host_state(
        HOST,
        HostState {
          away: Some(false),
          idle: false,
        },
      )
      .await
      .expect("the display actor is up");
    assert!(applied);
    assert!(
      settles_at(&backlight, 204, Duration::from_millis(100)).await,
      "undimming lights the panel at once rather than easing up: {}",
      panel(&backlight)
    );
    assert!(!rig.display.snapshot().brightness.dimmed);
  }

  #[tokio::test]
  async fn a_dimmed_auto_screen_scales_what_the_sensor_asks_for() {
    let root = scratch("display-test-dim-auto");
    let backlight = backlight_at(&root, 255);
    std::fs::write(root.join("in_intensity0_raw"), "1500\n").expect("sensor at full scale");
    let rig = rig(&root).await;
    assert!(settles_at(&backlight, 255, Duration::from_secs(2)).await);

    set_dim(&rig.display, dimming_to(0.5))
      .await
      .expect("actor")
      .expect("valid");
    host_away(&rig.display, true).await;
    assert!(
      settles_at(&backlight, 128, Duration::from_secs(2)).await,
      "half of what the sensor asks for: {}",
      panel(&backlight)
    );

    std::fs::write(root.join("in_intensity0_raw"), "0\n").expect("sensor in the dark");
    assert!(
      settles_at(&backlight, 8, Duration::from_secs(2)).await,
      "half of the dark room floor: {}",
      panel(&backlight)
    );
  }

  #[tokio::test]
  async fn room_light_follows_the_sensor_and_dimming_does_not_move_it() {
    let root = scratch("display-test-ambient");
    let backlight = backlight_at(&root, 255);
    std::fs::write(root.join("in_intensity0_raw"), "1500\n").expect("sensor at full scale");
    let rig = rig(&root).await;
    assert!(
      ambient_reaches(&rig.display, 100).await,
      "a full-scale reading is a bright room"
    );

    set_dim(&rig.display, dimming_to(0.5))
      .await
      .expect("actor")
      .expect("valid");
    host_away(&rig.display, true).await;
    assert!(settles_at(&backlight, 128, Duration::from_secs(2)).await);
    assert_eq!(
      rig.display.snapshot().ambient_level,
      100,
      "a dimmed panel is not a darker room"
    );

    rig
      .display
      .set_mode(BrightnessMode::Manual)
      .await
      .expect("mode switch")
      .expect("valid");
    rig
      .display
      .set_level(0.1)
      .await
      .expect("level write")
      .expect("manual level");
    assert_eq!(rig.display.snapshot().ambient_level, 100, "nor is a low manual level");

    std::fs::write(root.join("in_intensity0_raw"), "0\n").expect("sensor in the dark");
    assert!(ambient_reaches(&rig.display, 0).await, "a dark room reads as zero");
  }

  #[tokio::test]
  async fn a_dim_level_of_zero_goes_dark() {
    let root = scratch("display-test-dim-dark");
    let backlight = backlight_at(&root, 255);
    let rig = rig(&root).await;
    manual(&rig.display, 0.5).await;
    set_dim(&rig.display, dimming_to(0.0))
      .await
      .expect("actor")
      .expect("valid");
    host_away(&rig.display, true).await;
    assert!(
      settles_at(&backlight, 0, Duration::from_secs(2)).await,
      "{}",
      panel(&backlight)
    );
    let held = rig.display.snapshot().brightness;
    assert!(
      (held.level - 0.5).abs() < f32::EPSILON,
      "dimming does not rewrite the chosen level"
    );
    rig.display.note_input().await;
    assert!(settles_at(&backlight, 128, Duration::from_millis(100)).await);
  }

  #[tokio::test]
  async fn a_host_report_is_answered_once_the_screen_already_follows_it() {
    let root = scratch("display-test-host-ack");
    backlight_at(&root, 200);
    let rig = rig(&root).await;
    let away = HostState {
      away: Some(true),
      idle: false,
    };

    assert!(
      !rig
        .display
        .host_state(HOST, away)
        .await
        .expect("the display actor is up"),
      "a report from a gateway with no link is not taken in"
    );
    assert!(!rig.display.snapshot().brightness.dimmed);

    host_away(&rig.display, true).await;
    assert!(
      rig.display.snapshot().brightness.dimmed,
      "the answer comes after the dim decision, not before it"
    );
  }

  #[tokio::test]
  async fn a_gateway_hears_the_watch_on_link_up_only_while_a_timer_is_set_and_on_every_change() {
    let root = scratch("display-test-watch");
    let mut rig = rig(&root).await;
    rig.display.link_up(HOST).await;
    set_dim(&rig.display, dimming_to(0.6))
      .await
      .expect("actor")
      .expect("valid");
    assert!(
      watches(&mut rig.sent).is_empty(),
      "without a timer nothing is asked of a host"
    );

    set_dim(&rig.display, timed(300)).await.expect("actor").expect("valid");
    assert_eq!(
      watches(&mut rig.sent),
      vec![(
        Some(HOST),
        HostWatch {
          idle_after_s: Some(300)
        }
      )]
    );

    set_dim(
      &rig.display,
      DimSettings {
        level: 0.0,
        ..timed(300)
      },
    )
    .await
    .expect("actor")
    .expect("valid");
    assert!(
      watches(&mut rig.sent).is_empty(),
      "the dim level is not the host's business"
    );

    rig.display.link_up(OTHER).await;
    set_dim(&rig.display, rig.display.snapshot().dim)
      .await
      .expect("actor")
      .expect("an unchanged setting is accepted");
    assert_eq!(
      watches(&mut rig.sent),
      vec![(
        Some(OTHER),
        HostWatch {
          idle_after_s: Some(300)
        }
      )],
      "a link that comes up while a timer is set hears it at once"
    );

    set_dim(&rig.display, DimSettings::default())
      .await
      .expect("actor")
      .expect("valid");
    let mut sent = watches(&mut rig.sent);
    sent.sort_by_key(|(addr, _)| addr.map(|a| a.0));
    assert_eq!(
      sent,
      vec![(Some(HOST), HostWatch::default()), (Some(OTHER), HostWatch::default())],
      "clearing the timer tells every linked gateway to stop idle reports"
    );
  }
}
