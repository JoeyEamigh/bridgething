use std::{
  collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
  io::{BufRead, BufReader, Write},
  path::{Path, PathBuf},
  process::{Child, ChildStdin, Command, Stdio},
  sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
  },
  thread,
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use bridgething_companion::backend::{
  ImageScaler, MediaArt, MediaArtSink, MediaControl, MediaQueueEntry, MediaRepeatMode, MediaSessionBackend,
  MediaSessionInbox, MediaSessionSnapshot, MediaSnapshotSink,
};
use serde::{Deserialize, Serialize};

use super::{fnv::fingerprint, image::ImageIoScaler};
use crate::{
  backends::jpeg,
  store::{JsonFile, stored},
};

const HELPER_NAME: &str = "libbridgething-mediaremote.dylib";
const STAGE_RECORD: &str = "mediaremote-stages.json";
const PERL: &str = "/usr/bin/perl";
const PERL_LOADER: &str = "use DynaLoader; DynaLoader::dl_load_file($ARGV[0], 0x01) or die; sleep while 1;";
const JPEG: &str = "image/jpeg";
const MAX_ART_EDGE: u32 = 512;
const ART_JPEG_QUALITY: f32 = 0.6;
const ART_WAIT: Duration = Duration::from_secs(4);
const HELPER_PATIENCE: Duration = Duration::from_secs(5);
const HELPER_SETTLED: Duration = Duration::from_secs(5);
const RESTART_FLOOR: Duration = Duration::from_millis(500);
const RESTART_CEILING: Duration = Duration::from_secs(30);
const HELPER_BACKLOG: usize = 256;
const START_ATTEMPTS: u32 = 5;

const COMMAND_PLAY: i32 = 0;
const COMMAND_PAUSE: i32 = 1;
const COMMAND_NEXT: i32 = 4;
const COMMAND_PREVIOUS: i32 = 5;
const COMMAND_CHANGE_RATE: i32 = 19;
const COMMAND_LIKE: i32 = 21;
const COMMAND_DISLIKE: i32 = 22;
const COMMAND_SEEK: i32 = 24;
const COMMAND_REPEAT: i32 = 25;
const COMMAND_SHUFFLE: i32 = 26;
const COMMAND_PLAY_QUEUE_ITEM: i32 = 131;

const OPTION_PLAYBACK_POSITION: &str = "kMRMediaRemoteOptionPlaybackPosition";
const OPTION_PLAYBACK_RATE: &str = "kMRMediaRemoteOptionPlaybackRate";
const OPTION_REPEAT_MODE: &str = "kMRMediaRemoteOptionRepeatMode";
const OPTION_SHUFFLE_MODE: &str = "kMRMediaRemoteOptionShuffleMode";
const OPTION_CONTENT_ITEM_ID: &str = "kMRMediaRemoteOptionContentItemID";

const REPEAT_OFF: i64 = 1;
const REPEAT_ONE: i64 = 2;
const REPEAT_ALL: i64 = 3;
const SHUFFLE_OFF: i64 = 1;
const SHUFFLE_ITEMS: i64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Stage {
  Symbols,
  Client,
  Player,
  Queue,
  Artwork,
  Notifications,
  Commands,
}

