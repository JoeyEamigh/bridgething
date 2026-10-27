use libbridgething::{RangePart, RangeSpec};
use tokio_util::bytes::Bytes;

use super::cache::{CacheIndex, CacheSeg};

pub(super) const MULTIPART_BOUNDARY: &str = "bridgething-ota-range-boundary";

pub(super) fn multipart_part_header(part: &RangePart, total: u32) -> String {
  format!(
    "\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\nContent-Range: bytes {start}-{end}/{total}\r\n\r\n",
    boundary = MULTIPART_BOUNDARY,
    start = part.start,
    end = part.start + part.length - 1,
  )
}

fn multipart_closing() -> String {
  format!("\r\n--{MULTIPART_BOUNDARY}--\r\n")
}

enum Seg {
  Lit(Bytes),
  Data { zck_start: u32, len: u32 },
}

fn seg_len(seg: &Seg) -> u64 {
  match seg {
    Seg::Lit(b) => b.len() as u64,
    Seg::Data { len, .. } => *len as u64,
  }
}

pub(super) struct BodyLayout {
  segs: Vec<Seg>,
  pub total_body: u64,
  pub single: Option<RangePart>,
}

impl BodyLayout {
  pub(super) fn is_multipart(&self) -> bool {
    self.single.is_none()
  }
}

pub(super) fn build(parts: &[RangePart], total: u32) -> BodyLayout {
  if parts.len() == 1 {
    let p = parts[0];
    return BodyLayout {
      segs: vec![Seg::Data {
        zck_start: p.start,
        len: p.length,
      }],
      total_body: p.length as u64,
      single: Some(p),
    };
  }

  let mut segs = Vec::with_capacity(parts.len() * 2 + 1);
  for p in parts {
    segs.push(Seg::Lit(Bytes::from(multipart_part_header(p, total))));
    segs.push(Seg::Data {
      zck_start: p.start,
      len: p.length,
    });
  }
  segs.push(Seg::Lit(Bytes::from(multipart_closing())));
  let total_body = segs.iter().map(seg_len).sum();
  BodyLayout {
    segs,
    total_body,
    single: None,
  }
}

pub(super) enum EmitStep {
  Lit(Bytes),
  Cached { log_off: u64, len: u32 },
  Fetch(u32),
}

pub(super) struct EmitPlan {
  pub steps: Vec<EmitStep>,
  pub companion_ranges: Vec<RangeSpec>,
  pub companion_bytes: u64,
  pub cached_bytes: u64,
}

pub(super) fn plan_from(layout: &BodyLayout, start: u64, index: &CacheIndex) -> Option<EmitPlan> {
  if start > layout.total_body {
    return None;
  }
  let mut steps = Vec::new();
  let mut companion_ranges = Vec::new();
  let mut companion_bytes = 0u64;
  let mut cached_bytes = 0u64;
  let mut offset = 0u64;
  for seg in &layout.segs {
    let seg_end = offset + seg_len(seg);
    if seg_end <= start {
      offset = seg_end;
      continue;
    }
    let skip = start.saturating_sub(offset);
    match seg {
      Seg::Lit(bytes) => {
        let tail = bytes.slice(skip as usize..);
        if !tail.is_empty() {
          steps.push(EmitStep::Lit(tail));
        }
      }
      Seg::Data { zck_start, len } => {
        let remaining = (*len as u64 - skip) as u32;
        for piece in index.segments(*zck_start + skip as u32, remaining) {
          match piece {
            CacheSeg::Cached { log_off, len } => {
              steps.push(EmitStep::Cached { log_off, len });
              cached_bytes += len as u64;
            }
            CacheSeg::Missing { zck_start, len } => {
              companion_ranges.push(RangeSpec {
                start: zck_start,
                length: len,
              });
              steps.push(EmitStep::Fetch(len));
              companion_bytes += len as u64;
            }
          }
        }
      }
    }
    offset = seg_end;
  }
  Some(EmitPlan {
    steps,
    companion_ranges,
    companion_bytes,
    cached_bytes,
  })
}

#[cfg(test)]
mod tests {
  use tokio_util::bytes::BytesMut;

  use super::*;

  fn fake_byte(i: u32) -> u8 {
    (i % 251) as u8
  }

  fn gather(ranges: &[RangeSpec]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in ranges {
      for i in r.start..r.start + r.length {
        out.push(fake_byte(i));
      }
    }
    out
  }

  fn fake_log(index: &CacheIndex) -> Vec<u8> {
    let mut log = Vec::new();
    for (zck, log_off, len) in index.spans() {
      let end = log_off as usize + len as usize;
      if log.len() < end {
        log.resize(end, 0);
      }
      for i in 0..len {
        log[log_off as usize + i as usize] = fake_byte(zck + i);
      }
    }
    log
  }

  fn assemble(steps: &[EmitStep], companion: &[u8], log: &[u8]) -> Bytes {
    let total: usize = steps
      .iter()
      .map(|s| match s {
        EmitStep::Lit(b) => b.len(),
        EmitStep::Cached { len, .. } => *len as usize,
        EmitStep::Fetch(n) => *n as usize,
      })
      .sum();
    let mut out = BytesMut::with_capacity(total);
    let mut consumed = 0usize;
    for s in steps {
      match s {
        EmitStep::Lit(b) => out.extend_from_slice(b),
        EmitStep::Cached { log_off, len } => {
          out.extend_from_slice(&log[*log_off as usize..*log_off as usize + *len as usize]);
        }
        EmitStep::Fetch(n) => {
          let next = consumed + *n as usize;
          out.extend_from_slice(&companion[consumed..next]);
          consumed = next;
        }
      }
    }
    out.freeze()
  }

