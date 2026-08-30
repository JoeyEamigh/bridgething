use std::{
  collections::{BTreeMap, BTreeSet},
  fs,
  io::Read,
  path::{Component, Path, PathBuf},
  sync::{Arc, Mutex},
};

use libbridgething::{ExtensionManifest, ExtensionPermission, WebappManifest};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const RECORD: &str = "extension.json";
const DATA: &str = "data";
const KV: &str = "kv.json";
const MANIFEST: &str = "manifest.json";
const BUNDLE_PREFIX: &str = "extension/";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionRecord {
  #[serde(rename = "id")]
  pub webapp: Uuid,
  pub name: String,
  pub version: String,
  pub entry: String,
  pub permissions: Vec<ExtensionPermission>,
  pub api: u32,
  pub enabled: bool,
  #[serde(default)]
  pub devices: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
  pub webapp: Uuid,
  pub name: String,
  pub version: String,
  pub permissions: Vec<ExtensionPermission>,
  pub api: u32,
  pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Adopted {
  None,
  Installed(ExtensionRecord),
  Refused(Refusal),
}

#[derive(Clone)]
pub struct ExtensionStore {
  root: PathBuf,
  writes: Arc<Mutex<()>>,
}

impl ExtensionStore {
  pub fn open(state_dir: &Path) -> Self {
    Self {
      root: state_dir.join("extensions"),
      writes: Arc::new(Mutex::new(())),
    }
  }

  pub fn list(&self) -> Vec<ExtensionRecord> {
    let Ok(entries) = fs::read_dir(&self.root) else {
      return Vec::new();
    };
    let mut held: Vec<ExtensionRecord> = entries
      .flatten()
      .filter_map(|entry| Uuid::parse_str(entry.file_name().to_str()?).ok())
      .filter_map(|webapp| self.record(webapp))
      .collect();
    held.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.webapp.cmp(&b.webapp)));
    held
  }

  pub fn record(&self, webapp: Uuid) -> Option<ExtensionRecord> {
    let raw = fs::read_to_string(self.home(webapp).join(RECORD)).ok()?;
    let record: ExtensionRecord = serde_json::from_str(&raw).ok()?;
    (record.webapp == webapp).then_some(record)
  }

  pub fn set_enabled(&self, webapp: Uuid, enabled: bool) -> Option<ExtensionRecord> {
    let _writing = self.writes.lock().unwrap();
    let mut record = self.record(webapp)?;
    if record.enabled == enabled {
      return Some(record);
    }
    record.enabled = enabled;
    self.keep(&record).ok()?;
    Some(record)
  }

  pub fn entry(&self, record: &ExtensionRecord) -> Option<PathBuf> {
    bundled(&self.home(record.webapp).join(&record.version), &record.entry)
  }

  pub fn data_dir(&self, webapp: Uuid) -> PathBuf {
    self.home(webapp).join(DATA)
  }

  pub fn kv_path(&self, webapp: Uuid) -> PathBuf {
    self.data_dir(webapp).join(KV)
  }

  pub fn claim(&self, webapp: Uuid, device: &str) -> Option<BTreeSet<String>> {
    self.amend(webapp, |record| record.devices.insert(device.to_owned()))
  }

  pub fn disown(&self, webapp: Uuid, device: &str) -> Option<BTreeSet<String>> {
    self.amend(webapp, |record| record.devices.remove(device))
  }

  pub fn rekey(&self, webapp: Uuid, from: &str, to: &str) -> Option<BTreeSet<String>> {
    self.amend(webapp, |record| {
      if !record.devices.remove(from) {
        return false;
      }
      record.devices.insert(to.to_owned());
      true
    })
  }

  fn amend(&self, webapp: Uuid, change: impl FnOnce(&mut ExtensionRecord) -> bool) -> Option<BTreeSet<String>> {
    let _writing = self.writes.lock().unwrap();
    let mut record = self.record(webapp)?;
    if !change(&mut record) {
      return None;
    }
    self.write(&record);
    Some(record.devices)
  }

  pub fn remove(&self, webapp: Uuid) {
    let _writing = self.writes.lock().unwrap();
    let home = self.home(webapp);
    if let Err(error) = fs::remove_dir_all(&home)
      && error.kind() != std::io::ErrorKind::NotFound
    {
      tracing::warn!(%error, path = %home.display(), "an uninstalled extension could not be cleared");
    }
  }

  pub fn adopt(
    &self,
    device: Option<&str>,
    archive: &Path,
    confirmed: Option<&[ExtensionPermission]>,
  ) -> Result<Adopted, String> {
    let file = fs::File::open(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|error| error.to_string())?;

    let manifest = manifest(&mut zip)?;
    let Some(declared) = manifest.extension.clone() else {
      return Ok(Adopted::None);
    };
    let refuse = |reason: String| {
      Ok(Adopted::Refused(Refusal {
        webapp: manifest.id,
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        permissions: declared.permissions.clone(),
        api: declared.api,
        reason,
      }))
    };

    match unconfirmed(confirmed, &declared.permissions) {
      Unconfirmed::Nothing => {}
      Unconfirmed::NeverOffered => {
        return refuse("this install never asked whether the extension could run here".to_owned());
      }
      Unconfirmed::Beyond(beyond) => {
        return refuse(format!(
          "the bundle asks for {}, which the install did not offer",
          beyond.join(", ")
        ));
      }
    }

    if declared.entry.trim().is_empty() {
      return refuse("the extension block names no entry".to_owned());
    }

    let home = self.home(manifest.id);
    let staging = home.join(format!("{}.staging", manifest.version));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|error| format!("{}: {error}", staging.display()))?;

    let taken = extract(&mut zip, &staging)?;
    if taken == 0 {
      let _ = fs::remove_dir_all(&staging);
      return refuse(format!("the bundle carries no {BUNDLE_PREFIX} directory"));
    }
    let Some(entry) = bundled(&staging, &declared.entry) else {
      let _ = fs::remove_dir_all(&staging);
      return refuse(format!(
        "the declared entry {} is not a path inside the bundle's {BUNDLE_PREFIX} directory",
        declared.entry
      ));
    };
    if !entry.is_file() {
      let _ = fs::remove_dir_all(&staging);
      return refuse(format!("the bundle is missing its declared entry {}", declared.entry));
    }

    let installed = home.join(&manifest.version);
    let _ = fs::remove_dir_all(&installed);
    fs::rename(&staging, &installed).map_err(|error| format!("{}: {error}", installed.display()))?;
    self.prune(manifest.id, &manifest.version);
    fs::create_dir_all(self.data_dir(manifest.id)).map_err(|error| error.to_string())?;

    let record = {
      let _writing = self.writes.lock().unwrap();
      let held = self.record(manifest.id);
      let mut devices = held.as_ref().map(|held| held.devices.clone()).unwrap_or_default();
      devices.extend(device.map(ToOwned::to_owned));
      let record = ExtensionRecord {
        webapp: manifest.id,
        name: manifest.name,
        version: manifest.version,
        entry: declared.entry,
        permissions: declared.permissions,
        api: declared.api,
        enabled: held.map(|held| held.enabled).unwrap_or(true),
        devices,
      };
      self.keep(&record)?;
      record
    };
    Ok(Adopted::Installed(record))
  }

  fn home(&self, webapp: Uuid) -> PathBuf {
    self.root.join(webapp.to_string())
  }

  fn write(&self, record: &ExtensionRecord) {
    if let Err(error) = self.keep(record) {
      tracing::warn!(%error, webapp = %record.webapp, "an extension record could not be written");
    }
  }

  fn keep(&self, record: &ExtensionRecord) -> Result<(), String> {
    let home = self.home(record.webapp);
    fs::create_dir_all(&home).map_err(|error| format!("{}: {error}", home.display()))?;
    let body = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    fs::write(home.join(RECORD), body).map_err(|error| error.to_string())
  }

  fn prune(&self, webapp: Uuid, keep: &str) {
    let home = self.home(webapp);
    let Ok(entries) = fs::read_dir(&home) else { return };
    for entry in entries.flatten() {
      let name = entry.file_name();
      let name = name.to_string_lossy();
      if name == keep || name == RECORD || name == DATA {
        continue;
      }
      let _ = fs::remove_dir_all(entry.path());
    }
  }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Unconfirmed {
  Nothing,
  NeverOffered,
  Beyond(Vec<String>),
}