impl Stage {
  fn name(self) -> &'static str {
    match self {
      Stage::Symbols => "symbols",
      Stage::Client => "client",
      Stage::Player => "player",
      Stage::Queue => "queue",
      Stage::Artwork => "artwork",
      Stage::Notifications => "notifications",
      Stage::Commands => "commands",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum HelperEvent {
  State(HelperState),
  #[serde(rename = "none")]
  Idle,
  Art(HelperArt),
  Stage {
    name: Stage,
  },
  #[serde(rename = "tick")]
  Beat,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperState {
  package: String,
  title: Option<String>,
  artist: Option<String>,
  album: Option<String>,
  duration_ms: Option<i64>,
  elapsed_ms: Option<i64>,
  timestamp_unix_ms: Option<i64>,
  rate: Option<f32>,
  playing: bool,
  artwork_id: Option<String>,
  #[serde(default)]
  commands: Option<Vec<i32>>,
  #[serde(default)]
  queue: Vec<HelperQueueItem>,
  active_index: Option<i64>,
  queue_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperQueueItem {
  id: Option<String>,
  title: Option<String>,
  subtitle: Option<String>,
  artwork_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperArt {
  token: Option<String>,
  artwork_id: Option<String>,
  mime: Option<String>,
  base64: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
enum HelperOption {
  Mode(i64),
  Amount(f64),
  Item(String),
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
enum HelperCommand {
  Send {
    id: i32,
    options: BTreeMap<&'static str, HelperOption>,
  },
  Art {
    token: String,
    edge: u32,
  },
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Disabled {
  build: String,
  version: String,
  stages: BTreeSet<Stage>,
}

fn os_build() -> String {
  let name = c"kern.osversion";
  let mut size: libc::size_t = 0;
  // SAFETY: a null buffer with a zero length asks sysctl for the size it needs and writes nothing.
  if unsafe { libc::sysctlbyname(name.as_ptr(), std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 {
    return String::new();
  }
  let mut buffer = vec![0u8; size];
  let into = buffer.as_mut_ptr().cast();
  // SAFETY: the buffer holds exactly the byte count sysctl just asked for, and size bounds the write.
  if unsafe { libc::sysctlbyname(name.as_ptr(), into, &mut size, std::ptr::null_mut(), 0) } != 0 {
    return String::new();
  }
  buffer.truncate(size.saturating_sub(1));
  String::from_utf8(buffer).unwrap_or_default()
}

fn carried(held: Option<Disabled>, build: &str, version: &str) -> Disabled {
  match held {
    Some(held) if held.build == build && held.version == version => held,
    _ => Disabled {
      build: build.to_owned(),
      version: version.to_owned(),
      stages: BTreeSet::new(),
    },
  }
}

struct Stages(JsonFile<Disabled>);

impl Stages {
  fn open(config_dir: &Path) -> Self {
    let path = config_dir.join(STAGE_RECORD);
    let held = carried(stored(&path), &os_build(), env!("BRIDGETHING_MEDIAREMOTE_VERSION"));
    tracing::debug!(stages = ?held.stages, build = %held.build, "the mediaremote stage record opened");
    Self(JsonFile::new(path, "mediaremote stage record", held))
  }

  fn skipped(&self) -> Vec<Stage> {
    self.0.read(|held| held.stages.iter().copied().collect())
  }

  fn disable(&self, stage: Stage) -> bool {
    self.0.write(|held| held.stages.insert(stage))
  }
}

fn survived(life: Life, lived: Duration) -> bool {
  life.answered && lived >= HELPER_SETTLED
}

fn kept(previous: Option<&HelperState>, mut state: HelperState) -> HelperState {
  if state.commands.is_none()
    && let Some(previous) = previous.filter(|held| held.package == state.package)
  {
    state.commands = previous.commands.clone();
  }
  state
}

fn repeated(previous: Option<Stage>, current: Option<Stage>) -> Option<Stage> {
  current.filter(|stage| previous == Some(*stage))
}

fn supports(state: &HelperState, command: i32) -> Option<bool> {
  state.commands.as_ref().map(|set| set.contains(&command))
}

fn tappable(state: &HelperState) -> bool {
  supports(state, COMMAND_PLAY_QUEUE_ITEM).unwrap_or(true)
}

fn identify(item: &HelperQueueItem, at: usize) -> i64 {
  match item.id.as_deref() {
    Some(id) => (fingerprint(id.as_bytes()) >> 1) as i64,
    None => -(at as i64) - 1,
  }
}

fn queued(state: &HelperState) -> Vec<MediaQueueEntry> {
  if !tappable(state) {
    return Vec::new();
  }
  state
    .queue
    .iter()
    .enumerate()
    .map(|(at, item)| MediaQueueEntry {
      queue_id: identify(item, at),
      title: item.title.clone(),
      subtitle: item.subtitle.clone(),
      art_token: item.artwork_id.clone(),
    })
    .collect()
}

fn active(state: &HelperState) -> Option<i64> {
  if !tappable(state) {
    return None;
  }
  let at = usize::try_from(state.active_index?).ok()?;
  state.queue.get(at).map(|item| identify(item, at))
}

fn permitted(state: &HelperState, command: &HelperCommand) -> bool {
  match command {
    HelperCommand::Send { id, .. } => supports(state, *id).unwrap_or(true),
    HelperCommand::Art { .. } => true,
  }
}

fn snapshot(state: &HelperState, now_unix_ms: i64) -> MediaSessionSnapshot {
  MediaSessionSnapshot {
    package: state.package.clone(),
    title: state.title.clone(),
    artist: state.artist.clone(),
    album: state.album.clone(),
    duration_ms: state.duration_ms.filter(|ms| *ms > 0),
    position_ms: state.elapsed_ms.unwrap_or_default().max(0),
    playing: state.playing,
    can_seek: supports(state, COMMAND_SEEK).unwrap_or_else(|| state.duration_ms.is_some_and(|ms| ms > 0)),
    art_token: state.artwork_id.clone(),
    queue: queued(state),
    active_queue_id: active(state),
    shuffle: None,
    repeat: None,
    speed: state.playing.then_some(state.rate).flatten().filter(|rate| *rate > 0.0),
    position_age_ms: state
      .playing
      .then_some(state.timestamp_unix_ms)
      .flatten()
      .map(|at| (now_unix_ms - at).max(0)),
    liked: None,
    like_supported: false,
    queue_title: state.queue_title.clone(),
  }
}

fn command(state: &HelperState, cmd: MediaControl) -> Option<HelperCommand> {
  let (id, options): (i32, &[(&'static str, HelperOption)]) = match cmd {
    MediaControl::Play => (COMMAND_PLAY, &[]),
    MediaControl::Pause => (COMMAND_PAUSE, &[]),
    MediaControl::SkipNext => (COMMAND_NEXT, &[]),
    MediaControl::SkipPrev => (COMMAND_PREVIOUS, &[]),
    MediaControl::SeekTo { position_ms } => {
      let seconds = HelperOption::Amount(position_ms.max(0) as f64 / 1000.0);
      return Some(HelperCommand::Send {
        id: COMMAND_SEEK,
        options: BTreeMap::from([(OPTION_PLAYBACK_POSITION, seconds)]),
      });
    }
    MediaControl::SetSpeed { speed } => {
      return Some(HelperCommand::Send {
        id: COMMAND_CHANGE_RATE,
        options: BTreeMap::from([(OPTION_PLAYBACK_RATE, HelperOption::Amount(f64::from(speed)))]),
      });
    }
    MediaControl::SetShuffle { on } => (
      COMMAND_SHUFFLE,
      if on {
        &[(OPTION_SHUFFLE_MODE, HelperOption::Mode(SHUFFLE_ITEMS))]
      } else {
        &[(OPTION_SHUFFLE_MODE, HelperOption::Mode(SHUFFLE_OFF))]
      },
    ),
    MediaControl::SetRepeat { mode } => (
      COMMAND_REPEAT,
      match mode {
        MediaRepeatMode::Off => &[(OPTION_REPEAT_MODE, HelperOption::Mode(REPEAT_OFF))],
        MediaRepeatMode::One => &[(OPTION_REPEAT_MODE, HelperOption::Mode(REPEAT_ONE))],
        MediaRepeatMode::All => &[(OPTION_REPEAT_MODE, HelperOption::Mode(REPEAT_ALL))],
      },
    ),
    MediaControl::SetLiked { liked } => (if liked { COMMAND_LIKE } else { COMMAND_DISLIKE }, &[]),
    MediaControl::SkipToQueueItem { queue_id } => {
      let identifier = state
        .queue
        .iter()
        .enumerate()
        .find(|(at, item)| identify(item, *at) == queue_id)
        .and_then(|(_, item)| item.id.clone())?;
      return Some(HelperCommand::Send {
        id: COMMAND_PLAY_QUEUE_ITEM,
        options: BTreeMap::from([(OPTION_CONTENT_ITEM_ID, HelperOption::Item(identifier))]),
      });
    }
  };
  Some(HelperCommand::Send {
    id,
    options: options.iter().cloned().collect(),
  })
}

fn shape(reply: HelperArt, token: &str) -> Option<MediaArt> {
  if reply.artwork_id.as_deref() != Some(token) {
    return None;
  }
  let bytes = STANDARD.decode(reply.base64?).ok()?;
  let mime = reply.mime.unwrap_or_else(|| JPEG.to_owned());
  if mime == JPEG && jpeg::edge(&bytes).is_some_and(|edge| edge <= MAX_ART_EDGE) {
    return Some(MediaArt { bytes, mime });
  }
  ImageIoScaler
    .downsample_jpeg(bytes, MAX_ART_EDGE, ART_JPEG_QUALITY)
    .map(|bytes| MediaArt {
      bytes,
      mime: JPEG.to_owned(),
    })
}

fn now_unix_ms() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|since| since.as_millis() as i64)
    .unwrap_or_default()
}

fn helper_path() -> Option<PathBuf> {
  let bundled = std::env::current_exe()
    .ok()
    .and_then(|exe| {
      exe
        .parent()
        .map(|dir| dir.join("..").join("Frameworks").join(HELPER_NAME))
    })
    .filter(|path| path.exists());
  #[cfg(debug_assertions)]
  let bundled = bundled.or_else(|| {
    let built = PathBuf::from(env!("BRIDGETHING_MEDIAREMOTE_HELPER"));
    built.exists().then_some(built)
  });
  bundled
}

fn host(path: &Path, skipped: &[Stage]) -> std::io::Result<Child> {
  let mut perl = Command::new(PERL);
  perl.arg("-e").arg(PERL_LOADER).arg(path);
  if !skipped.is_empty() {
    let names: Vec<&str> = skipped.iter().map(|stage| stage.name()).collect();
    perl.arg(format!("skip={}", names.join(",")));
  }
  perl
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
}

#[derive(Default)]
struct Backlogged {
  lines: VecDeque<String>,
  closed: bool,
}

#[derive(Debug, PartialEq)]
enum Taken {
  Line(String),
  Silent,
  Closed,
}

#[derive(Default)]
struct Backlog {
  held: Mutex<Backlogged>,
  ready: Condvar,
}

impl Backlog {
  fn push(&self, line: String) {
    let mut held = self.held.lock().unwrap();
    if held.lines.len() >= HELPER_BACKLOG
      && let Some(dropped) = held.lines.pop_front()
    {
      tracing::warn!(%dropped, "the mediaremote helper backlog is full and dropped its oldest line");
    }
    held.lines.push_back(line);
    drop(held);
    self.ready.notify_one();
  }

  fn close(&self) {
    self.held.lock().unwrap().closed = true;
    self.ready.notify_all();
  }

  fn take(&self, patience: Duration) -> Taken {
    let deadline = Instant::now() + patience;
    let mut held = self.held.lock().unwrap();
    loop {
      if let Some(line) = held.lines.pop_front() {
        return Taken::Line(line);
      }
      if held.closed {
        return Taken::Closed;
      }
      let Some(left) = deadline.checked_duration_since(Instant::now()) else {
        return Taken::Silent;
      };
      held = self.ready.wait_timeout(held, left).unwrap().0;
    }
  }
}

struct Pending {
  token: String,
  deadline: Instant,
  sink: Arc<MediaArtSink>,
}

enum Absorbed {
  Stage(Stage),
  Payload,
  Beat,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Life {
  answered: bool,
  stage: Option<Stage>,
}

#[derive(Default)]
struct Helper {
  generation: AtomicU64,
  hosted: Mutex<Option<u32>>,
  ticket: AtomicU64,
  answered: AtomicBool,
  failures: AtomicU32,
  tripped: AtomicBool,
  latest: Mutex<Option<HelperState>>,
  writer: Mutex<Option<ChildStdin>>,
  waiting: Mutex<HashMap<u64, Pending>>,
  expiry: Condvar,
}

impl Helper {
  fn owns(&self, generation: u64) -> bool {
    self.generation.load(Ordering::SeqCst) == generation
  }

  fn held(&self) -> Option<HelperState> {
    self.latest.lock().unwrap().clone()
  }

  fn write(&self, command: &HelperCommand) -> bool {
    let mut line = match serde_json::to_vec(command) {
      Ok(line) => line,
      Err(error) => {
        tracing::warn!(%error, ?command, "a mediaremote command does not serialize");
        return false;
      }
    };
    line.push(b'\n');
    let mut held = self.writer.lock().unwrap();
    let Some(writer) = held.as_mut() else {
      return false;
    };
    tracing::trace!(?command, "writing a line to the mediaremote helper");
    writer.write_all(&line).and_then(|()| writer.flush()).is_ok()
  }

  fn kill(&self) {
    let hosted = self.hosted.lock().unwrap();
    let Some(pid) = *hosted else {
      return;
    };
    tracing::debug!(pid, "killing the hosted mediaremote helper");
    // SAFETY: the reaping path clears this under the same lock, so a held pid is always an unreaped child of ours.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
  }

  fn expect(&self, token: String, sink: Arc<MediaArtSink>) -> u64 {
    let ticket = self.ticket.fetch_add(1, Ordering::SeqCst);
    let pending = Pending {
      token,
      deadline: Instant::now() + ART_WAIT,
      sink,
    };
    self.waiting.lock().unwrap().insert(ticket, pending);
    self.expiry.notify_all();
    ticket
  }

  fn forget(&self, ticket: u64) -> Option<Arc<MediaArtSink>> {
    self.waiting.lock().unwrap().remove(&ticket).map(|pending| pending.sink)
  }

  fn settle(&self) {
    let waiting: Vec<Pending> = self
      .waiting
      .lock()
      .unwrap()
      .drain()
      .map(|(_, pending)| pending)
      .collect();
    self.expiry.notify_all();
    for pending in waiting {
      pending.sink.complete(None);
    }
  }

  fn deliver(&self, reply: HelperArt) {
    let Some(token) = reply.token.clone() else {
      return tracing::warn!("the mediaremote helper answered artwork with no token");
    };
    let waiting = self.take(&token);
    if waiting.is_empty() {
      return tracing::debug!(%token, "the mediaremote helper answered artwork nobody is waiting for");
    }
    let art = shape(reply, &token);
    for pending in waiting {
      pending.sink.complete(art.clone());
    }
  }

  fn take(&self, token: &str) -> Vec<Pending> {
    let mut waiting = self.waiting.lock().unwrap();
    let tickets: Vec<u64> = waiting
      .iter()
      .filter(|(_, pending)| pending.token == token)
      .map(|(ticket, _)| *ticket)
      .collect();
    tickets
      .into_iter()
      .filter_map(|ticket| waiting.remove(&ticket))
      .collect()
  }

  fn hold(&self, state: Option<HelperState>, inbox: &Arc<MediaSessionInbox>) {
    if !self.answered.swap(true, Ordering::SeqCst) {
      tracing::info!("the mediaremote helper answered its first now playing read");
    }
    {
      let mut held = self.latest.lock().unwrap();
      let state = state.map(|state| kept(held.as_ref(), state));
      if *held == state {
        return;
      }
      match &state {
        Some(state) => {
          tracing::debug!(package = %state.package, playing = state.playing, "the system now playing session changed")
        }
        None => tracing::debug!("the system has no now playing session"),
      }
      *held = state;
    }
    inbox.on_sessions_changed();
  }

  fn absorb(&self, line: &str, inbox: &Arc<MediaSessionInbox>) -> Absorbed {
    match serde_json::from_str::<HelperEvent>(line) {
      Ok(HelperEvent::Beat) => return Absorbed::Beat,
      Ok(HelperEvent::State(state)) => self.hold(Some(state), inbox),
      Ok(HelperEvent::Idle) => self.hold(None, inbox),
      Ok(HelperEvent::Art(reply)) => self.deliver(reply),
      Ok(HelperEvent::Stage { name }) => {
        tracing::trace!(stage = name.name(), "the mediaremote helper entered a stage");
        return Absorbed::Stage(name);
      }
      Err(error) => tracing::warn!(%error, "the mediaremote helper wrote a line that does not parse"),
    }
    Absorbed::Payload
  }

  fn pump(&self, mut child: Child, inbox: &Arc<MediaSessionInbox>, generation: u64) -> Life {
    let taken = child.stdout.take().zip(child.stdin.take());
    let Some((stdout, stdin)) = taken else {
      let _ = child.kill();
      let _ = child.wait();
      tracing::warn!("the mediaremote helper was hosted without pipes");
      return Life::default();
    };
    let pid = child.id();
    if !self.owns(generation) {
      let _ = child.kill();
      let _ = child.wait();
      tracing::debug!(pid, "the mediaremote helper was hosted after the watcher moved on");
      return Life::default();
    }
    *self.hosted.lock().unwrap() = Some(pid);
    *self.writer.lock().unwrap() = Some(stdin);

    let backlog = Arc::new(Backlog::default());
    let reading = backlog.clone();
    let reader = thread::Builder::new()
      .name("bridgething-mediaremote-lines".to_owned())
      .spawn(move || {
        for line in BufReader::new(stdout).lines() {
          match line {
            Ok(line) => reading.push(line),
            Err(error) => {
              tracing::warn!(%error, "the mediaremote helper stdout broke");
              break;
            }
          }
        }
        reading.close();
      });
    if let Err(error) = reader {
      tracing::warn!(%error, "the mediaremote helper stdout is unread");
      backlog.close();
    }

    let mut life = Life::default();
    loop {
      match backlog.take(HELPER_PATIENCE) {
        Taken::Line(line) if self.owns(generation) => {
          tracing::trace!(%line, "a mediaremote helper line arrived");
          match self.absorb(&line, inbox) {
            Absorbed::Stage(stage) => life.stage = Some(stage),
            Absorbed::Payload => life.stage = None,
            Absorbed::Beat => {}
          }
        }
        Taken::Silent => {
          tracing::warn!(pid, "the mediaremote helper stopped writing and is being torn down");
          break;
        }
        _ => break,
      }
    }

    if self.owns(generation) {
      life.answered = self.answered.swap(false, Ordering::SeqCst);
      self.writer.lock().unwrap().take();
      *self.latest.lock().unwrap() = None;
      self.settle();
      inbox.on_sessions_changed();
    }
    {
      let mut hosted = self.hosted.lock().unwrap();
      if *hosted == Some(pid) {
        hosted.take();
      }
      let _ = child.kill();
      let _ = child.wait();
    }
    tracing::debug!(
      answered = life.answered,
      stage = life.stage.map(Stage::name),
      "the mediaremote helper exited"
    );
    life
  }
}

fn expire(helper: Arc<Helper>, generation: u64) {
  let mut waiting = helper.waiting.lock().unwrap();
  while helper.owns(generation) {
    let now = Instant::now();
    let overdue: Vec<u64> = waiting
      .iter()
      .filter(|(_, pending)| pending.deadline <= now)
      .map(|(ticket, _)| *ticket)
      .collect();
    if !overdue.is_empty() {
      let expired: Vec<Pending> = overdue
        .into_iter()
        .filter_map(|ticket| waiting.remove(&ticket))
        .collect();
      drop(waiting);
      for pending in expired {
        tracing::warn!(token = %pending.token, "the mediaremote helper did not answer the artwork request");
        pending.sink.complete(None);
      }
      waiting = helper.waiting.lock().unwrap();
      continue;
    }
    waiting = match waiting.values().map(|pending| pending.deadline).min() {
      Some(at) => {
        helper
          .expiry
          .wait_timeout(waiting, at.saturating_duration_since(now))
          .unwrap()
          .0
      }
      None => helper.expiry.wait(waiting).unwrap(),
    };
  }
}

fn supervise(
  helper: Arc<Helper>,
  stages: Arc<Stages>,
  dylib: PathBuf,
  inbox: Arc<MediaSessionInbox>,
  halted: Receiver<()>,
  generation: u64,
) {
  let mut backoff = RESTART_FLOOR;
  let mut previous = None;
  while helper.owns(generation) {
    let skipped = stages.skipped();
    let started = Instant::now();
    let life = match host(&dylib, &skipped) {
      Ok(child) => {
        tracing::info!(pid = child.id(), ?skipped, "perl is hosting the mediaremote helper");
        helper.pump(child, &inbox, generation)
      }
      Err(error) => {
        tracing::warn!(%error, "perl could not host the mediaremote helper");
        Life::default()
      }
    };
    let lived = started.elapsed();
    let lasted = survived(life, lived);
    if let Some(stage) = repeated(previous, life.stage)
      && stages.disable(stage)
    {
      tracing::warn!(
        stage = stage.name(),
        "a mediaremote helper stage died twice and is now skipped"
      );
    }
    previous = life.stage;
    if lasted {
      helper.failures.store(0, Ordering::SeqCst);
    } else if helper.failures.fetch_add(1, Ordering::SeqCst) + 1 >= START_ATTEMPTS {
      helper.tripped.store(true, Ordering::SeqCst);
      return tracing::warn!("the mediaremote helper never lasted and will not be hosted again this launch");
    }
    match halted.recv_timeout(backoff) {
      Err(RecvTimeoutError::Timeout) => {
        backoff = if lasted {
          RESTART_FLOOR
        } else {
          (backoff * 2).min(RESTART_CEILING)
        };
        tracing::debug!(?backoff, ?lived, "waiting before hosting the mediaremote helper again");
      }
      _ => return,
    }
  }
}

pub struct MediaRemoteSessions {
  helper: Arc<Helper>,
  stages: Arc<Stages>,
  dylib: Option<PathBuf>,
  halt: Mutex<Option<Sender<()>>>,
}

impl MediaRemoteSessions {
  pub fn new(config_dir: &Path) -> Self {
    Self::hosting(helper_path(), Stages::open(config_dir))
  }

  fn hosting(dylib: Option<PathBuf>, stages: Stages) -> Self {
    Self {
      helper: Arc::new(Helper::default()),
      stages: Arc::new(stages),
      dylib,
      halt: Mutex::new(None),
    }
  }
}

impl Drop for MediaRemoteSessions {
  fn drop(&mut self) {
    self.stop();
  }
}

impl MediaSessionBackend for MediaRemoteSessions {
  fn is_access_granted(&self) -> bool {
    !self.helper.tripped.load(Ordering::SeqCst)
      && self.helper.answered.load(Ordering::SeqCst)
      && self.helper.writer.lock().unwrap().is_some()
  }

  fn start(&self, inbox: Arc<MediaSessionInbox>) {
    self.stop();
    if self.helper.tripped.load(Ordering::SeqCst) {
      return tracing::warn!("the mediaremote helper is out of attempts until the next launch");
    }
    let Some(dylib) = self.dylib.clone() else {
      return tracing::warn!("the mediaremote helper dylib is neither bundled nor built");
    };
    let generation = self.helper.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let (halt, halted) = mpsc::channel();
    let helper = self.helper.clone();
    let stages = self.stages.clone();
    match thread::Builder::new()
      .name("bridgething-mediaremote".to_owned())
      .spawn(move || supervise(helper, stages, dylib, inbox, halted, generation))
    {
      Ok(_) => {
        *self.halt.lock().unwrap() = Some(halt);
        tracing::debug!(generation, "the mediaremote watcher started");
      }
      Err(error) => return tracing::warn!(%error, "the mediaremote watcher could not be started"),
    }
    let helper = self.helper.clone();
    if let Err(error) = thread::Builder::new()
      .name("bridgething-mediaremote-art".to_owned())
      .spawn(move || expire(helper, generation))
    {
      tracing::warn!(%error, "the mediaremote artwork deadlines are unwatched");
    }
  }

  fn stop(&self) {
    self.helper.generation.fetch_add(1, Ordering::SeqCst);
    self.halt.lock().unwrap().take();
    self.helper.writer.lock().unwrap().take();
    self.helper.answered.store(false, Ordering::SeqCst);
    *self.helper.latest.lock().unwrap() = None;
    self.helper.kill();
    self.helper.settle();
    tracing::debug!("the mediaremote watcher stopped");
  }

  fn snapshot_all(&self, sink: Arc<MediaSnapshotSink>) {
    let now = now_unix_ms();
    let held = self.helper.latest.lock().unwrap();
    sink.complete(held.as_ref().map(|state| snapshot(state, now)).into_iter().collect());
  }

  fn control(&self, package: String, cmd: MediaControl) {
    let Some(state) = self.helper.held() else {
      return tracing::warn!(%package, ?cmd, "a control landed while nothing is playing");
    };
    if state.package != package {
      return tracing::warn!(%package, ?cmd, "a control landed for an app that is not playing");
    }
    let Some(command) = command(&state, cmd) else {
      return tracing::debug!(?cmd, "mediaremote carries no command for this control");
    };
    if !permitted(&state, &command) {
      return tracing::debug!(?cmd, package = %state.package, "the playing app does not claim this command");
    }
    if !self.helper.write(&command) {
      tracing::warn!(?cmd, "the mediaremote helper did not take the command");
    }
  }

  fn art(&self, package: String, token: String, sink: Arc<MediaArtSink>) {
    if self.helper.held().map(|state| state.package).as_deref() != Some(package.as_str()) {
      return sink.complete(None);
    }
    let ticket = self.helper.expect(token.clone(), sink);
    let ask = HelperCommand::Art {
      token: token.clone(),
      edge: MAX_ART_EDGE,
    };
    if !self.helper.write(&ask)
      && let Some(sink) = self.helper.forget(ticket)
    {
      tracing::warn!(%token, "the mediaremote helper did not take the artwork request");
      sink.complete(None);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const CLAIMED: &str = r#""commands":[0,1,4,5,19,21,22,24,25,26,131]"#;
  const PLAYING: &str = concat!(
    r#"{"event":"state","elapsedMs":123500,"title":"Probe Track","durationMs":213000,"#,
    r#""package":"com.bridgething.playerprobe","artworkId":"f985a0c4b38e78e8","#,
    r#""commands":[0,1,4,5,19,21,22,24,25,26,131],"rate":1,"album":"Probe Album","artist":"Probe Artist","#,
    r#""timestampUnixMs":1787291905231,"playing":true,"activeIndex":0,"queueTitle":"Probe Playlist","#,
    r#""queue":[{"id":"165::167","title":"Probe Track","subtitle":"Probe Artist","artworkId":"f985a0c4b38e78e8"},"#,
    r#"{"id":"165::169","title":"Second Track","subtitle":"Probe Artist","artworkId":null},"#,
    r#"{"id":"165::171","title":"Third Track","subtitle":null,"artworkId":null}]}"#
  );
  const VERSION: &str = "0f1e2d3c4b5a6978";
  const HEAD: i64 = 7_680_126_040_513_883_623;
  const MIDDLE: i64 = 7_680_129_339_048_768_256;
  const TAIL: i64 = 7_680_549_352_490_650_246;

  fn parse(line: &str) -> HelperEvent {
    serde_json::from_str(line).expect("the helper line parses")
  }

  fn state(line: &str) -> HelperState {
    match parse(line) {
      HelperEvent::State(state) => state,
      other => panic!("expected a state line, got {other:?}"),
    }
  }

  #[test]
  fn a_playing_state_line_becomes_a_snapshot_with_a_live_position() {
    let held = state(PLAYING);
    let taken = snapshot(&held, 1_787_291_907_231);

    assert_eq!(taken.package, "com.bridgething.playerprobe");
    assert_eq!(taken.title.as_deref(), Some("Probe Track"));
    assert_eq!(taken.artist.as_deref(), Some("Probe Artist"));
    assert_eq!(taken.album.as_deref(), Some("Probe Album"));
    assert_eq!(taken.duration_ms, Some(213_000));
    assert_eq!(taken.position_ms, 123_500);
    assert_eq!(taken.position_age_ms, Some(2_000));
    assert_eq!(taken.speed, Some(1.0));
    assert!(taken.playing);
    assert!(taken.can_seek);
    assert_eq!(taken.art_token.as_deref(), Some("f985a0c4b38e78e8"));
    assert_eq!(taken.active_queue_id, Some(HEAD));
    assert_eq!(taken.queue_title.as_deref(), Some("Probe Playlist"));
    assert_eq!(
      taken.queue,
      vec![
        MediaQueueEntry {
          queue_id: HEAD,
          title: Some("Probe Track".to_owned()),
          subtitle: Some("Probe Artist".to_owned()),
          art_token: Some("f985a0c4b38e78e8".to_owned()),
        },
        MediaQueueEntry {
          queue_id: MIDDLE,
          title: Some("Second Track".to_owned()),
          subtitle: Some("Probe Artist".to_owned()),
          art_token: None,
        },
        MediaQueueEntry {
          queue_id: TAIL,
          title: Some("Third Track".to_owned()),
          subtitle: None,
          art_token: None,
        },
      ]
    );
    assert_eq!(taken.shuffle, None);
    assert_eq!(taken.repeat, None);
    assert_eq!(taken.liked, None);
  }

  #[test]
  fn a_paused_state_line_carries_neither_a_speed_nor_a_position_age() {
    let held = state(&PLAYING.replace(r#""playing":true"#, r#""playing":false"#));
    let taken = snapshot(&held, 1_787_291_907_231);

    assert!(!taken.playing);
    assert_eq!(taken.position_ms, 123_500);
    assert_eq!(taken.position_age_ms, None);
    assert_eq!(taken.speed, None);
  }

  #[test]
  fn seeking_follows_the_command_set_the_player_claims() {
    let bare = state(&PLAYING.replace(CLAIMED, r#""commands":[0,1,4,5]"#));
    let taken = snapshot(&bare, 1_787_291_907_231);

    assert_eq!(taken.duration_ms, Some(213_000));
    assert!(!taken.can_seek, "a duration is not a promise that seeking works");
  }

  #[test]
  fn a_player_that_lists_no_commands_falls_back_to_seeking_whatever_has_a_duration() {
    let unknown = PLAYING.replace(CLAIMED, r#""commands":null"#);
    let held = state(&unknown);
    let taken = snapshot(&held, 1_787_291_907_231);

    assert!(taken.can_seek);

    let held = state(&unknown.replace(r#""durationMs":213000"#, r#""durationMs":null"#));
    let taken = snapshot(&held, 1_787_291_907_231);

    assert_eq!(taken.duration_ms, None);
    assert!(!taken.can_seek);
  }

  #[test]
  fn a_playing_line_that_reports_a_stopped_rate_carries_no_speed() {
    let held = state(&PLAYING.replace(r#""rate":1"#, r#""rate":0"#));
    let taken = snapshot(&held, 1_787_291_907_231);

    assert!(taken.playing);
    assert_eq!(taken.speed, None);
  }

  #[test]
  fn the_helper_reports_an_empty_desk_as_an_idle_event() {
    assert_eq!(parse(r#"{"event":"none"}"#), HelperEvent::Idle);
  }

  #[test]
  fn a_heartbeat_is_read_back_as_a_beat_that_blames_no_stage() {
    assert_eq!(parse(r#"{"event":"tick"}"#), HelperEvent::Beat);
  }

  #[test]
  fn every_stage_the_helper_announces_parses_back_into_the_stage_it_named() {
    let stages = [
      Stage::Symbols,
      Stage::Client,
      Stage::Player,
      Stage::Queue,
      Stage::Artwork,
      Stage::Notifications,
      Stage::Commands,
    ];

    for stage in stages {
      let line = format!(r#"{{"event":"stage","name":"{}"}}"#, stage.name());
      assert_eq!(parse(&line), HelperEvent::Stage { name: stage });
    }
    assert!(
      serde_json::from_str::<HelperEvent>(r#"{"event":"stage","name":"teleport"}"#).is_err(),
      "a stage this build does not know is not silently taken for another one"
    );
  }

  #[test]
  fn a_stage_record_survives_the_same_build_and_is_dropped_when_either_key_moves() {
    let held = Disabled {
      build: "25F84".to_owned(),
      version: VERSION.to_owned(),
      stages: BTreeSet::from([Stage::Artwork]),
    };

    assert_eq!(carried(Some(held.clone()), "25F84", VERSION), held);
    assert!(
      carried(Some(held.clone()), "25G12", VERSION).stages.is_empty(),
      "a new os build gets a fresh full attempt"
    );
    assert!(
      carried(Some(held.clone()), "25F84", "a9b8c7d6e5f40312")
        .stages
        .is_empty(),
      "a new helper does not inherit the last one's scars"
    );
    assert_eq!(
      carried(None, "25F84", VERSION),
      Disabled {
        build: "25F84".to_owned(),
        version: VERSION.to_owned(),
        stages: BTreeSet::new(),
      }
    );
  }

  #[test]
  fn the_helper_version_is_the_fingerprint_of_the_source_that_ships() {
    let body = std::fs::read(env!("BRIDGETHING_MEDIAREMOTE_SOURCE")).expect("the helper source is readable");

    assert_eq!(
      env!("BRIDGETHING_MEDIAREMOTE_VERSION"),
      format!("{:016x}", fingerprint(&body))
    );
  }

  #[test]
  fn a_stage_is_only_disabled_once_it_has_killed_the_helper_twice_running() {
    assert_eq!(repeated(None, Some(Stage::Queue)), None);
    assert_eq!(repeated(Some(Stage::Queue), Some(Stage::Queue)), Some(Stage::Queue));
    assert_eq!(repeated(Some(Stage::Queue), Some(Stage::Artwork)), None);
    assert_eq!(repeated(Some(Stage::Queue), None), None);
    assert_eq!(repeated(None, None), None);
  }

  #[test]
  fn a_disabled_stage_is_written_once_and_read_back_as_a_skip() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let stages = Stages::open(scratch.path());

    assert!(stages.skipped().is_empty());
    assert!(stages.disable(Stage::Artwork), "the first disable is new");
    assert!(!stages.disable(Stage::Artwork), "the second disable changes nothing");
    assert_eq!(stages.skipped(), vec![Stage::Artwork]);
    assert_eq!(Stages::open(scratch.path()).skipped(), vec![Stage::Artwork]);
  }

  #[test]
  fn a_command_the_player_does_not_claim_is_never_written_to_the_helper() {
    let held = state(&PLAYING.replace(CLAIMED, r#""commands":[0,1]"#));
    let sent = |held: &HelperState, cmd| command(held, cmd).expect("a command");

    assert!(permitted(&held, &sent(&held, MediaControl::Play)));
    assert!(permitted(&held, &sent(&held, MediaControl::Pause)));
    assert!(!permitted(&held, &sent(&held, MediaControl::SkipNext)));
    assert!(!permitted(
      &held,
      &sent(&held, MediaControl::SeekTo { position_ms: 1000 })
    ));

    let unknown = state(&PLAYING.replace(CLAIMED, r#""commands":null"#));

    assert!(
      permitted(&unknown, &sent(&unknown, MediaControl::SkipNext)),
      "a player that lists nothing is not held to a list"
    );
  }

  #[test]
  fn the_helper_backlog_drops_its_oldest_line_rather_than_stalling_the_helper() {
    let backlog = Backlog::default();
    for at in 0..HELPER_BACKLOG + 2 {
      backlog.push(at.to_string());
    }
    backlog.close();

    let held: Vec<String> = std::iter::from_fn(|| match backlog.take(HELPER_PATIENCE) {
      Taken::Line(line) => Some(line),
      taken => {
        assert_eq!(taken, Taken::Closed, "a closed backlog is never silent");
        None
      }
    })
    .collect();

    assert_eq!(held.len(), HELPER_BACKLOG);
    assert_eq!(held.first().map(String::as_str), Some("2"));
    assert_eq!(
      held.last().map(String::as_str),
      Some((HELPER_BACKLOG + 1).to_string().as_str())
    );
  }

  #[test]
  fn a_backlog_the_helper_stops_writing_to_goes_silent_instead_of_parking_its_reader() {
    let backlog = Backlog::default();

    assert_eq!(backlog.take(Duration::from_millis(50)), Taken::Silent);

    backlog.push("line".to_owned());

    assert_eq!(backlog.take(Duration::from_millis(50)), Taken::Line("line".to_owned()));
  }

  #[test]
  fn a_helper_that_answers_and_dies_at_once_is_counted_as_a_failure() {
    let answered = Life {
      answered: true,
      stage: None,
    };

    assert!(survived(answered, HELPER_SETTLED));
    assert!(
      !survived(answered, HELPER_SETTLED - Duration::from_millis(1)),
      "an answer is not a life, or a helper that dies on every first read restarts forever"
    );
    assert!(!survived(Life::default(), HELPER_SETTLED * 10));
  }

  #[test]
  fn a_missed_command_read_keeps_the_set_the_player_last_claimed() {
    let claimed = state(PLAYING);
    let unknown = state(&PLAYING.replace(CLAIMED, r#""commands":null"#));
    let refreshed = state(&PLAYING.replace(CLAIMED, r#""commands":[0,1]"#));
    let elsewhere = HelperState {
      package: "com.apple.Music".to_owned(),
      ..unknown.clone()
    };

    assert_eq!(
      kept(Some(&claimed), unknown.clone()).commands,
      claimed.commands,
      "a dropped command read does not reopen the ungated command path"
    );
    assert_eq!(kept(Some(&claimed), refreshed).commands, Some(vec![0, 1]));
    assert_eq!(kept(Some(&claimed), elsewhere).commands, None);
    assert_eq!(kept(None, unknown).commands, None);
  }

  #[test]
  fn liking_is_not_offered_while_nothing_reads_the_liked_state_back() {
    let taken = snapshot(&state(PLAYING), 1_787_291_907_231);

    assert_eq!(taken.liked, None);
    assert!(
      !taken.like_supported,
      "the player claims both halves of the toggle, but a toggle with no state to read is one way"
    );
  }

  #[test]
  fn transport_controls_map_onto_the_mediaremote_command_numbers() {
    let held = state(PLAYING);
    let plain = |id| {
      Some(HelperCommand::Send {
        id,
        options: BTreeMap::new(),
      })
    };

    assert_eq!(command(&held, MediaControl::Play), plain(0));
    assert_eq!(command(&held, MediaControl::Pause), plain(1));
    assert_eq!(command(&held, MediaControl::SkipNext), plain(4));
    assert_eq!(command(&held, MediaControl::SkipPrev), plain(5));
    assert_eq!(command(&held, MediaControl::SetLiked { liked: true }), plain(21));
    assert_eq!(command(&held, MediaControl::SetLiked { liked: false }), plain(22));
  }

  #[test]
  fn skipping_to_a_queue_entry_sends_the_identifier_the_publisher_gave_that_row() {
    let held = state(PLAYING);
    let item = |identifier: &str| {
      Some(HelperCommand::Send {
        id: 131,
        options: BTreeMap::from([(OPTION_CONTENT_ITEM_ID, HelperOption::Item(identifier.to_owned()))]),
      })
    };

    assert_eq!(
      command(&held, MediaControl::SkipToQueueItem { queue_id: HEAD }),
      item("165::167")
    );
    assert_eq!(
      command(&held, MediaControl::SkipToQueueItem { queue_id: TAIL }),
      item("165::171")
    );
    assert_eq!(
      command(&held, MediaControl::SkipToQueueItem { queue_id: 3 }),
      None,
      "a key no published row carries is not turned into a command"
    );

    let anonymous = state(&PLAYING.replace(r#"{"id":"165::171""#, r#"{"id":null"#));
    let unnamed = snapshot(&anonymous, 1_787_291_907_231).queue[2].queue_id;

    assert_eq!(
      command(&anonymous, MediaControl::SkipToQueueItem { queue_id: unnamed }),
      None,
      "an entry mediaremote gave no identifier cannot be played"
    );
  }

  #[test]
  fn a_queue_key_names_the_row_the_publisher_named_and_not_its_position() {
    let held = state(PLAYING);
    let advanced = state(&PLAYING.replace(
      r#"{"id":"165::167","title":"Probe Track","subtitle":"Probe Artist","artworkId":"f985a0c4b38e78e8"},"#,
      "",
    ));
    let key = |held: &HelperState, at: usize| snapshot(held, 1_787_291_907_231).queue[at].queue_id;

    assert_eq!(
      key(&held, 2),
      key(&advanced, 1),
      "a row keeps its key once the rows ahead of it have played"
    );
    assert_eq!(
      command(&advanced, MediaControl::SkipToQueueItem { queue_id: TAIL }),
      command(&held, MediaControl::SkipToQueueItem { queue_id: TAIL }),
      "a tap minted against the older queue still names the same track"
    );
    assert_eq!(
      snapshot(&advanced, 1_787_291_907_231).active_queue_id,
      Some(MIDDLE),
      "the active row is named the same way the queue rows are"
    );
  }

  #[test]
  fn a_player_that_does_not_claim_the_queue_command_publishes_no_up_next_list() {
    let held = state(&PLAYING.replace(CLAIMED, r#""commands":[0,1,4,5]"#));
    let taken = snapshot(&held, 1_787_291_907_231);

    assert!(
      taken.queue.is_empty(),
      "an up next list nobody can act on is not offered"
    );
    assert_eq!(taken.active_queue_id, None);
    assert!(
      !permitted(
        &held,
        &command(&held, MediaControl::SkipToQueueItem { queue_id: MIDDLE }).expect("a command")
      ),
      "the command the missing claim hides is the command the backend refuses"
    );

    let unknown = state(&PLAYING.replace(CLAIMED, r#""commands":null"#));

    assert_eq!(
      snapshot(&unknown, 1_787_291_907_231).queue.len(),
      3,
      "a player that lists nothing is not held to a list"
    );
  }

  #[test]
  fn an_active_index_outside_the_published_queue_is_not_offered_as_a_selection() {
    let held = state(&PLAYING.replace(r#""activeIndex":0"#, r#""activeIndex":9"#));

    assert_eq!(snapshot(&held, 1_787_291_907_231).active_queue_id, None);

    let absent = state(&PLAYING.replace(r#""activeIndex":0"#, r#""activeIndex":null"#));

    assert_eq!(snapshot(&absent, 1_787_291_907_231).active_queue_id, None);
  }

  #[test]
  fn seeking_and_rate_changes_carry_their_option_in_seconds_and_multiples() {
    let held = state(PLAYING);

    assert_eq!(
      command(&held, MediaControl::SeekTo { position_ms: 123_500 }),
      Some(HelperCommand::Send {
        id: 24,
        options: BTreeMap::from([(OPTION_PLAYBACK_POSITION, HelperOption::Amount(123.5))]),
      })
    );
    assert_eq!(
      command(&held, MediaControl::SeekTo { position_ms: -10 }),
      Some(HelperCommand::Send {
        id: 24,
        options: BTreeMap::from([(OPTION_PLAYBACK_POSITION, HelperOption::Amount(0.0))]),
      })
    );
    assert_eq!(
      command(&held, MediaControl::SetSpeed { speed: 1.5 }),
      Some(HelperCommand::Send {
        id: 19,
        options: BTreeMap::from([(OPTION_PLAYBACK_RATE, HelperOption::Amount(1.5))]),
      })
    );
  }

  #[test]
  fn repeat_and_shuffle_use_the_one_based_values_the_player_actually_receives() {
    let held = state(PLAYING);
    let repeat = |mode| command(&held, MediaControl::SetRepeat { mode });
    let shuffle = |on| command(&held, MediaControl::SetShuffle { on });
    let sent = |id, key, value| {
      Some(HelperCommand::Send {
        id,
        options: BTreeMap::from([(key, HelperOption::Mode(value))]),
      })
    };

    assert_eq!(repeat(MediaRepeatMode::Off), sent(25, OPTION_REPEAT_MODE, 1));
    assert_eq!(repeat(MediaRepeatMode::One), sent(25, OPTION_REPEAT_MODE, 2));
    assert_eq!(repeat(MediaRepeatMode::All), sent(25, OPTION_REPEAT_MODE, 3));
    assert_eq!(shuffle(false), sent(26, OPTION_SHUFFLE_MODE, 1));
    assert_eq!(shuffle(true), sent(26, OPTION_SHUFFLE_MODE, 3));
  }

  #[test]
  fn a_command_serializes_as_one_json_line_the_helper_understands() {
    let held = state(PLAYING);
    let line = |cmd| serde_json::to_string(&command(&held, cmd).expect("a command")).expect("the command serializes");

    assert_eq!(
      line(MediaControl::SeekTo { position_ms: 4_000 }),
      r#"{"cmd":"send","id":24,"options":{"kMRMediaRemoteOptionPlaybackPosition":4.0}}"#
    );
    assert_eq!(
      line(MediaControl::SetRepeat {
        mode: MediaRepeatMode::All
      }),
      r#"{"cmd":"send","id":25,"options":{"kMRMediaRemoteOptionRepeatMode":3}}"#
    );
    assert_eq!(
      line(MediaControl::SetShuffle { on: true }),
      r#"{"cmd":"send","id":26,"options":{"kMRMediaRemoteOptionShuffleMode":3}}"#
    );
    assert_eq!(
      line(MediaControl::SkipToQueueItem { queue_id: MIDDLE }),
      r#"{"cmd":"send","id":131,"options":{"kMRMediaRemoteOptionContentItemID":"165::169"}}"#
    );
  }

  #[test]
  fn an_artwork_request_serializes_with_the_token_the_reply_is_matched_against() {
    let line = serde_json::to_string(&HelperCommand::Art {
      token: "f985a0c4b38e78e8".to_owned(),
      edge: MAX_ART_EDGE,
    })
    .expect("the request serializes");

    assert_eq!(line, r#"{"cmd":"art","token":"f985a0c4b38e78e8","edge":512}"#);
  }

  #[test]
  fn artwork_that_is_already_a_small_jpeg_is_handed_over_untouched() {
    let bytes = jpeg::sample(512, 512);
    let taken = shape(
      HelperArt {
        token: Some("token".to_owned()),
        artwork_id: Some("token".to_owned()),
        mime: Some(JPEG.to_owned()),
        base64: Some(STANDARD.encode(&bytes)),
      },
      "token",
    );

    assert_eq!(
      taken,
      Some(MediaArt {
        bytes,
        mime: JPEG.to_owned(),
      })
    );
  }

  fn settled(backend: &MediaRemoteSessions) -> Option<MediaSessionSnapshot> {
    let (sink, mut answer) = MediaSnapshotSink::channel();
    backend.snapshot_all(sink);
    answer.try_recv().ok().and_then(|sessions| sessions.into_iter().next())
  }

  fn until(deadline: Duration, ready: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < deadline {
      if ready() {
        return true;
      }
      thread::sleep(Duration::from_millis(100));
    }
    false
  }

  fn hosted(backend: &MediaRemoteSessions) -> Option<u32> {
    *backend.helper.hosted.lock().unwrap()
  }

  fn faulted(into: &Path, faults: &str) -> MediaRemoteSessions {
    let dylib = into.join(HELPER_NAME);
    let built = Command::new("clang")
      .args(["-dynamiclib", "-fobjc-arc", "-framework", "Foundation"])
      .arg(format!("-DBRIDGETHING_MEDIAREMOTE_FAULTS=\"{faults}\""))
      .arg("-o")
      .arg(&dylib)
      .arg(env!("BRIDGETHING_MEDIAREMOTE_SOURCE"))
      .status()
      .expect("clang runs");
    assert!(built.success(), "clang built the faulted helper");
    MediaRemoteSessions::hosting(Some(dylib), Stages::open(into))
  }

  #[test]
  #[ignore = "hosts perl and kills it, so it is kept out of the default run"]
  fn a_helper_that_dies_is_hosted_again_and_a_stop_leaves_nothing_behind() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let backend = MediaRemoteSessions::new(scratch.path());
    let (inbox, _ticks) = MediaSessionInbox::channel();
    backend.start(inbox);
    assert!(
      until(Duration::from_secs(10), || backend.is_access_granted()),
      "the perl hosted helper never answered"
    );

    let first = hosted(&backend).expect("the watcher never recorded the hosted process");
    let _ = Command::new("/bin/kill")
      .arg("-9")
      .arg(first.to_string())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status();
    assert!(
      until(Duration::from_secs(5), || !backend.is_access_granted()),
      "killing the helper never showed up as lost access"
    );
    assert!(
      until(Duration::from_secs(15), || backend.is_access_granted()),
      "the watcher never hosted the helper again"
    );

    let last = hosted(&backend).expect("the watcher never recorded the second hosted process");
    assert!(
      backend.stages.skipped().is_empty(),
      "a helper that answered before it died blames no stage"
    );
    backend.stop();
    assert!(!backend.is_access_granted(), "stopping tears the helper down");
    assert!(
      until(Duration::from_secs(5), || {
        let alive = Command::new("/bin/kill")
          .arg("-0")
          .arg(last.to_string())
          .stdout(Stdio::null())
          .stderr(Stdio::null())
          .status();
        alive.is_ok_and(|alive| !alive.success())
      }),
      "a stopped watcher left a perl process behind"
    );
  }

  #[test]
  #[ignore = "hosts perl over and over and waits out the restart backoff"]
  fn a_helper_that_never_answers_stops_being_hosted_for_the_rest_of_the_launch() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let backend = MediaRemoteSessions::hosting(Some(scratch.path().join(HELPER_NAME)), Stages::open(scratch.path()));
    let (inbox, _ticks) = MediaSessionInbox::channel();
    backend.start(inbox);

    assert!(
      until(Duration::from_secs(30), || backend
        .helper
        .tripped
        .load(Ordering::SeqCst)),
      "a helper that is not there was hosted forever"
    );
    assert!(!backend.is_access_granted(), "a tripped breaker is not granted access");

    let (inbox, _ticks) = MediaSessionInbox::channel();
    backend.start(inbox);
    thread::sleep(Duration::from_secs(2));

    assert_eq!(
      hosted(&backend),
      None,
      "a tripped breaker hosts nothing on a later start"
    );
    backend.stop();
  }

  #[test]
  #[ignore = "needs a macos app playing a queue of at least two tracks with artwork, and drives it"]
  fn the_live_session_reads_back_transport_commands_and_hands_over_artwork() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let backend = MediaRemoteSessions::new(scratch.path());
    let (inbox, _ticks) = MediaSessionInbox::channel();
    backend.start(inbox);

    assert!(
      until(Duration::from_secs(10), || backend.is_access_granted()),
      "the perl hosted helper never answered"
    );
    let playing = settled(&backend).expect("something on this mac is publishing now playing state");
    assert!(!playing.package.is_empty(), "the session has a bundle identifier");
    assert!(playing.playing, "start something playing before running this");
    let token = playing
      .art_token
      .clone()
      .expect("play a track that carries artwork before running this");
    let active = playing
      .active_queue_id
      .and_then(|id| playing.queue.iter().position(|entry| entry.queue_id == id))
      .unwrap_or_default();
    let next = playing
      .queue
      .get(active + 1)
      .cloned()
      .expect("play a queue with a track after the one that is playing before running this");

    backend.control(playing.package.clone(), MediaControl::Pause);
    assert!(
      until(Duration::from_secs(5), || settled(&backend)
        .is_some_and(|held| !held.playing)),
      "the player never went to paused"
    );

    backend.control(playing.package.clone(), MediaControl::Play);
    assert!(
      until(Duration::from_secs(5), || settled(&backend)
        .is_some_and(|held| held.playing)),
      "the player never came back to playing"
    );

    let (sink, answer) = MediaArtSink::channel();
    backend.art(playing.package.clone(), token, sink);
    let art = answer
      .blocking_recv()
      .expect("the art sink settles")
      .expect("artwork bytes");
    assert_eq!(art.mime, JPEG);
    assert!(jpeg::edge(&art.bytes).is_some_and(|edge| edge <= MAX_ART_EDGE));

    backend.control(
      playing.package.clone(),
      MediaControl::SkipToQueueItem {
        queue_id: next.queue_id,
      },
    );
    assert!(
      until(Duration::from_secs(5), || settled(&backend)
        .is_some_and(|held| held.title == next.title)),
      "the player never moved to the queue entry that was asked for"
    );

    backend.stop();
    assert!(!backend.is_access_granted(), "stopping tears the helper down");
  }

  #[test]
  #[ignore = "compiles a faulted helper and needs a macos app publishing now playing state"]
  fn a_symbol_this_os_no_longer_exports_costs_one_feature_and_not_the_helper() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let backend = faulted(scratch.path(), "MRMediaRemoteGetSupportedCommands");
    let (inbox, _ticks) = MediaSessionInbox::channel();
    backend.start(inbox);

    assert!(
      until(Duration::from_secs(10), || backend.is_access_granted()),
      "the helper never answered without the supported command reader"
    );
    let playing = settled(&backend).expect("something on this mac is publishing now playing state");
    assert_eq!(
      playing.can_seek,
      playing.duration_ms.is_some_and(|ms| ms > 0),
      "a player whose claimed commands cannot be read falls back to seeking whatever has a duration"
    );

    let first = hosted(&backend);
    thread::sleep(Duration::from_secs(3));
    assert_eq!(
      hosted(&backend),
      first,
      "the helper was restarted over a missing symbol"
    );
    assert!(
      backend.stages.skipped().is_empty(),
      "a missing symbol disables no stage"
    );
    backend.stop();
  }

  #[test]
  #[ignore = "compiles a faulted helper and needs a macos app publishing now playing state"]
  fn a_selector_that_throws_costs_the_snapshot_and_not_the_helper() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let backend = faulted(scratch.path(), "dictionaryRepresentation");
    let (inbox, _ticks) = MediaSessionInbox::channel();
    backend.start(inbox);

    assert!(
      until(Duration::from_secs(10), || backend.is_access_granted()),
      "the helper never answered after the queue read threw"
    );
    assert_eq!(settled(&backend), None, "a queue that cannot be read is no session");

    let first = hosted(&backend);
    thread::sleep(Duration::from_secs(3));
    assert_eq!(
      hosted(&backend),
      first,
      "the helper was restarted over a thrown selector"
    );
    assert!(
      backend.stages.skipped().is_empty(),
      "a thrown selector disables no stage"
    );
    backend.stop();
  }

  #[test]
  #[ignore = "compiles a faulted helper that dies on purpose, so it is kept out of the default run"]
  fn a_stage_that_kills_the_helper_twice_is_skipped_on_the_next_start() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let backend = faulted(scratch.path(), "crash:queue");
    let (inbox, _ticks) = MediaSessionInbox::channel();
    backend.start(inbox);

    assert!(
      until(Duration::from_secs(20), || backend.stages.skipped()
        == vec![Stage::Queue]),
      "the stage that killed the helper twice was never disabled"
    );
    assert!(
      until(Duration::from_secs(15), || backend.is_access_granted()),
      "the helper never came up with the killing stage skipped"
    );
    assert_eq!(
      settled(&backend),
      None,
      "skipping the queue costs every field it carries"
    );

    let first = hosted(&backend);
    thread::sleep(Duration::from_secs(3));
    assert_eq!(hosted(&backend), first, "the helper churns with the stage skipped");
    assert_eq!(
      Stages::open(scratch.path()).skipped(),
      vec![Stage::Queue],
      "the disable record outlives the process that wrote it"
    );
    backend.stop();
  }

  #[test]
  #[ignore = "compiles a faulted helper that dies on purpose and needs a macos app publishing now playing state"]
  fn a_command_read_that_kills_the_helper_costs_the_command_set_and_not_the_snapshot() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let backend = faulted(scratch.path(), "crash:commands");
    let (inbox, _ticks) = MediaSessionInbox::channel();
    backend.start(inbox);

    assert!(
      until(Duration::from_secs(20), || backend.stages.skipped()
        == vec![Stage::Commands]),
      "the supported command read was never blamed for the deaths it caused"
    );
    assert!(
      until(Duration::from_secs(15), || backend.is_access_granted()),
      "the helper never came up with the command read skipped"
    );

    let playing = settled(&backend).expect("skipping the command read costs no other field");
    assert_eq!(
      playing.can_seek,
      playing.duration_ms.is_some_and(|ms| ms > 0),
      "a command set that cannot be read falls back to seeking whatever has a duration"
    );
    backend.stop();
  }

  #[test]
  #[ignore = "hosts perl and needs a macos app publishing now playing state"]
  fn a_disabled_artwork_stage_answers_the_request_instead_of_reading_the_artwork() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let stages = Stages::open(scratch.path());
    assert!(stages.disable(Stage::Artwork));
    let backend = MediaRemoteSessions::hosting(helper_path(), stages);
    let (inbox, _ticks) = MediaSessionInbox::channel();
    backend.start(inbox);

    assert!(
      until(Duration::from_secs(10), || backend.is_access_granted()),
      "the perl hosted helper never answered"
    );
    let playing = settled(&backend).expect("something on this mac is publishing now playing state");
    let token = playing.art_token.clone().expect("the playing app carries artwork");

    let (sink, answer) = MediaArtSink::channel();
    let asked = Instant::now();
    backend.art(playing.package.clone(), token, sink);

    assert_eq!(
      answer.blocking_recv().expect("the art sink settles"),
      None,
      "a skipped artwork stage hands over no bytes"
    );
    assert!(
      asked.elapsed() < ART_WAIT,
      "the helper answered rather than leaving the request to expire"
    );
    backend.stop();
  }

  #[test]
  fn one_failed_artwork_request_does_not_cancel_a_sibling_waiting_on_the_same_token() {
    let helper = Helper::default();
    let (dropped, mut dropped_answer) = MediaArtSink::channel();
    let (kept, mut kept_answer) = MediaArtSink::channel();
    let failing = helper.expect("token".to_owned(), dropped);
    helper.expect("token".to_owned(), kept);

    helper
      .forget(failing)
      .expect("the failing request owns its own sink")
      .complete(None);
    helper.deliver(HelperArt {
      token: Some("token".to_owned()),
      artwork_id: Some("token".to_owned()),
      mime: Some(JPEG.to_owned()),
      base64: Some(STANDARD.encode(jpeg::sample(512, 512))),
    });

    assert_eq!(dropped_answer.try_recv().expect("the failing request settles"), None);
    assert_eq!(
      kept_answer.try_recv().expect("the surviving request settles"),
      Some(MediaArt {
        bytes: jpeg::sample(512, 512),
        mime: JPEG.to_owned(),
      })
    );
  }

  #[test]
  fn a_settled_helper_completes_every_artwork_request_it_still_holds() {
    let helper = Helper::default();
    let (sink, mut answer) = MediaArtSink::channel();
    helper.expect("token".to_owned(), sink);

    helper.settle();

    assert_eq!(answer.try_recv().expect("the pending request settles"), None);
  }

  #[test]
  fn artwork_for_a_track_that_already_changed_is_refused() {
    let taken = shape(
      HelperArt {
        token: Some("stale".to_owned()),
        artwork_id: Some("fresh".to_owned()),
        mime: Some(JPEG.to_owned()),
        base64: Some(STANDARD.encode(jpeg::sample(512, 512))),
      },
      "stale",
    );

    assert_eq!(taken, None);
  }
}
