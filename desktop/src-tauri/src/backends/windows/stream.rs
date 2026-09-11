use std::{
  sync::{
    Arc, Mutex,
    mpsc::{Receiver, RecvTimeoutError, SendError, Sender, channel},
  },
  thread,
  time::{Duration, Instant},
};

use bridgething_companion::backend::{
  StreamBackend, StreamMetadata, StreamPresentation, StreamSink, StreamSource, StreamStatus, StreamTiming,
};
use windows::{
  Foundation::{Collections::IVectorChangedEventArgs, TimeSpan, TypedEventHandler, Uri},
  Media::{
    Core::{DataCue, MediaCueEventArgs, MediaSource, TimedMetadataTrack},
    MediaPlaybackStatus, MediaPlaybackType,
    Playback::{
      AutoLoadedDisplayPropertyKind, MediaPlaybackItem, MediaPlaybackSession, MediaPlaybackState, MediaPlayer,
      MediaPlayerFailedEventArgs, TimedMetadataTrackPresentationMode,
    },
    Streaming::Adaptive::{AdaptiveMediaSource, AdaptiveMediaSourceCreationStatus},
    SystemMediaTransportControls, SystemMediaTransportControlsTimelineProperties,
  },
  Storage::Streams::{DataReader, RandomAccessStreamReference},
  Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
  core::{Error, HSTRING, IInspectable, Interface, Ref, RuntimeType},
};

const BUNDLE: &str = "com.bridgething.desktop";
const TICK: Duration = Duration::from_secs(1);
const TICKS_PER_MS: i64 = 10_000;
const UNSAID: &str = "windows would not say why the stream stopped";
const THUMBNAIL_LIMIT: u64 = 4 * 1024 * 1024;

enum Task {
  Play {
    source: StreamSource,
    sink: Arc<StreamSink>,
  },
  Pause,
  Resume,
  Seek(u32),
  Stop,
  Shutdown,
  Opened(u64),
  State(u64, MediaPlaybackState),
  Ended(u64),
  Failed(u64, String),
  Tracks(u64),
  Cue(u64, StreamMetadata),
}

#[derive(Default)]
pub struct MediaPlayerStream {
  engine: Mutex<Option<Sender<Task>>>,
}

impl MediaPlayerStream {
  fn post(&self, task: Task) {
    let held = self.engine.lock().unwrap();
    if let Some(tasks) = held.as_ref() {
      let _ = tasks.send(task);
    }
  }
}

impl Drop for MediaPlayerStream {
  fn drop(&mut self) {
    if let Some(tasks) = self.engine.lock().unwrap().take() {
      let _ = tasks.send(Task::Shutdown);
    }
  }
}

impl StreamBackend for MediaPlayerStream {
  fn app_bundle(&self) -> String {
    BUNDLE.to_owned()
  }

  fn present(&self, _presentation: StreamPresentation) {}

  fn play(&self, source: StreamSource, sink: Arc<StreamSink>) {
    let mut held = self.engine.lock().unwrap();
    let task = Task::Play { source, sink };
    let task = match held.as_ref() {
      Some(tasks) => match tasks.send(task) {
        Ok(()) => return,
        Err(SendError(task)) => task,
      },
      None => task,
    };

    let (tasks, rx) = channel();
    let wake = tasks.clone();
    match thread::Builder::new()
      .name("bridgething-stream".to_owned())
      .spawn(move || run(wake, rx))
    {
      Ok(_) => {
        let _ = tasks.send(task);
        *held = Some(tasks);
      }
      Err(error) => {
        *held = None;
        tracing::warn!(%error, "the stream player could not be started");
        if let Task::Play { sink, .. } = task {
          sink.on_status(StreamStatus::Failed {
            reason: "this desktop could not start a player thread".to_owned(),
          });
        }
      }
    }
  }

  fn pause(&self) {
    self.post(Task::Pause);
  }

  fn resume(&self) {
    self.post(Task::Resume);
  }

  fn seek_to(&self, position_ms: u32) {
    self.post(Task::Seek(position_ms));
  }

  fn stop(&self) {
    self.post(Task::Stop);
  }
}

