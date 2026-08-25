pub(crate) mod daemon_swap;
mod patch;
mod range_proxy;
mod slots;
mod staging;
mod swupdate;
mod wakeword_swap;
mod webapp_swap;

use std::{
  collections::BTreeSet,
  future::Future,
  path::PathBuf,
  pin::Pin,
  sync::Arc,
  time::{Duration, Instant},
};

use libbridgething::{
  OtaError, OtaErrorCode, OtaFinished, OtaKind, OtaPhase, OtaProgress, PeerCompanionStatus, WebappError, WebappInfo,
  gateway::{
    BridgeToGatewaySystemMsgEvent, BridgeToGatewayTransferMsgEvent, OtaBegin, OtaBeginAck, OtaBeginRejected, OtaPatch,
    TransferAck,
  },
};
pub use range_proxy::RangeProxy;
use staging::StagedPiece;
use tokio::{
  sync::{mpsc, oneshot, watch},
  task::JoinHandle,
};
pub use wakeword_swap::{
  installed_version as wakeword_model_version, retire_superseded as retire_superseded_wakeword_model,
};

use crate::{
  asset::AssetCache,
  bluetooth::{Address, GatewayMan},
  peer::{PeerSnapshot, PeerTracker},
  transfer::{
    BeginOutcome, ChunkOutcome, ChunkedTransfer, TransferError,
    sinks::{AckPolicy, ForwardStream, TransferEvent, TransferSinks},
  },
};

pub type OtaEventTx = mpsc::Sender<BridgeToGatewaySystemMsgEvent>;

pub type WakewordReload = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub type InstalledWebappApply = Arc<
  dyn Fn(PathBuf, Option<String>) -> Pin<Box<dyn Future<Output = Result<WebappInfo, WebappError>> + Send>>
    + Send
    + Sync,
>;

#[derive(Debug)]
enum Command {
  Begin {
    req: OtaBegin,
    peer: Option<Address>,
    ack: oneshot::Sender<Result<OtaBeginAck, OtaBeginRejected>>,
  },
  Abandon {
    update_id: String,
  },
  Activate {
    expected: Vec<String>,
  },
  Cancel,
  HoldFailure {
    peer: Address,
    update_id: String,
    code: OtaErrorCode,
    msg: String,
  },
  WriteFinished,
  StageFinished {
    result: Result<StagedPiece, WriteError>,
    peer: Option<Address>,
  },
}

pub type TerminatorFn = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
pub struct OtaTerminators {
  pub reboot: TerminatorFn,
  pub restart_self: TerminatorFn,
}

#[derive(Clone)]
pub struct OtaOrchestrator {
  cmd_tx: mpsc::Sender<Command>,
}

impl std::fmt::Debug for OtaOrchestrator {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("OtaOrchestrator").finish_non_exhaustive()
  }
}

impl OtaOrchestrator {
  #[allow(clippy::too_many_arguments)]
  pub fn spawn(
    transfers: ChunkedTransfer,
    events_tx: OtaEventTx,
    gateway_man: GatewayMan,
    terminators: OtaTerminators,
    range_proxy: RangeProxy,
    peers: PeerTracker,
    installed_apply: InstalledWebappApply,
    wakeword_reload: WakewordReload,
    sinks: TransferSinks,
    assets: AssetCache,
  ) -> (Self, JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let peer_watch = peers.watch_snapshot();
    let failure_watch = peers.watch_snapshot();
    let actor = OtaActor {
      transfers,
      events_tx,
      gateway_man,
      terminators,
      range_proxy,
      peers,
      peer_watch,
      failure_watch,
      installed_apply,
      wakeword_reload,
      sinks,
      assets,
      self_tx: cmd_tx.clone(),
      cmd_rx,
      state: OtaState::Idle,
      last_streaming_emit_at: None,
      last_streaming_percent: None,
      staged: Vec::new(),
      staged_peer: None,
      unsent_failure: None,
    };
    let handle = tokio::spawn(actor.run());
    (Self { cmd_tx }, handle)
  }

  pub async fn begin(&self, req: OtaBegin, peer: Option<Address>) -> Result<OtaBeginAck, OtaBeginRejected> {
    let (ack, rx) = oneshot::channel();
    if self.cmd_tx.send(Command::Begin { req, peer, ack }).await.is_err() {
      return Err(OtaBeginRejected {
        reason: "ota orchestrator mailbox closed".into(),
      });
    }
    rx.await.unwrap_or_else(|_| {
      Err(OtaBeginRejected {
        reason: "ota orchestrator dropped reply".into(),
      })
    })
  }

  pub async fn abandon(&self, update_id: String) {
    if let Err(err) = self.cmd_tx.send(Command::Abandon { update_id }).await {
      tracing::error!(?err, "ota orchestrator mailbox closed; dropping OtaAbandon");
    }
  }

  pub async fn activate(&self, expected: Vec<String>) {
    if let Err(err) = self.cmd_tx.send(Command::Activate { expected }).await {
      tracing::error!(?err, "ota orchestrator mailbox closed; dropping OtaActivate");
    }
  }

  pub async fn cancel(&self) {
    if let Err(err) = self.cmd_tx.send(Command::Cancel).await {
      tracing::error!(?err, "ota orchestrator mailbox closed; dropping CancelUpdate");
    }
  }
}

#[derive(Debug, Clone, Default)]
struct BeginMeta {
  provenance: Option<String>,
  version: Option<String>,
}

enum OtaState {
  Idle,
  Streaming {
    kind: OtaKind,
    update_id: String,
    expected_size: u64,
    peer: Option<Address>,
    transfer_id: uuid::Uuid,
    stream_rx: ForwardStream,
    patch: Option<OtaPatch>,
    begin_meta: BeginMeta,
  },
  Writing {
    kind: OtaKind,
    update_id: String,
    cancel_tx: watch::Sender<bool>,
    peer: Option<Address>,
  },
}

impl OtaState {
  fn pinned_peer(&self) -> Option<Option<Address>> {
    match self {
      OtaState::Idle => None,
      OtaState::Streaming { peer, .. } | OtaState::Writing { peer, .. } => Some(*peer),
    }
  }
}

const STREAMING_PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(100);

struct OtaActor {
  transfers: ChunkedTransfer,
  events_tx: OtaEventTx,
  gateway_man: GatewayMan,
  terminators: OtaTerminators,
  range_proxy: RangeProxy,
  peers: PeerTracker,
  peer_watch: watch::Receiver<crate::peer::PeerSnapshot>,
  failure_watch: watch::Receiver<crate::peer::PeerSnapshot>,
  installed_apply: InstalledWebappApply,
  wakeword_reload: WakewordReload,
  sinks: TransferSinks,
  assets: AssetCache,
  self_tx: mpsc::Sender<Command>,
  cmd_rx: mpsc::Receiver<Command>,
  state: OtaState,
  last_streaming_emit_at: Option<Instant>,
  last_streaming_percent: Option<u8>,
  staged: Vec<StagedPiece>,
  staged_peer: Option<Address>,
  unsent_failure: Option<UnsentFailure>,
}

struct UnsentFailure {
  peer: Address,
  update_id: String,
  code: OtaErrorCode,
  msg: String,
}

impl OtaActor {
  async fn run(mut self) {
    tracing::info!("ota orchestrator started");
    staging::sweep_orphans().await;
    wakeword_swap::sweep_orphans().await;
    daemon_swap::record_running_version().await;
    loop {
      enum Step {
        Cmd(Option<Command>),
        Stream(Option<TransferEvent>),
        PeerLost,
        FailurePeerBack,
      }
      let step = {
        let cmd_rx = &mut self.cmd_rx;
        let state = &mut self.state;
        let peer_watch = &mut self.peer_watch;
        let failure_watch = &mut self.failure_watch;
        let failure_peer = self.unsent_failure.as_ref().map(|held| held.peer);
        let streaming_peer = match state {
          OtaState::Streaming { peer: Some(addr), .. } => Some(*addr),
          _ => None,
        };
        tokio::select! {
          cmd = cmd_rx.recv() => Step::Cmd(cmd),
          ev = async {
            match state {
              OtaState::Streaming { stream_rx, .. } => stream_rx.recv().await,
              _ => std::future::pending().await,
            }
          } => Step::Stream(ev),
          _ = async {
            match streaming_peer {
              Some(addr) => loop {
                if peer_watch.changed().await.is_err() {
                  std::future::pending::<()>().await;
                }
                if !Self::peer_link_alive(&peer_watch.borrow(), &addr) {
                  return;
                }
              },
              None => std::future::pending().await,
            }
          } => Step::PeerLost,
          _ = async {
            match failure_peer {
              Some(addr) => loop {
                if failure_watch.changed().await.is_err() {
                  std::future::pending::<()>().await;
                }
                if Self::peer_link_alive(&failure_watch.borrow(), &addr) {
                  return;
                }
              },
              None => std::future::pending().await,
            }
          } => Step::FailurePeerBack,
        }
      };
      match step {
        Step::Cmd(None) => break,
        Step::Cmd(Some(cmd)) => match cmd {
          Command::Begin { req, peer, ack } => self.handle_begin(req, peer, ack).await,
          Command::Abandon { update_id } => self.handle_abandon(update_id).await,
          Command::Activate { expected } => self.handle_activate(expected).await,
          Command::Cancel => self.handle_cancel().await,
          Command::HoldFailure {
            peer,
            update_id,
            code,
            msg,
          } => {
            send_error(&self.events_tx, code, msg.clone(), Some(update_id.clone()), false).await;
            self.unsent_failure = Some(UnsentFailure {
              peer,
              update_id,
              code,
              msg,
            });
          }
          Command::WriteFinished => {
            self.state = OtaState::Idle;
            self.range_proxy.deactivate().await;
          }
          Command::StageFinished { result, peer } => self.handle_stage_finished(result, peer).await,
        },
        Step::Stream(None) => {
          tracing::warn!("ota fragment sink unbound mid-stream; returning to idle");
          self.state = OtaState::Idle;
          self.range_proxy.deactivate().await;
        }
        Step::Stream(Some(ev)) => self.handle_stream_event(ev).await,
        Step::PeerLost => {
          if let OtaState::Streaming {
            update_id, transfer_id, ..
          } = &self.state
          {
            tracing::warn!(%update_id, "pinned peer disconnected mid-stream; partial retained for resume");
            self.sinks.unbind(*transfer_id);
            self.state = OtaState::Idle;
            self.range_proxy.deactivate().await;
          }
        }
        Step::FailurePeerBack => {
          if let Some(held) = self.unsent_failure.take() {
            tracing::info!(peer = %held.peer, update_id = %held.update_id, "replaying the failure its peer missed");
            send_error(&self.events_tx, held.code, held.msg, Some(held.update_id), true).await;
          }
        }
      }
    }
    tracing::info!("ota orchestrator exiting");
  }

