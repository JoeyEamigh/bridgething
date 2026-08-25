use std::{
  mem,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, RecvTimeoutError, Sender, channel},
  },
  thread,
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bridgething_companion::backend::{
  ImageScaler, MediaArt, MediaArtSink, MediaControl, MediaRepeatMode, MediaSessionBackend, MediaSessionInbox,
  MediaSessionSnapshot, MediaSnapshotSink,
};
use bridgething_delivery::bundle::fetch::sha256_hex;
use windows::{
  Foundation::{TimeSpan, TypedEventHandler},
  Media::{
    Control::{
      CurrentSessionChangedEventArgs, GlobalSystemMediaTransportControlsSession as Session,
      GlobalSystemMediaTransportControlsSessionManager as SessionManager,
      GlobalSystemMediaTransportControlsSessionMediaProperties as TrackProperties,
      GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus, MediaPropertiesChangedEventArgs,
      PlaybackInfoChangedEventArgs, SessionsChangedEventArgs, TimelinePropertiesChangedEventArgs,
    },
    MediaPlaybackAutoRepeatMode as AutoRepeatMode,
  },
  Storage::Streams::{DataReader, IRandomAccessStreamWithContentType},
  Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
  core::{HSTRING, IUnknown, Interface, RuntimeType},
};

use crate::backends::{jpeg, portable::PortableScaler};

const COALESCE: Duration = Duration::from_millis(180);
const TICKS_PER_MS: i64 = 10_000;
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
const MAX_ART_EDGE: u32 = 512;
const MAX_ART_BYTES: u64 = 8 << 20;
const ART_JPEG_QUALITY: f32 = 0.6;
const ART_MIME: &str = "image/jpeg";

type Wake = Arc<Mutex<Sender<Task>>>;
type Latest = Arc<Mutex<Vec<Seen>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Change {
  Playback,
  Track,
  Roster,
}

enum Task {
  Changed(Change),
  Control(String, MediaControl),
  Art(String, String, Arc<MediaArtSink>),
  Stop,
}

struct Art {
  package: String,
  props: TrackProperties,
  sink: Arc<MediaArtSink>,
}

struct Engine {
  tasks: Sender<Task>,
  granted: Arc<AtomicBool>,
  latest: Latest,
}

#[derive(Default)]
pub struct GlobalSystemMediaSessions {
  engine: Mutex<Option<Engine>>,
}

impl GlobalSystemMediaSessions {
  fn post(&self, task: Task) -> bool {
    let held = self.engine.lock().unwrap();
    held.as_ref().is_some_and(|engine| engine.tasks.send(task).is_ok())
  }
}

impl Drop for GlobalSystemMediaSessions {
  fn drop(&mut self) {
    self.stop();
  }
}

impl MediaSessionBackend for GlobalSystemMediaSessions {
  fn is_access_granted(&self) -> bool {
    let held = self.engine.lock().unwrap();
    held
      .as_ref()
      .is_some_and(|engine| engine.granted.load(Ordering::Relaxed))
  }

  fn start(&self, inbox: Arc<MediaSessionInbox>) {
    self.stop();

    let (tasks, rx) = channel();
    let granted = Arc::new(AtomicBool::new(false));
    let latest: Latest = Arc::new(Mutex::new(Vec::new()));
    let held = Arc::clone(&granted);
    let seen = Arc::clone(&latest);
    let wake = tasks.clone();
    match thread::Builder::new()
      .name("bridgething-media".to_owned())
      .spawn(move || run(inbox, held, seen, wake, rx))
    {
      Ok(_) => {
        tracing::debug!("the windows media watcher is coming up");
        *self.engine.lock().unwrap() = Some(Engine { tasks, granted, latest });
      }
      Err(error) => tracing::warn!(%error, "the windows media watcher could not be started"),
    }
  }

  fn stop(&self) {
    if let Some(engine) = self.engine.lock().unwrap().take() {
      tracing::debug!("the windows media watcher is being torn down");
      let _ = engine.tasks.send(Task::Stop);
    }
  }

  fn snapshot_all(&self, sink: Arc<MediaSnapshotSink>) {
    let now = universal_now(SystemTime::now());
    let sessions = {
      let held = self.engine.lock().unwrap();
      let Some(engine) = held.as_ref() else {
        tracing::warn!("no windows media watcher is up to answer a snapshot");
        return sink.complete(Vec::new());
      };
      let cached = engine.latest.lock().unwrap();
      cached.iter().map(|seen| seen.aged(now)).collect::<Vec<_>>()
    };
    sink.complete(sessions);
  }

  fn control(&self, package: String, cmd: MediaControl) {
    tracing::trace!(%package, ?cmd, "a command is on its way to a windows media session");
    if !self.post(Task::Control(package, cmd)) {
      tracing::warn!(?cmd, "no windows media watcher is up to carry a command");
    }
  }

