use std::{
  ptr::NonNull,
  sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering},
  },
  time::Duration,
};

use block2::RcBlock;
use bridgething_companion::backend::{
  StreamBackend, StreamMetadata, StreamPresentation, StreamSink, StreamSource, StreamStatus, StreamTiming,
};
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::{
  AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
  rc::Retained,
  runtime::{AnyObject, Bool, ProtocolObject},
};
use objc2_av_foundation::{
  AVAsset, AVMetadataCommonIdentifierAlbumName, AVMetadataCommonIdentifierArtist, AVMetadataCommonIdentifierArtwork,
  AVMetadataCommonIdentifierTitle, AVMetadataCommonKeyAlbumName, AVMetadataCommonKeyArtist, AVMetadataCommonKeyArtwork,
  AVMetadataCommonKeyTitle, AVMetadataIdentifierIcyMetadataStreamTitle, AVMetadataItem, AVPlayer, AVPlayerItem,
  AVPlayerItemDidPlayToEndTimeNotification, AVPlayerItemFailedToPlayToEndTimeErrorKey,
  AVPlayerItemFailedToPlayToEndTimeNotification, AVPlayerItemMetadataOutput, AVPlayerItemMetadataOutputPushDelegate,
  AVPlayerItemOutputPushDelegate, AVPlayerItemStatus, AVPlayerItemTrack, AVPlayerTimeControlStatus,
  AVTimedMetadataGroup,
};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_foundation::{
  NSArray, NSError, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSString, NSURL,
};

const APP_BUNDLE: &str = "com.bridgething.desktop";
const POLL: Duration = Duration::from_millis(250);
const TIMING_EVERY: u64 = 4;

pub struct AvPlayerStream {
  inner: Arc<Inner>,
}

impl AvPlayerStream {
  pub fn new() -> Self {
    Self {
      inner: Arc::new(Inner::default()),
    }
  }
}

impl StreamBackend for AvPlayerStream {
  fn app_bundle(&self) -> String {
    APP_BUNDLE.to_owned()
  }

  fn play(&self, source: StreamSource, sink: Arc<StreamSink>) {
    self.inner.on_main(move |inner, mtm| inner.start(mtm, source, sink));
  }

  fn present(&self, _presentation: StreamPresentation) {}

  fn pause(&self) {
    self.inner.on_main(|inner, mtm| {
      inner.with_player(mtm, |player| unsafe { player.pause() });
      inner.sample(mtm, 0);
    });
  }

  fn resume(&self) {
    self.inner.on_main(|inner, mtm| {
      inner.with_player(mtm, |player| unsafe { player.play() });
      inner.sample(mtm, 0);
    });
  }

  fn seek_to(&self, position_ms: u32) {
    self.inner.on_main(move |inner, mtm| {
      let held = inner.session.lock().unwrap();
      let Some(bound) = held.as_ref() else { return };
      let session = bound.get(mtm);
      if unsafe { duration_ms(&session.item, session.live) }.is_none() {
        return;
      }
      let landed = {
        let inner = Arc::downgrade(inner);
        RcBlock::new(move |_: Bool| {
          let Some(inner) = inner.upgrade() else { return };
          inner.on_main(|inner, mtm| inner.sample(mtm, 0));
        })
      };
      unsafe {
        session.player.seekToTime_completionHandler(
          CMTime {
            value: i64::from(position_ms),
            timescale: 1000,
            flags: CMTimeFlags::Valid,
            epoch: 0,
          },
          &landed,
        );
      }
    });
  }

  fn stop(&self) {
    self.inner.on_main(|inner, mtm| inner.teardown(mtm));
  }
}

#[derive(Default)]
struct Inner {
  session: Mutex<Option<MainThreadBound<Session>>>,
  playing: Mutex<Option<Playing>>,
  epoch: AtomicU64,
}

struct Playing {
  sink: Arc<StreamSink>,
  status: Option<StreamStatus>,
  metadata: StreamMetadata,
  station: Option<String>,
  terminal: bool,
  common_read: bool,
}

struct Session {
  live: bool,
  player: Retained<AVPlayer>,
  item: Retained<AVPlayerItem>,
  output: Retained<AVPlayerItemMetadataOutput>,
  _delegate: Retained<MetadataRelay>,
  observers: Vec<Retained<AnyObject>>,
  _blocks: Vec<RcBlock<dyn Fn(NonNull<NSNotification>)>>,
}

