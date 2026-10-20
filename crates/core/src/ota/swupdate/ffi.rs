use std::{
  ffi::CString,
  io::Read,
  os::raw::{c_char, c_int, c_uint, c_void},
  path::{Path, PathBuf},
  sync::{
    Arc, Once,
    atomic::{AtomicBool, Ordering},
  },
  time::{Duration, Instant},
};

use bridgething_swupdate_sys as sys;
use libbridgething::OtaPhase;
use tokio::{
  sync::{mpsc, watch},
  task,
};

use super::{Error, ProgressTick, Selector};
const CHUNK_SIZE: usize = 64 * 1024;
const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(100);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const SWUPDATE_CTRL_SOCKET: &str = "/tmp/sockinstctrl";
const SWUPDATE_PROGRESS_SOCKET: &str = "/tmp/swupdateprog";
const PROGRESS_CONNECT_RETRY: Duration = Duration::from_millis(100);
const PROGRESS_CONNECT_BUDGET: Duration = Duration::from_secs(2);
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

unsafe extern "C" {
  static mut SOCKET_CTRL_PATH: *mut c_char;
}

fn ensure_socket_paths() {
  static ONCE: Once = Once::new();
  ONCE.call_once(|| {
    let ctrl = CString::new(SWUPDATE_CTRL_SOCKET).expect("ctrl path has no NUL");
    let prog = CString::new(SWUPDATE_PROGRESS_SOCKET).expect("progress path has no NUL");
    unsafe {
      SOCKET_CTRL_PATH = ctrl.into_raw();
      sys::SOCKET_PROGRESS_PATH = prog.into_raw();
    }
  });
}

pub async fn install_swu<F>(
  swu_path: &Path,
  selector: &Selector,
  progress: &F,
  cancel_rx: &mut watch::Receiver<bool>,
) -> Result<(), Error>
where
  F: Fn(ProgressTick) + Send + Sync,
{
  ensure_socket_paths();
  let (prog_tx, mut prog_rx) = mpsc::channel::<sys::progress_msg>(32);
  let stop = StopSignal::new();
  let stop_for_reader = stop.handle();
  let _progress_handle = task::spawn_blocking(move || progress_reader(prog_tx, stop_for_reader));

  let path = swu_path.to_path_buf();
  let selector = selector.clone();
  let send_handle = task::spawn_blocking(move || install_blocking(path, selector));

  let mut send_handle = Some(send_handle);
  let mut send_done = false;
  let mut progress_closed = false;
  let mut last_emit: Option<(ProgressKey, Instant)> = None;
  let mut last_tick: Option<ProgressTick> = None;
  let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
  heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
  heartbeat.tick().await;

  loop {
    if send_done && progress_closed {
      tracing::warn!("progress socket closed after install bytes streamed; assuming success");
      return Ok(());
    }

    tokio::select! {
      msg = prog_rx.recv(), if !progress_closed => {
        let Some(msg) = msg else {
          progress_closed = true;
          continue;
        };
        let tick = translate(&msg);
        last_tick = Some(tick);
        heartbeat.reset();
        let terminal = matches!(msg.status, sys::RECOVERY_STATUS_SUCCESS | sys::RECOVERY_STATUS_FAILURE);
        if should_emit(&mut last_emit, &tick, terminal) {
          progress(tick);
        }
        match msg.status {
          sys::RECOVERY_STATUS_SUCCESS => {
            tracing::info!("libswupdate reported SUCCESS");
            return Ok(());
          }
          sys::RECOVERY_STATUS_FAILURE => {
            let detail = info_str(&msg);
            tracing::warn!(detail = %detail, "libswupdate reported FAILURE");
            return Err(Error::InstallFailed(detail));
          }
          _ => {}
        }
      }
      _ = heartbeat.tick() => {
        let beat = last_tick.unwrap_or(ProgressTick {
          phase: OtaPhase::Writing,
          percent: 0,
          step: 0,
          nsteps: 0,
          dwl_percent: 0,
          dwl_bytes: 0,
          eta_ms: None,
        });
        progress(beat);
      }
      res = wait_send(&mut send_handle), if send_handle.is_some() => {
        send_done = true;
        match res {
          Ok(Ok(())) => tracing::debug!("install bytes streamed; awaiting libswupdate completion via progress socket"),
          Ok(Err(e)) => return Err(e),
          Err(e) => return Err(Error::Ipc(format!("install task panic: {e}"))),
        }
      }
      _ = cancel_rx.changed() => {
        if *cancel_rx.borrow() {
          tracing::info!("ota cancellation observed; aborting install");
          return Err(Error::Cancelled);
        }
      }
    }
  }
}

