use std::{
  sync::{Arc, Mutex},
  thread,
  time::Duration,
};

use bridgething_companion::backend::{HostStateBackend, HostStateInbox};
use futures::StreamExt;
use tokio::sync::{oneshot, watch};
use zbus::{
  Connection, Proxy,
  proxy::SignalStream,
  zvariant::{OwnedFd, OwnedObjectPath},
};

use super::idle;

const LOGIN: &str = "org.freedesktop.login1";
const LOGIN_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const USER: &str = "org.freedesktop.login1.User";
const SCREENSAVERS: [(&str, &str); 2] = [
  ("org.freedesktop.ScreenSaver", "/org/freedesktop/ScreenSaver"),
  ("org.gnome.ScreenSaver", "/org/gnome/ScreenSaver"),
];
const DELAY_MAX: Duration = Duration::from_secs(5);
const DELAY_MARGIN: Duration = Duration::from_millis(500);

pub struct LinuxHostState {
  held: Mutex<Option<oneshot::Sender<()>>>,
  thresholds: watch::Sender<Vec<u32>>,
}

impl Default for LinuxHostState {
  fn default() -> Self {
    Self {
      held: Mutex::default(),
      thresholds: watch::Sender::new(Vec::new()),
    }
  }
}

impl HostStateBackend for LinuxHostState {
  fn start(&self, inbox: Arc<HostStateInbox>) {
    self.stop();

    let (stop, halted) = oneshot::channel();
    let thresholds = self.thresholds.subscribe();
    match thread::Builder::new()
      .name("bridgething-host-state".to_owned())
      .spawn(move || run(inbox, thresholds, halted))
    {
      Ok(_) => *self.held.lock().unwrap() = Some(stop),
      Err(error) => tracing::warn!(%error, "the host state watcher could not be started"),
    }
  }

  fn stop(&self) {
    self.held.lock().unwrap().take();
  }

  fn watch_idle(&self, thresholds_s: Vec<u32>) {
    self.thresholds.send_replace(thresholds_s);
  }
}

fn run(inbox: Arc<HostStateInbox>, thresholds: watch::Receiver<Vec<u32>>, halted: oneshot::Receiver<()>) {
  let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
    Ok(runtime) => runtime,
    Err(error) => {
      tracing::warn!(%error, "the host state watcher has no runtime; the desktop reads as present");
      inbox.on_away(false);
      return;
    }
  };
  runtime.block_on(async move {
    tokio::select! {
      _ = halted => {}
      _ = async { tokio::join!(away(Away::new(inbox.clone())), idle::watch(inbox, thresholds)) } => {}
    }
  });
}

struct Away {
  inbox: Arc<HostStateInbox>,
  locked: bool,
  sleeping: bool,
  said: Option<bool>,
}

impl Away {
  fn new(inbox: Arc<HostStateInbox>) -> Self {
    Self {
      inbox,
      locked: false,
      sleeping: false,
      said: None,
    }
  }

  fn report(&mut self) {
    let away = self.locked || self.sleeping;
    if self.said != Some(away) {
      self.said = Some(away);
      self.inbox.on_away(away);
    }
  }

  async fn sleep(&mut self, within: Duration) {
    self.sleeping = true;
    self.said = Some(true);
    let inbox = self.inbox.clone();
    let within_ms = u32::try_from(within.as_millis()).unwrap_or(u32::MAX);
    if let Err(error) = tokio::task::spawn_blocking(move || inbox.on_sleeping(within_ms)).await {
      tracing::warn!(%error, "the sleep report did not finish; sleep goes ahead");
    }
  }
}

async fn away(mut away: Away) {
  let logind = match Connection::system().await {
    Ok(bus) => logind(&bus, &mut away).await,
    Err(error) => Err(error),
  };
  let Err(error) = logind else { return };
  tracing::info!(%error, "logind has no session to watch; the screensaver stands in for the lock");
  if let Err(error) = screensaver(&mut away).await {
    tracing::warn!(%error, "no lock signal on this desktop; it reads as present");
    away.report();
  }
}

