use std::{
  io::{Read, Write},
  net::TcpListener,
  path::Path,
  sync::{Arc, Mutex, OnceLock, Weak},
  time::{Duration, Instant},
};

use bridgething_delivery::discovery::Discovery;
use bridgething_desktop::{
  hints::{Hint, HintSink},
  shell::{DEFAULT_GATEWAY_URL, DesktopPaths, Shell, ShellConfig},
};
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};
use tokio::sync::mpsc;

pub const DRIVE_DEADLINE: Duration = Duration::from_secs(120);
pub const SETTLE: Duration = Duration::from_secs(15);

pub struct Channel {
  pub tx: mpsc::UnboundedSender<Hint>,
}

impl HintSink for Channel {
  fn emit(&self, hint: Hint) {
    let _ = self.tx.send(hint);
  }
}

pub fn daemon_host(gateway_url: &str) -> &str {
  let rest = gateway_url.split_once("://").map_or(gateway_url, |(_, rest)| rest);
  let authority = rest.split('/').next().unwrap_or(rest);
  match authority.rsplit_once(':') {
    Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
    _ => authority,
  }
}

pub enum Daemon {
  Borrowed,
  Owned,
  Remote(String),
}

static SHARED: Mutex<Option<Weak<Daemon>>> = Mutex::new(None);

impl Daemon {
  pub fn shared() -> Arc<Self> {
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

  pub fn url(&self) -> String {
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

pub fn reachable(url: &str) -> bool {
  let authority = url
    .trim_start_matches("ws://")
    .trim_start_matches("wss://")
    .trim_end_matches('/');
  let Ok(mut addrs) = std::net::ToSocketAddrs::to_socket_addrs(authority) else {
    return false;
  };
  addrs.any(|addr| std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok())
}

pub fn supervise(action: &str) -> std::process::ExitStatus {
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

pub fn mock_app(shell: Arc<Shell>) -> tauri::App<MockRuntime> {
  mock_builder()
    .manage(Arc::clone(shell.extensions()))
    .manage(shell)
    .manage(Discovery::spawn(|_| ()).expect("the responder starts"))
    .invoke_handler(bridgething_desktop::desktop_commands!())
    .build(mock_context(noop_assets()))
    .expect("the shell's command surface builds without a window")
}

pub fn shell_config(url: impl Into<String>, spool: &Path) -> ShellConfig {
  model_root();
  ShellConfig::new(url, DesktopPaths::under(spool))
}

pub type ModelRoot = (String, Arc<Mutex<Vec<String>>>);

static MODEL_ROOT: OnceLock<ModelRoot> = OnceLock::new();

pub fn model_root() -> &'static ModelRoot {
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

    // SAFETY: set once first
    unsafe { std::env::set_var("BRIDGETHING_MODEL_ROOT", &url) };
    (url, asked)
  })
}
