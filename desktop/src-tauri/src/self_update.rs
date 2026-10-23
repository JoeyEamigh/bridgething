use std::{
  fs,
  future::Future,
  path::{Path, PathBuf},
  sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
  },
  time::{Duration, Instant, SystemTime},
};

use serde::Serialize;
use tauri::{AppHandle, Manager as _, Runtime, State};
use tauri_plugin_notification::{NotificationExt as _, PermissionState};
use tauri_plugin_updater::{Update, Updater, UpdaterExt as _};
use tokio::sync::{Notify, watch};

use crate::hints::{Hint, HintSink, SELF_UPDATE};

const EVERY: Duration = Duration::from_secs(15 * 60);
const TICK: Duration = Duration::from_secs(60);
const FIRST_RETRY: Duration = Duration::from_secs(60);
const STALL: Duration = Duration::from_secs(60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const NOTICE: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SelfUpdateState {
  Idle,
  Checking,
  Current {
    checked_at: String,
  },
  Downloading {
    version: String,
    received: u64,
    total: Option<u64>,
  },
  Ready {
    version: String,
    date: Option<String>,
    notes: Option<String>,
  },
  Installing {
    version: String,
  },
  Unavailable {
    reason: String,
  },
}

pub trait Release: Clone + Send + Sync + 'static {
  fn version(&self) -> &str;
  fn date(&self) -> Option<String>;
  fn notes(&self) -> Option<String>;
}

pub trait Feed: Send + Sync + 'static {
  type Release: Release;

  fn check(&self) -> impl Future<Output = Result<Option<Self::Release>, String>> + Send;

  fn download(
    &self,
    release: &Self::Release,
    progress: impl FnMut(usize, Option<u64>) + Send + 'static,
  ) -> impl Future<Output = Result<Vec<u8>, String>> + Send;

  fn install(&self, release: &Self::Release, bytes: &[u8]) -> Result<(), String>;
}

impl Release for Update {
  fn version(&self) -> &str {
    &self.version
  }

  fn date(&self) -> Option<String> {
    self
      .raw_json
      .get("pub_date")
      .and_then(serde_json::Value::as_str)
      .map(str::to_owned)
  }

  fn notes(&self) -> Option<String> {
    self.body.clone()
  }
}

pub struct TauriFeed(Result<Updater, String>);

impl TauriFeed {
  pub fn new<R: Runtime>(app: &AppHandle<R>) -> Self {
    let handle = app.clone();
    let updater = app
      .updater_builder()
      .timeout(CHECK_TIMEOUT)
      .on_before_exit(move || crate::process::release(&handle))
      .build()
      .map_err(|error| error.to_string());
    Self(updater)
  }
}

impl Feed for TauriFeed {
  type Release = Update;

  async fn check(&self) -> Result<Option<Update>, String> {
    let updater = self.0.as_ref().map_err(Clone::clone)?;
    updater.check().await.map_err(|error| error.to_string())
  }

  async fn download(
    &self,
    release: &Update,
    progress: impl FnMut(usize, Option<u64>) + Send + 'static,
  ) -> Result<Vec<u8>, String> {
    release
      .download(progress, || {})
      .await
      .map_err(|error| error.to_string())
  }

  fn install(&self, release: &Update, bytes: &[u8]) -> Result<(), String> {
    release.install(bytes).map_err(|error| error.to_string())
  }
}

pub type Updates = SelfUpdate<TauriFeed>;

struct Ready<T> {
  release: T,
  bytes: Arc<Vec<u8>>,
}

struct Held<T> {
  state: SelfUpdateState,
  ready: Option<Ready<T>>,
  offered: Option<String>,
}

type Offer = Box<dyn Fn(Option<&str>) + Send + Sync>;

pub struct SelfUpdate<F: Feed> {
  feed: F,
  held: Mutex<Held<F::Release>>,
  wake: Notify,
  hints: Arc<dyn HintSink>,
  offer: Offer,
}