impl Drop for Session {
  fn drop(&mut self) {
    unsafe {
      let center = NSNotificationCenter::defaultCenter();
      for observer in &self.observers {
        center.removeObserver(observer);
      }
      self.output.setDelegate_queue(None, None);
      self.item.removeOutput(&self.output);
      self.player.pause();
      self.player.replaceCurrentItemWithPlayerItem(None);
    }
  }
}

enum Observed {
  Failed(String),
  Live {
    status: StreamStatus,
    common: Option<StreamMetadata>,
    timing: Option<StreamTiming>,
  },
}

impl Inner {
  fn on_main(self: &Arc<Self>, work: impl FnOnce(&Arc<Self>, MainThreadMarker) + Send + 'static) {
    let inner = Arc::clone(self);
    DispatchQueue::main().exec_async(move || {
      let Some(mtm) = MainThreadMarker::new() else { return };
      work(&inner, mtm);
    });
  }

  fn start(self: &Arc<Self>, mtm: MainThreadMarker, source: StreamSource, sink: Arc<StreamSink>) {
    self.teardown(mtm);
    *self.playing.lock().unwrap() = Some(Playing {
      sink,
      status: None,
      metadata: StreamMetadata::default(),
      station: source.station,
      terminal: false,
      common_read: false,
    });

    let url = source.url;
    let Some(target) = NSURL::URLWithString(&NSString::from_str(&url)) else {
      self.finish(
        mtm,
        StreamStatus::Failed {
          reason: format!("not a playable url: {url}"),
        },
      );
      return;
    };

    let item = unsafe { AVPlayerItem::initWithURL(AVPlayerItem::alloc(mtm), &target) };
    let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
    let delegate = MetadataRelay::new(Arc::downgrade(self));
    let output = unsafe { AVPlayerItemMetadataOutput::initWithIdentifiers(AVPlayerItemMetadataOutput::alloc(), None) };
    unsafe {
      output.setDelegate_queue(Some(ProtocolObject::from_ref(&*delegate)), Some(DispatchQueue::main()));
      item.addOutput(&output);
    }

    let center = NSNotificationCenter::defaultCenter();
    let ended = {
      let inner = Arc::downgrade(self);
      RcBlock::new(move |_: NonNull<NSNotification>| {
        let Some(inner) = inner.upgrade() else { return };
        inner.on_main(|inner, mtm| inner.finish(mtm, StreamStatus::Ended));
      })
    };
    let cut_short = {
      let inner = Arc::downgrade(self);
      RcBlock::new(move |note: NonNull<NSNotification>| {
        let Some(inner) = inner.upgrade() else { return };
        let reason = failure_reason(unsafe { note.as_ref() });
        inner.on_main(move |inner, mtm| inner.finish(mtm, StreamStatus::Failed { reason }));
      })
    };
    let observers = unsafe {
      vec![
        Retained::cast_unchecked::<AnyObject>(center.addObserverForName_object_queue_usingBlock(
          Some(AVPlayerItemDidPlayToEndTimeNotification),
          Some(&item),
          None,
          &ended,
        )),
        Retained::cast_unchecked::<AnyObject>(center.addObserverForName_object_queue_usingBlock(
          Some(AVPlayerItemFailedToPlayToEndTimeNotification),
          Some(&item),
          None,
          &cut_short,
        )),
      ]
    };

    unsafe { player.play() };
    *self.session.lock().unwrap() = Some(MainThreadBound::new(
      Session {
        live: source.live,
        player,
        item,
        output,
        _delegate: delegate,
        observers,
        _blocks: vec![ended, cut_short],
      },
      mtm,
    ));

    let epoch = self.epoch.load(Ordering::SeqCst);
    self.announce_station();
    self.sample(mtm, 0);
    self.schedule(epoch, 1);
  }

  fn announce_station(&self) {
    let (sink, station) = {
      let held = self.playing.lock().unwrap();
      let Some(playing) = held.as_ref() else { return };
      let Some(station) = playing.station.clone() else { return };
      (Arc::clone(&playing.sink), station)
    };
    sink.on_metadata(StreamMetadata {
      title: Some(station),
      ..StreamMetadata::default()
    });
  }

  fn schedule(self: &Arc<Self>, epoch: u64, tick: u64) {
    if self.epoch.load(Ordering::SeqCst) != epoch {
      return;
    }
    let inner = Arc::clone(self);
    let when = DispatchTime::try_from(POLL).unwrap_or(DispatchTime::NOW);
    let _ = DispatchQueue::main().after(when, move || {
      let Some(mtm) = MainThreadMarker::new() else { return };
      if inner.epoch.load(Ordering::SeqCst) != epoch {
        return;
      }
      inner.sample(mtm, tick);
      inner.schedule(epoch, tick.wrapping_add(1));
    });
  }

