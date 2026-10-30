use std::{path::PathBuf, time::Instant};

use evdev::Device;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::{
  evdev_listener::{RETRY_BACKOFF, find_device},
  wake::{Source, Throttle},
};
use crate::state::State;

pub async fn listen_for_wake(state: State, cancel: CancellationToken) {
  tokio::join!(
    watch(Source::Buttons, state.clone(), cancel.clone()),
    watch(Source::Dial, state.clone(), cancel.clone()),
    watch(Source::Touch, state, cancel),
  );
}

async fn watch(source: Source, state: State, cancel: CancellationToken) {
  loop {
    if cancel.is_cancelled() {
      return;
    }
    match find_device(source.device_name()).await {
      Some(path) => {
        tracing::info!("screen wake: listening for {source:?} on {}", path.display());
        if let Err(e) = run(source, &path, &state, &cancel).await {
          tracing::warn!("screen wake loop for {source:?} on {} ended: {e}", path.display());
        }
      }
      None => tracing::debug!("screen wake: no {} evdev node yet, retrying", source.device_name()),
    }
    tokio::select! {
      _ = sleep(RETRY_BACKOFF) => {}
      _ = cancel.cancelled() => return,
    }
  }
}

async fn run(source: Source, path: &PathBuf, state: &State, cancel: &CancellationToken) -> Result<(), String> {
  let device = Device::open(path).map_err(|e| format!("open: {e}"))?;
  let mut events = device.into_event_stream().map_err(|e| format!("stream: {e}"))?;
  let mut throttle = Throttle::default();
  loop {
    tokio::select! {
      _ = cancel.cancelled() => return Ok(()),
      ev = events.next_event() => {
        let ev = ev.map_err(|e| format!("read: {e}"))?;
        if source.wakes(ev.event_type().0, ev.value()) && throttle.due(Instant::now()) {
          tracing::debug!(?source, "screen wake: input");
          state.presence.touch();
        }
      }
    }
  }
}
