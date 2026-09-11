use std::{
  collections::VecDeque,
  ffi::CStr,
  io::{self, Read, Seek, SeekFrom},
  pin::Pin,
  sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{Receiver, RecvTimeoutError, Sender, channel},
  },
  task::{Context, Poll, Waker},
  thread::{self, JoinHandle},
  time::{Duration, Instant},
};

use bridgething_companion::backend::{
  StreamBackend, StreamMetadata, StreamPresentation, StreamSink, StreamSource, StreamStatus, StreamTiming,
};
use futures::executor::block_on;
use pulseaudio::{Client, PlaybackSource, PlaybackStream, protocol};
use reqwest::header::HeaderMap;
use symphonia::core::{
  codecs::CodecParameters,
  errors::Error as DecodeFailure,
  formats::{FormatReader, SeekMode, SeekTo, TrackType, probe::Hint},
  io::{MediaSource, MediaSourceStream},
  meta::StandardTag,
  units::{Time, TimeBase},
};
use tokio::{runtime::Runtime, sync::Notify};

const APP_BUNDLE: &str = "com.bridgething.desktop";
const CLIENT_NAME: &CStr = c"bridgething";
const USER_AGENT: &str = concat!("bridgething/", env!("CARGO_PKG_VERSION"));
const SAMPLE_BYTES: usize = 2;
const MAX_CHANNELS: usize = 8;
const RING_LIMIT: usize = 512 * 1024;
const BODY_LIMIT: u64 = 4 * 1024 * 1024;
const RETAIN_LIMIT: u64 = 256 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_millis(50);
const WAIT: Duration = Duration::from_millis(100);
const TIMING_PERIOD: Duration = Duration::from_secs(1);
const SERVER_PERIOD: Duration = Duration::from_millis(200);

const HLS_TYPES: [&str; 6] = [
  "application/vnd.apple.mpegurl",
  "application/x-mpegurl",
  "application/mpegurl",
  "audio/mpegurl",
  "audio/x-mpegurl",
  "vnd.apple.mpegurl",
];

const POSITIONS: [protocol::ChannelPosition; MAX_CHANNELS] = [
  protocol::ChannelPosition::FrontLeft,
  protocol::ChannelPosition::FrontRight,
  protocol::ChannelPosition::FrontCenter,
  protocol::ChannelPosition::Lfe,
  protocol::ChannelPosition::RearLeft,
  protocol::ChannelPosition::RearRight,
  protocol::ChannelPosition::SideLeft,
  protocol::ChannelPosition::SideRight,
];

enum Command {
  Pause,
  Resume,
  Stop,
}

#[derive(Clone, Copy)]
struct Format {
  rate: u32,
  channels: usize,
}

#[derive(Default)]
pub struct PulseStream {
  active: Mutex<Option<Playback>>,
}

impl StreamBackend for PulseStream {
  fn app_bundle(&self) -> String {
    APP_BUNDLE.to_owned()
  }

  fn play(&self, source: StreamSource, sink: Arc<StreamSink>) {
    let mut active = self.active.lock().unwrap();
    if let Some(playback) = active.take() {
      playback.shutdown();
    }
    sink.on_status(StreamStatus::Buffering);
    *active = Playback::start(source, sink);
  }

  fn present(&self, _presentation: StreamPresentation) {}

  fn pause(&self) {
    self.send(Command::Pause);
  }

  fn resume(&self) {
    self.send(Command::Resume);
  }

  fn seek_to(&self, position_ms: u32) {
    let active = self.active.lock().unwrap();
    let Some(playback) = active.as_ref() else {
      return;
    };
    if !playback.shared.seekable.load(Ordering::SeqCst) {
      return;
    }
    *playback.shared.seek_ms.lock().unwrap() = Some(position_ms);
    playback.shared.pcm.block();
  }

  fn stop(&self) {
    let playback = self.active.lock().unwrap().take();
    if let Some(playback) = playback {
      playback.shutdown();
    }
  }
}

impl PulseStream {
  fn send(&self, command: Command) {
    let active = self.active.lock().unwrap();
    if let Some(playback) = active.as_ref() {
      let _ = playback.commands.send(command);
    }
  }
}

struct Playback {
  shared: Arc<Shared>,
  commands: Sender<Command>,
  control: JoinHandle<()>,
}

impl Playback {
  fn start(stream: StreamSource, sink: Arc<StreamSink>) -> Option<Self> {
    let shared = Arc::new(Shared::new(sink));
    let (commands, inbox) = channel();

    let fetching = Arc::clone(&shared);
    let source = thread::Builder::new()
      .name("bridgething-stream-source".to_owned())
      .spawn(move || source(fetching, stream));
    if let Err(error) = source {
      shared.fail(format!("the stream reader could not be started: {error}"));
      shared.report_failure();
      return None;
    }

    let playing = Arc::clone(&shared);
    let control = thread::Builder::new()
      .name("bridgething-stream-control".to_owned())
      .spawn(move || control(playing, inbox));
    let control = match control {
      Ok(control) => control,
      Err(error) => {
        shared.cancel();
        shared.fail(format!("the audio writer could not be started: {error}"));
        shared.report_failure();
        return None;
      }
    };

    Some(Playback {
      shared,
      commands,
      control,
    })
  }

