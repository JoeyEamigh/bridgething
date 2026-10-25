use std::{cmp::Ordering, collections::BTreeMap};

use libbridgething::{WebappInfo, WebappSource};
use serde::Deserialize;
use uuid::Uuid;

use crate::bundle::{
  ArtifactDigest,
  fetch::{ArtifactFetch, FetchError},
};

pub const OFFICIAL_CATALOG_URL: &str = "https://apps.bridgething.com/catalog.json";

pub const CATALOG_SCHEMA: &str = "catalog.v1";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Catalog {
  pub schema: String,
  pub apps: Vec<AppEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AppEntry {
  pub id: String,
  pub name: String,
  pub versions: Vec<AppVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AppVersion {
  pub version: String,
  pub released_at: String,
  pub download: Download,
  #[serde(default)]
  pub extension: Option<serde_json::Value>,
  pub min_libbridgething_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Download {
  pub url: String,
  pub size: u64,
  pub sha256: String,
}

impl Download {
  pub fn digest(&self) -> ArtifactDigest {
    ArtifactDigest {
      size: self.size,
      sha256: self.sha256.clone(),
    }
  }
}

impl AppEntry {
  pub fn uuid(&self) -> Option<Uuid> {
    Uuid::parse_str(&self.id).ok()
  }

  pub fn declares_extension(&self) -> bool {
    self.versions.iter().any(|version| version.extension.is_some())
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogUpdate {
  pub app_id: String,
  pub name: String,
  pub installed_version: String,
  pub target: AppVersion,
  pub source_url: String,
}

pub async fn fetch_catalog(fetch: &dyn ArtifactFetch, url: &str) -> Result<Catalog, FetchError> {
  let body = fetch.bytes(url).await?;
  let catalog: Catalog = serde_json::from_slice(&body).map_err(|e| FetchError::Decode(e.to_string()))?;
  if catalog.schema != CATALOG_SCHEMA {
    return Err(FetchError::Decode(format!(
      "{url} is a {} catalog, not {CATALOG_SCHEMA}",
      catalog.schema
    )));
  }
  Ok(catalog)
}

pub fn compare_versions(a: &str, b: &str) -> Ordering {
  let left = version_components(a);
  let right = version_components(b);
  (0..left.len().max(right.len()))
    .map(|at| {
      left
        .get(at)
        .copied()
        .unwrap_or(0)
        .cmp(&right.get(at).copied().unwrap_or(0))
    })
    .find(|order| order.is_ne())
    .unwrap_or(Ordering::Equal)
}

fn version_components(raw: &str) -> Vec<u64> {
  let trimmed = raw.trim();
  let bare = trimmed
    .strip_prefix('v')
    .or_else(|| trimmed.strip_prefix('V'))
    .unwrap_or(trimmed);
  let core = bare.split(['-', '+']).next().unwrap_or_default();
  core
    .split('.')
    .map(|part| {
      let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
      digits.parse().unwrap_or(0)
    })
    .collect()
}

pub fn satisfies(device_version: &str, minimum: &str) -> bool {
  compare_versions(device_version, minimum).is_ge()
}

fn released_at(raw: &str) -> Option<i64> {
  chrono::DateTime::parse_from_rfc3339(raw)
    .ok()
    .map(|instant| instant.timestamp_millis())
}

pub fn newest_compatible<'a>(app: &'a AppEntry, lib_version: &str) -> Option<&'a AppVersion> {
  let mut ordered: Vec<(usize, Option<i64>, &AppVersion)> = app
    .versions
    .iter()
    .enumerate()
    .map(|(at, version)| (at, released_at(&version.released_at), version))
    .collect();
  ordered.sort_by(|(left_at, left, _), (right_at, right, _)| match (left, right) {
    (Some(left), Some(right)) => right.cmp(left).then(left_at.cmp(right_at)),
    (Some(_), None) => Ordering::Less,
    (None, Some(_)) => Ordering::Greater,
    (None, None) => left_at.cmp(right_at),
  });
  ordered
    .into_iter()
    .map(|(_, _, version)| version)
    .find(|version| satisfies(lib_version, &version.min_libbridgething_version))
}

pub fn updates(
  catalogs: &BTreeMap<String, Catalog>,
  installed: &[WebappInfo],
  lib_version: &str,
) -> Vec<CatalogUpdate> {
  let mut out: Vec<CatalogUpdate> = installed
    .iter()
    .filter(|info| info.source == WebappSource::Installed && info.extension.is_none())
    .filter_map(|info| {
      let source_url = info.provenance.as_deref()?;
      let app = catalogs
        .get(source_url)?
        .apps
        .iter()
        .find(|app| app.uuid() == Some(info.id))?;
      if app.declares_extension() {
        return None;
      }
      let newest = newest_compatible(app, lib_version)?;
      compare_versions(&newest.version, &info.version)
        .is_gt()
        .then(|| CatalogUpdate {
          app_id: app.id.clone(),
          name: app.name.clone(),
          installed_version: info.version.clone(),
          target: newest.clone(),
          source_url: source_url.to_owned(),
        })
    })
    .collect();
  out.sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.app_id.cmp(&right.app_id)));
  out
}

pub fn subscriptions(urls: impl IntoIterator<Item = String>) -> Vec<String> {
  let mut out: Vec<String> = Vec::new();
  for url in urls {
    let url = url.trim();
    if !url.is_empty() && !out.iter().any(|held| held == url) {
      out.push(url.to_owned());
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use libbridgething::WebappRole;

  use super::*;

  const SOURCE: &str = "https://apps.example/catalog.json";
  const OTHER: &str = "https://other.example/catalog.json";
  const WEATHER: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001);

  fn version(version: &str, released_at: &str, min_lib: &str) -> AppVersion {
    AppVersion {
      version: version.to_owned(),
      released_at: released_at.to_owned(),
      download: Download {
        url: format!("https://apps.example/r/{version}.zip"),
        size: 1_024,
        sha256: "a".repeat(64),
      },
      extension: None,
      min_libbridgething_version: min_lib.to_owned(),
    }
  }

  fn app(versions: Vec<AppVersion>) -> AppEntry {
    AppEntry {
      id: WEATHER.to_string(),
      name: "weather".to_owned(),
      versions,
    }
  }

  fn catalogs(url: &str, apps: Vec<AppEntry>) -> BTreeMap<String, Catalog> {
    BTreeMap::from([(
      url.to_owned(),
      Catalog {
        schema: CATALOG_SCHEMA.to_owned(),
        apps,
      },
    )])
  }

  fn installed(version: &str, provenance: Option<&str>) -> WebappInfo {
    WebappInfo {
      id: WEATHER,
      name: "weather".to_owned(),
      source: WebappSource::Installed,
      role: WebappRole::Standard,
      version: version.to_owned(),
      description: None,
      icon_hash: None,
      settings_hash: None,
      overlay_hash: None,
      config: Vec::new(),
      permissions: Vec::new(),
      renders_voice_display: false,
      art: None,
      provenance: provenance.map(str::to_owned),
      extension: None,
    }
  }

  #[test]
  fn versions_compare_numerically_and_ignore_a_v_prefix_and_prerelease_tags() {
    assert!(compare_versions("1.10.0", "1.9.0").is_gt());
    assert!(compare_versions("v1.2.0", "1.2").is_eq());
    assert!(compare_versions("1.2.0-beta", "1.2.0").is_eq());
    assert!(satisfies("v0.14.0", "0.14.0"));
    assert!(!satisfies("v0.13.9", "0.14.0"));
  }

  #[test]
  fn the_newest_version_is_picked_by_release_time_and_never_by_list_order() {
    let app = app(vec![
      version("1.0.0", "2026-01-01T00:00:00Z", "0.1.0"),
      version("2.0.0", "2026-03-01T00:00:00+02:00", "0.1.0"),
    ]);

    assert_eq!(
      newest_compatible(&app, "v1.0.0").map(|v| v.version.as_str()),
      Some("2.0.0")
    );
  }

  #[test]
  fn a_version_the_device_cannot_run_falls_back_to_the_newest_it_can() {
    let app = app(vec![
      version("2.0.0", "2026-03-01T00:00:00Z", "0.20.0"),
      version("1.5.0", "2026-02-01T00:00:00Z", "0.10.0"),
    ]);

    assert_eq!(
      newest_compatible(&app, "v0.12.0").map(|v| v.version.as_str()),
      Some("1.5.0")
    );
  }

  #[test]
  fn an_installed_app_behind_its_pinned_source_is_owed_an_update() {
    let held = catalogs(
      SOURCE,
      vec![app(vec![version("1.1.0", "2026-03-01T00:00:00Z", "0.1.0")])],
    );

    let owed = updates(&held, &[installed("1.0.0", Some(SOURCE))], "v1.0.0");

    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].target.version, "1.1.0");
    assert_eq!(owed[0].source_url, SOURCE);
    assert_eq!(owed[0].installed_version, "1.0.0");
  }

  #[test]
  fn an_app_is_only_ever_updated_from_the_source_it_was_installed_from() {
    let held = catalogs(
      OTHER,
      vec![app(vec![version("9.0.0", "2026-03-01T00:00:00Z", "0.1.0")])],
    );

    assert!(
      updates(&held, &[installed("1.0.0", Some(SOURCE))], "v1.0.0").is_empty(),
      "another source listing the same uuid is a cross-grade, never a silent update"
    );
    assert!(updates(&held, &[installed("1.0.0", None)], "v1.0.0").is_empty());
  }

  #[test]
  fn a_current_or_newer_install_is_owed_nothing() {
    let held = catalogs(
      SOURCE,
      vec![app(vec![version("1.0.0", "2026-03-01T00:00:00Z", "0.1.0")])],
    );

    assert!(updates(&held, &[installed("1.0.0", Some(SOURCE))], "v1.0.0").is_empty());
    assert!(updates(&held, &[installed("1.2.0", Some(SOURCE))], "v1.0.0").is_empty());
  }

  #[test]
  fn an_app_that_ships_a_native_extension_is_never_auto_updated() {
    let mut extended = version("2.0.0", "2026-03-01T00:00:00Z", "0.1.0");
    extended.extension = Some(serde_json::json!({ "desktop": true, "permissions": [] }));
    let held = catalogs(SOURCE, vec![app(vec![extended])]);

    assert!(updates(&held, &[installed("1.0.0", Some(SOURCE))], "v1.0.0").is_empty());
  }

  #[test]
  fn an_installed_bundle_carrying_an_extension_is_never_auto_updated() {
    let held = catalogs(
      SOURCE,
      vec![app(vec![version("2.0.0", "2026-03-01T00:00:00Z", "0.1.0")])],
    );
    let mut info = installed("1.0.0", Some(SOURCE));
    info.extension = Some(libbridgething::ExtensionInfo {
      api: 1,
      permissions: Vec::new(),
    });

    assert!(updates(&held, &[info], "v1.0.0").is_empty());
  }

  #[test]
  fn a_launcher_from_a_catalog_is_updated_like_any_other_app() {
    let held = catalogs(
      SOURCE,
      vec![app(vec![version("2.0.0", "2026-03-01T00:00:00Z", "0.1.0")])],
    );
    let mut info = installed("1.0.0", Some(SOURCE));
    info.role = WebappRole::Launcher;

    assert_eq!(updates(&held, &[info], "v1.0.0").len(), 1);
  }

  #[test]
  fn a_builtin_is_never_offered_a_catalog_update() {
    let held = catalogs(
      SOURCE,
      vec![app(vec![version("2.0.0", "2026-03-01T00:00:00Z", "0.1.0")])],
    );
    let mut info = installed("1.0.0", Some(SOURCE));
    info.source = WebappSource::Builtin;

    assert!(updates(&held, &[info], "v1.0.0").is_empty());
  }

  #[test]
  fn subscriptions_keep_their_order_and_drop_repeats_and_blanks() {
    assert_eq!(
      subscriptions([
        OTHER.to_owned(),
        " ".to_owned(),
        SOURCE.to_owned(),
        format!(" {OTHER} ")
      ]),
      vec![OTHER.to_owned(), SOURCE.to_owned()]
    );
  }
}
