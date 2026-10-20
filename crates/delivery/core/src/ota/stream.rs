use std::{
  fs::File,
  io::{ErrorKind, Read, Seek, SeekFrom},
  path::PathBuf,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_gateway::OutboundLink;
use uuid::Uuid;

use crate::{
  bundle::fetch::{sha256_file, sha256_hex},
  ota::{event::OtaPhaseSnapshot, rate::RateTracker},
  seam::Clock,
  transfer::{AckWindow, BytesSource, FRAGMENT_BYTES, FragmentSource, LinkPush, SendError, SourceRange},
};

pub const PROGRESS_EMIT_INTERVAL_MS: u64 = 250;

pub trait Artifact: FragmentSource {
  fn size(&self) -> Result<u64, String>;
  fn sha256(&self) -> Result<String, String>;
}

pub struct FileSource {
  path: PathBuf,
  handle: Mutex<Option<File>>,
}

impl FileSource {
  pub fn open(path: impl Into<PathBuf>) -> Self {
    Self {
      path: path.into(),
      handle: Mutex::new(None),
    }
  }
}

impl FragmentSource for FileSource {
  fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
    let mut held = self.handle.lock().unwrap();
    let file = match held.as_mut() {
      Some(file) => file,
      None => held.insert(File::open(&self.path).map_err(|e| e.to_string())?),
    };
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;

    let mut filled = 0;
    while filled < buf.len() {
      match file.read(&mut buf[filled..]) {
        Ok(0) => break,
        Ok(read) => filled += read,
        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
        Err(e) => return Err(e.to_string()),
      }
    }
    Ok(filled)
  }
}

impl Artifact for FileSource {
  fn size(&self) -> Result<u64, String> {
    std::fs::metadata(&self.path)
      .map(|meta| meta.len())
      .map_err(|e| e.to_string())
  }

  fn sha256(&self) -> Result<String, String> {
    sha256_file(&self.path).map_err(|e| e.to_string())
  }
}

impl Artifact for BytesSource {
  fn size(&self) -> Result<u64, String> {
    Ok(self.len())
  }

  fn sha256(&self) -> Result<String, String> {
    Ok(sha256_hex(self.bytes()))
  }
}

struct ProgressSource<'a> {
  artifact: &'a dyn Artifact,
  label: &'a str,
  total: u64,
  transfer_id: Uuid,
  acks: &'a AckWindow,
  clock: Arc<dyn Clock>,
  sink: &'a (dyn Fn(OtaPhaseSnapshot) + Send + Sync),
  state: Mutex<(RateTracker, u64)>,
}

impl ProgressSource<'_> {
  fn tick(&self, sent: u64) {
    let sent = sent.min(self.total);
    let now = self.clock.unix_millis();
    let (rate_per_sec, eta_seconds) = {
      let mut held = self.state.lock().unwrap();
      held.0.record(sent);
      if now.saturating_sub(held.1) < PROGRESS_EMIT_INTERVAL_MS && sent < self.total {
        return;
      }
      held.1 = now;
      (
        held.0.rate_per_sec(),
        held.0.eta_seconds(self.total.saturating_sub(sent)),
      )
    };

    (self.sink)(OtaPhaseSnapshot::Streaming {
      asset: self.label.to_owned(),
      sent,
      total: self.total,
      rate_per_sec,
      eta_seconds,
    });
  }
}

impl FragmentSource for ProgressSource<'_> {
  fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
    self.tick(self.acks.received_bytes(self.transfer_id));
    self.artifact.read_at(offset, buf)
  }
}

#[derive(Clone)]
pub struct ArtifactStreamer {
  push: LinkPush,
  clock: Arc<dyn Clock>,
}

impl ArtifactStreamer {
  pub fn new(link: Arc<dyn OutboundLink>, acks: Arc<AckWindow>, clock: Arc<dyn Clock>) -> Self {
    Self {
      push: LinkPush::new(link, acks, clock.clone()),
      clock,
    }
  }

  pub fn acks(&self) -> &Arc<AckWindow> {
    self.push.acks()
  }

