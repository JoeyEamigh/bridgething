use std::{
  io::{Read, Write},
  net::TcpListener,
  path::{Path, PathBuf},
  sync::{Arc, Mutex, OnceLock, Weak},
  time::{Duration, Instant},
};

use bridgething_companion::provider::ResumeTarget;
use bridgething_delivery::discovery::Discovery;
use bridgething_desktop::{
  commands::{self, InstallOutcome, OtaOutcome},
  hints::{self, Hint, HintSink, Invalidation},
  shell::{DEFAULT_GATEWAY_URL, DesktopPaths, Shell, ShellConfig},
};
use libbridgething::{BRIDGETHING_MDNS_SERVICE_TYPE, BRIDGETHING_STOCK_WS_PORT, gateway::WebappResourceKind};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use tauri::{
  Manager,
  test::{MockRuntime, mock_builder, mock_context, noop_assets},
};
use tokio::sync::mpsc;
use uuid::Uuid;

const ARTIFACT_BYTES: usize = 512 * 1024;

const DRIVE_DEADLINE: Duration = Duration::from_secs(120);
const SETTLE: Duration = Duration::from_secs(15);

const ICON: &[u8] = b"\x89PNG\r\n\x1a\n-- not a real png, but it is bytes with a digest --";

struct Channel {
  tx: mpsc::UnboundedSender<Hint>,
}

impl HintSink for Channel {
  fn emit(&self, hint: Hint) {
    let _ = self.tx.send(hint);
  }
}

struct Heard {
  rx: Mutex<mpsc::UnboundedReceiver<Hint>>,
  seen: Mutex<Vec<Hint>>,
}

impl Heard {
  fn drain(&self) {
    let mut rx = self.rx.lock().unwrap();
    let mut seen = self.seen.lock().unwrap();
    while let Ok(hint) = rx.try_recv() {
      seen.push(hint);
    }
  }

  fn saw(&self, name: &str) -> bool {
    self.drain();
    self.seen.lock().unwrap().iter().any(|hint| hint.name == name)
  }

  fn ids_for(&self, name: &str) -> Vec<Option<String>> {
    self.drain();
    self
      .seen
      .lock()
      .unwrap()
      .iter()
      .filter(|hint| hint.name == name)
      .map(|hint| hint.id.clone())
      .collect()
  }

  fn all(&self) -> Vec<Hint> {
    self.drain();
    self.seen.lock().unwrap().clone()
  }

  async fn wait(&self, name: &str, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
      if self.saw(name) {
        return true;
      }
      if tokio::time::Instant::now() >= deadline {
        return false;
      }
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
  }
}

fn stock_url_for(gateway_url: &str) -> String {
  let rest = gateway_url.split_once("://").map_or(gateway_url, |(_, rest)| rest);
  let authority = rest.split('/').next().unwrap_or(rest);
  let host = match authority.rsplit_once(':') {
    Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
    _ => authority,
  };
  format!("ws://{host}:{BRIDGETHING_STOCK_WS_PORT}/")
}

fn ask_stock_onboarding(stock_url: &str) -> Option<String> {
  use tokio_tungstenite::tungstenite::{Message, connect, stream::MaybeTlsStream};

  let (mut stock, _) = connect(stock_url).expect("the stock websocket accepts a client");
  if let MaybeTlsStream::Plain(stream) = stock.get_mut() {
    stream
      .set_read_timeout(Some(SETTLE))
      .expect("the stock read has a deadline");
  }
  stock
    .send(Message::text(
      r#"{"type":"settings","action":"get","value_type":"string","key":"onboarding_status"}"#,
    ))
    .expect("the stock request goes out");

  while let Ok(message) = stock.read() {
    let Ok(text) = message.to_text() else { continue };
    let Ok(frame) = serde_json::from_str::<serde_json::Value>(text) else {
      continue;
    };
    if frame["type"] == "settings_response" && frame["payload"]["key"] == "onboarding_status" {
      return frame["payload"]["value"].as_str().map(str::to_owned);
    }
  }
  None
}

fn await_stock_onboarding(stock_url: &str) -> Option<String> {
  let deadline = Instant::now() + SETTLE;
  loop {
    let answer = ask_stock_onboarding(stock_url);
    if answer.is_some() || Instant::now() >= deadline {
      return answer;
    }
    std::thread::sleep(Duration::from_millis(50));
  }
}

enum Daemon {
  Borrowed,
  Owned,
  Remote(String),
}

static SHARED: Mutex<Option<Weak<Daemon>>> = Mutex::new(None);

impl Daemon {
  fn shared() -> Arc<Self> {
    let mut held = SHARED.lock().unwrap();
    if let Some(live) = held.as_ref().and_then(Weak::upgrade) {
      return live;
    }
    let fresh = Arc::new(Self::start());
    *held = Some(Arc::downgrade(&fresh));
    fresh
  }