  fn shutdown(self) {
    self.shared.cancel();
    let _ = self.commands.send(Command::Stop);
    let _ = self.control.join();
  }
}

struct Shared {
  sink: Arc<StreamSink>,
  pcm: Arc<Pcm>,
  cancelled: Arc<AtomicBool>,
  paused: AtomicBool,
  seekable: AtomicBool,
  duration_ms: AtomicU32,
  position_base_ms: AtomicU32,
  flush: AtomicBool,
  seek_ms: Mutex<Option<u32>>,
  failure: Mutex<Option<String>>,
  format: Mutex<Option<Format>>,
  ready: Condvar,
}

impl Shared {
  fn new(sink: Arc<StreamSink>) -> Self {
    let cancelled = Arc::new(AtomicBool::new(false));
    Shared {
      sink,
      pcm: Arc::new(Pcm::new(Arc::clone(&cancelled))),
      cancelled,
      paused: AtomicBool::new(false),
      seekable: AtomicBool::new(false),
      duration_ms: AtomicU32::new(0),
      position_base_ms: AtomicU32::new(0),
      flush: AtomicBool::new(false),
      seek_ms: Mutex::new(None),
      failure: Mutex::new(None),
      format: Mutex::new(None),
      ready: Condvar::new(),
    }
  }

  fn cancelled(&self) -> bool {
    self.cancelled.load(Ordering::SeqCst)
  }

  fn cancel(&self) {
    self.cancelled.store(true, Ordering::SeqCst);
    self.pcm.wake();
    self.ready.notify_all();
  }

  fn fail(&self, reason: String) {
    let mut failure = self.failure.lock().unwrap();
    if failure.is_none() {
      *failure = Some(reason);
    }
    self.ready.notify_all();
  }

  fn take_failure(&self) -> Option<String> {
    self.failure.lock().unwrap().take()
  }

  fn report_failure(&self) {
    if let Some(reason) = self.take_failure() {
      self.sink.on_status(StreamStatus::Failed { reason });
    }
  }

  fn publish(&self, format: Format) {
    *self.format.lock().unwrap() = Some(format);
    self.ready.notify_all();
  }

  fn await_format(&self) -> Option<Format> {
    let mut held = self.format.lock().unwrap();
    loop {
      if let Some(format) = *held {
        return Some(format);
      }
      if self.cancelled() || self.failure.lock().unwrap().is_some() {
        return None;
      }
      held = self.ready.wait_timeout(held, WAIT).unwrap().0;
    }
  }

  fn timing(&self, position_ms: u32) {
    let duration_ms = self.duration_ms.load(Ordering::SeqCst);
    self.sink.on_timing(StreamTiming {
      position_ms,
      duration_ms: (duration_ms > 0).then_some(duration_ms),
      seekable: self.seekable.load(Ordering::SeqCst),
    });
  }
}

struct Pcm {
  state: Mutex<PcmState>,
  drained: Condvar,
  cancelled: Arc<AtomicBool>,
}

struct PcmState {
  data: VecDeque<u8>,
  frame: usize,
  blocked: bool,
  finished: bool,
  waker: Option<Waker>,
}

impl Pcm {
  fn new(cancelled: Arc<AtomicBool>) -> Self {
    Pcm {
      state: Mutex::new(PcmState {
        data: VecDeque::new(),
        frame: 1,
        blocked: false,
        finished: false,
        waker: None,
      }),
      drained: Condvar::new(),
      cancelled,
    }
  }

  fn cancelled(&self) -> bool {
    self.cancelled.load(Ordering::SeqCst)
  }

  fn set_frame(&self, frame: usize) {
    self.state.lock().unwrap().frame = frame.max(1);
  }

  fn push(&self, bytes: &[u8]) {
    let mut state = self.state.lock().unwrap();
    while state.data.len() >= RING_LIMIT && !self.cancelled() {
      state = self.drained.wait_timeout(state, WAIT).unwrap().0;
    }
    if self.cancelled() {
      return;
    }
    state.data.extend(bytes);
    let waker = state.waker.take();
    drop(state);
    if let Some(waker) = waker {
      waker.wake();
    }
  }

  fn finish(&self) {
    let mut state = self.state.lock().unwrap();
    state.finished = true;
    let waker = state.waker.take();
    drop(state);
    if let Some(waker) = waker {
      waker.wake();
    }
  }

  fn block(&self) {
    let mut state = self.state.lock().unwrap();
    state.blocked = true;
    state.data.clear();
    drop(state);
    self.drained.notify_all();
  }

