use std::{
  net::{IpAddr, SocketAddr},
  sync::{Arc, RwLock, atomic::AtomicBool},
  time::Duration,
};

use headless_chrome::{
  Browser, Tab,
  protocol::cdp::{
    Network,
    Page::{
      AddScriptToEvaluateOnNewDocument, GetNavigationHistory, NavigateToHistoryEntry,
      RemoveScriptToEvaluateOnNewDocument,
    },
  },
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

const ENV_CHROME_PORT: &str = "BRIDGETHING_CHROME_PORT";

const CHROME_IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 24 * 365 * 10);
const CHROME_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const CHROME_CONNECT_BACKOFF: Duration = Duration::from_secs(1);
const ERROR_URL_PREFIX: &str = "chrome-error://";
const BLANK_URL_PREFIX: &str = "about:blank";
const STRANDED_CONFIRMATIONS: u8 = 2;
const DEV_HOST_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const DEV_HOST_LOST_CONFIRMATIONS: u8 = 3;

fn looks_stranded(uri: &str, serving: bool) -> bool {
  uri.starts_with(ERROR_URL_PREFIX) || (serving && uri.starts_with(BLANK_URL_PREFIX))
}

fn is_web_page(url: &str) -> bool {
  !(url.starts_with("chrome://")
    || url.starts_with("chrome-extension://")
    || url.starts_with("chrome-untrusted://")
    || url.starts_with("devtools://"))
}

fn dev_host_of(url: &str) -> Option<SocketAddr> {
  let parsed = url::Url::parse(url).ok()?;
  let ip = match parsed.host()? {
    url::Host::Ipv4(ip) => IpAddr::V4(ip),
    url::Host::Ipv6(ip) => IpAddr::V6(ip),
    url::Host::Domain(_) => return None,
  };
  let port = parsed.port_or_known_default()?;
  (!ip.is_loopback() && !ip.is_unspecified()).then_some(SocketAddr::new(ip, port))
}

#[cfg(not(debug_assertions))]
const DEFAULT_CHROME_PORT: u16 = 9223;
#[cfg(debug_assertions)]
const DEFAULT_CHROME_PORT: u16 = 9222;

fn chrome_port() -> u16 {
  if let Ok(raw) = std::env::var(ENV_CHROME_PORT) {
    match raw.parse::<u16>() {
      Ok(p) => return p,
      Err(_) => {
        tracing::warn!("ignoring invalid {ENV_CHROME_PORT}={raw:?}, falling back to default {DEFAULT_CHROME_PORT}")
      }
    }
  }
  DEFAULT_CHROME_PORT
}

fn chrome_status_url() -> String {
  format!("http://127.0.0.1:{}/json/version", chrome_port())
}

type ChromeTx = tokio::sync::mpsc::Sender<ChromeCommand>;
type ChromeRx = tokio::sync::mpsc::Receiver<ChromeCommand>;

#[derive(Debug, Clone)]
pub enum ChromeCommand {
  Navigate(String),
  NavigateExternal(String),
  HistoryBack,
  HistoryForward,
  Reload,
  ClearHttpCache,
  NoteServing,
  SetOverlay {
    script: Option<OverlayScript>,
    run_immediately: bool,
  },
}

#[derive(Clone)]
pub struct OverlayScript(pub Arc<String>);

impl std::fmt::Debug for OverlayScript {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "OverlayScript({} bytes)", self.0.len())
  }
}

type DevHost = Arc<RwLock<Option<SocketAddr>>>;

#[derive(Debug)]
pub struct Chrome {
  connected: Arc<AtomicBool>,
  external: Arc<AtomicBool>,
  dev_host: DevHost,
  tx: ChromeTx,

  cancel_token: tokio_util::sync::CancellationToken,
  _worker: tokio::task::JoinHandle<()>,
}

