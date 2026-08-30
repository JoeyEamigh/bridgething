use std::{
  io::Write as _,
  net::ToSocketAddrs as _,
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::backend::{
  ExtensionConfigEntry, ExtensionHost, ExtensionHostInbox, ExtensionMessage, ExtensionOutbound,
};
use bridgething_desktop::{
  extensions::{Deps, ExtensionStatus, Extensions, runtime::DENO_VERSION},
  hints::{Hint, HintSink},
  settings::Authorize,
};
use bridgething_io::{HttpDownloadSink, HttpExecutor, HttpRequest, HttpSink, HttpTransport};
use libbridgething::ExtensionPermission;
use tracing::field::{Field, Visit};
use tracing_subscriber::{
  layer::{Context, Layer, SubscriberExt as _},
  util::SubscriberInitExt as _,
};
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);
const REQUIRE_DENO: &str = "BRIDGETHING_REQUIRE_DENO";
const REGISTRY: &str = "registry.npmjs.org:443";
const REACH: Duration = Duration::from_secs(3);

const FIXTURE: &str = r#"
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const pending = new Map();
let next = 1;
let dataDir = '';
let config = {};

function emit(message) {
  return Deno.stdout.write(encoder.encode(`${JSON.stringify(message)}\n`));
}

function request(build) {
  const id = String(next++);
  return new Promise(resolve => {
    pending.set(id, resolve);
    void emit(build(id));
  });
}

async function handle(message) {
  if (message.t === 'hello') {
    dataDir = message.dataDir;
    console.log('this line is not protocol');
    await emit({ t: 'log', level: 'info', message: `awake as ${message.webapp.name}` });
    await request(id => ({ t: 'kv.set', id, key: 'seen', value: { count: 1 } }));
    await emit({ t: 'ready' });
    return;
  }
  if (message.t === 'reply') {
    const resolve = pending.get(message.id);
    pending.delete(message.id);
    if (resolve) resolve(message.ok ? message.value : null);
    return;
  }
  if (message.t === 'device.connected') {
    config = message.config;
    return;
  }
  if (message.t === 'device.message') {
    const held = await request(id => ({ t: 'kv.get', id, key: 'seen' }));
    const keys = await request(id => ({ t: 'kv.list', id }));
    await emit({
      t: 'device.send',
      device: message.device,
      message: { encoding: 'json', data: { echo: message.message.data, held, keys, config } },
    });
    return;
  }
  if (message.t === 'stop') {
    await Deno.writeTextFile(`${dataDir}/stopped`, 'graceful');
    Deno.exit(0);
  }
}

let buffered = '';
for await (const chunk of Deno.stdin.readable) {
  buffered += decoder.decode(chunk, { stream: true });
  for (let cut = buffered.indexOf('\n'); cut >= 0; cut = buffered.indexOf('\n')) {
    const line = buffered.slice(0, cut);
    buffered = buffered.slice(cut + 1);
    if (line.trim().length > 0) void handle(JSON.parse(line));
  }
}
"#;

const NPM_FIXTURE: &str = r#"
import isNumber from 'npm:is-number@7.0.0';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function emit(message) {
  return Deno.stdout.write(encoder.encode(`${JSON.stringify(message)}\n`));
}

let buffered = '';
for await (const chunk of Deno.stdin.readable) {
  buffered += decoder.decode(chunk, { stream: true });
  for (let cut = buffered.indexOf('\n'); cut >= 0; cut = buffered.indexOf('\n')) {
    const line = buffered.slice(0, cut);
    buffered = buffered.slice(cut + 1);
    if (line.trim().length === 0) continue;
    const message = JSON.parse(line);
    if (message.t === 'hello') {
      await emit({ t: 'log', level: 'info', message: `npm resolved: ${isNumber(7)}` });
      await emit({ t: 'ready' });
    }
    if (message.t === 'stop') Deno.exit(0);
  }
}
"#;

struct Offline;

impl HttpTransport for Offline {
  fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
    sink.fail("this test never reaches the network".to_owned());
  }

  fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    sink.on_failed("this test never reaches the network".to_owned());
  }
}

struct Deaf;

impl HintSink for Deaf {
  fn emit(&self, _hint: Hint) {}
}

#[derive(Default, Clone)]
struct Tapped(Arc<Mutex<Vec<String>>>);