  fn unblock(&self) {
    let mut state = self.state.lock().unwrap();
    state.blocked = false;
    let waker = state.waker.take();
    drop(state);
    if let Some(waker) = waker {
      waker.wake();
    }
  }

  fn wake(&self) {
    let waker = self.state.lock().unwrap().waker.take();
    self.drained.notify_all();
    if let Some(waker) = waker {
      waker.wake();
    }
  }

  fn drained(&self) -> bool {
    let state = self.state.lock().unwrap();
    state.finished && state.data.len() < state.frame
  }

  fn poll_take(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<usize> {
    let mut state = self.state.lock().unwrap();
    if self.cancelled() {
      return Poll::Ready(0);
    }
    if !state.blocked {
      let take = buf.len().min(state.data.len()) / state.frame * state.frame;
      if take > 0 {
        copy_at(&state.data, 0, &mut buf[..take]);
        state.data.drain(..take);
        drop(state);
        self.drained.notify_all();
        return Poll::Ready(take);
      }
      if state.finished && state.data.len() < state.frame {
        return Poll::Ready(0);
      }
    }
    state.waker = Some(cx.waker().clone());
    Poll::Pending
  }
}

struct PcmSource(Arc<Pcm>);

impl PlaybackSource for PcmSource {
  fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<usize> {
    self.0.poll_take(cx, buf)
  }
}

struct Bytestream {
  state: Mutex<Body>,
  ready: Condvar,
  room: Notify,
  cancelled: Arc<AtomicBool>,
  retain: bool,
  total: Option<u64>,
}

struct Body {
  data: VecDeque<u8>,
  base: u64,
  cursor: u64,
  done: bool,
  failure: Option<String>,
}

impl Bytestream {
  fn new(cancelled: Arc<AtomicBool>, retain: bool, total: Option<u64>) -> Self {
    Bytestream {
      state: Mutex::new(Body {
        data: VecDeque::new(),
        base: 0,
        cursor: 0,
        done: false,
        failure: None,
      }),
      ready: Condvar::new(),
      room: Notify::new(),
      cancelled,
      retain,
      total,
    }
  }

  fn cancelled(&self) -> bool {
    self.cancelled.load(Ordering::SeqCst)
  }

  fn push(&self, chunk: &[u8]) {
    let mut state = self.state.lock().unwrap();
    state.data.extend(chunk);
    drop(state);
    self.ready.notify_all();
  }

  fn full(&self) -> bool {
    if self.retain {
      return false;
    }
    let state = self.state.lock().unwrap();
    (state.base + state.data.len() as u64 - state.cursor) >= BODY_LIMIT
  }

  fn finish(&self) {
    let mut state = self.state.lock().unwrap();
    state.done = true;
    let held = state.base + state.data.len() as u64;
    if self.total.is_some_and(|total| held < total) {
      state.failure = Some("the body ended early".to_owned());
    }
    drop(state);
    self.ready.notify_all();
  }

  fn fail(&self, reason: String) {
    let mut state = self.state.lock().unwrap();
    state.failure = Some(reason);
    state.done = true;
    drop(state);
    self.ready.notify_all();
  }

  fn failure(&self) -> Option<String> {
    self.state.lock().unwrap().failure.clone()
  }
}

struct BodyReader(Arc<Bytestream>);

impl Read for BodyReader {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    if buf.is_empty() {
      return Ok(0);
    }
    let mut state = self.0.state.lock().unwrap();
    loop {
      if self.0.cancelled() {
        return Ok(0);
      }
      if let Some(reason) = &state.failure {
        return Err(io::Error::other(reason.clone()));
      }
      let available = state.base + state.data.len() as u64 - state.cursor;
      if available > 0 {
        let offset = (state.cursor - state.base) as usize;
        let take = buf.len().min(available as usize);
        copy_at(&state.data, offset, &mut buf[..take]);
        state.cursor += take as u64;
        if !self.0.retain {
          let consumed = (state.cursor - state.base) as usize;
          state.data.drain(..consumed);
          state.base = state.cursor;
          self.0.room.notify_waiters();
        }
        return Ok(take);
      }
      if state.done {
        return Ok(0);
      }
      state = self.0.ready.wait_timeout(state, WAIT).unwrap().0;
    }
  }
}

impl Seek for BodyReader {
  fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
    if !self.0.retain {
      return Err(io::Error::new(io::ErrorKind::Unsupported, "the stream is live"));
    }
    let mut state = self.0.state.lock().unwrap();
    let target = match pos {
      SeekFrom::Start(offset) => offset,
      SeekFrom::Current(offset) => state.cursor.saturating_add_signed(offset),
      SeekFrom::End(offset) => match self.0.total {
        Some(total) => total.saturating_add_signed(offset),
        None => {
          return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the stream length is unknown",
          ));
        }
      },
    };
    loop {
      if self.0.cancelled() {
        return Err(io::Error::other("playback stopped"));
      }
      if let Some(reason) = &state.failure {
        return Err(io::Error::other(reason.clone()));
      }
      let held = state.base + state.data.len() as u64;
      if target <= held || state.done {
        state.cursor = target.min(held);
        return Ok(state.cursor);
      }
      state = self.0.ready.wait_timeout(state, WAIT).unwrap().0;
    }
  }
}

