use std::{
  io,
  os::unix::fs::PermissionsExt,
  path::{Path, PathBuf},
};

use libbridgething::OtaKind;
use tokio::fs;

use super::staging::{self, StagePaths, StagedPiece};
use crate::paths::{ON_DEVICE_SENTINEL, is_on_device};

const DAEMON_DIR: &str = "/opt/bridgething/daemon";
const CURRENT_NAME: &str = "bridgething.current";
const PREVIOUS_NAME: &str = "bridgething.previous";
const INCOMING_NAME: &str = "bridgething.incoming";
const RUNNING_VERSION_NAME: &str = ".running-version";

#[derive(Debug, thiserror::Error)]
pub enum SwapError {
  #[error("io error during {step}: {source}")]
  Io {
    step: &'static str,
    #[source]
    source: io::Error,
  },
}

fn io_err(step: &'static str) -> impl Fn(io::Error) -> SwapError {
  move |source| SwapError::Io { step, source }
}

pub fn current_binary_path() -> PathBuf {
  PathBuf::from(DAEMON_DIR).join(CURRENT_NAME)
}

pub fn patch_source_path() -> PathBuf {
  if is_on_device() {
    return current_binary_path();
  }
  std::env::current_exe().unwrap_or_else(|_| current_binary_path())
}

fn running_version_path() -> PathBuf {
  PathBuf::from(DAEMON_DIR).join(RUNNING_VERSION_NAME)
}

pub async fn record_running_version() {
  if !is_on_device() {
    return;
  }

  let path = running_version_path();
  if let Err(err) = write_running_version(&path).await {
    tracing::warn!(path = %path.display(), %err, "could not stamp the running daemon version; an image ota may floor this daemon");
  }
}

async fn write_running_version(path: &Path) -> io::Result<()> {
  fs::write(path, format!("{}\n", env!("CARGO_PKG_VERSION"))).await
}

pub async fn stage(staged_binary: &Path, update_id: String) -> Result<StagedPiece, SwapError> {
  if !is_on_device() {
    tracing::warn!("daemon stage requested but {ON_DEVICE_SENTINEL} is missing - no-op (off-device safety gate)");
    return Ok(StagedPiece {
      kind: OtaKind::Daemon,
      update_id,
      paths: None,
    });
  }

  let daemon_dir = PathBuf::from(DAEMON_DIR);
  let current = current_binary_path();
  let previous = daemon_dir.join(PREVIOUS_NAME);
  let incoming = daemon_dir.join(INCOMING_NAME);

  fs::create_dir_all(&daemon_dir)
    .await
    .map_err(io_err("mkdir daemon dir"))?;

  staging::remove_any(&incoming).await;
  staging::remove_any(&previous).await;

  fs::copy(staged_binary, &incoming)
    .await
    .map_err(io_err("copy staged -> incoming"))?;

  fs::set_permissions(&incoming, std::fs::Permissions::from_mode(0o755))
    .await
    .map_err(io_err("chmod incoming"))?;
  sync_file(&incoming).await?;

  tracing::info!("daemon binary staged at {}", incoming.display());
  Ok(StagedPiece {
    kind: OtaKind::Daemon,
    update_id,
    paths: Some(StagePaths {
      incoming,
      current,
      previous,
    }),
  })
}

async fn sync_file(path: &Path) -> Result<(), SwapError> {
  let f = fs::OpenOptions::new()
    .read(true)
    .open(path)
    .await
    .map_err(io_err("open incoming for fsync"))?;
  f.sync_all().await.map_err(io_err("fsync incoming"))?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn the_stamp_is_the_bare_semver_the_recipe_pins() {
    let dir = std::env::temp_dir().join(format!("bridgething-running-version-{}", uuid::Uuid::now_v7().simple()));
    fs::create_dir_all(&dir).await.unwrap();
    let path = dir.join(RUNNING_VERSION_NAME);

    write_running_version(&path).await.unwrap();

    let stamped = fs::read_to_string(&path).await.unwrap();
    assert_eq!(stamped.trim(), env!("CARGO_PKG_VERSION"));
    assert!(!stamped.starts_with('v'), "stamped {stamped:?}");
  }
}