  fn peer_link_alive(snapshot: &PeerSnapshot, addr: &Address) -> bool {
    snapshot
      .peers
      .get(addr)
      .is_some_and(|p| !matches!(p.companion, PeerCompanionStatus::None))
  }

  async fn handle_begin(
    &mut self,
    req: OtaBegin,
    peer: Option<Address>,
    ack: oneshot::Sender<Result<OtaBeginAck, OtaBeginRejected>>,
  ) {
    if self.unsent_failure.as_ref().is_some_and(|held| Some(held.peer) == peer) {
      self.unsent_failure = None;
    }
    if let OtaState::Writing { update_id, .. } = &self.state {
      let _ = ack.send(Err(OtaBeginRejected {
        reason: format!("ota write of {update_id} in progress; cancel or wait first"),
      }));
      return;
    }

    let pinned = match self.state.pinned_peer() {
      Some(p) => Some(p),
      None if !self.staged.is_empty() => Some(self.staged_peer),
      None => None,
    };
    if let Some(pinned) = pinned
      && pinned != peer
    {
      let orphaned = self.state.pinned_peer().is_none()
        && !self.staged.is_empty()
        && pinned.is_none_or(|addr| !Self::peer_link_alive(&self.peer_watch.borrow(), &addr));
      if orphaned {
        tracing::warn!(
          pieces = self.staged.len(),
          "staged batch's companion is gone; discarding it for the incoming push"
        );
        for piece in std::mem::take(&mut self.staged) {
          staging::discard(&piece).await;
          let _ = self.transfers.abandon(piece.update_id.clone()).await;
        }
        self.staged_peer = None;
      } else {
        let _ = ack.send(Err(OtaBeginRejected {
          reason: "ota in progress, pinned to a different companion".into(),
        }));
        return;
      }
    }

    if matches!(req.kind, OtaKind::Image) && !self.staged.is_empty() {
      let _ = ack.send(Err(OtaBeginRejected {
        reason: "bandaid updates staged; activate or abandon them before an image OTA".into(),
      }));
      return;
    }

    if let OtaState::Streaming {
      update_id: existing,
      transfer_id: prior_transfer,
      ..
    } = &self.state
    {
      if existing != &req.update_id {
        tracing::info!(
          prior = %existing,
          new = %req.update_id,
          "OtaBegin for new update_id during streaming; abandoning prior partial",
        );
        let _ = self.transfers.abandon(existing.clone()).await;
        self.range_proxy.deactivate().await;
      }
      self.sinks.unbind(*prior_transfer);
      self.state = OtaState::Idle;
    }

    let Some(expected_sha256) = req.transfer.sha256.clone() else {
      let _ = ack.send(Err(OtaBeginRejected {
        reason: "ota transfer ref must carry sha256".into(),
      }));
      return;
    };

    if req.patch.is_some() && !matches!(req.kind, OtaKind::Daemon) {
      let _ = ack.send(Err(OtaBeginRejected {
        reason: "patch delta is only supported for daemon updates".into(),
      }));
      return;
    }

    if matches!(req.kind, OtaKind::WakewordModel) && wakeword_swap::target().is_none() {
      let _ = ack.send(Err(OtaBeginRejected {
        reason: "this build has nowhere to install a wake word model".into(),
      }));
      return;
    }

    if req.provenance.is_some() && !matches!(req.kind, OtaKind::InstalledWebapp) {
      let _ = ack.send(Err(OtaBeginRejected {
        reason: "provenance is only recorded for installed-webapp updates".into(),
      }));
      return;
    }

    if let Some(value) = req.provenance.as_deref()
      && value.len() > libbridgething::WEBAPP_PROVENANCE_MAX_LEN
    {
      let _ = ack.send(Err(OtaBeginRejected {
        reason: format!("provenance exceeds {} bytes", libbridgething::WEBAPP_PROVENANCE_MAX_LEN),
      }));
      return;
    }

    let kind = req.kind;
    let target_dir = match kind {
      OtaKind::Image | OtaKind::InstalledWebapp | OtaKind::WakewordModel => None,
      OtaKind::Daemon | OtaKind::BuiltinWebapp => {
        if crate::paths::is_on_device() {
          Some(crate::paths::bandaid_transfers_dir())
        } else {
          None
        }
      }
    };
    let expected_size = req.transfer.total_size as u64;

    if let Some(piece) = self.staged.iter().find(|p| p.update_id == req.update_id) {
      tracing::info!(update_id = %piece.update_id, kind = ?piece.kind, "OtaBegin for an already-staged piece; nothing to transfer");
      let _ = ack.send(Ok(OtaBeginAck {
        resume_from_offset: expected_size as u32,
      }));
      emit_progress(&self.events_tx, OtaPhase::Streaming, 100, None).await;
      emit_progress(&self.events_tx, OtaPhase::Verifying, 100, None).await;
      emit_progress(&self.events_tx, OtaPhase::Writing, 100, None).await;
      return;
    }

    if target_dir.is_none()
      && let Err(err) = self.assets.reserve_disk(expected_size).await
    {
      tracing::warn!(?err, update_id = %req.update_id, "ota: asset cache reserve_disk failed; proceeding");
    }
    let result = self
      .transfers
      .begin(
        req.update_id.clone(),
        expected_size,
        Some(expected_sha256),
        target_dir.clone(),
      )
      .await;
    match result {
      Ok(BeginOutcome::AlreadyComplete { path }) => {
        if let Some(dir) = &target_dir {
          let free = crate::paths::partition_free_bytes(dir);
          let needed = bandaid_bytes_needed(expected_size, expected_size, req.patch.as_ref());
          if free < needed {
            let _ = ack.send(Err(OtaBeginRejected {
              reason: format!("insufficient bandaid space: {free} bytes free, {needed} needed"),
            }));
            return;
          }
        }
        tracing::info!(update_id = %req.update_id, ?kind, "OtaBegin found a verified retained artifact; skipping transfer");
        let update_id = req.update_id.clone();
        let _ = ack.send(Ok(OtaBeginAck {
          resume_from_offset: expected_size as u32,
        }));
        emit_progress(&self.events_tx, OtaPhase::Streaming, 100, None).await;
        emit_progress(&self.events_tx, OtaPhase::Verifying, 100, None).await;
        self.last_streaming_emit_at = None;
        self.last_streaming_percent = None;
        if matches!(kind, OtaKind::Image) {
          self.range_proxy.activate(update_id.clone(), peer).await;
        }
        let begin_meta = BeginMeta {
          provenance: req.provenance.clone(),
          version: req.version.clone(),
        };
        self
          .spawn_write(kind, update_id, peer, path, req.patch.clone(), begin_meta)
          .await;
      }
      Ok(BeginOutcome::Resume {
        offset: resume_from_offset,
      }) => {
        if let Some(dir) = &target_dir {
          let free = crate::paths::partition_free_bytes(dir);
          let needed = bandaid_bytes_needed(expected_size, resume_from_offset, req.patch.as_ref());
          if free < needed {
            let _ = ack.send(Err(OtaBeginRejected {
              reason: format!("insufficient bandaid space: {free} bytes free, {needed} needed"),
            }));
            return;
          }
        }
        let resume_percent = phase_percent(resume_from_offset, expected_size);
        emit_progress(&self.events_tx, OtaPhase::Streaming, resume_percent, None).await;
        self.last_streaming_emit_at = Some(Instant::now());
        self.last_streaming_percent = Some(resume_percent);
        let update_id = req.update_id.clone();
        if matches!(kind, OtaKind::Image) {
          self.range_proxy.activate(update_id.clone(), peer).await;
        }
        let stream_rx = self.sinks.bind_forward(req.transfer.id, AckPolicy::OnDrain);
        self.sinks.seed_forward_ack(req.transfer.id, resume_from_offset as u32);
        self.state = OtaState::Streaming {
          kind,
          update_id,
          expected_size,
          peer,
          transfer_id: req.transfer.id,
          stream_rx,
          patch: req.patch.clone(),
          begin_meta: BeginMeta {
            provenance: req.provenance.clone(),
            version: req.version.clone(),
          },
        };
        let _ = ack.send(Ok(OtaBeginAck {
          resume_from_offset: resume_from_offset as u32,
        }));
      }
      Err(err) => {
        let _ = ack.send(Err(OtaBeginRejected {
          reason: format!("transfer begin failed: {err}"),
        }));
      }
    }
  }