impl MediaSource for BodyReader {
  fn is_seekable(&self) -> bool {
    self.0.retain
  }

  fn byte_len(&self) -> Option<u64> {
    self.0.total
  }
}

fn copy_at(data: &VecDeque<u8>, offset: usize, buf: &mut [u8]) {
  let (front, back) = data.as_slices();
  let mut written = 0;
  if offset < front.len() {
    let take = (front.len() - offset).min(buf.len());
    buf[..take].copy_from_slice(&front[offset..offset + take]);
    written = take;
  }
  if written < buf.len() {
    let start = (offset + written) - front.len();
    let take = buf.len() - written;
    buf[written..].copy_from_slice(&back[start..start + take]);
  }
}

fn source(shared: Arc<Shared>, stream: StreamSource) {
  let runtime = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(1)
    .thread_name("bridgething-stream-http")
    .enable_all()
    .build();
  let runtime = match runtime {
    Ok(runtime) => runtime,
    Err(error) => return shared.fail(format!("the stream fetch could not be started: {error}")),
  };
  fetch(&shared, &runtime, stream);
  shared.pcm.finish();
  runtime.shutdown_background();
}

fn fetch(shared: &Arc<Shared>, runtime: &Runtime, stream: StreamSource) {
  bridgething_io::install_crypto_provider();
  let client = reqwest::Client::builder()
    .user_agent(USER_AGENT)
    .connect_timeout(CONNECT_TIMEOUT)
    .build();
  let client = match client {
    Ok(client) => client,
    Err(error) => return shared.fail(format!("no http client for the stream: {error}")),
  };

  let request = client.get(&stream.url).send();
  let response = match runtime.block_on(request) {
    Ok(response) => response,
    Err(error) => return shared.fail(format!("the stream could not be reached: {error}")),
  };
  let status = response.status();
  if !status.is_success() {
    return shared.fail(format!("the stream answered http {}", status.as_u16()));
  }

  let content_type = header(response.headers(), "content-type");
  if is_hls(&stream.url, content_type.as_deref()) {
    return shared.fail("hls is not supported on desktop".to_owned());
  }
  let length = response.content_length();

  if let Some(name) = stream.station.clone().filter(|_| !shared.cancelled()) {
    shared.sink.on_metadata(StreamMetadata {
      title: Some(name),
      ..StreamMetadata::default()
    });
  }

  let retain = length.is_some_and(|length| length <= RETAIN_LIMIT);
  let body = Arc::new(Bytestream::new(
    Arc::clone(&shared.cancelled),
    retain,
    length.filter(|_| retain),
  ));
  runtime.spawn(pump(response, Arc::clone(&body)));

  decode(
    shared,
    BodyReader(Arc::clone(&body)),
    &stream,
    content_type.as_deref(),
    length.is_some(),
  );
  if let Some(reason) = body.failure() {
    shared.fail(format!("the stream stopped: {reason}"));
  }
}

async fn pump(mut response: reqwest::Response, body: Arc<Bytestream>) {
  loop {
    if body.cancelled() {
      return;
    }
    match response.chunk().await {
      Ok(Some(chunk)) => {
        body.push(&chunk);
        while body.full() {
          if body.cancelled() {
            return;
          }
          let _ = tokio::time::timeout(WAIT, body.room.notified()).await;
        }
      }
      Ok(None) => return body.finish(),
      Err(error) => return body.fail(error.to_string()),
    }
  }
}

fn tagged(format: &mut dyn FormatReader, station: Option<&str>) -> Option<StreamMetadata> {
  let mut log = format.metadata();
  let revision = log.skip_to_latest()?;
  let mut found = StreamMetadata::default();
  for tag in &revision.media.tags {
    match &tag.std {
      Some(StandardTag::TrackTitle(title)) => found.title = Some(title.to_string()),
      Some(StandardTag::Artist(artist)) => found.artist = Some(artist.to_string()),
      Some(StandardTag::Album(album)) => found.album = Some(album.to_string()),
      _ => {}
    }
  }
  found.artwork = revision.media.visuals.first().map(|visual| visual.data.to_vec());
  if found == StreamMetadata::default() {
    return None;
  }
  found.title = found.title.or_else(|| station.map(str::to_owned));
  Some(found)
}