  fn sample(self: &Arc<Self>, mtm: MainThreadMarker, tick: u64) {
    let want_common = self
      .playing
      .lock()
      .unwrap()
      .as_ref()
      .is_some_and(|playing| !playing.common_read);
    let observed = {
      let held = self.session.lock().unwrap();
      let Some(bound) = held.as_ref() else { return };
      observe(bound.get(mtm), tick, want_common)
    };
    match observed {
      Observed::Failed(reason) => self.finish(mtm, StreamStatus::Failed { reason }),
      Observed::Live { status, common, timing } => {
        if let Some(common) = common {
          self.take_common(common);
        }
        self.report_status(status);
        if let Some(timing) = timing {
          self.report_timing(timing);
        }
      }
    }
  }

  fn with_player(&self, mtm: MainThreadMarker, work: impl FnOnce(&AVPlayer)) {
    let held = self.session.lock().unwrap();
    if let Some(bound) = held.as_ref() {
      work(&bound.get(mtm).player);
    }
  }

  fn finish(self: &Arc<Self>, mtm: MainThreadMarker, status: StreamStatus) {
    self.report_status(status);
    self.teardown(mtm);
  }

  fn teardown(&self, mtm: MainThreadMarker) {
    self.epoch.fetch_add(1, Ordering::SeqCst);
    let session = self.session.lock().unwrap().take();
    if let Some(session) = session {
      drop(session.into_inner(mtm));
    }
    let playing = self.playing.lock().unwrap().take();
    drop(playing);
  }

  fn report_status(&self, status: StreamStatus) {
    let sink = {
      let mut held = self.playing.lock().unwrap();
      let Some(playing) = held.as_mut() else { return };
      if playing.terminal || playing.status.as_ref() == Some(&status) {
        return;
      }
      playing.terminal = matches!(status, StreamStatus::Ended | StreamStatus::Failed { .. });
      playing.status = Some(status.clone());
      Arc::clone(&playing.sink)
    };
    sink.on_status(status);
  }

  fn report_metadata(&self, apply: impl FnOnce(&mut StreamMetadata)) {
    let (sink, next) = {
      let mut held = self.playing.lock().unwrap();
      let Some(playing) = held.as_mut() else { return };
      if playing.terminal {
        return;
      }
      let mut next = playing.metadata.clone();
      apply(&mut next);
      if next == playing.metadata {
        return;
      }
      playing.metadata = next.clone();
      if next.title.is_none() {
        next.title = playing.station.clone();
      }
      (Arc::clone(&playing.sink), next)
    };
    sink.on_metadata(next);
  }

  fn report_timing(&self, timing: StreamTiming) {
    let sink = {
      let held = self.playing.lock().unwrap();
      let Some(playing) = held.as_ref() else { return };
      if playing.terminal {
        return;
      }
      Arc::clone(&playing.sink)
    };
    sink.on_timing(timing);
  }

  fn take_common(&self, common: StreamMetadata) {
    if let Some(playing) = self.playing.lock().unwrap().as_mut() {
      playing.common_read = true;
    }
    self.report_metadata(move |metadata| {
      metadata.title = metadata.title.take().or(common.title);
      metadata.artist = metadata.artist.take().or(common.artist);
      metadata.album = metadata.album.take().or(common.album);
      metadata.artwork = metadata.artwork.take().or(common.artwork);
    });
  }

  fn ingest(self: &Arc<Self>, groups: &NSArray<AVTimedMetadataGroup>) {
    self.report_metadata(|metadata| {
      for group in groups {
        for entry in unsafe { group.items() }.iter() {
          let Some(identifier) = (unsafe { entry.identifier() }) else {
            continue;
          };
          if same(&identifier, unsafe { AVMetadataCommonIdentifierArtwork }) {
            metadata.artwork = artwork(&entry);
            continue;
          }
          let Some(value) = text(&entry) else { continue };
          if same(&identifier, unsafe { AVMetadataIdentifierIcyMetadataStreamTitle })
            || same(&identifier, unsafe { AVMetadataCommonIdentifierTitle })
          {
            metadata.title = Some(value);
          } else if same(&identifier, unsafe { AVMetadataCommonIdentifierArtist }) {
            metadata.artist = Some(value);
          } else if same(&identifier, unsafe { AVMetadataCommonIdentifierAlbumName }) {
            metadata.album = Some(value);
          }
        }
      }
    });
  }
}

