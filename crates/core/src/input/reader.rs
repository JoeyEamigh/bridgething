use std::{
  path::{Path, PathBuf},
  time::{Duration, Instant},
};

use evdev::{Device, EventType, KeyCode};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::{
  device::{InputDevice, Throttle},
  hub_gesture::HubGestures,
};
use crate::{deadline::past_deadline, state::State};

const RETRY_BACKOFF: Duration = Duration::from_secs(5);

pub(super) async fn listen(device: InputDevice, state: State, cancel: CancellationToken) {
  loop {
    if cancel.is_cancelled() {
      return;
    }
    match find_device(device.name()).await {
      Some(path) => {
        tracing::info!("input: reading {device:?} on {}", path.display());
        if let Err(e) = read(device, &path, &state, &cancel).await {
          tracing::warn!("input: {device:?} on {} ended: {e}", path.display());
        }
      }
      None => tracing::debug!("input: no {} evdev node yet, retrying", device.name()),
    }
    tokio::select! {
      _ = sleep(RETRY_BACKOFF) => {}
      _ = cancel.cancelled() => return,
    }
  }
}

async fn find_device(wanted: &str) -> Option<PathBuf> {
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
    if dev_name.contains(wanted) {
      return Some(path);
    }
  }
  None
}

fn open_name(path: &Path) -> Option<String> {
  let dev = Device::open(path).ok()?;
  Some(dev.name().unwrap_or("").to_string())
}

async fn read(device: InputDevice, path: &Path, state: &State, cancel: &CancellationToken) -> Result<(), String> {
  let mut events = Device::open(path)
    .map_err(|e| format!("open: {e}"))?
    .into_event_stream()
    .map_err(|e| format!("stream: {e}"))?;
  let mut throttle = Throttle::default();
  let mut gestures = (device == InputDevice::Buttons).then(|| HubGestures::new(state));
  loop {
    let hold = gestures.as_ref().and_then(HubGestures::hold_deadline);
    tokio::select! {
      _ = cancel.cancelled() => return Ok(()),
      () = past_deadline(hold) => {
        if let Some(gestures) = gestures.as_mut() {
          gestures.held(state).await;
        }
      }
      ev = events.next_event() => {
        let ev = ev.map_err(|e| format!("read: {e}"))?;
        if device.is_activity(ev.event_type().0, ev.value()) && throttle.due(Instant::now()) {
          state.display.note_input().await;
        }
        if let Some(gestures) = gestures.as_mut()
          && ev.event_type() == EventType::KEY
        {
          gestures.key(state, KeyCode::new(ev.code()), ev.value()).await;
        }
      }
    }
  }
}