async fn wait_send(
  handle: &mut Option<task::JoinHandle<Result<(), Error>>>,
) -> Result<Result<(), Error>, task::JoinError> {
  let h = handle.take().expect("wait_send only called when handle is Some");
  h.await
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProgressKey {
  phase: OtaPhase,
  percent: u8,
  step: u8,
  dwl_percent: u8,
}

fn should_emit(last: &mut Option<(ProgressKey, Instant)>, tick: &ProgressTick, terminal: bool) -> bool {
  let now = Instant::now();
  let key = ProgressKey {
    phase: tick.phase,
    percent: tick.percent,
    step: tick.step,
    dwl_percent: tick.dwl_percent,
  };
  let (phase_changed, dup, intervaled) = match last {
    Some((prev, t)) => (
      prev.phase != key.phase,
      *prev == key,
      now.duration_since(*t) < PROGRESS_MIN_INTERVAL,
    ),
    None => (true, false, false),
  };
  if !terminal && !phase_changed && (dup || intervaled) {
    return false;
  }
  *last = Some((key, now));
  true
}

fn write_cstr_field(field: &mut [c_char], value: &str) -> Result<(), Error> {
  let bytes = value.as_bytes();
  if bytes.len() + 1 > field.len() {
    return Err(Error::Ipc(format!(
      "selector field {value:?} too long for swupdate_request slot ({} bytes)",
      field.len()
    )));
  }
  for (dst, src) in field.iter_mut().zip(bytes.iter().copied().chain(std::iter::repeat(0))) {
    *dst = src as c_char;
  }
  Ok(())
}

fn install_blocking(swu_path: PathBuf, selector: Selector) -> Result<(), Error> {
  let mut file = std::fs::File::open(&swu_path)?;
  let total_len = file.metadata()?.len();
  let mut buf = vec![0u8; CHUNK_SIZE];

  // SAFETY: every libswupdate IPC call below is documented blocking-but-safe
  unsafe {
    let mut req: sys::swupdate_request = std::mem::zeroed();
    sys::swupdate_prepare_req(&mut req);
    req.apiversion = sys::SWUPDATE_API_VERSION;
    req.source = sys::sourcetype_SOURCE_LOCAL;
    req.dry_run = sys::run_type_RUN_INSTALL;
    write_cstr_field(&mut req.software_set, &selector.software_set)?;
    write_cstr_field(&mut req.running_mode, &selector.running_mode)?;

    let fd = sys::ipc_inst_start_ext(
      &mut req as *mut _ as *mut c_void,
      std::mem::size_of::<sys::swupdate_request>() as isize,
    );
    if fd < 0 {
      return Err(Error::Ipc(format!("ipc_inst_start_ext returned {fd}")));
    }

    let mut sent = 0u64;
    loop {
      let n = match file.read(&mut buf) {
        Ok(0) => break,
        Ok(n) => n,
        Err(err) => {
          sys::ipc_end(fd);
          return Err(Error::Io(err));
        }
      };
      let r = sys::ipc_send_data(fd, buf.as_ptr() as *mut c_char, n as c_int);
      if r < 0 {
        sys::ipc_end(fd);
        return Err(Error::Ipc(format!(
          "ipc_send_data returned {r} after {sent}/{total_len}"
        )));
      }
      sent += n as u64;
    }

    sys::ipc_end(fd);
  }
  Ok(())
}

struct StopSignal(Arc<AtomicBool>);

impl StopSignal {
  fn new() -> Self {
    Self(Arc::new(AtomicBool::new(false)))
  }

  fn handle(&self) -> Arc<AtomicBool> {
    self.0.clone()
  }
}

impl Drop for StopSignal {
  fn drop(&mut self) {
    self.0.store(true, Ordering::Relaxed);
  }
}

fn progress_reader(tx: mpsc::Sender<sys::progress_msg>, stop: Arc<AtomicBool>) {
  let Some(mut fd) = connect_progress(&stop) else {
    return;
  };

  while !stop.load(Ordering::Relaxed) {
    let mut msg: sys::progress_msg = unsafe { std::mem::zeroed() };
    // SAFETY: `fd` is the connected progress socket; the _nb arm polls instead of blocking
    let r = unsafe { sys::progress_ipc_receive_nb(&mut fd, &mut msg) };
    if r == 0 {
      std::thread::sleep(PROGRESS_POLL_INTERVAL);
      continue;
    }
    if r < 0 {
      tracing::debug!("progress_ipc_receive returned {r}; reader exiting");
      break;
    }
    let terminal = matches!(msg.status, sys::RECOVERY_STATUS_SUCCESS | sys::RECOVERY_STATUS_FAILURE);
    if tx.blocking_send(msg).is_err() || terminal {
      break;
    }
  }

  if fd >= 0 {
    // SAFETY: the fd is ours and nothing else holds it
    unsafe { libc::close(fd) };
  }
}

fn connect_progress(stop: &AtomicBool) -> Option<c_int> {
  let deadline = Instant::now() + PROGRESS_CONNECT_BUDGET;
  loop {
    if stop.load(Ordering::Relaxed) {
      return None;
    }
    let fd = unsafe { sys::progress_ipc_connect(false) };
    if fd >= 0 {
      return Some(fd);
    }
    if Instant::now() >= deadline {
      tracing::warn!("swupdate progress socket unreachable; no progress events will surface");
      return None;
    }
    std::thread::sleep(PROGRESS_CONNECT_RETRY);
  }
}

fn translate(msg: &sys::progress_msg) -> ProgressTick {
  ProgressTick {
    phase: OtaPhase::Writing,
    percent: msg.cur_percent.min(100) as u8,
    step: msg.cur_step.min(u8::MAX as c_uint) as u8,
    nsteps: msg.nsteps.min(u8::MAX as c_uint) as u8,
    dwl_percent: msg.dwl_percent.min(100) as u8,
    dwl_bytes: u32::try_from(msg.dwl_bytes).unwrap_or(u32::MAX),
    eta_ms: None,
  }
}

fn info_str(msg: &sys::progress_msg) -> String {
  let bytes = &msg.info[..msg.infolen.min(msg.info.len() as c_uint) as usize];
  let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
  let utf8: Vec<u8> = bytes[..end]
    .iter()
    .map(|b| u8::from_ne_bytes(b.to_ne_bytes()))
    .collect();
  String::from_utf8_lossy(&utf8).into_owned()
}

#[cfg(test)]
mod tests {
  use std::{io::Write, os::unix::net::UnixListener};

  use super::*;

  fn serve_one_install(listener: UnixListener) -> std::io::Result<usize> {
    let (mut stream, _) = listener.accept()?;
    let size = std::mem::size_of::<sys::ipc_message>();
    let mut msg: sys::ipc_message = unsafe { std::mem::zeroed() };

    stream.read_exact(unsafe { std::slice::from_raw_parts_mut(&mut msg as *mut _ as *mut u8, size) })?;
    msg.type_ = sys::msgtype_ACK as c_int;
    stream.write_all(unsafe { std::slice::from_raw_parts(&msg as *const _ as *const u8, size) })?;

    let mut payload = Vec::new();
    stream.read_to_end(&mut payload)?;
    Ok(payload.len())
  }

  #[tokio::test]
  async fn install_returns_when_the_progress_socket_never_answers() {
    ensure_socket_paths();
    let _ = std::fs::remove_file(SWUPDATE_CTRL_SOCKET);
    let listener = UnixListener::bind(SWUPDATE_CTRL_SOCKET).expect("bind the stand-in control socket");
    let server = task::spawn_blocking(move || serve_one_install(listener));

    let dir = std::env::temp_dir().join(format!("bridgething-swupdate-test-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let swu = dir.join("update.swu");
    std::fs::write(&swu, vec![0x5au8; 4096]).unwrap();

    let selector = Selector {
      software_set: "stable".into(),
      running_mode: "slot_b".into(),
    };
    let (_cancel_tx, mut cancel_rx) = watch::channel(false);
    let progress = |_: ProgressTick| {};

    let outcome = tokio::time::timeout(
      PROGRESS_CONNECT_BUDGET * 4,
      install_swu(&swu, &selector, &progress, &mut cancel_rx),
    )
    .await
    .expect("install must give a verdict once the progress socket is written off, not heartbeat forever");

    assert!(
      outcome.is_ok(),
      "streamed bytes with no verdict assume success: {outcome:?}"
    );
    assert_eq!(
      server.await.unwrap().unwrap(),
      4096,
      "the whole .swu reached the socket"
    );

    let _ = std::fs::remove_file(SWUPDATE_CTRL_SOCKET);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[tokio::test]
  async fn progress_reader_stops_instead_of_stranding_the_blocking_pool() {
    ensure_socket_paths();
    let (tx, _rx) = mpsc::channel::<sys::progress_msg>(1);
    let stop = StopSignal::new();
    let handle = {
      let stop = stop.handle();
      task::spawn_blocking(move || progress_reader(tx, stop))
    };

    tokio::time::sleep(PROGRESS_CONNECT_RETRY * 2).await;
    drop(stop);
    tokio::time::timeout(PROGRESS_CONNECT_BUDGET * 4, handle)
      .await
      .expect("progress reader must exit; a stranded one blocks runtime shutdown forever")
      .expect("progress reader panicked");
  }
}
