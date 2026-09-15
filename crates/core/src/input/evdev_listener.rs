use std::{
  collections::VecDeque,
  path::{Path, PathBuf},
  time::{Duration, Instant},
};

use evdev::{Device, EventType, KeyCode};
use libbridgething::LauncherGesture;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::{gesture_threshold, gesture_window, trigger_hub_switch};
use crate::{chrome::ChromeCommand, handler::gateway::webapp::navigate_url_for_active, state::State};

const RETRY_BACKOFF: Duration = Duration::from_secs(5);
const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(800);

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
  let mut window: VecDeque<Instant> = VecDeque::with_capacity(gesture_threshold());
  let span = gesture_window();
  let threshold = gesture_threshold();
  let mut m_held = false;
  let mut m_hold_deadline = Box::pin(sleep(Duration::ZERO));
  let mut last_gesture = state.input.gesture().await;

  loop {
    tokio::select! {
      _ = cancel.cancelled() => return Ok(()),
      _ = &mut m_hold_deadline, if m_held => {
        m_held = false;
        if state.input.gesture().await != LauncherGesture::LongPress {
          continue;
        }
        tracing::debug!("hub gesture: KEY_M long-press");
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
        let pressed = ev.value() == 1;
        let released = ev.value() == 0;

        if key == KeyCode::KEY_M {
          if pressed {
            let gesture = state.input.gesture().await;
            if gesture != last_gesture {
              last_gesture = gesture;
              window.clear();
              m_held = false;
            }
            if gesture == LauncherGesture::LongPress {
              m_held = true;
              m_hold_deadline
                .as_mut()
                .reset(tokio::time::Instant::now() + LONG_PRESS_THRESHOLD);
            } else {
              let now = Instant::now();
              while let Some(front) = window.front() {
                if now.duration_since(*front) > span {
                  window.pop_front();
                } else {
                  break;
                }
              }
              window.push_back(now);
              tracing::trace!(count = window.len(), "hub gesture: KEY_M press");
              if window.len() >= threshold {
                window.clear();
                trigger_hub_switch(state).await;
              }
            }
          } else if released {
            m_held = false;
          }
          continue;
        }

        if !pressed {
          continue;
        }

        if key == KeyCode::KEY_ESC || key == KeyCode::KEY_1 || key == KeyCode::KEY_4 {
          handle_browser_nav(state, key).await;
        }
      }
    }
  }
}
