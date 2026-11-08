use std::{
  cell::RefCell,
  ffi::c_void,
  sync::{Arc, Mutex, mpsc},
  thread::{self, JoinHandle},
  time::Duration,
};

use windows::{
  Win32::{
    Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    System::{
      LibraryLoader::GetModuleHandleW,
      Power::{
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, POWERBROADCAST_SETTING, PowerSettingRegisterNotification,
        PowerSettingUnregisterNotification, RegisterSuspendResumeNotification, UnregisterSuspendResumeNotification,
      },
      RemoteDesktop::{
        NOTIFY_FOR_THIS_SESSION, WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK, WTSFreeMemory,
        WTSINFOEXW, WTSQuerySessionInformationW, WTSRegisterSessionNotification, WTSSessionInfoEx,
        WTSUnRegisterSessionNotification,
      },
      SystemInformation::GetTickCount,
      SystemServices::GUID_CONSOLE_DISPLAY_STATE,
      Threading::GetCurrentThreadId,
    },
    UI::{
      Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
      WindowsAndMessaging::{
        CreateWindowExW, DEVICE_NOTIFY_CALLBACK, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        HWND_MESSAGE, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND, PBT_POWERSETTINGCHANGE,
        PostThreadMessageW, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_QUIT, WM_WTSSESSION_CHANGE, WNDCLASSW,
        WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
      },
    },
  },
  core::{PCWSTR, PWSTR, w},
};

use crate::backends::host_state::{Away, AwaySource, AwayWatch, HostSignals};

const DISPLAY_ON: u8 = 1;
const CLASS: PCWSTR = w!("bridgething-host-state");
const DISPLAY_SEED: Duration = Duration::from_secs(1);
const SUSPEND_HOLD: Duration = Duration::from_millis(1500);

pub fn host_state() -> HostSignals<WinAway> {
  HostSignals::new(WinAway::default(), idle_seconds)
}

fn idle_seconds() -> u32 {
  let mut input = LASTINPUTINFO {
    cbSize: size_of::<LASTINPUTINFO>() as u32,
    dwTime: 0,
  };
  if !unsafe { GetLastInputInfo(&mut input) }.as_bool() {
    return 0;
  }
  unsafe { GetTickCount() }.wrapping_sub(input.dwTime) / 1000
}

fn session_locked() -> bool {
  let mut info = PWSTR::null();
  let mut size = 0;
  let queried = unsafe {
    WTSQuerySessionInformationW(
      Some(WTS_CURRENT_SERVER_HANDLE),
      WTS_CURRENT_SESSION,
      WTSSessionInfoEx,
      &mut info,
      &mut size,
    )
  };
  if queried.is_err() || info.is_null() {
    return false;
  }
  let held = unsafe { &*info.0.cast::<WTSINFOEXW>() };
  let locked = held.Level == 1 && unsafe { held.Data.WTSInfoExLevel1.SessionFlags } == WTS_SESSIONSTATE_LOCK as i32;
  unsafe { WTSFreeMemory(info.0.cast()) };
  locked
}

struct Power {
  away: Arc<Away>,
  seeded: Mutex<Option<mpsc::SyncSender<()>>>,
}

unsafe extern "system" fn on_power(context: *const c_void, kind: u32, setting: *const c_void) -> u32 {
  let power = unsafe { &*context.cast::<Power>() };
  match kind {
    PBT_APMSUSPEND => power.away.sleep(SUSPEND_HOLD),
    PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => power.away.set(AwaySource::Sleep, false),
    PBT_POWERSETTINGCHANGE if !setting.is_null() => {
      let setting = unsafe { &*setting.cast::<POWERBROADCAST_SETTING>() };
      if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE && setting.DataLength >= 1 {
        power.away.set(AwaySource::Displays, setting.Data[0] != DISPLAY_ON);
        if let Some(seeded) = power.seeded.lock().unwrap().take() {
          let _ = seeded.send(());
        }
      }
    }
    _ => {}
  }
  0
}

