use super::CompanionSession;

#[uniffi::export]
impl CompanionSession {
  pub fn catalog_sources(&self) -> Vec<String> {
    self.session.catalog_sources()
  }

  pub fn set_catalog_sources(&self, urls: Vec<String>) -> Vec<String> {
    self.session.set_catalog_sources(urls)
  }
}
