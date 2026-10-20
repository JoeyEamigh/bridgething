use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::Mutex;

use crate::{
  paths::{ON_DEVICE_SENTINEL, is_on_device},
  uboot_env,
};

const FACTORY_RESET_FLAG: &str = "factory_reset";
const FACTORY_RESET_ARMED: &str = "1";
#[cfg(feature = "systemd")]
const WIPE_UNIT: &str = "superbird-factory-reset.service";

pub type TerminatorFn = Arc<dyn Fn() + Send + Sync + 'static>;
pub type RigError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetSupport {
  Ready,
  NoWipeStep,
  OffDevice,
}

pub trait ResetRig: Send + Sync + 'static {
  fn support(&self) -> BoxFuture<'_, Result<ResetSupport, RigError>>;
  fn armed(&self) -> BoxFuture<'_, Result<bool, RigError>>;
  fn arm(&self) -> BoxFuture<'_, Result<(), RigError>>;
  fn disarm(&self) -> BoxFuture<'_, Result<(), RigError>>;
  fn reboot(&self) -> BoxFuture<'_, Result<(), RigError>>;
}

#[derive(Debug, thiserror::Error)]
pub enum FactoryResetError {
  #[error("factory reset only runs on a device")]
  OffDevice,
  #[error("this system image cannot erase the device; update the system image, then reset again")]
  NoWipeStep,
  #[error("could not confirm the system image can erase the device, nothing was erased: {0}")]
  Probe(#[source] RigError),
  #[error("could not arm the reset, nothing was erased: {0}")]
  Arm(#[source] RigError),
  #[error("the device did not restart, nothing was erased: {0}")]
  Reboot(#[source] RigError),
  #[error("the device did not restart ({reboot}) and the reset is still armed ({disarm}); the next restart erases it")]
  StillArmed { reboot: RigError, disarm: RigError },
}

pub struct FactoryReset {
  rig: Arc<dyn ResetRig>,
  underway: Mutex<bool>,
}

impl FactoryReset {
  pub fn new(rig: Arc<dyn ResetRig>) -> Self {
    Self {
      rig,
      underway: Mutex::new(false),
    }
  }

  pub async fn disarm_stale(&self) {
    match self.rig.armed().await {
      Ok(false) => {}
      Ok(true) => match self.rig.disarm().await {
        Ok(()) => tracing::warn!("a factory reset was armed and no boot step erased the device; called it off"),
        Err(err) => tracing::error!("a stale factory reset is armed and could not be called off: {err}"),
      },
      Err(err) => tracing::warn!("could not read the factory reset flag: {err}"),
    }
  }

  pub async fn run(&self) -> Result<(), FactoryResetError> {
    let mut underway = self.underway.lock().await;
    if !*underway {
      match self.rig.support().await.map_err(FactoryResetError::Probe)? {
        ResetSupport::Ready => {}
        ResetSupport::NoWipeStep => return Err(FactoryResetError::NoWipeStep),
        ResetSupport::OffDevice => return Err(FactoryResetError::OffDevice),
      }
      self.rig.arm().await.map_err(FactoryResetError::Arm)?;
    }
    if let Err(reboot) = self.rig.reboot().await {
      *underway = false;
      return Err(match self.rig.disarm().await {
        Ok(()) => FactoryResetError::Reboot(reboot),
        Err(disarm) => FactoryResetError::StillArmed { reboot, disarm },
      });
    }
    tracing::info!("data partition wipe armed; rebooting");
    *underway = true;
    Ok(())
  }
}

pub struct DeviceRig;

impl ResetRig for DeviceRig {
  fn support(&self) -> BoxFuture<'_, Result<ResetSupport, RigError>> {
    Box::pin(async {
      if !is_on_device() {
        return Ok(ResetSupport::OffDevice);
      }
      Ok(if wipe_unit_enabled().await? {
        ResetSupport::Ready
      } else {
        ResetSupport::NoWipeStep
      })
    })
  }

  fn armed(&self) -> BoxFuture<'_, Result<bool, RigError>> {
    Box::pin(async {
      if !is_on_device() {
        return Ok(false);
      }
      Ok(uboot_env::read(FACTORY_RESET_FLAG).await?.trim() == FACTORY_RESET_ARMED)
    })
  }

  fn arm(&self) -> BoxFuture<'_, Result<(), RigError>> {
    Box::pin(async { Ok(uboot_env::set(FACTORY_RESET_FLAG, FACTORY_RESET_ARMED).await?) })
  }