impl Chrome {
  pub async fn init(home_url: String) -> Result<Self> {
    tracing::debug!("initializing chrome worker (port {})", chrome_port());
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let connected = Arc::new(AtomicBool::new(false));
    let external = Arc::new(AtomicBool::new(false));
    let dev_host: DevHost = Arc::new(RwLock::new(None));

    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut worker = ChromeWorker::new(
      connected.clone(),
      external.clone(),
      dev_host.clone(),
      rx,
      cancel_token.clone(),
      home_url,
    )?;

    Ok(Self {
      connected: connected.clone(),
      external,
      dev_host,
      tx,

      cancel_token: cancel_token.clone(),
      _worker: tokio::spawn(async move { worker.run().await }),
    })
  }

  pub fn connected(&self) -> bool {
    self.connected.load(std::sync::atomic::Ordering::SeqCst)
  }

  pub fn is_external(&self) -> bool {
    self.external.load(std::sync::atomic::Ordering::SeqCst)
  }

  pub fn dev_host(&self) -> Option<IpAddr> {
    self
      .dev_host
      .read()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .map(|target| target.ip())
  }

  pub async fn send(&self, command: ChromeCommand) -> Result<()> {
    tracing::debug!("sending command to chrome: {:?}", command);
    Ok(self.tx.send(command).await?)
  }

  pub async fn shutdown(&self) {
    self.cancel_token.cancel();
  }
}

struct ChromeWorker {
  connected: Arc<AtomicBool>,
  external: Arc<AtomicBool>,
  dev_host: DevHost,
  serving: bool,
  browser: Option<Browser>,
  http: reqwest::Client,
  overlay_id: Option<String>,
  overlay: Option<OverlayScript>,
  overlay_run_immediately: bool,
  home_url: String,
  settled: bool,
  stranded_streak: u8,
  dev_host_misses: u8,

  rx: ChromeRx,
  cancel_token: CancellationToken,
}

impl ChromeWorker {
  fn new(
    connected: Arc<AtomicBool>,
    external: Arc<AtomicBool>,
    dev_host: DevHost,
    rx: ChromeRx,
    cancel_token: CancellationToken,
    home_url: String,
  ) -> Result<Self> {
    let http = reqwest::Client::builder()
      .connect_timeout(CHROME_PROBE_TIMEOUT)
      .timeout(CHROME_PROBE_TIMEOUT)
      .build()
      .map_err(|e| ChromeError::Connect(Box::new(e)))?;

    Ok(Self {
      connected,
      external,
      dev_host,
      serving: false,
      browser: None,
      http,
      overlay_id: None,
      overlay: None,
      overlay_run_immediately: false,
      home_url,
      settled: false,
      stranded_streak: 0,
      dev_host_misses: 0,

      rx,
      cancel_token,
    })
  }