impl Tapped {
  fn lines(&self) -> Vec<String> {
    self.0.lock().unwrap().clone()
  }
}

impl<S: tracing::Subscriber> Layer<S> for Tapped {
  fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
    let mut rendered = Rendered(String::new());
    event.record(&mut rendered);
    if !rendered.0.is_empty() {
      self.0.lock().unwrap().push(rendered.0);
    }
  }
}

struct Rendered(String);

impl Visit for Rendered {
  fn record_str(&mut self, field: &Field, value: &str) {
    if field.name() == "message" {
      self.0.push_str(value);
    }
  }

  fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
    if field.name() == "message" {
      self.0.push_str(&format!("{value:?}"));
    }
  }
}

fn deno() -> Option<PathBuf> {
  let found = std::process::Command::new("which").arg("deno").output().ok()?;
  if !found.status.success() {
    return None;
  }
  let path = PathBuf::from(String::from_utf8_lossy(&found.stdout).trim());
  path.is_file().then_some(path)
}

fn plant(state_dir: &Path, deno: &Path) {
  let home = state_dir.join("runtime").join(format!("deno-{DENO_VERSION}"));
  std::fs::create_dir_all(&home).expect("the runtime home");
  let binary = home.join("deno");
  #[cfg(unix)]
  std::os::unix::fs::symlink(deno, &binary).expect("the local deno stands in for the pinned one");
  #[cfg(not(unix))]
  std::fs::copy(deno, &binary).expect("the local deno stands in for the pinned one");
}

fn bundle(dir: &Path, webapp: Uuid) -> PathBuf {
  written(dir, "fixture.zip", webapp, "echo", "write", FIXTURE)
}

fn written(dir: &Path, name: &str, webapp: Uuid, app: &str, permission: &str, entry: &str) -> PathBuf {
  let path = dir.join(name);
  let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).expect("an archive"));
  let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
  zip.start_file("manifest.json", options).expect("a manifest entry");
  zip
    .write_all(
      format!(
        r#"{{"id":"{webapp}","name":"{app}","version":"1.0.0","config":[],"permissions":[],"extension":{{"entry":"extension/desktop.mjs","permissions":["{permission}"],"api":1}}}}"#
      )
      .as_bytes(),
    )
    .expect("the manifest body");
  zip.start_file("extension/desktop.mjs", options).expect("an entry");
  zip.write_all(entry.as_bytes()).expect("the entry body");
  zip.finish().expect("a finished archive");
  path
}

fn deno_or_skip(lane: &str) -> Option<PathBuf> {
  if let Some(deno) = deno() {
    return Some(deno);
  }
  assert!(
    std::env::var_os(REQUIRE_DENO).is_none(),
    "{REQUIRE_DENO} is set and no deno is on PATH: {lane} drives a real sidecar, and skipping it \
     reports ok having spawned nothing"
  );
  eprintln!(
    "skipping {lane}: no deno on PATH, so there is no runtime to spawn an extension under. \
     install deno, or set {REQUIRE_DENO}=1 to make its absence a failure."
  );
  None
}

fn registry_or_skip(lane: &str) -> bool {
  if bounded(REACH, || {
    REGISTRY
      .to_socket_addrs()
      .into_iter()
      .flatten()
      .next()
      .is_some_and(|address| std::net::TcpStream::connect_timeout(&address, REACH).is_ok())
  }) {
    return true;
  }
  assert!(
    std::env::var_os(REQUIRE_DENO).is_none(),
    "{REQUIRE_DENO} is set and {REGISTRY} does not answer: {lane} resolves an npm specifier through it, \
     and skipping it reports ok having resolved nothing"
  );
  eprintln!(
    "skipping {lane}: {REGISTRY} does not answer, so the child cannot resolve the npm specifier this \
     lane is about. connect this machine to a network, or set {REQUIRE_DENO}=1 to make the registry \
     being unreachable a failure."
  );
  false
}

fn bounded(budget: Duration, probe: impl FnOnce() -> bool + Send + 'static) -> bool {
  let (answered, waiting) = std::sync::mpsc::channel();
  std::thread::spawn(move || {
    let _ = answered.send(probe());
  });
  waiting.recv_timeout(budget).unwrap_or(false)
}