  fn start() -> Self {
    if let Ok(url) = std::env::var("BRIDGETHING_GATEWAY_URL") {
      return Self::Remote(url);
    }
    if reachable(DEFAULT_GATEWAY_URL) {
      return Self::Borrowed;
    }
    assert!(
      supervise("start").success(),
      "the dev daemon did not come up; its log is .dev/dev-daemon.log"
    );
    Self::Owned
  }

  fn url(&self) -> String {
    match self {
      Self::Remote(url) => url.clone(),
      _ => DEFAULT_GATEWAY_URL.to_owned(),
    }
  }
}

impl Drop for Daemon {
  fn drop(&mut self) {
    if !matches!(self, Self::Owned) {
      return;
    }
    let _serialized = SHARED.lock().unwrap_or_else(|held| held.into_inner());
    supervise("stop");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && reachable(DEFAULT_GATEWAY_URL) {
      std::thread::sleep(Duration::from_millis(50));
    }
  }
}

fn reachable(url: &str) -> bool {
  let authority = url
    .trim_start_matches("ws://")
    .trim_start_matches("wss://")
    .trim_end_matches('/');
  let Ok(mut addrs) = std::net::ToSocketAddrs::to_socket_addrs(authority) else {
    return false;
  };
  addrs.any(|addr| std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok())
}

fn supervise(action: &str) -> std::process::ExitStatus {
  let root = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("../..")
    .canonicalize()
    .expect("the repo root resolves");
  std::process::Command::new(root.join("scripts/dev-daemon.sh"))
    .arg(action)
    .current_dir(&root)
    .status()
    .expect("the dev daemon script runs")
}

fn write_artifact(dir: &Path, name: &str, len: usize) -> PathBuf {
  let path = dir.join(name);
  let body: Vec<u8> = (0..len).map(|at| (at % 251) as u8).collect();
  std::fs::write(&path, body).expect("the artifact spools");
  path
}

fn write_bundle(dir: &Path, id: Uuid) -> PathBuf {
  let path = dir.join("bundle.zip");
  let mut bundle = zip::ZipWriter::new(std::fs::File::create(&path).expect("create the bundle"));
  let opts = zip::write::SimpleFileOptions::default();
  bundle.start_file("index.html", opts).expect("start index.html");
  bundle
    .write_all(b"<!doctype html><title>desktop shell</title>")
    .expect("write index");
  bundle.start_file("icon.png", opts).expect("start icon.png");
  bundle.write_all(ICON).expect("write icon");
  bundle.start_file("manifest.json", opts).expect("start manifest.json");
  bundle
    .write_all(
      format!(
        r#"{{"id":"{id}","name":"desktop shell","version":"0.1.0","icon":"icon.png","config":[],"permissions":[]}}"#
      )
      .as_bytes(),
    )
    .expect("write manifest");
  bundle.finish().expect("finish the bundle");
  path
}

fn write_extension_bundle(dir: &Path, id: Uuid) -> PathBuf {
  let path = dir.join("extension-bundle.zip");
  let mut bundle = zip::ZipWriter::new(std::fs::File::create(&path).expect("create the bundle"));
  let opts = zip::write::SimpleFileOptions::default();
  bundle.start_file("index.html", opts).expect("start index.html");
  bundle
    .write_all(b"<!doctype html><title>sidecar</title>")
    .expect("write index");
  bundle.start_file("icon.png", opts).expect("start icon.png");
  bundle.write_all(ICON).expect("write icon");
  bundle
    .start_file("extension/desktop.mjs", opts)
    .expect("start the extension");
  bundle.write_all(b"export {}").expect("write the extension");
  bundle.start_file("manifest.json", opts).expect("start manifest.json");
  bundle
    .write_all(
      format!(
        r#"{{"id":"{id}","name":"sidecar","version":"0.1.0","icon":"icon.png","config":[],"permissions":[],"extension":{{"entry":"extension/desktop.mjs","permissions":["all"],"api":1}}}}"#
      )
      .as_bytes(),
    )
    .expect("write manifest");
  bundle.finish().expect("finish the bundle");
  path
}

fn mock_app(shell: Arc<Shell>) -> tauri::App<MockRuntime> {
  mock_builder()
    .manage(Arc::clone(shell.extensions()))
    .manage(shell)
    .manage(Discovery::spawn(|_| ()).expect("the responder starts"))
    .invoke_handler(bridgething_desktop::desktop_commands!())
    .build(mock_context(noop_assets()))
    .expect("the shell's command surface builds without a window")
}