impl<F: Feed> SelfUpdate<F> {
  pub fn new(feed: F, hints: Arc<dyn HintSink>, offer: impl Fn(Option<&str>) + Send + Sync + 'static) -> Arc<Self> {
    Arc::new(Self {
      feed,
      held: Mutex::new(Held {
        state: SelfUpdateState::Idle,
        ready: None,
        offered: None,
      }),
      wake: Notify::new(),
      hints,
      offer: Box::new(offer),
    })
  }

  pub fn spawn(self: &Arc<Self>, online: watch::Receiver<bool>) {
    tauri::async_runtime::spawn(Arc::clone(self).run(online));
  }

  pub fn state(&self) -> SelfUpdateState {
    self.held.lock().unwrap().state.clone()
  }

  pub fn check_now(&self) {
    self.wake.notify_one();
  }

  async fn run(self: Arc<Self>, mut online: watch::Receiver<bool>) {
    let mut due = SystemTime::now();
    let mut failures = 0u32;
    loop {
      if SystemTime::now() >= due {
        if self.sweep().await {
          failures = 0;
          due = SystemTime::now() + EVERY;
        } else {
          failures = failures.saturating_add(1);
          due = SystemTime::now() + backoff(failures);
        }
      }
      tokio::select! {
        () = tokio::time::sleep(TICK) => {}
        () = self.wake.notified() => due = SystemTime::now(),
        Ok(()) = online.changed() => {
          if *online.borrow_and_update() {
            tracing::debug!("the host came online; checking for a desktop update");
            due = SystemTime::now();
          }
        }
      }
    }
  }

  async fn sweep(self: &Arc<Self>) -> bool {
    let mut busy = false;
    let mut holding = None;
    self.settle(|held| {
      busy = installing(held);
      holding = held.ready.as_ref().map(|ready| ready.release.version().to_owned());
      if !busy && holding.is_none() {
        held.state = SelfUpdateState::Checking;
      }
    });
    if busy {
      return true;
    }

    let release = match self.feed.check().await {
      Ok(Some(release)) => release,
      Ok(None) => {
        self.settle(|held| {
          if !installing(held) {
            held.ready = None;
            held.state = SelfUpdateState::Current {
              checked_at: chrono::Utc::now().to_rfc3339(),
            };
          }
        });
        return true;
      }
      Err(reason) => {
        tracing::warn!(%reason, "the desktop update feed did not answer");
        self.settle(|held| {
          if !installing(held) && held.ready.is_none() {
            held.state = SelfUpdateState::Unavailable { reason };
          }
        });
        return false;
      }
    };
    if holding.as_deref() == Some(release.version()) {
      return true;
    }

    let version = release.version().to_owned();
    let mut started = false;
    self.settle(|held| {
      if !installing(held) {
        started = true;
        held.state = SelfUpdateState::Downloading {
          version: version.clone(),
          received: 0,
          total: None,
        };
      }
    });
    if !started {
      return true;
    }

    tracing::info!(%version, "fetching a desktop update");
    match self.fetch(&release).await {
      Ok(bytes) => {
        tracing::info!(%version, "a desktop update is ready to install");
        self.settle(|held| {
          if !installing(held) {
            held.state = ready(&release);
            held.ready = Some(Ready {
              release,
              bytes: Arc::new(bytes),
            });
          }
        });
        true
      }
      Err(reason) => {
        tracing::warn!(%version, %reason, "a desktop update did not download");
        self.settle(|held| {
          if !installing(held) {
            held.state = match &held.ready {
              Some(kept) => ready(&kept.release),
              None => SelfUpdateState::Unavailable { reason },
            };
          }
        });
        false
      }
    }
  }