  #[allow(clippy::too_many_arguments)]
  pub async fn stream(
    &self,
    transfer_id: Uuid,
    artifact: &dyn Artifact,
    label: &str,
    ranges: &[SourceRange],
    resume_from: u64,
    ack_timeout: Duration,
    progress: &(dyn Fn(OtaPhaseSnapshot) + Send + Sync),
  ) -> Result<u64, SendError> {
    let source = ProgressSource {
      artifact,
      label,
      total: ranges.iter().map(|range| range.length).sum(),
      transfer_id,
      acks: self.push.acks().as_ref(),
      clock: self.clock.clone(),
      sink: progress,
      state: Mutex::new((RateTracker::new(self.clock.clone()), 0)),
    };
    let pushed = self
      .push
      .run(transfer_id, &source, ranges, resume_from, FRAGMENT_BYTES, ack_timeout)
      .await?;
    source.tick(source.total);
    Ok(pushed)
  }
}

#[cfg(test)]
mod tests {
  use std::{
    sync::{Arc, Mutex},
    time::Duration,
  };

  use libbridgething::Priority;
  use uuid::Uuid;

  use super::{ArtifactStreamer, FileSource, PROGRESS_EMIT_INTERVAL_MS};
  use crate::{
    ota::{
      event::OtaPhaseSnapshot,
      harness::{FakeDevice, Spool, TestClock, linked_gateway, pattern, route_acks},
    },
    transfer::{AckWindow, FRAGMENT_BYTES, FragmentSource, MAX_WINDOW_BYTES, MIN_WINDOW_BYTES, SendError, SourceRange},
  };

  const ACK_TIMEOUT: Duration = Duration::from_secs(15);

  struct Rig {
    streamer: ArtifactStreamer,
    acks: Arc<AckWindow>,
    spool: Spool,
    device: FakeDevice,
  }

  fn rig() -> Rig {
    let (gateway, device) = linked_gateway();
    let acks = Arc::new(AckWindow::new());
    route_acks(&gateway, &acks);
    let streamer = ArtifactStreamer::new(Arc::new(gateway), acks.clone(), TestClock::new());
    Rig {
      streamer,
      acks,
      spool: Spool::new(),
      device,
    }
  }

  fn whole(len: u64) -> Vec<SourceRange> {
    vec![SourceRange { start: 0, length: len }]
  }

  #[derive(Default)]
  struct Snapshots(Mutex<Vec<OtaPhaseSnapshot>>);

  impl Snapshots {
    fn seen(&self) -> Vec<OtaPhaseSnapshot> {
      self.0.lock().unwrap().clone()
    }
  }

  fn initial_window_fragments() -> usize {
    (MIN_WINDOW_BYTES as usize) / FRAGMENT_BYTES
  }

  #[test]
  fn the_initial_window_spans_several_fragments() {
    assert!(
      initial_window_fragments() >= 4,
      "the whole point of the window is that it holds more than one fragment"
    );
  }

  #[tokio::test]
  async fn fragments_arrive_contiguous_and_inside_one_frame() {
    let mut rig = rig();
    let body = pattern(96 * 1024);
    let artifact = rig.spool.write("update.bin", &body);
    let transfer_id = Uuid::now_v7();
    let total = body.len() as u64;

    let streamer = rig.streamer.clone();
    let sending = tokio::spawn(async move {
      streamer
        .stream(
          transfer_id,
          &FileSource::open(artifact),
          "daemon",
          &whole(total),
          0,
          ACK_TIMEOUT,
          &|_| {},
        )
        .await
    });

    let mut assembled: Vec<u8> = Vec::new();
    while (assembled.len() as u64) < total {
      let fragment = rig.device.next_fragment(transfer_id).await;
      assert_eq!(
        fragment.offset as usize,
        assembled.len(),
        "fragments arrive in offset order"
      );
      assert!(
        fragment.bytes.len() <= FRAGMENT_BYTES,
        "an ota fragment must stay inside one frame"
      );
      assembled.extend_from_slice(&fragment.bytes);
      rig.device.ack(transfer_id, assembled.len() as u32);
    }

    let pushed = sending.await.expect("the stream task").expect("the stream succeeds");
    assert_eq!(pushed, total);
    assert_eq!(assembled, body, "the streamed bytes are the artifact");
  }

