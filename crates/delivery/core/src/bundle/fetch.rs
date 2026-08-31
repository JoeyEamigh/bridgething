use std::{
  io::{Read, Write},
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
};

use bridgething_io::{DownloadBody, HttpError, HttpExecutor, HttpHeader, HttpMethod, HttpRequest};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::bundle::ArtifactDigest;

pub const MEMORY_BODY_CAP: usize = 256 * 1024;

const READ_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestField {
  Size,
  Sha256,
}

impl std::fmt::Display for DigestField {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(match self {
      DigestField::Size => "size",
      DigestField::Sha256 => "sha256",
    })
  }
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
  #[error("{0}")]
  Transport(String),
  #[error("fetch returned HTTP {0}")]
  HttpStatus(u16),
  #[error("{asset} {field} does not match the manifest; refusing to install")]
  DigestMismatch { asset: String, field: DigestField },
  #[error("response body exceeds the {limit} byte memory cap")]
  TooLarge { limit: usize },
  #[error("{0}")]
  Decode(String),
  #[error("{0}")]
  Io(String),
}

#[derive(Clone)]
pub struct DownloadRequest {
  pub url: String,
  pub dir: PathBuf,
  pub filename: String,
  pub asset: String,
  pub expected: Option<ArtifactDigest>,
  pub progress: Option<Arc<dyn Fn(u64, u64) + Send + Sync>>,
}

#[async_trait::async_trait]
pub trait ArtifactFetch: Send + Sync {
  async fn text(&self, url: &str) -> Result<String, FetchError>;
  async fn download(&self, request: DownloadRequest) -> Result<PathBuf, FetchError>;
}

pub async fn fetch_json<T: DeserializeOwned>(fetch: &dyn ArtifactFetch, url: &str) -> Result<T, FetchError> {
  let body = fetch.text(url).await?;
  serde_json::from_str(&body).map_err(|e| FetchError::Decode(e.to_string()))
}

pub struct HttpArtifactFetch {
  http: HttpExecutor,
}

impl HttpArtifactFetch {
  pub fn new(http: HttpExecutor) -> Self {
    HttpArtifactFetch { http }
  }
}

#[async_trait::async_trait]
impl ArtifactFetch for HttpArtifactFetch {
  async fn text(&self, url: &str) -> Result<String, FetchError> {
    let buffer = Arc::new(Mutex::new(MemoryBody::default()));
    let outcome = self
      .http
      .download(get(url), Box::new(MemoryWriter { shared: buffer.clone() }))
      .await;

    let held = buffer.lock().unwrap();
    match outcome {
      Ok(outcome) if !outcome.ok() => Err(FetchError::HttpStatus(outcome.status)),
      Ok(_) => Ok(String::from_utf8_lossy(&held.bytes).into_owned()),
      Err(_) if held.overflowed => Err(FetchError::TooLarge { limit: MEMORY_BODY_CAP }),
      Err(e) => Err(transport(e)),
    }
  }

  async fn download(&self, request: DownloadRequest) -> Result<PathBuf, FetchError> {
    std::fs::create_dir_all(&request.dir).map_err(io)?;
    let cache_name = match &request.expected {
      Some(digest) => format!("{}-{}", request.filename, digest.sha256),
      None => request.filename.clone(),
    };
    let target = request.dir.join(&cache_name);

    if let Ok(meta) = std::fs::metadata(&target) {
      let reusable = match &request.expected {
        Some(digest) => meta.len() == digest.size,
        None => meta.len() > 0,
      };
      if reusable {
        return Ok(target);
      }
      let _ = std::fs::remove_file(&target);
    }

    let staging = request.dir.join(format!("{cache_name}.download"));
    match self.spool(&request, &staging, &target).await {
      Ok(()) => Ok(target),
      Err(e) => {
        let _ = std::fs::remove_file(&staging);
        Err(e)
      }
    }
  }
}

impl HttpArtifactFetch {
  async fn spool(&self, request: &DownloadRequest, staging: &Path, target: &Path) -> Result<(), FetchError> {
    let total = request.expected.as_ref().map(|d| d.size).unwrap_or(0);
    let file = std::fs::File::create(staging).map_err(io)?;
    let outcome = self
      .http
      .download(
        get(&request.url),
        Box::new(SpoolWriter {
          file,
          received: 0,
          total,
          progress: request.progress.clone(),
        }),
      )
      .await
      .map_err(transport)?;

    if !outcome.ok() {
      return Err(FetchError::HttpStatus(outcome.status));
    }

    if let Some(expected) = &request.expected {
      let landed = std::fs::metadata(staging).map_err(io)?.len();
      if landed != expected.size {
        return Err(FetchError::DigestMismatch {
          asset: request.asset.clone(),
          field: DigestField::Size,
        });
      }
      if sha256_file(staging)? != expected.sha256 {
        return Err(FetchError::DigestMismatch {
          asset: request.asset.clone(),
          field: DigestField::Sha256,
        });
      }
    }

    if target.exists() {
      std::fs::remove_file(target).map_err(io)?;
    }
    std::fs::rename(staging, target).map_err(io)?;
    Ok(())
  }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
  hex::encode(Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> Result<String, FetchError> {
  let mut file = std::fs::File::open(path).map_err(io)?;
  let mut hasher = Sha256::new();
  let mut buffer = vec![0u8; READ_CHUNK];
  loop {
    let read = file.read(&mut buffer).map_err(io)?;
    if read == 0 {
      break;
    }
    hasher.update(&buffer[..read]);
  }
  Ok(hex::encode(hasher.finalize()))
}

fn get(url: &str) -> HttpRequest {
  HttpRequest {
    method: HttpMethod::Get,
    url: url.to_string(),
    headers: Vec::new(),
    body: Vec::new(),
    timeout_ms: 0,
  }
}

fn io(e: std::io::Error) -> FetchError {
  FetchError::Io(e.to_string())
}

fn transport(e: HttpError) -> FetchError {
  match e {
    HttpError::Body(reason) => FetchError::Io(reason),
    other => FetchError::Transport(other.to_string()),
  }
}

#[derive(Default)]
struct MemoryBody {
  bytes: Vec<u8>,
  overflowed: bool,
}

struct MemoryWriter {
  shared: Arc<Mutex<MemoryBody>>,
}

impl DownloadBody for MemoryWriter {
  fn on_response(&mut self, status: u16, _headers: &[HttpHeader], _content_length: Option<u64>) -> bool {
    (200..300).contains(&status)
  }

  fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
    let mut held = self.shared.lock().unwrap();
    if held.bytes.len() + chunk.len() > MEMORY_BODY_CAP {
      held.overflowed = true;
      return Err(format!("response body exceeds the {MEMORY_BODY_CAP} byte memory cap"));
    }
    held.bytes.extend_from_slice(chunk);
    Ok(())
  }
}

struct SpoolWriter {
  file: std::fs::File,
  received: u64,
  total: u64,
  progress: Option<Arc<dyn Fn(u64, u64) + Send + Sync>>,
}

impl DownloadBody for SpoolWriter {
  fn on_response(&mut self, status: u16, _headers: &[HttpHeader], _content_length: Option<u64>) -> bool {
    (200..300).contains(&status)
  }

  fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
    if self.total > 0 && self.received + chunk.len() as u64 > self.total {
      return Err(format!("body ran past the declared {} bytes", self.total));
    }
    self.file.write_all(chunk).map_err(|e| e.to_string())?;
    self.received += chunk.len() as u64;
    if let Some(progress) = &self.progress {
      progress(self.received, self.total);
    }
    Ok(())
  }
}
