use std::{
  fs::{File, Metadata},
  io::{self, Read},
  path::Path,
};

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
  hex::encode(Sha256::digest(bytes))
}

pub async fn sha256_file(path: &Path) -> io::Result<String> {
  Ok(sha256_file_as_read(path).await?.0)
}

pub async fn sha256_file_as_read(path: &Path) -> io::Result<(String, Metadata)> {
  let path = path.to_owned();
  tokio::task::spawn_blocking(move || {
    let mut file = File::open(&path)?;
    let digest = hash(&mut file)?;
    Ok((digest, file.metadata()?))
  })
  .await
  .map_err(io::Error::other)?
}

fn hash(file: &mut File) -> io::Result<String> {
  let mut hasher = Sha256::new();
  let mut buf = vec![0u8; 64 * 1024];
  loop {
    let read = file.read(&mut buf)?;
    if read == 0 {
      return Ok(hex::encode(hasher.finalize()));
    }
    hasher.update(&buf[..read]);
  }
}
