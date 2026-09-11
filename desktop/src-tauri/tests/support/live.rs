use std::{future::Future, sync::Arc, time::Duration};
#[cfg(target_os = "macos")]
use std::{
  panic::AssertUnwindSafe,
  sync::atomic::{AtomicBool, Ordering},
};

use bridgething_desktop::{commands, shell::Shell};
use libbridgething::BRIDGETHING_WS_MODERN_PORT;
#[cfg(target_os = "macos")]
use objc2_core_foundation::{CFRunLoop, CFRunLoopRunResult, kCFRunLoopDefaultMode};
use tauri::{Manager, test::MockRuntime};

use super::support::{DRIVE_DEADLINE, daemon_host};

pub fn client_url_for(gateway_url: &str) -> String {
  format!("ws://{}:{BRIDGETHING_WS_MODERN_PORT}/", daemon_host(gateway_url))
}

pub async fn now_playing_settles(
  app: &tauri::App<MockRuntime>,
  holds: impl Fn(Option<&bridgething_companion::api::NowPlaying>) -> bool,
) -> Option<bridgething_companion::api::NowPlaying> {
  let deadline = tokio::time::Instant::now() + DRIVE_DEADLINE;
  loop {
    let held = commands::now_playing(app.state()).await.expect("now playing answers");
    if holds(held.as_ref()) {
      return held;
    }
    if tokio::time::Instant::now() >= deadline {
      let source = app.state::<Arc<Shell>>().session().companion_debug().arbitrated_source;
      panic!("now playing never settled; last seen {held:?} from {source:?}");
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
  }
}

pub fn runtime() -> tokio::runtime::Runtime {
  tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .expect("a multi-thread runtime")
}

#[cfg(target_os = "macos")]
const IDLE: Duration = Duration::from_millis(25);
#[cfg(target_os = "macos")]
const PUMP_SECONDS: f64 = 0.25;

#[cfg(target_os = "macos")]
pub fn drive<F: Future<Output = ()>>(lane: impl FnOnce() -> F + Send + 'static) {
  let done = Arc::new(AtomicBool::new(false));
  let failed = Arc::new(AtomicBool::new(false));
  let worker = {
    let done = Arc::clone(&done);
    let failed = Arc::clone(&failed);
    std::thread::spawn(move || {
      let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| runtime().block_on(lane())));
      failed.store(outcome.is_err(), Ordering::SeqCst);
      done.store(true, Ordering::SeqCst);
      if let Some(main) = CFRunLoop::main() {
        main.stop();
      }
    })
  };

  while !done.load(Ordering::SeqCst) {
    let spun = CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, PUMP_SECONDS, false);
    if spun == CFRunLoopRunResult::Finished {
      std::thread::sleep(IDLE);
    }
  }

  if worker.join().is_err() || failed.load(Ordering::SeqCst) {
    std::process::exit(1);
  }
}

#[cfg(not(target_os = "macos"))]
pub fn drive<F: Future<Output = ()>>(lane: impl FnOnce() -> F + Send + 'static) {
  runtime().block_on(lane());
}
