use bridgething_sdk_runtime::{Batch, LaneFeed, LaneItem, OutboundLanes, lanes};
use bytes::{Bytes, BytesMut};
use libbridgething::Priority;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{frame::LINK_FRAME_OVERHEAD, link::Iap2Command};

pub(crate) const EA_LINK_SESSION_ID: u8 = 3;
const EA_STREAM_ID_PREFIX_LEN: usize = 2;
const EA_LANE_BYTES: usize = 256 * 1024;
const EA_BATCH_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
struct EaFrame {
  stream_id: u16,
  bytes: Bytes,
}

impl LaneItem for EaFrame {
  fn len(&self) -> usize {
    self.bytes.len()
  }
}

#[derive(Debug, thiserror::Error)]
pub enum EaSendError {
  #[error("EA chunker is no longer running for this link")]
  ChannelClosed,
}

#[derive(Debug, Clone)]
pub struct EaStreamSender {
  stream_id: u16,
  lanes: OutboundLanes<EaFrame>,
}

impl EaStreamSender {
  pub fn stream_id(&self) -> u16 {
    self.stream_id
  }

  pub async fn send(&self, priority: Priority, frame: Bytes) -> Result<(), EaSendError> {
    let frame = EaFrame {
      stream_id: self.stream_id,
      bytes: frame,
    };
    if self.lanes.send(priority, frame).await {
      Ok(())
    } else {
      Err(EaSendError::ChannelClosed)
    }
  }
}

pub(crate) struct EaChunker {
  lanes: OutboundLanes<EaFrame>,
  task: JoinHandle<()>,
}

impl Drop for EaChunker {
  fn drop(&mut self) {
    self.task.abort();
  }
}

impl EaChunker {
  pub(crate) fn new(link_command_tx: mpsc::Sender<Iap2Command>, peer_max_len: u16) -> Self {
    let (lanes, feed) = lanes(EA_LANE_BYTES, EA_BATCH_BYTES);
    let task = tokio::spawn(chunker_task(feed, link_command_tx, max_chunk_payload(peer_max_len)));
    Self { lanes, task }
  }

  pub(crate) fn sender(&self, stream_id: u16) -> EaStreamSender {
    EaStreamSender {
      stream_id,
      lanes: self.lanes.clone(),
    }
  }
}

pub(crate) fn split_stream_frame(payload: &Bytes) -> Option<(u16, Bytes)> {
  if payload.len() < 2 {
    return None;
  }
  let stream_id = u16::from_be_bytes([payload[0], payload[1]]);
  Some((stream_id, payload.slice(2..)))
}

const fn max_chunk_payload(peer_max_len: u16) -> usize {
  let overhead = LINK_FRAME_OVERHEAD + EA_STREAM_ID_PREFIX_LEN;
  let total = peer_max_len as usize;
  if total <= overhead { 1 } else { total - overhead }
}

async fn chunker_task(mut feed: LaneFeed<EaFrame>, link_tx: mpsc::Sender<Iap2Command>, max_payload: usize) {
  while feed.ready().await {
    let Ok(permit) = link_tx.reserve().await else {
      return;
    };
    let Some(batch) = feed.take_batch() else {
      continue;
    };
    let mut packets = ea_packets(batch, max_payload).into_iter();
    let Some(first) = packets.next() else {
      continue;
    };
    permit.send(Iap2Command::Send {
      session_id: EA_LINK_SESSION_ID,
      payload: first,
    });
    for payload in packets {
      let sent = link_tx
        .send(Iap2Command::Send {
          session_id: EA_LINK_SESSION_ID,
          payload,
        })
        .await;
      if sent.is_err() {
        return;
      }
    }
  }
}

