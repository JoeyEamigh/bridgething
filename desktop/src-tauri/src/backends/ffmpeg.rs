use std::{
  ffi::OsString,
  fs::{self, File},
  io::{self, BufRead, BufReader, Cursor, Read},
  path::{Path, PathBuf},
  process::{Child, ChildStdout, Command, ExitStatus, Stdio},
  sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
  },
  thread::{self, JoinHandle},
};

use bridgething_companion::{
  backend::{MediaTranscoder, TranscodeJob, TranscodeRequest, TranscodeSink, TranscodedVideo},
  media::{VideoPlan, plan_video, sniff::movie},
};

const BINARY: &str = "bridgething-ffmpeg";
const REASON_LINES: usize = 3;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static SCRATCH: AtomicU64 = AtomicU64::new(0);
static BUNDLED: OnceLock<Option<Sidecar>> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
  #[error("the ffmpeg sidecar did not start: {0}")]
  Spawn(io::Error),
  #[error("the ffmpeg sidecar exited with {status}")]
  Failed { status: ExitStatus, reason: String },
  #[error("the ffmpeg sidecar wrote {0}")]
  Malformed(&'static str),
  #[error("media scratch io failed: {0}")]
  Io(#[from] io::Error),
  #[error("the ffmpeg sidecar is halted")]
  Halted,
}

#[derive(Debug, Default)]
struct Live {
  halted: bool,
  children: Vec<Arc<Mutex<Child>>>,
}

#[derive(Debug, Clone)]
pub struct Sidecar {
  binary: PathBuf,
  live: Arc<Mutex<Live>>,
  job: Option<Arc<Mutex<Live>>>,
}

impl Sidecar {
  pub fn bundled() -> Option<Self> {
    BUNDLED
      .get_or_init(|| {
        let beside = std::env::current_exe()
          .ok()?
          .with_file_name(format!("{BINARY}{}", std::env::consts::EXE_SUFFIX));
        let found = Self::at(beside.clone());
        if found.is_none() {
          tracing::warn!(path = %beside.display(), "no ffmpeg sidecar, so heif stills and video are refused");
        }
        found
      })
      .clone()
  }

  pub fn at(path: PathBuf) -> Option<Self> {
    path.is_file().then(|| Self {
      binary: path,
      live: Arc::default(),
      job: None,
    })
  }

  pub fn halt(&self) {
    halt(&self.live);
  }

  fn scoped(&self) -> Self {
    Self {
      job: Some(Arc::default()),
      ..self.clone()
    }
  }

  fn cancel(&self) {
    if let Some(job) = &self.job {
      halt(job);
    }
  }

  fn scopes(&self) -> impl Iterator<Item = &Arc<Mutex<Live>>> {
    std::iter::once(&self.live).chain(self.job.as_ref())
  }

  pub fn frame(&self, bytes: &[u8]) -> Result<Vec<u8>, SidecarError> {
    let scratch = Scratch::holding(bytes)?;
    self.frame_of(&scratch.0)
  }

  pub fn poster(&self, source: &Path) -> Result<Vec<u8>, SidecarError> {
    let frame = self.frame_of(source)?;
    let decoded = image::load_from_memory_with_format(&frame, image::ImageFormat::Pnm)
      .map_err(|_| SidecarError::Malformed("a frame the image decoder refused"))?;
    let mut png = Cursor::new(Vec::new());
    decoded
      .write_to(&mut png, image::ImageFormat::Png)
      .map_err(|_| SidecarError::Malformed("a frame that would not encode as png"))?;
    Ok(png.into_inner())
  }

  fn frame_of(&self, source: &Path) -> Result<Vec<u8>, SidecarError> {
    let (run, mut stdout) = self.spawn(first_frame(source))?;
    let mut frame = Vec::new();
    stdout.read_to_end(&mut frame)?;
    run.finish()?;
    Ok(frame)
  }

  pub fn size(&self, bytes: &[u8]) -> Result<(u32, u32), SidecarError> {
    let scratch = Scratch::holding(bytes)?;
    self.upright_size(&scratch.0)
  }

  pub fn transcode(&self, request: &TranscodeRequest) -> Result<TranscodedVideo, SidecarError> {
    let source = Path::new(&request.source);
    let destination = Path::new(&request.destination);
    let (width, height) = self.upright_size(source)?;
    let plan = plan_video(
      width,
      height,
      request.cover_width,
      request.cover_height,
      request.max_bitrate,
    );
    let (run, mut stdout) = self.spawn(encode(source, destination, &plan))?;
    io::copy(&mut stdout, &mut io::sink())?;
    run.finish()?;
    Ok(TranscodedVideo {
      width: plan.width,
      height: plan.height,
      duration_ms: duration_ms(destination)?,
    })
  }

  fn upright_size(&self, source: &Path) -> Result<(u32, u32), SidecarError> {
    let (run, stdout) = self.spawn(first_frame(source))?;
    match pam_size(stdout) {
      Some(size) => Ok(size),
      None => run.finish().and(Err(SidecarError::Malformed("no frame header"))),
    }
  }

  fn spawn(&self, args: Vec<OsString>) -> Result<(Run, ChildStdout), SidecarError> {
    let mut command = Command::new(&self.binary);
    command
      .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"])
      .args(args)
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    #[cfg(target_os = "linux")]
    dies_with_us(&mut command);
    let mut held: Vec<_> = self.scopes().map(|scope| scope.lock().unwrap()).collect();
    if held.iter().any(|live| live.halted) {
      return Err(SidecarError::Halted);
    }
    let mut child = command.spawn().map_err(SidecarError::Spawn)?;
    #[cfg(windows)]
    windows_job::adopt(&child);
    let stdout = child.stdout.take().expect("the sidecar's stdout is piped");
    let errors = child.stderr.take().map(|mut stderr| {
      thread::spawn(move || {
        let mut written = Vec::new();
        let _ = stderr.read_to_end(&mut written);
        written
      })
    });
    let child = Arc::new(Mutex::new(child));
    for live in &mut held {
      live.children.push(Arc::clone(&child));
    }
    let run = Run {
      child,
      errors,
      scopes: self.scopes().cloned().collect(),
    };
    Ok((run, stdout))
  }
}

fn halt(live: &Mutex<Live>) {
  let mut live = live.lock().unwrap();
  live.halted = true;
  for child in live.children.drain(..) {
    kill_unless_reaped(&child);
  }
}

#[cfg(target_os = "linux")]
fn dies_with_us(command: &mut Command) {
  let parent = std::process::id();
  let arm = move || {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0 {
      return Err(io::Error::last_os_error());
    }
    if u32::try_from(unsafe { libc::getppid() }).ok() != Some(parent) {
      unsafe { libc::raise(libc::SIGKILL) };
    }
    Ok(())
  };
  unsafe { std::os::unix::process::CommandExt::pre_exec(command, arm) };
}

#[cfg(windows)]
mod windows_job {
  use std::{os::windows::io::AsRawHandle, process::Child, sync::OnceLock};

  use windows::{
    Win32::{
      Foundation::HANDLE,
      System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
      },
    },
    core::PCWSTR,
  };

  static JOB: OnceLock<Option<usize>> = OnceLock::new();

  fn job() -> Option<HANDLE> {
    let raw = JOB.get_or_init(|| {
      let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
        .inspect_err(|error| tracing::warn!(%error, "no job object, so an ffmpeg child can outlive a crash"))
        .ok()?;
      let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
      limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
      let limited = unsafe {
        SetInformationJobObject(
          job,
          JobObjectExtendedLimitInformation,
          std::ptr::from_ref(&limits).cast(),
          u32::try_from(std::mem::size_of_val(&limits)).ok()?,
        )
      };
      limited
        .inspect_err(|error| tracing::warn!(%error, "the job object would not kill on close"))
        .ok()?;
      Some(job.0 as usize)
    });
    raw.map(|raw| HANDLE(raw as *mut _))
  }

  pub fn adopt(child: &Child) {
    let Some(job) = job() else {
      return;
    };
    if let Err(error) = unsafe { AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())) } {
      tracing::warn!(%error, "an ffmpeg child did not join the job object");
    }
  }
}

