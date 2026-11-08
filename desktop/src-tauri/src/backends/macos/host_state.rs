use std::{
  ffi::c_void,
  ptr::NonNull,
  sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
  },
  time::Duration,
};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSWorkspace, NSWorkspaceScreensDidSleepNotification, NSWorkspaceScreensDidWakeNotification};
use objc2_core_foundation::{CFBoolean, CFString, CFType};
use objc2_core_graphics::{
  CGDirectDisplayID, CGDisplayIsAsleep, CGEventSource, CGEventSourceStateID, CGEventType, CGGetOnlineDisplayList,
  CGSessionCopyCurrentDictionary,
};
use objc2_foundation::{
  NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSNotificationName, NSString,
};
use objc2_io_kit::{
  IO_OBJECT_NULL, IOAllowPowerChange, IODeregisterForSystemPower, IONotificationPort, IONotificationPortRef,
  IORegisterForSystemPower, IOServiceClose, io_object_t, io_service_t, kIOMessageCanSystemSleep as CAN_SLEEP,
  kIOMessageSystemHasPoweredOn as POWERED_ON, kIOMessageSystemWillSleep as WILL_SLEEP,
};

use crate::backends::host_state::{Away, AwaySource, AwayWatch, HostSignals};

const SCREEN_LOCKED: &str = "com.apple.screenIsLocked";
const SCREEN_UNLOCKED: &str = "com.apple.screenIsUnlocked";
const SESSION_SCREEN_IS_LOCKED: &str = "CGSSessionScreenIsLocked";
const ANY_INPUT: CGEventType = CGEventType(u32::MAX);
const MAX_DISPLAYS: u32 = 32;
const SLEEP_HOLD: Duration = Duration::from_secs(3);

pub fn host_state() -> HostSignals<MacAway> {
  HostSignals::new(MacAway::default(), idle_seconds)
}

fn idle_seconds() -> u32 {
  let idle = CGEventSource::seconds_since_last_event_type(CGEventSourceStateID::CombinedSessionState, ANY_INPUT);
  if idle.is_finite() && idle > 0.0 { idle as u32 } else { 0 }
}

fn screen_locked() -> bool {
  let Some(session) = CGSessionCopyCurrentDictionary() else {
    return false;
  };
  let key = CFString::from_static_str(SESSION_SCREEN_IS_LOCKED);
  let value = unsafe { session.value(std::ptr::from_ref::<CFString>(&key).cast()) };
  if value.is_null() {
    return false;
  }
  unsafe { &*(value as *const CFType) }
    .downcast_ref::<CFBoolean>()
    .is_some_and(CFBoolean::as_bool)
}

fn displays_asleep() -> bool {
  let mut displays: [CGDirectDisplayID; MAX_DISPLAYS as usize] = [0; MAX_DISPLAYS as usize];
  let mut count = 0;
  if unsafe { CGGetOnlineDisplayList(MAX_DISPLAYS, displays.as_mut_ptr(), &mut count) }.0 != 0 || count == 0 {
    return false;
  }
  displays[..count as usize]
    .iter()
    .all(|display| CGDisplayIsAsleep(*display))
}

struct Power {
  away: Arc<Away>,
  root: AtomicU32,
}

impl Power {
  fn allow(&self, argument: *mut c_void) {
    let root = self.root.load(Ordering::Acquire);
    if root != IO_OBJECT_NULL {
      IOAllowPowerChange(root, argument as isize);
    }
  }
}

unsafe extern "C-unwind" fn on_power(refcon: *mut c_void, _: io_service_t, message: u32, argument: *mut c_void) {
  let power = unsafe { &*refcon.cast::<Power>() };
  match message {
    CAN_SLEEP => power.allow(argument),
    WILL_SLEEP => {
      power.away.sleep(SLEEP_HOLD);
      power.allow(argument);
    }
    POWERED_ON => power.away.set(AwaySource::Sleep, false),
    _ => {}
  }
}

struct PowerWatch {
  power: Arc<Power>,
  port: IONotificationPortRef,
  notifier: io_object_t,
  queue: DispatchRetained<DispatchQueue>,
}

