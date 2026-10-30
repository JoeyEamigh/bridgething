use std::sync::Arc;

#[cfg(feature = "mic")]
use libbridgething::gateway::VoiceFrame;
use libbridgething::{
  VoiceCaptureReason, VoiceDispatchErrorCode,
  client::{BridgeToClientVoiceMsg, VoiceActivity, VoiceActivityError, VoicePhase, VoiceState},
  gateway::{
    BridgeToGatewayVoiceMsgEvent, VoiceCloseReason, VoiceCodec, VoiceFormat, VoiceStreamClose, VoiceStreamOpen,
  },
  wire::MsgMeta,
};
use tokio::{
  sync::{RwLock, mpsc, oneshot, watch},
  task::JoinHandle,
};
use uuid::Uuid;

use crate::{
  bluetooth::{Address, BluetoothMan},
  capabilities::CapabilitiesRegistry,
  net::WireEventBus,
  peer::PeerSnapshot,
};

#[cfg(feature = "mic")]
mod alsa_capture;
#[cfg(feature = "mic")]
mod encoder;
#[cfg(feature = "mic")]
mod wakeword;

const UPLINK_BACKLOG: usize = 256;

#[cfg(feature = "mic")]
type Link = Option<wakeword::WakeWordLink>;
#[cfg(not(feature = "mic"))]
type Link = ();

const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Clone)]
struct CapturedFrame {
  pcm: bytes::Bytes,
  #[cfg(feature = "mic")]
  on_target: Option<bridgething_dsp::scene::TargetEvidence>,
}

#[cfg(feature = "mic")]
const WAKEWORD_THRESHOLD: f32 = 0.35;

#[derive(Debug, Clone, Copy)]
struct Detection {
  score: f32,
}