fn decode(shared: &Arc<Shared>, source: BodyReader, played: &StreamSource, content_type: Option<&str>, sized: bool) {
  let seekable = source.is_seekable();
  let mut hint = Hint::new();
  if let Some(content_type) = content_type {
    hint.mime_type(content_type.split(';').next().unwrap_or(content_type).trim());
  }
  if let Some(extension) = extension(&played.url) {
    hint.with_extension(&extension);
  }

  let stream = MediaSourceStream::new(Box::new(source), Default::default());
  let mut format = match symphonia::default::get_probe().probe(&hint, stream, Default::default(), Default::default()) {
    Ok(format) => format,
    Err(error) => {
      if !shared.cancelled() {
        shared.fail(format!("the stream could not be decoded: {error}"));
      }
      return;
    }
  };

  if let Some(metadata) = tagged(format.as_mut(), played.station.as_deref()).filter(|_| !shared.cancelled()) {
    shared.sink.on_metadata(metadata);
  }

  let Some(track) = format.default_track(TrackType::Audio) else {
    return shared.fail("the stream carries no audio".to_owned());
  };
  let track_id = track.id;
  let time_base = track.time_base;
  let duration_ms = track
    .duration
    .zip(time_base)
    .and_then(|(duration, time_base)| time_base.calc_duration(duration))
    .map(millis)
    .filter(|_| sized)
    .unwrap_or_default();
  let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
    return shared.fail("the stream carries no audio".to_owned());
  };

  let mut decoder = match symphonia::default::get_codecs().make_audio_decoder(&params, &Default::default()) {
    Ok(decoder) => decoder,
    Err(error) => return shared.fail(format!("the stream could not be decoded: {error}")),
  };

  shared.duration_ms.store(duration_ms, Ordering::SeqCst);
  shared.seekable.store(
    seekable
      && duration_ms > 0
      && format
        .seek(
          SeekMode::Accurate,
          SeekTo::Time {
            time: Time::ZERO,
            track_id: Some(track_id),
          },
        )
        .is_ok(),
    Ordering::SeqCst,
  );

  let mut samples: Vec<i16> = Vec::new();
  let mut bytes: Vec<u8> = Vec::new();
  let mut published = false;

  loop {
    if shared.cancelled() {
      return;
    }
    let request = shared.seek_ms.lock().unwrap().take();
    if let Some(target) = request {
      match seek(format.as_mut(), track_id, time_base, target) {
        Ok(landed) => {
          decoder.reset();
          shared.position_base_ms.store(landed, Ordering::SeqCst);
          shared.flush.store(true, Ordering::SeqCst);
        }
        Err(error) => {
          tracing::debug!(%error, "the stream refused a seek");
          shared.pcm.unblock();
        }
      }
    }

    let packet = match format.next_packet() {
      Ok(Some(packet)) => packet,
      Ok(None) => return,
      Err(DecodeFailure::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => return,
      Err(error) => {
        if !shared.cancelled() {
          shared.fail(format!("the stream stopped: {error}"));
        }
        return;
      }
    };
    if packet.track_id != track_id {
      continue;
    }

    let decoded = match decoder.decode(&packet) {
      Ok(decoded) => decoded,
      Err(DecodeFailure::DecodeError(reason)) => {
        tracing::debug!(%reason, "a stream packet did not decode");
        continue;
      }
      Err(error) => {
        if !shared.cancelled() {
          shared.fail(format!("the stream could not be decoded: {error}"));
        }
        return;
      }
    };

    if !published {
      let spec = decoded.spec();
      shared.publish(Format {
        rate: spec.rate(),
        channels: spec.channels().count(),
      });
      published = true;
    }

    decoded.copy_to_vec_interleaved(&mut samples);
    bytes.clear();
    bytes.reserve(samples.len() * SAMPLE_BYTES);
    for sample in &samples {
      bytes.extend_from_slice(&sample.to_le_bytes());
    }
    shared.pcm.push(&bytes);
  }
}

fn seek(
  format: &mut dyn FormatReader,
  track_id: u32,
  time_base: Option<TimeBase>,
  target_ms: u32,
) -> Result<u32, DecodeFailure> {
  let time = Time::try_new(i64::from(target_ms / 1000), (target_ms % 1000) * 1_000_000).unwrap_or(Time::ZERO);
  let landed = format.seek(
    SeekMode::Accurate,
    SeekTo::Time {
      time,
      track_id: Some(track_id),
    },
  )?;
  Ok(
    time_base
      .and_then(|time_base| time_base.calc_time(landed.actual_ts))
      .map(millis)
      .unwrap_or(target_ms),
  )
}

fn control(shared: Arc<Shared>, commands: Receiver<Command>) {
  let Some(format) = shared.await_format() else {
    shared.report_failure();
    return;
  };
  let Some(params) = params(format) else {
    shared.fail(format!("{} audio channels are not supported", format.channels));
    shared.report_failure();
    return;
  };

  let frame = format.channels * SAMPLE_BYTES;
  shared.pcm.set_frame(frame);

  let client = match Client::from_env(CLIENT_NAME) {
    Ok(client) => client,
    Err(error) => {
      shared.sink.on_status(StreamStatus::Failed {
        reason: format!("pulseaudio did not answer: {error}"),
      });
      return;
    }
  };
  let stream = block_on(client.create_playback_stream(params, PcmSource(Arc::clone(&shared.pcm))));
  let stream = match stream {
    Ok(stream) => stream,
    Err(error) => {
      shared.sink.on_status(StreamStatus::Failed {
        reason: format!("pulseaudio refused a playback stream: {error}"),
      });
      return;
    }
  };

  serve(&shared, &stream, commands, (format.rate as u64) * frame as u64);
}

