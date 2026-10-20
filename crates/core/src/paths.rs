use std::{
  ffi::OsString,
  io,
  path::{Path, PathBuf},
};

use tokio::{
  fs::{File, OpenOptions},
  io::AsyncRead,
};
use uuid::Uuid;

pub const ON_DEVICE_SENTINEL: &str = "/etc/superbird";

const ENV_STATE_DIR: &str = "BRIDGETHING_STATE_DIR";
const ENV_WEBAPPS_DIR: &str = "BRIDGETHING_WEBAPPS_DIR";
const ENV_WEBAPP_BLOBS_DIR: &str = "BRIDGETHING_WEBAPP_BLOBS_DIR";
const ENV_RO_WEBAPPS_DIR: &str = "BRIDGETHING_RO_WEBAPPS_DIR";
const ENV_EXAMPLES_DIR: &str = "BRIDGETHING_EXAMPLES_DIR";
const ENV_WAKEWORD_MODEL: &str = "BRIDGETHING_WAKEWORD_MODEL";

#[cfg(not(debug_assertions))]
const PROD_STATE_DIR: &str = "/var/lib/bridgething/state";
#[cfg(not(debug_assertions))]
const PROD_WEBAPPS_DIR: &str = "/var/bridgething/webapps";
#[cfg(not(debug_assertions))]
const PROD_WEBAPP_BLOBS_DIR: &str = "/var/bridgething/webapp-blobs";
#[cfg(not(debug_assertions))]
const PROD_RO_WEBAPPS_DIR: &str = "/opt/bridgething/webapps";
#[cfg(not(debug_assertions))]
const PROD_EXAMPLES_DIR: &str = "/opt/bridgething/examples";
#[cfg(not(debug_assertions))]
const PROD_WAKEWORD_DIR: &str = "/var/bridgething/wakeword";
#[cfg(not(debug_assertions))]
const PROD_WAKEWORD_BASELINE_DIR: &str = "/usr/share/bridgething/wakeword";

pub fn state_dir() -> PathBuf {
  if let Ok(p) = std::env::var(ENV_STATE_DIR) {
    return PathBuf::from(p);
  }

  #[cfg(debug_assertions)]
  {
    dirs::config_dir()
      .unwrap_or_else(|| PathBuf::from("/tmp"))
      .join("bridgething")
      .join("state")
  }

  #[cfg(not(debug_assertions))]
  PathBuf::from(PROD_STATE_DIR)
}

pub fn webapps_dir() -> PathBuf {
  if let Ok(p) = std::env::var(ENV_WEBAPPS_DIR) {
    return PathBuf::from(p);
  }

  #[cfg(debug_assertions)]
  {
    dirs::data_dir()
      .unwrap_or_else(|| PathBuf::from("/tmp"))
      .join("bridgething")
      .join("webapps")
  }

  #[cfg(not(debug_assertions))]
  PathBuf::from(PROD_WEBAPPS_DIR)
}

pub fn webapp_blobs_dir() -> PathBuf {
  if let Ok(p) = std::env::var(ENV_WEBAPP_BLOBS_DIR) {
    return PathBuf::from(p);
  }

  #[cfg(debug_assertions)]
  {
    dirs::data_dir()
      .unwrap_or_else(|| PathBuf::from("/tmp"))
      .join("bridgething")
      .join("webapp-blobs")
  }

  #[cfg(not(debug_assertions))]
  PathBuf::from(PROD_WEBAPP_BLOBS_DIR)
}

pub fn ro_webapps_dir() -> PathBuf {
  if let Ok(p) = std::env::var(ENV_RO_WEBAPPS_DIR) {
    return PathBuf::from(p);
  }

  #[cfg(debug_assertions)]
  {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
      .join("../../")
      .join("packages/webapps/builtin/hub")
  }

  #[cfg(not(debug_assertions))]
  PathBuf::from(PROD_RO_WEBAPPS_DIR)
}

