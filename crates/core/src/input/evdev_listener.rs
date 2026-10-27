use std::{
  path::{Path, PathBuf},
  time::{Duration, Instant},
};

use evdev::{Device, EventType, KeyCode};
use libbridgething::LauncherGesture;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::press_window::{Key, PressWindow};
use crate::{chrome::ChromeCommand, handler::gateway::webapp::navigate_url_for_active, state::State};

const RETRY_BACKOFF: Duration = Duration::from_secs(5);
const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(800);
const DOUBLE_PRESS_WINDOW: Duration = Duration::from_millis(400);
const FIVE_PRESS_WINDOW: Duration = Duration::from_millis(1500);

fn press_gesture(gesture: LauncherGesture) -> Option<PressWindow> {
  match gesture {
    LauncherGesture::LongPress => None,
    LauncherGesture::DoublePress => Some(PressWindow::new(2, DOUBLE_PRESS_WINDOW)),
    LauncherGesture::FivePress => Some(PressWindow::new(5, FIVE_PRESS_WINDOW)),
  }
}

pub async fn listen_for_hub_gesture(state: State, cancel: CancellationToken) {
  loop {
    if cancel.is_cancelled() {
      return;
    }
    match find_gpio_keys_device().await {
      Some(path) => {
        tracing::info!("hub gesture: listening on {}", path.display());
        if let Err(e) = run_loop(&path, &state, &cancel).await {
          tracing::warn!("hub gesture loop on {} ended: {:?}", path.display(), e);
        }
      }
      None => {
        tracing::debug!("hub gesture: no gpio-keys-polled evdev node yet, retrying");
      }
    }
    tokio::select! {
      _ = sleep(RETRY_BACKOFF) => {}
      _ = cancel.cancelled() => return,
    }
  }
}

async fn find_gpio_keys_device() -> Option<PathBuf> {
  let mut rd = tokio::fs::read_dir("/dev/input").await.ok()?;
  while let Ok(Some(entry)) = rd.next_entry().await {
    let path = entry.path();
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
      continue;
    };
    if !name.starts_with("event") {
      continue;
    }
    let probe_path = path.clone();
    let dev_name = match tokio::task::spawn_blocking(move || open_name(&probe_path)).await {
      Ok(Some(n)) => n,
      _ => continue,
    };
    if dev_name.contains("gpio-keys") {
      return Some(path);
    }
  }
  None
}

fn open_name(path: &Path) -> Option<String> {
  let dev = Device::open(path).ok()?;
  Some(dev.name().unwrap_or("").to_string())
}

async fn handle_browser_nav(state: &State, key: KeyCode) {
  if !state.chrome.is_external() {
    return;
  }
  match state.active_webapp().await {
    Ok(Some(active)) if active == crate::state::BROWSER_WEBAPP_ID => {}
    _ => return,
  }
  let cmd = if key == KeyCode::KEY_ESC {
    ChromeCommand::Navigate(navigate_url_for_active(state).await)
  } else if key == KeyCode::KEY_1 {
    ChromeCommand::HistoryBack
  } else if key == KeyCode::KEY_4 {
    ChromeCommand::HistoryForward
  } else {
    return;
  };
  if let Err(e) = state.chrome.send(cmd).await {
    tracing::warn!("browser nav key: dispatch failed: {:?}", e);
  }
}

async fn run_loop(path: &Path, state: &State, cancel: &CancellationToken) -> Result<(), String> {
  let device = Device::open(path).map_err(|e| format!("open: {e}"))?;
  let mut events = device.into_event_stream().map_err(|e| format!("stream: {e}"))?;
  let mut gesture_rx = state.meta.subscribe_launcher_gesture();
  let mut gesture = *gesture_rx.borrow_and_update();
  let mut presses = press_gesture(gesture);
  let mut held = false;
  let mut hold_deadline = Box::pin(sleep(Duration::ZERO));

  loop {
    tokio::select! {
      _ = cancel.cancelled() => return Ok(()),
      Ok(()) = gesture_rx.changed() => {
        gesture = *gesture_rx.borrow_and_update();
        presses = press_gesture(gesture);
        held = false;
        tracing::debug!(?gesture, "hub gesture: launcher gesture changed");
      }
      _ = &mut hold_deadline, if held => {
        held = false;
        tracing::debug!("hub gesture: KEY_M held");
        trigger_hub_switch(state).await;
      }
      ev = events.next_event() => {
        let ev = match ev {
          Ok(ev) => ev,
          Err(e) => return Err(format!("read: {e}")),
        };
        if ev.event_type() != EventType::KEY {
          continue;
        }

        let key = KeyCode::new(ev.code());

        if let Some(window) = presses.as_mut() {
          let pressed = if key == KeyCode::KEY_M { Key::Launcher } else { Key::Other };
          if window.key(pressed, ev.value(), Instant::now()) {
            tracing::debug!(?gesture, "hub gesture: KEY_M presses");
            trigger_hub_switch(state).await;
          }
        }

        if key == KeyCode::KEY_M {
          if gesture == LauncherGesture::LongPress {
            match ev.value() {
              1 => {
                held = true;
                hold_deadline
                  .as_mut()
                  .reset(tokio::time::Instant::now() + LONG_PRESS_THRESHOLD);
              }
              0 => held = false,
              _ => {}
            }
          }
          continue;
        }

        if ev.value() != 1 {
          continue;
        }

        if key == KeyCode::KEY_ESC || key == KeyCode::KEY_1 || key == KeyCode::KEY_4 {
          handle_browser_nav(state, key).await;
        }
      }
    }
  }
}

async fn trigger_hub_switch(state: &State) {
  let Ok(Some(id)) = state.launcher_webapp().await else {
    tracing::warn!("hub gesture fired but no launcher resolves; ignoring");
    return;
  };
  if matches!(state.active_webapp().await, Ok(Some(active)) if active == id) {
    tracing::debug!("hub gesture fired while already on the launcher; ignoring");
    return;
  }
  if state.webapps.resolve(id).await.is_none() {
    state.webapps.rescan().await;
  }
  if state.webapps.resolve(id).await.is_none() {
    tracing::warn!("hub gesture fired but launcher {id} is not installed; ignoring");
    return;
  }
  if let Err(e) = state.set_active_webapp(id).await {
    tracing::warn!("hub gesture: failed to set active webapp: {:?}", e);
    return;
  }
  let url = navigate_url_for_active(state).await;
  if let Err(e) = state.chrome.send(ChromeCommand::Navigate(url)).await {
    tracing::warn!("hub gesture: failed to navigate kiosk: {:?}", e);
  } else {
    tracing::info!("hub gesture fired: switched to launcher");
  }
}