thread_local! {
  static LOCKS: RefCell<Option<Arc<Away>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_message(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
  if message != WM_WTSSESSION_CHANGE {
    return unsafe { DefWindowProcW(window, message, wparam, lparam) };
  }
  let locked = match wparam.0 as u32 {
    WTS_SESSION_LOCK => true,
    WTS_SESSION_UNLOCK => false,
    _ => return LRESULT(0),
  };
  LOCKS.with_borrow(|away| {
    if let Some(away) = away {
      away.set(AwaySource::Locked, locked);
    }
  });
  LRESULT(0)
}

fn message_window() -> windows::core::Result<HWND> {
  let instance: HINSTANCE = unsafe { GetModuleHandleW(PCWSTR::null()) }?.into();
  let class = WNDCLASSW {
    lpfnWndProc: Some(on_message),
    hInstance: instance,
    lpszClassName: CLASS,
    ..Default::default()
  };
  unsafe { RegisterClassW(&class) };
  unsafe {
    CreateWindowExW(
      WINDOW_EX_STYLE::default(),
      CLASS,
      PCWSTR::null(),
      WINDOW_STYLE::default(),
      0,
      0,
      0,
      0,
      Some(HWND_MESSAGE),
      None,
      Some(instance),
      None,
    )
  }
}

fn watch_locks(away: Arc<Away>, ready: mpsc::Sender<Option<u32>>) {
  LOCKS.set(Some(away));
  let window = match message_window() {
    Ok(window) => window,
    Err(error) => {
      tracing::warn!(%error, "no message window, so session locks are not observed");
      let _ = ready.send(None);
      return;
    }
  };
  if let Err(error) = unsafe { WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) } {
    tracing::warn!(%error, "session lock notifications were refused");
  }
  let _ = ready.send(Some(unsafe { GetCurrentThreadId() }));
  let mut message = MSG::default();
  while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
    unsafe { DispatchMessageW(&message) };
  }
  unsafe {
    let _ = WTSUnRegisterSessionNotification(window);
    let _ = DestroyWindow(window);
  }
  LOCKS.set(None);
}

struct Watching {
  locks: Option<(u32, JoinHandle<()>)>,
  display: Option<isize>,
  suspend: Option<HPOWERNOTIFY>,
  _subscription: Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>,
  _power: Arc<Power>,
}

unsafe impl Send for Watching {}

impl Drop for Watching {
  fn drop(&mut self) {
    if let Some((thread_id, worker)) = self.locks.take() {
      let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
      let _ = worker.join();
    }
    if let Some(display) = self.display.take() {
      let _ = unsafe { PowerSettingUnregisterNotification(HPOWERNOTIFY(display)) };
    }
    if let Some(suspend) = self.suspend.take() {
      let _ = unsafe { UnregisterSuspendResumeNotification(suspend) };
    }
  }
}

#[derive(Default)]
pub struct WinAway(Mutex<Option<Watching>>);

impl AwayWatch for WinAway {
  fn watch(&self, away: Arc<Away>) {
    let (ready, started) = mpsc::channel();
    let locks = {
      let away = away.clone();
      thread::Builder::new()
        .name("bridgething-session-lock".to_owned())
        .spawn(move || watch_locks(away, ready))
        .inspect_err(|error| tracing::warn!(%error, "the session lock watcher could not be started"))
        .ok()
        .and_then(|worker| Some((started.recv().ok().flatten()?, worker)))
    };
    away.set(AwaySource::Locked, session_locked());
    away.set(AwaySource::Sleep, false);

    let (seeded, displayed) = mpsc::sync_channel(1);
    let power = Arc::new(Power {
      away,
      seeded: Mutex::new(Some(seeded)),
    });
    let subscription = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
      Callback: Some(on_power),
      Context: Arc::as_ptr(&power).cast_mut().cast(),
    });
    let recipient = HANDLE(std::ptr::from_ref(&*subscription).cast_mut().cast());
    let mut display = std::ptr::null_mut();
    let registered = unsafe {
      PowerSettingRegisterNotification(
        &GUID_CONSOLE_DISPLAY_STATE,
        DEVICE_NOTIFY_CALLBACK,
        recipient,
        &mut display,
      )
    };
    let display = if registered.is_ok() {
      if displayed.recv_timeout(DISPLAY_SEED).is_err() {
        tracing::warn!("windows never reported the display state; assuming the displays are on");
      }
      Some(display as isize)
    } else {
      tracing::warn!(code = registered.0, "display power notifications were refused");
      None
    };
    let suspend = unsafe { RegisterSuspendResumeNotification(recipient, DEVICE_NOTIFY_CALLBACK) }
      .inspect_err(|error| tracing::warn!(%error, "suspend notifications were refused"))
      .ok();

    *self.0.lock().unwrap() = Some(Watching {
      locks,
      display,
      suspend,
      _subscription: subscription,
      _power: power,
    });
  }

  fn unwatch(&self) {
    self.0.lock().unwrap().take();
  }
}
