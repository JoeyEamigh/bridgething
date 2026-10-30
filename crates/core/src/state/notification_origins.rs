use std::{
  collections::HashMap,
  sync::{Arc, RwLock},
};

use crate::bluetooth::Address;

#[derive(Debug, Clone, Default)]
pub struct NotificationOrigins {
  inner: Arc<RwLock<HashMap<String, Address>>>,
}

impl NotificationOrigins {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn note(&self, id: String, origin: Address) {
    self
      .inner
      .write()
      .expect("notification origins poisoned")
      .insert(id, origin);
  }

  pub fn forget(&self, id: &str) {
    self.inner.write().expect("notification origins poisoned").remove(id);
  }

  pub fn origin_of(&self, id: &str) -> Option<Address> {
    self
      .inner
      .read()
      .expect("notification origins poisoned")
      .get(id)
      .copied()
  }

  pub fn drop_for(&self, origin: Address) {
    self
      .inner
      .write()
      .expect("notification origins poisoned")
      .retain(|_, posted_by| *posted_by != origin);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn gw(last: u8) -> Address {
    Address([0xAA, 0, 0, 0, 0, last])
  }

  #[test]
  fn an_invoke_finds_the_companion_that_posted() {
    let origins = NotificationOrigins::new();
    origins.note("phone-1".into(), gw(1));
    origins.note("desktop-1".into(), gw(2));
    assert_eq!(origins.origin_of("phone-1"), Some(gw(1)));
    assert_eq!(origins.origin_of("desktop-1"), Some(gw(2)));
    assert_eq!(origins.origin_of("unknown"), None);
  }

  #[test]
  fn removal_and_companion_loss_forget_the_origin() {
    let origins = NotificationOrigins::new();
    origins.note("phone-1".into(), gw(1));
    origins.note("phone-2".into(), gw(1));
    origins.note("desktop-1".into(), gw(2));

    origins.forget("phone-1");
    assert_eq!(origins.origin_of("phone-1"), None);

    origins.drop_for(gw(1));
    assert_eq!(origins.origin_of("phone-2"), None);
    assert_eq!(
      origins.origin_of("desktop-1"),
      Some(gw(2)),
      "one companion leaving forgot another's notifications"
    );
  }
}