  async fn send_ack(&self, peer: Option<Address>, transfer_id: uuid::Uuid, received: u32) {
    if let Some(address) = peer {
      self
        .gateway_man
        .send_event(
          address,
          BridgeToGatewayTransferMsgEvent::Ack(TransferAck { transfer_id, received }),
        )
        .await;
    }
  }

  async fn handle_stream_event(&mut self, event: TransferEvent) {
    let (kind, current_id, expected_size, peer, transfer_id, patch, begin_meta) = match &self.state {
      OtaState::Streaming {
        kind,
        update_id,
        expected_size,
        peer,
        transfer_id,
        patch,
        begin_meta,
        ..
      } => (
        *kind,
        update_id.clone(),
        *expected_size,
        *peer,
        *transfer_id,
        patch.clone(),
        begin_meta.clone(),
      ),
      _ => return,
    };

    let (offset, bytes) = match event {
      TransferEvent::Fragment { offset, bytes } => (offset, bytes),
      TransferEvent::Abandon { reason } => {
        tracing::info!(%current_id, %reason, "companion abandoned ota stream; partial retained for resume");
        self.sinks.unbind(transfer_id);
        self.state = OtaState::Idle;
        self.range_proxy.deactivate().await;
        return;
      }
    };

    let last = offset as u64 + bytes.len() as u64 >= expected_size;
    let outcome = self
      .transfers
      .accept_chunk(current_id.clone(), offset as u64, bytes, last)
      .await;
    match outcome {
      Ok(ChunkOutcome::Continue { received }) => {
        self.send_ack(peer, transfer_id, received as u32).await;
        let percent = phase_percent(received, expected_size);
        let changed = self.last_streaming_percent != Some(percent);
        let floor_ok = self
          .last_streaming_emit_at
          .is_none_or(|t| t.elapsed() >= STREAMING_PROGRESS_MIN_INTERVAL);
        if changed && floor_ok {
          emit_progress(&self.events_tx, OtaPhase::Streaming, percent, None).await;
          self.last_streaming_emit_at = Some(Instant::now());
          self.last_streaming_percent = Some(percent);
        }
      }
      Ok(ChunkOutcome::Completed { path, .. }) => {
        self.send_ack(peer, transfer_id, expected_size as u32).await;
        emit_progress(&self.events_tx, OtaPhase::Streaming, 100, None).await;
        emit_progress(&self.events_tx, OtaPhase::Verifying, 100, None).await;
        self.last_streaming_emit_at = None;
        self.last_streaming_percent = None;
        self.sinks.unbind(transfer_id);
        self.spawn_write(kind, current_id, peer, path, patch, begin_meta).await;
      }
      Err(err) => {
        let code = transfer_error_code(&err);
        emit_error(&self.events_tx, code, format!("ota fragment: {err}")).await;
        let _ = self.transfers.abandon(current_id).await;
        self.sinks.unbind(transfer_id);
        self.state = OtaState::Idle;
        self.range_proxy.deactivate().await;
      }
    }
  }

  async fn handle_abandon(&mut self, update_id: String) {
    let _ = self.transfers.abandon(update_id.clone()).await;
    self.range_proxy.clear_cache(update_id.clone()).await;
    if let Some(pos) = self.staged.iter().position(|p| p.update_id == update_id) {
      let piece = self.staged.remove(pos);
      staging::discard(&piece).await;
      if self.staged.is_empty() {
        self.staged_peer = None;
      }
    }
    if let OtaState::Streaming {
      update_id: streaming,
      transfer_id,
      ..
    } = &self.state
      && streaming == &update_id
    {
      self.sinks.unbind(*transfer_id);
      self.state = OtaState::Idle;
      self.range_proxy.deactivate().await;
    }
  }

  async fn handle_cancel(&mut self) {
    match &self.state {
      OtaState::Idle => tracing::debug!("cancel requested with no run in flight; ignoring"),
      OtaState::Streaming {
        update_id, transfer_id, ..
      } => {
        tracing::info!(%update_id, "cancel during streaming; partial retained for resume");
        self.sinks.unbind(*transfer_id);
        self.state = OtaState::Idle;
        self.range_proxy.deactivate().await;
      }
      OtaState::Writing { kind, cancel_tx, .. } => match kind {
        OtaKind::Image => {
          tracing::info!("cancel during image write; signalling libswupdate");
          let _ = cancel_tx.send(true);
        }
        OtaKind::Daemon | OtaKind::BuiltinWebapp | OtaKind::InstalledWebapp | OtaKind::WakewordModel => {
          tracing::info!(?kind, "cancel during swap/install; not interruptible, ignoring");
        }
      },
    }
  }

  async fn spawn_write(
    &mut self,
    kind: OtaKind,
    update_id: String,
    peer: Option<Address>,
    payload: PathBuf,
    patch: Option<OtaPatch>,
    begin_meta: BeginMeta,
  ) {
    let (cancel_tx, cancel_rx) = watch::channel(false);
    self.state = OtaState::Writing {
      kind,
      update_id: update_id.clone(),
      cancel_tx: cancel_tx.clone(),
      peer,
    };

    let events_tx = self.events_tx.clone();
    let self_tx = self.self_tx.clone();

    match kind {
      OtaKind::Image => {
        if let Some(addr) = peer {
          let cancel_tx_for_watcher = cancel_tx.clone();
          let self_tx_for_watcher = self.self_tx.clone();
          let update_id_for_watcher = update_id.clone();
          let mut snapshot_rx = self.peers.watch_snapshot();
          tokio::spawn(async move {
            loop {
              if snapshot_rx.changed().await.is_err() {
                return;
              }
              let alive = Self::peer_link_alive(&snapshot_rx.borrow(), &addr);
              if !alive {
                tracing::warn!(%addr, "pinned peer disconnected mid-write; signalling cancel + holding OtaError");
                let _ = self_tx_for_watcher
                  .send(Command::HoldFailure {
                    peer: addr,
                    update_id: update_id_for_watcher.clone(),
                    code: OtaErrorCode::Internal,
                    msg: "companion disconnected mid-install".into(),
                  })
                  .await;
                let _ = cancel_tx_for_watcher.send(true);
                return;
              }
            }
          });
        }

        let terminator = self.terminators.reboot.clone();
        let tally = self.range_proxy.tally();
        let transfers = self.transfers.clone();
        let range_proxy = self.range_proxy.clone();
        tokio::spawn(async move {
          let outcome = run_image_write(&events_tx, &payload, tally, cancel_rx).await;
          match outcome {
            Ok(()) => {
              let _ = transfers.abandon(update_id.clone()).await;
              range_proxy.clear_cache(update_id.clone()).await;
              tracing::info!(%update_id, "image write complete; rebooting");
              emit_finished(&events_tx, OtaKind::Image, update_id.clone()).await;
              (terminator)();
            }
            Err(err) => {
              tracing::warn!(?err, "image write terminated with error; artifact retained for retry");
              range_proxy.note_write_failure(update_id.clone()).await;
              emit_error(&events_tx, err.code, err.msg).await;
            }
          }
          let _ = self_tx.send(Command::WriteFinished).await;
        });
      }
      OtaKind::Daemon | OtaKind::BuiltinWebapp => {
        tokio::spawn(async move {
          let result = run_stage(&events_tx, kind, &payload, update_id, patch, cancel_rx).await;
          let _ = self_tx.send(Command::StageFinished { result, peer }).await;
        });
      }
      OtaKind::WakewordModel => {
        let reload = self.wakeword_reload.clone();
        let transfers = self.transfers.clone();
        tokio::spawn(async move {
          emit_progress(&events_tx, OtaPhase::Writing, 0, None).await;
          let result = match wakeword_swap::target() {
            Some(dest) => wakeword_swap::apply(&payload, &dest, begin_meta.version.as_deref())
              .await
              .map(|()| dest),
            None => Err(wakeword_swap::ApplyError::NoTarget),
          };
          match result {
            Ok(path) => {
              let _ = transfers.abandon(update_id.clone()).await;
              (reload)().await;
              tracing::info!(%update_id, model = %path.display(), "wake word model applied and reloaded");
              emit_finished(&events_tx, OtaKind::WakewordModel, update_id.clone()).await;
            }
            Err(err) => {
              tracing::warn!(%update_id, ?err, "wake word model apply failed; the live model is untouched");
              emit_error(
                &events_tx,
                OtaErrorCode::WriteFailed,
                format!("wake word model apply failed: {err}"),
              )
              .await;
            }
          }
          let _ = self_tx.send(Command::WriteFinished).await;
        });
      }
      OtaKind::InstalledWebapp => {
        let apply = self.installed_apply.clone();
        let transfers = self.transfers.clone();
        tokio::spawn(async move {
          emit_progress(&events_tx, OtaPhase::Writing, 0, None).await;
          let result = (apply)(payload.clone(), begin_meta.provenance).await;
          match result {
            Ok(info) => {
              let _ = transfers.abandon(update_id.clone()).await;
              tracing::info!(%update_id, id = %info.id, name = %info.name, "installed webapp applied");
              emit_finished(&events_tx, OtaKind::InstalledWebapp, update_id.clone()).await;
            }
            Err(err) => {
              tracing::warn!(%update_id, ?err, "installed webapp apply failed");
              emit_error(
                &events_tx,
                OtaErrorCode::WriteFailed,
                format!("install failed: {err:?}"),
              )
              .await;
            }
          }
          let _ = self_tx.send(Command::WriteFinished).await;
        });
      }
    }
  }

