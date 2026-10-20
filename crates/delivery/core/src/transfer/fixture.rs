use std::sync::{
  Arc, Mutex,
  atomic::{AtomicU64, Ordering},
};

use bridgething_sdk_runtime::rt::Instant;
use libbridgething::gateway::TransferAck;

use super::{AckSink, FragmentSource};
use crate::seam::Clock;

pub struct TestClock {
  base: Instant,
  nanos: AtomicU64,
}

impl TestClock {
  pub fn new() -> Arc<Self> {
    Arc::new(Self {
      base: Instant::now(),
      nanos: AtomicU64::new(0),
    })
  }

  pub fn advance(&self, duration: std::time::Duration) {
    self.nanos.fetch_add(duration.as_nanos() as u64, Ordering::SeqCst);
  }

  pub fn advance_secs(&self, seconds: f64) {
    self.nanos.fetch_add((seconds * 1e9) as u64, Ordering::SeqCst);
  }
}

impl Clock for TestClock {
  fn now(&self) -> Instant {
    self.base + std::time::Duration::from_nanos(self.nanos.load(Ordering::SeqCst))
  }

  fn unix_millis(&self) -> u64 {
    self.nanos.load(Ordering::SeqCst) / 1_000_000
  }
}

pub struct RecordingAcks {
  acks: Mutex<Vec<TransferAck>>,
}

impl RecordingAcks {
  pub fn new() -> Arc<Self> {
    Arc::new(Self {
      acks: Mutex::new(Vec::new()),
    })
  }

  pub fn received(&self) -> Vec<u32> {
    self.acks.lock().unwrap().iter().map(|ack| ack.received).collect()
  }

  pub fn all(&self) -> Vec<TransferAck> {
    self.acks.lock().unwrap().clone()
  }
}

impl AckSink for RecordingAcks {
  fn ack(&self, ack: TransferAck) {
    self.acks.lock().unwrap().push(ack);
  }
}

pub struct SliceSource {
  bytes: Vec<u8>,
  short_read_at: Option<u64>,
}

impl SliceSource {
  pub fn new(bytes: Vec<u8>) -> Self {
    Self {
      bytes,
      short_read_at: None,
    }
  }

  pub fn truncated_at(bytes: Vec<u8>, offset: u64) -> Self {
    Self {
      bytes,
      short_read_at: Some(offset),
    }
  }
}

impl FragmentSource for SliceSource {
  fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
    if self.short_read_at.is_some_and(|at| offset >= at) {
      return Ok(0);
    }
    let start = offset as usize;
    if start >= self.bytes.len() {
      return Ok(0);
    }
    let end = (start + buf.len()).min(self.bytes.len());
    buf[..end - start].copy_from_slice(&self.bytes[start..end]);
    Ok(end - start)
  }
}

pub struct EndlessSource;

impl FragmentSource for EndlessSource {
  fn read_at(&self, _offset: u64, buf: &mut [u8]) -> Result<usize, String> {
    buf.fill(0x5a);
    Ok(buf.len())
  }
}

pub fn ramp(len: usize) -> Vec<u8> {
  (0..len).map(|i| (i % 251) as u8).collect()
}