  fn art(&self, package: String, token: String, sink: Arc<MediaArtSink>) {
    if !self.post(Task::Art(package, token, Arc::clone(&sink))) {
      tracing::warn!("no windows media watcher is up to answer for cover art");
      sink.complete(None);
    }
  }
}

struct Granted(Arc<AtomicBool>);

impl Granted {
  fn armed(flag: Arc<AtomicBool>) -> Self {
    flag.store(true, Ordering::Relaxed);
    Self(flag)
  }
}

impl Drop for Granted {
  fn drop(&mut self) {
    self.0.store(false, Ordering::Relaxed);
  }
}

fn run(
  inbox: Arc<MediaSessionInbox>,
  granted: Arc<AtomicBool>,
  latest: Latest,
  wake: Sender<Task>,
  tasks: Receiver<Task>,
) {
  // SAFETY: gsmtc raises its events on com threadpool threads, so this thread has to join the mta itself.
  let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

  match Watch::open(Arc::new(Mutex::new(wake)), latest) {
    Ok(mut watch) => {
      let _granted = Granted::armed(granted);
      tracing::info!("the windows media session registry is being watched");
      watch.serve(&inbox, tasks);
      watch.close();
      tracing::debug!("the windows media watcher stood down");
    }
    Err(error) => tracing::warn!(%error, "windows refused a media session manager; no local player is observed"),
  }

  // SAFETY: paired with the CoInitializeEx above, on this same thread, after every winrt object is dropped.
  unsafe { CoUninitialize() };
}

struct Watch {
  manager: SessionManager,
  sessions_token: Option<i64>,
  current_token: Option<i64>,
  tracked: Vec<Tracked>,
  art: Option<Sender<Art>>,
  wake: Wake,
  latest: Latest,
}

impl Watch {
  fn open(wake: Wake, latest: Latest) -> windows::core::Result<Self> {
    let manager = SessionManager::RequestAsync()?.join()?;
    let sessions_token = manager
      .SessionsChanged(&on_change::<SessionManager, SessionsChangedEventArgs>(
        &wake,
        Change::Roster,
      ))
      .ok();
    let current_token = manager
      .CurrentSessionChanged(&on_change::<SessionManager, CurrentSessionChangedEventArgs>(
        &wake,
        Change::Roster,
      ))
      .ok();

    Ok(Self {
      manager,
      sessions_token,
      current_token,
      tracked: Vec::new(),
      art: art_thread(),
      wake,
      latest,
    })
  }