  #[tokio::test]
  async fn fragments_ride_the_background_lane() {
    let mut rig = rig();
    let body = pattern(64 * 1024);
    let artifact = rig.spool.write("update.bin", &body);
    let transfer_id = Uuid::now_v7();
    let total = body.len() as u64;

    let streamer = rig.streamer.clone();
    let sending = tokio::spawn(async move {
      streamer
        .stream(
          transfer_id,
          &FileSource::open(artifact),
          "daemon",
          &whole(total),
          0,
          ACK_TIMEOUT,
          &|_| {},
        )
        .await
    });

    let mut sent = 0u32;
    while u64::from(sent) < total {
      let fragment = rig.device.next_fragment(transfer_id).await;
      sent = fragment.offset + fragment.bytes.len() as u32;
      rig.device.ack(transfer_id, sent);
    }
    sending.await.expect("the stream task").expect("the stream succeeds");

    let lanes = rig.device.fragment_lanes();
    assert!(!lanes.is_empty());
    assert!(
      lanes.iter().all(|lane| *lane == Priority::Background),
      "an update must never share a lane with what the screen is doing, got {lanes:?}"
    );
  }

  #[tokio::test]
  async fn the_sender_stops_at_its_window_until_an_ack_arrives() {
    let mut rig = rig();
    let body = pattern(512 * 1024);
    let artifact = rig.spool.write("update.bin", &body);
    let transfer_id = Uuid::now_v7();
    let total = body.len() as u64;

    let streamer = rig.streamer.clone();
    let sending = tokio::spawn(async move {
      streamer
        .stream(
          transfer_id,
          &FileSource::open(artifact),
          "daemon",
          &whole(total),
          0,
          ACK_TIMEOUT,
          &|_| {},
        )
        .await
    });

    let mut sent = 0usize;
    for _ in 0..initial_window_fragments() {
      let fragment = rig.device.next_fragment(transfer_id).await;
      assert_eq!(fragment.offset as usize, sent);
      sent += fragment.bytes.len();
    }
    assert!(
      rig.device.no_fragment(transfer_id, Duration::from_millis(600)).await,
      "the sender ran past its window without an ack"
    );

    let mut acked = sent as u32;
    rig.device.ack(transfer_id, acked);
    while (sent as u64) < total {
      let fragment = rig.device.next_fragment(transfer_id).await;
      assert!(
        fragment.offset < acked + MAX_WINDOW_BYTES as u32,
        "the sender must stay inside the max window of what was acked"
      );
      sent = fragment.offset as usize + fragment.bytes.len();
      acked = sent as u32;
      rig.device.ack(transfer_id, acked);
    }

    sending.await.expect("the stream task").expect("the stream succeeds");
  }

  #[tokio::test]
  async fn a_resume_starts_at_the_daemons_offset_and_streams_the_remainder() {
    let mut rig = rig();
    let body = pattern(160 * 1024);
    let artifact = rig.spool.write("update.bin", &body);
    let transfer_id = Uuid::now_v7();
    let total = body.len() as u64;
    let resume: u64 = 64 * 1024;

    let streamer = rig.streamer.clone();
    let sending = tokio::spawn(async move {
      streamer
        .stream(
          transfer_id,
          &FileSource::open(artifact),
          "daemon",
          &whole(total),
          resume,
          ACK_TIMEOUT,
          &|_| {},
        )
        .await
    });

    let first = rig.device.next_fragment(transfer_id).await;
    assert_eq!(
      u64::from(first.offset),
      resume,
      "the first fragment resumes at the daemon's offset, not at zero"
    );
    let mut expected = u64::from(first.offset) + first.bytes.len() as u64;
    let mut tail = first.bytes.to_vec();
    rig.device.ack(transfer_id, expected as u32);

    while expected < total {
      let fragment = rig.device.next_fragment(transfer_id).await;
      assert_eq!(u64::from(fragment.offset), expected, "resume fragments stay contiguous");
      expected = u64::from(fragment.offset) + fragment.bytes.len() as u64;
      tail.extend_from_slice(&fragment.bytes);
      rig.device.ack(transfer_id, expected as u32);
    }

    sending.await.expect("the stream task").expect("the stream succeeds");
    assert_eq!(expected, total, "the whole remainder past the resume point streams");
    assert_eq!(
      tail,
      body[resume as usize..],
      "and it is the right part of the artifact"
    );
  }

