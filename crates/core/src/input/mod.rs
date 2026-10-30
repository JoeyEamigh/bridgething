#[cfg(feature = "input")]
mod evdev_listener;
#[cfg(any(feature = "input", test))]
mod press_window;
#[cfg(any(feature = "input", test))]
mod wake;
#[cfg(feature = "input")]
mod wake_listener;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::state::State;

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
  tokio::join!(
    evdev_listener::listen_for_hub_gesture(state.clone(), cancel.clone()),
    wake_listener::listen_for_wake(state, cancel),
  );
}

#[cfg(not(feature = "input"))]
async fn run(_state: State, cancel: CancellationToken) {
  tracing::debug!("input feature disabled; gesture listener idle");
  cancel.cancelled().await;
}
