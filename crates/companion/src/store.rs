use std::{
  fs,
  path::{Path, PathBuf},
  sync::Mutex,
};

use serde::{Serialize, de::DeserializeOwned};

pub fn stored<T: DeserializeOwned>(path: &Path) -> Option<T> {
  serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

pub struct JsonFile<T> {
  path: Option<PathBuf>,
  noun: &'static str,
  held: Mutex<T>,
}

impl<T: Serialize> JsonFile<T> {
  pub fn new(path: PathBuf, noun: &'static str, held: T) -> Self {
    Self {
      path: Some(path),
      noun,
      held: Mutex::new(held),
    }
  }

  pub fn in_memory(noun: &'static str, held: T) -> Self {
    Self {
      path: None,
      noun,
      held: Mutex::new(held),
    }
  }

  pub fn read<R>(&self, view: impl FnOnce(&T) -> R) -> R {
    view(&self.held.lock().unwrap())
  }

  pub fn write<R>(&self, mutate: impl FnOnce(&mut T) -> R) -> R {
    let mut held = self.held.lock().unwrap();
    let out = mutate(&mut held);
    self.flush(&held);
    out
  }

  fn flush(&self, held: &T) {
    let Some(path) = &self.path else { return };
    let Some(parent) = path.parent() else { return };
    if let Err(error) = fs::create_dir_all(parent) {
      tracing::warn!(%error, path = %parent.display(), "the {} directory could not be created", self.noun);
      return;
    }
    let body = match serde_json::to_vec_pretty(held) {
      Ok(body) => body,
      Err(error) => {
        tracing::warn!(%error, "the {} could not be serialized", self.noun);
        return;
      }
    };
    let staged = path.with_extension("json.part");
    if let Err(error) = fs::write(&staged, body).and_then(|()| fs::rename(&staged, path)) {
      tracing::warn!(%error, path = %path.display(), "the {} could not be written", self.noun);
    }
  }
}

impl<T: Serialize + PartialEq> JsonFile<T> {
  pub fn set(&self, next: T) {
    let mut held = self.held.lock().unwrap();
    if *held == next {
      return;
    }
    *held = next;
    self.flush(&held);
  }
}