fn kill_unless_reaped(child: &Mutex<Child>) {
  if let Ok(mut child) = child.try_lock() {
    let _ = child.kill();
  }
}

pub struct FfmpegTranscoder(Sidecar);

impl FfmpegTranscoder {
  pub fn new(sidecar: Sidecar) -> Self {
    Self(sidecar)
  }
}

struct FfmpegJob(Sidecar);

impl TranscodeJob for FfmpegJob {
  fn cancel(&self) {
    self.0.cancel();
  }
}

impl MediaTranscoder for FfmpegTranscoder {
  fn transcode(&self, request: TranscodeRequest, sink: Arc<TranscodeSink>) -> Arc<dyn TranscodeJob> {
    let sidecar = self.0.scoped();
    let job = Arc::new(FfmpegJob(sidecar.clone()));
    let unstarted = Arc::clone(&sink);
    let spawned =
      thread::Builder::new()
        .name("ffmpeg-transcode".into())
        .spawn(move || match sidecar.transcode(&request) {
          Ok(video) => sink.complete(video),
          Err(error) => {
            let _ = fs::remove_file(&request.destination);
            tracing::warn!(?error, source = %request.source, "a video could not be transcoded");
            sink.fail(error.to_string());
          }
        });
    if let Err(error) = spawned {
      unstarted.fail(format!("the transcode thread did not start: {error}"));
    }
    job
  }