async fn logind(bus: &Connection, away: &mut Away) -> zbus::Result<()> {
  let manager = Proxy::new(bus, LOGIN, LOGIN_PATH, MANAGER).await?;
  let session = Proxy::new(bus, LOGIN, session(&manager).await?, SESSION).await?;
  let mut locks = session.receive_property_changed::<bool>("LockedHint").await;
  let mut sleeps = manager.receive_signal("PrepareForSleep").await?;
  let mut held = inhibit(&manager).await;
  let budget = delay_max(&manager).await.saturating_sub(DELAY_MARGIN);
  away.locked = session.get_property("LockedHint").await?;
  away.report();

  loop {
    tokio::select! {
      lock = locks.next() => {
        let Some(lock) = lock else { return Ok(()) };
        match lock.get().await {
          Ok(locked) => {
            away.locked = locked;
            away.report();
          }
          Err(error) => tracing::warn!(%error, "logind sent a lock edge that is not a lock state"),
        }
      }
      sleep = sleeps.next() => {
        let Some(sleep) = sleep else { return Ok(()) };
        match sleep.body().deserialize::<bool>() {
          Ok(true) => {
            away.sleep(budget).await;
            drop(held.take());
          }
          Ok(false) => {
            held = inhibit(&manager).await;
            away.sleeping = false;
            away.report();
          }
          Err(error) => tracing::warn!(%error, "logind sent a sleep edge that is not a sleep state"),
        }
      }
    }
  }
}

async fn session(manager: &Proxy<'_>) -> zbus::Result<OwnedObjectPath> {
  match display(manager).await {
    Ok(Some(path)) => Ok(path),
    _ => manager.call("GetSession", &("auto",)).await,
  }
}

async fn display(manager: &Proxy<'_>) -> zbus::Result<Option<OwnedObjectPath>> {
  let user: OwnedObjectPath = manager.call("GetUserByPID", &(std::process::id(),)).await?;
  let user = Proxy::new(manager.connection(), LOGIN, user, USER).await?;
  let (id, path): (String, OwnedObjectPath) = user.get_property("Display").await?;
  Ok((!id.is_empty()).then_some(path))
}

async fn delay_max(manager: &Proxy<'_>) -> Duration {
  match manager.get_property::<u64>("InhibitDelayMaxUSec").await {
    Ok(usec) => Duration::from_micros(usec),
    Err(error) => {
      tracing::info!(%error, "logind did not say how long sleep may be delayed; assuming its default");
      DELAY_MAX
    }
  }
}

async fn inhibit(manager: &Proxy<'_>) -> Option<OwnedFd> {
  match manager
    .call(
      "Inhibit",
      &(
        "sleep",
        "bridgething",
        "tell the car thing the desktop is away",
        "delay",
      ),
    )
    .await
  {
    Ok(inhibitor) => Some(inhibitor),
    Err(error) => {
      tracing::warn!(%error, "logind refused a sleep delay; the away edge may land after the desktop sleeps");
      None
    }
  }
}

async fn screensaver(away: &mut Away) -> zbus::Result<()> {
  let bus = Connection::session().await?;
  let mut found = Err(zbus::Error::InterfaceNotFound);
  for (service, path) in SCREENSAVERS {
    found = saver(&bus, service, path).await;
    if found.is_ok() {
      break;
    }
  }
  let (mut edges, active) = found?;
  away.locked = active;
  away.report();

  while let Some(edge) = edges.next().await {
    match edge.body().deserialize::<bool>() {
      Ok(active) => {
        away.locked = active;
        away.report();
      }
      Err(error) => tracing::warn!(%error, "the screensaver sent an edge that is not a lock state"),
    }
  }
  Ok(())
}

async fn saver(
  bus: &Connection,
  service: &'static str,
  path: &'static str,
) -> zbus::Result<(SignalStream<'static>, bool)> {
  let proxy = Proxy::new(bus, service, path, service).await?;
  let edges = proxy.receive_signal("ActiveChanged").await?;
  let active = proxy.call("GetActive", &()).await?;
  Ok((edges, active))
}

#[cfg(test)]
mod tests {
  use std::{os::unix::net::UnixStream, time::Duration};