async fn next_detection(hits: &mut Option<mpsc::Receiver<Detection>>) -> Detection {
  let Some(hits) = hits else {
    return std::future::pending().await;
  };
  match hits.recv().await {
    Some(hit) => hit,
    None => std::future::pending().await,
  }
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureFormat {
  pub sample_rate_hz: u32,
  pub channels: u16,
  pub frame_samples: u32,
}

impl Default for CaptureFormat {
  fn default() -> Self {
    Self {
      sample_rate_hz: 16_000,
      channels: 1,
      frame_samples: 256,
    }
  }
}

impl CaptureFormat {
  pub fn wire(&self) -> VoiceFormat {
    VoiceFormat {
      codec: VoiceCodec::Opus,
      sample_rate_hz: self.sample_rate_hz,
      channels: self.channels,
    }
  }
}

#[derive(Debug, Clone)]
pub struct MicConfig {
  pub format: CaptureFormat,
  pub device: String,
  pub max_uplink: std::time::Duration,
  #[cfg(feature = "mic")]
  pub dsp: bridgething_dsp::pipeline::Config,
  #[cfg(feature = "mic")]
  pub wakeword_models: Vec<std::path::PathBuf>,
  #[cfg(feature = "mic")]
  pub wakeword_threshold: f32,
}

impl Default for MicConfig {
  fn default() -> Self {
    Self {
      format: CaptureFormat::default(),
      device: "hw:0,0".to_string(),
      max_uplink: std::time::Duration::from_secs(30),
      #[cfg(feature = "mic")]
      dsp: bridgething_dsp::pipeline::Config {
        adaptation: Some(bridgething_dsp::scene::Config::default()),
        ..bridgething_dsp::pipeline::Config::default()
      },
      #[cfg(feature = "mic")]
      wakeword_models: crate::paths::wakeword_models(),
      #[cfg(feature = "mic")]
      wakeword_threshold: WAKEWORD_THRESHOLD,
    }
  }
}

#[derive(Debug, thiserror::Error)]
pub enum MicError {
  #[error("manager loop has exited")]
  Closed,
  #[error("mic capture is not available in this build")]
  Unavailable,
  #[error("alsa: {0}")]
  Alsa(String),
  #[error("opus: {0}")]
  Encode(String),
  #[error("no connected companion can transcribe a voice turn")]
  NoListener,
  #[error("turn {0} is not awaiting an answer")]
  NotResolving(Uuid),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Turn {
  stream_id: Uuid,
  reason: VoiceCaptureReason,
  score: Option<f32>,
  listener: Address,
}

impl Turn {
  fn activity(&self, phase: VoicePhase) -> VoiceActivity {
    VoiceActivity {
      stream_id: Some(self.stream_id),
      reason: Some(self.reason),
      score: self.score,
      ..VoiceActivity::new(phase)
    }
  }
}

#[derive(Debug)]
struct Resolving {
  turn: Turn,
  by: tokio::time::Instant,
}

fn claim(resolving: &mut Option<Resolving>, stream_id: Uuid) -> Result<Turn, MicError> {
  match resolving.take() {
    Some(held) if held.turn.stream_id == stream_id => Ok(held.turn),
    held => {
      *resolving = held;
      Err(MicError::NotResolving(stream_id))
    }
  }
}

fn listener_gone(turn: &Turn, peers: &PeerSnapshot) -> bool {
  !peers.companion_linked(&turn.listener)
}

fn listener_lost(turn: &Turn) -> VoiceActivity {
  VoiceActivity {
    error: Some(VoiceActivityError {
      code: VoiceDispatchErrorCode::NoCompanion,
      msg: format!("companion {} disconnected before it answered the turn", turn.listener),
    }),
    ..turn.activity(VoicePhase::Failed)
  }
}

#[derive(Debug, Default)]
struct State {
  muted: bool,
  phase: VoicePhase,
  turn: Option<Turn>,
}

impl State {
  fn snapshot(&self) -> VoiceState {
    VoiceState {
      muted: self.muted,
      capturing: self.turn.is_some(),
      phase: self.phase,
    }
  }

  fn wake_gate(&self) -> WakeGate {
    if self.muted {
      return WakeGate::Muted;
    }
    match self.turn {
      Some(turn) => WakeGate::TurnOpen(turn.stream_id),
      None => WakeGate::Free,
    }
  }

  fn settle(&mut self, resolving: &Option<Resolving>) {
    self.phase = if self.turn.is_some() {
      VoicePhase::Listening
    } else if resolving.is_some() {
      VoicePhase::Thinking
    } else {
      VoicePhase::Idle
    };
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WakeGate {
  Muted,
  TurnOpen(Uuid),
  Free,
}

#[derive(Debug)]
enum Cmd {
  Start {
    reason: VoiceCaptureReason,
    reply: oneshot::Sender<Result<Uuid, MicError>>,
  },
  Claim {
    stream_id: Uuid,
    reply: oneshot::Sender<Result<Box<VoiceActivity>, MicError>>,
  },
  Finish {
    activity: Box<VoiceActivity>,
    reply: oneshot::Sender<()>,
  },
  ReloadWakeWord {
    reply: oneshot::Sender<()>,
  },
  Stop {
    reason: VoiceCloseReason,
    reply: oneshot::Sender<()>,
  },
  Mute {
    preserve: bool,
    reply: oneshot::Sender<()>,
  },
  Unmute {
    reply: oneshot::Sender<()>,
  },
}

#[derive(Debug, Clone)]
pub struct MicManager {
  state: Arc<RwLock<State>>,
  tx: mpsc::Sender<Cmd>,
}

impl MicManager {
  pub async fn init(
    bus: WireEventBus,
    bluetooth: BluetoothMan,
    capabilities: CapabilitiesRegistry,
    peers: watch::Receiver<PeerSnapshot>,
    config: MicConfig,
  ) -> MicManagerInit {
    let state = Arc::new(RwLock::new(State::default()));
    let (tx, rx) = mpsc::channel(16);
    let manager = Self {
      state: state.clone(),
      tx,
    };
    MicManagerInit {
      manager,
      rx,
      state,
      bus,
      bluetooth,
      capabilities,
      peers,
      config,
    }
  }

  pub async fn snapshot(&self) -> VoiceState {
    self.state.read().await.snapshot()
  }

  pub async fn push_to_talk(&self) -> Result<Uuid, MicError> {
    self.open(VoiceCaptureReason::PushToTalk).await
  }

  pub async fn open(&self, reason: VoiceCaptureReason) -> Result<Uuid, MicError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .tx
      .send(Cmd::Start {
        reason,
        reply: reply_tx,
      })
      .await
      .map_err(|_| MicError::Closed)?;
    reply_rx.await.map_err(|_| MicError::Closed)?
  }

  pub async fn reload_wakeword(&self) -> Result<(), MicError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .tx
      .send(Cmd::ReloadWakeWord { reply: reply_tx })
      .await
      .map_err(|_| MicError::Closed)?;
    reply_rx.await.map_err(|_| MicError::Closed)
  }

  pub async fn claim(&self, stream_id: Uuid) -> Result<VoiceActivity, MicError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .tx
      .send(Cmd::Claim {
        stream_id,
        reply: reply_tx,
      })
      .await
      .map_err(|_| MicError::Closed)?;
    reply_rx.await.map_err(|_| MicError::Closed)?.map(|activity| *activity)
  }

  pub async fn finish(&self, activity: VoiceActivity) -> Result<(), MicError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .tx
      .send(Cmd::Finish {
        activity: Box::new(activity),
        reply: reply_tx,
      })
      .await
      .map_err(|_| MicError::Closed)?;
    reply_rx.await.map_err(|_| MicError::Closed)
  }

  pub async fn cancel(&self) -> Result<(), MicError> {
    self.stop_with(VoiceCloseReason::Cancelled).await
  }

  pub async fn stop_with(&self, reason: VoiceCloseReason) -> Result<(), MicError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .tx
      .send(Cmd::Stop {
        reason,
        reply: reply_tx,
      })
      .await
      .map_err(|_| MicError::Closed)?;
    reply_rx.await.map_err(|_| MicError::Closed)
  }

  pub async fn set_muted(&self, muted: bool, preserve: bool) -> Result<(), MicError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    let cmd = if muted {
      Cmd::Mute {
        preserve,
        reply: reply_tx,
      }
    } else {
      Cmd::Unmute { reply: reply_tx }
    };
    self.tx.send(cmd).await.map_err(|_| MicError::Closed)?;
    reply_rx.await.map_err(|_| MicError::Closed)
  }
}

