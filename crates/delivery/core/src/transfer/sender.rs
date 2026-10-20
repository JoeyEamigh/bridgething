use std::time::Duration;

use bytes::Bytes;
use libbridgething::gateway::TransferFragment;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{AckWindow, Pacer, ack::TransferStalled};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
  pub start: u64,
  pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
  #[error(transparent)]
  Stalled(#[from] TransferStalled),
  #[error("source ended at {offset} of {total} before the stream did")]
  UnexpectedEof { offset: u64, total: u64 },
  #[error("stream offset {offset} does not fit the wire's offset field")]
  OffsetOverflow { offset: u64 },
  #[error("the link closed at offset {offset}")]
  SinkClosed { offset: u64 },
  #[error("source failed at offset {offset}: {reason}")]
  Source { offset: u64, reason: String },
}

pub trait FragmentSource: Send + Sync {
  fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, String>;
}

pub struct FragmentStream<'a> {
  pub transfer_id: Uuid,
  pub source: &'a dyn FragmentSource,
  pub ranges: &'a [SourceRange],
  pub resume_from: u64,
  pub fragment_bytes: usize,
  pub sink: &'a mpsc::Sender<TransferFragment>,
  pub acks: &'a AckWindow,
  pub ack_timeout: Duration,
}

impl FragmentStream<'_> {
  pub async fn run(&self, pacer: &mut Pacer) -> Result<u64, SendError> {
    self.acks.open(self.transfer_id, self.resume_from);

    let total: u64 = self.ranges.iter().map(|range| range.length).sum();
    let mut buf = vec![0u8; self.fragment_bytes];
    let mut stream_offset = self.resume_from;
    let mut pushed = 0u64;
    let mut range_start = 0u64;

    for range in self.ranges {
      let range_end = range_start + range.length;
      while stream_offset < range_end {
        pacer.observe(self.acks.received_bytes(self.transfer_id));
        self
          .acks
          .await_window(self.transfer_id, stream_offset, pacer.window_bytes(), self.ack_timeout)
          .await?;

        let offset = u32::try_from(stream_offset).map_err(|_| SendError::OffsetOverflow { offset: stream_offset })?;
        let want = buf.len().min((range_end - stream_offset) as usize);
        let source_offset = range.start + (stream_offset - range_start);
        let read = self
          .source
          .read_at(source_offset, &mut buf[..want])
          .map_err(|reason| SendError::Source {
            offset: stream_offset,
            reason,
          })?;
        if read == 0 {
          return Err(SendError::UnexpectedEof {
            offset: stream_offset,
            total,
          });
        }

        self
          .sink
          .send(TransferFragment {
            transfer_id: self.transfer_id,
            offset,
            bytes: Bytes::copy_from_slice(&buf[..read]),
          })
          .await
          .map_err(|_| SendError::SinkClosed { offset: stream_offset })?;
        stream_offset += read as u64;
        pushed += read as u64;
      }
      range_start = range_end;
    }

    Ok(pushed)
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use tokio::time::timeout;

  use super::{
    super::{
      FRAGMENT_BYTES, MAX_WINDOW_BYTES, MIN_WINDOW_BYTES,
      fixture::{EndlessSource, SliceSource, TestClock, ramp},
    },
    *,
  };

  const SHORT: Duration = Duration::from_millis(200);
  const PATIENT: Duration = Duration::from_secs(3);

  struct Rig {
    id: Uuid,
    acks: Arc<AckWindow>,
    tx: mpsc::Sender<TransferFragment>,
    rx: mpsc::Receiver<TransferFragment>,
    clock: Arc<TestClock>,
  }

  fn rig() -> Rig {
    let (tx, rx) = mpsc::channel(64);
    Rig {
      id: Uuid::now_v7(),
      acks: Arc::new(AckWindow::new()),
      tx,
      rx,
      clock: TestClock::new(),
    }
  }

  fn whole(len: usize) -> Vec<SourceRange> {
    vec![SourceRange {
      start: 0,
      length: len as u64,
    }]
  }

  fn spawn_run(
    rig: &Rig,
    source: Arc<dyn FragmentSource>,
    ranges: Vec<SourceRange>,
    resume_from: u64,
    ack_timeout: Duration,
  ) -> tokio::task::JoinHandle<Result<u64, SendError>> {
    let (id, acks, tx, clock) = (rig.id, rig.acks.clone(), rig.tx.clone(), rig.clock.clone());
    tokio::spawn(async move {
      let mut pacer = Pacer::new(clock, resume_from);
      FragmentStream {
        transfer_id: id,
        source: source.as_ref(),
        ranges: &ranges,
        resume_from,
        fragment_bytes: FRAGMENT_BYTES,
        sink: &tx,
        acks: &acks,
        ack_timeout,
      }
      .run(&mut pacer)
      .await
    })
  }

  #[tokio::test]
  async fn every_fragment_stays_within_one_frame_and_arrives_in_offset_order() {
    let rig = rig();
    let payload = ramp(96 * 1024);
    let run = spawn_run(
      &rig,
      Arc::new(SliceSource::new(payload.clone())),
      whole(payload.len()),
      0,
      Duration::from_secs(30),
    );

    let mut rx = rig.rx;
    let mut assembled = Vec::new();
    while assembled.len() < payload.len() {
      let fragment = timeout(PATIENT, rx.recv())
        .await
        .expect("a fragment")
        .expect("the sink is open");
      assert!(
        fragment.bytes.len() <= FRAGMENT_BYTES,
        "a fragment must fit one frame, got {}",
        fragment.bytes.len()
      );
      assert_eq!(
        fragment.offset as usize,
        assembled.len(),
        "fragments must arrive contiguous in offset order"
      );
      assert_eq!(fragment.transfer_id, rig.id);
      assembled.extend_from_slice(&fragment.bytes);
      rig.acks.note(rig.id, assembled.len() as u64);
    }

    assert_eq!(assembled, payload);
    assert_eq!(
      timeout(PATIENT, run).await.expect("the run ended").unwrap().unwrap(),
      payload.len() as u64
    );
  }

  #[tokio::test]
  async fn the_first_window_spans_several_fragments_and_then_holds() {
    let rig = rig();
    let payload = ramp(256 * 1024);
    let _run = spawn_run(
      &rig,
      Arc::new(SliceSource::new(payload.clone())),
      whole(payload.len()),
      0,
      Duration::from_secs(30),
    );

    let in_flight = MIN_WINDOW_BYTES as usize / FRAGMENT_BYTES;
    assert!(in_flight >= 4, "the opening window must span several fragments");

    let mut rx = rig.rx;
    for expected in 0..in_flight {
      let fragment = timeout(PATIENT, rx.recv())
        .await
        .expect("a fragment")
        .expect("the sink is open");
      assert_eq!(fragment.offset as usize, expected * FRAGMENT_BYTES);
    }
    assert!(
      timeout(SHORT, rx.recv()).await.is_err(),
      "the sender ran past its window without an ack"
    );
  }

  #[tokio::test]
  async fn the_sender_never_runs_past_the_max_window_of_acked() {
    let rig = rig();
    let payload = ramp(512 * 1024);
    let run = spawn_run(
      &rig,
      Arc::new(SliceSource::new(payload.clone())),
      whole(payload.len()),
      0,
      Duration::from_secs(30),
    );

    let mut rx = rig.rx;
    let mut acked = 0u64;
    let mut sent = 0usize;
    while sent < payload.len() {
      let fragment = timeout(PATIENT, rx.recv())
        .await
        .expect("a fragment")
        .expect("the sink is open");
      assert!(
        (fragment.offset as u64) < acked + MAX_WINDOW_BYTES,
        "offset {} ran past the max window of acked {acked}",
        fragment.offset
      );
      sent = fragment.offset as usize + fragment.bytes.len();
      acked = sent as u64;
      rig.clock.advance(Duration::from_millis(50));
      rig.acks.note(rig.id, acked);
    }
    timeout(PATIENT, run).await.expect("the run ended").unwrap().unwrap();
  }

  #[tokio::test]
  async fn a_resume_offset_seeds_the_window_and_streams_only_the_remainder() {
    let rig = rig();
    let payload = ramp(160 * 1024);
    let resume = 64 * 1024u64;
    let run = spawn_run(
      &rig,
      Arc::new(SliceSource::new(payload.clone())),
      whole(payload.len()),
      resume,
      Duration::from_secs(30),
    );

    let mut rx = rig.rx;
    let mut expected = resume;
    while expected < payload.len() as u64 {
      let fragment = timeout(PATIENT, rx.recv())
        .await
        .expect("a fragment")
        .expect("the sink is open");
      assert_eq!(
        fragment.offset as u64, expected,
        "a resumed stream starts at the peer's offset, not at zero"
      );
      assert_eq!(
        fragment.bytes,
        payload[expected as usize..expected as usize + fragment.bytes.len()]
      );
      expected += fragment.bytes.len() as u64;
      rig.acks.note(rig.id, expected);
    }

    assert_eq!(
      timeout(PATIENT, run).await.expect("the run ended").unwrap().unwrap(),
      payload.len() as u64 - resume,
      "the run reports what it pushed, not what the transfer weighs"
    );
  }

  #[tokio::test]
  async fn ranges_stream_as_one_contiguous_offset_space() {
    let rig = rig();
    let source = ramp(1024);
    let ranges = vec![
      SourceRange {
        start: 512,
        length: 256,
      },
      SourceRange { start: 0, length: 256 },
    ];
    let run = spawn_run(
      &rig,
      Arc::new(SliceSource::new(source.clone())),
      ranges,
      0,
      Duration::from_secs(30),
    );

    let mut rx = rig.rx;
    let first = timeout(PATIENT, rx.recv())
      .await
      .expect("a fragment")
      .expect("the sink is open");
    assert_eq!(first.offset, 0);
    assert_eq!(first.bytes, source[512..768], "the first range is served first");

    let second = timeout(PATIENT, rx.recv())
      .await
      .expect("a fragment")
      .expect("the sink is open");
    assert_eq!(second.offset, 256, "wire offsets are the stream's, not the source's");
    assert_eq!(second.bytes, source[0..256]);
    assert!(
      timeout(SHORT, rx.recv()).await.is_err(),
      "a fragment must never span two ranges"
    );

    assert_eq!(
      timeout(PATIENT, run).await.expect("the run ended").unwrap().unwrap(),
      512
    );
  }

  #[tokio::test]
  async fn an_empty_range_set_sends_nothing() {
    let rig = rig();
    let run = spawn_run(
      &rig,
      Arc::new(SliceSource::new(ramp(1024))),
      vec![],
      0,
      Duration::from_secs(30),
    );

    assert_eq!(timeout(PATIENT, run).await.expect("the run ended").unwrap().unwrap(), 0);
    let mut rx = rig.rx;
    assert!(timeout(SHORT, rx.recv()).await.is_err(), "there was nothing to send");
  }

  #[tokio::test]
  async fn a_silent_peer_stalls_the_stream() {
    let rig = rig();
    let payload = ramp(512 * 1024);
    let run = spawn_run(
      &rig,
      Arc::new(SliceSource::new(payload.clone())),
      whole(payload.len()),
      0,
      Duration::from_millis(150),
    );

    let err = timeout(PATIENT, run)
      .await
      .expect("the run gave up on its own")
      .unwrap()
      .expect_err("a peer that never acks must not park the sender forever");
    assert!(
      matches!(err, SendError::Stalled(ref stalled) if stalled.transfer_id == rig.id),
      "got {err}"
    );
  }

  #[tokio::test]
  async fn a_source_that_ends_early_is_an_unexpected_eof() {
    let rig = rig();
    let run = spawn_run(
      &rig,
      Arc::new(SliceSource::truncated_at(ramp(64 * 1024), 32 * 1024)),
      whole(64 * 1024),
      0,
      Duration::from_secs(30),
    );

    let mut rx = rig.rx;
    let mut acked = 0u64;
    while acked < 32 * 1024 {
      let fragment = timeout(PATIENT, rx.recv())
        .await
        .expect("a fragment")
        .expect("the sink is open");
      acked = fragment.offset as u64 + fragment.bytes.len() as u64;
      rig.acks.note(rig.id, acked);
    }

    let err = timeout(PATIENT, run)
      .await
      .expect("the run ended")
      .unwrap()
      .expect_err("a payload that shrank under the sender is not a short stream");
    assert_eq!(
      err,
      SendError::UnexpectedEof {
        offset: 32 * 1024,
        total: 64 * 1024
      }
    );
  }

  #[tokio::test]
  async fn a_closed_link_ends_the_stream() {
    let rig = rig();
    let payload = ramp(64 * 1024);
    drop(rig.rx);
    let (id, acks, tx, clock) = (rig.id, rig.acks.clone(), rig.tx.clone(), rig.clock.clone());
    let source = SliceSource::new(payload.clone());
    let ranges = whole(payload.len());

    let mut pacer = Pacer::new(clock, 0);
    let err = FragmentStream {
      transfer_id: id,
      source: &source,
      ranges: &ranges,
      resume_from: 0,
      fragment_bytes: FRAGMENT_BYTES,
      sink: &tx,
      acks: &acks,
      ack_timeout: Duration::from_secs(30),
    }
    .run(&mut pacer)
    .await
    .expect_err("a closed link is not a completed stream");
    assert_eq!(err, SendError::SinkClosed { offset: 0 });
  }

  #[tokio::test]
  async fn a_stream_offset_past_the_wire_field_is_refused() {
    let rig = rig();
    let resume = u32::MAX as u64 - FRAGMENT_BYTES as u64;
    let run = spawn_run(
      &rig,
      Arc::new(EndlessSource),
      vec![SourceRange {
        start: 0,
        length: u32::MAX as u64 + 64 * 1024,
      }],
      resume,
      Duration::from_secs(30),
    );

    let err = timeout(PATIENT, run)
      .await
      .expect("the run ended")
      .unwrap()
      .expect_err("an offset the wire cannot carry is refused rather than truncated");
    assert_eq!(
      err,
      SendError::OffsetOverflow {
        offset: u32::MAX as u64 + FRAGMENT_BYTES as u64
      }
    );
  }
}