  fn poster(&self, source: String) -> Option<Vec<u8>> {
    self
      .0
      .poster(Path::new(&source))
      .inspect_err(|error| tracing::warn!(?error, %source, "a video has no poster frame"))
      .ok()
  }
}

struct Run {
  child: Arc<Mutex<Child>>,
  errors: Option<JoinHandle<Vec<u8>>>,
  scopes: Vec<Arc<Mutex<Live>>>,
}

impl Run {
  fn finish(mut self) -> Result<(), SidecarError> {
    let status = self.child.lock().unwrap().wait()?;
    let errors = self
      .errors
      .take()
      .and_then(|reader| reader.join().ok())
      .unwrap_or_default();
    if status.success() {
      return Ok(());
    }
    let written = String::from_utf8_lossy(&errors);
    let mut lines: Vec<&str> = written.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let kept = lines.len().saturating_sub(REASON_LINES);
    let reason = lines.split_off(kept).join(" / ");
    Err(SidecarError::Failed { status, reason })
  }
}

impl Drop for Run {
  fn drop(&mut self) {
    let held = &self.child;
    for scope in &self.scopes {
      scope.lock().unwrap().children.retain(|child| !Arc::ptr_eq(child, held));
    }
    let mut child = held.lock().unwrap();
    let _ = child.kill();
    let _ = child.wait();
  }
}

struct Scratch(PathBuf);

impl Scratch {
  fn holding(bytes: &[u8]) -> io::Result<Self> {
    let serial = SCRATCH.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("bridgething-media-{}-{serial}", std::process::id()));
    fs::write(&path, bytes)?;
    Ok(Self(path))
  }
}

impl Drop for Scratch {
  fn drop(&mut self) {
    let _ = fs::remove_file(&self.0);
  }
}

fn first_frame(source: &Path) -> Vec<OsString> {
  let mut args = vec![OsString::from("-i"), source.into()];
  args.extend(
    [
      "-an",
      "-sn",
      "-dn",
      "-frames:v",
      "1",
      "-c:v",
      "pam",
      "-f",
      "image2pipe",
      "pipe:1",
    ]
    .map(OsString::from),
  );
  args
}