pub fn examples_dir() -> PathBuf {
  if let Ok(p) = std::env::var(ENV_EXAMPLES_DIR) {
    return PathBuf::from(p);
  }

  #[cfg(debug_assertions)]
  {
    dirs::data_dir()
      .unwrap_or_else(|| PathBuf::from("/tmp"))
      .join("bridgething")
      .join("examples")
  }

  #[cfg(not(debug_assertions))]
  PathBuf::from(PROD_EXAMPLES_DIR)
}

pub fn is_on_device() -> bool {
  Path::new(ON_DEVICE_SENTINEL).exists()
}

pub fn transfers_dir() -> PathBuf {
  state_dir().join("transfers")
}

pub fn blob_transfers_dir() -> PathBuf {
  state_dir().join("blob-transfers")
}

pub fn bandaid_transfers_dir() -> PathBuf {
  PathBuf::from("/opt/bridgething/.transfers")
}

#[derive(Debug, Clone, Copy)]
pub struct PartitionBytes {
  pub total: u64,
  pub free: u64,
}

pub fn partition_bytes(path: &Path) -> Option<PartitionBytes> {
  use std::os::unix::ffi::OsStrExt;
  let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
  // SAFETY: statvfs takes a NUL-terminated path and writes a POSIX struct into our stack allocation; both pointers are valid for the duration of the call
  let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
  let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
  if rc != 0 {
    return None;
  }
  let block = widen(stat.f_frsize);
  Some(PartitionBytes {
    total: widen(stat.f_blocks).saturating_mul(block),
    free: widen(stat.f_bavail).saturating_mul(block),
  })
}

pub fn partition_free_bytes(path: &Path) -> u64 {
  partition_bytes(path).map_or(u64::MAX, |bytes| bytes.free)
}

fn widen<T: Into<u64>>(value: T) -> u64 {
  value.into()
}

pub async fn fsync(path: &Path) -> io::Result<()> {
  File::open(path).await?.sync_all().await
}

pub async fn fsync_entry(path: &Path) -> io::Result<()> {
  fsync(path).await?;
  fsync(containing_dir(path)).await
}

pub async fn rename_or_copy(source: &Path, dest: &Path) -> io::Result<()> {
  fsync(source).await?;
  match tokio::fs::rename(source, dest).await {
    Ok(()) => {}
    Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
      write_into_place(File::open(source).await?, dest).await?;
      tokio::fs::remove_file(source).await.ok();
    }
    Err(err) => return Err(err),
  }
  fsync(containing_dir(dest)).await
}

async fn write_into_place(mut content: impl AsyncRead + Unpin, dest: &Path) -> io::Result<()> {
  let staged = staging_path(dest);
  let written = async {
    let mut file = OpenOptions::new().write(true).create_new(true).open(&staged).await?;
    tokio::io::copy(&mut content, &mut file).await?;
    file.sync_all().await?;
    tokio::fs::rename(&staged, dest).await
  }
  .await;
  if written.is_err() {
    tokio::fs::remove_file(&staged).await.ok();
  }
  written
}

fn staging_path(dest: &Path) -> PathBuf {
  let mut name = OsString::from(".");
  name.push(dest.file_name().unwrap_or_default());
  name.push(format!(".{}.staged", Uuid::now_v7().simple()));
  dest.with_file_name(name)
}

fn containing_dir(path: &Path) -> &Path {
  match path.parent() {
    Some(parent) if !parent.as_os_str().is_empty() => parent,
    _ => Path::new("."),
  }
}

pub fn assets_blobs_dir() -> PathBuf {
  state_dir().join("assets")
}