  async fn handle_stage_finished(&mut self, result: Result<StagedPiece, WriteError>, peer: Option<Address>) {
    self.state = OtaState::Idle;
    match result {
      Ok(piece) => {
        tracing::info!(update_id = %piece.update_id, kind = ?piece.kind, "ota piece staged; awaiting activate");
        self.staged.retain(|p| p.update_id != piece.update_id);
        self.staged.push(piece);
        self.staged_peer = peer;
        emit_progress(&self.events_tx, OtaPhase::Writing, 100, None).await;
      }
      Err(err) => {
        tracing::warn!(?err, "ota stage terminated with error");
        emit_error(&self.events_tx, err.code, err.msg).await;
      }
    }
  }

  async fn handle_activate(&mut self, expected: Vec<String>) {
    if !matches!(self.state, OtaState::Idle) {
      emit_error(
        &self.events_tx,
        OtaErrorCode::Internal,
        "cannot activate while an OTA transfer is in flight".into(),
      )
      .await;
      return;
    }

    let have: BTreeSet<&str> = self.staged.iter().map(|p| p.update_id.as_str()).collect();
    let want: BTreeSet<&str> = expected.iter().map(String::as_str).collect();
    if self.staged.is_empty() || have != want {
      emit_error(
        &self.events_tx,
        OtaErrorCode::Internal,
        format!(
          "no matching staged batch (staged {}, expected {})",
          self.staged.len(),
          expected.len()
        ),
      )
      .await;
      return;
    }

    let mut batch = std::mem::take(&mut self.staged);
    self.staged_peer = None;
    batch.sort_by_key(|p| match p.kind {
      OtaKind::BuiltinWebapp => 0,
      OtaKind::Daemon => 1,
      OtaKind::Image | OtaKind::InstalledWebapp | OtaKind::WakewordModel => 2,
    });

    let mut committed: Vec<&StagedPiece> = Vec::new();
    for piece in &batch {
      match staging::commit(piece).await {
        Ok(()) => committed.push(piece),
        Err(err) => {
          tracing::error!(?err, kind = ?piece.kind, "commit failed; rolling back batch");
          for &done in committed.iter().rev() {
            if let Err(rb) = staging::rollback(done).await {
              tracing::error!(?rb, "rollback failed during batch unwind");
            }
          }
          for p in &batch {
            staging::discard(p).await;
          }
          emit_error(
            &self.events_tx,
            OtaErrorCode::WriteFailed,
            format!("commit failed: {err}"),
          )
          .await;
          return;
        }
      }
    }

    tracing::info!(pieces = batch.len(), "bandaid batch committed; restarting service");
    emit_progress(&self.events_tx, OtaPhase::Reboot, 0, None).await;
    for piece in &batch {
      let _ = self.transfers.abandon(piece.update_id.clone()).await;
      emit_finished(&self.events_tx, piece.kind, piece.update_id.clone()).await;
    }
    drain_events(&self.events_tx).await;
    (self.terminators.restart_self)();
  }
}

const DRAIN_POLLS: u32 = 50;
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

async fn drain_events(events_tx: &OtaEventTx) {
  for _ in 0..DRAIN_POLLS {
    if events_tx.capacity() == events_tx.max_capacity() {
      return;
    }
    tokio::time::sleep(DRAIN_POLL_INTERVAL).await;
  }
  tracing::warn!("ota events still queued at restart; the terminal frame may not reach a listener");
}

async fn emit_finished(events_tx: &OtaEventTx, kind: OtaKind, update_id: String) {
  let _ = events_tx
    .send(BridgeToGatewaySystemMsgEvent::OtaFinished(OtaFinished {
      kind,
      update_id,
    }))
    .await;
}

async fn emit_error(events_tx: &OtaEventTx, code: OtaErrorCode, msg: String) {
  send_error(events_tx, code, msg, None, false).await
}

async fn send_error(
  events_tx: &OtaEventTx,
  code: OtaErrorCode,
  msg: String,
  update_id: Option<String>,
  replayed: bool,
) {
  let _ = events_tx
    .send(BridgeToGatewaySystemMsgEvent::OtaError(OtaError {
      code,
      msg,
      update_id,
      replayed,
    }))
    .await;
}

#[derive(Debug)]
pub(crate) struct WriteError {
  pub(crate) code: OtaErrorCode,
  pub(crate) msg: String,
}

async fn run_image_write(
  events_tx: &OtaEventTx,
  swu_path: &std::path::Path,
  tally: Arc<range_proxy::RangeTally>,
  mut cancel_rx: watch::Receiver<bool>,
) -> Result<(), WriteError> {
  if check_cancel(&mut cancel_rx) {
    return Err(WriteError {
      code: OtaErrorCode::Cancelled,
      msg: "cancelled before writing".into(),
    });
  }

  let target = slots::inactive_slot().await.map_err(|err| WriteError {
    code: OtaErrorCode::Internal,
    msg: format!("failed to read inactive slot: {err}"),
  })?;
  tracing::info!(?target, "ota target slot resolved");
  let selector = swupdate::Selector {
    software_set: "stable".into(),
    running_mode: target.selector().into(),
  };

  let progress_emitter = {
    let tx = events_tx.clone();
    move |mut tick: swupdate::ProgressTick| {
      let (served, expected) = tally.snapshot();
      if expected > 0 {
        tick.dwl_bytes = served.min(u32::MAX as u64) as u32;
        tick.dwl_percent = ((served.saturating_mul(100) / expected).min(100)) as u8;
      }
      let tx = tx.clone();
      tokio::spawn(async move {
        emit_progress_tick(&tx, tick).await;
      });
    }
  };

  swupdate::install_swu(swu_path, &selector, &progress_emitter, &mut cancel_rx)
    .await
    .map_err(|err| WriteError {
      code: match err {
        swupdate::Error::Cancelled => OtaErrorCode::Cancelled,
        swupdate::Error::Io(_) => OtaErrorCode::WriteFailed,
        #[cfg(feature = "swupdate")]
        swupdate::Error::Ipc(_) | swupdate::Error::InstallFailed(_) => OtaErrorCode::WriteFailed,
      },
      msg: format!("swupdate failed: {err}"),
    })?;

  emit_progress(events_tx, OtaPhase::Confirming, 0, None).await;
  slots::confirm_target(target).await.map_err(|err| WriteError {
    code: OtaErrorCode::ConfirmFailed,
    msg: format!("failed to confirm target slot {:?}: {err}", target),
  })?;
  emit_progress(events_tx, OtaPhase::Confirming, 100, None).await;

  emit_progress(events_tx, OtaPhase::Reboot, 0, None).await;
  Ok(())
}

async fn run_stage(
  events_tx: &OtaEventTx,
  kind: OtaKind,
  payload: &std::path::Path,
  update_id: String,
  patch: Option<OtaPatch>,
  mut cancel_rx: watch::Receiver<bool>,
) -> Result<StagedPiece, WriteError> {
  if check_cancel(&mut cancel_rx) {
    return Err(WriteError {
      code: OtaErrorCode::Cancelled,
      msg: "cancelled before staging".into(),
    });
  }

  emit_progress(events_tx, OtaPhase::Writing, 0, None).await;
  let piece = match kind {
    OtaKind::Daemon => {
      let staged = match patch {
        Some(spec) => patch::apply(daemon_swap::patch_source_path(), payload.to_path_buf(), spec)
          .await
          .map_err(|err| WriteError {
            code: OtaErrorCode::WriteFailed,
            msg: format!("daemon patch apply failed: {err}"),
          })?,
        None if crate::paths::is_on_device() => {
          let copy = payload.with_extension("stage");
          tokio::fs::copy(payload, &copy).await.map_err(|err| WriteError {
            code: OtaErrorCode::WriteFailed,
            msg: format!("daemon stage copy failed: {err}"),
          })?;
          copy
        }
        None => payload.to_path_buf(),
      };
      let result = daemon_swap::stage(&staged, update_id).await.map_err(|err| WriteError {
        code: OtaErrorCode::WriteFailed,
        msg: format!("daemon stage failed: {err}"),
      });
      if result.is_err() && staged != payload {
        let _ = tokio::fs::remove_file(&staged).await;
      }
      result?
    }
    OtaKind::BuiltinWebapp => webapp_swap::stage(payload, update_id).await.map_err(|err| WriteError {
      code: OtaErrorCode::WriteFailed,
      msg: format!("builtin-webapp stage failed: {err}"),
    })?,
    OtaKind::Image | OtaKind::InstalledWebapp | OtaKind::WakewordModel => {
      return Err(WriteError {
        code: OtaErrorCode::Internal,
        msg: "run_stage called for a non-bandaid kind".into(),
      });
    }
  };

  Ok(piece)
}

