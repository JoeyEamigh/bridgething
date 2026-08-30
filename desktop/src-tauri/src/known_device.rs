use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::store::{JsonFile, stored};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownDevice {
  #[serde(default)]
  pub id: String,
  pub url: String,
  pub name: String,
  pub last_connected_at: Option<String>,
}

pub struct KnownDevices(JsonFile<Vec<KnownDevice>>);

impl KnownDevices {
  pub fn open(config_dir: &Path) -> Self {
    let path = config_dir.join("known-devices.json");
    let mut held: Vec<KnownDevice> = stored(&path).unwrap_or_default();
    for known in &mut held {
      if known.id.is_empty() {
        known.id.clone_from(&known.url);
      }
    }
    Self(JsonFile::new(path, "known device list", held))
  }

  pub fn list(&self) -> Vec<KnownDevice> {
    self.0.read(|held| held.clone())
  }

  pub fn seen(&self, id: &str, url: &str, label: Option<&str>) {
    let at = Some(chrono::Utc::now().to_rfc3339());
    self.0.write(|held| {
      let claims = |known: &KnownDevice| known.id == id || known.id == url;
      let seat = held
        .iter()
        .position(|known| known.id == id)
        .or_else(|| held.iter().position(claims));
      if let Some(seat) = seat {
        let folded: Vec<usize> = held
          .iter()
          .enumerate()
          .filter(|(other, known)| *other != seat && claims(known))
          .map(|(other, _)| other)
          .collect();
        for gone in folded.into_iter().rev() {
          held.remove(gone);
        }
      }

      match seat {
        Some(seat) => {
          let known = &mut held[seat];
          known.id = id.to_owned();
          known.url = url.to_owned();
          if let Some(label) = label {
            known.name = label.to_owned();
          }
          known.last_connected_at = at;
        }
        None => held.push(KnownDevice {
          id: id.to_owned(),
          url: url.to_owned(),
          name: label.unwrap_or("bridgething daemon").to_owned(),
          last_connected_at: at,
        }),
      }
    });
  }

  pub fn forget(&self, id: &str) -> Option<KnownDevice> {
    self.0.write(|held| {
      let seat = held.iter().position(|known| known.id == id)?;
      Some(held.remove(seat))
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const URL: &str = "ws://bridgething.local:8892/";
  const OTHER_URL: &str = "ws://bridgething-q61r.local:8892/";
  const SERIAL: &str = "8558R481Q61R";

  #[test]
  fn a_device_that_came_up_once_is_remembered_across_the_process_that_saw_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");

    let first = KnownDevices::open(dir.path());
    assert!(first.list().is_empty(), "a fresh host remembers nothing");
    first.seen(URL, URL, None);

    let held = KnownDevices::open(dir.path()).list();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].name, "bridgething daemon");
    assert!(held[0].last_connected_at.is_some(), "and it says when it last answered");
  }

  #[test]
  fn the_serial_adopts_the_row_the_dial_opened() {
    let dir = tempfile::tempdir().expect("a scratch directory");

    let devices = KnownDevices::open(dir.path());
    devices.seen(URL, URL, Some("Kitchen Thing"));
    devices.seen(SERIAL, URL, None);

    let held = devices.list();
    assert_eq!(held.len(), 1, "adoption re-keys the row rather than adding one");
    assert_eq!(held[0].id, SERIAL);
    assert_eq!(held[0].url, URL);
    assert_eq!(
      held[0].name, "Kitchen Thing",
      "and it keeps everything the url-keyed row had learned"
    );
  }

  #[test]
  fn a_device_that_moved_address_keeps_its_history() {
    let dir = tempfile::tempdir().expect("a scratch directory");

    let devices = KnownDevices::open(dir.path());
    devices.seen(SERIAL, URL, Some("Kitchen Thing"));
    devices.seen(SERIAL, OTHER_URL, None);

    let held = devices.list();
    assert_eq!(held.len(), 1, "the same device at a new address is the same device");
    assert_eq!(held[0].url, OTHER_URL, "and the row follows it to the new address");
    assert_eq!(held[0].name, "Kitchen Thing");
  }

  #[test]
  fn a_different_device_at_a_remembered_address_does_not_inherit_the_name() {
    let dir = tempfile::tempdir().expect("a scratch directory");

    let devices = KnownDevices::open(dir.path());
    devices.seen(SERIAL, URL, Some("Kitchen Thing"));
    devices.seen("OTHERSERIAL01", URL, Some("Garage Thing"));

    let mut held = devices.list();
    held.sort_by(|left, right| left.id.cmp(&right.id));
    assert_eq!(held.len(), 2, "two serials are two devices however they were reached");
    assert_eq!(held[0].id, "8558R481Q61R");
    assert_eq!(held[1].name, "Garage Thing");
  }

  #[test]
  fn adoption_folds_a_row_the_url_had_already_opened_into_the_one_the_serial_holds() {
    let dir = tempfile::tempdir().expect("a scratch directory");

    let devices = KnownDevices::open(dir.path());
    devices.seen(SERIAL, OTHER_URL, Some("Kitchen Thing"));
    devices.seen(URL, URL, None);
    assert_eq!(
      devices.list().len(),
      2,
      "an unadopted dial is its own row until it speaks"
    );

    devices.seen(SERIAL, URL, None);
    let held = devices.list();
    assert_eq!(held.len(), 1, "and the serial claims it once it does");
    assert_eq!(held[0].name, "Kitchen Thing");
    assert_eq!(held[0].url, URL);
  }

  #[test]
  fn a_row_written_before_identities_existed_is_adopted_rather_than_orphaned() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    std::fs::write(
      dir.path().join("known-devices.json"),
      format!(r#"[{{"url":"{URL}","name":"Kitchen Thing","lastConnectedAt":null}}]"#),
    )
    .expect("the legacy file writes");

    let devices = KnownDevices::open(dir.path());
    assert_eq!(
      devices.list()[0].id,
      URL,
      "a row with no identity is keyed by its address"
    );

    devices.seen(SERIAL, URL, None);
    let held = devices.list();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, SERIAL);
    assert_eq!(held[0].name, "Kitchen Thing");
  }

  #[test]
  fn a_nameless_reconnect_does_not_erase_what_discovery_learned() {
    let dir = tempfile::tempdir().expect("a scratch directory");

    let devices = KnownDevices::open(dir.path());
    devices.seen(SERIAL, URL, Some("Kitchen Thing"));
    devices.seen(SERIAL, URL, None);
    assert_eq!(devices.list()[0].name, "Kitchen Thing");
  }

  #[test]
  fn a_forgotten_device_is_gone_from_disk() {
    let dir = tempfile::tempdir().expect("a scratch directory");

    let devices = KnownDevices::open(dir.path());
    assert!(devices.forget(SERIAL).is_none(), "there is nothing to forget yet");
    devices.seen(SERIAL, URL, None);
    assert_eq!(devices.forget(SERIAL).map(|gone| gone.url), Some(URL.to_owned()));

    assert!(
      KnownDevices::open(dir.path()).list().is_empty(),
      "forgetting is flushed as eagerly as recording"
    );
  }
}