fn run(wake: Sender<Task>, tasks: Receiver<Task>) {
  // SAFETY: after every winrt object is dropped
  let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
  let mut engine = Engine {
    wake,
    live: None,
    generation: 0,
  };

  let mut next = Instant::now() + TICK;
  loop {
    match tasks.recv_timeout(next.saturating_duration_since(Instant::now())) {
      Ok(task) => {
        if !engine.handle(task) {
          break;
        }
      }
      Err(RecvTimeoutError::Disconnected) => break,
      Err(RecvTimeoutError::Timeout) => {}
    }
    let now = Instant::now();
    if now >= next {
      next = now + TICK;
      engine.tick();
    }
  }

  engine.teardown();
  // SAFETY: after every winrt object is dropped.
  unsafe { CoUninitialize() };
}

struct Engine {
  wake: Sender<Task>,
  live: Option<Live>,
  generation: u64,
}

impl Engine {
  fn handle(&mut self, task: Task) -> bool {
    match task {
      Task::Play { source, sink } => self.start(source, sink),
      Task::Pause => self.drive(|live| live.player.Pause()),
      Task::Resume => self.drive(|live| live.player.Play()),
      Task::Seek(position_ms) => self.drive(|live| match live.source.live {
        true => Ok(()),
        false => live.session.SetPosition(ticks(position_ms)),
      }),
      Task::Stop => self.teardown(),
      Task::Opened(generation) => self.opened(generation),
      Task::State(generation, state) => {
        if let Some(status) = status_of(state) {
          self.publish(generation, status);
        }
      }
      Task::Ended(generation) => self.finish(generation, StreamStatus::Ended),
      Task::Failed(generation, reason) => self.finish(generation, StreamStatus::Failed { reason }),
      Task::Tracks(generation) => self.scan(generation),
      Task::Cue(generation, metadata) => self.describe(generation, metadata),
      Task::Shutdown => return false,
    }
    true
  }

  fn start(&mut self, source: StreamSource, sink: Arc<StreamSink>) {
    self.teardown();
    self.generation += 1;
    sink.on_status(StreamStatus::Buffering);

    match open(&source, self.generation, &self.wake, &sink) {
      Ok(live) => {
        live.show(&StreamMetadata::default(), None);
        let named = live.source.station.clone();
        self.live = Some(live);
        if let Some(title) = named {
          self.describe(
            self.generation,
            StreamMetadata {
              title: Some(title),
              ..StreamMetadata::default()
            },
          );
        }
      }
      Err(error) => {
        tracing::warn!(%error, url = %source.url, "windows would not open the stream");
        sink.on_status(StreamStatus::Failed {
          reason: error.message(),
        });
      }
    }
  }

  fn drive(&self, verb: impl FnOnce(&Live) -> windows::core::Result<()>) {
    let Some(live) = self.live.as_ref().filter(|live| !live.finished) else {
      return;
    };
    if let Err(error) = verb(live) {
      tracing::warn!(%error, "the stream player refused a transport verb");
    }
  }

  fn opened(&mut self, generation: u64) {
    let wake = self.wake.clone();
    let Some(live) = self.live.as_mut().filter(|live| live.generation == generation) else {
      return;
    };
    let (metadata, thumbnail) = described(&live.item);
    live.show(&metadata, thumbnail);
    live.watch(&wake);
    if metadata == StreamMetadata::default() || live.metadata.as_ref() == Some(&metadata) {
      return;
    }
    live.metadata = Some(metadata.clone());
    live.sink.on_metadata(metadata);
  }

  fn scan(&mut self, generation: u64) {
    let wake = self.wake.clone();
    if let Some(live) = self.live.as_mut().filter(|live| live.generation == generation) {
      live.watch(&wake);
    }
  }

  fn describe(&mut self, generation: u64, metadata: StreamMetadata) {
    let Some(live) = self.live.as_mut().filter(|live| live.generation == generation) else {
      return;
    };
    if live.metadata.as_ref() == Some(&metadata) {
      return;
    }
    live.metadata = Some(metadata.clone());
    live.show(&metadata, None);
    live.sink.on_metadata(metadata);
  }

  fn publish(&mut self, generation: u64, status: StreamStatus) {
    let Some(live) = self
      .live
      .as_mut()
      .filter(|live| live.generation == generation && !live.finished)
    else {
      return;
    };
    if live.status.as_ref() == Some(&status) {
      return;
    }
    live.status = Some(status.clone());
    live.mirror(&status);
    live.sink.on_status(status);
  }

  fn finish(&mut self, generation: u64, status: StreamStatus) {
    self.publish(generation, status);
    if let Some(live) = self.live.as_mut().filter(|live| live.generation == generation) {
      live.finished = true;
    }
  }