impl PowerWatch {
  fn register(away: Arc<Away>) -> Option<Self> {
    let power = Arc::new(Power {
      away,
      root: AtomicU32::new(IO_OBJECT_NULL),
    });
    let mut port: IONotificationPortRef = std::ptr::null_mut();
    let mut notifier = IO_OBJECT_NULL;
    let root = unsafe {
      IORegisterForSystemPower(
        Arc::as_ptr(&power).cast_mut().cast(),
        &mut port,
        Some(on_power),
        &mut notifier,
      )
    };
    if root == IO_OBJECT_NULL || port.is_null() {
      tracing::warn!("system power notifications were refused; sleep is not held for the car thing");
      return None;
    }
    power.root.store(root, Ordering::Release);
    let queue = DispatchQueue::new("com.bridgething.desktop.power", None);
    unsafe { IONotificationPort::set_dispatch_queue(port, Some(&queue)) };
    Some(Self {
      power,
      port,
      notifier,
      queue,
    })
  }
}

impl Drop for PowerWatch {
  fn drop(&mut self) {
    unsafe {
      IODeregisterForSystemPower(&mut self.notifier);
      IONotificationPort::set_dispatch_queue(self.port, None);
    }
    self.queue.exec_sync(|| {});
    IOServiceClose(self.power.root.load(Ordering::Acquire));
    unsafe { IONotificationPort::destroy(self.port) };
  }
}

type Observer = Retained<AnyObject>;

struct Watching {
  workspace: Retained<NSNotificationCenter>,
  distributed: Retained<NSDistributedNotificationCenter>,
  on_workspace: Vec<Observer>,
  on_distributed: Vec<Observer>,
  _power: Option<PowerWatch>,
}

impl Drop for Watching {
  fn drop(&mut self) {
    unsafe {
      for observer in &self.on_workspace {
        self.workspace.removeObserver(observer);
      }
      for observer in &self.on_distributed {
        self.distributed.removeObserver(observer);
      }
    }
  }
}

unsafe impl Send for Watching {}

#[derive(Default)]
pub struct MacAway(Mutex<Option<Watching>>);

fn observe(
  center: &NSNotificationCenter,
  name: &NSNotificationName,
  away: &Arc<Away>,
  source: AwaySource,
  on: bool,
) -> Observer {
  let away = away.clone();
  let block = RcBlock::new(move |_: NonNull<NSNotification>| away.set(source, on));
  unsafe { Retained::cast_unchecked(center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)) }
}

impl AwayWatch for MacAway {
  fn watch(&self, away: Arc<Away>) {
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    let distributed = NSDistributedNotificationCenter::defaultCenter();
    let on_workspace = unsafe {
      vec![
        observe(
          &workspace,
          NSWorkspaceScreensDidSleepNotification,
          &away,
          AwaySource::Displays,
          true,
        ),
        observe(
          &workspace,
          NSWorkspaceScreensDidWakeNotification,
          &away,
          AwaySource::Displays,
          false,
        ),
      ]
    };
    let on_distributed = vec![
      observe(
        &distributed,
        &NSString::from_str(SCREEN_LOCKED),
        &away,
        AwaySource::Locked,
        true,
      ),
      observe(
        &distributed,
        &NSString::from_str(SCREEN_UNLOCKED),
        &away,
        AwaySource::Locked,
        false,
      ),
    ];
    away.set(AwaySource::Locked, screen_locked());
    away.set(AwaySource::Displays, displays_asleep());
    away.set(AwaySource::Sleep, false);
    let power = PowerWatch::register(away);
    *self.0.lock().unwrap() = Some(Watching {
      workspace,
      distributed,
      on_workspace,
      on_distributed,
      _power: power,
    });
  }

  fn unwatch(&self) {
    self.0.lock().unwrap().take();
  }
}

#[cfg(test)]
mod tests {
  use std::thread;

  use bridgething_companion::backend::{HostEdge, HostStateBackend, HostStateInbox};
  use tokio::sync::mpsc::UnboundedReceiver;

  use super::*;

  #[derive(Default, Clone)]
  struct Captured(Arc<Mutex<Option<Arc<Away>>>>);

  impl AwayWatch for Captured {
    fn watch(&self, away: Arc<Away>) {
      *self.0.lock().unwrap() = Some(away);
    }