pub struct MicManagerInit {
  pub manager: MicManager,
  rx: mpsc::Receiver<Cmd>,
  state: Arc<RwLock<State>>,
  bus: WireEventBus,
  bluetooth: BluetoothMan,
  capabilities: CapabilitiesRegistry,
  peers: watch::Receiver<PeerSnapshot>,
  config: MicConfig,
}

impl MicManagerInit {
  pub fn spawn(self) -> (MicManager, JoinHandle<()>) {
    let manager = self.manager.clone();
    let handle = tokio::spawn(run_loop(
      self.rx,
      self.state,
      self.bus,
      self.bluetooth,
      self.capabilities,
      self.peers,
      self.config,
    ));
    (manager, handle)
  }
}

async fn run_loop(
  mut rx: mpsc::Receiver<Cmd>,
  state: Arc<RwLock<State>>,
  bus: WireEventBus,
  bluetooth: BluetoothMan,
  capabilities: CapabilitiesRegistry,
  mut peers: watch::Receiver<PeerSnapshot>,
  config: MicConfig,
) {
  let (frame_tx, mut frame_rx) = mpsc::channel::<CapturedFrame>(64);
  #[cfg(feature = "mic")]
  let (mut link, mut hits) = match wakeword::WakeWordLink::spawn(&config.wakeword_models, config.wakeword_threshold) {
    Some((link, hits)) => (Some(link), Some(hits)),
    None => (None, None),
  };
  #[cfg(not(feature = "mic"))]
  let (mut link, mut hits): (Link, Option<mpsc::Receiver<Detection>>) = ((), None);
  #[cfg(feature = "mic")]
  let mut capture = open_capture(&config, &frame_tx);
  #[cfg(not(feature = "mic"))]
  let mut capture: Option<Capture> = None;
  let mut stream: Option<Stream> = None;
  #[cfg(feature = "mic")]
  let mut level = Level::new();

  let mut resolving: Option<Resolving> = None;

  loop {
    tokio::select! {
      cmd = rx.recv() => {
        let Some(cmd) = cmd else { break; };
        handle_cmd(cmd, &state, &bus, &bluetooth, &capabilities, &config, &frame_tx, &mut capture, &mut stream, &mut resolving, &mut link, &mut hits).await;
      }
      Some(frame) = frame_rx.recv() => {
        #[cfg(feature = "mic")]
        if let Some(link) = link.as_ref() {
          link.offer(frame.pcm.clone());
        }
        #[cfg(feature = "mic")]
        level.convert(&frame);
        if let Some(open) = stream.as_mut() {
          #[cfg(feature = "mic")]
          let ended = open.endpoint(&level, frame.on_target);
          #[cfg(not(feature = "mic"))]
          let ended = open.endpoint();
          if !open.forward(frame.pcm) {
            tracing::error!("gateway fell a full uplink buffer behind; ending the stream rather than holing it");
            stop_stream(&state, &bus, &capture, &mut stream, VoiceCloseReason::Error, &mut resolving, None).await;
          } else if let Some(reason) = ended {
            stop_stream(&state, &bus, &capture, &mut stream, reason, &mut resolving, None).await;
          }
        }
        #[cfg(feature = "mic")]
        level.remember();
      }
      hit = next_detection(&mut hits) => {
        on_wake_word(hit, &state, &bus, &bluetooth, &capabilities, &config, &frame_tx, &mut capture, &mut stream).await;
      }
      () = past_cap(stream.as_ref()) => {
        tracing::warn!(cap = ?config.max_uplink, "uplink hit the cap; answering with what the turn already carried");
        stop_stream(&state, &bus, &capture, &mut stream, VoiceCloseReason::EndOfSpeech, &mut resolving, None).await;
      }
      () = past_deadline(resolving.as_ref().map(|held| held.by)) => {
        if let Some(expired) = resolving.take() {
          tracing::warn!(wait = ?RESOLVE_TIMEOUT, stream = %expired.turn.stream_id, "companion never answered a closed turn");
          let activity = VoiceActivity {
            error: Some(VoiceActivityError {
              code: VoiceDispatchErrorCode::Internal,
              msg: "companion did not answer before the resolve timeout".into(),
            }),
            ..expired.turn.activity(VoicePhase::Failed)
          };
          state.write().await.settle(&resolving);
          broadcast_activity(&bus, activity).await;
          broadcast_state(&bus, &state).await;
        }
      }
      () = peers_changed(&mut peers) => {
        let snapshot = peers.borrow_and_update().clone();
        drop_lost_listener(&state, &bus, &capture, &mut stream, &mut resolving, &snapshot).await;
      }
      else => break,
    }
  }

  if let Some(c) = capture {
    c.stop();
  }
  tracing::debug!("mic manager loop exiting");
}

fn open_capture(config: &MicConfig, frames: &mpsc::Sender<CapturedFrame>) -> Option<Capture> {
  match Capture::start(config.clone(), frames.clone()) {
    Ok(capture) => {
      tracing::info!(device = config.device, "microphone open");
      Some(capture)
    }
    Err(err) => {
      tracing::warn!("microphone unavailable: {err}");
      None
    }
  }
}