  fn disarm(&self) -> BoxFuture<'_, Result<(), RigError>> {
    Box::pin(async { Ok(uboot_env::unset(FACTORY_RESET_FLAG).await?) })
  }

  fn reboot(&self) -> BoxFuture<'_, Result<(), RigError>> {
    Box::pin(async { Ok(invoke_reboot().await?) })
  }
}

#[cfg(feature = "systemd")]
#[zbus::proxy(
  interface = "org.freedesktop.systemd1.Manager",
  default_service = "org.freedesktop.systemd1",
  default_path = "/org/freedesktop/systemd1"
)]
trait SystemdManager {
  fn reboot(&self) -> zbus::Result<()>;
  fn power_off(&self) -> zbus::Result<()>;
  fn reset_failed_unit(&self, name: &str) -> zbus::Result<()>;
  fn restart_unit(&self, name: &str, mode: &str) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
  fn load_unit(&self, name: &str) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

#[cfg(feature = "systemd")]
#[zbus::proxy(
  interface = "org.freedesktop.systemd1.Unit",
  default_service = "org.freedesktop.systemd1"
)]
trait SystemdUnit {
  #[zbus(property)]
  fn load_state(&self) -> zbus::Result<String>;
  #[zbus(property)]
  fn unit_file_state(&self) -> zbus::Result<String>;
}

#[derive(Debug, thiserror::Error)]
pub enum PowerError {
  #[cfg(feature = "systemd")]
  #[error("systemd dbus call failed: {0}")]
  Dbus(#[from] zbus::Error),
  #[cfg(not(feature = "systemd"))]
  #[error("systemd cargo feature disabled; power control unavailable")]
  Disabled,
}

pub async fn reboot() -> Result<(), PowerError> {
  if !is_on_device() {
    tracing::warn!("reboot requested but {ON_DEVICE_SENTINEL} is missing - no-op (off-device safety gate)");
    return Ok(());
  }
  invoke_reboot().await
}

pub async fn power_off() -> Result<(), PowerError> {
  if !is_on_device() {
    tracing::warn!("power_off requested but {ON_DEVICE_SENTINEL} is missing - no-op (off-device safety gate)");
    return Ok(());
  }
  invoke_power_off().await
}

pub async fn restart_self() -> Result<(), PowerError> {
  if !is_on_device() {
    tracing::warn!("restart_self requested but {ON_DEVICE_SENTINEL} is missing - no-op (off-device safety gate)");
    return Ok(());
  }
  invoke_restart_self().await
}

#[cfg(feature = "systemd")]
async fn invoke_reboot() -> Result<(), PowerError> {
  let conn = zbus::Connection::system().await?;
  let proxy = SystemdManagerProxy::new(&conn).await?;
  proxy.reboot().await?;
  Ok(())
}

#[cfg(feature = "systemd")]
async fn wipe_unit_enabled() -> Result<bool, PowerError> {
  let conn = zbus::Connection::system().await?;
  let manager = SystemdManagerProxy::new(&conn).await?;
  let path = manager.load_unit(WIPE_UNIT).await?;
  let unit = SystemdUnitProxy::builder(&conn)
    .path(path)?
    .cache_properties(zbus::proxy::CacheProperties::No)
    .build()
    .await?;
  Ok(unit.load_state().await? == "loaded" && unit.unit_file_state().await? == "enabled")
}

#[cfg(feature = "systemd")]
async fn invoke_power_off() -> Result<(), PowerError> {
  let conn = zbus::Connection::system().await?;
  let proxy = SystemdManagerProxy::new(&conn).await?;
  proxy.power_off().await?;
  Ok(())
}

#[cfg(feature = "systemd")]
async fn invoke_restart_self() -> Result<(), PowerError> {
  let conn = zbus::Connection::system().await?;
  let proxy = SystemdManagerProxy::new(&conn).await?;
  if let Err(err) = proxy.reset_failed_unit("bridgething.service").await {
    tracing::warn!(
      ?err,
      "reset-failed bridgething.service before restart failed (continuing)"
    );
  }
  proxy.restart_unit("bridgething.service", "replace").await?;
  Ok(())
}

#[cfg(not(feature = "systemd"))]
async fn invoke_reboot() -> Result<(), PowerError> {
  Err(PowerError::Disabled)
}

#[cfg(not(feature = "systemd"))]
async fn wipe_unit_enabled() -> Result<bool, PowerError> {
  Err(PowerError::Disabled)
}

#[cfg(not(feature = "systemd"))]
async fn invoke_power_off() -> Result<(), PowerError> {
  Err(PowerError::Disabled)
}

#[cfg(not(feature = "systemd"))]
async fn invoke_restart_self() -> Result<(), PowerError> {
  Err(PowerError::Disabled)
}