    fn unwatch(&self) {}
  }

  fn deliver(power: &Arc<Power>, message: u32) {
    unsafe {
      on_power(
        Arc::as_ptr(power).cast_mut().cast(),
        IO_OBJECT_NULL,
        message,
        std::ptr::null_mut(),
      )
    };
  }

  fn drained(edges: &mut UnboundedReceiver<HostEdge>) -> Vec<HostEdge> {
    std::iter::from_fn(|| edges.try_recv().ok()).collect()
  }

  fn post(name: &NSNotificationName) {
    unsafe {
      NSWorkspace::sharedWorkspace()
        .notificationCenter()
        .postNotificationName_object(name, None)
    };
  }

  #[test]
  fn the_event_source_answers_with_a_real_idle_reading() {
    let idle_s = idle_seconds();

    assert!(idle_s < 60 * 60 * 24 * 365, "idle for {idle_s}s is not a real reading");
  }

  #[test]
  fn the_real_backend_reports_away_now_and_follows_the_screens_to_sleep_and_back() {
    let backend = host_state();
    let (inbox, mut edges) = HostStateInbox::channel();

    backend.start(inbox);
    let [HostEdge::Away(initial)] = drained(&mut edges)[..] else {
      panic!("start reports away exactly once before it returns");
    };
    assert_eq!(initial, screen_locked() || displays_asleep());

    post(unsafe { NSWorkspaceScreensDidSleepNotification });
    let slept = drained(&mut edges);
    post(unsafe { NSWorkspaceScreensDidWakeNotification });
    let woke = drained(&mut edges);
    backend.stop();
    post(unsafe { NSWorkspaceScreensDidSleepNotification });

    if initial {
      assert_eq!(slept, [], "already away, so dark screens change nothing");
      assert_eq!(woke, [], "waking the screens leaves the lock in charge");
    } else {
      assert_eq!(slept, [HostEdge::Away(true)]);
      assert_eq!(woke, [HostEdge::Away(false)]);
    }
    assert_eq!(drained(&mut edges), [], "a stopped backend observes nothing");
  }

  #[test]
  fn system_sleep_is_allowed_only_once_the_away_report_settles() {
    let captured = Captured::default();
    let signals = HostSignals::new(captured.clone(), idle_seconds);
    let (inbox, mut edges) = HostStateInbox::channel();
    signals.start(inbox.clone());
    let away = captured.0.lock().unwrap().clone().expect("the backend watches");
    drained(&mut edges);
    let power = Arc::new(Power {
      away,
      root: AtomicU32::new(IO_OBJECT_NULL),
    });

    deliver(&power, CAN_SLEEP);
    assert_eq!(drained(&mut edges), [], "an idle sleep being pondered is not away");

    let sleeping = {
      let power = power.clone();
      thread::spawn(move || deliver(&power, WILL_SLEEP))
    };
    assert_eq!(edges.blocking_recv(), Some(HostEdge::Sleeping(1)));
    thread::sleep(Duration::from_millis(200));
    assert!(!sleeping.is_finished(), "sleep was allowed before the report settled");

    inbox.settle(1);
    sleeping.join().expect("the power callback returns");

    deliver(&power, POWERED_ON);
    assert_eq!(drained(&mut edges), [HostEdge::Away(false)]);
    signals.stop();
  }

  #[test]
  fn the_system_power_watch_registers_and_tears_down() {
    let away = Arc::new(Away::default());

    let watch = PowerWatch::register(away).expect("system power notifications are available");

    drop(watch);
  }

  #[test]
  fn the_real_backend_reports_idle_thresholds_from_the_event_source() {
    let backend = host_state();
    let (inbox, mut edges) = HostStateInbox::channel();
    backend.start(inbox);
    drained(&mut edges);

    backend.watch_idle(vec![0, 86_400 * 365]);

    assert_eq!(
      drained(&mut edges)
        .into_iter()
        .filter(|edge| matches!(edge, HostEdge::Idle { .. }))
        .collect::<Vec<_>>(),
      [
        HostEdge::Idle { after_s: 0, idle: true },
        HostEdge::Idle {
          after_s: 86_400 * 365,
          idle: false
        },
      ]
    );
    backend.stop();
  }
}