fn ea_packets(batch: Batch<EaFrame>, max_payload: usize) -> Vec<Bytes> {
  let capacity = EA_STREAM_ID_PREFIX_LEN + max_payload.min(batch.bytes.max(1));
  let full = EA_STREAM_ID_PREFIX_LEN + max_payload;
  let mut packets = Vec::new();
  let mut open: Option<(u16, BytesMut)> = None;

  for frame in batch.items {
    if let Some((stream_id, _)) = &open
      && *stream_id != frame.stream_id
    {
      let (_, buf) = open.take().expect("a packet is open");
      packets.push(buf.freeze());
    }

    let mut rest = frame.bytes;
    while !rest.is_empty() {
      let (_, buf) = open.get_or_insert_with(|| {
        let mut buf = BytesMut::with_capacity(capacity);
        buf.extend_from_slice(&frame.stream_id.to_be_bytes());
        (frame.stream_id, buf)
      });
      let take = (full - buf.len()).min(rest.len());
      buf.extend_from_slice(&rest.split_to(take));
      if buf.len() == full {
        let (_, buf) = open.take().expect("a packet is open");
        packets.push(buf.freeze());
      }
    }
  }

  if let Some((_, buf)) = open {
    packets.push(buf.freeze());
  }
  packets
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::*;

  fn spawn_chunker(link_tx: mpsc::Sender<Iap2Command>, max_batch: usize, max_payload: usize) -> OutboundLanes<EaFrame> {
    let (lanes, feed) = lanes(EA_LANE_BYTES, max_batch);
    tokio::spawn(chunker_task(feed, link_tx, max_payload));
    lanes
  }

  async fn feed(lanes: &OutboundLanes<EaFrame>, priority: Priority, stream_id: u16, bytes: Bytes) {
    assert!(
      lanes.send(priority, EaFrame { stream_id, bytes }).await,
      "the chunker is still running"
    );
  }

  fn drain_chunks(rx: &mut mpsc::Receiver<Iap2Command>) -> Vec<Bytes> {
    let mut out = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
      if let Iap2Command::Send { session_id, payload } = cmd {
        assert_eq!(session_id, EA_LINK_SESSION_ID);
        out.push(payload);
      }
    }
    out
  }

  fn assert_chunk(payload: &Bytes, expected_stream: u16, expected_data: &[u8]) {
    assert!(payload.len() >= 2);
    let stream = u16::from_be_bytes([payload[0], payload[1]]);
    assert_eq!(stream, expected_stream);
    assert_eq!(&payload[2..], expected_data);
  }

  #[tokio::test]
  async fn chunker_splits_large_frame() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let lanes = spawn_chunker(link_tx, EA_BATCH_BYTES, 4);

    let payload = Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    feed(&lanes, Priority::Normal, 0x0100, payload).await;
    drop(lanes);

    tokio::time::sleep(Duration::from_millis(20)).await;
    let chunks = drain_chunks(&mut link_rx);
    assert_eq!(chunks.len(), 3, "9 bytes / 4 chunk size = 3 chunks");
    assert_chunk(&chunks[0], 0x0100, &[1, 2, 3, 4]);
    assert_chunk(&chunks[1], 0x0100, &[5, 6, 7, 8]);
    assert_chunk(&chunks[2], 0x0100, &[9]);
  }

  #[tokio::test]
  async fn chunker_normal_preempts_bulk_at_chunk_boundary() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let lanes = spawn_chunker(link_tx, EA_BATCH_BYTES, 4);

    feed(
      &lanes,
      Priority::Bulk,
      0x0200,
      Bytes::from_static(&[0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7]),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(10)).await;

    feed(&lanes, Priority::Normal, 0x0100, Bytes::from_static(&[0xA0, 0xA1])).await;

    drop(lanes);
    tokio::time::sleep(Duration::from_millis(40)).await;

    let chunks = drain_chunks(&mut link_rx);
    let stream_seq: Vec<u16> = chunks.iter().map(|p| u16::from_be_bytes([p[0], p[1]])).collect();
    assert!(
      stream_seq.windows(2).any(|w| w[0] == 0x0200 && w[1] == 0x0100),
      "Normal stream chunk lands between Bulk chunks (got {:?})",
      stream_seq
    );
    let collected_bulk: Vec<u8> = chunks
      .iter()
      .filter(|p| u16::from_be_bytes([p[0], p[1]]) == 0x0200)
      .flat_map(|p| p[2..].to_vec())
      .collect();
    assert_eq!(collected_bulk, vec![0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7]);
  }

  #[tokio::test]
  async fn chunker_coalesces_same_stream_frames_into_full_packets() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let lanes = spawn_chunker(link_tx, EA_BATCH_BYTES, 4);

    feed(&lanes, Priority::Normal, 0x0100, Bytes::from_static(&[1, 2, 3])).await;
    feed(&lanes, Priority::Normal, 0x0100, Bytes::from_static(&[4, 5, 6])).await;
    drop(lanes);
    tokio::time::sleep(Duration::from_millis(40)).await;

    let chunks = drain_chunks(&mut link_rx);
    let sizes: Vec<usize> = chunks.iter().map(|p| p.len() - 2).collect();
    assert_eq!(sizes, vec![4, 2], "frames must coalesce to fill the link budget");
    let collected: Vec<u8> = chunks.iter().flat_map(|p| p[2..].to_vec()).collect();
    assert_eq!(collected, vec![1, 2, 3, 4, 5, 6]);
  }

  #[tokio::test]
  async fn chunker_never_mixes_streams_in_one_packet() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let lanes = spawn_chunker(link_tx, EA_BATCH_BYTES, 8);

    feed(&lanes, Priority::Normal, 0x0100, Bytes::from_static(&[1, 2, 3])).await;
    feed(&lanes, Priority::Normal, 0x0200, Bytes::from_static(&[4, 5, 6])).await;
    drop(lanes);
    tokio::time::sleep(Duration::from_millis(40)).await;

    let chunks = drain_chunks(&mut link_rx);
    assert_eq!(
      chunks.len(),
      2,
      "a packet carries one stream-id prefix; streams must not mix"
    );
    assert_eq!(u16::from_be_bytes([chunks[0][0], chunks[0][1]]), 0x0100);
    assert_eq!(&chunks[0][2..], &[1, 2, 3]);
    assert_eq!(u16::from_be_bytes([chunks[1][0], chunks[1][1]]), 0x0200);
    assert_eq!(&chunks[1][2..], &[4, 5, 6]);
  }

  #[tokio::test]
  async fn chunker_bulk_preempts_background_at_chunk_boundary() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let lanes = spawn_chunker(link_tx, EA_BATCH_BYTES, 4);

    feed(
      &lanes,
      Priority::Background,
      0x0300,
      Bytes::from_static(&[0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7]),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(10)).await;

    feed(&lanes, Priority::Bulk, 0x0200, Bytes::from_static(&[0xB0, 0xB1])).await;

    drop(lanes);
    tokio::time::sleep(Duration::from_millis(40)).await;

    let chunks = drain_chunks(&mut link_rx);
    let stream_seq: Vec<u16> = chunks.iter().map(|p| u16::from_be_bytes([p[0], p[1]])).collect();
    assert!(
      stream_seq.windows(2).any(|w| w[0] == 0x0300 && w[1] == 0x0200),
      "Bulk stream chunk lands between Background chunks (got {:?})",
      stream_seq
    );
    let collected_background: Vec<u8> = chunks
      .iter()
      .filter(|p| u16::from_be_bytes([p[0], p[1]]) == 0x0300)
      .flat_map(|p| p[2..].to_vec())
      .collect();
    assert_eq!(
      collected_background,
      vec![0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7]
    );
  }

  #[tokio::test]
  async fn a_saturated_normal_lane_cannot_hold_the_link_forever() {
    const CEILING: usize = 100;

    let (link_tx, mut link_rx) = mpsc::channel(4096);
    let lanes = spawn_chunker(link_tx, CEILING, CEILING);

    let feeder = {
      let lanes = lanes.clone();
      tokio::spawn(async move {
        for _ in 0..2000 {
          let frame = EaFrame {
            stream_id: 0x0100,
            bytes: Bytes::from_static(&[0xA0; 4]),
          };
          if !lanes.send(Priority::Normal, frame).await {
            return;
          }
        }
      })
    };

    tokio::time::sleep(Duration::from_millis(30)).await;
    let _ = drain_chunks(&mut link_rx);
    feed(&lanes, Priority::Bulk, 0x0200, Bytes::from_static(&[0xB0; 4])).await;

    tokio::time::sleep(Duration::from_millis(30)).await;
    let chunks = drain_chunks(&mut link_rx);
    feeder.abort();

    let bulk_at = chunks
      .iter()
      .position(|p| u16::from_be_bytes([p[0], p[1]]) == 0x0200)
      .expect("bulk must get a turn while normal is saturated");
    assert!(
      bulk_at <= 4,
      "bulk waited {bulk_at} packets after becoming available, past the share the emission owes it"
    );
  }

  #[test]
  fn split_stream_frame_extracts_prefix() {
    let mut wire = BytesMut::new();
    wire.extend_from_slice(&0x0100u16.to_be_bytes());
    wire.extend_from_slice(&[0xDE, 0xAD]);
    let (id, rest) = split_stream_frame(&wire.freeze()).unwrap();
    assert_eq!(id, 0x0100);
    assert_eq!(&rest[..], &[0xDE, 0xAD]);
  }

  #[test]
  fn split_stream_frame_rejects_short() {
    assert!(split_stream_frame(&Bytes::from_static(&[0x01])).is_none());
  }

  #[tokio::test]
  async fn prefixed_packet_never_exceeds_link_payload_budget() {
    let peer_max_len: u16 = 2048;
    let link_budget = peer_max_len as usize - LINK_FRAME_OVERHEAD;
    let max_chunk = max_chunk_payload(peer_max_len);

    let (link_tx, mut link_rx) = mpsc::channel(256);
    let lanes = spawn_chunker(link_tx, EA_BATCH_BYTES, max_chunk);

    feed(
      &lanes,
      Priority::Normal,
      0x0100,
      Bytes::from(vec![0xAB; max_chunk * 3 + 7]),
    )
    .await;
    drop(lanes);
    tokio::time::sleep(Duration::from_millis(40)).await;

    let chunks = drain_chunks(&mut link_rx);
    assert!(!chunks.is_empty());
    for c in &chunks {
      assert!(
        c.len() <= link_budget,
        "wire packet {} bytes exceeds link payload budget {}",
        c.len(),
        link_budget
      );
    }
  }
}