fn encode(source: &Path, destination: &Path, plan: &VideoPlan) -> Vec<OsString> {
  let filter = format!(
    "scale=w={}:h={}:out_color_matrix=bt709:out_primaries=bt709:out_transfer=bt709:out_range=tv,format=yuv420p",
    plan.width, plan.height
  );
  let max_fps = plan.max_fps.to_string();
  let bitrate = plan.bitrate.to_string();
  let keyframes = format!("expr:gte(t,n_forced*{})", plan.keyframe_seconds);
  let mut args = vec![OsString::from("-i"), source.into()];
  args.extend(
    [
      "-an",
      "-sn",
      "-dn",
      "-map_metadata",
      "-1",
      "-map_chapters",
      "-1",
      "-vf",
      &filter,
      "-fpsmax",
      &max_fps,
      "-c:v",
      "libopenh264",
      "-profile:v",
      "high",
      "-rc_mode",
      "bitrate",
      "-b:v",
      &bitrate,
      "-g",
      "0",
      "-force_key_frames",
      &keyframes,
      "-color_primaries",
      "bt709",
      "-color_trc",
      "bt709",
      "-colorspace",
      "bt709",
      "-color_range",
      "tv",
      "-movflags",
      "+faststart",
      "-f",
      "mp4",
    ]
    .map(OsString::from),
  );
  args.push(destination.into());
  args
}

fn pam_size(stdout: impl Read) -> Option<(u32, u32)> {
  let mut lines = BufReader::new(stdout).lines();
  if lines.next()?.ok()? != "P7" {
    return None;
  }
  let (mut width, mut height) = (None, None);
  for line in lines {
    let line = line.ok()?;
    match line.split_once(' ') {
      Some(("WIDTH", value)) => width = value.parse().ok(),
      Some(("HEIGHT", value)) => height = value.parse().ok(),
      _ if line == "ENDHDR" => return width.zip(height),
      _ => {}
    }
  }
  None
}

fn duration_ms(path: &Path) -> Result<u64, SidecarError> {
  movie(&mut BufReader::new(File::open(path)?))
    .and_then(|movie| movie.duration_ms)
    .ok_or(SidecarError::Malformed("an mp4 without a usable movie header"))
}

#[cfg(test)]
#[path = "../../../../crates/companion/src/media/fixture.rs"]
pub mod fixture;

#[cfg(all(test, unix))]
pub mod fake {
  use std::{io::Write, path::Path};

  use super::*;

  pub const STALLED: &str = "#!/bin/sh\nexec sleep 600\n";
  pub const CANNED: &str = "#!/bin/sh\nexec cat \"$0.pam\"\n";
  pub const LOGGED: &str = "#!/bin/sh\necho \"$@\" >> \"$0.log\"\nexec cat \"$0.pam\"\n";
  pub const REFUSING: &str = "#!/bin/sh\necho \"Error opening input file $*\" >&2\nexit 8\n";

  pub fn sidecar(script: &str) -> (tempfile::TempDir, Sidecar) {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("sidecar");
    let mut writer = Command::new("/bin/sh")
      .args(["-c", "cat > \"$0\" && chmod 755 \"$0\""])
      .arg(&binary)
      .stdin(Stdio::piped())
      .spawn()
      .unwrap();
    writer.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    assert!(writer.wait().unwrap().success());
    let sidecar = Sidecar::at(binary).unwrap();
    (dir, sidecar)
  }

  pub fn canned(sidecar: &Sidecar, width: u32, height: u32) {
    let mut frame =
      format!("P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 3\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\n").into_bytes();
    frame.resize(frame.len() + (width * height * 3) as usize, 200);
    let mut beside = sidecar.binary.clone().into_os_string();
    beside.push(".pam");
    fs::write(Path::new(&beside), frame).unwrap();
  }
}

#[cfg(test)]
mod tests {
  #[cfg(unix)]
  use std::{sync::mpsc, time::Duration};

  use super::{fixture, *};

  #[cfg(unix)]
  const KILLED_WITHIN: Duration = Duration::from_secs(10);

  #[cfg(unix)]
  fn closes_within(mut stdout: ChildStdout, deadline: Duration) -> bool {
    let (closed, close) = mpsc::channel();
    thread::spawn(move || {
      let _ = io::copy(&mut stdout, &mut io::sink());
      let _ = closed.send(());
    });
    close.recv_timeout(deadline).is_ok()
  }