  fn serve(&mut self, inbox: &MediaSessionInbox, tasks: Receiver<Task>) {
    let mut coalesce = Coalescer::new(COALESCE);
    self.apply(Change::Roster);
    inbox.on_sessions_changed();

    loop {
      let waited = match coalesce.wait(Instant::now()) {
        Some(window) => tasks.recv_timeout(window),
        None => tasks.recv().map_err(|_| RecvTimeoutError::Disconnected),
      };
      match waited {
        Ok(Task::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        Ok(Task::Changed(change)) => coalesce.arm(Instant::now(), change),
        Ok(Task::Control(package, cmd)) => self.drive(&package, cmd),
        Ok(Task::Art(package, token, sink)) => self.hand_off(&package, &token, sink),
        Err(RecvTimeoutError::Timeout) => {}
      }
      if let Some(change) = coalesce.due(Instant::now()) {
        self.apply(change);
        inbox.on_sessions_changed();
      }
    }
  }

  fn close(&mut self) {
    self.art.take();
    self.latest.lock().unwrap().clear();
    for tracked in self.tracked.drain(..) {
      tracked.release();
    }
    if let Some(token) = self.sessions_token.take() {
      let _ = self.manager.RemoveSessionsChanged(token);
    }
    if let Some(token) = self.current_token.take() {
      let _ = self.manager.RemoveCurrentSessionChanged(token);
    }
  }

  fn apply(&mut self, change: Change) {
    match change {
      Change::Roster => self.resync(),
      Change::Track => self.reread(),
      Change::Playback => {}
    }
    self.publish();
  }

  fn publish(&self) {
    let seen: Vec<Seen> = self.tracked.iter().filter_map(Tracked::seen).collect();
    tracing::trace!(
      sessions = seen.len(),
      "the windows media watcher republished what it sees"
    );
    *self.latest.lock().unwrap() = seen;
  }

  fn reread(&mut self) {
    for tracked in &mut self.tracked {
      tracked.refresh();
    }
  }

  fn resync(&mut self) {
    let sessions = match self.manager.GetSessions() {
      Ok(sessions) => sessions,
      Err(error) => return tracing::warn!(%error, "windows would not list its media sessions"),
    };
    let count = match sessions.Size() {
      Ok(count) => count,
      Err(error) => return tracing::warn!(%error, "windows would not count its media sessions"),
    };

    let mut stale = mem::take(&mut self.tracked);
    let mut live = Vec::with_capacity(count as usize);
    for index in 0..count {
      let session = match sessions.GetAt(index) {
        Ok(session) => session,
        Err(error) => {
          tracing::warn!(%error, index, "windows listed a media session it would not hand over");
          continue;
        }
      };
      match stale.iter().position(|tracked| same(&tracked.session, &session)) {
        Some(at) => {
          let mut held = stale.remove(at);
          held.refresh();
          live.push(held);
        }
        None => live.extend(self.absorb(session)),
      }
    }
    for tracked in stale {
      tracked.release();
    }

    self.tracked = live;
    tracing::debug!(
      sessions = self.tracked.len(),
      "the windows media session registry changed"
    );
  }

  fn absorb(&self, session: Session) -> Option<Tracked> {
    let aumid = match session.SourceAppUserModelId() {
      Ok(aumid) => aumid.to_string(),
      Err(error) => {
        tracing::warn!(%error, "a media session would not name the app behind it");
        return None;
      }
    };

    let mut tracked = Tracked {
      media: session
        .MediaPropertiesChanged(&on_change::<Session, MediaPropertiesChangedEventArgs>(
          &self.wake,
          Change::Track,
        ))
        .ok(),
      playback: session
        .PlaybackInfoChanged(&on_change::<Session, PlaybackInfoChangedEventArgs>(
          &self.wake,
          Change::Playback,
        ))
        .ok(),
      timeline: session
        .TimelinePropertiesChanged(&on_change::<Session, TimelinePropertiesChangedEventArgs>(
          &self.wake,
          Change::Playback,
        ))
        .ok(),
      aumid,
      session,
      props: None,
      title: None,
      artist: None,
      album: None,
      art_token: None,
    };
    tracked.refresh();
    Some(tracked)
  }

  fn drive(&self, package: &str, cmd: MediaControl) {
    let live: Vec<(&str, bool)> = self
      .tracked
      .iter()
      .map(|tracked| (tracked.aumid.as_str(), tracked.playing()))
      .collect();
    let Some(at) = pick(&live, package) else {
      return tracing::warn!(%package, ?cmd, "no windows media session answers to that app id");
    };

    let session = &self.tracked[at].session;
    tracing::debug!(%package, ?cmd, "driving a windows media session");
    let answered = match cmd {
      MediaControl::Play => session.TryPlayAsync().and_then(|held| held.join()),
      MediaControl::Pause => session.TryPauseAsync().and_then(|held| held.join()),
      MediaControl::SkipNext => session.TrySkipNextAsync().and_then(|held| held.join()),
      MediaControl::SkipPrev => session.TrySkipPreviousAsync().and_then(|held| held.join()),
      MediaControl::SeekTo { position_ms } => session
        .TryChangePlaybackPositionAsync(seek_ticks(position_ms, start_ticks(session)))
        .and_then(|held| held.join()),
      MediaControl::SetShuffle { on } => session.TryChangeShuffleActiveAsync(on).and_then(|held| held.join()),
      MediaControl::SetRepeat { mode } => session
        .TryChangeAutoRepeatModeAsync(auto_repeat(mode))
        .and_then(|held| held.join()),
      MediaControl::SetSpeed { speed } => session
        .TryChangePlaybackRateAsync(f64::from(speed))
        .and_then(|held| held.join()),
      MediaControl::SkipToQueueItem { .. } | MediaControl::SetLiked { .. } => {
        return tracing::debug!(%package, ?cmd, "gsmtc carries neither a queue nor a rating, so the command is dropped");
      }
    };

    match answered {
      Ok(true) => tracing::trace!(%package, ?cmd, "the app took the command"),
      Ok(false) => tracing::warn!(%package, ?cmd, "the app declined the command"),
      Err(error) => tracing::warn!(%package, ?cmd, %error, "the command never reached the app"),
    }
  }

  fn hand_off(&self, package: &str, token: &str, sink: Arc<MediaArtSink>) {
    let props = self
      .tracked
      .iter()
      .find(|tracked| tracked.aumid == package && tracked.art_token.as_deref() == Some(token))
      .and_then(|tracked| tracked.props.clone());
    let Some(props) = props else {
      tracing::warn!(%package, %token, "no windows media session holds cover art under that token");
      return sink.complete(None);
    };
    let Some(art) = self.art.as_ref() else {
      tracing::warn!(%package, "the windows cover art thread never came up");
      return sink.complete(None);
    };

    let handed = art.send(Art {
      package: package.to_owned(),
      props,
      sink: Arc::clone(&sink),
    });
    if handed.is_err() {
      tracing::warn!(%package, "the windows cover art thread is gone");
      sink.complete(None);
    }
  }
}

struct Tracked {
  aumid: String,
  session: Session,
  props: Option<TrackProperties>,
  title: Option<String>,
  artist: Option<String>,
  album: Option<String>,
  art_token: Option<String>,
  media: Option<i64>,
  playback: Option<i64>,
  timeline: Option<i64>,
}

impl Tracked {
  fn refresh(&mut self) {
    let props = match self.session.TryGetMediaPropertiesAsync().and_then(|held| held.join()) {
      Ok(props) => Some(props),
      Err(error) => {
        tracing::warn!(aumid = %self.aumid, %error, "an app would not say what it is playing");
        None
      }
    };
    let title = props.as_ref().and_then(|props| text(props.Title()));
    let artist = props
      .as_ref()
      .and_then(|props| text(props.Artist()).or_else(|| text(props.AlbumArtist())));
    let album = props.as_ref().and_then(|props| text(props.AlbumTitle()));
    let art_token = props
      .as_ref()
      .filter(|props| props.Thumbnail().is_ok())
      .map(|_| art_token(&self.aumid, title.as_deref(), artist.as_deref(), album.as_deref()));

    let art = art_token.is_some();
    tracing::trace!(aumid = %self.aumid, ?title, ?artist, ?album, art, "a windows media session was read");
    self.props = props;
    self.title = title;
    self.artist = artist;
    self.album = album;
    self.art_token = art_token;
  }