pub(crate) fn check_cancel(rx: &mut watch::Receiver<bool>) -> bool {
  *rx.borrow_and_update()
}

async fn emit_progress(events_tx: &OtaEventTx, phase: OtaPhase, percent: u8, eta_ms: Option<u32>) {
  let _ = events_tx
    .send(BridgeToGatewaySystemMsgEvent::OtaProgress(OtaProgress {
      phase,
      percent,
      step: 0,
      nsteps: 0,
      dwl_percent: 0,
      dwl_bytes: 0,
      eta_ms,
    }))
    .await;
}

async fn emit_progress_tick(events_tx: &OtaEventTx, tick: swupdate::ProgressTick) {
  let _ = events_tx
    .send(BridgeToGatewaySystemMsgEvent::OtaProgress(OtaProgress {
      phase: tick.phase,
      percent: tick.percent,
      step: tick.step,
      nsteps: tick.nsteps,
      dwl_percent: tick.dwl_percent,
      dwl_bytes: tick.dwl_bytes,
      eta_ms: tick.eta_ms,
    }))
    .await;
}

fn phase_percent(received: u64, expected: u64) -> u8 {
  if expected == 0 {
    return 100;
  }
  ((received.saturating_mul(100)) / expected).min(100) as u8
}

const BANDAID_PREFLIGHT_SLACK_BYTES: u64 = 4 * 1024 * 1024;

fn bandaid_bytes_needed(expected_size: u64, resume_from_offset: u64, patch: Option<&OtaPatch>) -> u64 {
  let reconstructed = patch.map(|p| p.result_size as u64).unwrap_or(0);
  expected_size
    .saturating_sub(resume_from_offset)
    .saturating_add(reconstructed)
    .saturating_add(BANDAID_PREFLIGHT_SLACK_BYTES)
}

fn transfer_error_code(err: &TransferError) -> OtaErrorCode {
  match err {
    TransferError::OffsetMismatch { .. } => OtaErrorCode::OffsetMismatch,
    TransferError::SizeOverflow { .. } | TransferError::SizeMismatch { .. } => OtaErrorCode::SizeMismatch,
    TransferError::HashMismatch { .. } => OtaErrorCode::HashMismatch,
    TransferError::UnknownTransfer { .. } => OtaErrorCode::UnknownUpdate,
    _ => OtaErrorCode::Internal,
  }
}

#[cfg(test)]
mod tests {
  use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

  use libbridgething::{
    WebappRole, WebappSource,
    gateway::{BridgeToGatewayMsgData, BridgeToGatewayTransferMsg, OtaBegin, TransferRef},
  };
  use sha2::{Digest, Sha256};
  use tokio::time::{Duration, timeout};
  use tokio_util::bytes::Bytes;

  use super::*;

  #[test]
  fn bandaid_preflight_counts_remaining_transfer_plus_reconstruction() {
    let patch = OtaPatch {
      algorithm: libbridgething::gateway::OtaPatchAlgorithm::ZstdPatchFrom,
      result_sha256: "aa".into(),
      result_size: 43_000_000,
      source_sha256: None,
    };
    let needed = bandaid_bytes_needed(13_000_000, 3_000_000, Some(&patch));
    assert_eq!(needed, 10_000_000 + 43_000_000 + BANDAID_PREFLIGHT_SLACK_BYTES);

    let raw = bandaid_bytes_needed(43_000_000, 0, None);
    assert_eq!(raw, 43_000_000 + BANDAID_PREFLIGHT_SLACK_BYTES);

    let resumed_past_end = bandaid_bytes_needed(100, 200, None);
    assert_eq!(resumed_past_end, BANDAID_PREFLIGHT_SLACK_BYTES);
  }

  fn dummy_info() -> WebappInfo {
    WebappInfo {
      id: uuid::Uuid::now_v7(),
      name: "test-app".into(),
      source: WebappSource::Installed,
      role: WebappRole::Standard,
      version: "0.1.0".into(),
      description: None,
      icon_hash: None,
      settings_hash: None,
      overlay_hash: None,
      config: vec![],
      permissions: vec![],
      renders_voice_display: false,
      art: None,
      provenance: None,
    }
  }

  fn fixture_bytes() -> (Vec<u8>, String, u32) {
    let bytes = b"fake-swu-payload-for-orchestrator-tests".to_vec();
    let sha = {
      let mut h = Sha256::new();
      h.update(&bytes);
      hex::encode(h.finalize())
    };
    let size = bytes.len() as u32;
    (bytes, sha, size)
  }

  fn sized_fixture(n: usize) -> (Vec<u8>, String, u32) {
    let bytes: Vec<u8> = (0..n).map(|i| (i * 31 + 7) as u8).collect();
    let sha = {
      let mut h = Sha256::new();
      h.update(&bytes);
      hex::encode(h.finalize())
    };
    let size = bytes.len() as u32;
    (bytes, sha, size)
  }

  fn temp_root() -> PathBuf {
    let p = std::env::temp_dir().join(format!("bridgething-ota-test-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&p).unwrap();
    p
  }

  fn tid_for(update_id: impl AsRef<str>) -> uuid::Uuid {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, update_id.as_ref().as_bytes())
  }

  struct Harness {
    ota: OtaOrchestrator,
    sinks: TransferSinks,
    events: mpsc::Receiver<BridgeToGatewaySystemMsgEvent>,
    reboot_calls: Arc<AtomicUsize>,
    restart_self_calls: Arc<AtomicUsize>,
    installed_apply_calls: Arc<AtomicUsize>,
    installed_apply_ok: Arc<AtomicBool>,
    installed_apply_provenance: Arc<std::sync::Mutex<Option<Option<String>>>>,
    wakeword_reload_calls: Arc<AtomicUsize>,
    captured_acks: Arc<std::sync::Mutex<Vec<TransferAck>>>,
    root: PathBuf,
  }

  async fn boot() -> Harness {
    boot_with_peers(PeerTracker::noop()).await
  }

  async fn boot_with_peers(peers: PeerTracker) -> Harness {
    boot_on(temp_root(), peers).await
  }

  async fn boot_on(root: PathBuf, peers: PeerTracker) -> Harness {
    let pending = ChunkedTransfer::init(root.clone(), Vec::new()).await.unwrap();
    let (transfers, _xfer_handle) = pending.spawn();
    let (events_tx, events) = mpsc::channel(64);
    let reboot_calls = Arc::new(AtomicUsize::new(0));
    let restart_self_calls = Arc::new(AtomicUsize::new(0));
    let reboot_counter = reboot_calls.clone();
    let restart_counter = restart_self_calls.clone();
    let terminators = OtaTerminators {
      reboot: Arc::new(move || {
        reboot_counter.fetch_add(1, Ordering::SeqCst);
      }),
      restart_self: Arc::new(move || {
        restart_counter.fetch_add(1, Ordering::SeqCst);
      }),
    };
    let installed_apply_calls = Arc::new(AtomicUsize::new(0));
    let installed_apply_ok = Arc::new(AtomicBool::new(true));
    let installed_apply_provenance = Arc::new(std::sync::Mutex::new(None::<Option<String>>));
    let apply_calls = installed_apply_calls.clone();
    let apply_ok = installed_apply_ok.clone();
    let apply_provenance = installed_apply_provenance.clone();
    let installed_apply: InstalledWebappApply = Arc::new(move |_path, provenance| {
      let calls = apply_calls.clone();
      let ok = apply_ok.clone();
      let seen = apply_provenance.clone();
      Box::pin(async move {
        *seen.lock().unwrap() = Some(provenance);
        calls.fetch_add(1, Ordering::SeqCst);
        if ok.load(Ordering::SeqCst) {
          Ok(dummy_info())
        } else {
          Err(WebappError::Internal {
            reason: "test apply failure".into(),
          })
        }
      })
    });
    let wakeword_reload_calls = Arc::new(AtomicUsize::new(0));
    let wakeword_reload_counter = wakeword_reload_calls.clone();
    let wakeword_reload: WakewordReload = Arc::new(move || {
      let counter = wakeword_reload_counter.clone();
      Box::pin(async move {
        counter.fetch_add(1, Ordering::SeqCst);
      })
    });
    let sinks = TransferSinks::default();
    let asset_db = crate::db::open(None).await.unwrap();
    let (assets, _asset_handle) = AssetCache::init(asset_db, root.join("assets")).await.unwrap().spawn();
    let (gateway_man, mut gw_rx) = crate::bluetooth::GatewayMan::capturing();
    let captured_acks: Arc<std::sync::Mutex<Vec<TransferAck>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let acks_sink = captured_acks.clone();
    tokio::spawn(async move {
      while let Some(out) = gw_rx.recv().await {
        if let BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Ack(ack)) = &out.msg.data {
          acks_sink.lock().unwrap().push(ack.clone());
        }
      }
    });
    let (ota, _ota_handle) = OtaOrchestrator::spawn(
      transfers,
      events_tx,
      gateway_man,
      terminators,
      range_proxy::noop_proxy(),
      peers,
      installed_apply,
      wakeword_reload,
      sinks.clone(),
      assets,
    );
    Harness {
      ota,
      sinks,
      events,
      reboot_calls,
      restart_self_calls,
      installed_apply_calls,
      installed_apply_ok,
      installed_apply_provenance,
      wakeword_reload_calls,
      captured_acks,
      root,
    }
  }