#[allow(clippy::too_many_arguments)]
async fn on_wake_word(
  hit: Detection,
  state: &Arc<RwLock<State>>,
  bus: &WireEventBus,
  bluetooth: &BluetoothMan,
  capabilities: &CapabilitiesRegistry,
  config: &MicConfig,
  frames: &mpsc::Sender<CapturedFrame>,
  capture: &mut Option<Capture>,
  stream: &mut Option<Stream>,
) {
  let gate = state.read().await.wake_gate();
  match gate {
    WakeGate::Muted => {
      tracing::debug!("wake word fired while muted; ignoring");
      return;
    }
    WakeGate::TurnOpen(open) => {
      tracing::debug!(score = hit.score, stream = %open, "wake word fired during an open turn; ignoring");
      return;
    }
    WakeGate::Free => {}
  }
  if let Some(open) = capture.as_ref() {
    open.mark_target();
    open.hold_noise(true);
  }
  match start_stream(
    state,
    bus,
    bluetooth,
    capabilities,
    config,
    frames,
    capture,
    stream,
    VoiceCaptureReason::WakeWord,
    Some(hit.score),
  )
  .await
  {
    Ok(id) => tracing::info!(score = hit.score, stream = %id, "wake word opened the uplink"),
    Err(err) => {
      if let Some(open) = capture.as_ref() {
        open.hold_noise(false);
      }
      tracing::warn!("wake word could not open the uplink: {err}");
    }
  }
}

#[allow(clippy::too_many_arguments)]
async fn handle_cmd(
  cmd: Cmd,
  state: &Arc<RwLock<State>>,
  bus: &WireEventBus,
  bluetooth: &BluetoothMan,
  capabilities: &CapabilitiesRegistry,
  config: &MicConfig,
  frame_tx: &mpsc::Sender<CapturedFrame>,
  capture: &mut Option<Capture>,
  stream: &mut Option<Stream>,
  resolving: &mut Option<Resolving>,
  link: &mut Link,
  hits: &mut Option<mpsc::Receiver<Detection>>,
) {
  match cmd {
    Cmd::Start { reason, reply } => {
      let outcome = start_stream(
        state,
        bus,
        bluetooth,
        capabilities,
        config,
        frame_tx,
        capture,
        stream,
        reason,
        None,
      )
      .await;
      let _ = reply.send(outcome);
    }
    Cmd::Stop { reason, reply } => {
      stop_stream(state, bus, capture, stream, reason, resolving, None).await;
      let _ = reply.send(());
    }
    Cmd::Claim { stream_id, reply } => {
      let claimed = claim(resolving, stream_id).map(|turn| Box::new(turn.activity(VoicePhase::Done)));
      match &claimed {
        Ok(_) => tracing::debug!(stream = %stream_id, "a companion answered the turn"),
        Err(err) => tracing::debug!(stream = %stream_id, "refusing a dispatch: {err}"),
      }
      let _ = reply.send(claimed);
    }
    Cmd::Finish { activity, reply } => {
      state.write().await.settle(resolving);
      broadcast_activity(bus, *activity).await;
      broadcast_state(bus, state).await;
      let _ = reply.send(());
    }
    Cmd::ReloadWakeWord { reply } => {
      #[cfg(feature = "mic")]
      {
        (*link, *hits) = match wakeword::WakeWordLink::spawn(&config.wakeword_models, config.wakeword_threshold) {
          Some((next, rx)) => (Some(next), Some(rx)),
          None => (None, None),
        };
      }
      #[cfg(not(feature = "mic"))]
      {
        let _ = (link, hits);
      }
      let _ = reply.send(());
    }
    Cmd::Mute { preserve, reply } => {
      let was_open = {
        let mut guard = state.write().await;
        guard.muted = true;
        guard.turn.is_some()
      };
      if let Some(open) = capture.take() {
        open.stop();
      }
      if was_open && !preserve {
        stop_stream(state, bus, capture, stream, VoiceCloseReason::Muted, resolving, None).await;
      } else {
        broadcast_state(bus, state).await;
      }
      let _ = reply.send(());
    }
    Cmd::Unmute { reply } => {
      {
        let mut guard = state.write().await;
        guard.muted = false;
      }
      if capture.is_none() {
        *capture = open_capture(config, frame_tx);
      }
      broadcast_state(bus, state).await;
      let _ = reply.send(());
    }
  }
}

