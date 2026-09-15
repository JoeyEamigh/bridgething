use std::{
  path::{Path, PathBuf},
  sync::Arc,
};

use libbridgething::LauncherGesture;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

const INPUT_PREFS_FILE: &str = "input.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct InputPrefs {
  #[serde(default)]
  gesture: LauncherGesture,
}

fn load_prefs(path: &Path) -> LauncherGesture {
  std::fs::read(path)
    .ok()
    .and_then(|bytes| serde_json::from_slice::<InputPrefs>(&bytes).ok())
    .map(|prefs| prefs.gesture)
    .unwrap_or_default()
}

async fn save_prefs(path: &Path, prefs: InputPrefs) {
  if let Some(dir) = path.parent()
    && let Err(err) = tokio::fs::create_dir_all(dir).await
  {
    tracing::warn!(path = %path.display(), "input: cannot create prefs dir: {err}");
    return;
  }
  let body = match serde_json::to_vec(&prefs) {
    Ok(body) => body,
    Err(err) => {
      tracing::warn!("input: cannot serialize prefs: {err}");
      return;
    }
  };
  let tmp = path.with_extension("tmp");
  if let Err(err) = tokio::fs::write(&tmp, body).await {
    tracing::warn!(path = %path.display(), "input: cannot write prefs: {err}");
  } else if let Err(err) = tokio::fs::rename(&tmp, path).await {
    tracing::warn!(path = %path.display(), "input: cannot replace prefs: {err}");
  }
}

#[derive(Debug, Clone)]
pub struct InputSettings {
  inner: Arc<RwLock<LauncherGesture>>,
  path: PathBuf,
}

impl InputSettings {
  pub fn load() -> Self {
    let path = crate::paths::state_dir().join(INPUT_PREFS_FILE);
    let gesture = load_prefs(&path);
    Self {
      inner: Arc::new(RwLock::new(gesture)),
      path,
    }
  }

  pub async fn gesture(&self) -> LauncherGesture {
    *self.inner.read().await
  }

  pub async fn set_gesture(&self, gesture: LauncherGesture) {
    *self.inner.write().await = gesture;
    save_prefs(&self.path, InputPrefs { gesture }).await;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn scratch_path(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root.join(INPUT_PREFS_FILE)
  }

  fn settings_at(path: PathBuf) -> InputSettings {
    let gesture = load_prefs(&path);
    InputSettings {
      inner: Arc::new(RwLock::new(gesture)),
      path,
    }
  }

  #[tokio::test]
  async fn gesture_persists_across_reload() {
    let path = scratch_path("bridgething-input-settings-test");
    let settings = settings_at(path.clone());
    assert_eq!(settings.gesture().await, LauncherGesture::FivePress);
    settings.set_gesture(LauncherGesture::LongPress).await;
    assert_eq!(settings.gesture().await, LauncherGesture::LongPress);

    let reloaded = settings_at(path);
    assert_eq!(reloaded.gesture().await, LauncherGesture::LongPress);
  }

  #[tokio::test]
  async fn corrupt_prefs_fall_back_to_default() {
    let path = scratch_path("bridgething-input-settings-corrupt-test");
    std::fs::write(&path, b"not json").unwrap();
    let settings = settings_at(path);
    assert_eq!(settings.gesture().await, LauncherGesture::FivePress);
  }
}
