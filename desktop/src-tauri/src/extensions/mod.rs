pub mod protocol;
pub mod runtime;
pub mod store;
pub mod supervisor;

use std::{
  collections::{BTreeSet, HashMap},
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::{
  api::WebappBundleSink,
  backend::{ExtensionConfigEntry, ExtensionHost, ExtensionHostInbox, ExtensionMessage},
};
use bridgething_io::HttpExecutor;
use libbridgething::ExtensionPermission;
use serde::Serialize;
use store::{Adopted, ExtensionStore};
use supervisor::{Command, Supervisor};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{hints::HintSink, settings::Authorize};

const HALT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct Deps {
  pub http: HttpExecutor,
  pub authorize: Arc<Authorize>,
  pub open_url: Arc<dyn Fn(String) -> Result<(), String> + Send + Sync>,
  pub hints: Arc<dyn HintSink>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExtensionStatus {
  Starting,
  Running,
  Crashed { reason: String },
  Stopped,
  RuntimeMissing { reason: String },
  Refused { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionEntry {
  pub id: String,
  pub name: String,
  pub version: String,
  pub permissions: Vec<String>,
  pub api: u32,
  pub enabled: bool,
  pub data_dir: String,
  pub status: ExtensionStatus,
  pub orphaned: bool,
}

pub struct Extensions {
  tx: mpsc::UnboundedSender<Command>,
  rx: Mutex<Option<mpsc::UnboundedReceiver<Command>>>,
  snapshot: Arc<Mutex<Vec<ExtensionEntry>>>,
  store: ExtensionStore,
  state_dir: PathBuf,
  identities: Mutex<HashMap<String, String>>,
}

impl Extensions {
  pub fn init(state_dir: &Path) -> Arc<Self> {
    let (tx, rx) = mpsc::unbounded_channel();
    Arc::new(Self {
      tx,
      rx: Mutex::new(Some(rx)),
      snapshot: Arc::new(Mutex::new(Vec::new())),
      store: ExtensionStore::open(state_dir),
      state_dir: state_dir.to_path_buf(),
      identities: Mutex::new(HashMap::new()),
    })
  }

  pub fn spawn(&self, deps: Deps) {
    let Some(rx) = self.rx.lock().unwrap().take() else {
      return;
    };
    let supervisor = Supervisor::new(
      self.state_dir.clone(),
      self.store.clone(),
      deps,
      Arc::clone(&self.snapshot),
      self.tx.clone(),
    );
    tauri::async_runtime::spawn(supervisor.run(rx));
  }

  pub fn list(&self) -> Vec<ExtensionEntry> {
    self.snapshot.lock().unwrap().clone()
  }

  pub fn data_dir(&self, webapp: Uuid) -> PathBuf {
    self.store.data_dir(webapp)
  }

  pub fn sink(
    self: &Arc<Self>,
    device: &str,
    confirmed: Option<Vec<ExtensionPermission>>,
  ) -> Arc<dyn WebappBundleSink> {
    Arc::new(Consented {
      extensions: Arc::clone(self),
      device: device.to_owned(),
      confirmed,
    })
  }

  pub fn adopt(&self, device: Option<&str>, archive: &Path, confirmed: Option<&[ExtensionPermission]>) {
    let device = device.map(|device| self.identity(device));
    match self.store.adopt(device.as_deref(), archive, confirmed) {
      Ok(Adopted::Installed(record)) => {
        tracing::info!(name = %record.name, version = %record.version, "a webapp brought a native extension");
        let _ = self.tx.send(Command::Installed(Box::new(record)));
      }
      Ok(Adopted::Refused(refusal)) => {
        let _ = self.tx.send(Command::Refused(Box::new(refusal)));
      }
      Ok(Adopted::None) => {}
      Err(error) => tracing::warn!(%error, path = %archive.display(), "a bundle's extension could not be taken out"),
    }
  }

  pub async fn adopt_off_worker(
    self: &Arc<Self>,
    device: String,
    archive: PathBuf,
    confirmed: Option<Vec<ExtensionPermission>>,
  ) {
    let me = Arc::clone(self);
    let _ = tokio::task::spawn_blocking(move || me.adopt(Some(&device), &archive, confirmed.as_deref())).await;
  }

  pub fn reconcile(&self, device: &str, installed: BTreeSet<Uuid>) {
    let device = self.identity(device);
    for record in self.store.list() {
      if installed.contains(&record.webapp) {
        if let Some(holders) = self.store.claim(record.webapp, &device) {
          let _ = self.tx.send(Command::Claims {
            webapp: record.webapp,
            holders,
          });
        }
      } else {
        self.removed(&device, record.webapp);
      }
    }
  }

  pub fn identified(&self, device: &str, identity: &str) {
    if device == identity {
      return;
    }
    let known = self
      .identities
      .lock()
      .unwrap()
      .insert(device.to_owned(), identity.to_owned());
    if known.as_deref() == Some(identity) {
      return;
    }
    for record in self.store.list() {
      if let Some(holders) = self.store.rekey(record.webapp, device, identity) {
        let _ = self.tx.send(Command::Claims {
          webapp: record.webapp,
          holders,
        });
      }
    }
  }

  pub fn link_gone(&self, device: &str) {
    self.identities.lock().unwrap().remove(device);
  }

  fn identity(&self, device: &str) -> String {
    self
      .identities
      .lock()
      .unwrap()
      .get(device)
      .cloned()
      .unwrap_or_else(|| device.to_owned())
  }

  pub fn forget_device(&self, device: &str) {
    self.let_go(&self.identity(device));
  }

  pub fn forget_address(&self, url: &str) {
    self.let_go(url);
  }

  fn let_go(&self, holder: &str) {
    for record in self.store.list() {
      self.dropped(holder, record.webapp);
    }
  }

  pub fn removed(&self, device: &str, webapp: Uuid) {
    self.dropped(&self.identity(device), webapp);
  }

  fn dropped(&self, holder: &str, webapp: Uuid) {
    let Some(left) = self.store.disown(webapp, holder) else {
      return;
    };
    if !left.is_empty() {
      let _ = self.tx.send(Command::Claims { webapp, holders: left });
      return;
    }
    tracing::info!(%webapp, "the last daemon holding a webapp let it go; its extension goes with it");
    let _ = self.tx.send(Command::Uninstalled(webapp));
  }

  pub fn remove(&self, webapp: Uuid) -> Result<(), String> {
    let Some(record) = self.store.record(webapp) else {
      return Err("this computer holds no such extension".to_owned());
    };
    if !record.devices.is_empty() {
      return Err("a Car Thing still has this app installed; uninstall it there".to_owned());
    }
    let _ = self.tx.send(Command::Uninstalled(webapp));
    Ok(())
  }

  pub fn set_enabled(&self, webapp: Uuid, enabled: bool) {
    let _ = self.tx.send(Command::Enabled { webapp, enabled });
  }

  pub fn retry_runtime(&self) {
    let _ = self.tx.send(Command::AcquireRuntime);
  }

  pub fn halt(&self) {
    let (done, waiting) = std::sync::mpsc::sync_channel(1);
    if self.tx.send(Command::Halt(done)).is_err() {
      return;
    }
    if waiting.recv_timeout(HALT_TIMEOUT).is_err() {
      tracing::warn!("an extension did not stop inside the grace period; leaving it to the process exit");
    }
  }
}

impl ExtensionHost for Extensions {
  fn start(&self, inbox: Arc<ExtensionHostInbox>) {
    let _ = self.tx.send(Command::Inbox(inbox));
  }

  fn stop(&self) {
    self.halt();
  }

  fn deliver(&self, device: String, webapp: String, message: ExtensionMessage) {
    let Some(webapp) = parse(&webapp) else { return };
    let _ = self.tx.send(Command::Deliver {
      device,
      webapp,
      message,
    });
  }

  fn device_connected(&self, device: String, name: String, config: Vec<ExtensionConfigEntry>, webapps: Vec<String>) {
    let _ = self.tx.send(Command::DeviceConnected {
      device,
      name,
      config,
      webapps: webapps.iter().filter_map(|id| parse(id)).collect(),
    });
  }

  fn device_disconnected(&self, device: String) {
    let _ = self.tx.send(Command::DeviceDisconnected { device });
  }

  fn device_active(&self, device: String, webapp: String, active: bool) {
    let Some(webapp) = parse(&webapp) else { return };
    let _ = self.tx.send(Command::DeviceActive { device, webapp, active });
  }

  fn config_changed(&self, device: String, webapp: String, key: String, value: Option<String>) {
    let Some(webapp) = parse(&webapp) else { return };
    let _ = self.tx.send(Command::ConfigChanged {
      device,
      webapp,
      key,
      value,
    });
  }
}

struct Consented {
  extensions: Arc<Extensions>,
  device: String,
  confirmed: Option<Vec<ExtensionPermission>>,
}

impl WebappBundleSink for Consented {
  fn installed(&self, bundle: String) {
    self
      .extensions
      .adopt(Some(&self.device), Path::new(&bundle), self.confirmed.as_deref());
  }
}

fn parse(raw: &str) -> Option<Uuid> {
  match Uuid::parse_str(raw) {
    Ok(webapp) => Some(webapp),
    Err(_) => {
      tracing::warn!(webapp = %raw, "the companion named a webapp that is not a uuid");
      None
    }
  }
}

#[cfg(test)]
pub fn sample_bundle(dir: &Path, webapp: Uuid, permissions: &str) -> PathBuf {
  use std::io::Write as _;

  let path = dir.join(format!("{webapp}.zip"));
  let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).expect("an archive"));
  let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
  zip.start_file("manifest.json", options).expect("a manifest entry");
  zip
    .write_all(
      format!(
        r#"{{"id":"{webapp}","name":"weather","version":"1.0.0","extension":{{"entry":"extension/desktop.mjs","permissions":{permissions},"api":1}}}}"#
      )
      .as_bytes(),
    )
    .expect("the manifest body");
  zip.start_file("extension/desktop.mjs", options).expect("an entry");
  zip.write_all(b"export {}").expect("the entry body");
  zip.finish().expect("a finished archive");
  path
}

#[cfg(test)]
mod tests {
  use super::*;

  const WEATHER: Uuid = Uuid::from_u128(0x2f0c_1a4b_0000_4000_8000_0000_0000_0001);
  const CLOCK: Uuid = Uuid::from_u128(0x2f0c_1a4b_0000_4000_8000_0000_0000_0002);
  const ONE: &str = "sn-1";
  const TWO: &str = "sn-2";

  fn drained(extensions: &Extensions) -> Vec<Command> {
    let mut rx = extensions.rx.lock().unwrap();
    let rx = rx.as_mut().expect("nothing took the receiver");
    let mut held = Vec::new();
    while let Ok(command) = rx.try_recv() {
      held.push(command);
    }
    held
  }

  fn only(extensions: &Extensions) -> Command {
    let mut held = drained(extensions);
    assert_eq!(held.len(), 1, "expected exactly one command");
    held.remove(0)
  }

  fn descriptors(raw: &[&str]) -> Vec<ExtensionPermission> {
    raw.iter().map(|raw| raw.parse().expect("a descriptor")).collect()
  }

  #[test]
  fn a_bundle_asking_beyond_the_confirmation_is_refused_rather_than_spawned() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    let archive = sample_bundle(dir.path(), WEATHER, r#"["all"]"#);

    extensions
      .sink(ONE, Some(descriptors(&["net:discord.com"])))
      .installed(archive.display().to_string());

    let Command::Refused(refusal) = only(&extensions) else {
      panic!("the manifest asked for more than the dialog showed, so nothing may be adopted")
    };
    assert_eq!(refusal.webapp, WEATHER);
    assert!(refusal.reason.contains("all"));
    assert!(
      extensions.store.list().is_empty(),
      "a refused bundle leaves nothing on disk to spawn on the next launch"
    );
  }

  #[test]
  fn a_bundle_matching_the_confirmation_installs() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    let archive = sample_bundle(dir.path(), WEATHER, r#"["net:discord.com"]"#);

    extensions
      .sink(ONE, Some(descriptors(&["net:discord.com"])))
      .installed(archive.display().to_string());

    let Command::Installed(record) = only(&extensions) else {
      panic!("what the dialog showed is what the manifest declares")
    };
    assert_eq!(record.webapp, WEATHER);
  }

  #[test]
  fn a_bundle_with_no_confirmation_behind_it_is_refused() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    let archive = sample_bundle(dir.path(), WEATHER, r#"["net:discord.com"]"#);

    extensions.sink(ONE, None).installed(archive.display().to_string());

    assert!(
      matches!(only(&extensions), Command::Refused(refusal) if refusal.reason.contains("never asked")),
      "an install that showed no dialog consents to nothing, however little the bundle asks for"
    );
  }

  #[test]
  fn overlapping_installs_each_land_against_their_own_confirmation() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    let narrow = sample_bundle(dir.path(), WEATHER, r#"["all"]"#);
    let wide = sample_bundle(dir.path(), CLOCK, r#"["all"]"#);

    let first = extensions.sink(ONE, Some(descriptors(&["net:discord.com"])));
    let second = extensions.sink(TWO, Some(descriptors(&["all"])));
    first.installed(narrow.display().to_string());
    second.installed(wide.display().to_string());

    let held = drained(&extensions);
    assert!(
      matches!(&held[0], Command::Refused(refusal) if refusal.webapp == WEATHER && refusal.reason.contains("all")),
      "an install that started while another was still collecting consent may not spend the other one's"
    );
    assert!(
      matches!(&held[1], Command::Installed(record) if record.webapp == CLOCK),
      "the install whose dialog showed all is the one that may run an all extension"
    );
  }

  #[test]
  fn an_extension_whose_webapp_is_gone_from_every_daemon_is_uninstalled() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(Some(ONE), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    let _ = drained(&extensions);

    extensions.reconcile(ONE, BTreeSet::from([WEATHER]));
    assert!(
      drained(&extensions).is_empty(),
      "an installed webapp keeps its extension"
    );

    extensions.reconcile(ONE, BTreeSet::new());
    assert!(
      matches!(only(&extensions), Command::Uninstalled(webapp) if webapp == WEATHER),
      "an uninstall from a phone never reaches this host's install command, so nothing else would ever notice"
    );
  }

  #[test]
  fn a_second_daemon_that_has_never_reported_does_not_uninstall_the_first_ones_extensions() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(Some(ONE), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    let _ = drained(&extensions);

    extensions.reconcile(ONE, BTreeSet::from([WEATHER]));
    extensions.reconcile(TWO, BTreeSet::new());

    assert!(
      drained(&extensions).is_empty(),
      "two Car Things do not have to hold the same apps for either one's extension to be legitimate"
    );
  }

  #[test]
  fn the_daemon_that_reports_first_after_a_restart_does_not_wipe_the_other_ones_extension() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(Some(ONE), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));

    let restarted = Extensions::init(dir.path());
    restarted.reconcile(TWO, BTreeSet::new());
    assert!(
      drained(&restarted).is_empty(),
      "which daemon's link comes up first is a race, so it may not decide whose extensions survive"
    );

    restarted.reconcile(ONE, BTreeSet::from([WEATHER]));
    assert!(
      drained(&restarted).is_empty(),
      "the daemon that owns the webapp still reports it, so nothing is gone"
    );
  }

  #[test]
  fn uninstalling_from_one_daemon_leaves_an_extension_a_second_one_still_holds() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(Some(ONE), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    extensions.reconcile(TWO, BTreeSet::from([WEATHER]));
    let _ = drained(&extensions);

    extensions.removed(ONE, WEATHER);
    assert!(
      !drained(&extensions)
        .iter()
        .any(|command| matches!(command, Command::Uninstalled(_))),
      "uninstalling from the selected device says nothing about the second Car Thing still running it"
    );

    extensions.removed(TWO, WEATHER);
    assert!(
      matches!(only(&extensions), Command::Uninstalled(webapp) if webapp == WEATHER),
      "the last holder letting go is what takes the extension with it"
    );
  }

  #[test]
  fn an_uninstall_over_a_named_link_reaches_the_claim_that_link_holds() {
    const URL: &str = "ws://bridgething.local:8892/";
    const SERIAL: &str = "8558R481Q61R";

    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(Some(URL), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    extensions.identified(URL, SERIAL);
    let _ = drained(&extensions);

    extensions.removed(URL, WEATHER);

    assert!(
      matches!(only(&extensions), Command::Uninstalled(webapp) if webapp == WEATHER),
      "the window uninstalls over a live link, which is addressed by url, and the claim that link made \
       is under what its daemon named itself"
    );
  }

  #[test]
  fn forgetting_the_last_device_that_held_a_webapp_takes_its_extension_with_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(Some(ONE), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    let _ = drained(&extensions);

    extensions.forget_device(ONE);

    assert!(
      matches!(only(&extensions), Command::Uninstalled(webapp) if webapp == WEATHER),
      "a device the user told the app to forget never reports a webapp list again, so nothing else \
       would ever disown it and the sidecar would run forever with no app to reach it from"
    );
  }

  #[test]
  fn forgetting_a_named_device_still_reaches_a_claim_left_under_the_address_it_answers_at() {
    const URL: &str = "ws://bridgething.local:8892/";
    const SERIAL: &str = "8558R481Q61R";

    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(Some(URL), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    extensions.identified(URL, SERIAL);
    extensions.store.claim(WEATHER, URL);
    let _ = drained(&extensions);

    extensions.forget_device(SERIAL);
    assert!(
      !drained(&extensions)
        .iter()
        .any(|command| matches!(command, Command::Uninstalled(_))),
      "the address is still holding it, so the row is not free of daemons yet"
    );

    extensions.forget_address(URL);

    assert!(
      matches!(only(&extensions), Command::Uninstalled(webapp) if webapp == WEATHER),
      "while the link is live the url answers to the name its daemon gave, so a claim under the url \
       is only reachable by a disown that does not translate it"
    );
  }

  #[test]
  fn forgetting_one_of_two_devices_leaves_the_extension_the_other_still_holds() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(Some(ONE), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    extensions.reconcile(TWO, BTreeSet::from([WEATHER]));
    let _ = drained(&extensions);

    extensions.forget_device(ONE);

    let Command::Claims { webapp, holders } = only(&extensions) else {
      panic!("the second Car Thing still has the app, so the extension stays rather than being uninstalled")
    };
    assert_eq!(webapp, WEATHER);
    assert_eq!(
      holders,
      BTreeSet::from([TWO.to_owned()]),
      "the window is told who is left holding it, or the row keeps saying nobody does"
    );
    assert_eq!(
      extensions.store.record(WEATHER).expect("the record survives").devices,
      BTreeSet::from([TWO.to_owned()])
    );
  }

  #[test]
  fn an_extension_no_device_claims_can_be_removed_by_hand() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(None, &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    let _ = drained(&extensions);

    extensions.remove(WEATHER).expect("an unclaimed extension is removable");

    assert!(
      matches!(only(&extensions), Command::Uninstalled(webapp) if webapp == WEATHER),
      "a record written before claims existed has no holder to let go of it, so the window is the only way out"
    );
  }

  #[test]
  fn an_extension_a_device_still_holds_is_not_removable_by_hand() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let extensions = Extensions::init(dir.path());
    extensions.adopt(Some(ONE), &sample_bundle(dir.path(), WEATHER, "[]"), Some(&[]));
    let _ = drained(&extensions);

    assert!(
      extensions.remove(WEATHER).is_err(),
      "the app is installed on a Car Thing; the extension goes when the app does"
    );
    assert!(drained(&extensions).is_empty());
    assert!(extensions.store.record(WEATHER).is_some());
  }
}