  fn release(&self) {
    if let Some(token) = self.media {
      let _ = self.session.RemoveMediaPropertiesChanged(token);
    }
    if let Some(token) = self.playback {
      let _ = self.session.RemovePlaybackInfoChanged(token);
    }
    if let Some(token) = self.timeline {
      let _ = self.session.RemoveTimelinePropertiesChanged(token);
    }
  }

  fn playing(&self) -> bool {
    self
      .session
      .GetPlaybackInfo()
      .and_then(|info| info.PlaybackStatus())
      .is_ok_and(playing_of)
  }

  fn seen(&self) -> Option<Seen> {
    if self.title.is_none() && self.artist.is_none() && self.album.is_none() {
      return None;
    }

    let info = self.session.GetPlaybackInfo().ok()?;
    let timeline = self.session.GetTimelineProperties().ok()?;
    let playing = info.PlaybackStatus().is_ok_and(playing_of);
    let start = span(timeline.StartTime());
    let position = elapsed_ms(start, span(timeline.Position()));
    let updated_ticks = timeline
      .LastUpdatedTime()
      .map(|at| at.UniversalTime)
      .unwrap_or_default();

    let controls = info.Controls().ok();
    let can_seek = controls
      .as_ref()
      .and_then(|controls| controls.IsPlaybackPositionEnabled().ok())
      .unwrap_or_default();
    tracing::trace!(
      aumid = %self.aumid,
      playing,
      can_seek,
      can_next = ?controls.as_ref().and_then(|controls| controls.IsNextEnabled().ok()),
      can_previous = ?controls.as_ref().and_then(|controls| controls.IsPreviousEnabled().ok()),
      start_ticks = ?timeline.StartTime().map(|held| held.Duration).ok(),
      position_ticks = ?timeline.Position().map(|held| held.Duration).ok(),
      end_ticks = ?timeline.EndTime().map(|held| held.Duration).ok(),
      updated_ticks,
      position,
      duration = ?duration_ms(start, span(timeline.EndTime())),
      "a windows media session snapshot was taken"
    );
    let snapshot = MediaSessionSnapshot {
      package: self.aumid.clone(),
      title: self.title.clone(),
      artist: self.artist.clone(),
      album: self.album.clone(),
      duration_ms: duration_ms(start, span(timeline.EndTime())),
      position_ms: position,
      playing,
      can_seek,
      art_token: self.art_token.clone(),
      queue: Vec::new(),
      active_queue_id: None,
      shuffle: info.IsShuffleActive().ok().and_then(|held| held.Value().ok()),
      repeat: info
        .AutoRepeatMode()
        .ok()
        .and_then(|held| held.Value().ok())
        .map(repeat_of),
      speed: speed_of(playing, info.PlaybackRate().ok().and_then(|held| held.Value().ok())),
      position_age_ms: None,
      liked: None,
      like_supported: false,
      queue_title: None,
    };
    Some(Seen {
      snapshot,
      updated_ticks,
    })
  }
}

struct Seen {
  snapshot: MediaSessionSnapshot,
  updated_ticks: i64,
}

impl Seen {
  fn aged(&self, now_ticks: i64) -> MediaSessionSnapshot {
    let mut snapshot = self.snapshot.clone();
    snapshot.position_age_ms = snapshot
      .playing
      .then(|| age_ms(self.updated_ticks, now_ticks))
      .flatten();
    snapshot
  }
}

struct Coalescer {
  window: Duration,
  due: Option<(Instant, Change)>,
}

impl Coalescer {
  fn new(window: Duration) -> Self {
    Self { window, due: None }
  }

  fn arm(&mut self, now: Instant, change: Change) {
    match &mut self.due {
      Some((_, held)) => *held = (*held).max(change),
      slot => *slot = Some((now + self.window, change)),
    }
  }