pub fn unconfirmed(confirmed: Option<&[ExtensionPermission]>, declared: &[ExtensionPermission]) -> Unconfirmed {
  let Some(confirmed) = confirmed else {
    return Unconfirmed::NeverOffered;
  };
  let confirmed: Vec<String> = confirmed.iter().map(ExtensionPermission::to_string).collect();
  let beyond: Vec<String> = declared
    .iter()
    .map(ExtensionPermission::to_string)
    .filter(|wanted| {
      let kind = wanted.split_once(':').map_or(wanted.as_str(), |(kind, _)| kind);
      !confirmed
        .iter()
        .any(|held| held == "all" || held == wanted || held == kind)
    })
    .collect();
  if beyond.is_empty() {
    Unconfirmed::Nothing
  } else {
    Unconfirmed::Beyond(beyond)
  }
}

pub fn read_kv(path: &Path) -> BTreeMap<String, serde_json::Value> {
  let raw = match fs::read_to_string(path) {
    Ok(raw) => raw,
    Err(error) => {
      if error.kind() != std::io::ErrorKind::NotFound {
        tracing::warn!(%error, path = %path.display(), "an extension's store could not be read; it starts empty");
      }
      return BTreeMap::new();
    }
  };
  match serde_json::from_str(&raw) {
    Ok(held) => held,
    Err(error) => {
      tracing::warn!(
        %error,
        path = %path.display(),
        bytes = raw.len(),
        "an extension's store did not parse; it starts empty and the next write replaces it"
      );
      BTreeMap::new()
    }
  }
}