  async fn fetch(self: &Arc<Self>, release: &F::Release) -> Result<Vec<u8>, String> {
    let received = Arc::new(AtomicU64::new(0));
    let progress = {
      let this = Arc::clone(self);
      let received = Arc::clone(&received);
      let version = release.version().to_owned();
      let mut last: Option<Instant> = None;
      move |chunk: usize, total: Option<u64>| {
        let sum = received.fetch_add(chunk as u64, Ordering::Relaxed) + chunk as u64;
        if last.is_some_and(|at| at.elapsed() < PROGRESS_EVERY) {
          return;
        }
        last = Some(Instant::now());
        this.settle(|held| {
          if let SelfUpdateState::Downloading { version: fetching, .. } = &held.state
            && *fetching == version
          {
            held.state = SelfUpdateState::Downloading {
              version: version.clone(),
              received: sum,
              total,
            };
          }
        });
      }
    };
    tokio::select! {
      outcome = self.feed.download(release, progress) => outcome,
      () = stalled(&received, STALL) => Err(format!("the download stalled for {}s", STALL.as_secs())),
    }
  }

  pub async fn apply(self: &Arc<Self>) -> bool {
    let mut taken = None;
    self.settle(|held| {
      if installing(held) {
        return;
      }
      if let Some(ready) = held.ready.take() {
        held.state = SelfUpdateState::Installing {
          version: ready.release.version().to_owned(),
        };
        taken = Some(ready);
      }
    });
    let Some(Ready { release, bytes }) = taken else {
      return false;
    };

    tracing::info!(version = release.version(), "installing a desktop update");
    let this = Arc::clone(self);
    let installed = tokio::task::spawn_blocking(move || this.feed.install(&release, &bytes))
      .await
      .map_err(|error| error.to_string())
      .and_then(|outcome| outcome);
    match installed {
      Ok(()) => true,
      Err(reason) => {
        tracing::error!(%reason, "a desktop update did not install");
        self.settle(|held| {
          held.state = SelfUpdateState::Unavailable {
            reason: format!("the update did not install: {reason}"),
          };
        });
        false
      }
    }
  }

  fn settle(&self, change: impl FnOnce(&mut Held<F::Release>)) {
    let offer = {
      let mut held = self.held.lock().unwrap();
      change(&mut held);
      let ready = held.ready.as_ref().map(|ready| ready.release.version().to_owned());
      (held.offered != ready).then(|| {
        held.offered.clone_from(&ready);
        ready
      })
    };
    self.hints.emit(Hint::bare(SELF_UPDATE));
    if let Some(ready) = offer {
      (self.offer)(ready.as_deref());
    }
  }
}

fn installing<T>(held: &Held<T>) -> bool {
  matches!(held.state, SelfUpdateState::Installing { .. })
}

fn ready(release: &impl Release) -> SelfUpdateState {
  SelfUpdateState::Ready {
    version: release.version().to_owned(),
    date: release.date(),
    notes: release.notes(),
  }
}

fn backoff(failures: u32) -> Duration {
  FIRST_RETRY
    .saturating_mul(1 << failures.saturating_sub(1).min(5))
    .min(EVERY)
}

async fn stalled(received: &AtomicU64, window: Duration) {
  let mut seen = received.load(Ordering::Relaxed);
  loop {
    tokio::time::sleep(window).await;
    let now = received.load(Ordering::Relaxed);
    if now == seen {
      return;
    }
    seen = now;
  }
}

pub struct Announced {
  path: PathBuf,
  last: Mutex<Option<String>>,
}

impl Announced {
  pub fn open(state_dir: &Path) -> Self {
    let path = state_dir.join("announced-update");
    let last = fs::read_to_string(&path)
      .ok()
      .map(|held| held.trim().to_owned())
      .filter(|held| !held.is_empty());
    Self {
      path,
      last: Mutex::new(last),
    }
  }

  fn first(&self, version: &str) -> bool {
    let mut last = self.last.lock().unwrap();
    if last.as_deref() == Some(version) {
      return false;
    }
    *last = Some(version.to_owned());
    if let Err(error) = fs::write(&self.path, version) {
      tracing::warn!(%error, path = %self.path.display(), "the announced update could not be kept");
    }
    true
  }
}