  #[cfg(unix)]
  fn request(dir: &Path, max_bitrate: Option<u32>) -> TranscodeRequest {
    TranscodeRequest {
      source: dir.join("source.mov").to_string_lossy().into_owned(),
      destination: dir.join("normalized.mp4").to_string_lossy().into_owned(),
      cover_width: 800,
      cover_height: 480,
      max_bitrate,
    }
  }

  #[cfg(unix)]
  #[test]
  fn halting_kills_a_run_in_flight_and_refuses_the_next() {
    let (_dir, sidecar) = fake::sidecar(fake::STALLED);
    let (run, stdout) = sidecar.spawn(Vec::new()).unwrap();

    sidecar.halt();

    assert!(closes_within(stdout, KILLED_WITHIN), "the halted child is still running");
    assert!(matches!(run.finish(), Err(SidecarError::Failed { status, .. }) if status.code().is_none()));
    assert!(matches!(sidecar.spawn(Vec::new()), Err(SidecarError::Halted)));
  }

  #[cfg(unix)]
  #[test]
  fn cancelling_a_transcode_kills_its_sidecar_and_settles_the_sink_without_touching_others() {
    let (dir, sidecar) = fake::sidecar(fake::STALLED);
    let (bystander, stdout) = sidecar.spawn(Vec::new()).unwrap();
    let (sink, settled) = TranscodeSink::channel();
    let request = request(dir.path(), None);
    std::fs::write(&request.destination, b"partial").unwrap();

    let job = FfmpegTranscoder::new(sidecar.clone()).transcode(request.clone(), sink);
    let deadline = std::time::Instant::now() + KILLED_WITHIN;
    while sidecar.live.lock().unwrap().children.len() < 2 {
      assert!(std::time::Instant::now() < deadline, "the transcode never spawned its sidecar");
      thread::sleep(Duration::from_millis(5));
    }
    job.cancel();

    let (done, outcome) = mpsc::channel();
    thread::spawn(move || {
      let _ = done.send(settled.blocking_recv());
    });
    let reason = outcome
      .recv_timeout(KILLED_WITHIN)
      .expect("the cancelled transcode settles")
      .unwrap()
      .unwrap_err();
    assert!(!reason.is_empty());
    assert!(!Path::new(&request.destination).exists());
    assert_eq!(sidecar.live.lock().unwrap().children.len(), 1, "only the bystander is left");
    assert!(sidecar.spawn(Vec::new()).is_ok(), "a cancelled job does not halt the sidecar");

    sidecar.halt();
    assert!(closes_within(stdout, KILLED_WITHIN));
    drop(bystander);
  }

  #[cfg(unix)]
  #[test]
  fn a_transcode_asks_the_sidecar_for_no_more_than_the_ceiling() {
    for (ceiling, expected) in [(None, 3_412_800), (Some(900_000), 900_000)] {
      let (dir, sidecar) = fake::sidecar(fake::LOGGED);
      fake::canned(&sidecar, 1080, 1920);
      let mut log = sidecar.binary.clone().into_os_string();
      log.push(".log");

      let _ = sidecar.transcode(&request(dir.path(), ceiling));

      let invocations = std::fs::read_to_string(Path::new(&log)).unwrap();
      let encode = invocations
        .lines()
        .find(|line| line.contains("libopenh264"))
        .expect("the encode ran");
      assert!(encode.contains(&format!("-b:v {expected} ")), "{ceiling:?}: {encode}");
    }
  }

