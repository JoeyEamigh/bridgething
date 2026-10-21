use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::bundle::{
  ArtifactDigest,
  fetch::{ArtifactFetch, FetchError, fetch_json},
};

pub const WAKEWORD_MODEL_FILE: &str = "hey_bridgething.btww";

pub async fn fetch_discover_manifest(
  fetch: &dyn ArtifactFetch,
  root_url: &str,
) -> Result<OtaDiscoverManifest, FetchError> {
  fetch_json(fetch, &format!("{}/manifest.json", root_url.trim_end_matches('/'))).await
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct OtaDiscoverManifest {
  pub manifest_version: u32,
  pub updated_at: String,
  pub channels: BTreeMap<String, OtaManifestChannel>,
  pub releases: BTreeMap<String, OtaManifestRelease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct OtaManifestChannel {
  pub name: String,
  pub stability: String,
  #[serde(rename(deserialize = "default", serialize = "isDefault"))]
  pub is_default: bool,
  pub latest: String,
  pub releases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct OtaManifestRelease {
  pub version: String,
  pub channel: String,
  pub yanked: Option<String>,
  #[serde(default)]
  pub deprecated: bool,
  #[serde(default)]
  pub builtin_webapps: BTreeMap<String, String>,
  pub wakeword: Option<OtaWakeword>,
  pub artifacts: Option<OtaReleaseArtifacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct OtaWakeword {
  /// the daemon version this release's model was trained against; the embedding graph is baked into the daemon.
  pub runtime: String,
  pub model: String,
  #[serde(default)]
  pub model_trained_against: BTreeMap<String, String>,
}

impl OtaWakeword {
  pub fn trained_against(&self, model: &str) -> &str {
    self.model_trained_against.get(model).unwrap_or(&self.runtime)
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct OtaReleaseArtifacts {
  pub daemon: Option<ArtifactDigest>,
  pub daemon_zst: Option<ArtifactDigest>,
  pub image_swu: Option<ArtifactDigest>,
  pub image_zck: Option<ArtifactDigest>,
  pub image_boot_zck: Option<ArtifactDigest>,
  #[serde(default)]
  pub webapps: BTreeMap<String, ArtifactDigest>,
  pub wakeword: Option<OtaWakewordArtifacts>,
  #[serde(default)]
  pub daemon_patches: BTreeMap<String, OtaPatchDigest>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct OtaWakewordArtifacts {
  pub runtime: Option<ArtifactDigest>,
  pub model: Option<ArtifactDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct OtaPatchDigest {
  pub size: u64,
  pub sha256: String,
  pub source_sha256: Option<String>,
}

impl OtaPatchDigest {
  pub fn digest(&self) -> ArtifactDigest {
    ArtifactDigest {
      size: self.size,
      sha256: self.sha256.clone(),
    }
  }
}

pub fn patch_source_matches(declared: Option<&str>, running: Option<&str>) -> bool {
  match (declared, running) {
    (Some(declared), Some(running)) => declared.eq_ignore_ascii_case(running),
    _ => true,
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OtaCompositeVersion {
  pub daemon: String,
  pub image: String,
}

impl OtaCompositeVersion {
  pub fn parse(raw: &str) -> Option<Self> {
    let (daemon, suffix) = raw.split_once('+')?;
    let image = suffix.strip_prefix("image.")?;
    if daemon.is_empty() || image.is_empty() {
      return None;
    }
    Some(Self {
      daemon: daemon.to_owned(),
      image: image.to_owned(),
    })
  }

  pub fn composite(&self) -> String {
    format!("{}+image.{}", self.daemon, self.image)
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OtaArtifactUrls {
  pub daemon_binary: String,
  pub daemon_binary_zst: String,
  pub image_swu: String,
  pub image_zck: String,
  pub image_boot_zck: String,
}

impl OtaArtifactUrls {
  pub fn build(root_url: &str, channel: &str, daemon_version: &str, image_version: &str, image_variant: &str) -> Self {
    let root = root_url.trim_end_matches('/');
    let image_name = format!("bridgething-{image_variant}-image");

    Self {
      daemon_binary: format!("{root}/daemon/{channel}/{daemon_version}/bridgething"),
      daemon_binary_zst: format!("{root}/daemon/{channel}/{daemon_version}/bridgething.zst"),
      image_swu: format!("{root}/images/{channel}/{image_version}/{image_name}.swu"),
      image_zck: format!("{root}/images/{channel}/{image_version}/{image_name}.zck"),
      image_boot_zck: format!("{root}/images/{channel}/{image_version}/{image_name}-boot.zck"),
    }
  }

  pub fn builtin_webapp(root_url: &str, channel: &str, name: &str, version: &str) -> String {
    let root = root_url.trim_end_matches('/');
    format!("{root}/webapps/{channel}/{name}/{version}/{name}.zip")
  }

  pub fn wakeword_model(root_url: &str, channel: &str, version: &str) -> String {
    let root = root_url.trim_end_matches('/');
    format!("{root}/wakeword/{channel}/model/{version}/{WAKEWORD_MODEL_FILE}")
  }

  pub fn daemon_patch(root_url: &str, channel: &str, to_version: &str, from_version: &str) -> String {
    let root = root_url.trim_end_matches('/');
    format!("{root}/daemon/{channel}/{to_version}/patches/from-{from_version}.zst")
  }
}
