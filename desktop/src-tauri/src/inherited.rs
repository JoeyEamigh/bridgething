use std::path::{Path, PathBuf};

use bridgething_companion::{
  api::{CapabilityFlags, CompanionSession},
  store::stored,
};
use serde_json::Value;

const CAPABILITIES_FILE: &str = "capabilities.json";
const SOURCES_FILE: &str = "catalog-sources.json";

pub struct Inherited {
  capabilities: Option<(PathBuf, CapabilityFlags)>,
  sources: Option<(PathBuf, Vec<String>)>,
}

impl Inherited {
  pub fn find(config_dir: &Path, fresh: CapabilityFlags) -> Self {
    let capabilities = config_dir.join(CAPABILITIES_FILE);
    let sources = config_dir.join(SOURCES_FILE);
    Self {
      capabilities: stored::<Value>(&capabilities).map(|body| (capabilities, fresh.overlaid(body))),
      sources: stored::<Vec<String>>(&sources).map(|urls| (sources, urls)),
    }
  }

  pub fn capabilities(&self) -> Option<CapabilityFlags> {
    self.capabilities.as_ref().map(|(_, flags)| *flags)
  }

  pub async fn hand_to(self, session: &CompanionSession) {
    if let Some((path, flags)) = self.capabilities {
      session.set_capability_flags(flags).await;
      retire(&path);
    }
    if let Some((path, urls)) = self.sources {
      let mut held = session.catalog_sources();
      held.extend(urls.into_iter().filter(|url| !held.contains(url)).collect::<Vec<_>>());
      session.set_catalog_sources(held);
      retire(&path);
    }
  }
}

fn retire(path: &Path) {
  if let Err(error) = std::fs::remove_file(path) {
    tracing::warn!(%error, path = %path.display(), "an inherited settings file could not be removed");
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::*;

  const FRESH: CapabilityFlags = CapabilityFlags {
    geo: true,
    notifications: false,
    net_fetch: true,
    net_ws: true,
    audio_tts: true,
    voice_model: false,
  };

  #[test]
  fn a_host_with_no_earlier_choices_inherits_nothing() {
    let dir = tempfile::tempdir().expect("a scratch directory");

    let inherited = Inherited::find(dir.path(), FRESH);

    assert!(inherited.capabilities().is_none());
    assert!(inherited.sources.is_none());
  }

  #[test]
  fn an_earlier_refusal_is_inherited_and_a_capability_it_predates_takes_the_fresh_default() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    fs::write(dir.path().join(CAPABILITIES_FILE), br#"{"geo":false,"netFetch":true}"#)
      .expect("a stored file from an older build");

    let flags = Inherited::find(dir.path(), FRESH)
      .capabilities()
      .expect("the earlier choices");

    assert!(!flags.geo, "what the file does say is honored");
    assert!(flags.audio_tts, "what it does not say falls back to the fresh default");
  }

  #[test]
  fn earlier_catalog_subscriptions_are_found() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    fs::write(
      dir.path().join(SOURCES_FILE),
      br#"["https://example.invalid/catalog.json"]"#,
    )
    .expect("a stored file from an older build");

    let inherited = Inherited::find(dir.path(), FRESH);

    assert_eq!(
      inherited.sources.map(|(_, urls)| urls),
      Some(vec!["https://example.invalid/catalog.json".to_owned()])
    );
  }
}