  use bridgething_companion::backend::HostEdge;
  use tokio::{io::AsyncReadExt, sync::mpsc::UnboundedReceiver};
  use zbus::object_server::SignalEmitter;

  use super::*;

  const USER_PATH: &str = "/org/freedesktop/login1/user/_1000";
  const SESSION_PATH: &str = "/org/freedesktop/login1/session/_34";
  const ARMED: Duration = Duration::from_secs(10);
  const FAKE_DELAY_MAX: Duration = Duration::from_secs(2);

  struct FakeManager {
    inhibitors: Arc<Mutex<Vec<UnixStream>>>,
  }

  #[zbus::interface(name = "org.freedesktop.login1.Manager")]
  impl FakeManager {
    #[zbus(name = "GetUserByPID")]
    fn get_user_by_pid(&self, _pid: u32) -> OwnedObjectPath {
      OwnedObjectPath::try_from(USER_PATH).expect("a user path")
    }

    fn inhibit(&self, what: &str, _who: &str, _why: &str, mode: &str) -> zbus::fdo::Result<OwnedFd> {
      assert_eq!((what, mode), ("sleep", "delay"));
      let (kept, handed) = UnixStream::pair().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
      self.inhibitors.lock().unwrap().push(kept);
      Ok(OwnedFd::from(std::os::fd::OwnedFd::from(handed)))
    }

    #[zbus(property, name = "InhibitDelayMaxUSec")]
    fn inhibit_delay_max_usec(&self) -> u64 {
      u64::try_from(FAKE_DELAY_MAX.as_micros()).expect("a delay in range")
    }

    #[zbus(signal)]
    async fn prepare_for_sleep(emitter: &SignalEmitter<'_>, start: bool) -> zbus::Result<()>;
  }

  struct FakeUser;

  #[zbus::interface(name = "org.freedesktop.login1.User")]
  impl FakeUser {
    #[zbus(property)]
    fn display(&self) -> (String, OwnedObjectPath) {
      (
        "34".to_owned(),
        OwnedObjectPath::try_from(SESSION_PATH).expect("a session path"),
      )
    }
  }

  struct FakeSession {
    locked: bool,
  }

  #[zbus::interface(name = "org.freedesktop.login1.Session")]
  impl FakeSession {
    #[zbus(property)]
    fn locked_hint(&self) -> bool {
      self.locked
    }
  }

  struct FakeSaver {
    active: bool,
  }

  #[zbus::interface(name = "org.gnome.ScreenSaver")]
  impl FakeSaver {
    fn get_active(&self) -> bool {
      self.active
    }

    #[zbus(signal)]
    async fn active_changed(emitter: &SignalEmitter<'_>, active: bool) -> zbus::Result<()>;
  }

  async fn next(edges: &mut UnboundedReceiver<HostEdge>) -> HostEdge {
    tokio::time::timeout(ARMED, edges.recv())
      .await
      .expect("an edge arrives")
      .expect("the inbox stays open")
  }

  fn held(inhibitor: &UnixStream) -> bool {
    inhibitor.set_nonblocking(true).expect("a nonblocking inhibitor end");
    let mut byte = [0u8; 1];
    std::io::Read::read(&mut &*inhibitor, &mut byte).map_err(|error| error.kind())
      == Err(std::io::ErrorKind::WouldBlock)
  }

  async fn released(inhibitor: UnixStream) {
    inhibitor.set_nonblocking(true).expect("a nonblocking inhibitor end");
    let mut inhibitor = tokio::net::UnixStream::from_std(inhibitor).expect("an async inhibitor end");
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(ARMED, inhibitor.read(&mut byte))
      .await
      .expect("the inhibitor is released")
      .expect("a clean close");
    assert_eq!(read, 0, "logind only ever sees the inhibitor close");
  }

  async fn set_locked(bus: &Connection, locked: bool) {
    let session = bus
      .object_server()
      .interface::<_, FakeSession>(SESSION_PATH)
      .await
      .expect("the fake session");
    let mut fake = session.get_mut().await;
    fake.locked = locked;
    fake
      .locked_hint_changed(session.signal_emitter())
      .await
      .expect("a lock edge");
  }

