use std::{
  io,
  path::{Path, PathBuf},
};

use libbridgething::OtaKind;
use tokio::fs;

const DAEMON_DIR: &str = "/opt/bridgething/daemon";
const DAEMON_INCOMING: &str = "bridgething.incoming";
const WEBAPPS_DIR: &str = "/opt/bridgething/webapps";

#[derive(Debug, Clone)]
pub(crate) struct StagePaths {
  pub incoming: PathBuf,
  pub current: PathBuf,
  pub previous: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct StagedPiece {
  pub kind: OtaKind,
  pub update_id: String,
  pub paths: Option<StagePaths>,
}

pub(crate) async fn commit(piece: &StagedPiece) -> io::Result<()> {
  let Some(paths) = &piece.paths else {
    return Ok(());
  };
  if fs::try_exists(&paths.current).await? {
    fs::rename(&paths.current, &paths.previous).await?;
  }
  fs::rename(&paths.incoming, &paths.current).await?;
  Ok(())
}

pub(crate) async fn rollback(piece: &StagedPiece) -> io::Result<()> {
  let Some(paths) = &piece.paths else {
    return Ok(());
  };
  if fs::try_exists(&paths.current).await? {
    fs::rename(&paths.current, &paths.incoming).await?;
  }
  if fs::try_exists(&paths.previous).await? {
    fs::rename(&paths.previous, &paths.current).await?;
  }
  Ok(())
}

pub(crate) async fn discard(piece: &StagedPiece) {
  if let Some(paths) = &piece.paths {
    remove_any(&paths.incoming).await;
  }
}

pub(crate) async fn remove_any(path: &Path) {
  if let Ok(meta) = fs::symlink_metadata(path).await {
    let _ = if meta.is_dir() {
      fs::remove_dir_all(path).await
    } else {
      fs::remove_file(path).await
    };
  }
}

pub(crate) async fn sweep_orphans() {
  if !crate::paths::is_on_device() {
    return;
  }
  remove_any(&PathBuf::from(DAEMON_DIR).join(DAEMON_INCOMING)).await;
  sweep_prefixed(&PathBuf::from(DAEMON_DIR), &["bridgething.broken."]).await;
  sweep_prefixed(&PathBuf::from(WEBAPPS_DIR), &[".incoming.", ".tmp.", ".previous."]).await;
  for name in ["hub", "browser", "stock"] {
    remove_any(&PathBuf::from(WEBAPPS_DIR).join(format!("{name}.previous"))).await;
  }
}

async fn sweep_prefixed(dir: &Path, prefixes: &[&str]) {
  let Ok(mut rd) = fs::read_dir(dir).await else {
    return;
  };
  while let Ok(Some(entry)) = rd.next_entry().await {
    let name = entry.file_name();
    let name = name.to_string_lossy();
    if prefixes.iter().any(|prefix| name.starts_with(prefix)) {
      remove_any(&entry.path()).await;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  async fn write_file(path: &Path, body: &str) {
    fs::write(path, body).await.unwrap();
  }

  async fn read_file(path: &Path) -> String {
    fs::read_to_string(path).await.unwrap()
  }

  fn temp_root() -> PathBuf {
    let p = std::env::temp_dir().join(format!("bridgething-staging-test-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(&p).unwrap();
    p
  }

  fn piece(root: &Path, name: &str) -> StagedPiece {
    StagedPiece {
      kind: OtaKind::Daemon,
      update_id: name.to_string(),
      paths: Some(StagePaths {
        incoming: root.join(format!("{name}.incoming")),
        current: root.join(name),
        previous: root.join(format!("{name}.previous")),
      }),
    }
  }

  #[tokio::test]
  async fn commit_rotates_current_to_previous_and_promotes_incoming() {
    let root = temp_root();
    let p = piece(&root, "daemon");
    let paths = p.paths.clone().unwrap();
    write_file(&paths.current, "old").await;
    write_file(&paths.incoming, "new").await;

    commit(&p).await.unwrap();

    assert_eq!(read_file(&paths.current).await, "new");
    assert_eq!(read_file(&paths.previous).await, "old");
    assert!(!fs::try_exists(&paths.incoming).await.unwrap());
  }

  #[tokio::test]
  async fn commit_with_no_existing_current_just_promotes() {
    let root = temp_root();
    let p = piece(&root, "daemon");
    let paths = p.paths.clone().unwrap();
    write_file(&paths.incoming, "new").await;

    commit(&p).await.unwrap();

    assert_eq!(read_file(&paths.current).await, "new");
    assert!(!fs::try_exists(&paths.previous).await.unwrap());
  }

  #[tokio::test]
  async fn rollback_restores_previous_and_parks_new_in_incoming() {
    let root = temp_root();
    let p = piece(&root, "daemon");
    let paths = p.paths.clone().unwrap();
    write_file(&paths.current, "old").await;
    write_file(&paths.incoming, "new").await;
    commit(&p).await.unwrap();

    rollback(&p).await.unwrap();

    assert_eq!(read_file(&paths.current).await, "old");
    assert_eq!(read_file(&paths.incoming).await, "new");
    assert!(!fs::try_exists(&paths.previous).await.unwrap());

    discard(&p).await;
    assert!(!fs::try_exists(&paths.incoming).await.unwrap());
  }

  #[tokio::test]
  async fn sweep_prefixed_removes_only_matching_entries() {
    let root = temp_root();
    write_file(&root.join("bridgething.broken.20260810T203357Z"), "carcass").await;
    write_file(&root.join("bridgething.broken.20260812T053758Z"), "carcass").await;
    write_file(&root.join("bridgething.current"), "live").await;
    write_file(&root.join("bridgething.previous"), "rollback").await;

    sweep_prefixed(&root, &["bridgething.broken."]).await;

    assert!(
      !fs::try_exists(&root.join("bridgething.broken.20260810T203357Z"))
        .await
        .unwrap()
    );
    assert!(
      !fs::try_exists(&root.join("bridgething.broken.20260812T053758Z"))
        .await
        .unwrap()
    );
    assert!(fs::try_exists(&root.join("bridgething.current")).await.unwrap());
    assert!(fs::try_exists(&root.join("bridgething.previous")).await.unwrap());
  }

  #[tokio::test]
  async fn commit_and_rollback_noop_when_paths_none() {
    let p = StagedPiece {
      kind: OtaKind::Daemon,
      update_id: "x".into(),
      paths: None,
    };
    commit(&p).await.unwrap();
    rollback(&p).await.unwrap();
    discard(&p).await;
  }
}