fn observe(session: &Session, tick: u64, want_common: bool) -> Observed {
  unsafe {
    let item = &*session.item;
    let status = item.status();
    if status == AVPlayerItemStatus::Failed {
      return Observed::Failed(
        item
          .error()
          .map(|error| error.localizedDescription().to_string())
          .unwrap_or_else(|| "playback failed".to_owned()),
      );
    }

    let control = session.player.timeControlStatus();
    let reported = if control == AVPlayerTimeControlStatus::Playing {
      StreamStatus::Playing
    } else if control == AVPlayerTimeControlStatus::Paused {
      StreamStatus::Paused
    } else {
      StreamStatus::Buffering
    };

    let duration = duration_ms(item, session.live);
    let timing = tick.is_multiple_of(TIMING_EVERY).then(|| StreamTiming {
      position_ms: millis(item.currentTime()).unwrap_or(0),
      duration_ms: duration,
      seekable: duration.is_some(),
    });
    let common = (want_common && status == AVPlayerItemStatus::ReadyToPlay).then(|| common_metadata(&item.asset()));

    Observed::Live {
      status: reported,
      common,
      timing,
    }
  }
}

unsafe fn duration_ms(item: &AVPlayerItem, live: bool) -> Option<u32> {
  if live {
    return None;
  }
  millis(unsafe { item.duration() }).filter(|duration| *duration > 0)
}

fn millis(time: CMTime) -> Option<u32> {
  let seconds = unsafe { time.seconds() };
  if !seconds.is_finite() {
    return None;
  }
  Some((seconds * 1000.0).round().clamp(0.0, f64::from(u32::MAX)) as u32)
}

fn common_metadata(asset: &AVAsset) -> StreamMetadata {
  let mut found = StreamMetadata::default();
  for entry in unsafe { asset.commonMetadata() }.iter() {
    let Some(key) = (unsafe { entry.commonKey() }) else {
      continue;
    };
    if same(&key, unsafe { AVMetadataCommonKeyArtwork }) {
      found.artwork = artwork(&entry);
      continue;
    }
    let Some(value) = text(&entry) else { continue };
    if same(&key, unsafe { AVMetadataCommonKeyTitle }) {
      found.title = Some(value);
    } else if same(&key, unsafe { AVMetadataCommonKeyArtist }) {
      found.artist = Some(value);
    } else if same(&key, unsafe { AVMetadataCommonKeyAlbumName }) {
      found.album = Some(value);
    }
  }
  found
}

fn artwork(entry: &AVMetadataItem) -> Option<Vec<u8>> {
  unsafe { entry.dataValue() }
    .map(|data| data.to_vec())
    .filter(|bytes| !bytes.is_empty())
}

fn text(entry: &AVMetadataItem) -> Option<String> {
  unsafe { entry.stringValue() }
    .map(|value| value.to_string())
    .filter(|value| !value.trim().is_empty())
}

fn same(held: &NSString, wanted: Option<&NSString>) -> bool {
  wanted.is_some_and(|wanted| held.isEqualToString(wanted))
}

fn failure_reason(note: &NSNotification) -> String {
  note
    .userInfo()
    .and_then(|info| info.objectForKey(unsafe { AVPlayerItemFailedToPlayToEndTimeErrorKey }))
    .and_then(|value| value.downcast::<NSError>().ok())
    .map(|error| error.localizedDescription().to_string())
    .unwrap_or_else(|| "the stream ended early".to_owned())
}

struct RelayState {
  inner: Weak<Inner>,
}

define_class!(
  #[unsafe(super(NSObject))]
  #[ivars = RelayState]
  struct MetadataRelay;

  unsafe impl NSObjectProtocol for MetadataRelay {}

  unsafe impl AVPlayerItemOutputPushDelegate for MetadataRelay {}

  unsafe impl AVPlayerItemMetadataOutputPushDelegate for MetadataRelay {
    #[unsafe(method(metadataOutput:didOutputTimedMetadataGroups:fromPlayerItemTrack:))]
    fn did_output(
      &self,
      _output: &AVPlayerItemMetadataOutput,
      groups: &NSArray<AVTimedMetadataGroup>,
      _track: Option<&AVPlayerItemTrack>,
    ) {
      let Some(inner) = self.ivars().inner.upgrade() else {
        return;
      };
      inner.ingest(groups);
    }
  }
);

unsafe impl Send for MetadataRelay {}
unsafe impl Sync for MetadataRelay {}

