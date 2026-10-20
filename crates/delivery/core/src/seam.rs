use std::path::Path;

use bridgething_sdk_runtime::rt::Instant;

pub trait Clock: Send + Sync {
  fn now(&self) -> Instant;
  fn unix_millis(&self) -> u64;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
  fn now(&self) -> Instant {
    bridgething_sdk_runtime::rt::now()
  }

  fn unix_millis(&self) -> u64 {
    bridgething_sdk_runtime::rt::unix_millis()
  }
}

pub trait BlobStore: Send + Sync {
  fn contains(&self, digest: &str) -> bool;
  fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, String>;
  fn put(&self, digest: &str, bytes: &[u8]) -> Result<(), String>;
  fn remove(&self, digest: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedResource {
  pub digest: String,
  pub mime: Option<String>,
}

pub trait SlotIndex: Send + Sync {
  fn get(&self, slot: &str) -> Option<CachedResource>;
  fn set(&self, slot: &str, resource: &CachedResource) -> Result<(), String>;
  fn remove(&self, slot: &str) -> Result<(), String>;
  fn entries(&self) -> Vec<(String, CachedResource)>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
  Trace,
  Debug,
  Info,
  Warn,
  Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
  NluModel,
  AsrModel,
}

pub trait ArtifactValidator: Send + Sync {
  fn validate(&self, kind: ArtifactKind, staged: &Path) -> Result<(), String>;
}

pub trait TransferPolicy: Send + Sync {
  fn allows_large_transfer(&self) -> bool;
}