  async fn wait_for(
    events: &mut mpsc::Receiver<BridgeToGatewaySystemMsgEvent>,
    deadline: Duration,
    pred: impl Fn(&BridgeToGatewaySystemMsgEvent) -> bool,
  ) -> BridgeToGatewaySystemMsgEvent {
    timeout(deadline, async {
      loop {
        let ev = events.recv().await.expect("event channel closed");
        if pred(&ev) {
          return ev;
        }
      }
    })
    .await
    .expect("timed out waiting for matching event")
  }

  async fn drain_until_restart(
    events: &mut mpsc::Receiver<BridgeToGatewaySystemMsgEvent>,
    calls: &AtomicUsize,
    deadline: Duration,
  ) -> Vec<BridgeToGatewaySystemMsgEvent> {
    let mut seen = Vec::new();
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
      while let Ok(ev) = events.try_recv() {
        seen.push(ev);
      }
      if calls.load(Ordering::SeqCst) > 0 {
        break;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    seen
  }

  fn count_finished(events: &[BridgeToGatewaySystemMsgEvent]) -> usize {
    events
      .iter()
      .filter(|ev| matches!(ev, BridgeToGatewaySystemMsgEvent::OtaFinished(_)))
      .count()
  }

  fn announced_reboot(events: &[BridgeToGatewaySystemMsgEvent]) -> bool {
    events
      .iter()
      .any(|ev| matches!(ev, BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Reboot)))
  }

  #[tokio::test]
  async fn happy_path_streams_completes_and_reboots() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();