fn serve(shared: &Arc<Shared>, stream: &PlaybackStream, commands: Receiver<Command>, rate: u64) {
  let mut anchor = 0i64;
  let mut playing = false;
  let mut settled = 0u8;
  let mut last_timing = Instant::now();
  let mut last_server = Instant::now();

  shared.timing(0);

  loop {
    if shared.cancelled() {
      return;
    }
    match commands.recv_timeout(TICK) {
      Ok(Command::Pause) => match block_on(stream.cork()) {
        Ok(()) => {
          shared.paused.store(true, Ordering::SeqCst);
          shared.sink.on_status(StreamStatus::Paused);
        }
        Err(error) => return failed(shared, format!("pulseaudio refused to pause: {error}")),
      },
      Ok(Command::Resume) => match block_on(stream.uncork()) {
        Ok(()) => {
          shared.paused.store(false, Ordering::SeqCst);
          if playing {
            shared.sink.on_status(StreamStatus::Playing);
          }
        }
        Err(error) => return failed(shared, format!("pulseaudio refused to resume: {error}")),
      },
      Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return,
      Err(RecvTimeoutError::Timeout) => {}
    }

    if let Some(reason) = shared.take_failure() {
      shared.sink.on_status(StreamStatus::Failed { reason });
      return;
    }

    if shared.flush.swap(false, Ordering::SeqCst) {
      if let Err(error) = block_on(stream.flush()) {
        return failed(shared, format!("pulseaudio refused to flush: {error}"));
      }
      match block_on(stream.timing_info()) {
        Ok(info) => anchor = info.read_offset,
        Err(error) => return failed(shared, format!("pulseaudio stopped answering: {error}")),
      }
      shared.pcm.unblock();
      shared.timing(shared.position_base_ms.load(Ordering::SeqCst));
      last_timing = Instant::now();
    }

    if last_server.elapsed() < SERVER_PERIOD {
      continue;
    }
    last_server = Instant::now();
    let info = match block_on(stream.timing_info()) {
      Ok(info) => info,
      Err(error) => return failed(shared, format!("pulseaudio stopped answering: {error}")),
    };
    let played = (info.read_offset - anchor).max(0) as u64;
    let position = shared
      .position_base_ms
      .load(Ordering::SeqCst)
      .saturating_add((played * 1000 / rate.max(1)) as u32);

    if !playing && info.playing && !shared.paused.load(Ordering::SeqCst) {
      playing = true;
      shared.sink.on_status(StreamStatus::Playing);
    }
    if last_timing.elapsed() >= TIMING_PERIOD {
      last_timing = Instant::now();
      shared.timing(position);
    }
    if shared.pcm.drained() && info.read_offset >= info.write_offset {
      settled += 1;
      if settled > 1 {
        shared.timing(position);
        shared.sink.on_status(StreamStatus::Ended);
        return;
      }
    } else {
      settled = 0;
    }
  }
}

fn failed(shared: &Arc<Shared>, reason: String) {
  shared.sink.on_status(StreamStatus::Failed { reason });
}

fn params(format: Format) -> Option<protocol::PlaybackStreamParams> {
  let channels = u8::try_from(format.channels).ok().filter(|channels| *channels > 0)?;
  let map = match format.channels {
    1 => protocol::ChannelMap::mono(),
    2 => protocol::ChannelMap::stereo(),
    channels if channels <= MAX_CHANNELS => protocol::ChannelMap::new(POSITIONS[..channels].iter().copied()),
    _ => return None,
  };
  Some(protocol::PlaybackStreamParams {
    sample_spec: protocol::SampleSpec {
      format: protocol::SampleFormat::S16Le,
      channels,
      sample_rate: format.rate,
    },
    channel_map: map,
    cvolume: Some(protocol::ChannelVolume::norm(channels)),
    sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
    buffer_attr: protocol::stream::BufferAttr {
      pre_buffering: 0,
      ..Default::default()
    },
    ..Default::default()
  })
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
  headers
    .get(name)
    .and_then(|value| value.to_str().ok())
    .map(|value| value.to_owned())
}

fn is_hls(url: &str, content_type: Option<&str>) -> bool {
  let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
  if path.ends_with(".m3u8") {
    return true;
  }
  let Some(content_type) = content_type else {
    return false;
  };
  let mime = content_type
    .split(';')
    .next()
    .unwrap_or(content_type)
    .trim()
    .to_ascii_lowercase();
  HLS_TYPES.contains(&mime.as_str())
}