#[cfg(test)]
mod trace {
  use std::{collections::HashMap, time::Duration};

  use bridgething_sdk_runtime::lane_corpus::{
    CaseIn, Constants, Emission, Emitted, EmittedCase, EmittedStep, Op, Segment, assert_conforms, constants, corpus,
    write_trace,
  };
  use futures::FutureExt;
  use tokio::task::JoinHandle;

  use super::*;

  const SETTLE: Duration = Duration::from_millis(60);
  const BOOKKEEPING_SETTLE: Duration = Duration::from_millis(5);

  struct Parked {
    id: String,
    byte_len: u64,
    task: JoinHandle<bool>,
  }

  struct Arm {
    lanes: OutboundLanes<EaFrame>,
    link_rx: mpsc::Receiver<Iap2Command>,
    chunker: JoinHandle<()>,
    frames: HashMap<u8, String>,
    next_index: u16,
    parked: Vec<Parked>,
    enqueued: u64,
    emitted: u64,
  }

  impl Arm {
    fn new(case: &CaseIn) -> Self {
      let ceiling = case.max_emission_bytes();
      let (lanes, feed) = lanes(case.max_lane_bytes(), ceiling);
      let (link_tx, link_rx) = mpsc::channel::<Iap2Command>(1);
      link_tx
        .try_send(Iap2Command::Disconnect)
        .expect("the link starts plugged");
      let chunker = tokio::spawn(chunker_task(feed, link_tx, ceiling));
      Self {
        lanes,
        link_rx,
        chunker,
        frames: HashMap::new(),
        next_index: 0,
        parked: Vec::new(),
        enqueued: 0,
        emitted: 0,
      }
    }