#[allow(clippy::too_many_arguments)]
async fn start_stream(
  state: &Arc<RwLock<State>>,
  bus: &WireEventBus,
  bluetooth: &BluetoothMan,
  capabilities: &CapabilitiesRegistry,
  config: &MicConfig,
  frame_tx: &mpsc::Sender<CapturedFrame>,
  capture: &mut Option<Capture>,
  stream: &mut Option<Stream>,
  reason: VoiceCaptureReason,
  score: Option<f32>,
) -> Result<Uuid, MicError> {
  {
    let guard = state.read().await;
    if guard.muted {
      return Err(MicError::Unavailable);
    }
    if let Some(open) = guard.turn {
      return Ok(open.stream_id);
    }
  }

  if capture.is_none() {
    *capture = open_capture(config, frame_tx);
  }
  if capture.is_none() {
    return Err(MicError::Unavailable);
  }

  let Some(listener) = capabilities.voice_listener() else {
    let refused = VoiceActivity {
      reason: Some(reason),
      score,
      error: Some(VoiceActivityError {
        code: VoiceDispatchErrorCode::NoCompanion,
        msg: MicError::NoListener.to_string(),
      }),
      ..VoiceActivity::new(VoicePhase::Failed)
    };
    broadcast_activity(bus, refused).await;
    return Err(MicError::NoListener);
  };

  let turn = Turn {
    stream_id: Uuid::now_v7(),
    reason,
    score,
    listener,
  };
  *stream = Some(Stream::open(turn, config.max_uplink, config.format, bluetooth.clone()));
  {
    let mut guard = state.write().await;
    guard.turn = Some(turn);
    guard.phase = VoicePhase::Listening;
  }

  bluetooth
    .gateway_man
    .send_event(
      listener,
      BridgeToGatewayVoiceMsgEvent::StreamOpen(VoiceStreamOpen {
        stream_id: turn.stream_id,
        format: config.format.wire(),
        reason,
      }),
    )
    .await;
  broadcast_activity(bus, turn.activity(VoicePhase::Listening)).await;
  broadcast_state(bus, state).await;
  Ok(turn.stream_id)
}

fn settled_phase(reason: VoiceCloseReason) -> VoicePhase {
  match reason {
    VoiceCloseReason::EndOfSpeech => VoicePhase::Thinking,
    VoiceCloseReason::Error => VoicePhase::Failed,
    VoiceCloseReason::Cancelled | VoiceCloseReason::Muted => VoicePhase::Idle,
  }
}

async fn stop_stream(
  state: &Arc<RwLock<State>>,
  bus: &WireEventBus,
  capture: &Option<Capture>,
  stream: &mut Option<Stream>,
  reason: VoiceCloseReason,
  resolving: &mut Option<Resolving>,
  ended: Option<VoiceActivity>,
) {
  let next = settled_phase(reason);
  if let Some(open) = capture.as_ref() {
    open.hold_noise(false);
  }

  let mut guard = state.write().await;
  let Some(turn) = guard.turn.take() else {
    return;
  };
  let superseded = if next == VoicePhase::Thinking {
    resolving.replace(Resolving {
      turn,
      by: tokio::time::Instant::now() + RESOLVE_TIMEOUT,
    })
  } else {
    None
  };
  guard.settle(resolving);
  drop(guard);

  if let Some(open) = stream.take() {
    open.close(reason);
  }
  if let Some(prior) = superseded {
    tracing::debug!(stream = %prior.turn.stream_id, "a newer turn closed before this one was answered");
    broadcast_activity(bus, prior.turn.activity(VoicePhase::Idle)).await;
  }
  broadcast_activity(bus, ended.unwrap_or_else(|| turn.activity(next))).await;
  broadcast_state(bus, state).await;
}

async fn drop_lost_listener(
  state: &Arc<RwLock<State>>,
  bus: &WireEventBus,
  capture: &Option<Capture>,
  stream: &mut Option<Stream>,
  resolving: &mut Option<Resolving>,
  peers: &PeerSnapshot,
) {
  let open = state.read().await.turn.filter(|turn| listener_gone(turn, peers));
  if let Some(turn) = open {
    tracing::warn!(stream = %turn.stream_id, listener = %turn.listener, "the companion listening to the turn dropped");
    stop_stream(
      state,
      bus,
      capture,
      stream,
      VoiceCloseReason::Error,
      resolving,
      Some(listener_lost(&turn)),
    )
    .await;
  }

  if !resolving.as_ref().is_some_and(|held| listener_gone(&held.turn, peers)) {
    return;
  }
  let Some(orphaned) = resolving.take() else {
    return;
  };
  tracing::warn!(
    stream = %orphaned.turn.stream_id,
    listener = %orphaned.turn.listener,
    "the companion resolving the turn dropped"
  );
  state.write().await.settle(resolving);
  broadcast_activity(bus, listener_lost(&orphaned.turn)).await;
  broadcast_state(bus, state).await;
}

async fn peers_changed(peers: &mut watch::Receiver<PeerSnapshot>) {
  if peers.changed().await.is_err() {
    std::future::pending::<()>().await;
  }
}

async fn past_cap(stream: Option<&Stream>) {
  match stream {
    Some(open) => tokio::time::sleep_until(open.deadline).await,
    None => std::future::pending().await,
  }
}

async fn past_deadline(at: Option<tokio::time::Instant>) {
  match at {
    Some(at) => tokio::time::sleep_until(at).await,
    None => std::future::pending().await,
  }
}

async fn broadcast_state(bus: &WireEventBus, state: &Arc<RwLock<State>>) {
  let snapshot = state.read().await.snapshot();
  if let Err(errors) = bus
    .broadcast(BridgeToClientVoiceMsg::StateChanged(snapshot), MsgMeta::Event)
    .await
  {
    tracing::trace!("mic state broadcast had {} ws error(s)", errors.len());
  }
}

async fn broadcast_activity(bus: &WireEventBus, activity: VoiceActivity) {
  if let Err(errors) = bus
    .broadcast(BridgeToClientVoiceMsg::Activity(activity), MsgMeta::Event)
    .await
  {
    tracing::trace!("voice activity broadcast had {} ws error(s)", errors.len());
  }
}