  #[cfg(unix)]
  #[test]
  fn a_failed_transcode_tells_the_page_nothing_about_local_paths() {
    let (dir, sidecar) = fake::sidecar(fake::REFUSING);
    let source = dir.path().join("private").join("holiday.mov");
    let destination = dir.path().join("normalized.mp4");
    let (sink, settled) = TranscodeSink::channel();

    FfmpegTranscoder::new(sidecar).transcode(
      TranscodeRequest {
        source: source.to_string_lossy().into_owned(),
        destination: destination.to_string_lossy().into_owned(),
        cover_width: 800,
        cover_height: 480,
        max_bitrate: None,
      },
      sink,
    );

    let reason = settled.blocking_recv().unwrap().unwrap_err();
    assert!(reason.contains("exit status: 8"), "{reason}");
    assert!(!reason.contains("holiday"), "{reason}");
    assert!(!reason.contains(&*dir.path().to_string_lossy()), "{reason}");
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn a_sidecar_dies_with_the_thread_that_started_it_even_when_nothing_kills_it() {
    let (_dir, sidecar) = fake::sidecar(fake::STALLED);
    let spawner = sidecar.clone();
    let stdout = thread::spawn(move || {
      let (run, stdout) = spawner.spawn(Vec::new()).unwrap();
      std::mem::forget(run);
      stdout
    })
    .join()
    .unwrap();

    assert!(
      closes_within(stdout, KILLED_WITHIN),
      "the orphaned sidecar kept running after the thread that owned it was gone"
    );
    sidecar.halt();
  }

  #[cfg(unix)]
  #[test]
  fn a_finished_run_is_no_longer_held() {
    let (_dir, sidecar) = fake::sidecar(fake::STALLED);
    let (run, _stdout) = sidecar.spawn(Vec::new()).unwrap();
    assert_eq!(sidecar.live.lock().unwrap().children.len(), 1);

    drop(run);

    assert!(sidecar.live.lock().unwrap().children.is_empty());
  }

  fn mp4(header: Vec<u8>) -> tempfile::NamedTempFile {
    let bytes = [
      fixture::ftyp(b"isom", &[b"isom", b"avc1"]),
      fixture::boxed(b"mdat", &[7; 64]),
      fixture::moov(header, &[fixture::Track::Video]),
    ]
    .concat();
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), bytes).unwrap();
    file
  }

  #[test]
  fn the_pam_header_names_the_upright_frame() {
    let frame = b"P7\nWIDTH 1080\nHEIGHT 1920\nDEPTH 3\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\n\xff\x00\xff".as_slice();
    assert_eq!(pam_size(frame), Some((1080, 1920)));
    assert_eq!(pam_size(b"P6\n1080 1920\n255\n".as_slice()), None);
    assert_eq!(pam_size(b"P7\nWIDTH 1080\nENDHDR\n".as_slice()), None);
    assert_eq!(pam_size(b"P7\nWIDTH 1080\nHEIGHT 1920\n".as_slice()), None);
  }

  #[test]
  fn the_movie_header_gives_the_duration_in_either_version() {
    assert_eq!(duration_ms(mp4(fixture::mvhd(0, 1000, 3067)).path()).unwrap(), 3067);
    assert_eq!(duration_ms(mp4(fixture::mvhd(1, 90_000, 180_000)).path()).unwrap(), 2000);
    assert!(matches!(
      duration_ms(mp4(fixture::mvhd(0, 0, 3067)).path()),
      Err(SidecarError::Malformed(_))
    ));
    assert!(matches!(
      duration_ms(mp4(fixture::boxed(b"udta", &[0; 8])).path()),
      Err(SidecarError::Malformed(_))
    ));
  }

  #[test]
  fn the_encode_carries_the_plan() {
    let plan = plan_video(1080, 1920, 800, 480, None);
    let args: Vec<String> = encode(Path::new("in.mov"), Path::new("out.mp4"), &plan)
      .into_iter()
      .map(|arg| arg.into_string().unwrap())
      .collect();
    let after = |flag: &str| args[args.iter().position(|arg| arg == flag).unwrap() + 1].as_str();
    assert!(after("-vf").starts_with("scale=w=800:h=1422:"));
    assert!(after("-vf").ends_with(",format=yuv420p"));
    assert_eq!(after("-fpsmax"), "30");
    assert_eq!(after("-b:v"), plan.bitrate.to_string());
    assert_eq!(after("-force_key_frames"), "expr:gte(t,n_forced*2)");
    assert_eq!(after("-c:v"), "libopenh264");
    assert_eq!(after("-i"), "in.mov");
    assert_eq!(args.last().map(String::as_str), Some("out.mp4"));
    assert!(args.iter().any(|arg| arg == "-an"));
  }
}
