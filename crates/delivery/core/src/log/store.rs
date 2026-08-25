use std::{
  collections::VecDeque,
  fs::{self, File, OpenOptions},
  io::{BufWriter, Read, Write},
  path::{Path, PathBuf},
  sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicU64, Ordering},
  },
  thread::JoinHandle,
  time::{Duration, Instant},
};

use chrono::{Datelike, Local, NaiveDateTime, TimeZone};

const SEGMENT_SUFFIX: &str = ".log";
const PIN_SUFFIX: &str = ".keep";
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
  Trace,
  Debug,
  Info,
  Notice,
  Warn,
  Error,
  Fatal,
}

impl Level {
  fn letter(self) -> char {
    match self {
      Level::Trace => 'V',
      Level::Debug => 'D',
      Level::Info => 'I',
      Level::Notice => 'N',
      Level::Warn => 'W',
      Level::Error => 'E',
      Level::Fatal => 'F',
    }
  }

  fn from_letter(letter: char) -> Option<Self> {
    match letter {
      'V' => Some(Level::Trace),
      'D' => Some(Level::Debug),
      'I' => Some(Level::Info),
      'N' => Some(Level::Notice),
      'W' => Some(Level::Warn),
      'E' => Some(Level::Error),
      'F' => Some(Level::Fatal),
      _ => None,
    }
  }

