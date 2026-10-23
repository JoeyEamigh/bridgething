use std::{
  collections::HashMap,
  path::PathBuf,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
  provider::ResumeTarget,
  store::{JsonFile, stored},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeviceRecord {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub last_seen: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub auto_resume: Option<bool>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub resume_target: Option<ResumeTarget>,
}

impl DeviceRecord {
  fn absorb(&mut self, earlier: DeviceRecord) {
    self.last_seen = self.last_seen.max(earlier.last_seen);
    self.auto_resume = self.auto_resume.or(earlier.auto_resume);
    self.resume_target = self.resume_target.or(earlier.resume_target);
  }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Held {
  #[serde(default)]
  devices: HashMap<String, DeviceRecord>,
  #[serde(default)]
  aliases: HashMap<String, String>,
}

pub(super) struct DeviceRecords(JsonFile<Held>);

impl DeviceRecords {
  pub(super) fn in_memory() -> Self {
    Self(JsonFile::in_memory("device records", Held::default()))
  }

  pub(super) fn persisted(file: PathBuf) -> Self {
    let held = stored(&file).unwrap_or_default();
    Self(JsonFile::new(file, "device records", held))
  }

  pub(super) fn identity_of(&self, link: &str) -> Option<String> {
    self.0.read(|held| held.aliases.get(link).cloned())
  }

  pub(super) fn get(&self, key: &str) -> DeviceRecord {
    self.0.read(|held| held.devices.get(key).copied().unwrap_or_default())
  }

  pub(super) fn all(&self) -> HashMap<String, DeviceRecord> {
    self.0.read(|held| held.devices.clone())
  }

  pub(super) fn edit(&self, key: &str, change: impl FnOnce(&mut DeviceRecord)) {
    self
      .0
      .write(|held| change(held.devices.entry(key.to_owned()).or_default()));
  }

  pub(super) fn adopt(&self, link: &str, identity: &str) {
    if link == identity {
      return;
    }
    self.0.write(|held| {
      held.aliases.insert(link.to_owned(), identity.to_owned());
      if let Some(earlier) = held.devices.remove(link) {
        held.devices.entry(identity.to_owned()).or_default().absorb(earlier);
      }
    });
  }

  pub(super) fn forget(&self, keys: &[&str]) {
    self.0.write(|held| {
      for key in keys {
        held.devices.remove(*key);
      }
      held
        .aliases
        .retain(|link, identity| !keys.contains(&link.as_str()) && !keys.contains(&identity.as_str()));
    });
  }

  pub(super) fn absent_for(&self, key: &str) -> Option<Duration> {
    let seen = self.get(key).last_seen?;
    Some(Duration::from_millis(now_millis().saturating_sub(seen)))
  }

  pub(super) fn stamp(&self, keys: &[String]) {
    if keys.is_empty() {
      return;
    }
    let now = now_millis();
    self.0.write(|held| {
      for key in keys {
        held.devices.entry(key.clone()).or_default().last_seen = Some(now);
      }
    });
  }
}

fn now_millis() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|elapsed| elapsed.as_millis() as u64)
    .unwrap_or_default()
}