impl MetadataRelay {
  fn new(inner: Weak<Inner>) -> Retained<Self> {
    let this = Self::alloc().set_ivars(RelayState { inner });
    unsafe { msg_send![super(this), init] }
  }
}

#[cfg(test)]
mod tests {
  use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::atomic::AtomicBool,
    time::Instant,
  };

  use bridgething_companion::backend::StreamEvent;

  use super::*;

  const TONE_MS: u32 = 1_000;

  fn tone(millis: u32) -> Vec<u8> {
    let rate = 44_100u32;
    let frames = rate * millis / 1_000;
    let bytes = frames * 2;
    let mut wav = Vec::with_capacity(44 + bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for frame in 0..frames {
      let phase = frame as f32 / rate as f32 * 440.0 * std::f32::consts::TAU;
      wav.extend_from_slice(&((phase.sin() * 6_000.0) as i16).to_le_bytes());
    }
    wav
  }

  fn serve(body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();
    let body = Arc::new(body);
    std::thread::spawn(move || {
      for accepted in listener.incoming() {
        let Ok(stream) = accepted else { continue };
        let body = Arc::clone(&body);
        std::thread::spawn(move || answer(stream, &body));
      }
    });
    format!("http://127.0.0.1:{port}/tone.wav")
  }

  fn answer(mut stream: TcpStream, body: &[u8]) {
    let mut buffer = [0u8; 4096];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
    let total = body.len();
    let wanted = request
      .lines()
      .find_map(|line| line.strip_prefix("Range: bytes="))
      .map(|range| {
        let (from, to) = range.split_once('-').unwrap_or((range, ""));
        let from = from.trim().parse::<usize>().unwrap_or(0).min(total);
        let to = to.trim().parse::<usize>().unwrap_or(total - 1).min(total - 1);
        from..=to
      });
    let head = match &wanted {
      Some(range) => format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/wav\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{total}\r\nContent-Length: {}\r\n\r\n",
        range.start(),
        range.end(),
        range.end() - range.start() + 1
      ),
      None => {
        format!("HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nAccept-Ranges: bytes\r\nContent-Length: {total}\r\n\r\n")
      }
    };
    let _ = stream.write_all(head.as_bytes());
    if !request.starts_with("HEAD") {
      let served = match wanted {
        Some(range) => &body[*range.start()..=*range.end()],
        None => body,
      };
      let _ = stream.write_all(served);
    }
    let _ = stream.flush();
  }

  fn main_queue_drains() -> bool {
    let seen = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&seen);
    DispatchQueue::main().exec_async(move || flag.store(true, Ordering::SeqCst));
    for _ in 0..40 {
      if seen.load(Ordering::SeqCst) {
        return true;
      }
      std::thread::sleep(Duration::from_millis(25));
    }
    false
  }

  #[test]
  fn a_served_wav_reaches_playing_then_ends_with_a_finite_duration() {
    if !main_queue_drains() {
      println!(
        "skipped: AVPlayer only advances while the process main thread drains the main dispatch queue, and the \
         libtest harness never runs a test body on the main thread"
      );
      return;
    }

    let backend = AvPlayerStream::new();
    let (sink, mut events) = StreamSink::channel();
    backend.play(
      StreamSource {
        url: serve(tone(TONE_MS)),
        live: false,
        station: None,
      },
      sink,
    );

    let mut statuses = Vec::new();
    let mut duration = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
      match events.try_recv() {
        Ok(StreamEvent::Status(status)) => {
          let ended = status == StreamStatus::Ended;
          statuses.push(status);
          if ended {
            break;
          }
        }
        Ok(StreamEvent::Timing(timing)) => duration = duration.or(timing.duration_ms),
        Ok(StreamEvent::Metadata(_)) => {}
        Err(_) => std::thread::sleep(Duration::from_millis(20)),
      }
    }
    backend.stop();

    assert!(
      !statuses
        .iter()
        .any(|status| matches!(status, StreamStatus::Failed { .. })),
      "the served wav played without a failure, got {statuses:?}"
    );
    assert!(
      statuses.contains(&StreamStatus::Playing),
      "the player reached Playing, got {statuses:?}"
    );
    assert_eq!(
      statuses.last(),
      Some(&StreamStatus::Ended),
      "the player ran to the end of the wav, got {statuses:?}"
    );
    let duration = duration.expect("timing carried a duration for the finite wav");
    assert!(
      duration.abs_diff(TONE_MS) < 200,
      "the reported duration tracks the served wav, got {duration}ms"
    );
  }
}