  fn tick(&mut self) {
    let Some(live) = self.live.as_mut().filter(|live| !live.finished) else {
      return;
    };
    live.unscrub();
    let timing = timed(
      live.source.live,
      StreamTiming {
        position_ms: span_ms(live.session.Position()).unwrap_or_default(),
        duration_ms: span_ms(live.session.NaturalDuration()),
        seekable: live.session.CanSeek().unwrap_or_default(),
      },
    );
    if live.timing == Some(timing) {
      return;
    }
    live.timing = Some(timing);
    live.sink.on_timing(timing);
  }

  fn teardown(&mut self) {
    if let Some(mut live) = self.live.take() {
      live.release();
    }
  }
}

struct Live {
  generation: u64,
  player: MediaPlayer,
  item: MediaPlaybackItem,
  session: MediaPlaybackSession,
  controls: Option<SystemMediaTransportControls>,
  source: StreamSource,
  sink: Arc<StreamSink>,
  opened: Option<i64>,
  ended: Option<i64>,
  failed: Option<i64>,
  state: Option<i64>,
  tracks: Option<i64>,
  cues: Vec<(TimedMetadataTrack, i64)>,
  status: Option<StreamStatus>,
  metadata: Option<StreamMetadata>,
  timing: Option<StreamTiming>,
  finished: bool,
}

impl Live {
  fn watch(&mut self, wake: &Sender<Task>) {
    for (track, token) in self.cues.drain(..) {
      let _ = track.RemoveCueEntered(token);
    }
    let Ok(tracks) = self.item.TimedMetadataTracks() else {
      return;
    };
    for index in 0..tracks.Size().unwrap_or_default() {
      let Ok(track) = tracks.GetAt(index) else {
        continue;
      };
      let _ = tracks.SetPresentationMode(index, TimedMetadataTrackPresentationMode::ApplicationPresented);
      match track.CueEntered(&on_cue(wake, self.generation)) {
        Ok(token) => self.cues.push((track, token)),
        Err(error) => tracing::debug!(%error, "a timed metadata track would not hand over its cues"),
      }
    }
  }

  fn mirror(&self, status: &StreamStatus) {
    let Some(controls) = self.controls.as_ref() else {
      return;
    };
    let mirrored = match status {
      StreamStatus::Buffering => MediaPlaybackStatus::Changing,
      StreamStatus::Playing => MediaPlaybackStatus::Playing,
      StreamStatus::Paused => MediaPlaybackStatus::Paused,
      StreamStatus::Ended | StreamStatus::Failed { .. } => MediaPlaybackStatus::Stopped,
    };
    let _ = controls.SetPlaybackStatus(mirrored);
  }

  fn show(&self, metadata: &StreamMetadata, thumbnail: Option<RandomAccessStreamReference>) {
    let Some(controls) = self.controls.as_ref() else {
      return;
    };
    if let Err(error) = display(controls, metadata, thumbnail) {
      tracing::debug!(%error, "the windows media flyout would not take the stream metadata");
    }
    self.unscrub();
  }

  fn unscrub(&self) {
    let Some(controls) = self.controls.as_ref().filter(|_| self.source.live) else {
      return;
    };
    if let Err(error) = timeline(controls) {
      tracing::debug!(%error, "the windows media flyout would not drop its scrubber for a live stream");
    }
  }

  fn release(&mut self) {
    for (track, token) in self.cues.drain(..) {
      let _ = track.RemoveCueEntered(token);
    }
    if let Some(token) = self.opened.take() {
      let _ = self.player.RemoveMediaOpened(token);
    }
    if let Some(token) = self.ended.take() {
      let _ = self.player.RemoveMediaEnded(token);
    }
    if let Some(token) = self.failed.take() {
      let _ = self.player.RemoveMediaFailed(token);
    }
    if let Some(token) = self.state.take() {
      let _ = self.session.RemovePlaybackStateChanged(token);
    }
    if let Some(token) = self.tracks.take() {
      let _ = self.item.RemoveTimedMetadataTracksChanged(token);
    }
    let _ = self.player.Pause();
    if let Some(controls) = self.controls.take() {
      let _ = controls.DisplayUpdater().and_then(|updater| updater.ClearAll());
      let _ = controls.SetPlaybackStatus(MediaPlaybackStatus::Closed);
      let _ = controls.SetIsEnabled(false);
    }
    let _ = self.player.Close();
  }
}