  fn wait(&self, now: Instant) -> Option<Duration> {
    self.due.map(|(at, _)| at.saturating_duration_since(now))
  }

  fn due(&mut self, now: Instant) -> Option<Change> {
    match self.due {
      Some((at, change)) if now >= at => {
        self.due = None;
        Some(change)
      }
      _ => None,
    }
  }
}

fn art_thread() -> Option<Sender<Art>> {
  let (tasks, rx) = channel();
  match thread::Builder::new()
    .name("bridgething-media-art".to_owned())
    .spawn(move || serve_art(rx))
  {
    Ok(_) => Some(tasks),
    Err(error) => {
      tracing::warn!(%error, "the windows cover art thread could not be started");
      None
    }
  }
}

fn serve_art(tasks: Receiver<Art>) {
  // SAFETY: this thread reads winrt stream objects of its own, so it has to join the mta itself.
  let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

  while let Ok(task) = tasks.recv() {
    task.sink.complete(fetch(&task.package, &task.props));
  }

  // SAFETY: paired with the CoInitializeEx above, on this same thread, after every winrt object is dropped.
  unsafe { CoUninitialize() };
  tracing::debug!("the windows cover art thread stood down");
}

fn fetch(package: &str, props: &TrackProperties) -> Option<MediaArt> {
  let stream = props
    .Thumbnail()
    .and_then(|reference| reference.OpenReadAsync())
    .and_then(|held| held.join());
  let stream = match stream {
    Ok(stream) => stream,
    Err(error) => {
      tracing::warn!(%package, %error, "an app would not open its cover art");
      return None;
    }
  };

  let mime = stream.ContentType().map(|mime| mime.to_string()).unwrap_or_default();
  let raw = match read(&stream) {
    Ok(raw) if !raw.is_empty() => raw,
    Ok(_) => {
      tracing::warn!(%package, %mime, "an app offered cover art with no bytes behind it");
      return None;
    }
    Err(error) => {
      tracing::warn!(%package, %mime, %error, "an app cut its cover art short");
      return None;
    }
  };

  tracing::trace!(%package, %mime, bytes = raw.len(), "cover art came off a windows media session");
  match shaped(&mime, raw) {
    Some(art) => Some(art),
    None => {
      tracing::warn!(%package, %mime, "windows handed over cover art this desktop cannot re-encode");
      None
    }
  }
}

fn shaped(mime: &str, raw: Vec<u8>) -> Option<MediaArt> {
  if mime == ART_MIME && jpeg::edge(&raw).is_some_and(|edge| edge <= MAX_ART_EDGE) {
    return Some(MediaArt {
      bytes: raw,
      mime: mime.to_owned(),
    });
  }
  PortableScaler
    .downsample_jpeg(raw, MAX_ART_EDGE, ART_JPEG_QUALITY)
    .map(|bytes| MediaArt {
      bytes,
      mime: ART_MIME.to_owned(),
    })
}

fn on_change<S: RuntimeType + 'static, A: RuntimeType + 'static>(
  wake: &Wake,
  change: Change,
) -> TypedEventHandler<S, A> {
  let wake = Arc::clone(wake);
  TypedEventHandler::new(move |_, _| {
    if let Ok(wake) = wake.lock() {
      let _ = wake.send(Task::Changed(change));
    }
    Ok(())
  })
}

fn read(stream: &IRandomAccessStreamWithContentType) -> windows::core::Result<Vec<u8>> {
  let wanted = stream
    .Size()
    .ok()
    .filter(|size| *size > 0)
    .unwrap_or(MAX_ART_BYTES)
    .min(MAX_ART_BYTES) as u32;
  let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
  let mut bytes = Vec::new();
  while (bytes.len() as u32) < wanted {
    let loaded = reader.LoadAsync(wanted - bytes.len() as u32)?.join()?;
    if loaded == 0 {
      break;
    }
    let at = bytes.len();
    bytes.resize(at + loaded as usize, 0);
    reader.ReadBytes(&mut bytes[at..])?;
  }
  Ok(bytes)
}

fn same(one: &Session, two: &Session) -> bool {
  match (one.cast::<IUnknown>(), two.cast::<IUnknown>()) {
    (Ok(one), Ok(two)) => one.as_raw() == two.as_raw(),
    _ => false,
  }
}

fn start_ticks(session: &Session) -> i64 {
  session
    .GetTimelineProperties()
    .map(|timeline| span(timeline.StartTime()))
    .unwrap_or_default()
}

fn span(held: windows::core::Result<TimeSpan>) -> i64 {
  held.map(|span| span.Duration).unwrap_or_default()
}

fn text(held: windows::core::Result<HSTRING>) -> Option<String> {
  let held = held.ok()?.to_string();
  (!held.is_empty()).then_some(held)
}

fn playing_of(status: PlaybackStatus) -> bool {
  matches!(status, PlaybackStatus::Playing | PlaybackStatus::Changing)
}

fn duration_ms(start_ticks: i64, end_ticks: i64) -> Option<i64> {
  let ms = end_ticks.saturating_sub(start_ticks) / TICKS_PER_MS;
  (ms > 0).then_some(ms)
}

fn elapsed_ms(start_ticks: i64, position_ticks: i64) -> i64 {
  (position_ticks.saturating_sub(start_ticks) / TICKS_PER_MS).max(0)
}

fn age_ms(updated_ticks: i64, now_ticks: i64) -> Option<i64> {
  (updated_ticks > 0).then(|| (now_ticks.saturating_sub(updated_ticks) / TICKS_PER_MS).max(0))
}

fn speed_of(playing: bool, rate: Option<f64>) -> Option<f32> {
  rate.filter(|rate| playing && *rate > 0.0).map(|rate| rate as f32)
}

fn seek_ticks(position_ms: i64, start_ticks: i64) -> i64 {
  start_ticks.saturating_add(position_ms.max(0).saturating_mul(TICKS_PER_MS))
}

fn universal_now(now: SystemTime) -> i64 {
  now
    .duration_since(UNIX_EPOCH)
    .map(|since| UNIX_EPOCH_TICKS.saturating_add((since.as_nanos() / 100).min(i64::MAX as u128) as i64))
    .unwrap_or(UNIX_EPOCH_TICKS)
}

fn repeat_of(mode: AutoRepeatMode) -> MediaRepeatMode {
  match mode {
    AutoRepeatMode::Track => MediaRepeatMode::One,
    AutoRepeatMode::List => MediaRepeatMode::All,
    _ => MediaRepeatMode::Off,
  }
}

fn auto_repeat(mode: MediaRepeatMode) -> AutoRepeatMode {
  match mode {
    MediaRepeatMode::Off => AutoRepeatMode::None,
    MediaRepeatMode::One => AutoRepeatMode::Track,
    MediaRepeatMode::All => AutoRepeatMode::List,
  }
}

fn art_token(aumid: &str, title: Option<&str>, artist: Option<&str>, album: Option<&str>) -> String {
  let mut seed = String::new();
  for field in [Some(aumid), title, artist, album] {
    let field = field.unwrap_or_default();
    seed.push_str(&field.len().to_string());
    seed.push(':');
    seed.push_str(field);
  }
  sha256_hex(seed.as_bytes())
}

fn pick(sessions: &[(&str, bool)], package: &str) -> Option<usize> {
  sessions
    .iter()
    .position(|(aumid, playing)| *aumid == package && *playing)
    .or_else(|| sessions.iter().position(|(aumid, _)| *aumid == package))
}

#[cfg(test)]
mod tests {
  use image::{
    ExtendedColorType, ImageEncoder,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
  };