fn shell_config(url: impl Into<String>, spool: &Path) -> ShellConfig {
  model_root();
  ShellConfig::new(url, DesktopPaths::under(spool))
}

fn probe_shell(spool: &Path) -> Arc<Shell> {
  let (tx, _rx) = mpsc::unbounded_channel();
  Shell::create(shell_config(DEFAULT_GATEWAY_URL, spool), Arc::new(Channel { tx })).expect("the shell builds")
}

struct ModelRoot {
  url: String,
  asked: Arc<Mutex<Vec<String>>>,
}

static MODEL_ROOT: OnceLock<ModelRoot> = OnceLock::new();

fn model_root() -> &'static ModelRoot {
  MODEL_ROOT.get_or_init(|| {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let url = format!("http://{}", listener.local_addr().expect("the bound address"));
    let asked = Arc::new(Mutex::new(Vec::new()));

    let heard = Arc::clone(&asked);
    std::thread::spawn(move || {
      for stream in listener.incoming().flatten() {
        let mut stream = stream;
        let mut head = [0u8; 1024];
        let read = stream.read(&mut head).unwrap_or(0);
        if let Some(line) = String::from_utf8_lossy(&head[..read]).lines().next() {
          heard.lock().unwrap().push(line.to_owned());
        }
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
      }
    });

    // SAFETY: set once, before any test builds a shell, so no session reads it concurrently.
    unsafe { std::env::set_var("BRIDGETHING_MODEL_ROOT", &url) };
    ModelRoot { url, asked }
  })
}

fn announce(instance: &str, nickname: &str) -> ServiceDaemon {
  let registrar = ServiceDaemon::new().expect("the registrar starts");
  let info = ServiceInfo::new(
    &format!("{BRIDGETHING_MDNS_SERVICE_TYPE}.local."),
    instance,
    "bridgething-headless-probe.local.",
    "127.0.0.1",
    8892,
    &[("nickname", nickname)][..],
  )
  .expect("the announcement is well formed");
  registrar.register(info).expect("the announcement goes out");
  registrar
}

