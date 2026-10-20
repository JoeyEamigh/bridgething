use std::{
  collections::{BTreeMap, HashMap},
  sync::Mutex,
};

use super::is_digest;
use crate::seam::{BlobStore, CachedResource, SlotIndex};

#[derive(Default)]
pub struct MemoryBlobStore {
  held: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryBlobStore {
  pub fn new() -> Self {
    Self::default()
  }
}

fn checked(digest: &str) -> Result<(), String> {
  if is_digest(digest) {
    Ok(())
  } else {
    Err(format!("{digest} is not a sha256 digest"))
  }
}

impl BlobStore for MemoryBlobStore {
  fn contains(&self, digest: &str) -> bool {
    self.held.lock().unwrap().contains_key(digest)
  }

  fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, String> {
    checked(digest)?;
    Ok(self.held.lock().unwrap().get(digest).cloned())
  }

  fn put(&self, digest: &str, bytes: &[u8]) -> Result<(), String> {
    checked(digest)?;
    self
      .held
      .lock()
      .unwrap()
      .entry(digest.to_owned())
      .or_insert_with(|| bytes.to_vec());
    Ok(())
  }

  fn remove(&self, digest: &str) -> Result<(), String> {
    checked(digest)?;
    self.held.lock().unwrap().remove(digest);
    Ok(())
  }
}

#[derive(Default)]
pub struct MemorySlotIndex {
  held: Mutex<BTreeMap<String, CachedResource>>,
}

impl MemorySlotIndex {
  pub fn new() -> Self {
    Self::default()
  }
}

impl SlotIndex for MemorySlotIndex {
  fn get(&self, slot: &str) -> Option<CachedResource> {
    self.held.lock().unwrap().get(slot).cloned()
  }

  fn set(&self, slot: &str, resource: &CachedResource) -> Result<(), String> {
    self.held.lock().unwrap().insert(slot.to_owned(), resource.clone());
    Ok(())
  }

  fn remove(&self, slot: &str) -> Result<(), String> {
    self.held.lock().unwrap().remove(slot);
    Ok(())
  }

  fn entries(&self) -> Vec<(String, CachedResource)> {
    self
      .held
      .lock()
      .unwrap()
      .iter()
      .map(|(slot, resource)| (slot.clone(), resource.clone()))
      .collect()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::bundle::fetch::sha256_hex;

  #[test]
  fn a_put_round_trips_and_a_remove_forgets_it() {
    let store = MemoryBlobStore::new();
    let body = b"held in memory".to_vec();
    let digest = sha256_hex(&body);

    store.put(&digest, &body).expect("the blob stores");
    assert!(store.contains(&digest));
    assert_eq!(store.get(&digest).expect("readable"), Some(body));

    store.remove(&digest).expect("removable");
    assert!(!store.contains(&digest));
    assert_eq!(store.get(&digest).expect("absent is not an error"), None);
  }

  #[test]
  fn a_key_that_is_not_a_digest_is_refused() {
    let store = MemoryBlobStore::new();

    assert!(store.put("not-a-digest", b"no").is_err());
    assert!(store.get("not-a-digest").is_err());
  }

  #[test]
  fn a_slot_points_at_the_last_resource_set_for_it() {
    let index = MemorySlotIndex::new();
    let first = CachedResource {
      digest: sha256_hex(b"one"),
      mime: None,
    };
    let second = CachedResource {
      digest: sha256_hex(b"two"),
      mime: Some("text/html".into()),
    };

    index.set("app__settings", &first).expect("set");
    index.set("app__settings", &second).expect("reset");

    assert_eq!(index.get("app__settings"), Some(second.clone()));
    assert_eq!(index.entries(), vec![("app__settings".to_owned(), second)]);
    index.remove("app__settings").expect("remove");
    assert_eq!(index.get("app__settings"), None);
  }
}