  async fn run(&mut self) {
    let mut retry = tokio::time::interval(CHROME_CONNECT_BACKOFF);
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
      tokio::select! {
        _ = retry.tick() => {
          self.reconcile().await;
          self.watch_dev_host().await;
        }
        Some(message) = self.rx.recv() => {
          match message {
            ChromeCommand::Navigate(url) => {
              self.external.store(false, std::sync::atomic::Ordering::SeqCst);
              self.set_dev_host(dev_host_of(&url));
              self.handle_navigate(url).await
            }
            ChromeCommand::NavigateExternal(url) => {
              self.external.store(true, std::sync::atomic::Ordering::SeqCst);
              self.set_dev_host(None);
              self.handle_navigate(url).await
            }
            ChromeCommand::HistoryBack => self.handle_history(false).await,
            ChromeCommand::HistoryForward => self.handle_history(true).await,
            ChromeCommand::Reload => {
              self.external.store(false, std::sync::atomic::Ordering::SeqCst);
              self.handle_reload().await
            }
            ChromeCommand::ClearHttpCache => self.handle_clear_http_cache().await,
            ChromeCommand::NoteServing => {
              self.serving = true;
              self.settled = false;
            }
            ChromeCommand::SetOverlay { script, run_immediately } => {
              self.handle_set_overlay(script, run_immediately).await
            }
          }
        }
        _ = self.cancel_token.cancelled() => {
          tracing::debug!("chrome worker shutting down");
          break;
        }
      }
    }
  }

  fn dev_target(&self) -> Option<SocketAddr> {
    *self.dev_host.read().unwrap_or_else(|poisoned| poisoned.into_inner())
  }

  async fn watch_dev_host(&mut self) {
    let Some(target) = self.dev_target() else {
      self.dev_host_misses = 0;
      return;
    };
    let answered = tokio::time::timeout(DEV_HOST_PROBE_TIMEOUT, tokio::net::TcpStream::connect(target))
      .await
      .is_ok_and(|connected| connected.is_ok());
    if answered {
      self.dev_host_misses = 0;
      return;
    }
    self.dev_host_misses = self.dev_host_misses.saturating_add(1);
    if self.dev_host_misses < DEV_HOST_LOST_CONFIRMATIONS {
      return;
    }
    self.dev_host_misses = 0;
    tracing::info!(%target, "dev host stopped answering; handing the screen back");
    self.set_dev_host(None);
    let home = self.home_url.clone();
    self
      .with_first_tab("leave-dev-host", move |tab| tab.navigate_to(&home).map(|_| ()))
      .await;
  }

  fn set_dev_host(&self, next: Option<SocketAddr>) {
    let mut held = self.dev_host.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    if *held == next {
      return;
    }
    match next {
      Some(target) => tracing::info!(%target, "kiosk pointed at a dev host; the socks proxy reaches it directly"),
      None => tracing::info!("kiosk left the dev host"),
    }
    *held = next;
  }

  async fn handle_reload(&mut self) {
    tracing::debug!("reloading current chrome tab");
    self
      .with_first_tab("reload", |tab| tab.reload(true, None).map(|_| ()))
      .await;
  }

  async fn handle_navigate(&mut self, url: String) {
    tracing::debug!("navigating to {}", url);
    self.settled = false;
    self
      .with_first_tab("navigate", move |tab| {
        if url_matches_current(&tab.get_url(), &url) {
          tab.reload(true, None).map(|_| ())
        } else {
          tab.navigate_to(&url).map(|_| ())
        }
      })
      .await;
  }

  async fn handle_history(&mut self, forward: bool) {
    tracing::debug!("history navigate (forward={})", forward);
    self
      .with_first_tab("history", move |tab| {
        let history = tab.call_method(GetNavigationHistory(None))?;
        let current = history.current_index as usize;
        let target = if forward {
          current + 1
        } else if current == 0 {
          return Ok(());
        } else {
          current - 1
        };
        let Some(entry) = history.entries.get(target) else {
          return Ok(());
        };
        let entry_id = entry.id;
        tab.call_method(NavigateToHistoryEntry { entry_id }).map(|_| ())
      })
      .await;
  }

  async fn handle_set_overlay(&mut self, script: Option<OverlayScript>, run_immediately: bool) {
    self.overlay = script.clone();
    self.overlay_run_immediately = run_immediately;
    self.apply_overlay(script, run_immediately).await;
  }

  async fn apply_overlay(&mut self, script: Option<OverlayScript>, run_immediately: bool) {
    tracing::debug!(
      installed = script.is_some(),
      run_immediately,
      "setting overlay injection"
    );
    let prior = self.overlay_id.take();
    self.overlay_id = self
      .with_first_tab("set-overlay", move |tab| {
        if let Some(id) = &prior {
          let _ = tab.call_method(RemoveScriptToEvaluateOnNewDocument { identifier: id.clone() });
        }
        let Some(src) = &script else {
          return Ok(None);
        };
        let installed = tab.call_method(AddScriptToEvaluateOnNewDocument {
          source: (*src.0).clone(),
          world_name: None,
          include_command_line_api: None,
          run_immediately: run_immediately.then_some(true),
        })?;
        Ok(Some(installed.identifier))
      })
      .await
      .flatten();
  }

  async fn handle_clear_http_cache(&mut self) {
    tracing::debug!("clearing chromium http cache");
    self
      .with_first_tab("clear-http-cache", |tab| {
        tab.call_method(Network::ClearBrowserCache(None)).map(|_| ())
      })
      .await;
  }

  async fn reconcile(&mut self) {
    if self.settled && self.connected.load(std::sync::atomic::Ordering::SeqCst) {
      return;
    }

    if self.browser.is_none() {
      self.connect_browser().await;
      if self.browser.is_none() {
        return;
      }
    }

    let recovered = self.recover_stranded_tab().await;

    if self.overlay.is_some() && self.overlay_id.is_none() {
      let (script, run_immediately) = (self.overlay.clone(), self.overlay_run_immediately);
      self.apply_overlay(script, run_immediately).await;
    }

    self.settled = !recovered && self.connected.load(std::sync::atomic::Ordering::SeqCst);
  }

  async fn recover_stranded_tab(&mut self) -> bool {
    if self.external.load(std::sync::atomic::Ordering::SeqCst) {
      self.stranded_streak = 0;
      return false;
    }

    let serving = self.serving;
    let seen = self
      .with_first_tab("read-document-uri", move |tab| {
        let uri = tab
          .evaluate("document.documentURI", false)?
          .value
          .and_then(|v| v.as_str().map(str::to_owned))
          .unwrap_or_default();
        Ok(looks_stranded(&uri, serving))
      })
      .await;

    if seen != Some(true) {
      self.stranded_streak = 0;
      return false;
    }

    self.stranded_streak = self.stranded_streak.saturating_add(1);
    if self.stranded_streak < STRANDED_CONFIRMATIONS {
      return true;
    }
    self.stranded_streak = 0;
    self.set_dev_host(None);

    let home = self.home_url.clone();
    let target = home.clone();
    self
      .with_first_tab("recover-stranded", move |tab| tab.navigate_to(&target).map(|_| ()))
      .await;
    tracing::info!("chrome was stranded off-app; navigated back to {home}");
    true
  }

  async fn with_first_tab<F, T>(&mut self, label: &'static str, op: F) -> Option<T>
  where
    F: Fn(Arc<Tab>) -> anyhow::Result<T> + Clone + Send + 'static,
    T: Send + 'static,
  {
    for attempt in 0..2u8 {
      let tab = self.first_tab().await?;
      let op = op.clone();

      match tokio::task::spawn_blocking(move || safe_call(label, || op(tab))).await {
        Ok(Ok(value)) => return Some(value),
        Ok(Err(e)) => {
          tracing::warn!("chrome {label} failed (attempt {attempt}): {e:?}; dropping connection");
          self.drop_browser();
        }
        Err(e) => {
          tracing::warn!("chrome {label} task died (attempt {attempt}): {e:?}; dropping connection");
          self.drop_browser();
        }
      }
    }
    tracing::error!("chrome {label} gave up after one retry");
    None
  }

  fn drop_browser(&mut self) {
    self.browser = None;
    self.connected.store(false, std::sync::atomic::Ordering::SeqCst);
  }

  async fn first_tab(&mut self) -> Option<Arc<Tab>> {
    if self.browser.is_none() {
      self.connect_browser().await;
    }
    let browser = self.browser.take()?;

    let (browser, registered, tab) = tokio::task::spawn_blocking(move || {
      let registered = catch_panic("register_missing_tabs", || browser.register_missing_tabs());
      let tab = match browser.get_tabs().lock() {
        Ok(guard) => Ok(guard.iter().find(|t| is_web_page(&t.get_url())).cloned()),
        Err(e) => Err(format!("tabs mutex poisoned: {e:?}")),
      };
      (browser, registered, tab)
    })
    .await
    .ok()?;

    if let Err(e) = registered {
      tracing::warn!("{e:?}; dropping cached browser");
      self.connected.store(false, std::sync::atomic::Ordering::SeqCst);
      return None;
    }
    let tab = match tab {
      Ok(tab) => tab,
      Err(e) => {
        tracing::error!("{e}; dropping cached browser");
        self.connected.store(false, std::sync::atomic::Ordering::SeqCst);
        return None;
      }
    };

    self.browser = Some(browser);
    if tab.is_none() {
      tracing::warn!("chrome reports no tabs");
    }
    tab
  }

  async fn connect_browser(&mut self) {
    let url = chrome_status_url();
    tracing::debug!("probing {url}");

    let res = match self.http.get(&url).send().await {
      Ok(r) => r,
      Err(e) => return tracing::debug!("failed to GET {url}: {e}"),
    };

    let status = match res.json::<ChromeStatus>().await {
      Ok(s) => s,
      Err(e) => return tracing::debug!("failed to parse {url}: {e}"),
    };
    tracing::trace!("chrome status: {status:?}");

    let connected = tokio::task::spawn_blocking(move || Browser::connect_with_timeout(status.url, CHROME_IDLE_TIMEOUT))
      .await
      .unwrap_or_else(|e| Err(anyhow::anyhow!("connect task died: {e:?}")));

    match connected {
      Ok(browser) => {
        tracing::info!("connected to chrome on port {}", chrome_port());
        self.connected.store(true, std::sync::atomic::Ordering::SeqCst);
        self.browser = Some(browser);
      }
      Err(e) => tracing::warn!("Browser::connect_with_timeout failed: {e:?}"),
    }
  }
}