#[derive(Debug)]
struct Capture {
  #[cfg(feature = "mic")]
  worker: alsa_capture::WorkerHandle,
}

impl Capture {
  #[cfg(feature = "mic")]
  fn start(config: MicConfig, frames: mpsc::Sender<CapturedFrame>) -> Result<Self, MicError> {
    Ok(Self {
      worker: alsa_capture::WorkerHandle::start(config, frames)?,
    })
  }

  #[cfg(not(feature = "mic"))]
  fn start(_config: MicConfig, _frames: mpsc::Sender<CapturedFrame>) -> Result<Self, MicError> {
    if cfg!(feature = "test-tap") {
      Ok(Self {})
    } else {
      Err(MicError::Unavailable)
    }
  }

  #[cfg(feature = "mic")]
  fn mark_target(&self) {
    self.worker.mark_target();
  }

  #[cfg(not(feature = "mic"))]
  fn mark_target(&self) {}

  #[cfg(feature = "mic")]
  fn hold_noise(&self, holding: bool) {
    self.worker.hold_noise(holding);
  }

  #[cfg(not(feature = "mic"))]
  fn hold_noise(&self, _holding: bool) {}

  fn stop(self) {
    #[cfg(feature = "mic")]
    self.worker.stop();
  }
}

#[cfg(feature = "mic")]
struct Level {
  reference: bridgething_dsp::vad::LevelReference,
  samples: Vec<f32>,
}

#[cfg(feature = "mic")]
impl Level {
  fn new() -> Self {
    Self {
      reference: bridgething_dsp::vad::LevelReference::new(bridgething_dsp::vad::Leveling::default()),
      samples: Vec::with_capacity(1024),
    }
  }

  fn convert(&mut self, frame: &CapturedFrame) {
    self.samples.clear();
    bridgething_dsp::pipeline::from_pcm16(&frame.pcm, &mut self.samples);
  }

  fn remember(&mut self) {
    self.reference.observe(&self.samples);
  }

  fn gain(&self) -> f32 {
    let gain = self.reference.gain();
    tracing::debug!(
      gain_db = 20.0 * gain.log10(),
      talker_dbfs = self.reference.talker_dbfs(),
      floor_dbfs = self.reference.floor_dbfs(),
      "leveled the turn to the wake word"
    );
    gain
  }
}

#[cfg(feature = "mic")]
#[derive(Default)]
struct Endpointer {
  vad: Option<bridgething_dsp::vad::VoiceEndpointer>,
}

#[cfg(feature = "mic")]
impl Endpointer {
  fn observe(
    &mut self,
    level: &Level,
    on_target: Option<bridgething_dsp::scene::TargetEvidence>,
  ) -> Option<VoiceCloseReason> {
    let vad = self.vad.get_or_insert_with(|| {
      bridgething_dsp::vad::VoiceEndpointer::new(bridgething_dsp::vad::Config::default(), level.gain())
    });
    let end = vad.observe(&level.samples, on_target)?;
    tracing::debug!(?end, "the endpointer ended the turn");
    if end == bridgething_dsp::vad::TurnEnd::NoOnset {
      let trace = vad.onset_trace();
      tracing::debug!(
        peak_acoustic_run = trace.peak_acoustic_run,
        peak_gated_run = trace.peak_gated_run,
        vetoed_frames = trace.vetoed_frames,
        abstained_frames = trace.abstained_frames,
        max_score = trace.max_score,
        "no speech onset in the window"
      );
    }
    Some(close_for(end))
  }
}

#[cfg(feature = "mic")]
fn close_for(end: bridgething_dsp::vad::TurnEnd) -> VoiceCloseReason {
  match end {
    bridgething_dsp::vad::TurnEnd::NoOnset => VoiceCloseReason::Cancelled,
    bridgething_dsp::vad::TurnEnd::SilenceAfterSpeech => VoiceCloseReason::EndOfSpeech,
  }
}

#[cfg(feature = "mic")]
fn endpointer_for(reason: VoiceCaptureReason) -> Option<Endpointer> {
  (reason == VoiceCaptureReason::WakeWord).then(Endpointer::default)
}

struct Stream {
  deadline: tokio::time::Instant,
  pcm: mpsc::Sender<bytes::Bytes>,
  close: oneshot::Sender<VoiceCloseReason>,
  #[cfg(feature = "mic")]
  endpointer: Option<Endpointer>,
  _uplink: JoinHandle<()>,
}

impl Stream {
  fn open(turn: Turn, cap: std::time::Duration, format: CaptureFormat, bluetooth: BluetoothMan) -> Self {
    let (pcm, rx) = mpsc::channel(UPLINK_BACKLOG);
    let (close, closed) = oneshot::channel();
    let uplink = Uplink {
      bluetooth,
      listener: turn.listener,
      stream_id: turn.stream_id,
    };
    Self {
      deadline: tokio::time::Instant::now() + cap,
      pcm,
      close,
      #[cfg(feature = "mic")]
      endpointer: endpointer_for(turn.reason),
      _uplink: tokio::spawn(run_uplink(rx, closed, uplink, format)),
    }
  }

