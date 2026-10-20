use std::{
  collections::HashMap,
  path::PathBuf,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) struct PeerPresence {
  file: Option<PathBuf>,
  last_seen: HashMap<String, u64>,
}

impl PeerPresence {
  pub(super) fn in_memory() -> Self {
    Self {
      file: None,
      last_seen: HashMap::new(),
    }
  }

  pub(super) fn persisted(file: PathBuf) -> Self {
    let last_seen = match std::fs::read(&file) {
      Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(%error, path = %file.display(), "peer presence is unreadable; starting fresh");
        HashMap::new()
      }),
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
      Err(error) => {
        tracing::warn!(%error, path = %file.display(), "peer presence could not be read; starting fresh");
        HashMap::new()
      }
    };
    Self {
      file: Some(file),
      last_seen,
    }
  }

  pub(super) fn absent_for(&self, device_id: &str) -> Option<Duration> {
    let seen = *self.last_seen.get(device_id)?;
    Some(Duration::from_millis(now_millis().saturating_sub(seen)))
  }

  pub(super) fn stamp<'a>(&mut self, device_ids: impl IntoIterator<Item = &'a String>) {
    let now = now_millis();
    let mut stamped = false;
    for id in device_ids {
      self.last_seen.insert(id.clone(), now);
      stamped = true;
    }
    if stamped {
      self.persist();
    }
  }

  fn persist(&self) {
    let Some(file) = &self.file else { return };
    let staged = file.with_extension("json.tmp");
    let written = serde_json::to_vec(&self.last_seen)
      .map_err(std::io::Error::other)
      .and_then(|bytes| std::fs::write(&staged, bytes))
      .and_then(|()| std::fs::rename(&staged, file));
    if let Err(error) = written {
      tracing::warn!(%error, path = %file.display(), "peer presence could not be saved");
    }
  }
}

fn now_millis() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|elapsed| elapsed.as_millis() as u64)
    .unwrap_or_default()
}