fn extension(url: &str) -> Option<String> {
  let path = url.split(['?', '#']).next().unwrap_or(url);
  let name = path.rsplit('/').next()?;
  let (_, extension) = name.rsplit_once('.')?;
  (!extension.is_empty() && extension.len() <= 5).then(|| extension.to_ascii_lowercase())
}

fn millis(time: Time) -> u32 {
  u32::try_from(time.as_millis().max(0)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
  use std::{io::Write, net::TcpListener};

  use bridgething_companion::backend::StreamEvent;
  use tokio::sync::mpsc::UnboundedReceiver;

  use super::*;

  const RATE: u32 = 44_100;
  const FRAMES: usize = RATE as usize / 2;

  fn wav() -> Vec<u8> {
    let data = (FRAMES * SAMPLE_BYTES) as u32;
    let mut out = Vec::with_capacity(data as usize + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for frame in 0..FRAMES {
      let value = ((frame as f32 * 0.05).sin() * 8_000.0) as i16;
      out.extend_from_slice(&value.to_le_bytes());
    }
    out
  }

  fn id3_frame(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut frame = id.to_vec();
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(body);
    frame
  }

  fn id3_text(id: &[u8; 4], said: &str) -> Vec<u8> {
    let mut body = vec![3u8];
    body.extend_from_slice(said.as_bytes());
    id3_frame(id, &body)
  }

  fn id3_tagged(audio: &[u8], artwork: &[u8]) -> Vec<u8> {
    let mut frames = id3_text(b"TIT2", "Watussi");
    frames.extend_from_slice(&id3_text(b"TPE1", "Harmonia"));
    frames.extend_from_slice(&id3_text(b"TALB", "Musik von Harmonia"));
    let mut apic = vec![0u8];
    apic.extend_from_slice(b"image/png\0");
    apic.push(3);
    apic.push(0);
    apic.extend_from_slice(artwork);
    frames.extend_from_slice(&id3_frame(b"APIC", &apic));
    let size = frames.len();
    let mut out = b"ID3".to_vec();
    out.extend_from_slice(&[3, 0, 0]);
    out.extend_from_slice(&[
      ((size >> 21) & 0x7f) as u8,
      ((size >> 14) & 0x7f) as u8,
      ((size >> 7) & 0x7f) as u8,
      (size & 0x7f) as u8,
    ]);
    out.extend_from_slice(&frames);
    out.extend_from_slice(audio);
    out
  }

  fn serve(head: String, body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();
    thread::spawn(move || {
      if let Ok((mut socket, _)) = listener.accept() {
        let mut request = [0u8; 2048];
        let _ = socket.read(&mut request);
        let _ = socket.write_all(head.as_bytes());
        let _ = socket.write_all(&body);
        let _ = socket.flush();
      }
    });
    format!("http://127.0.0.1:{port}/audio")
  }

  fn sized(content_type: &str, body: Vec<u8>) -> String {
    let head = format!(
      "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
      body.len()
    );
    serve(head, body)
  }

  fn drained(shared: &Arc<Shared>) -> usize {
    shared.pcm.state.lock().unwrap().data.len()
  }

  fn events(inbox: &mut UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Ok(event) = inbox.try_recv() {
      events.push(event);
    }
    events
  }

  fn settled(inbox: &mut UnboundedReceiver<StreamEvent>, patience: Duration) -> Vec<StreamEvent> {
    let deadline = Instant::now() + patience;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
      seen.extend(events(inbox));
      if seen.iter().any(|event| {
        matches!(
          event,
          StreamEvent::Status(StreamStatus::Ended) | StreamEvent::Status(StreamStatus::Failed { .. })
        )
      }) {
        break;
      }
      thread::sleep(Duration::from_millis(20));
    }
    seen
  }

  fn statuses(seen: &[StreamEvent]) -> Vec<StreamStatus> {
    seen
      .iter()
      .filter_map(|event| match event {
        StreamEvent::Status(status) => Some(status.clone()),
        _ => None,
      })
      .collect()
  }

  fn plain(url: String) -> StreamSource {
    StreamSource {
      url,
      live: false,
      station: None,
    }
  }

  fn titles(seen: &[StreamEvent]) -> Vec<String> {
    seen
      .iter()
      .filter_map(|event| match event {
        StreamEvent::Metadata(metadata) => metadata.title.clone(),
        _ => None,
      })
      .collect()
  }

  fn fetched(url: String) -> (Arc<Shared>, Vec<StreamEvent>) {
    fetched_from(plain(url))
  }

  fn fetched_from(stream: StreamSource) -> (Arc<Shared>, Vec<StreamEvent>) {
    let (sink, mut inbox) = StreamSink::channel();
    let shared = Arc::new(Shared::new(sink));
    source(Arc::clone(&shared), stream);
    let seen = events(&mut inbox);
    (shared, seen)
  }

  #[test]
  fn a_progressive_body_decodes_whole_with_a_duration_and_real_seeking() {
    let (shared, _) = fetched(sized("audio/wav", wav()));

    assert_eq!(shared.take_failure(), None);
    let format = shared.format.lock().unwrap().expect("the decoder published a format");
    assert_eq!(format.rate, RATE);
    assert_eq!(format.channels, 1);
    assert_eq!(shared.duration_ms.load(Ordering::SeqCst), 500);
    assert!(shared.seekable.load(Ordering::SeqCst), "a sized body seeks for real");
    assert_eq!(drained(&shared), FRAMES * SAMPLE_BYTES);
  }

  #[test]
  fn embedded_tags_and_cover_art_are_reported_before_the_first_sample() {
    let (shared, seen) = fetched(sized("audio/mpeg", id3_tagged(&wav(), b"cover-png")));

    assert_eq!(shared.take_failure(), None);
    let tagged = seen
      .iter()
      .find_map(|event| match event {
        StreamEvent::Metadata(metadata) if metadata.artwork.is_some() => Some(metadata.clone()),
        _ => None,
      })
      .expect("the tag reaches the sink");
    assert_eq!(tagged.title.as_deref(), Some("Watussi"));
    assert_eq!(tagged.artist.as_deref(), Some("Harmonia"));
    assert_eq!(tagged.album.as_deref(), Some("Musik von Harmonia"));
    assert_eq!(tagged.artwork.as_deref(), Some(&b"cover-png"[..]));
    assert_eq!(drained(&shared), FRAMES * SAMPLE_BYTES);
  }

  #[test]
  fn a_station_named_by_the_source_titles_a_body_whose_origin_never_names_itself() {
    let stream = StreamSource {
      url: sized("audio/wav", wav()),
      live: true,
      station: Some("Groove Salad".to_owned()),
    };
    let (shared, seen) = fetched_from(stream);

    assert_eq!(shared.take_failure(), None);
    assert_eq!(titles(&seen), vec!["Groove Salad".to_owned()]);
  }

  #[test]
  fn a_playlist_url_or_content_type_is_recognised_as_hls() {
    assert!(is_hls("https://example/live/master.m3u8", None));
    assert!(is_hls("https://example/live/master.M3U8?token=1", None));
    assert!(is_hls(
      "https://example/live/stream",
      Some("application/vnd.apple.mpegurl")
    ));
    assert!(is_hls(
      "https://example/live/stream",
      Some("audio/x-mpegurl; charset=utf-8")
    ));
    assert!(!is_hls("https://example/live/stream.mp3", Some("audio/mpeg")));
    assert!(!is_hls("https://example/live/stream", None));
  }

  #[test]
  fn a_playlist_content_type_is_refused_before_anything_is_decoded() {
    let (shared, _) = fetched(sized("application/vnd.apple.mpegurl", b"#EXTM3U\n".to_vec()));

    assert_eq!(
      shared.take_failure().as_deref(),
      Some("hls is not supported on desktop")
    );
    assert!(shared.format.lock().unwrap().is_none());
  }

  #[test]
  fn a_refused_request_carries_its_status_into_the_failure() {
    let url = serve(
      "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_owned(),
      Vec::new(),
    );
    let (shared, _) = fetched(url);

    assert_eq!(shared.take_failure().as_deref(), Some("the stream answered http 404"));
  }

  #[test]
  fn a_body_that_is_not_audio_fails_to_decode() {
    let (shared, _) = fetched(sized("audio/wav", b"not audio at all".to_vec()));

    let failure = shared.take_failure().expect("undecodable bytes fail");
    assert!(failure.contains("could not be decoded"), "{failure}");
  }

  #[test]
  fn playing_a_stream_reports_a_truthful_status_sequence() {
    let (sink, mut inbox) = StreamSink::channel();
    let backend = PulseStream::default();
    backend.play(plain(sized("audio/wav", wav())), sink);
    let seen = settled(&mut inbox, Duration::from_secs(20));
    backend.stop();

    let statuses = statuses(&seen);
    assert_eq!(statuses.first(), Some(&StreamStatus::Buffering));

    if pulseaudio::socket_path_from_env().is_none() {
      let Some(StreamStatus::Failed { reason }) = statuses.last() else {
        panic!("without a sound server the stream has to fail: {statuses:?}");
      };
      assert!(reason.contains("pulseaudio"), "{reason}");
      return;
    }

    assert!(
      statuses.contains(&StreamStatus::Playing),
      "the sink reports playing once the server starts: {seen:?}"
    );
    assert_eq!(statuses.last(), Some(&StreamStatus::Ended), "{statuses:?}");
    let timings: Vec<StreamTiming> = seen
      .iter()
      .filter_map(|event| match event {
        StreamEvent::Timing(timing) => Some(*timing),
        _ => None,
      })
      .collect();
    assert!(timings.iter().all(|timing| timing.seekable));
    assert!(timings.iter().all(|timing| timing.duration_ms == Some(500)));
  }
}