pub fn offer<R: Runtime>(app: &AppHandle<R>, announced: &Announced, ready: Option<&str>) {
  crate::tray::offer(app, ready);
  if let Some(version) = ready
    && announced.first(version)
    && permitted(app)
    && let Err(error) = app
      .notification()
      .builder()
      .id(NOTICE)
      .title(format!("bridgething v{version} is ready"))
      .body("restart to update")
      .show()
  {
    tracing::warn!(%error, %version, "the update notification did not post");
  }
}

fn permitted<R: Runtime>(app: &AppHandle<R>) -> bool {
  let notification = app.notification();
  let state = match notification.permission_state() {
    Ok(PermissionState::Prompt | PermissionState::PromptWithRationale) => notification.request_permission(),
    held => held,
  };
  match state {
    Ok(PermissionState::Granted) => true,
    Ok(_) => false,
    Err(error) => {
      tracing::debug!(%error, "notification permission is unknown; staying quiet");
      false
    }
  }
}

pub fn listen<R: Runtime>(app: &AppHandle<R>) {
  let handle = app.clone();
  let listened = app.notification().on_action(move |performed| {
    if performed.action_id() == "tap" && performed.notification().is_some_and(|shown| shown.id() == NOTICE) {
      crate::tray::present(&handle);
    }
  });
  if let Err(error) = listened {
    tracing::warn!(%error, "clicks on the update notification will not open the window");
  }
}

pub fn restart_into<R: Runtime>(app: &AppHandle<R>) {
  let Some(updates) = app.try_state::<Arc<Updates>>() else {
    return;
  };
  let updates = Arc::clone(&updates);
  let app = app.clone();
  tauri::async_runtime::spawn(async move {
    if updates.apply().await {
      crate::process::restart(&app);
    }
  });
}

#[tauri::command]
pub fn self_update(updates: State<'_, Arc<Updates>>) -> SelfUpdateState {
  updates.state()
}

#[tauri::command]
pub fn check_self_update(updates: State<'_, Arc<Updates>>) {
  updates.check_now();
}

#[tauri::command]
pub fn apply_self_update<R: Runtime>(app: AppHandle<R>) {
  restart_into(&app);
}

#[cfg(test)]
mod tests {
  use std::sync::atomic::AtomicUsize;

  use super::*;

  #[derive(Clone)]
  struct Build(String);

  impl Release for Build {
    fn version(&self) -> &str {
      &self.0
    }

    fn date(&self) -> Option<String> {
      Some("2026-10-22T17:47:58Z".to_owned())
    }

    fn notes(&self) -> Option<String> {
      None
    }
  }

  struct Script {
    published: Mutex<Result<Option<String>, String>>,
    download: Mutex<Result<(), String>>,
    downloads: AtomicUsize,
    installed: Mutex<Vec<String>>,
  }

  #[derive(Clone)]
  struct Fake(Arc<Script>);

  impl Fake {
    fn new() -> Self {
      Self(Arc::new(Script {
        published: Mutex::new(Ok(None)),
        download: Mutex::new(Ok(())),
        downloads: AtomicUsize::new(0),
        installed: Mutex::new(Vec::new()),
      }))
    }

    fn publish(&self, version: &str) {
      *self.0.published.lock().unwrap() = Ok(Some(version.to_owned()));
    }

    fn unreachable(&self, reason: &str) {
      *self.0.published.lock().unwrap() = Err(reason.to_owned());
    }

    fn downloads(&self) -> usize {
      self.0.downloads.load(Ordering::Relaxed)
    }
  }

  impl Feed for Fake {
    type Release = Build;

    async fn check(&self) -> Result<Option<Build>, String> {
      self.0.published.lock().unwrap().clone().map(|found| found.map(Build))
    }

    async fn download(
      &self,
      _release: &Build,
      mut progress: impl FnMut(usize, Option<u64>) + Send + 'static,
    ) -> Result<Vec<u8>, String> {
      self.0.downloads.fetch_add(1, Ordering::Relaxed);
      self.0.download.lock().unwrap().clone()?;
      progress(4, Some(4));
      Ok(vec![0; 4])
    }