  use super::*;

  const SPOTIFY: &str = "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify";
  const CHROME: &str = "Chrome";
  const SECOND_TICKS: i64 = 1000 * TICKS_PER_MS;
  const MINUTE_TICKS: i64 = 60 * SECOND_TICKS;

  #[test]
  fn a_timeline_that_does_not_start_at_zero_still_reports_elapsed_time_from_the_track_head() {
    let start = 30 * MINUTE_TICKS;

    assert_eq!(elapsed_ms(start, start + MINUTE_TICKS), 60_000);
    assert_eq!(duration_ms(start, start + 3 * MINUTE_TICKS), Some(180_000));
    assert_eq!(
      elapsed_ms(start, start - MINUTE_TICKS),
      0,
      "a position behind the head is the publisher lying, not a negative elapsed time"
    );
  }

  #[test]
  fn a_publisher_that_reports_no_span_has_no_duration_to_show() {
    assert_eq!(duration_ms(0, 0), None, "chrome pages that never set a position state");
    assert_eq!(duration_ms(MINUTE_TICKS, MINUTE_TICKS), None);
    assert_eq!(
      duration_ms(0, TICKS_PER_MS / 2),
      None,
      "a span that rounds down to nothing is not a duration"
    );
    assert_eq!(duration_ms(0, TICKS_PER_MS), Some(1));
  }