pub fn write_kv(path: &Path, held: &BTreeMap<String, serde_json::Value>) -> Result<(), String> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
  }
  let body = serde_json::to_vec(held).map_err(|error| error.to_string())?;
  let mut staging = path.as_os_str().to_owned();
  staging.push(".part");
  let staging = PathBuf::from(staging);
  fs::write(&staging, body).map_err(|error| format!("{}: {error}", staging.display()))?;
  fs::rename(&staging, path).map_err(|error| format!("{}: {error}", path.display()))
}

fn bundled(root: &Path, entry: &str) -> Option<PathBuf> {
  entry
    .starts_with(BUNDLE_PREFIX)
    .then(|| contained(root, entry))
    .flatten()
}

pub fn contained(root: &Path, name: &str) -> Option<PathBuf> {
  let mut out = root.to_path_buf();
  for part in Path::new(name).components() {
    match part {
      Component::Normal(part) => out.push(part),
      Component::CurDir => {}
      _ => return None,
    }
  }
  out.starts_with(root).then_some(out)
}

pub fn declared_extension(archive: &Path) -> Result<Option<ExtensionManifest>, String> {
  let file = fs::File::open(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
  let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|error| error.to_string())?;
  Ok(manifest(&mut zip)?.extension)
}

fn manifest<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Result<WebappManifest, String> {
  let mut entry = zip
    .by_name(MANIFEST)
    .map_err(|_| format!("the bundle carries no {MANIFEST}"))?;
  let mut raw = String::new();
  entry.read_to_string(&mut raw).map_err(|error| error.to_string())?;
  serde_json::from_str(&raw).map_err(|error| format!("{MANIFEST}: {error}"))
}