fn open(
  source: &StreamSource,
  generation: u64,
  wake: &Sender<Task>,
  sink: &Arc<StreamSink>,
) -> windows::core::Result<Live> {
  let player = MediaPlayer::new()?;
  player.CommandManager()?.SetIsEnabled(false)?;
  player.SetAutoPlay(true)?;

  let item = MediaPlaybackItem::Create(&media_source(&source.url)?)?;
  let _ = item.SetAutoLoadedDisplayProperties(AutoLoadedDisplayPropertyKind::Music);
  let session = player.PlaybackSession()?;
  let controls = controls(&player);

  let opened = player
    .MediaOpened(&on_event::<MediaPlayer, IInspectable>(wake, move || {
      Task::Opened(generation)
    }))
    .ok();
  let ended = player
    .MediaEnded(&on_event::<MediaPlayer, IInspectable>(wake, move || {
      Task::Ended(generation)
    }))
    .ok();
  let failed = player.MediaFailed(&on_failed(wake, generation)).ok();
  let state = session.PlaybackStateChanged(&on_state(wake, generation)).ok();
  let tracks = item
    .TimedMetadataTracksChanged(&on_event::<MediaPlaybackItem, IVectorChangedEventArgs>(
      wake,
      move || Task::Tracks(generation),
    ))
    .ok();

  player.SetSource(&item)?;

  Ok(Live {
    generation,
    player,
    item,
    session,
    controls,
    source: source.clone(),
    sink: Arc::clone(sink),
    opened,
    ended,
    failed,
    state,
    tracks,
    cues: Vec::new(),
    status: Some(StreamStatus::Buffering),
    metadata: None,
    timing: None,
    finished: false,
  })
}

fn timed(live: bool, timing: StreamTiming) -> StreamTiming {
  match live {
    true => StreamTiming {
      position_ms: timing.position_ms,
      duration_ms: None,
      seekable: false,
    },
    false => timing,
  }
}

fn media_source(url: &str) -> windows::core::Result<MediaSource> {
  let uri = Uri::CreateUri(&HSTRING::from(url))?;
  match adaptive(url).then(|| manifest(&uri)).flatten() {
    Some(adapted) => MediaSource::CreateFromAdaptiveMediaSource(&adapted),
    None => MediaSource::CreateFromUri(&uri),
  }
}

fn manifest(uri: &Uri) -> Option<AdaptiveMediaSource> {
  let built = AdaptiveMediaSource::CreateFromUriAsync(uri).and_then(|pending| pending.join());
  let built = match built {
    Ok(built) => built,
    Err(error) => {
      tracing::debug!(%error, "the adaptive source could not be built; falling back to the plain uri");
      return None;
    }
  };
  match built.Status() {
    Ok(AdaptiveMediaSourceCreationStatus::Success) => built.MediaSource().ok(),
    status => {
      let status = status.map(|status| status.0).unwrap_or(-1);
      tracing::debug!(
        status,
        "the adaptive source refused the manifest; falling back to the plain uri"
      );
      None
    }
  }
}

fn controls(player: &MediaPlayer) -> Option<SystemMediaTransportControls> {
  let controls = match player.SystemMediaTransportControls() {
    Ok(controls) => controls,
    Err(error) => {
      tracing::debug!(%error, "this desktop has no media flyout to publish to");
      return None;
    }
  };
  let _ = controls.SetIsPlayEnabled(false);
  let _ = controls.SetIsPauseEnabled(false);
  let _ = controls.SetIsStopEnabled(false);
  let _ = controls.SetIsNextEnabled(false);
  let _ = controls.SetIsPreviousEnabled(false);
  let _ = controls.SetIsFastForwardEnabled(false);
  let _ = controls.SetIsRewindEnabled(false);
  let _ = controls.SetPlaybackStatus(MediaPlaybackStatus::Changing);
  let _ = controls.SetIsEnabled(true);
  Some(controls)
}

fn display(
  controls: &SystemMediaTransportControls,
  metadata: &StreamMetadata,
  thumbnail: Option<RandomAccessStreamReference>,
) -> windows::core::Result<()> {
  let updater = controls.DisplayUpdater()?;
  updater.SetType(MediaPlaybackType::Music)?;
  let music = updater.MusicProperties()?;
  music.SetTitle(&HSTRING::from(metadata.title.as_deref().unwrap_or_default()))?;
  music.SetArtist(&HSTRING::from(metadata.artist.as_deref().unwrap_or_default()))?;
  music.SetAlbumTitle(&HSTRING::from(metadata.album.as_deref().unwrap_or_default()))?;
  if let Some(thumbnail) = thumbnail {
    updater.SetThumbnail(&thumbnail)?;
  }
  updater.Update()
}