  fn assert_resumable_with(parts: &[RangePart], total: u32, index: &CacheIndex) {
    let layout = build(parts, total);
    let log = fake_log(index);
    let full_plan = plan_from(&layout, 0, index).expect("start 0 always valid");
    let full = assemble(&full_plan.steps, &gather(&full_plan.companion_ranges), &log);
    assert_eq!(full.len() as u64, layout.total_body, "full body length");
    assert_eq!(
      full_plan.companion_bytes + full_plan.cached_bytes,
      parts.iter().map(|p| p.length as u64).sum::<u64>()
    );

    for n in 0..=layout.total_body {
      let plan = plan_from(&layout, n, index).expect("in-range start");
      let got = assemble(&plan.steps, &gather(&plan.companion_ranges), &log);
      assert_eq!(
        &got[..],
        &full[n as usize..],
        "resume at offset {n} did not match the body suffix (parts={parts:?})"
      );
    }
    assert!(plan_from(&layout, layout.total_body + 1, index).is_none());
  }

  fn assert_resumable(parts: &[RangePart], total: u32) {
    assert_resumable_with(parts, total, &CacheIndex::default());
  }

  fn coverage(parts: &[RangePart], seed: u64) -> CacheIndex {
    let mut index = CacheIndex::default();
    let mut rng = seed;
    let mut log_off = 8u64;
    for p in parts {
      let mut pos = p.start;
      let end = p.start + p.length;
      let mut cached = seed.is_multiple_of(2);
      while pos < end {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let run = ((rng >> 33) % 7 + 1) as u32;
        let take = run.min(end - pos);
        if cached {
          index.insert(pos, log_off, take);
          log_off += take as u64 + 8;
        }
        pos += take;
        cached = !cached;
      }
    }
    index
  }

  #[test]
  fn single_range_resumes_at_every_offset() {
    assert_resumable(
      &[RangePart {
        start: 4096,
        length: 500,
      }],
      100_000,
    );
  }

  #[test]
  fn multipart_resumes_at_every_offset() {
    let parts = vec![
      RangePart { start: 100, length: 50 },
      RangePart {
        start: 1000,
        length: 300,
      },
      RangePart { start: 5000, length: 7 },
    ];
    assert_resumable(&parts, 100_000);
  }

  #[test]
  fn multipart_single_byte_parts_resume() {
    let parts = vec![
      RangePart { start: 0, length: 1 },
      RangePart { start: 9, length: 1 },
      RangePart { start: 42, length: 1 },
    ];
    assert_resumable(&parts, 64);
  }

  #[test]
  fn fresh_plan_requests_the_original_parts() {
    let parts = vec![
      RangePart { start: 100, length: 50 },
      RangePart {
        start: 1000,
        length: 300,
      },
    ];
    let layout = build(&parts, 100_000);
    let plan = plan_from(&layout, 0, &CacheIndex::default()).unwrap();
    assert_eq!(
      plan.companion_ranges,
      vec![
        RangeSpec { start: 100, length: 50 },
        RangeSpec {
          start: 1000,
          length: 300
        },
      ]
    );
    assert_eq!(plan.cached_bytes, 0);
  }

  #[test]
  fn a_fully_cached_plan_asks_the_companion_for_nothing() {
    let parts = vec![
      RangePart { start: 100, length: 50 },
      RangePart {
        start: 1000,
        length: 300,
      },
    ];
    let mut index = CacheIndex::default();
    index.insert(100, 8, 50);
    index.insert(1000, 66, 300);
    let layout = build(&parts, 100_000);
    let plan = plan_from(&layout, 0, &index).unwrap();
    assert!(plan.companion_ranges.is_empty());
    assert_eq!(plan.cached_bytes, 350);
    assert_eq!(plan.companion_bytes, 0);
    assert_resumable_with(&parts, 100_000, &index);
  }

  #[test]
  fn partial_coverage_only_fetches_the_gaps() {
    let parts = vec![RangePart {
      start: 1000,
      length: 100,
    }];
    let mut index = CacheIndex::default();
    index.insert(1000, 8, 40);
    index.insert(1080, 56, 20);
    let layout = build(&parts, 100_000);
    let plan = plan_from(&layout, 0, &index).unwrap();
    assert_eq!(
      plan.companion_ranges,
      vec![RangeSpec {
        start: 1040,
        length: 40
      }]
    );
    assert_eq!(plan.cached_bytes, 60);
    assert_resumable_with(&parts, 100_000, &index);
  }

  #[test]
  fn mixed_coverage_resumes_at_every_offset() {
    let parts = vec![
      RangePart { start: 0, length: 33 },
      RangePart {
        start: 1000,
        length: 41,
      },
      RangePart { start: 5000, length: 7 },
    ];
    for seed in 0..12u64 {
      assert_resumable_with(&parts, 100_000, &coverage(&parts, seed));
    }
  }

  #[test]
  fn mixed_coverage_resumes_for_a_single_part_body() {
    let parts = vec![RangePart {
      start: 4096,
      length: 61,
    }];
    for seed in 0..12u64 {
      assert_resumable_with(&parts, 100_000, &coverage(&parts, seed));
    }
  }
}
