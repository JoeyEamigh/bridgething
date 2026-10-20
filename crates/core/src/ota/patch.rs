use std::{
  io::{self, Write},
  path::{Path, PathBuf},
};

use libbridgething::gateway::{OtaPatch, OtaPatchAlgorithm};
use sha2::{Digest, Sha256};
use tokio::task;

use crate::digest::sha256_hex;

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
  #[error("io error during {step}: {source}")]
  Io {
    step: &'static str,
    #[source]
    source: io::Error,
  },
  #[error("zstd patch apply failed: {0}")]
  Apply(String),
  #[error("reconstructed artifact is {got} bytes, expected {expected}")]
  SizeMismatch { got: u64, expected: u64 },
  #[error("reconstructed artifact sha256 {got} != expected {expected}")]
  HashMismatch { got: String, expected: String },
  #[error("patch source sha256 {got} != expected {expected}; local binary is not what the patch was cut against")]
  SourceMismatch { got: String, expected: String },
}

pub async fn apply(source: PathBuf, patch: PathBuf, spec: OtaPatch) -> Result<PathBuf, PatchError> {
  let out = patch.with_extension("reconstructed");
  let job_out = out.clone();
  let expect_source = spec.source_sha256.clone();
  let prefixed = matches!(spec.algorithm, OtaPatchAlgorithm::ZstdPatchFrom);
  let result = task::spawn_blocking(move || {
    let source = prefixed.then_some(source);
    reconstruct(source.as_deref(), &patch, &job_out, expect_source.as_deref())
  })
  .await
  .map_err(|e| PatchError::Apply(format!("apply task join: {e}")))?;

  let (len, sha) = match result {
    Ok(v) => v,
    Err(err) => {
      let _ = tokio::fs::remove_file(&out).await;
      return Err(err);
    }
  };

  if len != spec.result_size as u64 {
    let _ = tokio::fs::remove_file(&out).await;
    return Err(PatchError::SizeMismatch {
      got: len,
      expected: spec.result_size as u64,
    });
  }
  if !sha.eq_ignore_ascii_case(&spec.result_sha256) {
    let _ = tokio::fs::remove_file(&out).await;
    return Err(PatchError::HashMismatch {
      got: sha,
      expected: spec.result_sha256,
    });
  }
  Ok(out)
}

fn reconstruct(
  source: Option<&Path>,
  patch: &Path,
  out: &Path,
  expect_source: Option<&str>,
) -> Result<(u64, String), PatchError> {
  let src_map = match source {
    Some(path) => {
      let src_file = std::fs::File::open(path).map_err(|source| PatchError::Io {
        step: "open source",
        source,
      })?;
      Some(
        unsafe { memmap2::Mmap::map(&src_file) }.map_err(|source| PatchError::Io {
          step: "mmap source",
          source,
        })?,
      )
    }
    None => None,
  };

  if let (Some(expected), Some(map)) = (expect_source, src_map.as_ref()) {
    let got = sha256_hex(map);
    if !got.eq_ignore_ascii_case(expected) {
      return Err(PatchError::SourceMismatch {
        got,
        expected: expected.to_string(),
      });
    }
  }

  let patch_file = std::fs::File::open(patch).map_err(|source| PatchError::Io {
    step: "open patch",
    source,
  })?;
  let reader = io::BufReader::new(patch_file);
  match src_map.as_ref() {
    Some(map) => {
      let mut decoder = zstd::stream::read::Decoder::with_ref_prefix(reader, map)
        .map_err(|e| PatchError::Apply(format!("init decoder: {e}")))?;
      decoder
        .window_log_max(31)
        .map_err(|e| PatchError::Apply(format!("window_log_max: {e}")))?;
      decode_into(decoder, out)
    }
    None => {
      let mut decoder =
        zstd::stream::read::Decoder::new(reader).map_err(|e| PatchError::Apply(format!("init decoder: {e}")))?;
      decoder
        .window_log_max(31)
        .map_err(|e| PatchError::Apply(format!("window_log_max: {e}")))?;
      decode_into(decoder, out)
    }
  }
}

fn decode_into<R: io::Read>(mut decoder: R, out: &Path) -> Result<(u64, String), PatchError> {
  let out_file = std::fs::File::create(out).map_err(|source| PatchError::Io {
    step: "create reconstruction",
    source,
  })?;
  let mut sink = HashingWriter {
    inner: io::BufWriter::new(out_file),
    hasher: Sha256::new(),
    len: 0,
  };
  io::copy(&mut decoder, &mut sink).map_err(|e| PatchError::Apply(format!("decode: {e}")))?;
  sink.inner.flush().map_err(|source| PatchError::Io {
    step: "flush reconstruction",
    source,
  })?;
  Ok((sink.len, hex::encode(sink.hasher.finalize())))
}

struct HashingWriter<W: Write> {
  inner: W,
  hasher: Sha256,
  len: u64,
}