fn timeline(controls: &SystemMediaTransportControls) -> windows::core::Result<()> {
  let stilled = SystemMediaTransportControlsTimelineProperties::new()?;
  stilled.SetStartTime(ticks(0))?;
  stilled.SetEndTime(ticks(0))?;
  stilled.SetMinSeekTime(ticks(0))?;
  stilled.SetMaxSeekTime(ticks(0))?;
  stilled.SetPosition(ticks(0))?;
  controls.UpdateTimelineProperties(&stilled)
}

fn described(item: &MediaPlaybackItem) -> (StreamMetadata, Option<RandomAccessStreamReference>) {
  let Ok(properties) = item.GetDisplayProperties() else {
    return (StreamMetadata::default(), None);
  };
  let thumbnail = properties.Thumbnail().ok();
  let Ok(music) = properties.MusicProperties() else {
    return (StreamMetadata::default(), thumbnail);
  };
  let metadata = StreamMetadata {
    title: text(music.Title()),
    artist: text(music.Artist()).or_else(|| text(music.AlbumArtist())),
    album: text(music.AlbumTitle()),
    artwork_url: None,
    artwork: thumbnail.as_ref().and_then(|reference| thumbnail_bytes(reference).ok()),
  };
  (metadata, thumbnail)
}

fn thumbnail_bytes(reference: &RandomAccessStreamReference) -> windows::core::Result<Vec<u8>> {
  let stream = reference.OpenReadAsync()?.join()?;
  let size = stream.Size()?;
  if size == 0 || size > THUMBNAIL_LIMIT {
    return Err(Error::empty());
  }
  let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
  reader.LoadAsync(size as u32)?.join()?;
  let mut bytes = vec![0u8; reader.UnconsumedBufferLength()? as usize];
  reader.ReadBytes(&mut bytes)?;
  Ok(bytes)
}

fn on_event<S: RuntimeType + 'static, A: RuntimeType + 'static>(
  wake: &Sender<Task>,
  task: impl Fn() -> Task + Send + 'static,
) -> TypedEventHandler<S, A> {
  let wake = wake.clone();
  TypedEventHandler::new(move |_, _| {
    let _ = wake.send(task());
    Ok(())
  })
}

fn on_state(wake: &Sender<Task>, generation: u64) -> TypedEventHandler<MediaPlaybackSession, IInspectable> {
  let wake = wake.clone();
  TypedEventHandler::new(move |session: Ref<MediaPlaybackSession>, _| {
    if let Some(state) = session.as_ref().and_then(|session| session.PlaybackState().ok()) {
      let _ = wake.send(Task::State(generation, state));
    }
    Ok(())
  })
}

fn on_failed(wake: &Sender<Task>, generation: u64) -> TypedEventHandler<MediaPlayer, MediaPlayerFailedEventArgs> {
  let wake = wake.clone();
  TypedEventHandler::new(move |_, args: Ref<MediaPlayerFailedEventArgs>| {
    let _ = wake.send(Task::Failed(generation, failure(args.as_ref())));
    Ok(())
  })
}

fn on_cue(wake: &Sender<Task>, generation: u64) -> TypedEventHandler<TimedMetadataTrack, MediaCueEventArgs> {
  let wake = wake.clone();
  TypedEventHandler::new(move |_, args: Ref<MediaCueEventArgs>| {
    if let Some(metadata) = args.as_ref().and_then(cued) {
      let _ = wake.send(Task::Cue(generation, metadata));
    }
    Ok(())
  })
}

fn cued(args: &MediaCueEventArgs) -> Option<StreamMetadata> {
  let cue = args.Cue().ok()?.cast::<DataCue>().ok()?;
  id3(&payload(&cue).ok()?)
}

fn payload(cue: &DataCue) -> windows::core::Result<Vec<u8>> {
  let data = cue.Data()?;
  let mut bytes = vec![0u8; data.Length()? as usize];
  DataReader::FromBuffer(&data)?.ReadBytes(&mut bytes)?;
  Ok(bytes)
}