    let ack = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("begin ok");
    assert_eq!(ack.resume_from_offset, 0);

    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));

    let _ = wait_for(&mut h.events, Duration::from_secs(10), |ev| {
      matches!(
        ev,
        BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Reboot)
      )
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.reboot_calls.load(Ordering::SeqCst), 1);
  }

  #[tokio::test]
  async fn receipt_acks_carry_the_stream_to_completion() {
    let h = boot().await;
    let (bytes, sha, size) = sized_fixture(40 * 1024);
    let peer = Address::new([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x01]);
    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        Some(peer),
      )
      .await
      .expect("begin ok");

    let frag = 4096usize;
    let mut off = 0usize;
    while off < bytes.len() {
      let end = (off + frag).min(bytes.len());
      h.sinks
        .fragment(tid_for(&sha), off as u32, Bytes::copy_from_slice(&bytes[off..end]));
      off = end;
      tokio::task::yield_now().await;
    }

    let acks = timeout(Duration::from_secs(5), async {
      loop {
        {
          let acks = h.captured_acks.lock().unwrap();
          if acks.last().is_some_and(|a| a.received == size) {
            return acks.clone();
          }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
      }
    })
    .await
    .expect("stream completed and emitted a final ack at received == size");

    assert_eq!(acks.last().unwrap().received, size);
    let mut prev = 0u32;
    for a in &acks {
      assert!(a.received > prev, "acks must be monotonically increasing");
      prev = a.received;
    }
  }

  #[tokio::test]
  async fn size_mismatch_emits_error() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();
    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size - 1,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("begin ok");

    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));

    let err = wait_for(&mut h.events, Duration::from_secs(2), |ev| {
      matches!(ev, BridgeToGatewaySystemMsgEvent::OtaError(_))
    })
    .await;
    let BridgeToGatewaySystemMsgEvent::OtaError(e) = err else {
      unreachable!()
    };
    assert_eq!(e.code, OtaErrorCode::SizeMismatch);
  }

  #[tokio::test]
  async fn hash_mismatch_emits_error() {
    let mut h = boot().await;
    let (bytes, _sha, size) = fixture_bytes();
    let bogus_sha = "0".repeat(64);
    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: bogus_sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&bogus_sha),
            total_size: size,
            sha256: Some(bogus_sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("begin ok");
    h.sinks.fragment(tid_for(&bogus_sha), 0, Bytes::from(bytes));

    let err = wait_for(&mut h.events, Duration::from_secs(2), |ev| {
      matches!(ev, BridgeToGatewaySystemMsgEvent::OtaError(_))
    })
    .await;
    let BridgeToGatewaySystemMsgEvent::OtaError(e) = err else {
      unreachable!()
    };
    assert_eq!(e.code, OtaErrorCode::HashMismatch);
  }

  #[tokio::test]
  async fn begin_without_sha_is_rejected() {
    let h = boot().await;
    let (_bytes, sha, size) = fixture_bytes();
    let err = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: None,
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .unwrap_err();
    assert!(err.reason.contains("sha256"), "got reason: {}", err.reason);
  }

  #[tokio::test]
  async fn resume_returns_received_offset() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();
    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("first begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes[..10].to_vec()));
    tokio::time::sleep(Duration::from_millis(200)).await;

    h.ota.cancel().await;
    let ack = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("resume begin ok");
    assert_eq!(ack.resume_from_offset, 10);

    h.sinks.fragment(tid_for(&sha), 10, Bytes::from(bytes[10..].to_vec()));
    let _ = wait_for(&mut h.events, Duration::from_secs(10), |ev| {
      matches!(
        ev,
        BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Reboot)
      )
    })
    .await;
  }

  #[tokio::test]
  async fn second_begin_during_write_is_rejected() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();
    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .unwrap();
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));
    let _ = wait_for(
      &mut h.events,
      Duration::from_secs(5),
      |ev| matches!(ev, BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Writing)),
    )
    .await;

    let err = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: "deadbeef".repeat(8),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for("deadbeef".repeat(8)),
            total_size: 32,
            sha256: Some("deadbeef".repeat(8)),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .unwrap_err();
    assert!(err.reason.contains("in progress"), "got reason: {}", err.reason);
  }

  #[tokio::test]
  async fn abandon_clears_streaming_state() {
    let h = boot().await;
    let (bytes, sha, size) = fixture_bytes();
    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .unwrap();
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes[..10].to_vec()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    h.ota.abandon(sha.clone()).await;
    let ack = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("begin after abandon");
    assert_eq!(ack.resume_from_offset, 0);
  }

  #[tokio::test]
  async fn a_failure_its_peer_missed_is_replayed_when_that_peer_returns() {
    use libbridgething::{Device, DeviceType, GatewayInfo, Peer};

    let (peers, snapshot_tx) = PeerTracker::scripted();
    let mut h = boot_with_peers(peers).await;
    let (bytes, sha, size) = fixture_bytes();
    let peer: Address = "AA:BB:CC:DD:EE:03".parse().unwrap();

    let connected = || {
      let mut p = Peer::new(Device {
        name: "test-phone".into(),
        device_type: DeviceType::default(),
        id: peer.to_string(),
        kind: libbridgething::LinkKind::Bluetooth,
        default: false,
      });
      p.companion = PeerCompanionStatus::Connected(GatewayInfo::default());
      p
    };
    let mut up = crate::peer::PeerSnapshot::default();
    up.peers.insert(peer, connected());
    snapshot_tx.send(up.clone()).unwrap();

    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        Some(peer),
      )
      .await
      .unwrap();
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));
    let _ = wait_for(
      &mut h.events,
      Duration::from_secs(5),
      |ev| matches!(ev, BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Writing)),
    )
    .await;

    let mut down = crate::peer::PeerSnapshot::default();
    down.peers.insert(
      peer,
      Peer::new(Device {
        name: "test-phone".into(),
        device_type: DeviceType::default(),
        id: peer.to_string(),
        kind: libbridgething::LinkKind::Bluetooth,
        default: false,
      }),
    );
    snapshot_tx.send(down).unwrap();

    let live = wait_for(&mut h.events, Duration::from_secs(5), |ev| {
      matches!(ev, BridgeToGatewaySystemMsgEvent::OtaError(_))
    })
    .await;
    let BridgeToGatewaySystemMsgEvent::OtaError(live) = live else {
      panic!("expected the mid-write failure");
    };
    assert_eq!(live.update_id.as_deref(), Some(sha.as_str()));
    assert!(!live.replayed, "the first emit is the live one, not a replay");

    snapshot_tx.send(up).unwrap();

    let replay = wait_for(
      &mut h.events,
      Duration::from_secs(5),
      |ev| matches!(ev, BridgeToGatewaySystemMsgEvent::OtaError(e) if e.replayed),
    )
    .await;
    let BridgeToGatewaySystemMsgEvent::OtaError(replay) = replay else {
      panic!("expected the replayed failure");
    };
    assert_eq!(
      replay.update_id.as_deref(),
      Some(sha.as_str()),
      "a resume re-drives the same update_id, so the replay must be distinguishable by its flag alone"
    );
    assert_eq!(replay.code, live.code);
  }

  #[tokio::test]
  async fn pinned_peer_disconnect_mid_stream_releases_for_resume() {
    use libbridgething::{Device, DeviceType, GatewayInfo, Peer};

    let (peers, snapshot_tx) = PeerTracker::scripted();
    let h = boot_with_peers(peers).await;
    let (bytes, sha, size) = fixture_bytes();
    let peer_a: Address = "AA:BB:CC:DD:EE:01".parse().unwrap();
    let peer_b: Address = "AA:BB:CC:DD:EE:02".parse().unwrap();

    let connected_peer = |addr: Address| {
      let mut p = Peer::new(Device {
        name: "test-phone".into(),
        device_type: DeviceType::default(),
        id: addr.to_string(),
        kind: libbridgething::LinkKind::Bluetooth,
        default: false,
      });
      p.companion = PeerCompanionStatus::Connected(GatewayInfo::default());
      p
    };
    let mut snap = crate::peer::PeerSnapshot::default();
    snap.peers.insert(peer_a, connected_peer(peer_a));
    snapshot_tx.send(snap).unwrap();

    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        Some(peer_a),
      )
      .await
      .expect("first begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes[..10].to_vec()));
    tokio::time::sleep(Duration::from_millis(200)).await;

    snapshot_tx.send(crate::peer::PeerSnapshot::default()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut snap = crate::peer::PeerSnapshot::default();
    snap.peers.insert(peer_b, connected_peer(peer_b));
    snapshot_tx.send(snap).unwrap();

    let ack = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        Some(peer_b),
      )
      .await
      .expect("begin from a new companion after the pinned one died");
    assert_eq!(ack.resume_from_offset, 10, "partial survives the peer loss");
  }

  #[tokio::test]
  async fn second_begin_from_different_peer_is_rejected() {
    let h = boot().await;
    let (_bytes, sha, size) = fixture_bytes();
    let peer_a: Address = "AA:BB:CC:DD:EE:01".parse().unwrap();
    let peer_b: Address = "AA:BB:CC:DD:EE:02".parse().unwrap();

    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        Some(peer_a),
      )
      .await
      .expect("first begin ok");

    let err = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::Image,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some("deadbeef".repeat(8)),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        Some(peer_b),
      )
      .await
      .unwrap_err();
    assert!(
      err.reason.contains("pinned to a different companion"),
      "got reason: {}",
      err.reason,
    );
  }

  async fn stage_and_assert_no_restart(h: &mut Harness, kind: OtaKind, bytes: Vec<u8>, sha: String, size: u32) {
    h.ota
      .begin(
        OtaBegin {
          kind,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("stage begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));
    wait_for(&mut h.events, Duration::from_secs(5), |ev| {
      matches!(ev, BridgeToGatewaySystemMsgEvent::OtaProgress(p) if p.phase == OtaPhase::Writing && p.percent == 100)
    })
    .await;
  }

  #[tokio::test]
  async fn daemon_stage_then_activate_restarts_once() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();

    stage_and_assert_no_restart(&mut h, OtaKind::Daemon, bytes, sha.clone(), size).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
      h.restart_self_calls.load(Ordering::SeqCst),
      0,
      "staging must not restart before activate"
    );

    h.ota.activate(vec![sha]).await;
    let seen = drain_until_restart(&mut h.events, &h.restart_self_calls, Duration::from_secs(5)).await;
    assert!(
      announced_reboot(&seen),
      "the reboot phase is announced before the restart"
    );
    assert_eq!(
      count_finished(&seen),
      1,
      "the terminal frame leaves the daemon before it restarts"
    );
    assert_eq!(
      h.restart_self_calls.load(Ordering::SeqCst),
      1,
      "activate restarts exactly once"
    );
    assert_eq!(
      h.reboot_calls.load(Ordering::SeqCst),
      0,
      "reboot must not fire for bandaid kinds"
    );
  }

  #[tokio::test]
  async fn daemon_size_mismatch_emits_size_mismatch() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();
    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Daemon,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size - 1,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("daemon begin ok");

    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));

    let err = wait_for(&mut h.events, Duration::from_secs(2), |ev| {
      matches!(ev, BridgeToGatewaySystemMsgEvent::OtaError(_))
    })
    .await;
    let BridgeToGatewaySystemMsgEvent::OtaError(e) = err else {
      unreachable!()
    };
    assert_eq!(e.code, OtaErrorCode::SizeMismatch);
    assert_eq!(h.restart_self_calls.load(Ordering::SeqCst), 0);
  }

  #[tokio::test]
  async fn daemon_hash_mismatch_emits_hash_mismatch() {
    let mut h = boot().await;
    let (bytes, _sha, size) = fixture_bytes();
    let bogus_sha = "0".repeat(64);
    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Daemon,
          update_id: bogus_sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&bogus_sha),
            total_size: size,
            sha256: Some(bogus_sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("daemon begin ok");
    h.sinks.fragment(tid_for(&bogus_sha), 0, Bytes::from(bytes));

    let err = wait_for(&mut h.events, Duration::from_secs(2), |ev| {
      matches!(ev, BridgeToGatewaySystemMsgEvent::OtaError(_))
    })
    .await;
    let BridgeToGatewaySystemMsgEvent::OtaError(e) = err else {
      unreachable!()
    };
    assert_eq!(e.code, OtaErrorCode::HashMismatch);
    assert_eq!(h.restart_self_calls.load(Ordering::SeqCst), 0);
  }

  fn fixture_seed(seed: &str) -> (Vec<u8>, String, u32) {
    let bytes = format!("fake-ota-payload-{seed}").into_bytes();
    let sha = {
      let mut hsh = Sha256::new();
      hsh.update(&bytes);
      hex::encode(hsh.finalize())
    };
    let size = bytes.len() as u32;
    (bytes, sha, size)
  }

  #[tokio::test]
  async fn coupled_batch_commits_with_single_restart() {
    let mut h = boot().await;
    let (db, dsha, dsz) = fixture_seed("daemon");
    let (hb, hsha, hsz) = fixture_seed("hub");
    let (sb, ssha, ssz) = fixture_seed("stock");

    stage_and_assert_no_restart(&mut h, OtaKind::Daemon, db, dsha.clone(), dsz).await;
    stage_and_assert_no_restart(&mut h, OtaKind::BuiltinWebapp, hb, hsha.clone(), hsz).await;
    stage_and_assert_no_restart(&mut h, OtaKind::BuiltinWebapp, sb, ssha.clone(), ssz).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
      h.restart_self_calls.load(Ordering::SeqCst),
      0,
      "three staged pieces must not have restarted yet"
    );

    h.ota.activate(vec![dsha, hsha, ssha]).await;
    let seen = drain_until_restart(&mut h.events, &h.restart_self_calls, Duration::from_secs(5)).await;
    assert!(
      announced_reboot(&seen),
      "the reboot phase is announced before the restart"
    );
    assert_eq!(
      count_finished(&seen),
      3,
      "every piece in the batch reports finished before the daemon restarts"
    );
    assert_eq!(
      h.restart_self_calls.load(Ordering::SeqCst),
      1,
      "a three-piece batch activates with exactly one restart"
    );
    assert_eq!(h.reboot_calls.load(Ordering::SeqCst), 0);
  }

  #[tokio::test]
  async fn partial_batch_without_activate_never_restarts() {
    let mut h = boot().await;
    let (db, dsha, dsz) = fixture_seed("daemon");
    let (hb, hsha, hsz) = fixture_seed("hub");

    stage_and_assert_no_restart(&mut h, OtaKind::Daemon, db, dsha, dsz).await;
    stage_and_assert_no_restart(&mut h, OtaKind::BuiltinWebapp, hb, hsha, hsz).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
      h.restart_self_calls.load(Ordering::SeqCst),
      0,
      "staged-but-not-activated batch must never restart"
    );
  }

  #[tokio::test]
  async fn activate_with_mismatched_expected_errors_and_does_not_restart() {
    let mut h = boot().await;
    let (db, dsha, dsz) = fixture_seed("daemon");
    stage_and_assert_no_restart(&mut h, OtaKind::Daemon, db, dsha, dsz).await;

    h.ota.activate(vec!["0".repeat(64)]).await;
    let err = wait_for(&mut h.events, Duration::from_secs(2), |ev| {
      matches!(ev, BridgeToGatewaySystemMsgEvent::OtaError(_))
    })
    .await;
    let BridgeToGatewaySystemMsgEvent::OtaError(e) = err else {
      unreachable!()
    };
    assert_eq!(e.code, OtaErrorCode::Internal);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.restart_self_calls.load(Ordering::SeqCst), 0);
  }

  #[tokio::test]
  async fn daemon_cancel_during_streaming_keeps_partial() {
    let h = boot().await;
    let (bytes, sha, size) = fixture_bytes();
    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::Daemon,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("daemon begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes[..10].to_vec()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    h.ota.cancel().await;

    let ack = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::Daemon,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("resume begin ok");
    assert_eq!(ack.resume_from_offset, 10, "partial should survive cancel");
    assert_eq!(h.restart_self_calls.load(Ordering::SeqCst), 0);
  }

  fn begin_req(kind: OtaKind, sha: &str, size: u32) -> OtaBegin {
    OtaBegin {
      kind,
      update_id: sha.to_string(),
      update_url_base: None,
      transfer: TransferRef {
        id: tid_for(sha),
        total_size: size,
        sha256: Some(sha.to_string()),
      },
      patch: None,
      provenance: None,
      version: None,
    }
  }

  #[tokio::test]
  async fn a_cancelled_image_write_retains_the_artifact_for_a_no_transfer_retry() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();

    h.ota
      .begin(begin_req(OtaKind::Image, &sha, size), None)
      .await
      .expect("begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));

    let _ = wait_for(&mut h.events, Duration::from_secs(10), |ev| {
      matches!(
        ev,
        BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Writing)
      )
    })
    .await;
    h.ota.cancel().await;
    let _ = wait_for(&mut h.events, Duration::from_secs(10), |ev| {
      matches!(
        ev,
        BridgeToGatewaySystemMsgEvent::OtaError(e) if matches!(e.code, OtaErrorCode::Cancelled)
      )
    })
    .await;
    assert_eq!(h.reboot_calls.load(Ordering::SeqCst), 0);

    let ack = h
      .ota
      .begin(begin_req(OtaKind::Image, &sha, size), None)
      .await
      .expect("retry begin ok");
    assert_eq!(
      ack.resume_from_offset, size,
      "a retained artifact answers with the full offset"
    );

    let _ = wait_for(&mut h.events, Duration::from_secs(10), |ev| {
      matches!(
        ev,
        BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Reboot)
      )
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
      h.reboot_calls.load(Ordering::SeqCst),
      1,
      "retry completes with zero re-transfer"
    );
  }

  #[tokio::test]
  async fn a_reboot_between_stage_and_activate_re_stages_from_the_retained_payload() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();

    h.ota
      .begin(begin_req(OtaKind::Daemon, &sha, size), None)
      .await
      .expect("begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));
    let _ = wait_for(&mut h.events, Duration::from_secs(10), |ev| {
      matches!(
        ev,
        BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Writing) && p.percent == 100
      )
    })
    .await;

    let root = h.root.clone();
    drop(h);
    let mut h2 = boot_on(root, PeerTracker::noop()).await;

    let ack = h2
      .ota
      .begin(begin_req(OtaKind::Daemon, &sha, size), None)
      .await
      .expect("re-begin after reboot ok");
    assert_eq!(
      ack.resume_from_offset, size,
      "the retained payload makes the re-drive transfer-free"
    );

    let _ = wait_for(&mut h2.events, Duration::from_secs(10), |ev| {
      matches!(
        ev,
        BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Writing) && p.percent == 100
      )
    })
    .await;
    h2.ota.activate(vec![sha.clone()]).await;
    let events = drain_until_restart(&mut h2.events, &h2.restart_self_calls, Duration::from_secs(10)).await;
    assert_eq!(h2.restart_self_calls.load(Ordering::SeqCst), 1);
    assert_eq!(count_finished(&events), 1);
  }

  #[tokio::test]
  async fn a_streaming_partial_survives_a_reboot_and_resumes_at_its_offset() {
    let h = boot().await;
    let (bytes, sha, size) = sized_fixture(8 * 1024);

    h.ota
      .begin(begin_req(OtaKind::Image, &sha, size), None)
      .await
      .expect("begin ok");
    h.sinks
      .fragment(tid_for(&sha), 0, Bytes::copy_from_slice(&bytes[..4096]));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let root = h.root.clone();
    drop(h);
    let mut h2 = boot_on(root, PeerTracker::noop()).await;

    let ack = h2
      .ota
      .begin(begin_req(OtaKind::Image, &sha, size), None)
      .await
      .expect("re-begin after reboot ok");
    assert_eq!(ack.resume_from_offset, 4096, "the partial survives the power cut");

    h2.sinks
      .fragment(tid_for(&sha), 4096, Bytes::copy_from_slice(&bytes[4096..]));
    let _ = wait_for(&mut h2.events, Duration::from_secs(10), |ev| {
      matches!(
        ev,
        BridgeToGatewaySystemMsgEvent::OtaProgress(p) if matches!(p.phase, OtaPhase::Reboot)
      )
    })
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h2.reboot_calls.load(Ordering::SeqCst), 1);
  }

  #[tokio::test]
  async fn installed_webapp_applies_without_restart_or_reboot() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();

    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::InstalledWebapp,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("installed-webapp begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));

    wait_for(
      &mut h.events,
      Duration::from_secs(5),
      |ev| matches!(ev, BridgeToGatewaySystemMsgEvent::OtaProgress(p) if p.phase == OtaPhase::Writing),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(h.installed_apply_calls.load(Ordering::SeqCst), 1, "apply ran once");
    assert_eq!(
      h.restart_self_calls.load(Ordering::SeqCst),
      0,
      "install must not restart"
    );
    assert_eq!(h.reboot_calls.load(Ordering::SeqCst), 0, "install must not reboot");
  }

  #[tokio::test]
  async fn wakeword_model_push_always_reaches_a_terminal_status() {
    let model = temp_root().join("hey_bridgething.btww");
    tokio::fs::create_dir_all(model.parent().unwrap()).await.unwrap();
    unsafe { std::env::set_var("BRIDGETHING_WAKEWORD_MODEL", &model) };

    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();

    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::WakewordModel,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: Some("1.1.0".into()),
        },
        None,
      )
      .await
      .expect("wakeword begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));

    let terminal = wait_for(&mut h.events, Duration::from_secs(5), |ev| {
      matches!(
        ev,
        BridgeToGatewaySystemMsgEvent::OtaFinished(f) if f.kind == OtaKind::WakewordModel
      ) || matches!(ev, BridgeToGatewaySystemMsgEvent::OtaError(_))
    })
    .await;

    assert_eq!(h.reboot_calls.load(Ordering::SeqCst), 0, "a model swap must not reboot");
    assert_eq!(
      h.restart_self_calls.load(Ordering::SeqCst),
      0,
      "a model swap must not restart"
    );
    match terminal {
      BridgeToGatewaySystemMsgEvent::OtaFinished(f) => {
        assert_eq!(f.update_id, sha, "finished names the run the sender began");
        assert_eq!(h.wakeword_reload_calls.load(Ordering::SeqCst), 1, "mic reloaded once");
      }
      BridgeToGatewaySystemMsgEvent::OtaError(_) => {
        assert_eq!(
          h.wakeword_reload_calls.load(Ordering::SeqCst),
          0,
          "a refused model must not reload the mic"
        );
      }
      other => unreachable!("matched a terminal status above, got {other:?}"),
    }
  }

  #[tokio::test]
  async fn installed_webapp_forwards_provenance_to_apply() {
    let mut h = boot().await;
    let (bytes, sha, size) = fixture_bytes();
    let source = "https://apps.bridgething.com/catalog.json";

    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::InstalledWebapp,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: Some(source.into()),
          version: None,
        },
        None,
      )
      .await
      .expect("installed-webapp begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));

    wait_for(
      &mut h.events,
      Duration::from_secs(5),
      |ev| matches!(ev, BridgeToGatewaySystemMsgEvent::OtaProgress(p) if p.phase == OtaPhase::Writing),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let seen = h.installed_apply_provenance.lock().unwrap().clone();
    assert_eq!(
      seen,
      Some(Some(source.to_string())),
      "provenance from OtaBegin must reach the apply step"
    );
  }

  #[tokio::test]
  async fn provenance_is_rejected_for_non_installed_webapp_kinds() {
    let h = boot().await;
    let (_, sha, size) = fixture_bytes();

    let err = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::Daemon,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: Some("https://apps.bridgething.com/catalog.json".into()),
          version: None,
        },
        None,
      )
      .await
      .expect_err("daemon-kind begin carrying provenance must be rejected");
    assert!(
      err.reason.contains("provenance"),
      "reason names provenance: {}",
      err.reason
    );
  }

  #[tokio::test]
  async fn overlong_provenance_is_rejected_at_begin() {
    let h = boot().await;
    let (_, sha, size) = fixture_bytes();

    let err = h
      .ota
      .begin(
        OtaBegin {
          kind: OtaKind::InstalledWebapp,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: Some("x".repeat(libbridgething::WEBAPP_PROVENANCE_MAX_LEN + 1)),
          version: None,
        },
        None,
      )
      .await
      .expect_err("overlong provenance must be rejected before streaming");
    assert!(
      err.reason.contains("provenance"),
      "reason names provenance: {}",
      err.reason
    );
  }

  #[tokio::test]
  async fn installed_webapp_apply_failure_emits_ota_error() {
    let mut h = boot().await;
    h.installed_apply_ok.store(false, Ordering::SeqCst);
    let (bytes, sha, size) = fixture_bytes();

    h.ota
      .begin(
        OtaBegin {
          kind: OtaKind::InstalledWebapp,
          update_id: sha.clone(),
          update_url_base: None,
          transfer: TransferRef {
            id: tid_for(&sha),
            total_size: size,
            sha256: Some(sha.clone()),
          },
          patch: None,
          provenance: None,
          version: None,
        },
        None,
      )
      .await
      .expect("installed-webapp begin ok");
    h.sinks.fragment(tid_for(&sha), 0, Bytes::from(bytes));

    let err = wait_for(&mut h.events, Duration::from_secs(5), |ev| {
      matches!(ev, BridgeToGatewaySystemMsgEvent::OtaError(_))
    })
    .await;
    let BridgeToGatewaySystemMsgEvent::OtaError(e) = err else {
      unreachable!()
    };
    assert_eq!(e.code, OtaErrorCode::WriteFailed);
    assert_eq!(h.restart_self_calls.load(Ordering::SeqCst), 0);
  }
}
