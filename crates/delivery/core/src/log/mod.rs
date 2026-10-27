#[cfg(not(target_arch = "wasm32"))]
pub mod store;

use std::{
  collections::VecDeque,
  sync::{Arc, Mutex},
};

use crate::seam::{Clock, LogLevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogOrigin {
  Device,
  Host,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLogRecord {
  pub seq: u64,
  pub ts_unix_ms: u64,
  pub origin: LogOrigin,
  pub level: LogLevel,
  pub target: String,
  pub message: String,
}

struct Held {
  records: VecDeque<DeviceLogRecord>,
  seq: u64,
}

pub struct DeviceLogRing {
  capacity: usize,
  clock: Arc<dyn Clock>,
  held: Mutex<Held>,
}

impl DeviceLogRing {
  pub fn new(capacity: usize, clock: Arc<dyn Clock>) -> Self {
    Self {
      capacity,
      clock,
      held: Mutex::new(Held {
        records: VecDeque::new(),
        seq: 0,
      }),
    }
  }

  pub fn push(&self, origin: LogOrigin, level: LogLevel, target: &str, message: &str) -> DeviceLogRecord {
    self.push_at(origin, level, self.clock.unix_millis(), target, message)
  }

  pub fn push_at(
    &self,
    origin: LogOrigin,
    level: LogLevel,
    ts_unix_ms: u64,
    target: &str,
    message: &str,
  ) -> DeviceLogRecord {
    let mut held = self.held.lock().unwrap();
    held.seq += 1;
    let record = DeviceLogRecord {
      seq: held.seq,
      ts_unix_ms,
      origin,
      level,
      target: target.to_owned(),
      message: message.to_owned(),
    };
    held.records.push_back(record.clone());
    while held.records.len() > self.capacity {
      held.records.pop_front();
    }
    record
  }

  pub fn tail(&self, limit: usize) -> Vec<DeviceLogRecord> {
    let held = self.held.lock().unwrap();
    let skip = held.records.len().saturating_sub(limit);
    held.records.iter().skip(skip).cloned().collect()
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use super::*;
  use crate::seam::SystemClock;

  fn ring(capacity: usize) -> DeviceLogRing {
    DeviceLogRing::new(capacity, Arc::new(SystemClock))
  }

  #[test]
  fn the_tail_is_bounded_and_keeps_the_newest() {
    let ring = ring(3);
    for n in 0..5 {
      ring.push(LogOrigin::Host, LogLevel::Info, "t", &format!("line {n}"));
    }
    let tail = ring.tail(10);
    assert_eq!(
      tail.iter().map(|record| record.message.as_str()).collect::<Vec<_>>(),
      vec!["line 2", "line 3", "line 4"]
    );
    assert_eq!(tail.iter().map(|record| record.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
  }

  #[test]
  fn a_limited_tail_returns_the_newest_oldest_first() {
    let ring = ring(10);
    ring.push(LogOrigin::Device, LogLevel::Warn, "daemon", "first");
    ring.push(LogOrigin::Host, LogLevel::Debug, "logcat", "second");
    let tail = ring.tail(1);
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].message, "second");
    assert_eq!(tail[0].origin, LogOrigin::Host);
  }

  #[test]
  fn both_origins_interleave_in_arrival_order() {
    let ring = ring(10);
    ring.push(LogOrigin::Device, LogLevel::Info, "daemon", "device line");
    ring.push(LogOrigin::Host, LogLevel::Info, "logcat", "host line");
    ring.push(LogOrigin::Device, LogLevel::Info, "daemon", "another");
    let origins: Vec<LogOrigin> = ring.tail(10).into_iter().map(|record| record.origin).collect();
    assert_eq!(origins, vec![LogOrigin::Device, LogOrigin::Host, LogOrigin::Device]);
  }
}