async fn ran(extensions: &Extensions) -> ExtensionStatus {
  let deadline = tokio::time::Instant::now() + DEADLINE;
  loop {
    let seen = extensions
      .list()
      .first()
      .map(|entry| entry.status.clone())
      .unwrap_or(ExtensionStatus::Stopped);
    let done = matches!(seen, ExtensionStatus::Running | ExtensionStatus::Crashed { .. });
    if done || tokio::time::Instant::now() > deadline {
      return seen;
    }
    tokio::time::sleep(POLL).await;
  }
}

async fn settled(extensions: &Extensions, wanted: ExtensionStatus) -> ExtensionStatus {
  let deadline = tokio::time::Instant::now() + DEADLINE;
  loop {
    let seen = extensions
      .list()
      .first()
      .map(|entry| entry.status.clone())
      .unwrap_or(ExtensionStatus::Stopped);
    if seen == wanted || tokio::time::Instant::now() > deadline {
      return seen;
    }
    tokio::time::sleep(POLL).await;
  }
}

#[test]
fn a_probe_that_never_answers_gives_up_inside_its_budget() {
  let started = std::time::Instant::now();

  let answered = bounded(Duration::from_millis(50), || {
    std::thread::sleep(Duration::from_secs(30));
    true
  });

  assert!(!answered, "nothing answered, so nothing is reachable");
  assert!(
    started.elapsed() < Duration::from_secs(5),
    "a probe with no budget of its own holds the whole lane for as long as the network feels like, took {:?}",
    started.elapsed()
  );
}

