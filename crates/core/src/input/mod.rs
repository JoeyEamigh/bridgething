#[cfg(feature = "input")]
mod evdev_listener;

use std::time::Duration;

use libbridgething::LauncherGesture;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{chrome::ChromeCommand, handler::gateway::webapp::navigate_url_for_active, state::State};

const FIVE_PRESS_THRESHOLD: usize = 5;
const FIVE_PRESS_WINDOW: Duration = Duration::from_millis(1500);
const DOUBLE_PRESS_THRESHOLD: usize = 2;
const DOUBLE_PRESS_WINDOW: Duration = Duration::from_millis(400);

#[derive(Debug)]
pub struct InputManager {
  _handle: JoinHandle<()>,
}

impl InputManager {
  pub fn spawn(state: State) -> Self {
    let cancel_token = CancellationToken::new();
    let handle = tokio::spawn(run(state, cancel_token));
    Self { _handle: handle }
  }
}

#[cfg(feature = "input")]
async fn run(state: State, cancel: CancellationToken) {
  evdev_listener::listen_for_hub_gesture(state, cancel).await;
}

#[cfg(not(feature = "input"))]
async fn run(_state: State, cancel: CancellationToken) {
  tracing::debug!("input feature disabled; gesture listener idle");
  cancel.cancelled().await;
}

#[cfg_attr(not(feature = "input"), allow(dead_code))]
pub(crate) async fn trigger_hub_switch(state: &State) {
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

/// How many presses of m, inside what window, fire a press gesture.
#[cfg_attr(not(feature = "input"), allow(dead_code))]
pub(crate) fn press_gesture(gesture: LauncherGesture) -> (usize, Duration) {
  match gesture {
    LauncherGesture::DoublePress => (DOUBLE_PRESS_THRESHOLD, DOUBLE_PRESS_WINDOW),
    LauncherGesture::FivePress | LauncherGesture::LongPress => (FIVE_PRESS_THRESHOLD, FIVE_PRESS_WINDOW),
  }
}