  #[tokio::test]
  #[ignore = "needs a session bus: dbus-run-session -- cargo test -p bridgething-desktop"]
  async fn logind_lock_and_sleep_edges_read_as_away_and_sleep_waits_for_the_devices() {
    let inhibitors = Arc::new(Mutex::new(Vec::new()));
    let fake = zbus::connection::Builder::session()
      .expect("a session bus")
      .name(LOGIN)
      .expect("the logind name is free on a private bus")
      .serve_at(
        LOGIN_PATH,
        FakeManager {
          inhibitors: inhibitors.clone(),
        },
      )
      .expect("the manager object")
      .serve_at(USER_PATH, FakeUser)
      .expect("the user object")
      .serve_at(SESSION_PATH, FakeSession { locked: false })
      .expect("the session object")
      .build()
      .await
      .expect("a stand-in logind");

    let bus = Connection::session().await.expect("a session bus");
    let (inbox, mut edges) = HostStateInbox::channel();
    let watcher = {
      let inbox = inbox.clone();
      tokio::spawn(async move { logind(&bus, &mut Away::new(inbox)).await })
    };

    assert_eq!(next(&mut edges).await, HostEdge::Away(false));
    assert_eq!(inhibitors.lock().unwrap().len(), 1, "sleep is delayed from the start");

    set_locked(&fake, true).await;
    assert_eq!(next(&mut edges).await, HostEdge::Away(true));
    set_locked(&fake, false).await;
    assert_eq!(next(&mut edges).await, HostEdge::Away(false));

    let manager = fake
      .object_server()
      .interface::<_, FakeManager>(LOGIN_PATH)
      .await
      .expect("the fake manager");
    FakeManager::prepare_for_sleep(manager.signal_emitter(), true)
      .await
      .expect("a sleep edge");
    assert_eq!(next(&mut edges).await, HostEdge::Sleeping(1));
    let first = inhibitors.lock().unwrap().remove(0);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
      held(&first),
      "sleep went ahead before the devices took in the away report"
    );
    inbox.settle(1);
    released(first).await;

    FakeManager::prepare_for_sleep(manager.signal_emitter(), false)
      .await
      .expect("a resume edge");
    assert_eq!(next(&mut edges).await, HostEdge::Away(false));
    assert_eq!(
      inhibitors.lock().unwrap().len(),
      1,
      "the delay is taken again after resume"
    );

    FakeManager::prepare_for_sleep(manager.signal_emitter(), true)
      .await
      .expect("a second sleep edge");
    assert_eq!(next(&mut edges).await, HostEdge::Sleeping(2));
    let second = inhibitors.lock().unwrap().remove(0);
    let asleep_from = std::time::Instant::now();
    released(second).await;
    assert!(
      asleep_from.elapsed() < FAKE_DELAY_MAX,
      "a report that never settles held sleep past what logind allows: {:?}",
      asleep_from.elapsed()
    );

    watcher.abort();
  }

  #[tokio::test]
  #[ignore = "needs a session bus: dbus-run-session -- cargo test -p bridgething-desktop"]
  async fn the_screensaver_stands_in_for_the_lock_without_logind() {
    let saver = zbus::connection::Builder::session()
      .expect("a session bus")
      .name("org.gnome.ScreenSaver")
      .expect("the screensaver name is free on a private bus")
      .serve_at("/org/gnome/ScreenSaver", FakeSaver { active: true })
      .expect("the screensaver object")
      .build()
      .await
      .expect("a stand-in screensaver");

    let (inbox, mut edges) = HostStateInbox::channel();
    let watcher = tokio::spawn(async move { screensaver(&mut Away::new(inbox)).await });

    assert_eq!(next(&mut edges).await, HostEdge::Away(true));
    let fake = saver
      .object_server()
      .interface::<_, FakeSaver>("/org/gnome/ScreenSaver")
      .await
      .expect("the fake screensaver");
    FakeSaver::active_changed(fake.signal_emitter(), false)
      .await
      .expect("an unlock edge");
    assert_eq!(next(&mut edges).await, HostEdge::Away(false));

    watcher.abort();
  }
}