fn extract<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, into: &Path) -> Result<usize, String> {
  let mut taken = 0;
  for index in 0..zip.len() {
    let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
    let name = entry.name().to_owned();
    if !name.starts_with(BUNDLE_PREFIX) {
      continue;
    }
    let out = contained(into, &name).ok_or_else(|| format!("{name} escapes the extension directory"))?;
    if entry.is_dir() {
      fs::create_dir_all(&out).map_err(|error| error.to_string())?;
      continue;
    }
    if let Some(parent) = out.parent() {
      fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut sink = fs::File::create(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    std::io::copy(&mut entry, &mut sink).map_err(|error| error.to_string())?;
    taken += 1;
  }
  Ok(taken)
}

#[cfg(test)]
mod tests {
  use std::io::Write as _;

  use super::*;

  const WEBAPP: Uuid = Uuid::from_u128(0x2f0c_1a4b_0000_4000_8000_0000_0000_0001);
  const DEVICE: &str = "sn-1";

  fn bundle(dir: &Path, name: &str, manifest: serde_json::Value, files: &[(&str, &str)]) -> PathBuf {
    let path = dir.join(name);
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).expect("an archive"));
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
    zip.start_file(MANIFEST, options).expect("a manifest entry");
    zip
      .write_all(manifest.to_string().as_bytes())
      .expect("the manifest body");
    for (entry, body) in files {
      zip.start_file(*entry, options).expect("an entry");
      zip.write_all(body.as_bytes()).expect("the entry body");
    }
    zip.finish().expect("a finished archive");
    path
  }

  fn manifest_json(version: &str, extension: Option<serde_json::Value>) -> serde_json::Value {
    let mut held = serde_json::json!({
      "id": WEBAPP.to_string(),
      "name": "weather",
      "version": version,
    });
    if let Some(extension) = extension {
      held["extension"] = extension;
    }
    held
  }

  fn permissions(descriptors: &[&str]) -> Vec<ExtensionPermission> {
    descriptors
      .iter()
      .map(|raw| raw.parse().unwrap_or_else(|_| panic!("{raw} parses")))
      .collect()
  }

  fn adopt(store: &ExtensionStore, archive: &Path, confirmed: &[&str]) -> Result<Adopted, String> {
    store.adopt(Some(DEVICE), archive, Some(&permissions(confirmed)))
  }

  fn refusal(adopted: Result<Adopted, String>) -> Refusal {
    match adopted.expect("the bundle was read") {
      Adopted::Refused(refusal) => refusal,
      other => panic!("the bundle was not refused: {other:?}"),
    }
  }

  const ADDRESS: &str = "ws://bridgething.local:8892/";
  const SERIAL: &str = "8558R481Q61R";
  const ROUNDS: u32 = 240;

  fn spin(wait: std::time::Duration) {
    let until = std::time::Instant::now() + wait;
    while std::time::Instant::now() < until {
      std::hint::spin_loop();
    }
  }

  fn extended(dir: &Path) -> PathBuf {
    bundle(
      dir,
      "extended.zip",
      manifest_json(
        "1.0.0",
        Some(serde_json::json!({ "entry": "extension/desktop.mjs", "permissions": [], "api": 1 })),
      ),
      &[("extension/desktop.mjs", "export {}")],
    )
  }

  #[test]
  fn two_writers_do_not_hand_each_other_back_the_claim_the_other_just_dropped() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = Arc::new(ExtensionStore::open(dir.path()));
    let archive = extended(dir.path());

    for round in 0..ROUNDS {
      store.remove(WEBAPP);
      store
        .adopt(Some(ADDRESS), &archive, Some(&[]))
        .expect("the bundle is read");

      let start = Arc::new(std::sync::Barrier::new(2));
      let claiming = {
        let store = Arc::clone(&store);
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
          start.wait();
          store.claim(WEBAPP, DEVICE);
        })
      };
      let rekeying = {
        let store = Arc::clone(&store);
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
          start.wait();
          store.rekey(WEBAPP, ADDRESS, SERIAL);
        })
      };
      claiming.join().expect("the claim finished");
      rekeying.join().expect("the rekey finished");

      assert_eq!(
        store.record(WEBAPP).expect("the record survives").devices,
        BTreeSet::from([DEVICE.to_owned(), SERIAL.to_owned()]),
        "round {round}: whichever writer lands second has to read what the first wrote, or one \
         daemon's claim is dropped and the address the rekey folded comes back"
      );
    }
  }

  #[test]
  fn an_adopt_that_lands_beside_a_rekey_does_not_write_the_folded_claim_back() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = Arc::new(ExtensionStore::open(dir.path()));
    let archive = extended(dir.path());

    let measured = std::time::Instant::now();
    store
      .adopt(Some(ADDRESS), &archive, Some(&[]))
      .expect("the bundle is read");
    let unpacking = measured.elapsed();

    for round in 0..ROUNDS {
      store.remove(WEBAPP);
      store
        .adopt(Some(ADDRESS), &archive, Some(&[]))
        .expect("the bundle is read");

      let start = Arc::new(std::sync::Barrier::new(2));
      let adopting = {
        let store = Arc::clone(&store);
        let archive = archive.clone();
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
          start.wait();
          store.adopt(None, &archive, Some(&[])).expect("the bundle is read");
        })
      };
      let rekeying = {
        let store = Arc::clone(&store);
        let start = Arc::clone(&start);
        let offset = unpacking.mul_f64(f64::from(round) / f64::from(ROUNDS));
        std::thread::spawn(move || {
          start.wait();
          spin(offset);
          store.rekey(WEBAPP, ADDRESS, SERIAL);
        })
      };
      adopting.join().expect("the adopt finished");
      rekeying.join().expect("the rekey finished");

      assert_eq!(
        store.record(WEBAPP).expect("the record survives").devices,
        BTreeSet::from([SERIAL.to_owned()]),
        "round {round}: an adopt that read the holders before the rekey and wrote them after puts the \
         address claim back, and forgetting the device never reaches it again"
      );
    }
  }

  #[test]
  fn a_bundle_without_an_extension_block_installs_nothing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let archive = bundle(
      dir.path(),
      "plain.zip",
      manifest_json("1.0.0", None),
      &[("index.html", "hi")],
    );

    assert_eq!(store.adopt(Some(DEVICE), &archive, None), Ok(Adopted::None));
    assert!(store.list().is_empty());
  }

  #[test]
  fn adopting_keeps_the_entry_permissions_and_name_the_manifest_declared() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let archive = bundle(
      dir.path(),
      "ext.zip",
      manifest_json(
        "1.2.0",
        Some(serde_json::json!({
          "entry": "extension/desktop.mjs",
          "permissions": ["net:example.com", "read"],
          "api": 1,
        })),
      ),
      &[("index.html", "hi"), ("extension/desktop.mjs", "export {}")],
    );

    let Adopted::Installed(record) = adopt(&store, &archive, &["net:example.com", "read"]).expect("the bundle adopts")
    else {
      panic!("the confirmed permissions match the manifest, so it installs")
    };
    assert_eq!(record.webapp, WEBAPP);
    assert_eq!(record.name, "weather");
    assert_eq!(record.version, "1.2.0");
    assert_eq!(record.entry, "extension/desktop.mjs");
    assert_eq!(record.api, 1);
    assert!(record.enabled, "a freshly installed extension runs without being asked");
    assert_eq!(
      ExtensionPermission::deno_flags(&record.permissions),
      vec!["--allow-net=example.com", "--allow-read"]
    );
    assert!(
      store.entry(&record).expect("a contained entry").is_file(),
      "the entry lands where the spawn looks"
    );
    assert_eq!(store.list(), vec![record]);
  }

  #[test]
  fn only_the_extension_subtree_is_taken_out_of_the_bundle() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let archive = bundle(
      dir.path(),
      "ext.zip",
      manifest_json(
        "1.0.0",
        Some(serde_json::json!({ "entry": "extension/desktop.mjs", "permissions": [], "api": 1 })),
      ),
      &[
        ("index.html", "page"),
        ("assets/app.js", "page code"),
        ("extension/desktop.mjs", "export {}"),
        ("extension/lib/helper.mjs", "export {}"),
      ],
    );

    let Adopted::Installed(record) = adopt(&store, &archive, &[]).expect("adopts") else {
      panic!("an extension that asks for nothing needs nothing confirmed")
    };
    let installed = dir.path().join("extensions").join(WEBAPP.to_string()).join("1.0.0");
    assert!(installed.join("extension/lib/helper.mjs").is_file());
    assert!(!installed.join("index.html").exists(), "the page never lands on disk");
    assert!(!installed.join("assets").exists());
    assert!(record.permissions.is_empty());
  }

  #[test]
  fn only_the_installed_version_survives_an_upgrade() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let block = serde_json::json!({ "entry": "extension/desktop.mjs", "permissions": ["all"], "api": 1 });

    let first = bundle(
      dir.path(),
      "v1.zip",
      manifest_json("1.0.0", Some(block.clone())),
      &[("extension/desktop.mjs", "one")],
    );
    adopt(&store, &first, &["all"]).expect("adopts");
    let second = bundle(
      dir.path(),
      "v2.zip",
      manifest_json("2.0.0", Some(block)),
      &[("extension/desktop.mjs", "two")],
    );
    let Adopted::Installed(record) = adopt(&store, &second, &["all"]).expect("adopts") else {
      panic!("the upgrade installs")
    };

    let home = dir.path().join("extensions").join(WEBAPP.to_string());
    assert!(!home.join("1.0.0").exists(), "the superseded version is gone");
    assert_eq!(
      fs::read_to_string(store.entry(&record).expect("a contained entry")).expect("the entry"),
      "two"
    );
    assert!(home.join(DATA).is_dir(), "the data directory outlives the upgrade");
  }

  #[test]
  fn a_disabled_extension_stays_disabled_across_an_upgrade() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let block = serde_json::json!({ "entry": "extension/desktop.mjs", "permissions": [], "api": 1 });
    let first = bundle(
      dir.path(),
      "v1.zip",
      manifest_json("1.0.0", Some(block.clone())),
      &[("extension/desktop.mjs", "one")],
    );

    adopt(&store, &first, &[]).expect("adopts");
    assert_eq!(
      store.set_enabled(WEBAPP, false).map(|record| record.enabled),
      Some(false)
    );

    let second = bundle(
      dir.path(),
      "v2.zip",
      manifest_json("2.0.0", Some(block)),
      &[("extension/desktop.mjs", "two")],
    );
    let Adopted::Installed(record) = adopt(&store, &second, &[]).expect("adopts") else {
      panic!("the upgrade installs")
    };

    assert!(
      !record.enabled,
      "an upgrade must not silently restart something the user turned off"
    );
  }

  #[test]
  fn a_bundle_whose_entry_is_not_in_it_is_refused_and_leaves_nothing_behind() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let archive = bundle(
      dir.path(),
      "ext.zip",
      manifest_json(
        "1.0.0",
        Some(serde_json::json!({ "entry": "extension/missing.mjs", "permissions": [], "api": 1 })),
      ),
      &[("extension/desktop.mjs", "export {}")],
    );

    assert!(
      refusal(adopt(&store, &archive, &[]))
        .reason
        .contains("missing its declared entry")
    );
    assert!(store.list().is_empty(), "a refused install leaves no runnable record");
  }

  #[test]
  fn uninstalling_takes_the_code_the_record_and_the_data_with_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let archive = bundle(
      dir.path(),
      "ext.zip",
      manifest_json(
        "1.0.0",
        Some(serde_json::json!({ "entry": "extension/desktop.mjs", "permissions": [], "api": 1 })),
      ),
      &[("extension/desktop.mjs", "export {}")],
    );
    adopt(&store, &archive, &[]).expect("adopts");
    write_kv(
      &store.kv_path(WEBAPP),
      &BTreeMap::from([("a".to_owned(), serde_json::json!(1))]),
    )
    .expect("a kv file");

    store.remove(WEBAPP);

    assert_eq!(store.record(WEBAPP), None);
    assert!(!dir.path().join("extensions").join(WEBAPP.to_string()).exists());
  }

  #[test]
  fn an_archive_entry_that_climbs_out_of_the_root_is_refused() {
    let root = Path::new("/tmp/extensions/app");
    assert_eq!(contained(root, "extension/a.mjs"), Some(root.join("extension/a.mjs")));
    assert_eq!(contained(root, "extension/./a.mjs"), Some(root.join("extension/a.mjs")));
    for hostile in ["../escape", "extension/../../escape", "/etc/passwd"] {
      assert_eq!(contained(root, hostile), None, "{hostile} must not resolve");
    }
  }

  #[test]
  fn what_a_picker_can_read_off_a_bundle_is_what_the_install_is_held_to() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let archive = bundle(
      dir.path(),
      "ext.zip",
      manifest_json(
        "1.0.0",
        Some(serde_json::json!({ "entry": "extension/desktop.mjs", "permissions": ["all"], "api": 1 })),
      ),
      &[("extension/desktop.mjs", "export {}")],
    );

    let declared = declared_extension(&archive)
      .expect("the bundle reads")
      .expect("the bundle declares an extension");
    assert_eq!(declared.permissions, permissions(&["all"]));
    assert_eq!(declared.api, 1);

    assert!(
      matches!(
        store.adopt(Some(DEVICE), &archive, Some(&declared.permissions)),
        Ok(Adopted::Installed(_))
      ),
      "confirming exactly what the picker showed installs, or the dialog is unusable"
    );
    assert!(
      matches!(store.adopt(Some(DEVICE), &archive, None), Ok(Adopted::Refused(_))),
      "picking a file off your own disk is not a confirmation of what it runs"
    );
  }

  #[test]
  fn a_bundle_that_declares_no_extension_offers_the_picker_nothing_to_confirm() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let archive = bundle(
      dir.path(),
      "plain.zip",
      manifest_json("1.0.0", None),
      &[("index.html", "hi")],
    );

    assert_eq!(declared_extension(&archive).expect("the bundle reads"), None);
  }

  #[test]
  fn something_that_is_not_a_bundle_is_an_error_rather_than_a_silent_nothing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("not-a-zip.zip");
    fs::write(&path, b"this is not an archive").expect("a decoy");

    assert!(
      declared_extension(&path).is_err(),
      "reporting no extension for a file nobody could read would install it unconfirmed"
    );
  }

  #[test]
  fn a_bundle_asking_for_more_than_the_install_offered_is_refused_and_never_lands() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let archive = bundle(
      dir.path(),
      "ext.zip",
      manifest_json(
        "1.0.0",
        Some(serde_json::json!({ "entry": "extension/desktop.mjs", "permissions": ["all"], "api": 1 })),
      ),
      &[("extension/desktop.mjs", "export {}")],
    );

    let refused = refusal(adopt(&store, &archive, &["net:discord.com"]));
    assert_eq!(refused.webapp, WEBAPP);
    assert_eq!(refused.name, "weather");
    assert_eq!(refused.permissions, permissions(&["all"]));
    assert!(
      refused.reason.contains("all"),
      "the row has to name what the bundle asked for; saw {}",
      refused.reason
    );
    assert!(
      store.list().is_empty(),
      "an unconfirmed grant must leave nothing for the supervisor to spawn"
    );
    assert!(
      !dir
        .path()
        .join("extensions")
        .join(WEBAPP.to_string())
        .join("1.0.0")
        .exists()
    );
  }

  #[test]
  fn an_extension_nobody_was_offered_is_refused_even_with_no_permissions() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let archive = bundle(
      dir.path(),
      "ext.zip",
      manifest_json(
        "1.0.0",
        Some(serde_json::json!({ "entry": "extension/desktop.mjs", "permissions": [], "api": 1 })),
      ),
      &[("extension/desktop.mjs", "export {}")],
    );

    let refused = refusal(store.adopt(Some(DEVICE), &archive, None));
    assert!(
      refused.reason.contains("never asked"),
      "a listing that hides the extension block must not buy silent consent; saw {}",
      refused.reason
    );
    assert!(store.list().is_empty());
  }

  #[test]
  fn consent_widens_only_the_way_the_descriptor_grammar_does() {
    assert_eq!(
      unconfirmed(Some(&permissions(&["all"])), &permissions(&["net:a.example", "ffi"])),
      Unconfirmed::Nothing,
      "confirming `all` confirms everything under it"
    );
    assert_eq!(
      unconfirmed(Some(&permissions(&["read"])), &permissions(&["read:/tmp"])),
      Unconfirmed::Nothing,
      "a bare kind covers its own scoped forms"
    );
    assert_eq!(
      unconfirmed(Some(&permissions(&["read:/tmp"])), &permissions(&["read"])),
      Unconfirmed::Beyond(vec!["read".to_owned()]),
      "a scoped grant must not widen into the bare kind"
    );
    assert_eq!(
      unconfirmed(Some(&permissions(&["net:a.example"])), &permissions(&["net:b.example"])),
      Unconfirmed::Beyond(vec!["net:b.example".to_owned()]),
      "one host is not another"
    );
    assert_eq!(
      unconfirmed(Some(&permissions(&["net"])), &permissions(&["all"])),
      Unconfirmed::Beyond(vec!["all".to_owned()]),
      "nothing short of `all` confirms `all`"
    );
    assert_eq!(unconfirmed(None, &[]), Unconfirmed::NeverOffered);
  }

  #[test]
  fn an_entry_that_climbs_out_of_the_bundle_is_refused_before_anything_can_run_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let outside = dir.path().join("evil.mjs");
    fs::write(&outside, "Deno.exit(0)").expect("a file outside the bundle");

    for hostile in [
      "../../../evil.mjs".to_owned(),
      outside.to_string_lossy().into_owned(),
      "extension/../../../evil.mjs".to_owned(),
      "noop.mjs".to_owned(),
    ] {
      let archive = bundle(
        dir.path(),
        "ext.zip",
        manifest_json(
          "1.0.0",
          Some(serde_json::json!({ "entry": hostile, "permissions": ["all"], "api": 1 })),
        ),
        &[("extension/noop.mjs", "export {}")],
      );

      let refused = refusal(adopt(&store, &archive, &["all"]));
      assert!(
        refused.reason.contains("not a path inside"),
        "{hostile} must be refused as an escape, not probed on disk; saw {}",
        refused.reason
      );
      assert!(store.list().is_empty(), "{hostile} left a runnable record behind");
    }
  }

  #[test]
  fn a_record_naming_a_path_outside_the_bundle_resolves_to_nothing_to_run() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = ExtensionStore::open(dir.path());
    let record = ExtensionRecord {
      webapp: WEBAPP,
      name: "weather".to_owned(),
      version: "1.0.0".to_owned(),
      entry: "extension/../../../../../../tmp/evil.mjs".to_owned(),
      permissions: Vec::new(),
      api: 1,
      enabled: true,
      devices: BTreeSet::new(),
    };

    assert_eq!(
      store.entry(&record),
      None,
      "the spawn path resolves the record too, not just the install"
    );
  }

  #[derive(Default, Clone)]
  struct Said(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

  impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Said {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
      struct Message<'a>(&'a mut String);
      impl tracing::field::Visit for Message<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
          if field.name() == "message" {
            self.0.push_str(&format!("{value:?}"));
          }
        }
      }
      let mut rendered = String::new();
      event.record(&mut Message(&mut rendered));
      if !rendered.is_empty() {
        self.0.lock().unwrap().push(rendered);
      }
    }
  }

  #[test]
  fn a_kv_file_that_does_not_parse_reads_empty_and_says_so() {
    use tracing_subscriber::layer::SubscriberExt as _;

    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("data").join(KV);
    fs::create_dir_all(path.parent().expect("a data directory")).expect("the data directory");
    fs::write(&path, br#"{"token":"ab"#).expect("a truncated store");

    let said = Said::default();
    let held = tracing::subscriber::with_default(tracing_subscriber::registry().with(said.clone()), || read_kv(&path));

    assert!(held.is_empty(), "a truncated store is not a usable store");
    let lines = said.0.lock().unwrap().clone();
    assert!(
      lines.iter().any(|line| line.contains("did not parse")),
      "losing a store silently is how an extension re-runs its oauth flow with nobody the wiser; saw {lines:?}"
    );

    let next = BTreeMap::from([("token".to_owned(), serde_json::json!("abc"))]);
    write_kv(&path, &next).expect("the kv file writes");
    assert_eq!(read_kv(&path), next);
  }

  #[test]
  fn a_missing_kv_file_is_not_worth_a_warning() {
    use tracing_subscriber::layer::SubscriberExt as _;

    let dir = tempfile::tempdir().expect("a scratch directory");
    let said = Said::default();
    let held = tracing::subscriber::with_default(tracing_subscriber::registry().with(said.clone()), || {
      read_kv(&dir.path().join("data").join(KV))
    });

    assert!(held.is_empty());
    assert!(
      said.0.lock().unwrap().is_empty(),
      "every extension starts without a store, so that is not news"
    );
  }

  #[test]
  fn a_kv_write_stages_beside_the_file_and_renames_it_into_place() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("data").join(KV);
    let mut staging = path.as_os_str().to_owned();
    staging.push(".part");
    let staging = PathBuf::from(staging);

    let first = BTreeMap::from([("token".to_owned(), serde_json::json!("abc"))]);
    write_kv(&path, &first).expect("the kv file writes");
    fs::write(&staging, b"a write that was interrupted").expect("a leftover staging file");

    let second = BTreeMap::from([("token".to_owned(), serde_json::json!("def"))]);
    write_kv(&path, &second).expect("the kv file writes again");

    assert!(
      !staging.exists(),
      "the store is renamed into place, so an interrupted write leaves the previous one intact"
    );
    assert_eq!(read_kv(&path), second);
  }

  #[test]
  fn the_kv_file_round_trips_and_a_missing_one_reads_empty() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("data").join(KV);
    assert!(read_kv(&path).is_empty());

    let held = BTreeMap::from([
      ("token".to_owned(), serde_json::json!("abc")),
      ("count".to_owned(), serde_json::json!(3)),
    ]);
    write_kv(&path, &held).expect("the kv file writes");

    assert_eq!(read_kv(&path), held);
  }
}