  #[tokio::test]
  async fn a_resume_seeds_the_ack_window_so_the_rate_is_not_one_giant_sample() {
    let rig = rig();
    let body = pattern(160 * 1024);
    let artifact = rig.spool.write("update.bin", &body);
    let transfer_id = Uuid::now_v7();
    let resume: u64 = 64 * 1024;
    let total = body.len() as u64;

    let streamer = rig.streamer.clone();
    tokio::spawn(async move {
      let _ = streamer
        .stream(
          transfer_id,
          &FileSource::open(artifact),
          "daemon",
          &whole(total),
          resume,
          ACK_TIMEOUT,
          &|_| {},
        )
        .await;
    });

    let seeded = tokio::time::timeout(Duration::from_secs(3), async {
      loop {
        if rig.acks.received_bytes(transfer_id) >= resume {
          return rig.acks.received_bytes(transfer_id);
        }
        tokio::task::yield_now().await;
      }
    })
    .await
    .expect("the resume point is noted before the first fragment");

    assert_eq!(seeded, resume);
  }

  #[tokio::test]
  async fn acks_that_stop_coming_fail_the_stream_rather_than_parking_it() {
    let mut rig = rig();
    let body = pattern(512 * 1024);
    let artifact = rig.spool.write("update.bin", &body);
    let transfer_id = Uuid::now_v7();
    let total = body.len() as u64;

    let streamer = rig.streamer.clone();
    let sending = tokio::spawn(async move {
      streamer
        .stream(
          transfer_id,
          &FileSource::open(artifact),
          "daemon",
          &whole(total),
          0,
          Duration::from_millis(200),
          &|_| {},
        )
        .await
    });

    for _ in 0..initial_window_fragments() {
      rig.device.next_fragment(transfer_id).await;
    }

    let err = tokio::time::timeout(Duration::from_secs(3), sending)
      .await
      .expect("the stream gave up instead of hanging")
      .expect("the stream task")
      .expect_err("a silent daemon fails the stream");

    assert!(matches!(err, SendError::Stalled(_)), "got {err}");
  }

  #[tokio::test]
  async fn an_artifact_shorter_than_its_declared_size_fails_at_the_end() {
    let mut rig = rig();
    let body = pattern(48 * 1024);
    let artifact = rig.spool.write("update.bin", &body);
    let transfer_id = Uuid::now_v7();
    let lied_total = body.len() as u64 + 4_096;

    let streamer = rig.streamer.clone();
    let sending = tokio::spawn(async move {
      streamer
        .stream(
          transfer_id,
          &FileSource::open(artifact),
          "daemon",
          &whole(lied_total),
          0,
          ACK_TIMEOUT,
          &|_| {},
        )
        .await
    });

    let mut sent = 0u32;
    while u64::from(sent) < body.len() as u64 {
      let fragment = rig.device.next_fragment(transfer_id).await;
      sent = fragment.offset + fragment.bytes.len() as u32;
      rig.device.ack(transfer_id, sent);
    }

    let err = tokio::time::timeout(Duration::from_secs(3), sending)
      .await
      .expect("the stream ended rather than looping on an empty read")
      .expect("the stream task")
      .expect_err("a short artifact fails");

    assert!(matches!(err, SendError::UnexpectedEof { .. }), "got {err}");
  }