fn failure(args: Option<&MediaPlayerFailedEventArgs>) -> String {
  let Some(args) = args else {
    return UNSAID.to_owned();
  };
  let said = args.ErrorMessage().map(|said| said.to_string()).unwrap_or_default();
  if !said.is_empty() {
    return said;
  }
  match args.ExtendedErrorCode() {
    Ok(code) if code.is_err() => Error::from_hresult(code).message(),
    _ => UNSAID.to_owned(),
  }
}

fn status_of(state: MediaPlaybackState) -> Option<StreamStatus> {
  match state {
    MediaPlaybackState::Opening | MediaPlaybackState::Buffering => Some(StreamStatus::Buffering),
    MediaPlaybackState::Playing => Some(StreamStatus::Playing),
    MediaPlaybackState::Paused => Some(StreamStatus::Paused),
    _ => None,
  }
}

fn adaptive(url: &str) -> bool {
  let path = url.split(['?', '#']).next().unwrap_or_default().to_ascii_lowercase();
  path.ends_with(".m3u8") || path.ends_with(".mpd") || path.ends_with("/manifest")
}

fn ticks(position_ms: u32) -> TimeSpan {
  TimeSpan {
    Duration: i64::from(position_ms) * TICKS_PER_MS,
  }
}

fn span_ms(held: windows::core::Result<TimeSpan>) -> Option<u32> {
  u32::try_from(held.ok()?.Duration / TICKS_PER_MS)
    .ok()
    .filter(|ms| *ms > 0)
}

fn text(held: windows::core::Result<HSTRING>) -> Option<String> {
  let held = held.ok()?.to_string();
  (!held.is_empty()).then_some(held)
}

fn id3(bytes: &[u8]) -> Option<StreamMetadata> {
  if bytes.len() < 10 || &bytes[..3] != b"ID3" || bytes[3] < 3 {
    return None;
  }
  let syncsafe = bytes[3] >= 4;
  let end = (10 + syncsafe_len(&bytes[6..10])?).min(bytes.len());

  let mut found = StreamMetadata::default();
  let mut at = 10;
  while at + 10 <= end {
    let id = &bytes[at..at + 4];
    if id[0] == 0 {
      break;
    }
    let size = match syncsafe {
      true => syncsafe_len(&bytes[at + 4..at + 8])?,
      false => u32::from_be_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize,
    };
    let from = at + 10;
    let to = from.checked_add(size).filter(|to| *to <= end)?;
    let slot = match id {
      b"TIT2" => Some(&mut found.title),
      b"TPE1" => Some(&mut found.artist),
      b"TALB" => Some(&mut found.album),
      _ => None,
    };
    if let Some(slot) = slot {
      *slot = frame_text(&bytes[from..to]);
    }
    at = to;
  }

  (found != StreamMetadata::default()).then_some(found)
}

fn syncsafe_len(bytes: &[u8]) -> Option<usize> {
  bytes.iter().try_fold(0usize, |size, byte| {
    (byte & 0x80 == 0).then(|| (size << 7) | usize::from(*byte))
  })
}

fn frame_text(payload: &[u8]) -> Option<String> {
  let (encoding, body) = payload.split_first()?;
  let said = match encoding {
    0 => body.iter().map(|byte| char::from(*byte)).collect(),
    1 | 2 => utf16(body, *encoding == 1)?,
    3 => String::from_utf8_lossy(body).into_owned(),
    _ => return None,
  };
  let said = said.trim_end_matches('\0').trim();
  (!said.is_empty()).then(|| said.to_owned())
}