#[test]
fn the_supervisor_comes_up_from_a_caller_that_never_entered_a_tokio_runtime() {
  assert!(
    tokio::runtime::Handle::try_current().is_err(),
    "tauri calls setup on the main thread with no runtime entered, and that is the shape this pins"
  );

  let spool = tempfile::tempdir().expect("a scratch directory");
  let state_dir = spool.path().join("state");
  std::fs::create_dir_all(&state_dir).expect("the state directory");

  let webapp = Uuid::now_v7();
  let archive = bundle(spool.path(), webapp);

  let extensions = Extensions::init(&state_dir);
  extensions.adopt(Some("sn-1"), &archive, Some(&[ExtensionPermission::Write(None)]));
  extensions.spawn(Deps {
    http: HttpExecutor::new(Arc::new(Offline)),
    authorize: Arc::new(Authorize::default()),
    open_url: Arc::new(|_| Ok(())),
    hints: Arc::new(Deaf),
  });

  let deadline = std::time::Instant::now() + DEADLINE;
  while extensions.list().is_empty() && std::time::Instant::now() < deadline {
    std::thread::sleep(POLL);
  }
  assert_eq!(
    extensions.list().len(),
    1,
    "the supervisor never ran, so the whole shell dies at startup before the window appears"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_extension_resolves_npm_specifiers_from_a_data_dir_under_a_package_json() {
  let Some(deno) = deno_or_skip("the npm resolution lane") else {
    return;
  };
  if !registry_or_skip("the npm resolution lane") {
    return;
  }

  let spool = tempfile::tempdir().expect("a scratch directory");
  std::fs::write(
    spool.path().join("package.json"),
    r#"{"name":"whatever-happens-to-be-above","private":true}"#,
  )
  .expect("the ancestor manifest");
  let state_dir = spool.path().join("state");
  std::fs::create_dir_all(&state_dir).expect("the state directory");
  plant(&state_dir, &deno);

  let webapp = Uuid::now_v7();
  let archive = written(spool.path(), "npm.zip", webapp, "npm", "all", NPM_FIXTURE);

  let extensions = Extensions::init(&state_dir);
  extensions.spawn(Deps {
    http: HttpExecutor::new(Arc::new(Offline)),
    authorize: Arc::new(Authorize::default()),
    open_url: Arc::new(|_| Ok(())),
    hints: Arc::new(Deaf),
  });
  extensions.adopt(Some("sn-1"), &archive, Some(&["all".parse().expect("a descriptor")]));

  assert_eq!(
    ran(&extensions).await,
    ExtensionStatus::Running,
    "an extension runs from its data directory, and where that lands is the user's home layout, not the \
     author's: with a package.json anywhere above it deno switches to node resolution and every npm: \
     specifier in a published bundle stops resolving"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_real_extension_talks_the_whole_protocol_and_stops_when_asked() {
  let Some(deno) = deno_or_skip("the sidecar contract lane") else {
    return;
  };

  let taps = Tapped::default();
  tracing_subscriber::registry().with(taps.clone()).init();

  let spool = tempfile::tempdir().expect("a scratch directory");
  let state_dir = spool.path().join("state");
  std::fs::create_dir_all(&state_dir).expect("the state directory");
  plant(&state_dir, &deno);

  let webapp = Uuid::now_v7();
  let archive = bundle(spool.path(), webapp);

  let extensions = Extensions::init(&state_dir);
  extensions.spawn(Deps {
    http: HttpExecutor::new(Arc::new(Offline)),
    authorize: Arc::new(Authorize::default()),
    open_url: Arc::new(|_| Ok(())),
    hints: Arc::new(Deaf),
  });

  let (inbox, mut outbound) = ExtensionHostInbox::channel();
  extensions.start(inbox);
  extensions.adopt(Some("sn-1"), &archive, Some(&[ExtensionPermission::Write(None)]));

  assert_eq!(
    settled(&extensions, ExtensionStatus::Running).await,
    ExtensionStatus::Running,
    "the fixture said ready, so the app row must say running"
  );
  assert_eq!(
    outbound.recv().await,
    Some(ExtensionOutbound::RunningChanged { webapps: Vec::new() }),
    "a host that attaches with nothing running says so, or the daemon keeps a stale forward surface"
  );
  assert_eq!(
    outbound.recv().await,
    Some(ExtensionOutbound::RunningChanged { webapps: vec![webapp] }),
    "the daemon learns the forward surface is live only from the running set"
  );

  extensions.device_connected(
    "sn-1".to_owned(),
    "car thing".to_owned(),
    vec![
      ExtensionConfigEntry {
        webapp: webapp.to_string(),
        key: "zip".to_owned(),
        value: "10001".to_owned(),
      },
      ExtensionConfigEntry {
        webapp: Uuid::now_v7().to_string(),
        key: "secret".to_owned(),
        value: "not yours".to_owned(),
      },
    ],
    vec![webapp.to_string()],
  );
  extensions.deliver(
    "sn-1".to_owned(),
    webapp.to_string(),
    ExtensionMessage::Json {
      json: r#"{"ping":1}"#.to_owned(),
    },
  );

  let echoed = tokio::time::timeout(DEADLINE, outbound.recv())
    .await
    .expect("the echo came back before the deadline")
    .expect("the host inbox stayed open");

  let ExtensionOutbound::SendToDevice {
    device,
    webapp: addressed,
    message,
  } = echoed
  else {
    panic!("the extension's forward arrived as {echoed:?}");
  };
  assert_eq!(device.as_deref(), Some("sn-1"));
  assert_eq!(addressed, webapp);
  assert_eq!(
    message,
    libbridgething::ForwardMessage::Json(serde_json::json!({
      "echo": { "ping": 1 },
      "held": { "count": 1 },
      "keys": ["seen"],
      "config": { "zip": "10001" },
    })),
    "the round trip covers the forward hop, both kv reads, and the per-webapp config projection"
  );

  let kv = std::fs::read_to_string(
    state_dir
      .join("extensions")
      .join(webapp.to_string())
      .join("data/kv.json"),
  )
  .expect("the kv file landed in the data directory");
  assert_eq!(kv, r#"{"seen":{"count":1}}"#, "kv.set writes through to disk");

  let lines = taps.lines();
  assert!(
    lines.iter().any(|line| line == "echo: awake as echo"),
    "a log message reaches the desktop log prefixed with the app name; saw {lines:?}"
  );
  assert!(
    lines.iter().any(|line| line == "echo: this line is not protocol"),
    "a plain stdout line is the extension's output, not a decode failure; saw {lines:?}"
  );

  extensions.stop();

  assert_eq!(
    settled(&extensions, ExtensionStatus::Stopped).await,
    ExtensionStatus::Stopped,
    "a stopped extension reads as stopped, never as crashed"
  );
  assert_eq!(
    std::fs::read_to_string(
      state_dir
        .join("extensions")
        .join(webapp.to_string())
        .join("data/stopped")
    )
    .expect("the fixture ran its shutdown before exiting"),
    "graceful",
    "the host asks before it signals"
  );
}