  #[tokio::test]
  async fn an_artifact_that_is_not_there_fails_before_anything_goes_out() {
    let mut rig = rig();
    let transfer_id = Uuid::now_v7();
    let missing = rig.spool.path().join("never-written.bin");

    let err = rig
      .streamer
      .stream(
        transfer_id,
        &FileSource::open(missing),
        "daemon",
        &whole(1_024),
        0,
        ACK_TIMEOUT,
        &|_| {},
      )
      .await
      .expect_err("an artifact that is not spooled cannot stream");

    assert!(matches!(err, SendError::Source { .. }), "got {err}");
    assert!(
      rig.device.no_fragment(transfer_id, Duration::from_millis(200)).await,
      "nothing may reach the wire for an artifact that does not exist"
    );
  }

  #[test]
  fn the_source_reads_only_what_the_caller_sized_for() {
    let spool = Spool::new();
    let body = pattern(512 * 1024);
    let artifact = spool.write("update.bin", &body);
    let source = FileSource::open(&artifact);
    let mut buf = vec![0u8; FRAGMENT_BYTES];

    let read = source.read_at(64 * 1024, &mut buf).expect("a readable artifact");

    assert_eq!(read, FRAGMENT_BYTES, "a whole fragment comes back from the middle");
    assert_eq!(
      buf,
      body[64 * 1024..64 * 1024 + FRAGMENT_BYTES],
      "and it is the right part of the file, so a half-megabyte artifact is never resident"
    );
  }

  #[test]
  fn the_source_stops_at_the_end_of_the_file() {
    let spool = Spool::new();
    let body = pattern(1_000);
    let artifact = spool.write("update.bin", &body);
    let source = FileSource::open(&artifact);
    let mut buf = vec![0u8; FRAGMENT_BYTES];

    let read = source.read_at(900, &mut buf).expect("a readable artifact");

    assert_eq!(read, 100, "a short tail reads short rather than padding");
  }

  #[tokio::test]
  async fn streaming_progress_is_throttled_and_always_lands_on_the_total() {
    let mut rig = rig();
    let body = pattern(256 * 1024);
    let artifact = rig.spool.write("update.bin", &body);
    let transfer_id = Uuid::now_v7();
    let total = body.len() as u64;
    let snapshots = Arc::new(Snapshots::default());

    let streamer = rig.streamer.clone();
    let sink = snapshots.clone();
    let sending = tokio::spawn(async move {
      streamer
        .stream(
          transfer_id,
          &FileSource::open(artifact),
          "update.swu",
          &whole(total),
          0,
          ACK_TIMEOUT,
          &move |snapshot| sink.0.lock().unwrap().push(snapshot),
        )
        .await
    });

    let mut sent = 0u32;
    while u64::from(sent) < total {
      let fragment = rig.device.next_fragment(transfer_id).await;
      sent = fragment.offset + fragment.bytes.len() as u32;
      rig.device.ack(transfer_id, sent);
    }
    sending.await.expect("the stream task").expect("the stream succeeds");

    let seen = snapshots.seen();
    assert!(
      seen.iter().all(|snapshot| matches!(
        snapshot,
        OtaPhaseSnapshot::Streaming { asset, total: reported, .. } if asset == "update.swu" && *reported == total
      )),
      "every tick names the asset and its size, got {seen:?}"
    );
    let sent_values: Vec<u64> = seen
      .iter()
      .filter_map(|snapshot| match snapshot {
        OtaPhaseSnapshot::Streaming { sent, .. } => Some(*sent),
        _ => None,
      })
      .collect();
    assert!(
      sent_values.windows(2).all(|pair| pair[0] <= pair[1]),
      "progress never goes backwards, got {sent_values:?}"
    );
    assert_eq!(
      sent_values.last().copied(),
      Some(total),
      "the final tick reports the whole artifact"
    );
    assert!(
      sent_values.len() < 16,
      "a {PROGRESS_EMIT_INTERVAL_MS}ms throttle must not emit per fragment, got {} ticks",
      sent_values.len()
    );
  }
}
