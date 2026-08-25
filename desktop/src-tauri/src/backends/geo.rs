use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, SendError, Sender, channel},
  },
  thread,
};

use bridgething_companion::backend::{GeoAccuracy, GeoInbox, GeoProvider};

pub enum Command {
  Configure(GeoAccuracy),
  RequestAuthorization,
  StartUpdating,
  StopUpdating,
  RequestOnce,
  CancelOnce,
  Shutdown,
}

type Engine = fn(Arc<Shared>, Receiver<Command>);

pub struct Shared {
  inbox: Mutex<Option<Arc<GeoInbox>>>,
  usable: AtomicBool,
  watching: AtomicBool,
  one_shot: AtomicBool,
}

impl Shared {
  pub fn report(&self, deliver: impl FnOnce(&GeoInbox)) {
    let held = self.inbox.lock().unwrap().clone();
    if let Some(inbox) = held {
      deliver(&inbox);
    }
  }

  pub fn publish_authorization(&self, granted: bool) {
    self.usable.store(granted, Ordering::Relaxed);
    self.report(|inbox| inbox.on_authorization_change(granted));
  }

  #[cfg(any(target_os = "linux", target_os = "macos"))]
  pub fn watching(&self) -> bool {
    self.watching.load(Ordering::Relaxed)
  }

  pub fn set_watching(&self, watching: bool) {
    self.watching.store(watching, Ordering::Relaxed);
  }

  pub fn set_one_shot(&self, pending: bool) {
    self.one_shot.store(pending, Ordering::Relaxed);
  }

  pub fn take_one_shot(&self) -> bool {
    self.one_shot.swap(false, Ordering::Relaxed)
  }

  pub fn park(&self) {
    self.set_watching(false);
    self.set_one_shot(false);
  }
}

pub struct Locator {
  shared: Arc<Shared>,
  engine: Mutex<Option<Sender<Command>>>,
  run: Engine,
}

impl Locator {
  pub fn new(run: Engine) -> Self {
    Self {
      shared: Arc::new(Shared {
        inbox: Mutex::new(None),
        usable: AtomicBool::new(true),
        watching: AtomicBool::new(false),
        one_shot: AtomicBool::new(false),
      }),
      engine: Mutex::new(None),
      run,
    }
  }

  fn send(&self, command: Command) {
    let mut held = self.engine.lock().unwrap();
    let command = match held.as_ref() {
      Some(engine) => match engine.send(command) {
        Ok(()) => return,
        Err(SendError(command)) => command,
      },
      None => command,
    };

    let (tx, rx) = channel();
    let shared = Arc::clone(&self.shared);
    let run = self.run;
    match thread::Builder::new()
      .name("bridgething-geo".to_owned())
      .spawn(move || run(shared, rx))
    {
      Ok(_) => {
        let _ = tx.send(command);
        *held = Some(tx);
      }
      Err(error) => {
        *held = None;
        tracing::warn!(%error, "the location engine could not be started");
      }
    }
  }

  fn shutdown(&self) {
    if let Some(engine) = self.engine.lock().unwrap().take() {
      let _ = engine.send(Command::Shutdown);
    }
  }
}

impl GeoProvider for Locator {
  fn can_provide_location(&self) -> bool {
    self.shared.usable.load(Ordering::Relaxed)
  }

  fn start(&self, inbox: Arc<GeoInbox>) {
    *self.shared.inbox.lock().unwrap() = Some(inbox);
    self.send(Command::RequestAuthorization);
  }

  fn stop(&self) {
    self.shared.inbox.lock().unwrap().take();
    self.shutdown();
  }

  fn configure(&self, accuracy: GeoAccuracy) {
    self.send(Command::Configure(accuracy));
  }

  fn request_authorization(&self) {
    self.send(Command::RequestAuthorization);
  }

  fn start_updating(&self) {
    self.send(Command::StartUpdating);
  }

  fn stop_updating(&self) {
    self.send(Command::StopUpdating);
  }

  fn request_once(&self) {
    self.send(Command::RequestOnce);
  }

  fn cancel_once(&self) {
    self.send(Command::CancelOnce);
  }
}