  fn pins(self) -> bool {
    matches!(self, Level::Error | Level::Fatal)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
  pub launches: usize,
  pub segments_per_launch: usize,
  pub segment_bytes: u64,
  pub queue_capacity: usize,
  pub pinned_bytes_limit: u64,
}

impl Default for Limits {
  fn default() -> Self {
    Self {
      launches: 3,
      segments_per_launch: 2,
      segment_bytes: 512 * 1024,
      queue_capacity: 4096,
      pinned_bytes_limit: 32 * 1024 * 1024,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogArchive {
  pub id: String,
  pub started_at_ms: u64,
  pub bytes: u64,
  pub pinned: bool,
  pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
  pub ts_unix_ms: u64,
  pub level: Level,
  pub label: String,
  pub message: String,
}

struct Entry {
  line: String,
  pins: bool,
}

#[derive(Default)]
struct Queue {
  pending: VecDeque<Entry>,
  running: bool,
  draining: bool,
  stopping: bool,
  dropped: u64,
  generation: u64,
}

#[derive(Default)]
struct Layout {
  root: Option<PathBuf>,
  launch: Option<String>,
}

struct Shared {
  limits: Limits,
  queue: Mutex<Queue>,
  wake: Condvar,
  layout: Mutex<Layout>,
}

pub struct LogStore {
  shared: Arc<Shared>,
  worker: Mutex<Option<JoinHandle<()>>>,
}

impl LogStore {
  pub fn new(limits: Limits) -> Self {
    Self {
      shared: Arc::new(Shared {
        limits,
        queue: Mutex::new(Queue::default()),
        wake: Condvar::new(),
        layout: Mutex::new(Layout::default()),
      }),
      worker: Mutex::new(None),
    }
  }

  pub fn install(&self, root: &Path) {
    let mut worker = self.worker.lock().unwrap();
    if worker.is_some() {
      return;
    }

    let name = Local::now().timestamp_millis().max(0).to_string();
    let launch = root.join(&name);
    let _ = fs::create_dir_all(&launch);

    {
      let mut layout = self.shared.layout.lock().unwrap();
      layout.root = Some(root.to_path_buf());
      layout.launch = Some(name);
    }
    prune_launches(root, &self.shared.limits);

    self.shared.queue.lock().unwrap().running = true;

    let shared = self.shared.clone();
    *worker = std::thread::Builder::new()
      .name("bridgething-logstore".to_owned())
      .spawn(move || run_writer(shared, launch))
      .ok();
  }

  pub fn record(&self, level: Level, label: &str, message: &str) {
    let mut queue = self.shared.queue.lock().unwrap();
    if !queue.running {
      return;
    }
    let prefix = format!(
      "{} {} {} {} {}: ",
      Local::now().format("%m-%d %H:%M:%S%.3f"),
      std::process::id(),
      thread_seq(),
      level.letter(),
      label
    );
    for part in message.split('\n') {
      if queue.pending.len() >= self.shared.limits.queue_capacity {
        queue.dropped += 1;
        continue;
      }
      queue.pending.push_back(Entry {
        line: format!("{prefix}{part}"),
        pins: level.pins(),
      });
    }
    self.shared.wake.notify_all();
  }

  pub fn write(&self, line: &str) {
    let mut queue = self.shared.queue.lock().unwrap();
    if !queue.running {
      return;
    }
    if queue.pending.len() >= self.shared.limits.queue_capacity {
      queue.dropped += 1;
      return;
    }
    queue.pending.push_back(Entry {
      line: line.to_owned(),
      pins: error_line(line),
    });
    self.shared.wake.notify_all();
  }

  pub fn archives(&self) -> Vec<LogArchive> {
    let (root, live) = self.current_layout();
    let Some(root) = root else {
      return Vec::new();
    };
    let mut archives: Vec<LogArchive> = launch_dirs(&root)
      .into_iter()
      .map(|dir| {
        let id = entry_name(&dir);
        LogArchive {
          started_at_ms: id.parse().unwrap_or(0),
          bytes: total_bytes(&segments(&dir)),
          pinned: !pinned_segments(&dir).is_empty(),
          current: Some(&id) == live.as_ref(),
          id,
        }
      })
      .collect();
    archives.sort_by_key(|archive| std::cmp::Reverse(archive.started_at_ms));
    archives
  }

  pub fn retained_bytes(&self) -> u64 {
    let (root, _) = self.current_layout();
    root
      .map(|root| launch_dirs(&root).iter().map(|dir| total_bytes(&segments(dir))).sum())
      .unwrap_or(0)
  }

  pub fn delete(&self, id: &str) {
    let layout = self.shared.layout.lock().unwrap();
    let Some(root) = layout.root.clone() else {
      return;
    };
    if !is_launch_id(id) {
      return;
    }
    let dir = root.join(id);
    if !dir.is_dir() {
      return;
    }
    self.flush();
    if Some(id) == layout.launch.as_deref() {
      truncate(&dir);
      self.reopen_sink();
    } else {
      let _ = fs::remove_dir_all(&dir);
    }
  }

  pub fn clear(&self) {
    let layout = self.shared.layout.lock().unwrap();
    let Some(root) = layout.root.clone() else {
      return;
    };
    self.flush();
    for dir in launch_dirs(&root) {
      if Some(entry_name(&dir).as_str()) == layout.launch.as_deref() {
        truncate(&dir);
      } else {
        let _ = fs::remove_dir_all(&dir);
      }
    }
    self.reopen_sink();
    self.shared.queue.lock().unwrap().dropped = 0;
  }

  /// the tail of one archive, oldest line first, parsed back out of the threadtime prefix the
  /// writer put on every line.
  pub fn read(&self, id: &str, limit: usize) -> Vec<LogLine> {
    if limit == 0 {
      return Vec::new();
    }
    self.flush();
    let started = id
      .parse::<i64>()
      .ok()
      .and_then(|millis| Local.timestamp_millis_opt(millis).single());
    let mut carried = started.map_or(0, |moment| moment.timestamp_millis().max(0) as u64);
    let mut lines: VecDeque<LogLine> = VecDeque::new();
    for dir in self.archive_dirs(Some(id)) {
      for segment in segments(&dir) {
        let Ok(body) = read_segment(&segment) else {
          continue;
        };
        for raw in String::from_utf8_lossy(&body).lines() {
          if lines.len() == limit {
            lines.pop_front();
          }
          lines.push_back(project_line(raw, started.map(|moment| moment.year()), &mut carried));
        }
      }
    }
    lines.into()
  }

  pub fn export_to(&self, target: &Path, id: Option<&str>) -> std::io::Result<PathBuf> {
    self.flush();
    if let Some(parent) = target.parent() {
      fs::create_dir_all(parent)?;
    }
    let mut out = BufWriter::new(File::create(target)?);

    let (_, live) = self.current_layout();
    let dirs = self.archive_dirs(id);
    let lost = self.shared.queue.lock().unwrap().dropped;

    out.write_all(b"bridgething log export\n")?;
    writeln!(out, "generated: {}", Local::now().format("%Y-%m-%d %H:%M:%S %z"))?;
    writeln!(out, "launches: {}", dirs.len())?;
    if lost > 0 {
      writeln!(out, "dropped lines (writer backpressure): {lost}")?;
    }
    out.write_all(b"\n")?;

    for dir in dirs {
      let name = entry_name(&dir);
      let stamp = name
        .parse::<i64>()
        .ok()
        .and_then(|millis| Local.timestamp_millis_opt(millis).single())
        .map(|moment| moment.format("%Y-%m-%d %H:%M:%S %z").to_string())
        .unwrap_or_else(|| name.clone());
      let current = if Some(&name) == live.as_ref() { " (current)" } else { "" };
      let pinned = if pinned_segments(&dir).is_empty() {
        ""
      } else {
        " [pinned: contains errors]"
      };
      writeln!(out, "===== launch {stamp}{current}{pinned} =====")?;
      for segment in segments(&dir) {
        match read_segment(&segment) {
          Ok(body) => {
            out.write_all(&body)?;
            if body.last() != Some(&b'\n') {
              out.write_all(b"\n")?;
            }
          }
          Err(reason) => writeln!(out, "<<unreadable segment {}: {reason}>>", entry_name(&segment))?,
        }
      }
      out.write_all(b"\n")?;
    }
    out.flush()?;
    Ok(target.to_path_buf())
  }

  pub fn flush(&self) {
    let deadline = Instant::now() + FLUSH_TIMEOUT;
    let mut queue = self.shared.queue.lock().unwrap();
    while !queue.pending.is_empty() || queue.draining {
      let remaining = deadline.saturating_duration_since(Instant::now());
      if remaining.is_zero() {
        return;
      }
      let (held, outcome) = self.shared.wake.wait_timeout(queue, remaining).unwrap();
      queue = held;
      if outcome.timed_out() {
        return;
      }
    }
  }

  fn current_layout(&self) -> (Option<PathBuf>, Option<String>) {
    let layout = self.shared.layout.lock().unwrap();
    (layout.root.clone(), layout.launch.clone())
  }

  fn archive_dirs(&self, id: Option<&str>) -> Vec<PathBuf> {
    let (root, _) = self.current_layout();
    let mut dirs = root.as_deref().map(launch_dirs).unwrap_or_default();
    if let Some(id) = id {
      dirs.retain(|dir| entry_name(dir) == id);
    }
    dirs
  }

  fn reopen_sink(&self) {
    let mut queue = self.shared.queue.lock().unwrap();
    queue.generation += 1;
  }
}

impl Drop for LogStore {
  fn drop(&mut self) {
    {
      let mut queue = self.shared.queue.lock().unwrap();
      queue.stopping = true;
      self.shared.wake.notify_all();
    }
    if let Some(worker) = self.worker.lock().unwrap().take() {
      let _ = worker.join();
    }
  }
}

fn run_writer(shared: Arc<Shared>, launch: PathBuf) {
  let mut sink: Option<Sink> = None;
  let mut generation = 0;
  loop {
    let (batch, wanted) = {
      let mut queue = shared.queue.lock().unwrap();
      while queue.pending.is_empty() && !queue.stopping {
        queue = shared.wake.wait(queue).unwrap();
      }
      if queue.pending.is_empty() {
        return;
      }
      let batch: Vec<Entry> = queue.pending.drain(..).collect();
      queue.draining = true;
      (batch, queue.generation)
    };

    if wanted != generation {
      sink = None;
      generation = wanted;
    }

    for entry in batch {
      if sink.is_none() {
        sink = Sink::open(&launch, &shared.limits).ok();
      }
      let Some(active) = sink.as_mut() else {
        break;
      };
      active.write(&entry.line, entry.pins);
      if active.bytes >= shared.limits.segment_bytes {
        if active.saw_error {
          active.pin();
        }
        sink = None;
      }
    }
    if let Some(active) = sink.as_mut() {
      if active.saw_error {
        active.pin();
      }
      if active.flush().is_err() {
        sink = None;
      }
    }

    let mut queue = shared.queue.lock().unwrap();
    queue.draining = false;
    shared.wake.notify_all();
  }
}

struct Sink {
  path: PathBuf,
  file: BufWriter<File>,
  bytes: u64,
  saw_error: bool,
  pinned: bool,
}

impl Sink {
  fn open(launch: &Path, limits: &Limits) -> std::io::Result<Self> {
    let existing = segments(launch);
    let newest = existing.last();
    let target = match newest {
      Some(path) if file_size(path) < limits.segment_bytes => path.clone(),
      _ => {
        let next = newest.map(|path| segment_index(path)).unwrap_or(-1) + 1;
        launch.join(format!("{next:04}{SEGMENT_SUFFIX}"))
      }
    };
    prune_segments(launch, &target, limits);
    let file = OpenOptions::new().create(true).append(true).open(&target)?;
    let bytes = file.metadata()?.len();
    Ok(Self {
      path: target,
      file: BufWriter::new(file),
      bytes,
      saw_error: false,
      pinned: false,
    })
  }

  fn write(&mut self, line: &str, pins: bool) {
    let _ = self.file.write_all(line.as_bytes());
    let _ = self.file.write_all(b"\n");
    self.bytes += line.len() as u64 + 1;
    if !self.pinned && pins {
      self.saw_error = true;
    }
  }

  fn pin(&mut self) {
    self.saw_error = false;
    if self.pinned {
      return;
    }
    let marker = pin_marker(&self.path);
    self.pinned = File::create(&marker).is_ok() || marker.exists();
  }

  fn flush(&mut self) -> std::io::Result<()> {
    self.file.flush()
  }
}

impl Drop for Sink {
  fn drop(&mut self) {
    let _ = self.file.flush();
  }
}

fn prune_launches(root: &Path, limits: &Limits) {
  let (pinned, rotating): (Vec<PathBuf>, Vec<PathBuf>) = launch_dirs(root)
    .into_iter()
    .partition(|dir| !pinned_segments(dir).is_empty());

  let excess = rotating.len().saturating_sub(limits.launches);
  for dir in rotating.iter().take(excess) {
    let _ = fs::remove_dir_all(dir);
  }

  let mut total: u64 = pinned.iter().map(|dir| total_bytes(&pinned_segments(dir))).sum();
  for dir in &pinned {
    if total <= limits.pinned_bytes_limit {
      break;
    }
    total -= total_bytes(&pinned_segments(dir));
    let _ = fs::remove_dir_all(dir);
  }
}

fn prune_segments(launch: &Path, keep_also: &Path, limits: &Limits) {
  let mut all = segments(launch);
  if !all.iter().any(|path| path.file_name() == keep_also.file_name()) {
    all.push(keep_also.to_path_buf());
  }
  all.sort_by_key(|path| segment_index(path));
  let rotating: Vec<PathBuf> = all.into_iter().filter(|path| !is_pinned(path)).collect();
  let excess = rotating.len().saturating_sub(limits.segments_per_launch);
  for path in rotating.iter().take(excess) {
    if path.file_name() != keep_also.file_name() {
      let _ = fs::remove_file(path);
    }
  }
}

fn truncate(dir: &Path) {
  for segment in segments(dir) {
    let _ = fs::remove_file(pin_marker(&segment));
    let _ = fs::remove_file(&segment);
  }
}

fn launch_dirs(root: &Path) -> Vec<PathBuf> {
  let Ok(entries) = fs::read_dir(root) else {
    return Vec::new();
  };
  let mut dirs: Vec<PathBuf> = entries
    .flatten()
    .map(|entry| entry.path())
    .filter(|path| path.is_dir() && is_launch_id(&entry_name(path)))
    .collect();
  dirs.sort_by_key(|path| entry_name(path));
  dirs
}

fn segments(dir: &Path) -> Vec<PathBuf> {
  let Ok(entries) = fs::read_dir(dir) else {
    return Vec::new();
  };
  let mut files: Vec<PathBuf> = entries
    .flatten()
    .map(|entry| entry.path())
    .filter(|path| path.is_file() && entry_name(path).ends_with(SEGMENT_SUFFIX))
    .collect();
  files.sort_by_key(|path| segment_index(path));
  files
}

fn segment_index(path: &Path) -> i64 {
  entry_name(path).trim_end_matches(SEGMENT_SUFFIX).parse().unwrap_or(-1)
}

fn pin_marker(segment: &Path) -> PathBuf {
  let stem = entry_name(segment);
  let stem = stem.trim_end_matches(SEGMENT_SUFFIX);
  segment
    .parent()
    .unwrap_or(Path::new("."))
    .join(format!("{stem}{PIN_SUFFIX}"))
}

fn is_pinned(segment: &Path) -> bool {
  pin_marker(segment).exists()
}

fn pinned_segments(dir: &Path) -> Vec<PathBuf> {
  segments(dir).into_iter().filter(|path| is_pinned(path)).collect()
}

fn file_size(path: &Path) -> u64 {
  fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn total_bytes(paths: &[PathBuf]) -> u64 {
  paths.iter().map(|path| file_size(path)).sum()
}

fn entry_name(path: &Path) -> String {
  path
    .file_name()
    .and_then(|name| name.to_str())
    .unwrap_or_default()
    .to_owned()
}

fn is_launch_id(id: &str) -> bool {
  !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
}

fn read_segment(path: &Path) -> std::io::Result<Vec<u8>> {
  let mut body = Vec::new();
  File::open(path)?.read_to_end(&mut body)?;
  Ok(body)
}

fn error_line(line: &str) -> bool {
  parse_threadtime(line).is_some_and(|parsed| parsed.level.pins())
}

struct Threadtime<'a> {
  stamp: &'a str,
  level: Level,
  rest: &'a str,
}

/// `MM-DD HH:MM:SS.mmm PID TID L TAG: message`, the shape `record` writes and android logcat
/// hands `write` verbatim.
fn parse_threadtime(line: &str) -> Option<Threadtime<'_>> {
  const SHAPE: &[u8] = b"##-## ##:##:##.###";
  let bytes = line.as_bytes();
  if bytes.len() < SHAPE.len() {
    return None;
  }
  for (index, expected) in SHAPE.iter().enumerate() {
    let matched = match expected {
      b'#' => bytes[index].is_ascii_digit(),
      literal => bytes[index] == *literal,
    };
    if !matched {
      return None;
    }
  }
  let mut rest = &line[SHAPE.len()..];
  for _ in 0..2 {
    rest = strip_run(rest, |glyph| glyph.is_whitespace())?;
    rest = strip_run(rest, |glyph| glyph.is_ascii_digit())?;
  }
  rest = strip_run(rest, |glyph| glyph.is_whitespace())?;
  let level = Level::from_letter(rest.chars().next()?)?;
  let rest = strip_run(rest.get(1..)?, |glyph| glyph.is_whitespace())?;
  Some(Threadtime {
    stamp: &line[..SHAPE.len()],
    level,
    rest,
  })
}

fn project_line(raw: &str, year: Option<i32>, carried: &mut u64) -> LogLine {
  let Some(parsed) = parse_threadtime(raw) else {
    return LogLine {
      ts_unix_ms: *carried,
      level: Level::Info,
      label: String::new(),
      message: raw.to_owned(),
    };
  };
  if let Some(stamped) = stamp_millis(parsed.stamp, year) {
    *carried = stamped;
  }
  let (label, message) = parsed.rest.split_once(": ").unwrap_or(("", parsed.rest));
  LogLine {
    ts_unix_ms: *carried,
    level: parsed.level,
    label: label.to_owned(),
    message: message.to_owned(),
  }
}

/// the written stamp carries no year, so an archive dates its own lines from the launch it belongs to.
fn stamp_millis(stamp: &str, year: Option<i32>) -> Option<u64> {
  let year = year.unwrap_or_else(|| Local::now().year());
  let moment = NaiveDateTime::parse_from_str(&format!("{year}-{stamp}"), "%Y-%m-%d %H:%M:%S%.3f").ok()?;
  u64::try_from(Local.from_local_datetime(&moment).earliest()?.timestamp_millis()).ok()
}

fn strip_run(text: &str, accept: impl Fn(char) -> bool) -> Option<&str> {
  let taken: usize = text
    .chars()
    .take_while(|glyph| accept(*glyph))
    .map(char::len_utf8)
    .sum();
  (taken > 0).then(|| &text[taken..])
}

fn thread_seq() -> u64 {
  static NEXT: AtomicU64 = AtomicU64::new(1);
  thread_local! {
    static SEQ: u64 = NEXT.fetch_add(1, Ordering::Relaxed);
  }
  SEQ.with(|seq| *seq)
}
