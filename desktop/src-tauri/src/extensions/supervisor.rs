use std::{
  collections::{BTreeMap, BTreeSet, HashMap},
  path::{Path, PathBuf},
  process::Stdio,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::backend::{ExtensionConfigEntry, ExtensionHostInbox, ExtensionMessage};
use libbridgething::{EXTENSION_API_VERSION, ExtensionPermission};
use tokio::{
  io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
  process::Command as Spawn,
  sync::mpsc,
};
use uuid::Uuid;

use super::{
  Deps, ExtensionEntry, ExtensionStatus,
  protocol::{ChildMessage, HostMessage, Stdout, WebappIdentity, WireForward, WireLogLevel, read_line},
  runtime::{self, DenoRuntime},
  store::{ExtensionRecord, ExtensionStore, Refusal, read_kv, write_kv},
};
use crate::hints::{EXTENSIONS, Hint};

const STOP_GRACE: Duration = Duration::from_millis(1500);
const TERM_GRACE: Duration = Duration::from_millis(1500);
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_CEILING: Duration = Duration::from_secs(60);

pub enum Command {
  Inbox(Arc<ExtensionHostInbox>),
  Halt(std::sync::mpsc::SyncSender<()>),

  DeviceConnected {
    device: String,
    name: String,
    config: Vec<ExtensionConfigEntry>,
    webapps: Vec<Uuid>,
  },
  DeviceDisconnected {
    device: String,
  },
  DeviceActive {
    device: String,
    webapp: Uuid,
    active: bool,
  },
  ConfigChanged {
    device: String,
    webapp: Uuid,
    key: String,
    value: Option<String>,
  },
  Deliver {
    device: String,
    webapp: Uuid,
    message: ExtensionMessage,
  },
  Authorized {
    waiting: Waiting,
    outcome: Result<String, String>,
  },

  Installed(Box<ExtensionRecord>),
  Refused(Box<Refusal>),
  Uninstalled(Uuid),
  Claims {
    webapp: Uuid,
    holders: BTreeSet<String>,
  },
  Enabled {
    webapp: Uuid,
    enabled: bool,
  },
  AcquireRuntime,
  RuntimeReady(Result<DenoRuntime, String>),

  Line {
    webapp: Uuid,
    generation: u64,
    stream: Stream,
    line: String,
  },
  Exited {
    webapp: Uuid,
    generation: u64,
    reason: String,
  },
  Restart {
    webapp: Uuid,
    generation: u64,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
  Out,
  Err,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Waiting {
  pub webapp: Uuid,
  pub generation: u64,
  pub id: String,
}

enum Signal {
  Term,
  Kill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intent {
  Run,
  Leave,
}

#[derive(Default)]
enum Runtime {
  #[default]
  Absent,
  Acquiring,
  Ready(DenoRuntime),
  Missing(String),
}

#[derive(Default)]
struct Device {
  name: String,
  config: BTreeMap<Uuid, BTreeMap<String, String>>,
  collected: BTreeSet<Uuid>,
  active: Option<Uuid>,
}

impl Device {
  fn keep_settings_of_everyone_but(&mut self, named: &[Uuid], mut fresh: BTreeMap<Uuid, BTreeMap<String, String>>) {
    for webapp in named {
      self.config.insert(*webapp, fresh.remove(webapp).unwrap_or_default());
      self.collected.insert(*webapp);
    }
  }
}

struct Child {
  record: ExtensionRecord,
  status: ExtensionStatus,
  attempt: u64,
  armed: Option<u64>,
  seq: u64,
  intent: Intent,
  stdin: Option<mpsc::UnboundedSender<HostMessage>>,
  kill: Option<mpsc::UnboundedSender<Signal>>,
  kv: BTreeMap<String, serde_json::Value>,
  failures: u32,
  announced: BTreeMap<String, HostMessage>,
}

impl Child {
  fn new(record: ExtensionRecord) -> Self {
    Self {
      record,
      status: ExtensionStatus::Stopped,
      attempt: 0,
      armed: None,
      seq: 0,
      intent: Intent::Leave,
      stdin: None,
      kill: None,
      kv: BTreeMap::new(),
      failures: 0,
      announced: BTreeMap::new(),
    }
  }

  fn token(&mut self) -> u64 {
    self.seq += 1;
    self.seq
  }

  fn write(&self, message: HostMessage) {
    if let Some(stdin) = self.stdin.as_ref() {
      let _ = stdin.send(message);
    }
  }
}

pub struct Supervisor {
  store: ExtensionStore,
  state_dir: PathBuf,
  deps: Deps,
  snapshot: Arc<Mutex<Vec<ExtensionEntry>>>,
  tx: mpsc::UnboundedSender<Command>,
  inbox: Option<Arc<ExtensionHostInbox>>,
  runtime: Runtime,
  devices: BTreeMap<String, Device>,
  children: HashMap<Uuid, Child>,
  waiting: HashMap<Waiting, u64>,
  refused: BTreeMap<Uuid, Refusal>,
  published: BTreeSet<Uuid>,
}

impl Supervisor {
  pub fn new(
    state_dir: PathBuf,
    store: ExtensionStore,
    deps: Deps,
    snapshot: Arc<Mutex<Vec<ExtensionEntry>>>,
    tx: mpsc::UnboundedSender<Command>,
  ) -> Self {
    Self {
      store,
      state_dir,
      deps,
      snapshot,
      tx,
      inbox: None,
      runtime: Runtime::default(),
      devices: BTreeMap::new(),
      children: HashMap::new(),
      waiting: HashMap::new(),
      refused: BTreeMap::new(),
      published: BTreeSet::new(),
    }
  }

  pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Command>) {
    for record in self.store.list() {
      self.children.insert(record.webapp, Child::new(record));
    }
    self.publish();
    if !self.children.is_empty() {
      self.acquire();
    }

    while let Some(command) = rx.recv().await {
      self.handle(command).await;
    }
    self.halt().await;
  }

  async fn handle(&mut self, command: Command) {
    match command {
      Command::Halt(done) => {
        self.halt().await;
        let _ = done.send(());
      }
      Command::Inbox(inbox) => {
        self.published = self.running_set();
        inbox.running_changed(self.published.iter().map(Uuid::to_string).collect());
        self.inbox = Some(inbox);
        for webapp in self.enabled_ids() {
          self.start(webapp);
        }
      }
      Command::DeviceConnected {
        device,
        name,
        config,
        webapps,
      } => self.device_connected(device, name, config, webapps),
      Command::DeviceDisconnected { device } => self.device_disconnected(&device),
      Command::DeviceActive { device, webapp, active } => self.device_active(device, webapp, active),
      Command::ConfigChanged {
        device,
        webapp,
        key,
        value,
      } => self.config_changed(device, webapp, key, value),
      Command::Deliver {
        device,
        webapp,
        message,
      } => self.deliver(device, webapp, message),
      Command::Authorized { waiting, outcome } => self.settle(waiting, outcome),
      Command::Installed(record) => self.installed(*record).await,
      Command::Refused(refusal) => self.refused(*refusal).await,
      Command::Uninstalled(webapp) => self.uninstalled(webapp).await,
      Command::Claims { webapp, holders } => self.claims(webapp, holders),
      Command::Enabled { webapp, enabled } => self.enabled(webapp, enabled).await,
      Command::AcquireRuntime => self.acquire(),
      Command::RuntimeReady(outcome) => self.runtime_ready(outcome),
      Command::Line {
        webapp,
        generation,
        stream,
        line,
      } => self.line(webapp, generation, stream, line),
      Command::Exited {
        webapp,
        generation,
        reason,
      } => self.exited(webapp, generation, reason),
      Command::Restart { webapp, generation } => {
        let armed = self
          .children
          .get_mut(&webapp)
          .filter(|child| child.armed == Some(generation));
        if let Some(child) = armed {
          child.armed = None;
          self.start(webapp);
        }
      }
    }
  }

  fn device_connected(&mut self, device: String, name: String, config: Vec<ExtensionConfigEntry>, webapps: Vec<Uuid>) {
    let mut fresh: BTreeMap<Uuid, BTreeMap<String, String>> = BTreeMap::new();
    for entry in config {
      let Ok(webapp) = Uuid::parse_str(&entry.webapp) else {
        continue;
      };
      fresh.entry(webapp).or_default().insert(entry.key, entry.value);
    }

    let held = self.devices.entry(device.clone()).or_default();
    held.name = name;
    held.keep_settings_of_everyone_but(&webapps, fresh);

    for webapp in webapps {
      self.announce_connected(webapp, &device);
    }
  }

  fn announce_connected(&mut self, webapp: Uuid, device: &str) {
    if !self
      .devices
      .get(device)
      .is_some_and(|held| held.collected.contains(&webapp))
    {
      return;
    }
    let message = self.connected_message(device, webapp);
    self.announce(webapp, device, message);
  }

  fn announce_devices(&mut self, webapp: Uuid) {
    for device in self.devices.keys().cloned().collect::<Vec<_>>() {
      self.announce_connected(webapp, &device);
    }
  }

  fn announce(&mut self, webapp: Uuid, device: &str, message: HostMessage) {
    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    if child.announced.get(device) == Some(&message) {
      return;
    }
    child.announced.insert(device.to_owned(), message.clone());
    child.write(message);
  }

  fn device_disconnected(&mut self, device: &str) {
    if self.devices.remove(device).is_none() {
      return;
    }
    for child in self.children.values_mut() {
      if child.announced.remove(device).is_none() {
        continue;
      }
      child.write(HostMessage::DeviceDisconnected {
        device: device.to_owned(),
      });
    }
  }

  fn device_active(&mut self, device: String, webapp: Uuid, active: bool) {
    let held = self.devices.entry(device.clone()).or_default();
    let was = held.active == Some(webapp);
    if active {
      held.active = Some(webapp);
    } else if was {
      held.active = None;
    }
    if was == active {
      return;
    }
    let message = HostMessage::DeviceActive {
      device: device.clone(),
      active,
    };
    self.tell(webapp, &device, message);
  }

  fn config_changed(&mut self, device: String, webapp: Uuid, key: String, value: Option<String>) {
    let held = self.devices.entry(device.clone()).or_default();
    let settings = held.config.entry(webapp).or_default();
    match value.clone() {
      Some(set) => {
        settings.insert(key.clone(), set);
      }
      None => {
        settings.remove(&key);
      }
    }
    let message = HostMessage::ConfigChanged {
      device: device.clone(),
      key,
      value,
    };
    self.tell(webapp, &device, message);
  }

  fn deliver(&mut self, device: String, webapp: Uuid, message: ExtensionMessage) {
    match WireForward::try_from(message) {
      Ok(message) => {
        let hosted = HostMessage::DeviceMessage {
          device: device.clone(),
          message,
        };
        self.tell(webapp, &device, hosted);
      }
      Err(error) => tracing::warn!(%webapp, %error, "a forward did not survive the hop to stdio"),
    }
  }

  fn settle(&mut self, waiting: Waiting, outcome: Result<String, String>) {
    let Some(token) = self.waiting.remove(&waiting) else {
      return;
    };
    self.deps.authorize.release(token);
    let message = match outcome {
      Ok(url) => HostMessage::answer(waiting.id, serde_json::Value::String(url)),
      Err(reason) => HostMessage::refuse(waiting.id, reason),
    };
    self.send(waiting.webapp, message);
  }

  fn abandon(&mut self, webapp: Uuid, generation: u64) {
    let dead: Vec<Waiting> = self
      .waiting
      .keys()
      .filter(|held| held.webapp == webapp && held.generation == generation)
      .cloned()
      .collect();
    for held in dead {
      if let Some(token) = self.waiting.remove(&held) {
        self.deps.authorize.release(token);
      }
    }
  }

  async fn installed(&mut self, record: ExtensionRecord) {
    let webapp = record.webapp;
    let enabled = record.enabled;
    self.refused.remove(&webapp);
    match self.children.get_mut(&webapp) {
      Some(child) => child.record = record,
      None => {
        self.children.insert(webapp, Child::new(record));
      }
    }
    self.stop(webapp, Intent::Leave);
    self.reap(webapp).await;
    if enabled {
      self.start(webapp);
    } else {
      self.mark(webapp, ExtensionStatus::Stopped);
    }
  }

  async fn refused(&mut self, refusal: Refusal) {
    tracing::warn!(
      name = %refusal.name,
      version = %refusal.version,
      reason = %refusal.reason,
      "a webapp's native extension was refused and will not run"
    );
    let webapp = refusal.webapp;
    self.uninstalled(webapp).await;
    self.refused.insert(webapp, refusal);
    self.publish();
  }

  async fn uninstalled(&mut self, webapp: Uuid) {
    self.stop(webapp, Intent::Leave);
    self.reap(webapp).await;
    self.children.remove(&webapp);
    self.refused.remove(&webapp);
    self.store.remove(webapp);
    self.publish();
    self.announce_running();
  }

  fn claims(&mut self, webapp: Uuid, holders: BTreeSet<String>) {
    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    if child.record.devices == holders {
      return;
    }
    child.record.devices = holders;
    self.publish();
  }

  async fn enabled(&mut self, webapp: Uuid, enabled: bool) {
    let Some(record) = self.store.set_enabled(webapp, enabled) else {
      return;
    };
    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    child.record = record;
    if enabled {
      self.start(webapp);
      return;
    }
    self.stop(webapp, Intent::Leave);
    self.reap(webapp).await;
    self.mark(webapp, ExtensionStatus::Stopped);
  }

  fn acquire(&mut self) {
    if matches!(self.runtime, Runtime::Acquiring | Runtime::Ready(_)) {
      return;
    }
    self.runtime = Runtime::Acquiring;
    for webapp in self.enabled_ids() {
      self.mark(webapp, ExtensionStatus::Starting);
    }
    let state_dir = self.state_dir.clone();
    let http = self.deps.http.clone();
    let tx = self.tx.clone();
    tokio::spawn(async move {
      let outcome = runtime::acquire(&state_dir, &http).await;
      if let Err(error) = &outcome {
        tracing::warn!(%error, "the deno runtime could not be acquired");
      }
      let _ = tx.send(Command::RuntimeReady(outcome));
    });
  }

  fn runtime_ready(&mut self, outcome: Result<DenoRuntime, String>) {
    match outcome {
      Ok(runtime) => {
        self.runtime = Runtime::Ready(runtime);
        for webapp in self.enabled_ids() {
          self.start(webapp);
        }
      }
      Err(reason) => {
        self.runtime = Runtime::Missing(reason.clone());
        for webapp in self.enabled_ids() {
          self.mark(webapp, ExtensionStatus::RuntimeMissing { reason: reason.clone() });
        }
      }
    }
  }

  fn start(&mut self, webapp: Uuid) {
    let runtime = match &self.runtime {
      Runtime::Ready(runtime) => runtime.clone(),
      Runtime::Missing(reason) => {
        let reason = reason.clone();
        self.mark(webapp, ExtensionStatus::RuntimeMissing { reason });
        return;
      }
      Runtime::Absent | Runtime::Acquiring => {
        self.acquire();
        self.mark(webapp, ExtensionStatus::Starting);
        return;
      }
    };

    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    if child.stdin.is_some() || !child.record.enabled {
      return;
    }
    child.armed = None;
    child.intent = Intent::Run;
    child.attempt = child.token();
    let generation = child.attempt;
    let record = child.record.clone();

    let Some(entry) = self.store.entry(&record) else {
      let reason = format!("{} is not a path inside the extracted bundle", record.entry);
      tracing::error!(%webapp, %reason, "an extension names something the host will not run");
      self.fail(webapp, generation, reason);
      return;
    };
    let mut spawn = Spawn::new(&runtime.binary);
    spawn
      .args(deno_args(&record.permissions, home_dir().as_deref()))
      .arg(&entry)
      .current_dir(self.store.data_dir(record.webapp))
      .env("DENO_DIR", &runtime.cache)
      .env("DENO_NO_PACKAGE_JSON", "1")
      .env("DENO_NO_UPDATE_CHECK", "1")
      .env("NO_COLOR", "1")
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .kill_on_drop(true);

    let mut process = match spawn.spawn() {
      Ok(process) => process,
      Err(error) => {
        let reason = format!("{} did not spawn: {error}", entry.display());
        tracing::error!(%webapp, %reason, "an extension could not start");
        self.fail(webapp, generation, reason);
        return;
      }
    };
    tracing::info!(name = %record.name, version = %record.version, "an extension is starting");

    let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();
    let (kill_tx, kill_rx) = mpsc::unbounded_channel();
    let stdin = process.stdin.take().expect("a piped stdin");
    let stdout = process.stdout.take().expect("a piped stdout");
    let stderr = process.stderr.take().expect("a piped stderr");

    tokio::spawn(feed(stdin, stdin_rx));
    tokio::spawn(drain(stdout, webapp, generation, Stream::Out, self.tx.clone()));
    tokio::spawn(drain(stderr, webapp, generation, Stream::Err, self.tx.clone()));
    tokio::spawn(reap(process, webapp, generation, kill_rx, self.tx.clone()));

    let data_dir = self.store.data_dir(record.webapp);
    let kv = read_kv(&self.store.kv_path(record.webapp));

    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    child.stdin = Some(stdin_tx);
    child.kill = Some(kill_tx);
    child.kv = kv;
    child.status = ExtensionStatus::Starting;
    child.announced.clear();
    child.write(HostMessage::Hello {
      api: EXTENSION_API_VERSION,
      webapp: WebappIdentity {
        id: record.webapp.to_string(),
        name: record.name.clone(),
        version: record.version.clone(),
      },
      data_dir: data_dir.to_string_lossy().into_owned(),
    });

    self.announce_devices(webapp);
    self.publish();
  }

  fn stop(&mut self, webapp: Uuid, intent: Intent) {
    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    child.intent = intent;
    child.armed = None;
    let Some(kill) = child.kill.clone() else {
      child.stdin = None;
      return;
    };
    child.write(HostMessage::Stop);
    child.stdin = None;
    tokio::spawn(async move {
      tokio::time::sleep(STOP_GRACE).await;
      if kill.send(Signal::Term).is_err() {
        return;
      }
      tokio::time::sleep(TERM_GRACE).await;
      let _ = kill.send(Signal::Kill);
    });
  }

  async fn reap(&mut self, webapp: Uuid) {
    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    let Some(kill) = child.kill.take() else { return };
    let deadline = tokio::time::Instant::now() + STOP_GRACE + TERM_GRACE;
    while !kill.is_closed() && tokio::time::Instant::now() < deadline {
      tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let _ = kill.send(Signal::Kill);
  }

  fn exited(&mut self, webapp: Uuid, generation: u64, reason: String) {
    self.abandon(webapp, generation);
    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    if child.attempt != generation {
      return;
    }
    child.stdin = None;
    child.kill = None;
    if child.intent == Intent::Leave {
      child.status = ExtensionStatus::Stopped;
      tracing::info!(name = %child.record.name, "an extension stopped");
      self.publish();
      self.announce_running();
      return;
    }
    tracing::warn!(name = %child.record.name, %reason, "an extension exited on its own");
    self.fail(webapp, generation, reason);
  }

  fn fail(&mut self, webapp: Uuid, generation: u64, reason: String) {
    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    if child.attempt != generation {
      return;
    }
    child.status = ExtensionStatus::Crashed { reason };
    child.failures = child.failures.saturating_add(1);
    let armed = child.token();
    child.armed = Some(armed);
    let wait = backoff(child.failures);
    let tx = self.tx.clone();
    tokio::spawn(async move {
      tokio::time::sleep(wait).await;
      let _ = tx.send(Command::Restart {
        webapp,
        generation: armed,
      });
    });
    self.publish();
    self.announce_running();
  }

  async fn halt(&mut self) {
    for webapp in self.children.keys().copied().collect::<Vec<_>>() {
      self.stop(webapp, Intent::Leave);
    }
    for webapp in self.children.keys().copied().collect::<Vec<_>>() {
      self.reap(webapp).await;
    }
  }

  fn line(&mut self, webapp: Uuid, generation: u64, stream: Stream, line: String) {
    if self
      .children
      .get(&webapp)
      .is_none_or(|child| child.attempt != generation)
    {
      return;
    }
    if stream == Stream::Err {
      self.tap(webapp, WireLogLevel::Warn, &line);
      return;
    }
    match read_line(&line) {
      Stdout::Output(raw) => self.tap(webapp, WireLogLevel::Info, &raw),
      Stdout::Protocol(message) => self.spoken(webapp, *message),
    }
  }

  fn spoken(&mut self, webapp: Uuid, message: ChildMessage) {
    match message {
      ChildMessage::Ready => {
        let Some(child) = self.children.get_mut(&webapp) else {
          return;
        };
        child.failures = 0;
        child.status = ExtensionStatus::Running;
        tracing::info!(name = %child.record.name, "an extension is running");
        self.publish();
        self.announce_running();
      }
      ChildMessage::Log { level, message } => self.tap(webapp, level, &message),
      ChildMessage::DeviceSend { device, message } => match message.into_message() {
        Ok(message) => {
          if let Some(inbox) = self.inbox.as_ref() {
            inbox.send_to_device(device, webapp.to_string(), message);
          }
        }
        Err(error) => tracing::warn!(%webapp, %error, "an extension sent a forward that does not decode"),
      },
      ChildMessage::KvGet { id, key } => {
        let value = self
          .children
          .get(&webapp)
          .and_then(|child| child.kv.get(&key).cloned())
          .unwrap_or(serde_json::Value::Null);
        self.send(webapp, HostMessage::answer(id, value));
      }
      ChildMessage::KvList { id } => {
        let keys = self
          .children
          .get(&webapp)
          .map(|child| child.kv.keys().cloned().collect::<Vec<_>>())
          .unwrap_or_default();
        self.send(webapp, HostMessage::answer(id, serde_json::json!(keys)));
      }
      ChildMessage::KvSet { id, key, value } => {
        let Some(child) = self.children.get_mut(&webapp) else {
          return;
        };
        child.kv.insert(key, value);
        let outcome = write_kv(&self.store.kv_path(webapp), &self.children[&webapp].kv);
        self.answer_write(webapp, id, outcome);
      }
      ChildMessage::KvDelete { id, key } => {
        let Some(child) = self.children.get_mut(&webapp) else {
          return;
        };
        child.kv.remove(&key);
        let outcome = write_kv(&self.store.kv_path(webapp), &self.children[&webapp].kv);
        self.answer_write(webapp, id, outcome);
      }
      ChildMessage::Authorize { id, url } => self.authorize(webapp, id, url),
    }
  }

  fn answer_write(&mut self, webapp: Uuid, id: String, outcome: Result<(), String>) {
    let message = match outcome {
      Ok(()) => HostMessage::answer(id, serde_json::Value::Null),
      Err(reason) => HostMessage::refuse(id, reason),
    };
    self.send(webapp, message);
  }

  fn authorize(&mut self, webapp: Uuid, id: String, url: String) {
    let Some(generation) = self.children.get(&webapp).map(|child| child.attempt) else {
      return;
    };
    let awaiting = match self.deps.authorize.begin(url, &*self.deps.open_url) {
      Ok(awaiting) => awaiting,
      Err(error) => {
        self.send(webapp, HostMessage::refuse(id, error.to_string()));
        return;
      }
    };

    let waiting = Waiting { webapp, generation, id };
    self.waiting.insert(waiting.clone(), awaiting.token);
    let tx = self.tx.clone();
    tokio::spawn(async move {
      let outcome = awaiting
        .settled()
        .await
        .map(|callback| callback.to_string())
        .map_err(|error| error.to_string());
      let _ = tx.send(Command::Authorized { waiting, outcome });
    });
  }

  fn tap(&self, webapp: Uuid, level: WireLogLevel, line: &str) {
    let name = self
      .children
      .get(&webapp)
      .map(|child| child.record.name.as_str())
      .unwrap_or("extension");
    match level {
      WireLogLevel::Debug => tracing::debug!("{name}: {line}"),
      WireLogLevel::Info => tracing::info!("{name}: {line}"),
      WireLogLevel::Warn => tracing::warn!("{name}: {line}"),
      WireLogLevel::Error => tracing::error!("{name}: {line}"),
    }
  }

  fn send(&self, webapp: Uuid, message: HostMessage) {
    if let Some(child) = self.children.get(&webapp) {
      child.write(message);
    }
  }

  fn tell(&self, webapp: Uuid, device: &str, message: HostMessage) {
    let Some(child) = self.children.get(&webapp) else {
      return;
    };
    if !child.announced.contains_key(device) {
      return;
    }
    child.write(message);
  }

  fn connected_message(&self, device: &str, webapp: Uuid) -> HostMessage {
    let held = self.devices.get(device);
    HostMessage::DeviceConnected {
      device: device.to_owned(),
      name: held.map(|held| held.name.clone()).unwrap_or_default(),
      config: held
        .and_then(|held| held.config.get(&webapp).cloned())
        .unwrap_or_default(),
      active: held.is_some_and(|held| held.active == Some(webapp)),
    }
  }

  fn enabled_ids(&self) -> Vec<Uuid> {
    self
      .children
      .iter()
      .filter(|(_, child)| child.record.enabled)
      .map(|(webapp, _)| *webapp)
      .collect()
  }

  fn mark(&mut self, webapp: Uuid, status: ExtensionStatus) {
    let Some(child) = self.children.get_mut(&webapp) else {
      return;
    };
    if child.status == status {
      return;
    }
    child.status = status;
    self.publish();
    self.announce_running();
  }

  fn running_set(&self) -> BTreeSet<Uuid> {
    self
      .children
      .iter()
      .filter(|(_, child)| child.status == ExtensionStatus::Running)
      .map(|(webapp, _)| *webapp)
      .collect()
  }

  fn announce_running(&mut self) {
    let running = self.running_set();
    if running == self.published {
      return;
    }
    self.published = running;
    let Some(inbox) = self.inbox.as_ref() else { return };
    inbox.running_changed(self.published.iter().map(Uuid::to_string).collect());
  }

  fn publish(&self) {
    let refused = self
      .refused
      .values()
      .filter(|refusal| !self.children.contains_key(&refusal.webapp))
      .map(|refusal| ExtensionEntry {
        id: refusal.webapp.to_string(),
        name: refusal.name.clone(),
        version: refusal.version.clone(),
        permissions: refusal.permissions.iter().map(ExtensionPermission::to_string).collect(),
        api: refusal.api,
        enabled: false,
        data_dir: self.store.data_dir(refusal.webapp).to_string_lossy().into_owned(),
        status: ExtensionStatus::Refused {
          reason: refusal.reason.clone(),
        },
        orphaned: false,
      });
    let mut entries: Vec<ExtensionEntry> = self
      .children
      .values()
      .map(|child| ExtensionEntry {
        id: child.record.webapp.to_string(),
        name: child.record.name.clone(),
        version: child.record.version.clone(),
        permissions: child
          .record
          .permissions
          .iter()
          .map(ExtensionPermission::to_string)
          .collect(),
        api: child.record.api,
        enabled: child.record.enabled,
        data_dir: self.store.data_dir(child.record.webapp).to_string_lossy().into_owned(),
        status: child.status.clone(),
        orphaned: child.record.devices.is_empty(),
      })
      .chain(refused)
      .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    *self.snapshot.lock().unwrap() = entries;
    self.deps.hints.emit(Hint::bare(EXTENSIONS));
  }
}

pub fn deno_args(permissions: &[ExtensionPermission], home: Option<&Path>) -> Vec<String> {
  let mut args = vec!["run".to_owned(), "--no-prompt".to_owned()];
  args.extend(ExtensionPermission::deno_flags(&resolved(permissions, home)));
  args
}

fn resolved(permissions: &[ExtensionPermission], home: Option<&Path>) -> Vec<ExtensionPermission> {
  const PATHY: &[&str] = &["read", "write", "run", "ffi"];
  let Some(home) = home else {
    return permissions.to_vec();
  };
  permissions
    .iter()
    .map(|permission| {
      let descriptor = permission.to_string();
      let Some((kind, scope)) = descriptor.split_once(':') else {
        return permission.clone();
      };
      if !PATHY.contains(&kind) {
        return permission.clone();
      }
      let expanded = match scope.strip_prefix('~') {
        Some("") => home.to_string_lossy().into_owned(),
        Some(rest) if rest.starts_with('/') => format!("{}{rest}", home.to_string_lossy()),
        _ => return permission.clone(),
      };
      format!("{kind}:{expanded}")
        .parse()
        .unwrap_or_else(|_| permission.clone())
    })
    .collect()
}

fn home_dir() -> Option<PathBuf> {
  directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

pub fn backoff(failures: u32) -> Duration {
  let step = failures.saturating_sub(1).min(16);
  BACKOFF_CEILING.min(BACKOFF_BASE * 2u32.saturating_pow(step))
}

async fn feed(mut stdin: tokio::process::ChildStdin, mut rx: mpsc::UnboundedReceiver<HostMessage>) {
  while let Some(message) = rx.recv().await {
    let stop = matches!(message, HostMessage::Stop);
    if stdin.write_all(message.line().as_bytes()).await.is_err() {
      return;
    }
    if stdin.flush().await.is_err() || stop {
      return;
    }
  }
}

async fn drain<R: tokio::io::AsyncRead + Unpin>(
  source: R,
  webapp: Uuid,
  generation: u64,
  stream: Stream,
  tx: mpsc::UnboundedSender<Command>,
) {
  let mut lines = BufReader::new(source).lines();
  while let Ok(Some(line)) = lines.next_line().await {
    if line.trim().is_empty() {
      continue;
    }
    if tx
      .send(Command::Line {
        webapp,
        generation,
        stream,
        line,
      })
      .is_err()
    {
      return;
    }
  }
}

async fn reap(
  mut process: tokio::process::Child,
  webapp: Uuid,
  generation: u64,
  mut kill: mpsc::UnboundedReceiver<Signal>,
  tx: mpsc::UnboundedSender<Command>,
) {
  let reason = loop {
    tokio::select! {
      done = process.wait() => break match done {
        Ok(status) => format!("exited {status}"),
        Err(error) => format!("could not be waited on: {error}"),
      },
      signal = kill.recv() => match signal {
        Some(Signal::Term) => terminate(&process),
        Some(Signal::Kill) | None => {
          let _ = process.start_kill();
        }
      },
    }
  };
  let _ = tx.send(Command::Exited {
    webapp,
    generation,
    reason,
  });
}

#[cfg(unix)]
fn terminate(process: &tokio::process::Child) {
  if let Some(pid) = process.id() {
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
  }
}

#[cfg(not(unix))]
fn terminate(_process: &tokio::process::Child) {}

#[cfg(test)]
mod tests {
  use bridgething_companion::backend::ExtensionOutbound;
  use bridgething_io::{HttpDownloadSink, HttpExecutor, HttpRequest, HttpSink, HttpTransport};

  use super::*;
  use crate::{hints::HintSink, settings::Authorize};

  struct Offline;

  impl HttpTransport for Offline {
    fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
      sink.fail("no network in a state machine test".to_owned());
    }

    fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      sink.on_failed("no network in a state machine test".to_owned());
    }
  }

  struct Deaf;

  impl HintSink for Deaf {
    fn emit(&self, _hint: Hint) {}
  }

  const WEATHER: Uuid = Uuid::from_u128(0x2f0c_1a4b_0000_4000_8000_0000_0000_0001);
  const CLOCK: Uuid = Uuid::from_u128(0x2f0c_1a4b_0000_4000_8000_0000_0000_0002);

  #[derive(Default, Clone)]
  struct Opened(Arc<Mutex<Vec<String>>>);

  impl Opened {
    fn urls(&self) -> Vec<String> {
      self.0.lock().unwrap().clone()
    }
  }

  fn probe() -> (Supervisor, mpsc::UnboundedReceiver<Command>) {
    let (supervisor, rx, _) = probe_opening();
    (supervisor, rx)
  }

  fn probe_opening() -> (Supervisor, mpsc::UnboundedReceiver<Command>, Opened) {
    let opened = Opened::default();
    let seen = opened.clone();
    let (supervisor, rx) = probe_with(Arc::new(move |url| {
      seen.0.lock().unwrap().push(url);
      Ok(())
    }));
    (supervisor, rx, opened)
  }

  fn probe_with(
    open_url: Arc<dyn Fn(String) -> Result<(), String> + Send + Sync>,
  ) -> (Supervisor, mpsc::UnboundedReceiver<Command>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let supervisor = Supervisor::new(
      PathBuf::from("/does/not/exist"),
      ExtensionStore::open(Path::new("/does/not/exist")),
      Deps {
        http: HttpExecutor::new(Arc::new(Offline)),
        authorize: Arc::new(Authorize::default()),
        open_url,
        hints: Arc::new(Deaf),
      },
      Arc::new(Mutex::new(Vec::new())),
      tx,
    );
    (supervisor, rx)
  }

  fn attach(supervisor: &mut Supervisor, webapp: Uuid, name: &str) -> mpsc::UnboundedReceiver<HostMessage> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut child = Child::new(ExtensionRecord {
      webapp,
      name: name.to_owned(),
      version: "1.0.0".to_owned(),
      entry: "extension/desktop.mjs".to_owned(),
      permissions: Vec::new(),
      api: EXTENSION_API_VERSION,
      enabled: true,
      devices: BTreeSet::new(),
    });
    child.stdin = Some(tx);
    child.status = ExtensionStatus::Running;
    supervisor.children.insert(webapp, child);
    rx
  }

  fn heard(rx: &mut mpsc::UnboundedReceiver<HostMessage>) -> Vec<HostMessage> {
    let mut held = Vec::new();
    while let Ok(message) = rx.try_recv() {
      held.push(message);
    }
    held
  }

  fn setting(webapp: Uuid, key: &str, value: &str) -> ExtensionConfigEntry {
    ExtensionConfigEntry {
      webapp: webapp.to_string(),
      key: key.to_owned(),
      value: value.to_owned(),
    }
  }

  #[test]
  fn a_connecting_device_reaches_every_live_extension_with_only_its_own_settings() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");
    let mut clock = attach(&mut supervisor, CLOCK, "clock");

    supervisor.device_connected(
      "sn-1".to_owned(),
      "car thing".to_owned(),
      vec![setting(WEATHER, "zip", "10001"), setting(CLOCK, "chime", "on")],
      vec![WEATHER, CLOCK],
    );

    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceConnected {
        device: "sn-1".to_owned(),
        name: "car thing".to_owned(),
        config: BTreeMap::from([("zip".to_owned(), "10001".to_owned())]),
        active: false,
      }],
      "an extension sees its own settings and nobody else's"
    );
    assert_eq!(
      heard(&mut clock),
      vec![HostMessage::DeviceConnected {
        device: "sn-1".to_owned(),
        name: "car thing".to_owned(),
        config: BTreeMap::from([("chime".to_owned(), "on".to_owned())]),
        active: false,
      }]
    );
  }

  #[test]
  fn a_reannounced_device_carries_the_active_flag_the_host_already_learned() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");

    supervisor.device_connected("sn-1".to_owned(), "car thing".to_owned(), Vec::new(), vec![WEATHER]);
    supervisor.device_active("sn-1".to_owned(), WEATHER, true);
    let _ = heard(&mut weather);
    supervisor.device_connected("sn-1".to_owned(), "car thing".to_owned(), Vec::new(), vec![WEATHER]);

    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceConnected {
        device: "sn-1".to_owned(),
        name: "car thing".to_owned(),
        config: BTreeMap::new(),
        active: true,
      }],
      "the seam re-announces on every running-set change; the flag must not reset to false"
    );
  }

  #[test]
  fn an_active_change_only_reaches_the_extension_it_names() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");
    let mut clock = attach(&mut supervisor, CLOCK, "clock");

    supervisor.device_connected(
      "sn-1".to_owned(),
      "car thing".to_owned(),
      Vec::new(),
      vec![WEATHER, CLOCK],
    );
    let _ = heard(&mut weather);
    let _ = heard(&mut clock);
    supervisor.device_active("sn-1".to_owned(), WEATHER, true);

    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceActive {
        device: "sn-1".to_owned(),
        active: true,
      }]
    );
    assert!(
      heard(&mut clock).is_empty(),
      "one webapp going on screen is not news to another's extension"
    );
  }

  #[test]
  fn a_disconnect_only_goes_out_for_a_device_the_host_announced() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");

    supervisor.device_disconnected("never-seen");
    assert!(heard(&mut weather).is_empty());

    supervisor.device_connected("sn-1".to_owned(), "car thing".to_owned(), Vec::new(), vec![WEATHER]);
    let _ = heard(&mut weather);
    supervisor.device_disconnected("sn-1");

    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceDisconnected {
        device: "sn-1".to_owned()
      }]
    );
  }

  #[test]
  fn a_reset_setting_reaches_the_extension_as_null_and_leaves_the_snapshot() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");

    supervisor.device_connected("sn-1".to_owned(), "car thing".to_owned(), Vec::new(), vec![WEATHER]);
    let _ = heard(&mut weather);
    supervisor.config_changed("sn-1".to_owned(), WEATHER, "zip".to_owned(), Some("10001".to_owned()));
    supervisor.config_changed("sn-1".to_owned(), WEATHER, "zip".to_owned(), None);

    assert_eq!(
      heard(&mut weather),
      vec![
        HostMessage::ConfigChanged {
          device: "sn-1".to_owned(),
          key: "zip".to_owned(),
          value: Some("10001".to_owned()),
        },
        HostMessage::ConfigChanged {
          device: "sn-1".to_owned(),
          key: "zip".to_owned(),
          value: None,
        },
      ],
      "a delete is an absent value, never an empty string"
    );
    assert!(
      supervisor.devices["sn-1"].config[&WEATHER].is_empty(),
      "the reset key leaves the snapshot a newly spawned child is handed"
    );
  }

  #[test]
  fn a_forward_that_cannot_cross_is_dropped_rather_than_delivered() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");
    supervisor.device_connected("sn-1".to_owned(), "car thing".to_owned(), Vec::new(), vec![WEATHER]);
    let _ = heard(&mut weather);

    supervisor.deliver(
      "sn-1".to_owned(),
      WEATHER,
      ExtensionMessage::Json { json: "{".to_owned() },
    );
    assert!(
      heard(&mut weather).is_empty(),
      "a malformed document never reaches stdio"
    );

    supervisor.deliver(
      "sn-1".to_owned(),
      WEATHER,
      ExtensionMessage::Text {
        text: "ping".to_owned(),
      },
    );
    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceMessage {
        device: "sn-1".to_owned(),
        message: WireForward::Text {
          data: "ping".to_owned()
        },
      }]
    );
  }

  #[test]
  fn a_forward_for_a_device_an_extension_was_never_told_about_does_not_reach_it() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");

    supervisor.deliver(
      "sn-1".to_owned(),
      WEATHER,
      ExtensionMessage::Text {
        text: "early".to_owned(),
      },
    );
    assert!(
      heard(&mut weather).is_empty(),
      "the daemon derives forward from the running set, so a forward can arrive before the announce \
       that names the device it came from"
    );

    supervisor.device_connected("sn-1".to_owned(), "car thing".to_owned(), Vec::new(), vec![WEATHER]);
    let _ = heard(&mut weather);
    supervisor.deliver(
      "sn-1".to_owned(),
      WEATHER,
      ExtensionMessage::Text { text: "now".to_owned() },
    );

    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceMessage {
        device: "sn-1".to_owned(),
        message: WireForward::Text { data: "now".to_owned() },
      }],
      "and once the device has been announced the forwards it sends land"
    );
  }

  #[test]
  fn an_active_flag_that_did_not_move_is_not_a_lifecycle_event() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");

    supervisor.device_active("sn-1".to_owned(), WEATHER, true);
    supervisor.device_connected("sn-1".to_owned(), "car thing".to_owned(), Vec::new(), vec![WEATHER]);
    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceConnected {
        device: "sn-1".to_owned(),
        name: "car thing".to_owned(),
        config: BTreeMap::new(),
        active: true,
      }],
      "the flag the seam sent ahead of the connect is folded into it"
    );

    supervisor.device_active("sn-1".to_owned(), WEATHER, true);

    assert!(
      heard(&mut weather).is_empty(),
      "the seam re-announces on every running-set change, and an extension told the same flag twice \
       re-runs work the first one already made it do"
    );
  }

  #[test]
  fn an_authorization_answer_only_reaches_the_extension_that_asked() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");
    let mut clock = attach(&mut supervisor, CLOCK, "clock");
    let held = Waiting {
      webapp: WEATHER,
      generation: 0,
      id: "7".to_owned(),
    };
    supervisor.waiting.insert(held.clone(), 1);

    supervisor.settle(
      Waiting {
        id: "9".to_owned(),
        ..held.clone()
      },
      Ok("bridgething://oauth/callback?code=x".to_owned()),
    );
    supervisor.settle(held, Ok("bridgething://oauth/callback?code=x".to_owned()));

    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::answer(
        "7".to_owned(),
        serde_json::Value::String("bridgething://oauth/callback?code=x".to_owned()),
      )],
      "the callback carries a token, so an unknown id must answer nobody"
    );
    assert!(heard(&mut clock).is_empty());
  }

  #[test]
  fn the_running_set_is_published_once_per_change_not_once_per_status_write() {
    let (mut supervisor, _pending) = probe();
    let _weather = attach(&mut supervisor, WEATHER, "weather");
    let _clock = attach(&mut supervisor, CLOCK, "clock");
    let (inbox, mut outbound) = ExtensionHostInbox::channel();
    supervisor.inbox = Some(inbox);
    supervisor.published = BTreeSet::from([WEATHER, CLOCK]);

    supervisor.announce_running();
    assert!(
      outbound.try_recv().is_err(),
      "an unchanged set must not churn the daemon's forward availability"
    );

    supervisor.mark(
      CLOCK,
      ExtensionStatus::Crashed {
        reason: "exited 1".to_owned(),
      },
    );
    supervisor.mark(
      CLOCK,
      ExtensionStatus::Crashed {
        reason: "exited 1".to_owned(),
      },
    );

    assert_eq!(
      outbound.try_recv(),
      Ok(ExtensionOutbound::RunningChanged { webapps: vec![WEATHER] })
    );
    assert!(
      outbound.try_recv().is_err(),
      "writing the same status again says nothing"
    );
  }

  #[tokio::test]
  async fn a_non_web_authorization_url_never_reaches_the_opener() {
    let (mut supervisor, _pending, opened) = probe_opening();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");

    for hostile in ["file:///Users/joey/.ssh/", "myapp://x", "javascript:alert(1)"] {
      supervisor.authorize(WEATHER, "1".to_owned(), hostile.to_owned());
    }

    assert!(
      opened.urls().is_empty(),
      "the opener hands a url to the OS, so a custom scheme would escape the deno sandbox"
    );
    assert!(
      supervisor.deps.authorize.claim().is_ok(),
      "a refused url must not have taken the one browser slot"
    );
    let refusals = heard(&mut weather);
    assert_eq!(refusals.len(), 3);
    for refusal in refusals {
      let HostMessage::Reply { ok, error, .. } = refusal else {
        panic!("the extension was not answered")
      };
      assert!(!ok);
      assert!(
        error.as_deref().is_some_and(|error| error.starts_with("unsupported:")),
        "the sdk types the leading token, and authorize has no invalid_url kind"
      );
    }
  }

  #[tokio::test]
  async fn a_web_authorization_url_opens_and_claims_the_slot() {
    let (mut supervisor, _pending, opened) = probe_opening();
    let _weather = attach(&mut supervisor, WEATHER, "weather");

    supervisor.authorize(
      WEATHER,
      "1".to_owned(),
      "https://discord.com/oauth2/authorize".to_owned(),
    );

    assert_eq!(opened.urls(), vec!["https://discord.com/oauth2/authorize".to_owned()]);
    assert!(
      matches!(
        supervisor.deps.authorize.claim(),
        Err(crate::settings::SettingsError::Busy)
      ),
      "the slot is held until the callback lands or the request is abandoned"
    );
  }

  #[tokio::test]
  async fn a_browser_that_will_not_open_answers_with_a_kind_the_sdk_carries() {
    let (mut supervisor, _pending) = probe_with(Arc::new(|_| Err("no browser on this box".to_owned())));
    let mut weather = attach(&mut supervisor, WEATHER, "weather");

    supervisor.authorize(
      WEATHER,
      "1".to_owned(),
      "https://discord.com/oauth2/authorize".to_owned(),
    );

    let refusals = heard(&mut weather);
    assert_eq!(refusals.len(), 1);
    let HostMessage::Reply { ok, error, .. } = &refusals[0] else {
      panic!("the extension was not answered")
    };
    assert!(!ok);
    assert!(
      error.as_deref().is_some_and(|error| error.starts_with("unsupported:")),
      "the settings page and the extension host share one flow, so they share its error kinds: {error:?}"
    );
    assert!(
      supervisor.deps.authorize.claim().is_ok(),
      "a browser that never opened must not keep the one slot"
    );
  }

  #[tokio::test]
  async fn a_child_that_dies_takes_its_pending_authorization_and_the_browser_slot_with_it() {
    let (mut supervisor, _pending, _opened) = probe_opening();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");

    supervisor.authorize(
      WEATHER,
      "1".to_owned(),
      "https://discord.com/oauth2/authorize".to_owned(),
    );
    let _ = heard(&mut weather);
    supervisor.exited(WEATHER, 0, "exited 1".to_owned());

    assert!(
      supervisor.deps.authorize.claim().is_ok(),
      "a crashed extension must not wedge the slot the settings page shares for five minutes"
    );

    let restarted = attach(&mut supervisor, WEATHER, "weather");
    supervisor.settle(
      Waiting {
        webapp: WEATHER,
        generation: 0,
        id: "1".to_owned(),
      },
      Ok("bridgething://oauth/callback?code=secret".to_owned()),
    );

    let mut restarted = restarted;
    assert!(
      heard(&mut restarted).is_empty(),
      "request ids restart at 1 on every spawn, so a stale callback must not resolve the new child's first request"
    );
  }

  #[tokio::test]
  async fn a_refused_extension_replaces_whatever_was_running_with_a_row_that_says_why() {
    let (mut supervisor, _pending) = probe();
    let _weather = attach(&mut supervisor, WEATHER, "weather");
    supervisor.publish();
    assert_eq!(supervisor.snapshot.lock().unwrap().len(), 1);

    supervisor
      .refused(Refusal {
        webapp: WEATHER,
        name: "weather".to_owned(),
        version: "2.0.0".to_owned(),
        permissions: descriptors(&["all"]),
        api: EXTENSION_API_VERSION,
        reason: "the bundle asks for all, which the install did not offer".to_owned(),
      })
      .await;

    assert!(
      !supervisor.children.contains_key(&WEATHER),
      "the code on disk is no longer what the app ships, so nothing may keep running it"
    );
    let held = supervisor.snapshot.lock().unwrap().clone();
    assert_eq!(
      held.len(),
      1,
      "the app still gets a row; silence would read as no extension"
    );
    assert_eq!(held[0].id, WEATHER.to_string());
    assert_eq!(held[0].version, "2.0.0");
    assert_eq!(held[0].permissions, vec!["all".to_owned()]);
    assert!(!held[0].enabled, "there is no toggle that could make a refusal run");
    assert_eq!(
      held[0].status,
      ExtensionStatus::Refused {
        reason: "the bundle asks for all, which the install did not offer".to_owned()
      }
    );
    assert!(
      !supervisor.running_set().contains(&WEATHER),
      "the daemon must not be told a forward surface is live"
    );
  }

  #[tokio::test]
  async fn a_real_uninstall_clears_the_refusal_row_with_the_app() {
    let (mut supervisor, _pending) = probe();
    supervisor
      .refused(Refusal {
        webapp: WEATHER,
        name: "weather".to_owned(),
        version: "2.0.0".to_owned(),
        permissions: Vec::new(),
        api: EXTENSION_API_VERSION,
        reason: "never asked".to_owned(),
      })
      .await;
    assert_eq!(supervisor.snapshot.lock().unwrap().len(), 1);

    supervisor.uninstalled(WEATHER).await;

    assert!(
      supervisor.snapshot.lock().unwrap().is_empty(),
      "an app that is gone leaves no row behind to explain an extension nobody has"
    );
  }

  #[test]
  fn a_running_extension_is_not_told_a_device_connected_twice() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");
    let mut clock = attach(&mut supervisor, CLOCK, "clock");

    supervisor.device_connected(
      "sn-1".to_owned(),
      "car thing".to_owned(),
      vec![setting(WEATHER, "zip", "10001"), setting(CLOCK, "chime", "on")],
      vec![WEATHER, CLOCK],
    );
    assert_eq!(heard(&mut weather).len(), 1);
    assert_eq!(heard(&mut clock).len(), 1);

    supervisor.device_connected(
      "sn-1".to_owned(),
      "car thing".to_owned(),
      vec![setting(CLOCK, "chime", "on")],
      vec![CLOCK],
    );

    assert!(
      heard(&mut weather).is_empty(),
      "a running-set change is not a reconnect for an extension that never lost the device"
    );
    assert!(
      heard(&mut clock).is_empty(),
      "even the named extension hears nothing when the announce says what it already knows"
    );
    assert_eq!(
      supervisor.devices["sn-1"].config[&WEATHER],
      BTreeMap::from([("zip".to_owned(), "10001".to_owned())]),
      "a partial announce carries only the started extension's settings and must not drop the rest"
    );
  }

  fn starting(supervisor: &mut Supervisor, webapp: Uuid) {
    supervisor.children.get_mut(&webapp).expect("an attached child").status = ExtensionStatus::Starting;
  }

  #[test]
  fn an_extension_still_coming_up_hears_a_device_once_and_with_its_settings() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");
    starting(&mut supervisor, WEATHER);

    supervisor.device_connected("sn-1".to_owned(), "car thing".to_owned(), Vec::new(), Vec::new());
    supervisor.announce_devices(WEATHER);
    assert!(
      heard(&mut weather).is_empty(),
      "the link-up announce names what it read settings for, and an extension still starting is not in \
       it; announcing anyway makes its first connect a lie about its own config"
    );

    supervisor.children.get_mut(&WEATHER).expect("the child").status = ExtensionStatus::Running;
    supervisor.device_connected(
      "sn-1".to_owned(),
      "car thing".to_owned(),
      vec![setting(WEATHER, "zip", "10001")],
      vec![WEATHER],
    );

    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceConnected {
        device: "sn-1".to_owned(),
        name: "car thing".to_owned(),
        config: BTreeMap::from([("zip".to_owned(), "10001".to_owned())]),
        active: false,
      }],
      "one connect per link, and the one the child sees carries the settings its connect-time work needs"
    );
  }

  #[test]
  fn an_extension_that_said_ready_while_the_link_was_being_read_still_hears_one_connect() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");
    let mut clock = attach(&mut supervisor, CLOCK, "clock");
    starting(&mut supervisor, WEATHER);

    supervisor.children.get_mut(&WEATHER).expect("the child").status = ExtensionStatus::Running;
    supervisor.device_connected(
      "sn-1".to_owned(),
      "car thing".to_owned(),
      vec![setting(CLOCK, "chime", "on")],
      vec![CLOCK],
    );

    assert!(
      heard(&mut weather).is_empty(),
      "the seam read this link's settings while weather was still starting, so this announce knows \
       nothing about weather however long ago it said ready"
    );
    assert_eq!(
      heard(&mut clock).len(),
      1,
      "the extension it did read settings for hears it"
    );

    supervisor.device_connected(
      "sn-1".to_owned(),
      "car thing".to_owned(),
      vec![setting(WEATHER, "zip", "10001")],
      vec![WEATHER],
    );

    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceConnected {
        device: "sn-1".to_owned(),
        name: "car thing".to_owned(),
        config: BTreeMap::from([("zip".to_owned(), "10001".to_owned())]),
        active: false,
      }],
      "and the announce that did read them is its first and only connect, not a second one after an \
       empty first"
    );
  }

  #[test]
  fn a_device_a_starting_extension_was_not_told_about_reaches_it_in_the_connect_that_follows() {
    let (mut supervisor, _pending) = probe();
    let mut weather = attach(&mut supervisor, WEATHER, "weather");
    starting(&mut supervisor, WEATHER);

    supervisor.device_connected("sn-1".to_owned(), "car thing".to_owned(), Vec::new(), Vec::new());
    supervisor.device_active("sn-1".to_owned(), WEATHER, true);
    supervisor.config_changed("sn-1".to_owned(), WEATHER, "zip".to_owned(), Some("10001".to_owned()));
    assert!(
      heard(&mut weather).is_empty(),
      "a device the child has not been told about has no state for it to update"
    );

    supervisor.children.get_mut(&WEATHER).expect("the child").status = ExtensionStatus::Running;
    supervisor.device_connected(
      "sn-1".to_owned(),
      "car thing".to_owned(),
      vec![setting(WEATHER, "zip", "10001")],
      vec![WEATHER],
    );

    assert_eq!(
      heard(&mut weather),
      vec![HostMessage::DeviceConnected {
        device: "sn-1".to_owned(),
        name: "car thing".to_owned(),
        config: BTreeMap::from([("zip".to_owned(), "10001".to_owned())]),
        active: true,
      }],
      "nothing is lost by waiting: the connect carries every flag and setting learned while it was starting"
    );
  }

  #[test]
  fn a_record_every_daemon_stopped_reporting_is_marked_orphaned_for_the_window() {
    let (mut supervisor, _pending) = probe();
    let _weather = attach(&mut supervisor, WEATHER, "weather");

    supervisor.claims(WEATHER, BTreeSet::from(["sn-1".to_owned()]));
    assert!(
      !supervisor.snapshot.lock().unwrap()[0].orphaned,
      "a Car Thing holds the app, so the app detail screen can reach this row"
    );

    supervisor.claims(WEATHER, BTreeSet::new());
    assert!(
      supervisor.snapshot.lock().unwrap()[0].orphaned,
      "no device reports the webapp any more, so the only way to the toggle is a row of its own"
    );
  }

  fn descriptors(raw: &[&str]) -> Vec<ExtensionPermission> {
    raw.iter().map(|raw| raw.parse().expect("a descriptor")).collect()
  }

  #[test]
  fn a_tilde_scope_is_expanded_against_the_host_home_before_it_becomes_a_flag() {
    let home = PathBuf::from("/home/joey");
    let permissions = descriptors(&["read:~/Music", "write:~", "net:~.example", "read:/tmp"]);

    assert_eq!(
      deno_args(&permissions, Some(&home)),
      vec![
        "run",
        "--no-prompt",
        "--allow-net=~.example",
        "--allow-read=/home/joey/Music,/tmp",
        "--allow-write=/home/joey"
      ],
      "there is no shell between the host and deno, so an unexpanded ~ resolves against the data directory instead"
    );
    assert_eq!(
      deno_args(&permissions, None),
      vec![
        "run",
        "--no-prompt",
        "--allow-net=~.example",
        "--allow-read=~/Music,/tmp",
        "--allow-write=~"
      ],
      "a host with no home directory leaves the descriptors alone rather than guessing"
    );
  }

  #[test]
  fn expansion_happens_before_the_argv_is_folded_not_after() {
    let home = PathBuf::from("/home/joey");
    assert_eq!(
      deno_args(&descriptors(&["all", "read:~/Music"]), Some(&home)),
      vec!["run", "--no-prompt", "--allow-all"],
      "`all` still collapses the whole argv"
    );
    assert_eq!(
      deno_args(&[], Some(&home)),
      vec!["run", "--no-prompt"],
      "an extension that asks for nothing gets nothing"
    );
  }

  #[test]
  fn backoff_climbs_from_a_second_and_settles_at_a_minute() {
    assert_eq!(backoff(1), Duration::from_secs(1));
    assert_eq!(backoff(2), Duration::from_secs(2));
    assert_eq!(backoff(3), Duration::from_secs(4));
    assert_eq!(backoff(7), Duration::from_secs(64).min(BACKOFF_CEILING));
    assert_eq!(backoff(30), BACKOFF_CEILING, "a wedged extension never spins the cpu");
  }

  #[test]
  fn a_first_failure_still_waits_before_the_retry() {
    assert!(backoff(1) >= BACKOFF_BASE, "an instant respawn would be a hot loop");
  }
}