pub fn wakeword_models() -> Vec<PathBuf> {
  if let Ok(p) = std::env::var(ENV_WAKEWORD_MODEL) {
    return vec![PathBuf::from(p)];
  }

  #[cfg(debug_assertions)]
  {
    vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wakeword/models/hey_bridgething.btww")]
  }

  #[cfg(not(debug_assertions))]
  vec![
    PathBuf::from(PROD_WAKEWORD_DIR).join("hey_bridgething.btww"),
    PathBuf::from(PROD_WAKEWORD_BASELINE_DIR).join("hey_bridgething.btww"),
  ]
}

#[cfg(test)]
mod tests {
  use std::{
    pin::Pin,
    task::{Context, Poll},
  };

  use tokio::io::ReadBuf;

  use super::*;

  struct FailsAfter {
    prefix: &'static [u8],
    served: bool,
  }

  impl AsyncRead for FailsAfter {
    fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
      if self.served {
        return Poll::Ready(Err(io::Error::other("the source died mid-copy")));
      }
      self.served = true;
      buf.put_slice(self.prefix);
      Poll::Ready(Ok(()))
    }
  }

  fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bridgething-paths-test-{}", Uuid::now_v7().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
  }

  fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
      .unwrap()
      .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
      .collect();
    names.sort();
    names
  }

  #[tokio::test]
  async fn rename_or_copy_moves_the_source_into_place() {
    let dir = temp_dir();
    let source = dir.join("source.partial");
    let dest_dir = dir.join("store");
    std::fs::create_dir_all(&dest_dir).unwrap();
    let dest = dest_dir.join("blob");
    std::fs::write(&source, b"payload").unwrap();

    rename_or_copy(&source, &dest).await.unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
    assert!(!source.exists());
    assert_eq!(names_in(&dest_dir), vec!["blob"]);
  }

  #[tokio::test]
  async fn rename_or_copy_reports_a_rename_failure_and_keeps_the_source() {
    let dir = temp_dir();
    let source = dir.join("source.partial");
    std::fs::write(&source, b"payload").unwrap();
    let dest = dir.join("missing").join("blob");

    let err = rename_or_copy(&source, &dest).await.unwrap_err();

    assert_eq!(err.kind(), io::ErrorKind::NotFound);
    assert_eq!(std::fs::read(&source).unwrap(), b"payload");
  }

  #[tokio::test]
  async fn a_copy_lands_whole_and_leaves_nothing_else_behind() {
    let dir = temp_dir();
    let dest = dir.join("blob");

    write_into_place(&b"payload"[..], &dest).await.unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
    assert_eq!(names_in(&dir), vec!["blob"]);
  }

  #[tokio::test]
  async fn a_copy_that_dies_midway_never_shows_at_the_destination() {
    let dir = temp_dir();
    let dest = dir.join("blob");
    let content = FailsAfter {
      prefix: b"half of the pay",
      served: false,
    };

    write_into_place(content, &dest).await.unwrap_err();

    assert!(names_in(&dir).is_empty(), "left behind: {:?}", names_in(&dir));
  }

  #[tokio::test]
  async fn a_copy_that_dies_midway_keeps_what_the_destination_held() {
    let dir = temp_dir();
    let dest = dir.join("blob");
    std::fs::write(&dest, b"the earlier whole payload").unwrap();
    let content = FailsAfter {
      prefix: b"half",
      served: false,
    };

    write_into_place(content, &dest).await.unwrap_err();

    assert_eq!(std::fs::read(&dest).unwrap(), b"the earlier whole payload");
    assert_eq!(names_in(&dir), vec!["blob"]);
  }

  #[tokio::test]
  async fn fsync_reaches_files_and_directories() {
    let dir = temp_dir();
    let file = dir.join("blob");
    std::fs::write(&file, b"payload").unwrap();

    fsync(&file).await.unwrap();
    fsync(&dir).await.unwrap();
    fsync_entry(&file).await.unwrap();
    assert_eq!(containing_dir(&file), dir);
    assert_eq!(containing_dir(Path::new("blob")), Path::new("."));
    fsync(&dir.join("absent")).await.unwrap_err();
  }
}