    async fn enqueue(&mut self, id: String, priority: Priority, byte_len: usize, stream: u16) -> EmittedStep {
      assert!(
        self.next_index < 256,
        "a case enqueues more than 256 frames; the index-tag encoding needs widening"
      );
      let index = self.next_index as u8;
      self.next_index += 1;
      self.frames.insert(index, id.clone());
      self.enqueued += byte_len as u64;

      let frame = EaFrame {
        stream_id: stream,
        bytes: Bytes::from(vec![index; byte_len]),
      };
      let admitted = tokio::task::unconstrained(self.lanes.send(priority, frame.clone())).now_or_never();
      let outcome = match admitted {
        Some(true) => "accepted",
        Some(false) => panic!("lane closed unexpectedly"),
        None => {
          let lanes = self.lanes.clone();
          let task = tokio::spawn(async move { lanes.send(priority, frame).await });
          self.parked.push(Parked {
            id,
            byte_len: byte_len as u64,
            task,
          });
          "parked"
        }
      };

      let mut step = EmittedStep::new("enqueue");
      step.outcome = Some(outcome.to_string());
      step
    }

    async fn drain(&mut self) -> EmittedStep {
      let mut step = EmittedStep::new("drain");
      let deadline = tokio::time::Instant::now() + SETTLE;
      while let Ok(Some(command)) = tokio::time::timeout_at(deadline, self.link_rx.recv()).await {
        let Iap2Command::Send { payload, .. } = command else {
          continue;
        };
        step.segments = self.decode(&payload);
        self.emitted += step.segments.iter().map(|segment| segment.bytes).sum::<u64>();
        break;
      }
      step
    }

