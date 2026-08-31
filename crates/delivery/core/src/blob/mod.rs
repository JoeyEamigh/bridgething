use std::{
  fs,
  path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

mod slots;

pub use slots::FsSlotIndex;

use crate::seam::BlobStore;

pub fn digest_of(bytes: &[u8]) -> String {
  hex::encode(Sha256::digest(bytes))
}

pub fn is_digest(digest: &str) -> bool {
  digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub struct FsBlobStore {
  root: PathBuf,
}

impl FsBlobStore {
  pub fn new(root: impl Into<PathBuf>) -> Self {
    Self { root: root.into() }
  }

  pub fn path_of(&self, digest: &str) -> Option<PathBuf> {
    is_digest(digest).then(|| self.root.join(digest))
  }

  fn located(&self, digest: &str) -> Result<PathBuf, String> {
    self
      .path_of(digest)
      .ok_or_else(|| format!("{digest} is not a sha256 digest"))
  }
}

impl BlobStore for FsBlobStore {
  fn contains(&self, digest: &str) -> bool {
    self.located(digest).map(|path| path.is_file()).unwrap_or(false)
  }

  fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, String> {
    let path = self.located(digest)?;
    match fs::read(&path) {
      Ok(bytes) => Ok(Some(bytes)),
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
      Err(err) => Err(err.to_string()),
    }
  }

  fn put(&self, digest: &str, bytes: &[u8]) -> Result<(), String> {
    let path = self.located(digest)?;
    if path.is_file() {
      return Ok(());
    }
    fs::create_dir_all(&self.root).map_err(|err| err.to_string())?;
    let staged = self.root.join(format!("{digest}.{}.part", uuid::Uuid::now_v7()));
    write_then_rename(&staged, &path, bytes).inspect_err(|_| {
      let _ = fs::remove_file(&staged);
    })
  }

  fn remove(&self, digest: &str) -> Result<(), String> {
    let path = self.located(digest)?;
    match fs::remove_file(&path) {
      Ok(()) => Ok(()),
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
      Err(err) => Err(err.to_string()),
    }
  }
}

fn write_then_rename(staged: &Path, dest: &Path, bytes: &[u8]) -> Result<(), String> {
  fs::write(staged, bytes).map_err(|err| err.to_string())?;
  fs::rename(staged, dest).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_put_round_trips_and_repeats_without_error() {
    let root = tempfile::tempdir().expect("a scratch directory");
    let store = FsBlobStore::new(root.path());
    let body = b"one more time".to_vec();
    let digest = digest_of(&body);

    assert!(!store.contains(&digest));
    store.put(&digest, &body).expect("the blob stores");
    store.put(&digest, &body).expect("storing it again is a no-op");

    assert!(store.contains(&digest));
    assert_eq!(store.get(&digest).expect("readable"), Some(body));
  }

  #[test]
  fn a_missing_blob_reads_as_absent_rather_than_failing() {
    let root = tempfile::tempdir().expect("a scratch directory");
    let store = FsBlobStore::new(root.path());
    let digest = digest_of(b"never stored");

    assert_eq!(store.get(&digest).expect("absent is not an error"), None);
    store.remove(&digest).expect("removing what is not there is a no-op");
  }

  #[test]
  fn a_key_that_is_not_a_digest_is_refused_rather_than_resolved() {
    let root = tempfile::tempdir().expect("a scratch directory");
    let store = FsBlobStore::new(root.path());

    assert!(store.get("../escape").is_err());
    assert!(store.put("../escape", b"no").is_err());
    assert!(!store.contains("../escape"));
  }
}
