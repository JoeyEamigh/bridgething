use bridgething_delivery::catalog::subscriptions;

use super::Session;

impl Session {
  pub fn catalog_sources(&self) -> Vec<String> {
    self.settings.read(|held| held.catalog_sources.clone())
  }

  pub fn set_catalog_sources(&self, urls: Vec<String>) -> Vec<String> {
    let held = subscriptions(urls);
    self.settings.write(|settings| settings.catalog_sources = held.clone());
    self.ota.set_catalog_sources(held.clone());
    held
  }
}
