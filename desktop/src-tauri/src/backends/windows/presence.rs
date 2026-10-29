use std::{
  ffi::c_void,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
  },
  thread::{self, JoinHandle},
  time::Duration,
};

use bridgething_companion::backend::{HostDisplay, PresenceBackend, PresenceInbox, PresenceReport};
use windows::Win32::{
  Foundation::HANDLE,
  System::{
    Power::{
      DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, POWERBROADCAST_SETTING, PowerSettingRegisterNotification,
      PowerSettingUnregisterNotification,
    },
    SystemInformation::GetTickCount64,
    SystemServices::GUID_CONSOLE_DISPLAY_STATE,
  },
  UI::{
    Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
    WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_POWERSETTINGCHANGE},
  },
};

const POLL: Duration = Duration::from_secs(1);

/// `GUID_CONSOLE_DISPLAY_STATE` payload values.
const MONITOR_OFF: u8 = 0;
const MONITOR_ON: u8 = 1;
const MONITOR_DIMMED: u8 = 2;

struct Shared {
  monitor: AtomicU8,
}

unsafe extern "system" fn on_power_setting(context: *const c_void, kind: u32, setting: *const c_void) -> u32 {
  if kind != PBT_POWERSETTINGCHANGE || context.is_null() || setting.is_null() {
    return 0;
  }
  // SAFETY: `context` is the `Arc<Shared>` leaked in `start`, alive until `stop` unregisters; `setting`
  // is a POWERBROADCAST_SETTING the OS keeps valid for the duration of this call.
  let (shared, setting) = unsafe {
    (
      &*(context as *const Shared),
      &*(setting as *const POWERBROADCAST_SETTING),
    )
  };
  if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE && setting.DataLength >= 1 {
    shared.monitor.store(setting.Data[0], Ordering::SeqCst);
  }
  0
}

struct Registration {
  handle: *mut c_void,
  context: *const Shared,
  _params: Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>,
}

// SAFETY: the raw pointers are an OS registration handle and the leaked Arc, both only touched in start/stop.
unsafe impl Send for Registration {}

pub struct WinPresence {
  shared: Arc<Shared>,
  stop: Arc<AtomicBool>,
  worker: Mutex<Option<JoinHandle<()>>>,
  registration: Mutex<Option<Registration>>,
}

impl Default for WinPresence {
  fn default() -> Self {
    Self {
      shared: Arc::new(Shared {
        monitor: AtomicU8::new(MONITOR_ON),
      }),
      stop: Arc::new(AtomicBool::new(false)),
      worker: Mutex::new(None),
      registration: Mutex::new(None),
    }
  }
}

fn idle_seconds() -> u32 {
  let mut info = LASTINPUTINFO {
    cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
    dwTime: 0,
  };
  // SAFETY: `info` is a valid LASTINPUTINFO with `cbSize` set.
  if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
    return 0;
  }
  // SAFETY: no preconditions.
  let now = unsafe { GetTickCount64() } as u32;
  now.wrapping_sub(info.dwTime) / 1000
}

fn read(shared: &Shared) -> PresenceReport {
  let display = match shared.monitor.load(Ordering::SeqCst) {
    MONITOR_OFF => HostDisplay::Off,
    MONITOR_DIMMED => HostDisplay::Dim,
    _ => HostDisplay::Active,
  };
  PresenceReport {
    display,
    idle_s: idle_seconds(),
  }
}

impl PresenceBackend for WinPresence {
  fn start(&self, inbox: Arc<PresenceInbox>) {
    self.stop.store(false, Ordering::SeqCst);
    self.shared.monitor.store(MONITOR_ON, Ordering::SeqCst);

    let context = Arc::into_raw(self.shared.clone());
    let params = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
      Callback: Some(on_power_setting),
      Context: context as *mut c_void,
    });
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: `params` outlives the registration (held in `Registration`); `handle` is a valid out pointer.
    let status = unsafe {
      PowerSettingRegisterNotification(
        &GUID_CONSOLE_DISPLAY_STATE,
        DEVICE_NOTIFY_CALLBACK,
        HANDLE(&*params as *const DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS as *mut c_void),
        &mut handle,
      )
    };
    if status.0 == 0 {
      *self.registration.lock().unwrap() = Some(Registration {
        handle,
        context,
        _params: params,
      });
    } else {
      tracing::warn!(
        code = status.0,
        "presence: cannot watch the monitor power state; using idle time only"
      );
      // SAFETY: balances the `into_raw` above; nothing else holds `context`.
      drop(unsafe { Arc::from_raw(context) });
    }

    let stop = self.stop.clone();
    let shared = self.shared.clone();
    let worker = thread::Builder::new()
      .name("presence".to_owned())
      .spawn(move || {
        let mut last = None;
        while !stop.load(Ordering::SeqCst) {
          let report = read(&shared);
          if last != Some(report.display) {
            last = Some(report.display);
            inbox.on_changed(report.display, report.idle_s);
          }
          thread::sleep(POLL);
        }
      })
      .ok();
    if let Some(previous) = std::mem::replace(&mut *self.worker.lock().unwrap(), worker) {
      let _ = previous.join();
    }
  }

  fn stop(&self) {
    self.stop.store(true, Ordering::SeqCst);
    if let Some(worker) = self.worker.lock().unwrap().take() {
      let _ = worker.join();
    }
    if let Some(registration) = self.registration.lock().unwrap().take() {
      // SAFETY: `handle` came from a successful registration; after it returns no callback runs, so
      // the leaked Arc can be reclaimed.
      unsafe {
        let _ = PowerSettingUnregisterNotification(HPOWERNOTIFY(registration.handle as isize));
        drop(Arc::from_raw(registration.context));
      }
    }
  }

  fn snapshot(&self) -> PresenceReport {
    read(&self.shared)
  }
}