  #[cfg(feature = "mic")]
  fn endpoint(
    &mut self,
    level: &Level,
    on_target: Option<bridgething_dsp::scene::TargetEvidence>,
  ) -> Option<VoiceCloseReason> {
    self.endpointer.as_mut()?.observe(level, on_target)
  }

  #[cfg(not(feature = "mic"))]
  fn endpoint(&mut self) -> Option<VoiceCloseReason> {
    None
  }

  fn forward(&mut self, pcm: bytes::Bytes) -> bool {
    self.pcm.try_send(pcm).is_ok()
  }

  fn close(self, reason: VoiceCloseReason) {
    let _ = self.close.send(reason);
  }
}

struct Uplink {
  bluetooth: BluetoothMan,
  listener: Address,
  stream_id: Uuid,
}

impl Uplink {
  async fn send(&self, event: BridgeToGatewayVoiceMsgEvent) {
    self.bluetooth.gateway_man.send_event(self.listener, event).await;
  }

  #[cfg(feature = "mic")]
  async fn send_packets(&self, seq: &mut u32, packets: impl IntoIterator<Item = bytes::Bytes>) {
    for packet in packets {
      let frame = VoiceFrame {
        stream_id: self.stream_id,
        seq: *seq,
        packet,
      };
      *seq = seq.wrapping_add(1);
      self.send(BridgeToGatewayVoiceMsgEvent::Frame(frame)).await;
    }
  }

  async fn close(&self, closed: oneshot::Receiver<VoiceCloseReason>) {
    let Ok(reason) = closed.await else { return };
    self
      .send(BridgeToGatewayVoiceMsgEvent::StreamClose(VoiceStreamClose {
        stream_id: self.stream_id,
        reason,
      }))
      .await;
  }
}

#[cfg(feature = "mic")]
async fn run_uplink(
  mut pcm: mpsc::Receiver<bytes::Bytes>,
  closed: oneshot::Receiver<VoiceCloseReason>,
  uplink: Uplink,
  format: CaptureFormat,
) {
  let mut encoder = match encoder::Encoder::new(format) {
    Ok(encoder) => encoder,
    Err(err) => {
      tracing::error!("could not open the opus encoder, the uplink carries nothing: {err}");
      uplink.close(closed).await;
      return;
    }
  };
  let mut seq = 0u32;
  let mut packets = Vec::new();

  while let Some(chunk) = pcm.recv().await {
    if let Err(err) = encoder.push(&chunk, &mut packets) {
      tracing::error!("opus encode failed mid-utterance: {err}");
      uplink.close(closed).await;
      return;
    }
    uplink.send_packets(&mut seq, packets.drain(..)).await;
  }
  match encoder.flush() {
    Ok(tail) => uplink.send_packets(&mut seq, tail).await,
    Err(err) => tracing::warn!("the tail of the utterance would not encode: {err}"),
  }
  uplink.close(closed).await;
}

#[cfg(not(feature = "mic"))]
async fn run_uplink(
  mut pcm: mpsc::Receiver<bytes::Bytes>,
  closed: oneshot::Receiver<VoiceCloseReason>,
  uplink: Uplink,
  _format: CaptureFormat,
) {
  while pcm.recv().await.is_some() {}
  uplink.close(closed).await;
}

#[cfg(test)]
mod tests {
  use libbridgething::{GatewayInfo, Peer, PeerCompanionStatus};

  use super::*;

  fn listener() -> Address {
    Address::new([0xFE, 0xED, 0x00, 0x00, 0x00, 0x01])
  }

  fn wake_turn() -> Turn {
    Turn {
      stream_id: Uuid::now_v7(),
      reason: VoiceCaptureReason::WakeWord,
      score: Some(0.94),
      listener: listener(),
    }
  }

  fn listening() -> State {
    State {
      phase: VoicePhase::Listening,
      turn: Some(wake_turn()),
      ..State::default()
    }
  }

  fn resolving(turn: Turn) -> Option<Resolving> {
    Some(Resolving {
      turn,
      by: tokio::time::Instant::now() + RESOLVE_TIMEOUT,
    })
  }

  fn peers_with(addr: Address, companion: PeerCompanionStatus) -> PeerSnapshot {
    let mut peer = Peer::new(libbridgething::Device {
      name: "phone".into(),
      device_type: libbridgething::DeviceType::Android,
      id: addr.to_string(),
      kind: libbridgething::LinkKind::Bluetooth,
      default: false,
    });
    peer.companion = companion;
    PeerSnapshot {
      peers: [(addr, peer)].into_iter().collect(),
    }
  }

  #[test]
  fn a_listening_activity_carries_the_turn_context() {
    let turn = wake_turn();
    let activity = turn.activity(VoicePhase::Listening);
    assert_eq!(activity.reason, Some(VoiceCaptureReason::WakeWord));
    assert_eq!(activity.score, Some(0.94));
    assert_eq!(activity.stream_id, Some(turn.stream_id));
  }

  #[test]
  fn a_turn_that_ended_by_itself_is_still_answered() {
    assert_eq!(settled_phase(VoiceCloseReason::EndOfSpeech), VoicePhase::Thinking);
  }

