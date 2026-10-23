use std::path::Path;

use bridgething_delivery::catalog::OFFICIAL_CATALOG_URL;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
  api::OtaPollConfig,
  store::{JsonFile, stored},
};

const HOST_SETTINGS_FILE: &str = "host-settings.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HostSettings {
  #[serde(default)]
  pub capability_choices: Map<String, Value>,
  #[serde(default)]
  pub provider_priority: Vec<String>,
  #[serde(default = "polling")]
  pub ota_poll_config: Option<OtaPollConfig>,
  #[serde(default = "official_catalog")]
  pub catalog_sources: Vec<String>,
}

impl Default for HostSettings {
  fn default() -> Self {
    Self {
      capability_choices: Map::new(),
      provider_priority: Vec::new(),
      ota_poll_config: polling(),
      catalog_sources: official_catalog(),
    }
  }
}

fn polling() -> Option<OtaPollConfig> {
  Some(OtaPollConfig {
    interval_seconds: 3600,
    auto_push: true,
    root_url: None,
  })
}

fn official_catalog() -> Vec<String> {
  vec![OFFICIAL_CATALOG_URL.to_owned()]
}

pub(super) fn open(state_dir: &Path) -> JsonFile<HostSettings> {
  let path = state_dir.join(HOST_SETTINGS_FILE);
  let held = stored(&path).unwrap_or_default();
  JsonFile::new(path, "host settings", held)
}