#[tokio::test(flavor = "multi_thread")]
async fn an_announcing_gateway_is_offered_to_the_window() {
  let instance = format!("headless-{}", std::process::id());
  let _registrar = announce(&instance, "Headless Probe");

  let spool = tempfile::tempdir().expect("a scratch directory");
  let app = mock_app(probe_shell(spool.path()));

  let deadline = tokio::time::Instant::now() + SETTLE;
  let offered = loop {
    let found = commands::endpoints(app.state()).await.expect("the browse answers");
    if let Some(offered) = found.into_iter().find(|found| found.id.starts_with(&instance)) {
      break offered;
    }
    assert!(
      tokio::time::Instant::now() < deadline,
      "the announcement never reached the window"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
  };

  assert_eq!(
    offered.url, "ws://bridgething-headless-probe.local:8892/",
    "the window is offered a dialable gateway url"
  );
  assert_eq!(
    offered.nickname.as_deref(),
    Some("Headless Probe"),
    "the window is offered the nickname to label the row with"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_is_told_which_gateway_a_bare_connect_would_reach() {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let app = mock_app(probe_shell(spool.path()));

  assert_eq!(
    commands::default_gateway(app.state()).await.expect("the fallback url"),
    DEFAULT_GATEWAY_URL,
    "the standing row dials whatever a bare connect would have reached"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_capability_with_no_backend_is_never_offered() {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let app = mock_app(probe_shell(spool.path()));

  let support = commands::capability_support(app.state())
    .await
    .expect("the window asks what this build can serve");
  let flags = commands::capabilities(app.state()).await.expect("capabilities");

  assert!(
    support.net_fetch && support.net_ws,
    "a desktop owns its own sockets on every platform"
  );
  #[cfg(target_os = "linux")]
  assert!(
    support.notifications,
    "a dbus monitor reads the notifications going to other apps, so the toggle is live"
  );
  #[cfg(not(target_os = "linux"))]
  assert!(
    !support.notifications,
    "nothing off linux can read the notifications going to other apps, so the toggle stays dark"
  );
  assert!(
    flags.geo <= support.geo && flags.audio_tts <= support.audio_tts && flags.voice_model <= support.voice_model,
    "a fresh install never turns on what this build cannot serve"
  );

  #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
  assert!(
    support.voice_model && flags.voice_model,
    "every desktop compiles in a model runner and whisper, so voice understanding is live out of the box"
  );
  #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
  assert!(
    !support.voice_model,
    "a build with no runner leaves the toggle dark rather than downloading for nobody"
  );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[tokio::test(flavor = "multi_thread")]
async fn the_model_root_override_is_honored_so_a_test_run_never_pulls_from_the_published_bucket() {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let shell = probe_shell(spool.path());

  shell.start().await;

  let root = model_root();
  let deadline = tokio::time::Instant::now() + SETTLE;
  loop {
    let asked = root.asked.lock().unwrap().clone();
    if asked.iter().any(|line| line.contains("/nlu/stable/manifest.json")) {
      assert!(
        asked.iter().all(|line| !line.contains("ota.bridgething.com")),
        "the stand-in was asked for the manifest by host, not by the published url: {asked:?}"
      );
      return;
    }
    assert!(
      tokio::time::Instant::now() < deadline,
      "a started session with voice models on never asked {} for a manifest; it would have reached the published bucket",
      root.url
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn dialing_a_gateway_that_is_not_listening_names_the_url_it_tried() {
  let spool = tempfile::tempdir().expect("a scratch directory");
  let shell = probe_shell(spool.path());
  shell.start().await;
  let app = mock_app(shell);

  let dead = "ws://127.0.0.1:1/".to_owned();
  let refused = commands::connect(app.state(), Some(dead.clone()))
    .await
    .expect_err("a gateway that is not listening cannot be adopted");

  let told = serde_json::to_string(&refused).expect("the refusal reaches the window as json");
  assert!(told.contains(&dead), "the refusal names the url it tried, got {told}");
  assert!(
    commands::peers(app.state()).await.expect("peers").is_empty(),
    "a refused dial leaves no half-open peer behind"
  );
  assert!(
    commands::known_devices(app.state())
      .await
      .expect("known devices")
      .is_empty(),
    "a gateway that never answered is never dialed unasked at the next launch"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_died_comes_back_on_its_own_and_one_the_user_dropped_waits_to_be_replugged() {
  let daemon = Daemon::shared();
  let url = daemon.url();

  let spool = tempfile::tempdir().expect("a scratch directory");
  let (tx, _rx) = mpsc::unbounded_channel();
  let shell =
    Shell::create(shell_config(url.clone(), spool.path()), Arc::new(Channel { tx })).expect("the shell builds");
  shell.start().await;

  let attached = Arc::new(Mutex::new(Vec::new()));
  let source = Arc::clone(&attached);
  bridgething_desktop::autoconnect::spawn(shell.clone(), move || source.lock().unwrap().clone());

  tokio::time::sleep(Duration::from_millis(500)).await;
  assert!(
    !shell.is_linked(&url),
    "a daemon that is not on the link is not dialed; there is nowhere for it to be"
  );

  let plug = || *attached.lock().unwrap() = vec![endpoint(&url, Some("Desk Thing"))];
  let unplug = || attached.lock().unwrap().clear();

  plug();
  shell.wake().notify_one();
  assert!(
    settles(|| shell.is_linked(&url)).await,
    "a daemon on the link is dialed with no user action at all"
  );

  shell.session().disconnect_direct(&url).await;
  assert!(
    !shell.is_linked(&url),
    "the link is gone the way a dead socket leaves it"
  );
  assert!(
    settles(|| shell.is_linked(&url)).await,
    "a device that dropped is dialed again on its own, or the tray app is dead weight after a daemon restart"
  );

  shell.disconnect(Some(url.clone())).await;
  tokio::time::sleep(Duration::from_millis(500)).await;
  assert!(
    !shell.is_linked(&url),
    "a link the user dropped stays dropped while the device is still attached"
  );

  unplug();
  shell.wake().notify_one();
  tokio::time::sleep(Duration::from_millis(500)).await;
  plug();
  shell.wake().notify_one();
  assert!(
    settles(|| shell.is_linked(&url)).await,
    "and dropping it means until it is unplugged and back, not forever"
  );

  let held = shell.known_devices();
  assert_eq!(held.len(), 1);
  shell.forget_device(&held[0].id);
  assert!(shell.known_devices().is_empty(), "a forgotten device is forgotten");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_shows_up_on_the_link_is_dialed_and_names_itself() {
  let daemon = Daemon::shared();
  let url = daemon.url();

  let spool = tempfile::tempdir().expect("a scratch directory");
  let (tx, _rx) = mpsc::unbounded_channel();
  let shell =
    Shell::create(shell_config(url.clone(), spool.path()), Arc::new(Channel { tx })).expect("the shell builds");
  shell.start().await;

  let announced = endpoint(&url, Some("Desk Thing"));
  bridgething_desktop::autoconnect::spawn(shell.clone(), move || vec![announced.clone()]);

  assert!(
    settles(|| shell.is_linked(&url)).await,
    "a daemon announcing itself is linked without a first manual connect"
  );
  assert!(
    settles(|| shell.known_devices().first().is_some_and(|known| known.id != url)).await,
    "and the daemon's first frame re-keys the row off the address it was dialed on"
  );

  let known = shell.known_devices();
  assert_eq!(known.len(), 1, "one device is one row, dial and adoption together");
  assert_eq!(known[0].name, "Desk Thing", "under the name it announced");
  assert_eq!(known[0].url, url, "with the address it answered on kept as an address");
  assert_eq!(
    known[0].id, "8558R481Q61R",
    "and keyed on the serial the daemon reports, so two daemons at one address are two rows"
  );

  shell.disconnect(Some(url.clone())).await;
  tokio::time::sleep(Duration::from_millis(500)).await;
  assert!(
    !shell.is_linked(&url),
    "dropping it wins over the standing announcement"
  );
}

fn endpoint(url: &str, nickname: Option<&str>) -> bridgething_delivery::discovery::Endpoint {
  bridgething_delivery::discovery::Endpoint {
    id: "probe._bridgething._tcp.local.".to_owned(),
    url: url.to_owned(),
    host: "127.0.0.1".to_owned(),
    nickname: nickname.map(str::to_owned),
    serial: None,
    browsed: true,
  }
}

fn holders(spool: &Path, webapp: Uuid) -> Vec<String> {
  let path = spool
    .join("state/extensions")
    .join(webapp.to_string())
    .join("extension.json");
  let raw = std::fs::read_to_string(&path).expect("the extension record");
  let held: serde_json::Value = serde_json::from_str(&raw).expect("the record parses");
  held["devices"]
    .as_array()
    .expect("a claim list")
    .iter()
    .filter_map(|device| device.as_str().map(str::to_owned))
    .collect()
}

async fn settles(holds: impl Fn() -> bool) -> bool {
  let deadline = tokio::time::Instant::now() + SETTLE;
  while !holds() {
    if tokio::time::Instant::now() >= deadline {
      return false;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
  }
  true
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shell_holds_a_live_session_and_every_command_is_a_pull() {
  let daemon = Daemon::shared();
  let url = daemon.url();

  let spool = tempfile::tempdir().expect("a scratch directory");
  let (tx, rx) = mpsc::unbounded_channel();
  let heard = Heard {
    rx: Mutex::new(rx),
    seen: Mutex::new(Vec::new()),
  };
  let shell =
    Shell::create(shell_config(url.clone(), spool.path()), Arc::new(Channel { tx })).expect("the shell builds");
  shell.start().await;

  let app = mock_app(shell);

  let device_id = commands::connect(app.state(), None)
    .await
    .expect("the daemon accepts a link");
  assert_eq!(device_id, url, "the link is named by the daemon it reached");
  assert!(
    heard.wait(hints::PEERS, SETTLE).await,
    "adopting a link invalidates the peers query"
  );
  assert_eq!(
    heard.ids_for(hints::PEERS).first().cloned().flatten(),
    Some(device_id.clone()),
    "a peer hint names the peer and nothing else"
  );

  let snapshot = commands::session_snapshot(app.state()).await.expect("a snapshot");
  assert_eq!(snapshot.peers.len(), 1, "the live link is a peer");
  assert_eq!(snapshot.peers[0].id, device_id);
  assert_eq!(snapshot.host_info.app_name, "bridgething desktop");
  assert!(
    snapshot.capability_flags.net_fetch && snapshot.capability_flags.net_ws,
    "a desktop owns its own sockets and says so"
  );

  let again = commands::session_snapshot(app.state())
    .await
    .expect("a second snapshot");
  assert_eq!(
    snapshot.peers, again.peers,
    "a pull is idempotent: two reads with nothing between them agree"
  );

  assert_eq!(
    commands::peers(app.state()).await.expect("peers"),
    snapshot.peers,
    "a projection answers the same thing the whole snapshot does"
  );
  assert_eq!(
    commands::host_info(app.state()).await.expect("host info"),
    snapshot.host_info
  );
  assert_eq!(
    commands::capabilities(app.state()).await.expect("capabilities"),
    snapshot.capability_flags
  );
  let providers = commands::providers(app.state()).await.expect("providers");
  assert_eq!(
    providers.is_empty(),
    option_env!("BRIDGETHING_AUTH_PSK").is_none_or(str::is_empty),
    "the shell registers exactly the providers its baked-in psk can reach, and nothing else"
  );
  assert!(commands::now_playing(app.state()).await.is_ok());
  assert!(commands::voice_model(app.state()).await.is_ok());
  assert!(commands::ota_available(app.state()).await.is_ok());
  assert!(commands::ota_poll(app.state()).await.is_ok());
  assert!(commands::provider_priority(app.state()).await.is_ok());
  assert!(commands::library_provider(app.state()).await.is_ok());

  assert!(
    heard.wait(hints::DEVICE_META, SETTLE).await,
    "the daemon says who it is and that invalidates the device-meta query"
  );
  let meta = commands::device_meta(app.state()).await.expect("device meta");
  assert_eq!(
    meta.len(),
    1,
    "the hint carried no metadata, so the pull is where it came from"
  );
  assert!(!meta[0].meta.daemon_version.is_empty());

  let artifact = write_artifact(spool.path(), "daemon", ARTIFACT_BYTES);
  let outcome = tokio::time::timeout(DRIVE_DEADLINE, commands::ota_push_daemon(app.state(), artifact))
    .await
    .expect("the drive ended rather than parking on the watchdog")
    .expect("the drive ran");
  assert_eq!(
    outcome,
    OtaOutcome::Completed,
    "the daemon staged the piece, took the activate and reported reboot"
  );
  assert!(
    commands::ota_runs(app.state()).await.expect("ota runs").is_empty(),
    "a hand-pushed artifact leaves no run card: the store records what the poller decided, and a \
     push the host asked for by path was never planned. the terminal outcome is the whole answer, \
     so a progress bar for one needs a delivery-side reporter that does not exist yet"
  );

  commands::ota_check_now(app.state(), "http://127.0.0.1:1/manifest.json".into())
    .await
    .expect("a poll against nothing is still a poll");
  assert!(
    heard.wait(hints::OTA_POLL, SETTLE).await,
    "the store moving invalidates the poll query"
  );
  assert!(
    commands::ota_poll(app.state())
      .await
      .expect("poll status")
      .error
      .is_some(),
    "the hint said to look; the pull is what says what happened"
  );

  let webapp = Uuid::now_v7();
  let bundle = write_bundle(spool.path(), webapp);
  assert_eq!(
    commands::webapp_bundle_extension(bundle.clone()).expect("the picker reads the bundle"),
    None,
    "a plain webapp has nothing for the picker to confirm before it installs"
  );
  let installed = tokio::time::timeout(
    DRIVE_DEADLINE,
    commands::ota_install_webapp(
      app.state(),
      app.state(),
      bundle,
      Some("https://apps.bridgething.test/catalog.json".into()),
      None,
    ),
  )
  .await
  .expect("the install ended rather than parking")
  .expect("the install ran");
  assert_eq!(
    installed,
    InstallOutcome::Installed { id: webapp.to_string() },
    "an action hands back an id, never the record itself"
  );
  assert!(
    heard.wait(hints::WEBAPPS, SETTLE).await,
    "the device saying what it installed invalidates the webapp query"
  );

  let listed = commands::webapps(app.state()).await.expect("the device's webapp list");
  assert!(
    listed.iter().any(|info| info.id == webapp.to_string()),
    "the device's own registry is what the webapp query answers from"
  );
  assert!(commands::webapp_active(app.state()).await.is_ok());

  let sidecar = Uuid::now_v7();
  let declaring = write_extension_bundle(spool.path(), sidecar);
  assert_eq!(
    commands::webapp_bundle_extension(declaring.clone())
      .expect("the picker reads the bundle")
      .map(|declared| declared.permissions),
    Some(vec!["all".to_owned()]),
    "the picker has to be able to show what a local bundle would run before it runs it"
  );
  assert!(
    tokio::time::timeout(
      DRIVE_DEADLINE,
      commands::ota_install_webapp(app.state(), app.state(), declaring, None, None),
    )
    .await
    .expect("the install ended rather than parking")
    .is_ok()
  );
  assert!(
    !spool
      .path()
      .join("state/extensions")
      .join(sidecar.to_string())
      .join("0.1.0")
      .exists(),
    "picking a zip off your own disk is not consent to run it: with no confirmation, nothing is extracted"
  );

  let unreadable = Uuid::now_v7();
  let confirmed = write_extension_bundle(spool.path(), unreadable);
  assert!(
    tokio::time::timeout(
      DRIVE_DEADLINE,
      commands::ota_install_webapp(
        app.state(),
        app.state(),
        confirmed,
        None,
        Some(vec!["net:one,two".to_owned()]),
      ),
    )
    .await
    .expect("the install ended rather than parking")
    .is_err(),
    "a descriptor set the host cannot read is not a set anyone consented to"
  );
  let listed = commands::webapps(app.state()).await.expect("the device's webapp list");
  assert!(
    !listed.iter().any(|info| info.id == unreadable.to_string()),
    "the consent is checked before the device is touched, or the Car Thing ends up holding an app the \
     command reported as failed"
  );

  let orphan = Uuid::now_v7();
  app.state::<Arc<Shell>>().extensions().adopt(
    Some(&device_id),
    &write_extension_bundle(spool.path(), orphan),
    Some(&["all".parse().expect("a descriptor")]),
  );
  let remembered = commands::known_devices(app.state())
    .await
    .expect("known devices")
    .into_iter()
    .find(|known| known.url == device_id)
    .expect("the device that answered the dial is remembered");
  assert_ne!(
    remembered.id, device_id,
    "the daemon has named itself by now, so the row is keyed on that and not on the address"
  );
  assert_eq!(
    holders(spool.path(), orphan),
    vec![remembered.id.clone()],
    "an extension is claimed by the daemon whose install brought it, under what that daemon named \
     itself: an address is whoever answers there today"
  );
  commands::forget_known_device(app.state(), remembered.id)
    .await
    .expect("forgetting a device");
  assert!(
    holders(spool.path(), orphan).is_empty(),
    "a forgotten device never reports a webapp list again, so nothing else would ever drop its claim \
     and the sidecar outlives every app that could reach it"
  );

  let icon = commands::webapp_resource(app.state(), webapp.to_string(), WebappResourceKind::Icon)
    .await
    .expect("the icon comes off the device");
  assert_eq!(icon.bytes, ICON, "the resource pull hands back the bytes it cached");

  let cached = commands::webapp_resource(app.state(), webapp.to_string(), WebappResourceKind::Icon)
    .await
    .expect("the second fetch is served against the have cue");
  assert_eq!(
    cached, icon,
    "a resource pull is idempotent: the have cue makes the second one cheap, not different"
  );

  app.state::<Arc<Shell>>().session().resumed().await;
  assert!(
    heard.wait(hints::SESSION, SETTLE).await,
    "coming back to the foreground invalidates everything and refetches nothing itself"
  );

  let emitted = heard.all();
  assert!(emitted.len() > 3, "the run produced hints to check, got {emitted:?}");
  for hint in &emitted {
    let rendered = serde_json::to_value(Invalidation { id: hint.id.clone() }).expect("a hint renders");
    let object = rendered.as_object().expect("a hint is an object");
    assert_eq!(
      object.len(),
      1,
      "an event carries an id and nothing else; {} carried {rendered}",
      hint.name
    );
    assert!(object.contains_key("id"));
  }

  commands::ota_dismiss_run(app.state()).await.expect("dismiss");
  commands::disconnect(app.state(), None).await.expect("disconnect");
  assert!(
    commands::webapps(app.state()).await.is_err(),
    "a device pull with no link declines rather than waiting for one"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_gateway_finishes_stock_onboarding_and_it_outlives_the_link() {
  let daemon = Daemon::shared();
  let gateway_url = daemon.url();
  let stock_url = stock_url_for(&gateway_url);

  let spool = tempfile::tempdir().expect("a scratch directory");
  let (tx, _rx) = mpsc::unbounded_channel();
  let shell =
    Shell::create(shell_config(gateway_url, spool.path()), Arc::new(Channel { tx })).expect("the shell builds");
  shell.start().await;
  let app = mock_app(shell);
  let device_id = commands::connect(app.state(), None)
    .await
    .expect("the network gateway connects");

  let asked = stock_url.clone();
  assert_eq!(
    tokio::task::spawn_blocking(move || await_stock_onboarding(&asked))
      .await
      .expect("the stock reader runs")
      .as_deref(),
    Some("finished"),
    "a network gateway finishes stock onboarding on its own, with no phone ever paired"
  );

  commands::disconnect(app.state(), Some(device_id))
    .await
    .expect("the gateway link drops");

  assert_eq!(
    tokio::task::spawn_blocking(move || ask_stock_onboarding(&stock_url))
      .await
      .expect("the stock reader runs")
      .as_deref(),
    Some("finished"),
    "and it stays finished once the companion is gone; the stock ui re-reads this every time it is shown, \
     so a gate that hangs off a live link drops the device back into onboarding the moment the app quits"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_resume_target_preference_round_trips_and_invalidates_the_device_meta() {
  let daemon = Daemon::shared();
  let url = daemon.url();

  let spool = tempfile::tempdir().expect("a scratch directory");
  let (tx, rx) = mpsc::unbounded_channel();
  let heard = Heard {
    rx: Mutex::new(rx),
    seen: Mutex::new(Vec::new()),
  };
  let shell =
    Shell::create(shell_config(url.clone(), spool.path()), Arc::new(Channel { tx })).expect("the shell builds");
  shell.start().await;

  let app = mock_app(shell);

  let device_id = commands::connect(app.state(), None)
    .await
    .expect("the daemon accepts a link");
  assert_eq!(device_id, url, "the link is named by the daemon it reached");

  assert_eq!(
    commands::device_resume_target(app.state())
      .await
      .expect("a resume target"),
    ResumeTarget::AnySpeaker,
    "an unset preference answers the default for this kind of host, and a desktop is not carried anywhere, \
     so resuming onto it alone is the one thing the user did not ask for"
  );

  commands::set_device_resume_target(app.state(), ResumeTarget::PhoneOnly)
    .await
    .expect("the pick lands");
  assert!(
    heard.wait(hints::DEVICE_META, SETTLE).await,
    "the preference moving invalidates the device-meta query"
  );
  assert_eq!(
    commands::device_resume_target(app.state())
      .await
      .expect("the second read"),
    ResumeTarget::PhoneOnly,
    "the pull answers what the command wrote, through the same device"
  );

  commands::set_device_resume_target(app.state(), ResumeTarget::AnySpeaker)
    .await
    .expect("and back again");
  assert_eq!(
    commands::device_resume_target(app.state())
      .await
      .expect("the third read"),
    ResumeTarget::AnySpeaker,
    "the second write answers too; the preference is a value, not a sticky first one"
  );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_car_thing_on_the_link_answers_the_whole_pull_surface() {
  let url = std::env::var("BRIDGETHING_GATEWAY_URL").unwrap_or_else(|_| "ws://bridgething.local:8892/".to_owned());
  let spool = tempfile::tempdir().expect("a scratch directory");
  let (tx, rx) = mpsc::unbounded_channel();
  let heard = Heard {
    rx: Mutex::new(rx),
    seen: Mutex::new(Vec::new()),
  };
  let shell =
    Shell::create(shell_config(url.clone(), spool.path()), Arc::new(Channel { tx })).expect("the shell builds");
  shell.start().await;

  let app = mock_app(shell);
  let device_id = commands::connect(app.state(), Some(url.clone()))
    .await
    .expect("the device accepts a link");
  assert_eq!(device_id, url);
  assert!(
    heard.wait(hints::PEERS, SETTLE).await,
    "the device announced itself and the peers query went stale"
  );

  let snapshot = commands::session_snapshot(app.state()).await.expect("a snapshot");
  assert_eq!(snapshot.peers.len(), 1, "the device is the only peer");

  let webapps = commands::webapps(app.state()).await.expect("the device's registry");
  assert!(
    !webapps.is_empty(),
    "a booted device always carries at least its builtin webapps"
  );
  assert!(
    webapps.iter().all(|info| !info.id.is_empty() && !info.name.is_empty()),
    "every registry entry is fully formed"
  );

  commands::webapp_active(app.state()).await.expect("the active webapp");
  commands::device_meta(app.state()).await.expect("device meta");
  commands::ota_runs(app.state()).await.expect("ota runs");
  commands::device_logs(app.state(), 16).await.expect("a log tail");

  commands::disconnect(app.state(), None)
    .await
    .expect("the link closes cleanly");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_daemons_stay_linked_and_the_window_picks_between_them() {
  let Ok(second) = std::env::var("BRIDGETHING_SECOND_GATEWAY_URL") else {
    eprintln!("skipping: set BRIDGETHING_SECOND_GATEWAY_URL to a second daemon to exercise two links");
    return;
  };
  let daemon = Daemon::shared();
  let first = daemon.url();

  let spool = tempfile::tempdir().expect("a scratch directory");
  let (tx, _rx) = mpsc::unbounded_channel();
  let shell =
    Shell::create(shell_config(first.clone(), spool.path()), Arc::new(Channel { tx })).expect("the shell builds");
  shell.start().await;
  let app = mock_app(shell);

  commands::connect(app.state(), Some(first.clone()))
    .await
    .expect("first");
  commands::connect(app.state(), Some(second.clone()))
    .await
    .expect("second");

  let deadline = tokio::time::Instant::now() + SETTLE;
  let mut peers = commands::peers(app.state()).await.expect("peers");
  while peers.len() < 2 && tokio::time::Instant::now() < deadline {
    tokio::time::sleep(Duration::from_millis(50)).await;
    peers = commands::peers(app.state()).await.expect("peers");
  }
  assert_eq!(peers.len(), 2, "the second link did not evict the first");

  commands::select_device(app.state(), Some(first.clone()))
    .await
    .expect("select the first");
  assert_eq!(
    commands::selected_device(app.state()).await.expect("selection"),
    Some(first.clone())
  );

  commands::disconnect(app.state(), Some(first.clone()))
    .await
    .expect("drop the first");
  let left = commands::peers(app.state()).await.expect("peers after the drop");
  assert_eq!(left.len(), 1, "dropping one link left the other serving");
  assert_eq!(left[0].id, second, "the survivor is the one that was not dropped");
  assert_eq!(
    commands::selected_device(app.state()).await.expect("selection"),
    Some(second),
    "with one device left there is no choice to make, so it selects itself"
  );
}