  #[test]
  fn an_explicit_stop_drops_the_turn_rather_than_dispatching_it() {
    assert_eq!(settled_phase(VoiceCloseReason::Cancelled), VoicePhase::Idle);
    assert_eq!(settled_phase(VoiceCloseReason::Muted), VoicePhase::Idle);
    assert_eq!(settled_phase(VoiceCloseReason::Error), VoicePhase::Failed);
  }

  #[cfg(feature = "mic")]
  #[test]
  fn a_wake_word_nobody_followed_is_dropped_rather_than_transcribed() {
    use bridgething_dsp::vad::TurnEnd;

    assert_eq!(close_for(TurnEnd::NoOnset), VoiceCloseReason::Cancelled);
    assert_eq!(close_for(TurnEnd::SilenceAfterSpeech), VoiceCloseReason::EndOfSpeech);
  }

  #[cfg(feature = "mic")]
  #[test]
  fn only_a_wake_word_turn_is_endpointed() {
    assert!(endpointer_for(VoiceCaptureReason::WakeWord).is_some());
    assert!(
      endpointer_for(VoiceCaptureReason::PushToTalk).is_none(),
      "a held button is the user's own endpoint"
    );
  }

  #[test]
  fn the_snapshot_reports_phase_alongside_mute_and_capture() {
    let snapshot = listening().snapshot();
    assert_eq!(snapshot.phase, VoicePhase::Listening);
    assert!(snapshot.capturing);
    assert!(!snapshot.muted);
  }

  #[test]
  fn a_wake_word_during_an_open_push_to_talk_turn_is_ignored() {
    let turn = Turn {
      reason: VoiceCaptureReason::PushToTalk,
      score: None,
      ..wake_turn()
    };
    let state = State {
      phase: VoicePhase::Listening,
      turn: Some(turn),
      ..State::default()
    };
    assert_eq!(
      state.wake_gate(),
      WakeGate::TurnOpen(turn.stream_id),
      "a wake word mid-turn must not re-steer the beamformer or claim to open the turn"
    );
  }

  #[test]
  fn a_wake_word_while_the_last_turn_resolves_opens_a_fresh_one() {
    let mut state = State::default();
    let held = resolving(wake_turn());
    state.settle(&held);
    assert_eq!(state.phase, VoicePhase::Thinking);
    assert_eq!(state.wake_gate(), WakeGate::Free);
  }

  #[test]
  fn a_muted_mic_ignores_the_wake_word_before_touching_the_beamformer() {
    let state = State {
      muted: true,
      ..listening()
    };
    assert_eq!(state.wake_gate(), WakeGate::Muted);
  }

  #[test]
  fn the_first_answer_for_the_resolving_turn_claims_it_and_a_second_is_refused() {
    let turn = wake_turn();
    let mut held = resolving(turn);
    assert_eq!(claim(&mut held, turn.stream_id).ok(), Some(turn));
    assert!(
      matches!(claim(&mut held, turn.stream_id), Err(MicError::NotResolving(id)) if id == turn.stream_id),
      "a second dispatch of one turn must never reach the player"
    );
  }

  #[test]
  fn an_answer_for_another_turn_leaves_the_resolving_one_waiting() {
    let turn = wake_turn();
    let mut held = resolving(turn);
    assert!(matches!(
      claim(&mut held, Uuid::now_v7()),
      Err(MicError::NotResolving(_))
    ));
    assert_eq!(
      held.as_ref().map(|h| h.turn),
      Some(turn),
      "a stray dispatch must not settle the turn it does not name"
    );
    assert_eq!(claim(&mut held, turn.stream_id).ok(), Some(turn));
  }

  #[test]
  fn an_answer_with_nothing_resolving_is_refused() {
    let mut held = None;
    assert!(matches!(
      claim(&mut held, Uuid::now_v7()),
      Err(MicError::NotResolving(_))
    ));
  }

  #[test]
  fn the_resting_phase_follows_what_is_still_open() {
    let mut state = listening();
    state.settle(&None);
    assert_eq!(state.phase, VoicePhase::Listening);
    state.turn = None;
    state.settle(&resolving(wake_turn()));
    assert_eq!(state.phase, VoicePhase::Thinking);
    state.settle(&None);
    assert_eq!(state.phase, VoicePhase::Idle);
  }

  #[test]
  fn a_listener_that_left_the_peer_set_is_gone() {
    let turn = wake_turn();
    let connected = peers_with(turn.listener, PeerCompanionStatus::Connected(GatewayInfo::default()));
    assert!(!listener_gone(&turn, &connected));
    assert!(listener_gone(
      &turn,
      &peers_with(turn.listener, PeerCompanionStatus::None)
    ));
    assert!(listener_gone(&turn, &PeerSnapshot::default()));
  }

  #[test]
  fn a_lost_listener_fails_the_turn_it_held_and_says_why() {
    let turn = wake_turn();
    let activity = listener_lost(&turn);
    assert_eq!(activity.phase, VoicePhase::Failed);
    assert_eq!(activity.stream_id, Some(turn.stream_id));
    assert_eq!(
      activity.error.map(|e| e.code),
      Some(VoiceDispatchErrorCode::NoCompanion)
    );
  }
}