  #[test]
  fn the_age_of_a_position_is_measured_from_when_the_app_last_said_so() {
    let updated = universal_now(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let now = updated + 250 * TICKS_PER_MS;

    assert_eq!(age_ms(updated, now), Some(250));
    assert_eq!(
      age_ms(updated, updated - MINUTE_TICKS),
      Some(0),
      "a clock that ran backwards does not make the position younger than new"
    );
    assert_eq!(
      age_ms(0, now),
      None,
      "an app that never stamped its timeline has no age to report"
    );
  }

  #[test]
  fn a_universal_timestamp_counts_hundred_nanosecond_ticks_from_sixteen_oh_one() {
    assert_eq!(universal_now(UNIX_EPOCH), UNIX_EPOCH_TICKS);
    assert_eq!(
      universal_now(UNIX_EPOCH + Duration::from_secs(1)),
      UNIX_EPOCH_TICKS + 10_000_000
    );
    assert!(
      universal_now(SystemTime::now()) > UNIX_EPOCH_TICKS,
      "the wall clock is later than 1970"
    );
  }

  #[test]
  fn a_seek_is_expressed_in_the_publishers_own_timeline() {
    let start = 30 * MINUTE_TICKS;

    assert_eq!(seek_ticks(0, start), start);
    assert_eq!(seek_ticks(60_000, start), start + MINUTE_TICKS);
    assert_eq!(elapsed_ms(start, seek_ticks(90_000, start)), 90_000);
    assert_eq!(seek_ticks(-5, start), start, "a seek before the head lands on the head");
  }

  #[test]
  fn a_track_that_is_still_arriving_is_still_playing() {
    assert!(playing_of(PlaybackStatus::Playing));
    assert!(
      playing_of(PlaybackStatus::Changing),
      "an app between tracks must not blank now playing"
    );
    assert!(!playing_of(PlaybackStatus::Paused));
    assert!(!playing_of(PlaybackStatus::Stopped));
    assert!(!playing_of(PlaybackStatus::Closed));
    assert!(!playing_of(PlaybackStatus::Opened));
  }

  #[test]
  fn a_cached_snapshot_is_aged_against_the_clock_the_provider_asked_with() {
    let updated = universal_now(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let seen = cached(true, updated);

    assert_eq!(seen.aged(updated).position_age_ms, Some(0));
    assert_eq!(
      seen.aged(updated + 3 * SECOND_TICKS).position_age_ms,
      Some(3_000),
      "a snapshot the watcher took three seconds ago is three seconds old, not fresh"
    );
    assert_eq!(
      seen.aged(updated + MINUTE_TICKS).position_ms,
      42_000,
      "aging a cached snapshot never moves the position the publisher stamped"
    );
    assert_eq!(
      cached(false, updated).aged(updated + MINUTE_TICKS).position_age_ms,
      None,
      "a position nothing is advancing does not drift"
    );
  }

  fn cached(playing: bool, updated_ticks: i64) -> Seen {
    Seen {
      snapshot: MediaSessionSnapshot {
        package: SPOTIFY.to_owned(),
        title: Some("Faded".to_owned()),
        artist: Some("ZHU".to_owned()),
        album: None,
        duration_ms: Some(180_000),
        position_ms: 42_000,
        playing,
        can_seek: true,
        art_token: None,
        queue: Vec::new(),
        active_queue_id: None,
        shuffle: None,
        repeat: None,
        speed: None,
        position_age_ms: None,
        liked: None,
        like_supported: false,
        queue_title: None,
      },
      updated_ticks,
    }
  }

  #[test]
  fn cover_art_that_is_already_small_enough_is_handed_over_untouched() {
    let small = encoded_jpeg(300, 300);

    assert_eq!(
      shaped(ART_MIME, small.clone()),
      Some(MediaArt {
        bytes: small,
        mime: ART_MIME.to_owned(),
      }),
      "a publisher jpeg the device can already show is not decoded and re-encoded"
    );
  }

  #[test]
  fn cover_art_the_device_cannot_show_as_it_stands_comes_back_re_encoded() {
    let wide =
      shaped(ART_MIME, encoded_jpeg(1024, 768)).expect("art wider than the device needs is scaled, not dropped");

    assert_eq!(wide.mime, ART_MIME);
    assert_eq!(
      jpeg::edge(&wide.bytes),
      Some(MAX_ART_EDGE),
      "the long edge of oversized art lands on the ceiling the device asked for"
    );

    let converted = shaped("image/png", encoded_png(300, 300)).expect("a publisher png is converted, not dropped");

    assert_eq!(converted.mime, ART_MIME, "only jpeg ever reaches the car");
    assert_eq!(
      jpeg::edge(&converted.bytes),
      Some(300),
      "art the device can already show keeps its size through the conversion"
    );

    assert_eq!(
      shaped(ART_MIME, b"not an image at all".to_vec()),
      None,
      "bytes no decoder recognizes are not cover art"
    );
  }

  fn encoded_jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new(&mut out)
      .write_image(&gradient(width, height), width, height, ExtendedColorType::Rgb8)
      .expect("a gradient encodes as jpeg");
    out
  }

  fn encoded_png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
      .write_image(&gradient(width, height), width, height, ExtendedColorType::Rgb8)
      .expect("a gradient encodes as png");
    out
  }

  fn gradient(width: u32, height: u32) -> Vec<u8> {
    (0..width * height * 3).map(|at| (at % 251) as u8).collect()
  }

  #[test]
  fn a_rate_that_is_not_moving_the_track_is_no_rate_at_all() {
    assert_eq!(speed_of(true, Some(1.5)), Some(1.5));
    assert_eq!(speed_of(true, None), None);
    assert_eq!(speed_of(false, Some(1.0)), None, "a paused app is not playing at speed");
    assert_eq!(
      speed_of(true, Some(0.0)),
      None,
      "an app that never set a rate on its playback info reports zero, not a stopped track"
    );
    assert_eq!(speed_of(true, Some(-1.0)), None);
  }

