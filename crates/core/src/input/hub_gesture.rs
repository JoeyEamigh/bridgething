use std::time::{Duration, Instant};

use evdev::KeyCode;
use libbridgething::LauncherGesture;
use tokio::sync::watch;

use super::press_window::{Key, PressWindow};
use crate::{chrome::ChromeCommand, handler::gateway::webapp::navigate_url_for_active, state::State};

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

pub(super) struct HubGestures {
  gesture_rx: watch::Receiver<LauncherGesture>,
  gesture: LauncherGesture,
  presses: Option<PressWindow>,
  hold_until: Option<tokio::time::Instant>,
}

impl HubGestures {
  pub(super) fn new(state: &State) -> Self {
    let mut gesture_rx = state.meta.subscribe_launcher_gesture();
    let gesture = *gesture_rx.borrow_and_update();
    Self {
      gesture_rx,
      gesture,
      presses: press_gesture(gesture),
      hold_until: None,
    }
  }

  pub(super) fn hold_deadline(&self) -> Option<tokio::time::Instant> {
    self.hold_until
  }

  fn refresh(&mut self) {
    if !self.gesture_rx.has_changed().unwrap_or(false) {
      return;
    }
    self.gesture = *self.gesture_rx.borrow_and_update();
    self.presses = press_gesture(self.gesture);
    self.hold_until = None;
    tracing::debug!(gesture = ?self.gesture, "hub gesture: launcher gesture changed");
  }

  pub(super) async fn held(&mut self, state: &State) {
    self.refresh();
    if self.hold_until.take().is_some() {
      tracing::debug!("hub gesture: KEY_M held");
      trigger_hub_switch(state).await;
    }
  }

  pub(super) async fn key(&mut self, state: &State, key: KeyCode, value: i32) {
    self.refresh();
    if let Some(window) = self.presses.as_mut() {
      let pressed = if key == KeyCode::KEY_M {
        Key::Launcher
      } else {
        Key::Other
      };
      if window.key(pressed, value, Instant::now()) {
        tracing::debug!(gesture = ?self.gesture, "hub gesture: KEY_M presses");
        trigger_hub_switch(state).await;
      }
    }

    if key == KeyCode::KEY_M {
      if self.gesture == LauncherGesture::LongPress {
        match value {
          1 => self.hold_until = Some(tokio::time::Instant::now() + LONG_PRESS_THRESHOLD),
          0 => self.hold_until = None,
          _ => {}
        }
      }
      return;
    }

    if value == 1 && [KeyCode::KEY_ESC, KeyCode::KEY_1, KeyCode::KEY_4].contains(&key) {
      handle_browser_nav(state, key).await;
    }
  }
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
  } else {
    ChromeCommand::HistoryForward
  };
  if let Err(e) = state.chrome.send(cmd).await {
    tracing::warn!("browser nav key: dispatch failed: {:?}", e);
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