    fn install(&self, release: &Build, _bytes: &[u8]) -> Result<(), String> {
      self.0.installed.lock().unwrap().push(release.0.clone());
      Ok(())
    }
  }

  struct Deaf;

  impl HintSink for Deaf {
    fn emit(&self, _hint: Hint) {}
  }

  type Offers = Arc<Mutex<Vec<Option<String>>>>;

  fn rig() -> (Fake, Arc<SelfUpdate<Fake>>, Offers) {
    let feed = Fake::new();
    let offers = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&offers);
    let updates = SelfUpdate::new(feed.clone(), Arc::new(Deaf), move |ready| {
      seen.lock().unwrap().push(ready.map(str::to_owned));
    });
    (feed, updates, offers)
  }

  fn version(state: &SelfUpdateState) -> Option<&str> {
    match state {
      SelfUpdateState::Ready { version, .. } => Some(version),
      _ => None,
    }
  }

  #[tokio::test]
  async fn a_published_build_is_fetched_on_sight_and_offered_outside_the_window() {
    let (feed, updates, offers) = rig();
    feed.publish("0.15.0");

    assert!(updates.sweep().await);

    assert_eq!(version(&updates.state()), Some("0.15.0"));
    assert_eq!(*offers.lock().unwrap(), vec![Some("0.15.0".to_owned())]);
  }

  #[tokio::test]
  async fn nothing_newer_leaves_the_app_saying_when_it_last_looked() {
    let (_feed, updates, offers) = rig();

    assert!(updates.sweep().await);

    assert!(matches!(updates.state(), SelfUpdateState::Current { .. }));
    assert!(offers.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn a_build_already_waiting_is_not_fetched_again() {
    let (feed, updates, _offers) = rig();
    feed.publish("0.15.0");
    updates.sweep().await;

    updates.sweep().await;

    assert_eq!(feed.downloads(), 1);
    assert_eq!(version(&updates.state()), Some("0.15.0"));
  }

  #[tokio::test]
  async fn a_waiting_build_is_replaced_when_a_newer_one_is_published() {
    let (feed, updates, offers) = rig();
    feed.publish("0.15.0");
    updates.sweep().await;
    feed.publish("0.16.0");

    updates.sweep().await;

    assert_eq!(version(&updates.state()), Some("0.16.0"));
    assert_eq!(
      *offers.lock().unwrap(),
      vec![Some("0.15.0".to_owned()), Some("0.16.0".to_owned())]
    );
  }

  #[tokio::test]
  async fn a_feed_that_cannot_be_reached_is_reported_and_retried() {
    let (feed, updates, _offers) = rig();
    feed.unreachable("dns error");

    assert!(!updates.sweep().await);
    assert_eq!(
      updates.state(),
      SelfUpdateState::Unavailable {
        reason: "dns error".to_owned()
      }
    );

    feed.publish("0.15.0");
    assert!(updates.sweep().await);
    assert_eq!(version(&updates.state()), Some("0.15.0"));
  }

  #[tokio::test]
  async fn a_network_blip_never_hides_a_build_that_is_already_waiting() {
    let (feed, updates, _offers) = rig();
    feed.publish("0.15.0");
    updates.sweep().await;
    feed.unreachable("offline");

    assert!(!updates.sweep().await);

    assert_eq!(version(&updates.state()), Some("0.15.0"));
  }

  #[tokio::test]
  async fn a_failed_download_of_a_newer_build_falls_back_to_the_one_waiting() {
    let (feed, updates, _offers) = rig();
    feed.publish("0.15.0");
    updates.sweep().await;
    feed.publish("0.16.0");
    *feed.0.download.lock().unwrap() = Err("connection reset".to_owned());

    assert!(!updates.sweep().await);

    assert_eq!(version(&updates.state()), Some("0.15.0"));
  }

  #[tokio::test]
  async fn a_feed_that_pulls_the_release_withdraws_the_offer() {
    let (feed, updates, offers) = rig();
    feed.publish("0.15.0");
    updates.sweep().await;
    *feed.0.published.lock().unwrap() = Ok(None);

    updates.sweep().await;

    assert!(matches!(updates.state(), SelfUpdateState::Current { .. }));
    assert_eq!(offers.lock().unwrap().last(), Some(&None));
  }

  #[tokio::test]
  async fn installing_takes_the_waiting_build_and_a_second_click_does_nothing() {
    let (feed, updates, _offers) = rig();
    feed.publish("0.15.0");
    updates.sweep().await;

    assert!(updates.apply().await);
    assert!(!updates.apply().await);

    assert_eq!(*feed.0.installed.lock().unwrap(), vec!["0.15.0".to_owned()]);
  }

  #[tokio::test]
  async fn nothing_waiting_means_nothing_to_install() {
    let (_feed, updates, _offers) = rig();

    assert!(!updates.apply().await);
  }

  #[tokio::test]
  async fn a_download_that_stops_moving_is_given_up_on() {
    let received = AtomicU64::new(0);

    let gave_up = tokio::time::timeout(Duration::from_secs(5), stalled(&received, Duration::from_millis(20))).await;

    assert!(gave_up.is_ok());
  }

  #[tokio::test]
  async fn a_download_that_keeps_moving_is_left_alone() {
    let received = Arc::new(AtomicU64::new(0));
    let ticking = Arc::clone(&received);
    let feeder = tokio::spawn(async move {
      loop {
        ticking.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(5)).await;
      }
    });

    let gave_up = tokio::time::timeout(
      Duration::from_millis(300),
      stalled(&received, Duration::from_millis(60)),
    )
    .await;
    feeder.abort();

    assert!(gave_up.is_err());
  }

  async fn settles_on(updates: &SelfUpdate<Fake>, mut done: impl FnMut(&SelfUpdateState) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
      if done(&updates.state()) {
        return true;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
  }

  #[tokio::test]
  async fn a_launch_checks_on_its_own_with_no_window_asking() {
    let (feed, updates, offers) = rig();
    feed.publish("0.15.0");
    let (_online, watching) = watch::channel(true);

    tokio::spawn(Arc::clone(&updates).run(watching));

    assert!(settles_on(&updates, |state| version(state) == Some("0.15.0")).await);
    assert_eq!(*offers.lock().unwrap(), vec![Some("0.15.0".to_owned())]);
  }

  #[tokio::test]
  async fn coming_online_checks_at_once_instead_of_waiting_out_the_backoff() {
    let (feed, updates, _offers) = rig();
    feed.unreachable("no route to host");
    let (online, watching) = watch::channel(true);
    tokio::spawn(Arc::clone(&updates).run(watching));
    assert!(settles_on(&updates, |state| matches!(state, SelfUpdateState::Unavailable { .. })).await);

    online.send_replace(false);
    feed.publish("0.15.0");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
      matches!(updates.state(), SelfUpdateState::Unavailable { .. }),
      "going offline is no reason to look"
    );
    online.send_replace(true);

    assert!(
      settles_on(&updates, |state| version(state) == Some("0.15.0")).await,
      "the network coming up at login is checked well inside the first {}s retry",
      FIRST_RETRY.as_secs()
    );
  }

  #[test]
  fn a_build_is_announced_once_even_across_relaunches() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let announced = Announced::open(dir.path());

    assert!(announced.first("0.15.0"));
    assert!(
      !announced.first("0.15.0"),
      "a later sweep that finds the same build stays quiet"
    );
    assert!(
      !Announced::open(dir.path()).first("0.15.0"),
      "a relaunch that fetches the same build again stays quiet"
    );
    assert!(Announced::open(dir.path()).first("0.16.0"));
  }

  #[test]
  fn a_failing_feed_is_retried_soon_and_backs_off_to_the_normal_cadence() {
    assert_eq!(backoff(1), FIRST_RETRY);
    assert_eq!(backoff(2), FIRST_RETRY * 2);
    assert_eq!(backoff(40), EVERY);
  }
}