  #[test]
  fn repeat_survives_the_trip_in_both_directions() {
    for mode in [MediaRepeatMode::Off, MediaRepeatMode::One, MediaRepeatMode::All] {
      assert_eq!(repeat_of(auto_repeat(mode)), mode);
    }
    assert_eq!(repeat_of(AutoRepeatMode::None), MediaRepeatMode::Off);
    assert_eq!(
      repeat_of(AutoRepeatMode(9)),
      MediaRepeatMode::Off,
      "a mode this build has never heard of is not a repeat"
    );
  }

  #[test]
  fn an_art_token_moves_only_when_the_art_behind_it_could_have() {
    let one = art_token(SPOTIFY, Some("Faded"), Some("ZHU"), Some("Genesis Series"));

    assert_eq!(
      one,
      art_token(SPOTIFY, Some("Faded"), Some("ZHU"), Some("Genesis Series"))
    );
    assert_ne!(one, art_token(SPOTIFY, Some("Faded"), Some("ZHU"), Some("Nightday")));
    assert_ne!(one, art_token(SPOTIFY, Some("Cocaine Model"), Some("ZHU"), None));
    assert_ne!(
      one,
      art_token(CHROME, Some("Faded"), Some("ZHU"), Some("Genesis Series")),
      "two apps playing the same track do not share cover art"
    );
    assert_ne!(
      art_token(SPOTIFY, Some("a:b"), Some("c"), None),
      art_token(SPOTIFY, Some("a"), Some("b:c"), None),
      "a separator inside a field cannot smear one field into the next"
    );
    assert_eq!(
      art_token(SPOTIFY, None, Some("ZHU"), None),
      art_token(SPOTIFY, Some(""), Some("ZHU"), None),
      "a field an app left blank and one it never sent describe the same track"
    );
  }

  #[test]
  fn a_command_goes_to_the_session_of_that_app_that_is_actually_playing() {
    let live = [(CHROME, false), (SPOTIFY, false), (CHROME, true)];

    assert_eq!(pick(&live, CHROME), Some(2));
    assert_eq!(
      pick(&live, SPOTIFY),
      Some(1),
      "the only session an app has takes the command even when it is paused"
    );
    assert_eq!(pick(&live, "Spotify.exe"), None);
    assert_eq!(pick(&[], CHROME), None);
  }

  #[test]
  fn a_storm_of_events_wakes_the_provider_once() {
    let start = Instant::now();
    let mut coalesce = Coalescer::new(COALESCE);

    assert_eq!(coalesce.wait(start), None, "an idle watcher waits on the channel alone");
    assert_eq!(coalesce.due(start), None);

    coalesce.arm(start, Change::Playback);
    assert_eq!(coalesce.wait(start), Some(COALESCE));
    for late in 1..12 {
      coalesce.arm(start + Duration::from_millis(late * 10), Change::Playback);
    }
    assert_eq!(
      coalesce.wait(start),
      Some(COALESCE),
      "a later event does not push the flush further out"
    );

    assert_eq!(coalesce.due(start + COALESCE - Duration::from_millis(1)), None);
    assert_eq!(coalesce.due(start + COALESCE), Some(Change::Playback));
    assert_eq!(coalesce.due(start + COALESCE), None, "one storm is one flush");
    assert_eq!(coalesce.wait(start + COALESCE), None);
  }

  #[test]
  fn a_flush_carries_the_heaviest_change_the_storm_held() {
    let start = Instant::now();
    let mut coalesce = Coalescer::new(COALESCE);

    coalesce.arm(start, Change::Playback);
    coalesce.arm(start, Change::Track);
    coalesce.arm(start, Change::Playback);
    assert_eq!(
      coalesce.due(start + COALESCE),
      Some(Change::Track),
      "a timeline tick alongside a track change still has to re-read the metadata"
    );

    coalesce.arm(start, Change::Roster);
    coalesce.arm(start, Change::Track);
    assert_eq!(coalesce.due(start + COALESCE), Some(Change::Roster));

    coalesce.arm(start, Change::Playback);
    assert_eq!(
      coalesce.due(start + COALESCE),
      Some(Change::Playback),
      "a playhead tick on its own never re-lists the registry"
    );
  }

  #[test]
  fn a_watcher_that_slept_past_its_deadline_flushes_without_waiting() {
    let start = Instant::now();
    let mut coalesce = Coalescer::new(COALESCE);
    coalesce.arm(start, Change::Roster);

    assert_eq!(coalesce.wait(start + 2 * COALESCE), Some(Duration::ZERO));
    assert_eq!(coalesce.due(start + 2 * COALESCE), Some(Change::Roster));
  }
}
