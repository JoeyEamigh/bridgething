#[cfg(target_os = "macos")]
use std::{
  panic::AssertUnwindSafe,
  sync::atomic::{AtomicBool, Ordering},
};
use std::{sync::Arc, time::Duration};

use bridgething_desktop::{commands, shell::Shell};
use libbridgething::{BRIDGETHING_WS_MODERN_PORT, client::PlayUri};
#[cfg(target_os = "macos")]
use objc2_core_foundation::{CFRunLoop, CFRunLoopRunResult, kCFRunLoopDefaultMode};
use support::{Channel, DRIVE_DEADLINE, Daemon, SETTLE, daemon_host, mock_app, shell_config};
use tauri::{Manager, test::MockRuntime};
use tokio::sync::mpsc;

#[path = "support/mod.rs"]
mod support;

fn client_url_for(gateway_url: &str) -> String {
  format!("ws://{}:{BRIDGETHING_WS_MODERN_PORT}/", daemon_host(gateway_url))
}

async fn now_playing_settles(
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

#[cfg(target_os = "macos")]
const IDLE: Duration = Duration::from_millis(25);
#[cfg(target_os = "macos")]
const PUMP_SECONDS: f64 = 0.25;

fn main() {
  let Ok(stream_url) = std::env::var("BRIDGETHING_STREAM_LIVE_URL") else {
    eprintln!("skipped: set BRIDGETHING_STREAM_LIVE_URL to an http(s) media url to run the live stream lane");
    return;
  };
  drive(stream_url);
}

fn runtime() -> tokio::runtime::Runtime {
  tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .expect("a multi-thread runtime")
}

#[cfg(target_os = "macos")]
fn drive(stream_url: String) {
  let done = Arc::new(AtomicBool::new(false));
  let failed = Arc::new(AtomicBool::new(false));
  let worker = {
    let done = Arc::clone(&done);
    let failed = Arc::clone(&failed);
    std::thread::spawn(move || {
      let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| runtime().block_on(lane(stream_url))));
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
fn drive(stream_url: String) {
  runtime().block_on(lane(stream_url));
}

async fn lane(stream_url: String) {
  let _ = tracing_subscriber::fmt()
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .with_test_writer()
    .try_init();

  let daemon = Daemon::shared();
  let url = daemon.url();

  let spool = tempfile::tempdir().expect("a scratch directory");
  let (tx, _rx) = mpsc::unbounded_channel();
  let shell =
    Shell::create(shell_config(url.clone(), spool.path()), Arc::new(Channel { tx })).expect("the shell builds");
  shell.start().await;
  let app = mock_app(shell);
  let device_id = commands::connect(app.state(), None)
    .await
    .expect("the daemon accepts a link");

  let client = bridgething_client::Client::connect(&client_url_for(&url))
    .await
    .expect("the client wire answers")
    .with_timeout(SETTLE);
  let scheme = stream_url
    .split_once(':')
    .map(|(head, _)| head.to_ascii_lowercase())
    .expect("the stream url carries a scheme");
  let deadline = tokio::time::Instant::now() + DRIVE_DEADLINE;
  loop {
    let announced = client
      .capabilities()
      .get()
      .await
      .expect("the capabilities surface answers")
      .capabilities;
    if announced.gateway.is_some() && announced.uri_schemes.iter().any(|claimed| claimed == &scheme) {
      break;
    }
    assert!(
      tokio::time::Instant::now() < deadline,
      "the shell never claimed the {scheme} scheme; the daemon holds {:?}",
      announced.uri_schemes
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
  }

  client
    .player()
    .play(PlayUri {
      uri: stream_url.clone(),
      context: None,
    })
    .await
    .expect("the daemon takes the stream url");

  let playing = now_playing_settles(&app, |held| {
    held.is_some_and(|now| now.playback.playing && now.track.is_some())
  })
  .await
  .expect("a playing stream");
  eprintln!("stream playing: {:?}", playing.track);

  let titled = named(&app, &stream_url)
    .await
    .expect("the origin named the station before playback started");
  eprintln!("stream title: {titled}");

  let live = now_playing_settles(&app, |held| {
    held.is_some_and(|now| {
      now.playback.position_ms > 0 && now.track.as_ref().is_some_and(|track| track.duration_ms.is_none())
    })
  })
  .await
  .expect("a live stream that reports no duration");
  eprintln!(
    "live timing: position {}ms, duration {:?}",
    live.playback.position_ms,
    live.track.as_ref().and_then(|track| track.duration_ms)
  );

  client.player().pause().await.expect("pause");
  now_playing_settles(&app, |held| held.is_some_and(|now| !now.playback.playing)).await;

  client.player().resume().await.expect("resume");
  now_playing_settles(&app, |held| held.is_some_and(|now| now.playback.playing)).await;

  commands::disconnect(app.state(), Some(device_id))
    .await
    .expect("the link drops");
  now_playing_settles(&app, |held| held.is_none()).await;
}

async fn named(app: &tauri::App<tauri::test::MockRuntime>, stream_url: &str) -> Option<String> {
  let host = url::Url::parse(stream_url)
    .ok()
    .and_then(|parsed| parsed.host_str().map(str::to_owned));
  let deadline = tokio::time::Instant::now() + DRIVE_DEADLINE;
  loop {
    let held = commands::now_playing(app.state()).await.expect("now playing answers");
    let title = held.and_then(|now| now.track).and_then(|track| track.title);
    if let Some(title) = title.filter(|title| Some(title.as_str()) != host.as_deref()) {
      return Some(title);
    }
    if tokio::time::Instant::now() >= deadline {
      return None;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
  }
}
