#[cfg(any(feature = "input", test))]
mod device;
#[cfg(feature = "input")]
mod hub_gesture;
#[cfg(any(feature = "input", test))]
mod press_window;
#[cfg(feature = "input")]
mod reader;

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
  futures::future::join_all(
    device::InputDevice::ALL
      .into_iter()
      .map(|device| reader::listen(device, state.clone(), cancel.clone())),
  )
  .await;
}

#[cfg(not(feature = "input"))]
async fn run(_state: State, cancel: CancellationToken) {
  tracing::debug!("input feature disabled; input readers idle");
  cancel.cancelled().await;
}