fn url_matches_current(current: &str, target: &str) -> bool {
  let normalize = |s: &str| s.trim_end_matches('/').to_string();
  normalize(current) == normalize(target)
}

fn safe_call<F, R>(label: &str, f: F) -> anyhow::Result<R>
where
  F: FnOnce() -> anyhow::Result<R>,
{
  match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
    Ok(Ok(r)) => Ok(r),
    Ok(Err(e)) => Err(e),
    Err(p) => anyhow::bail!("{label} panicked: {p:?}"),
  }
}

fn catch_panic<F>(label: &str, f: F) -> anyhow::Result<()>
where
  F: FnOnce(),
{
  match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
    Ok(()) => Ok(()),
    Err(p) => anyhow::bail!("{label} panicked: {p:?}"),
  }
}

#[derive(Debug, Deserialize)]
struct ChromeStatus {
  #[serde(rename = "webSocketDebuggerUrl")]
  url: String,
}

type Result<T> = std::result::Result<T, ChromeError>;
#[derive(Debug, thiserror::Error)]
pub enum ChromeError {
  #[error("chrome connection error: {0}")]
  Connect(Box<dyn std::error::Error + Send + Sync>),
  #[error(transparent)]
  Tx(#[from] tokio::sync::mpsc::error::SendError<ChromeCommand>),
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn error_pages_are_stranded_whether_or_not_we_serve() {
    assert!(looks_stranded("chrome-error://chromewebdata/", false));
    assert!(looks_stranded("chrome-error://chromewebdata/", true));
  }

  #[test]
  fn blank_is_only_stranded_once_we_can_answer() {
    assert!(!looks_stranded("about:blank", false));
    assert!(looks_stranded("about:blank", true));
  }

  #[test]
  fn a_loaded_page_is_never_stranded() {
    assert!(!looks_stranded("http://127.0.0.1:8891/", false));
    assert!(!looks_stranded("http://127.0.0.1:8891/", true));
  }

  #[test]
  fn only_an_off_device_ip_literal_names_a_dev_host() {
    assert_eq!(
      dev_host_of("http://10.42.1.116:5173/"),
      Some("10.42.1.116:5173".parse().unwrap())
    );
    assert_eq!(
      dev_host_of("http://10.42.1.116/"),
      Some("10.42.1.116:80".parse().unwrap())
    );
    assert_eq!(
      dev_host_of("http://[fe80::1]:5173/"),
      Some("[fe80::1]:5173".parse().unwrap())
    );
    assert_eq!(dev_host_of("http://127.0.0.1:8891/"), None);
    assert_eq!(dev_host_of("http://127.0.0.1:8891/_hub/abc/"), None);
    assert_eq!(dev_host_of("http://0.0.0.0:5173/"), None);
    assert_eq!(dev_host_of("http://bridgething.local:5173/"), None);
    assert_eq!(dev_host_of("https://example.com/"), None);
    assert_eq!(dev_host_of("about:blank"), None);
    assert_eq!(dev_host_of("not a url"), None);
  }

  #[test]
  fn browser_internal_targets_are_never_the_kiosk_tab() {
    assert!(!is_web_page("chrome://omnibox-popup.top-chrome/"));
    assert!(!is_web_page("chrome-extension://abc/_generated_background_page.html"));
    assert!(!is_web_page("chrome-untrusted://new-tab-page/"));
    assert!(!is_web_page("devtools://devtools/bundled/inspector.html"));
    assert!(is_web_page("http://127.0.0.1:8891/"));
    assert!(is_web_page("about:blank"));
  }
}
