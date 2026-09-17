#[cfg(feature = "input")]
mod evdev_listener;

use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{chrome::ChromeCommand, handler::gateway::webapp::navigate_url_for_active, state::State};

const HUB_GESTURE_THRESHOLD: usize = 5;
const HUB_GESTURE_WINDOW: Duration = Duration::from_millis(1500);

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

#[cfg_attr(not(feature = "input"), allow(dead_code))]
pub(crate) fn gesture_window() -> Duration {
  HUB_GESTURE_WINDOW
}

#[cfg_attr(not(feature = "input"), allow(dead_code))]
pub(crate) fn gesture_threshold() -> usize {
  HUB_GESTURE_THRESHOLD
}
