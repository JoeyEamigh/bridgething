use std::{
  collections::{HashMap, HashSet},
  sync::{Arc, Mutex},
};

use bridgething_gateway::{OutboundLink, OutboundLinkExt};
use libbridgething::{
  CompanionAuthorityScope, PlaybackState, PlayerState,
  gateway::{
    AuthorityClaim, AuthorityRelease, GatewayToBridgeAuthorityMsgEvent, GatewayToBridgePlayerMsgEvent, PlaybackTargets,
    QueueSnapshot,
  },
};
use tokio::{
  sync::{broadcast, mpsc, oneshot},
  task::JoinHandle,
};

use crate::provider::PlayerTransport;

const NOW_PLAYING_SCOPES: [CompanionAuthorityScope; 2] = [
  CompanionAuthorityScope::NowPlayingPlayback,
  CompanionAuthorityScope::NowPlayingMetadata,
];

enum Op {
  Player {
    source: String,
    snapshot: Box<PlayerState>,
    app_bundle: String,
    has_item: bool,
    source_owns_volume: bool,
  },
  Queue {
    source: String,
    queue: QueueSnapshot,
  },
  Targets {
    source: String,
    targets: PlaybackTargets,
  },
  Clear {
    source: String,
  },
  Reconnect {
    done: Option<oneshot::Sender<()>>,
  },
}

#[derive(Clone)]
pub struct NowPlayingSink {
  tx: mpsc::UnboundedSender<Op>,
}

impl NowPlayingSink {
  pub fn submit_player(
    &self,
    source: &str,
    snapshot: PlayerState,
    app_bundle: &str,
    has_item: bool,
    source_owns_volume: bool,
  ) {
    let _ = self.tx.send(Op::Player {
      source: source.to_owned(),
      snapshot: Box::new(snapshot),
      app_bundle: app_bundle.to_owned(),
      has_item,
      source_owns_volume,
    });
  }

  pub fn submit_queue(&self, source: &str, queue: QueueSnapshot) {
    let _ = self.tx.send(Op::Queue {
      source: source.to_owned(),
      queue,
    });
  }

  pub fn submit_targets(&self, source: &str, targets: PlaybackTargets) {
    let _ = self.tx.send(Op::Targets {
      source: source.to_owned(),
      targets,
    });
  }