    fn decode(&self, payload: &[u8]) -> Vec<Segment> {
      let mut segments: Vec<Segment> = Vec::new();
      let mut rest = &payload[EA_STREAM_ID_PREFIX_LEN.min(payload.len())..];
      while let Some(&value) = rest.first() {
        let run = rest.iter().take_while(|byte| **byte == value).count();
        let id = self
          .frames
          .get(&value)
          .cloned()
          .unwrap_or_else(|| format!("unknown-index-{value}"));
        segments.push(Segment { id, bytes: run as u64 });
        rest = &rest[run..];
      }
      segments
    }

    async fn settle(&mut self) {
      if self.parked.is_empty() {
        return;
      }
      tokio::time::sleep(BOOKKEEPING_SETTLE).await;
      let mut still = Vec::new();
      for item in std::mem::take(&mut self.parked) {
        if item.task.is_finished() {
          assert!(item.task.await.expect("sender task"), "lane closed unexpectedly");
        } else {
          still.push(item);
        }
      }
      self.parked = still;
    }

    fn finish(&self, step: &mut EmittedStep) {
      let waiting: u64 = self.parked.iter().map(|item| item.byte_len).sum();
      step.parked_ids = self.parked.iter().map(|item| item.id.clone()).collect();
      step.queued_bytes = Some(self.enqueued - self.emitted - waiting);
    }
  }

  async fn run_case(case: &CaseIn) -> EmittedCase {
    let mut arm = Arm::new(case);
    let mut steps = Vec::new();
    for op in case.expand() {
      let mut step = match op {
        Op::Enqueue {
          id,
          priority,
          byte_len,
          stream,
        } => arm.enqueue(id, priority, byte_len, stream).await,
        Op::Drain => arm.drain().await,
        Op::WriteComplete => EmittedStep::new("write_complete"),
      };
      arm.settle().await;
      arm.finish(&mut step);
      steps.push(step);
    }

    for item in std::mem::take(&mut arm.parked) {
      item.task.abort();
    }
    arm.chunker.abort();

    EmittedCase {
      name: case.name.clone(),
      steps,
    }
  }

  #[tokio::test(start_paused = true)]
  async fn rust_ea_conforms_to_the_frozen_expectation() {
    let mut cases = Vec::new();
    for case in &corpus().cases {
      cases.push(run_case(case).await);
    }
    let emitted = Emitted {
      implementation: "rust-ea",
      constants: Constants {
        fragments_frames: true,
        ..constants()
      },
      cases,
    };

    write_trace(&emitted);
    assert_conforms(&emitted, Emission::Fragmented);
  }
}