impl<W: Write> Write for HashingWriter<W> {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    let n = self.inner.write(buf)?;
    self.hasher.update(&buf[..n]);
    self.len += n as u64;
    Ok(n)
  }

  fn flush(&mut self) -> io::Result<()> {
    self.inner.flush()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn temp_root() -> PathBuf {
    let p = std::env::temp_dir().join(format!("bridgething-patch-test-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&p).unwrap();
    p
  }

  fn make_patch(old: &[u8], new: &[u8]) -> Vec<u8> {
    let mut encoder = zstd::stream::write::Encoder::with_ref_prefix(Vec::new(), 19, old).unwrap();
    encoder
      .set_parameter(zstd::zstd_safe::CParameter::EnableLongDistanceMatching(true))
      .unwrap();
    encoder
      .set_parameter(zstd::zstd_safe::CParameter::WindowLog(27))
      .unwrap();
    encoder.write_all(new).unwrap();
    encoder.finish().unwrap()
  }

  #[tokio::test]
  async fn apply_reconstructs_and_verifies() {
    let root = temp_root();
    let old: Vec<u8> = (0u32..40_000).map(|i| (i * 7) as u8).collect();
    let mut new = old.clone();
    for i in (0..new.len()).step_by(97) {
      new[i] = new[i].wrapping_add(1);
    }
    new.extend_from_slice(b"tail bytes only in the new build");

    let source = root.join("bridgething.current");
    std::fs::write(&source, &old).unwrap();
    let patch = root.join("delta.zst");
    std::fs::write(&patch, make_patch(&old, &new)).unwrap();

    let spec = OtaPatch {
      algorithm: OtaPatchAlgorithm::ZstdPatchFrom,
      result_sha256: sha256_hex(&new),
      result_size: new.len() as u32,
      source_sha256: Some(sha256_hex(&old)),
    };
    let out = apply(source, patch, spec).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), new);
  }

  #[tokio::test]
  async fn wrong_source_fails_hash_and_discards() {
    let root = temp_root();
    let old: Vec<u8> = (0u32..20_000).map(|i| i as u8).collect();
    let new: Vec<u8> = (0u32..20_000).map(|i| (i + 3) as u8).collect();
    let patch_bytes = make_patch(&old, &new);

    let stale: Vec<u8> = (0u32..20_000).map(|i| (i + 99) as u8).collect();
    let source = root.join("bridgething.current");
    std::fs::write(&source, &stale).unwrap();
    let patch = root.join("delta.zst");
    std::fs::write(&patch, &patch_bytes).unwrap();

    let spec = OtaPatch {
      algorithm: OtaPatchAlgorithm::ZstdPatchFrom,
      result_sha256: sha256_hex(&new),
      result_size: new.len() as u32,
      source_sha256: None,
    };
    let err = apply(source, patch.clone(), spec).await.unwrap_err();
    assert!(matches!(err, PatchError::HashMismatch { .. }));
    assert!(!patch.with_extension("reconstructed").exists());
  }

  #[tokio::test]
  async fn plain_zstd_needs_no_source() {
    let root = temp_root();
    let new: Vec<u8> = (0u32..50_000).map(|i| (i * 13) as u8).collect();
    let patch = root.join("full.zst");
    std::fs::write(&patch, zstd::encode_all(new.as_slice(), 19).unwrap()).unwrap();

    let spec = OtaPatch {
      algorithm: OtaPatchAlgorithm::Zstd,
      result_sha256: sha256_hex(&new),
      result_size: new.len() as u32,
      source_sha256: None,
    };
    let out = apply(root.join("does-not-exist"), patch, spec).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), new);
  }

  #[tokio::test]
  async fn declared_source_mismatch_refuses_before_decoding() {
    let root = temp_root();
    let old: Vec<u8> = (0u32..20_000).map(|i| i as u8).collect();
    let new: Vec<u8> = (0u32..20_000).map(|i| (i + 3) as u8).collect();

    let stale: Vec<u8> = (0u32..20_000).map(|i| (i + 99) as u8).collect();
    let source = root.join("bridgething.current");
    std::fs::write(&source, &stale).unwrap();
    let patch = root.join("delta.zst");
    std::fs::write(&patch, make_patch(&old, &new)).unwrap();

    let spec = OtaPatch {
      algorithm: OtaPatchAlgorithm::ZstdPatchFrom,
      result_sha256: sha256_hex(&new),
      result_size: new.len() as u32,
      source_sha256: Some(sha256_hex(&old)),
    };
    let err = apply(source, patch.clone(), spec).await.unwrap_err();
    match err {
      PatchError::SourceMismatch { got, expected } => {
        assert_eq!(got, sha256_hex(&stale));
        assert_eq!(expected, sha256_hex(&old));
      }
      other => panic!("expected SourceMismatch, got {other:?}"),
    }
    assert!(!patch.with_extension("reconstructed").exists());
  }
}