fn utf16(body: &[u8], marked: bool) -> Option<String> {
  let (body, little) = match (marked, body) {
    (true, [0xff, 0xfe, rest @ ..]) => (rest, true),
    (true, [0xfe, 0xff, rest @ ..]) => (rest, false),
    _ => (body, false),
  };
  let units = body.as_chunks::<2>().0.iter().map(|pair| match little {
    true => u16::from_le_bytes(*pair),
    false => u16::from_be_bytes(*pair),
  });
  char::decode_utf16(units)
    .collect::<std::result::Result<String, _>>()
    .ok()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn frame(id: &[u8; 4], said: &str) -> Vec<u8> {
    let mut body = vec![3u8];
    body.extend_from_slice(said.as_bytes());
    let mut frame = id.to_vec();
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&body);
    frame
  }

  fn tag(version: u8, frames: Vec<u8>) -> Vec<u8> {
    let mut bytes = b"ID3".to_vec();
    bytes.extend_from_slice(&[version, 0, 0]);
    let size = frames.len();
    bytes.extend_from_slice(&[
      ((size >> 21) & 0x7f) as u8,
      ((size >> 14) & 0x7f) as u8,
      ((size >> 7) & 0x7f) as u8,
      (size & 0x7f) as u8,
    ]);
    bytes.extend_from_slice(&frames);
    bytes
  }

  #[test]
  fn a_radio_cue_that_only_carries_a_title_reports_only_a_title() {
    let mut frames = frame(b"TIT2", "Ghosteen");
    frames.extend_from_slice(&[0, 0, 0, 0]);
    let parsed = id3(&tag(3, frames)).expect("the cue names a track");

    assert_eq!(parsed.title.as_deref(), Some("Ghosteen"));
    assert_eq!(parsed.artist, None);
    assert_eq!(parsed.album, None);
  }

  #[test]
  fn a_full_tag_is_read_out_field_by_field() {
    let mut frames = frame(b"TIT2", "Bright Horses");
    frames.extend_from_slice(&frame(b"TPE1", "Nick Cave"));
    frames.extend_from_slice(&frame(b"TALB", "Ghosteen"));
    let parsed = id3(&tag(3, frames)).expect("the cue names a track");

    assert_eq!(parsed.title.as_deref(), Some("Bright Horses"));
    assert_eq!(parsed.artist.as_deref(), Some("Nick Cave"));
    assert_eq!(parsed.album.as_deref(), Some("Ghosteen"));
  }

  #[test]
  fn a_version_four_tag_sizes_its_frames_seven_bits_at_a_time() {
    let long = "x".repeat(200);
    let mut body = vec![3u8];
    body.extend_from_slice(long.as_bytes());
    let size = body.len();
    let mut frames = b"TIT2".to_vec();
    frames.extend_from_slice(&[0, 0, ((size >> 7) & 0x7f) as u8, (size & 0x7f) as u8]);
    frames.extend_from_slice(&[0, 0]);
    frames.extend_from_slice(&body);

    let parsed = id3(&tag(4, frames)).expect("the cue names a track");
    assert_eq!(parsed.title.as_deref(), Some(long.as_str()));
  }

  #[test]
  fn a_utf16_title_survives_its_byte_order_mark() {
    let mut body = vec![1u8, 0xff, 0xfe];
    for unit in "Jubilee Street".encode_utf16() {
      body.extend_from_slice(&unit.to_le_bytes());
    }
    let mut frames = b"TIT2".to_vec();
    frames.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frames.extend_from_slice(&[0, 0]);
    frames.extend_from_slice(&body);

    let parsed = id3(&tag(3, frames)).expect("the cue names a track");
    assert_eq!(parsed.title.as_deref(), Some("Jubilee Street"));
  }

  #[test]
  fn a_cue_that_carries_no_tag_names_nothing() {
    assert_eq!(id3(b"not a tag at all"), None);
    assert_eq!(id3(&tag(3, vec![0; 10])), None);
  }

  #[test]
  fn a_playlist_url_is_opened_through_the_adaptive_source() {
    assert!(adaptive("https://example.com/live/master.m3u8"));
    assert!(adaptive("https://example.com/live/master.M3U8?token=abc"));
    assert!(adaptive("https://example.com/live/stream.mpd"));
    assert!(!adaptive("https://example.com/radio/stream.mp3"));
    assert!(!adaptive("https://example.com/radio?playlist=master.m3u8"));
  }

  #[test]
  fn a_live_stream_keeps_its_position_but_loses_its_scrubber() {
    let measured = StreamTiming {
      position_ms: 4_000,
      duration_ms: Some(64_800_000),
      seekable: true,
    };

    assert_eq!(
      timed(true, measured),
      StreamTiming {
        position_ms: 4_000,
        duration_ms: None,
        seekable: false,
      }
    );
    assert_eq!(timed(false, measured), measured);
  }

  #[test]
  fn a_live_stream_reports_no_duration_at_all() {
    assert_eq!(span_ms(Ok(TimeSpan { Duration: 0 })), None);
    assert_eq!(span_ms(Ok(TimeSpan { Duration: i64::MAX })), None);
    assert_eq!(
      span_ms(Ok(TimeSpan {
        Duration: 90 * TICKS_PER_MS
      })),
      Some(90)
    );
  }
}