  pub fn clear_source(&self, source: &str) {
    let _ = self.tx.send(Op::Clear {
      source: source.to_owned(),
    });
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorityHold {
  pub scopes: Vec<CompanionAuthorityScope>,
  pub app_bundle: Option<String>,
}

#[derive(Default)]
struct Held {
  scopes: HashSet<CompanionAuthorityScope>,
  app_bundle: Option<String>,
}

#[derive(Default)]
struct SourceState {
  snapshot: Option<PlayerState>,
  app_bundle: String,
  has_item: bool,
  source_owns_volume: bool,
  queue: Option<QueueSnapshot>,
  targets: Option<PlaybackTargets>,
  seq: u64,
}

pub struct NowPlayingHub {
  tx: mpsc::UnboundedSender<Op>,
  current: Arc<Mutex<Option<String>>>,
  held: Arc<Mutex<Held>>,
  transports: Mutex<HashMap<String, Arc<dyn PlayerTransport>>>,
  arbitrated: broadcast::Sender<Option<PlayerState>>,
  pending: Mutex<Option<(Arbiter, mpsc::UnboundedReceiver<Op>)>>,
  task: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for NowPlayingHub {
  fn drop(&mut self) {
    if let Some(task) = self.task.lock().unwrap().take() {
      task.abort();
    }
  }
}

impl NowPlayingHub {
  pub fn init(link: Arc<dyn OutboundLink>, host_owns_volume: bool) -> Self {
    let (tx, rx) = mpsc::unbounded_channel();
    let current = Arc::new(Mutex::new(None));
    let held = Arc::new(Mutex::new(Held::default()));
    let arbitrated = broadcast::channel(32).0;
    let actor = Arbiter {
      link,
      host_owns_volume,
      current: current.clone(),
      held: held.clone(),
      arbitrated: arbitrated.clone(),
      sources: HashMap::new(),
      seq_counter: 0,
    };
    Self {
      tx,
      current,
      held,
      transports: Mutex::new(HashMap::new()),
      arbitrated,
      pending: Mutex::new(Some((actor, rx))),
      task: Mutex::new(None),
    }
  }

  pub fn start(&self) {
    let Some((mut actor, mut rx)) = self.pending.lock().unwrap().take() else {
      return;
    };
    let task = tokio::spawn(async move {
      while let Some(op) = rx.recv().await {
        actor.handle(op).await;
      }
    });
    *self.task.lock().unwrap() = Some(task);
  }

  pub fn arbitrated(&self) -> broadcast::Receiver<Option<PlayerState>> {
    self.arbitrated.subscribe()
  }

  pub fn sink(&self) -> NowPlayingSink {
    NowPlayingSink { tx: self.tx.clone() }
  }

  pub fn on_connect(&self) {
    let _ = self.tx.send(Op::Reconnect { done: None });
  }

  pub async fn resync(&self) {
    let (done, wait) = oneshot::channel();
    if self.tx.send(Op::Reconnect { done: Some(done) }).is_err() {
      return;
    }
    let _ = wait.await;
  }

  pub fn register(&self, source: &str, transport: Arc<dyn PlayerTransport>) {
    self.transports.lock().unwrap().insert(source.to_owned(), transport);
  }

  pub fn unregister(&self, source: &str) {
    self.transports.lock().unwrap().remove(source);
  }

  pub fn current_source(&self) -> Option<String> {
    self.current.lock().unwrap().clone()
  }

  pub fn current_transport(&self) -> Option<Arc<dyn PlayerTransport>> {
    let current = self.current.lock().unwrap().clone()?;
    self.transports.lock().unwrap().get(&current).cloned()
  }

  pub fn authority(&self) -> AuthorityHold {
    let held = self.held.lock().unwrap();
    AuthorityHold {
      scopes: held.scopes.iter().copied().collect(),
      app_bundle: held.app_bundle.clone(),
    }
  }
}

struct Arbiter {
  link: Arc<dyn OutboundLink>,
  host_owns_volume: bool,
  current: Arc<Mutex<Option<String>>>,
  held: Arc<Mutex<Held>>,
  arbitrated: broadcast::Sender<Option<PlayerState>>,
  sources: HashMap<String, SourceState>,
  seq_counter: u64,
}

impl Arbiter {
  fn current(&self) -> Option<String> {
    self.current.lock().unwrap().clone()
  }

  fn set_current(&self, id: Option<String>) {
    let mut held = self.current.lock().unwrap();
    if *held != id {
      tracing::debug!(
        from = ?held.as_deref(),
        to = ?id.as_deref(),
        sources = ?self.sources.keys().collect::<Vec<_>>(),
        "the audible source changed"
      );
    }
    *held = id;
  }

  async fn handle(&mut self, op: Op) {
    match op {
      Op::Player {
        source,
        snapshot,
        app_bundle,
        has_item,
        source_owns_volume,
      } => {
        self.seq_counter += 1;
        let state = self.sources.entry(source).or_default();
        state.snapshot = Some(*snapshot);
        state.app_bundle = app_bundle;
        state.has_item = has_item;
        state.source_owns_volume = source_owns_volume;
        state.seq = self.seq_counter;
        self.emit_arbitrated().await;
      }
      Op::Queue { source, queue } => {
        self.sources.entry(source.clone()).or_default().queue = Some(queue.clone());
        let current = self.current();
        if current.is_none() || current.as_deref() == Some(&source) {
          let _ = self
            .link
            .event(GatewayToBridgePlayerMsgEvent::QueueChanged(queue))
            .await;
        }
      }
      Op::Targets { source, targets } => {
        self.sources.entry(source.clone()).or_default().targets = Some(targets.clone());
        let current = self.current();
        if current.is_none() || current.as_deref() == Some(&source) {
          let _ = self
            .link
            .event(GatewayToBridgePlayerMsgEvent::TargetsChanged(targets))
            .await;
        }
      }
      Op::Clear { source } => {
        tracing::debug!(%source, "a source withdrew itself from arbitration");
        self.sources.remove(&source);
        if self.current().as_deref() == Some(&source) {
          self.set_current(None);
          self.emit_arbitrated().await;
        }
      }
      Op::Reconnect { done } => {
        {
          let mut held = self.held.lock().unwrap();
          held.scopes.clear();
          held.app_bundle = None;
        }
        self.reemit_current().await;
        if let Some(done) = done {
          let _ = done.send(());
        }
      }
    }
  }

  fn pick_current(&self) -> Option<String> {
    if self.sources.is_empty() {
      return None;
    }
    tracing::trace!(
      current = ?self.current().as_deref(),
      sources = ?self
        .sources
        .iter()
        .map(|(id, state)| (
          id.as_str(),
          state.has_item,
          state.snapshot.as_ref().map(|snapshot| snapshot.playback.state),
          state.seq
        ))
        .collect::<Vec<_>>(),
      "arbitrating"
    );
    let playing: Vec<(&String, &SourceState)> = self
      .sources
      .iter()
      .filter(|(_, s)| {
        s.has_item
          && s
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.playback.state == PlaybackState::Playing)
      })
      .collect();
    if let Some((id, _)) = playing.into_iter().max_by_key(|(_, s)| s.seq) {
      return Some(id.clone());
    }
    if let Some(held) = self.current()
      && self.sources.get(&held).is_some_and(|state| state.has_item)
    {
      return Some(held);
    }
    self.sources.iter().max_by_key(|(_, s)| s.seq).map(|(id, _)| id.clone())
  }

  async fn emit_arbitrated(&mut self) {
    let prev = self.current();
    let next = self.pick_current();
    self.set_current(next.clone());
    let Some(next) = next else {
      self.release_all().await;
      let _ = self.arbitrated.send(None);
      return;
    };
    let Some(state) = self.sources.get(&next) else {
      self.release_all().await;
      let _ = self.arbitrated.send(None);
      return;
    };
    let (app_bundle, has_item, source_owns_volume) =
      (state.app_bundle.clone(), state.has_item, state.source_owns_volume);
    let snapshot = state.snapshot.clone();
    let queue = state.queue.clone();
    let targets = state.targets.clone();
    if has_item {
      self.claim(&app_bundle, source_owns_volume).await;
    } else {
      self.release_all().await;
    }
    if let Some(snapshot) = snapshot {
      let _ = self.arbitrated.send(Some(snapshot.clone()));
      let _ = self
        .link
        .event(GatewayToBridgePlayerMsgEvent::Snapshot(Box::new(snapshot)))
        .await;
    }
    if let Some(prev) = prev
      && prev != next
    {
      let _ = self
        .link
        .event(GatewayToBridgePlayerMsgEvent::QueueChanged(queue.unwrap_or(
          QueueSnapshot {
            order: Vec::new(),
            items: Vec::new(),
          },
        )))
        .await;
      let _ = self
        .link
        .event(GatewayToBridgePlayerMsgEvent::TargetsChanged(
          targets.unwrap_or_default(),
        ))
        .await;
    }
  }

  async fn reemit_current(&mut self) {
    let next = match self.current() {
      Some(id) => id,
      None => {
        let Some(picked) = self.pick_current() else { return };
        self.set_current(Some(picked.clone()));
        picked
      }
    };
    let Some(state) = self.sources.get(&next) else { return };
    let (app_bundle, has_item, source_owns_volume) =
      (state.app_bundle.clone(), state.has_item, state.source_owns_volume);
    let snapshot = state.snapshot.clone();
    let queue = state.queue.clone();
    let targets = state.targets.clone();
    if has_item {
      self.claim(&app_bundle, source_owns_volume).await;
    }
    if let Some(snapshot) = snapshot {
      let _ = self.arbitrated.send(Some(snapshot.clone()));
      let _ = self
        .link
        .event(GatewayToBridgePlayerMsgEvent::Snapshot(Box::new(snapshot)))
        .await;
    }
    if let Some(queue) = queue {
      let _ = self
        .link
        .event(GatewayToBridgePlayerMsgEvent::QueueChanged(queue))
        .await;
    }
    if let Some(targets) = targets {
      let _ = self
        .link
        .event(GatewayToBridgePlayerMsgEvent::TargetsChanged(targets))
        .await;
    }
  }

  async fn claim(&mut self, app_bundle: &str, source_owns_volume: bool) {
    let volume = source_owns_volume || self.host_owns_volume;
    let bundle_changed = self.held.lock().unwrap().app_bundle.as_deref() != Some(app_bundle);
    let mut want = NOW_PLAYING_SCOPES.to_vec();
    if volume {
      want.push(CompanionAuthorityScope::Volume);
    }
    for scope in want {
      if !self.held.lock().unwrap().scopes.contains(&scope) || bundle_changed {
        let claimed = self
          .link
          .event(GatewayToBridgeAuthorityMsgEvent::Claim(AuthorityClaim {
            scope,
            app_bundle: Some(app_bundle.to_owned()),
          }))
          .await
          .is_ok();
        if claimed {
          self.held.lock().unwrap().scopes.insert(scope);
        }
      }
    }
    let dropped_volume = !volume
      && self
        .held
        .lock()
        .unwrap()
        .scopes
        .remove(&CompanionAuthorityScope::Volume);
    if dropped_volume {
      let _ = self
        .link
        .event(GatewayToBridgeAuthorityMsgEvent::Release(AuthorityRelease {
          scope: CompanionAuthorityScope::Volume,
        }))
        .await;
    }
    self.held.lock().unwrap().app_bundle = Some(app_bundle.to_owned());
  }

  async fn release_all(&mut self) {
    let scopes: Vec<CompanionAuthorityScope> = {
      let mut held = self.held.lock().unwrap();
      held.app_bundle = None;
      held.scopes.drain().collect()
    };
    for scope in scopes {
      let _ = self
        .link
        .event(GatewayToBridgeAuthorityMsgEvent::Release(AuthorityRelease { scope }))
        .await;
    }
  }
}
