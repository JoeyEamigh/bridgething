use std::{
  collections::{BTreeMap, HashMap},
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
};

use bridgething_gateway::{OutboundLink, OutboundLinkExt, VoiceHandler};
use bytes::Bytes;
use libbridgething::{
  NluResolvedIntent, NluStage, VoiceCaptureReason,
  gateway::{
    GatewayToBridgeVoiceMsgCommand, VoiceCloseReason, VoiceDispatch, VoiceDispatchFailed, VoiceDispatched, VoiceFormat,
    VoiceFrame, VoiceStreamClose, VoiceStreamOpen,
  },
  wire::WireError,
};
use tokio::{sync::oneshot, task::AbortHandle};
use uuid::Uuid;

use crate::{
  backend::{SpeechRecognizer, TranscriptionSink},
  voice::{
    controller::{Resolution, VoiceController, no_intent},
    opus,
  },
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
  #[error("{0}")]
  Failed(String),
}

#[async_trait::async_trait]
pub trait VoiceCatalogResolver: Send + Sync {
  async fn decorate(&self, resolved: NluResolvedIntent) -> Result<NluResolvedIntent, CatalogError>;
}

pub enum VoiceTurnPhase<'a> {
  Listening,
  Resolved(&'a NluResolvedIntent),
  Cancelled,
}

pub struct VoiceTurnUpdate<'a> {
  pub stream_id: Uuid,
  pub reason: VoiceCaptureReason,
  pub phase: VoiceTurnPhase<'a>,
}

pub trait VoiceTurnObserver: Send + Sync {
  fn turn_changed(&self, device_id: &str, update: VoiceTurnUpdate<'_>);
}

fn unresolved_catalog_command(resolved: &NluResolvedIntent) -> bool {
  resolved.slots.uri.is_none()
    && match resolved.intent.as_str() {
      "PLAY" => resolved.slots.has_catalog_slots(),
      "ADD_TO_QUEUE" => true,
      _ => false,
    }
}

pub struct VoiceDispatcherDeps {
  pub recognizer: Option<Arc<dyn SpeechRecognizer>>,
  pub controller: Arc<VoiceController>,
  pub link: Arc<dyn OutboundLink>,
  pub resolver: Option<Arc<dyn VoiceCatalogResolver>>,
  pub observer: Arc<dyn VoiceTurnObserver>,
  pub device_id: String,
}

pub struct VoiceDispatcher {
  inner: Arc<Inner>,
}

impl VoiceDispatcher {
  pub fn new(deps: VoiceDispatcherDeps) -> Self {
    Self {
      inner: Arc::new(Inner {
        recognizer: deps.recognizer,
        controller: deps.controller,
        link: deps.link,
        resolver: deps.resolver,
        observer: deps.observer,
        device_id: deps.device_id,
        captures: Mutex::new(HashMap::new()),
        running: Mutex::new(Vec::new()),
        tail: Mutex::new(None),
        prewarmed: AtomicBool::new(false),
      }),
    }
  }

  pub fn stop(&self) {
    for task in self.inner.running.lock().unwrap().drain(..) {
      task.abort();
    }
    *self.inner.tail.lock().unwrap() = None;
    let captures: Vec<(Uuid, Capture)> = self.inner.captures.lock().unwrap().drain().collect();
    for (stream_id, capture) in captures {
      self.inner.notify(stream_id, capture.reason, VoiceTurnPhase::Cancelled);
    }
    self.inner.prewarmed.store(false, Ordering::SeqCst);
  }
}

impl VoiceHandler for VoiceDispatcher {
  async fn stream_open(&self, payload: VoiceStreamOpen) -> Result<(), WireError> {
    let replaced = self.inner.captures.lock().unwrap().insert(
      payload.stream_id,
      Capture {
        format: payload.format,
        reason: payload.reason,
        packets: BTreeMap::new(),
      },
    );
    if let Some(replaced) = replaced {
      self
        .inner
        .notify(payload.stream_id, replaced.reason, VoiceTurnPhase::Cancelled);
    }
    self
      .inner
      .notify(payload.stream_id, payload.reason, VoiceTurnPhase::Listening);
    if !self.inner.prewarmed.swap(true, Ordering::SeqCst) {
      let controller = self.inner.controller.clone();
      self
        .inner
        .track(tokio::spawn(async move { controller.prewarm().await }).abort_handle());
    }
    Ok(())
  }

  async fn frame(&self, payload: VoiceFrame) -> Result<(), WireError> {
    if let Some(capture) = self.inner.captures.lock().unwrap().get_mut(&payload.stream_id) {
      capture.packets.insert(payload.seq, payload.packet);
    }
    Ok(())
  }

  async fn stream_close(&self, payload: VoiceStreamClose) -> Result<(), WireError> {
    let Some(turn) = self.inner.close(payload) else {
      return Ok(());
    };
    let (done, after) = self.inner.sequence();
    let inner = self.inner.clone();
    self
      .inner
      .track(tokio::spawn(async move { inner.answer(turn, after, done).await }).abort_handle());
    Ok(())
  }

  async fn dispatched(&self, payload: VoiceDispatched) -> Result<(), WireError> {
    tracing::debug!(intent = %payload.intent, target = ?payload.target, "voice turn landed");
    Ok(())
  }

  async fn dispatch_failed(&self, payload: VoiceDispatchFailed) -> Result<(), WireError> {
    tracing::warn!(intent = %payload.intent, code = ?payload.code, msg = %payload.msg, "voice turn refused");
    Ok(())
  }
}

struct Capture {
  format: VoiceFormat,
  reason: VoiceCaptureReason,
  packets: BTreeMap<u32, Bytes>,
}

struct Turn {
  stream_id: Uuid,
  format: VoiceFormat,
  reason: VoiceCaptureReason,
  packets: Vec<Bytes>,
}

struct Answer {
  resolved: NluResolvedIntent,
  stage: NluStage,
  asr_confidence: Option<f32>,
}

struct Heard {
  transcript: String,
  confidence: Option<f32>,
}

impl Heard {
  fn nothing() -> Self {
    Self {
      transcript: String::new(),
      confidence: None,
    }
  }
}

struct Inner {
  recognizer: Option<Arc<dyn SpeechRecognizer>>,
  controller: Arc<VoiceController>,
  link: Arc<dyn OutboundLink>,
  resolver: Option<Arc<dyn VoiceCatalogResolver>>,
  observer: Arc<dyn VoiceTurnObserver>,
  device_id: String,
  captures: Mutex<HashMap<Uuid, Capture>>,
  running: Mutex<Vec<AbortHandle>>,
  tail: Mutex<Option<oneshot::Receiver<()>>>,
  prewarmed: AtomicBool,
}

impl Inner {
  fn track(&self, task: AbortHandle) {
    let mut running = self.running.lock().unwrap();
    running.retain(|held| !held.is_finished());
    running.push(task);
  }

  fn sequence(&self) -> (oneshot::Sender<()>, Option<oneshot::Receiver<()>>) {
    let (done, wait) = oneshot::channel();
    let previous = self.tail.lock().unwrap().replace(wait);
    (done, previous)
  }

  fn notify(&self, stream_id: Uuid, reason: VoiceCaptureReason, phase: VoiceTurnPhase<'_>) {
    self.observer.turn_changed(
      &self.device_id,
      VoiceTurnUpdate {
        stream_id,
        reason,
        phase,
      },
    );
  }

  fn close(&self, msg: VoiceStreamClose) -> Option<Turn> {
    let capture = self.captures.lock().unwrap().remove(&msg.stream_id)?;
    if msg.reason != VoiceCloseReason::EndOfSpeech {
      self.notify(msg.stream_id, capture.reason, VoiceTurnPhase::Cancelled);
      return None;
    }
    Some(Turn {
      stream_id: msg.stream_id,
      format: capture.format,
      reason: capture.reason,
      packets: capture.packets.into_values().collect(),
    })
  }

  async fn answer(&self, turn: Turn, after: Option<oneshot::Receiver<()>>, done: oneshot::Sender<()>) {
    let Answer {
      resolved,
      stage,
      asr_confidence,
    } = self.resolve(&turn).await;
    if let Some(after) = after {
      let _ = after.await;
    }

    self.notify(turn.stream_id, turn.reason, VoiceTurnPhase::Resolved(&resolved));

    if let Err(error) = self
      .link
      .command(GatewayToBridgeVoiceMsgCommand::Dispatch(VoiceDispatch {
        resolved: Box::new(resolved),
        stage: Some(stage),
        asr_confidence,
      }))
      .await
    {
      tracing::warn!(%error, stream = %turn.stream_id, "dispatching the turn failed");
    }
    let _ = done.send(());
  }

  async fn resolve(&self, turn: &Turn) -> Answer {
    let Heard { transcript, confidence } = self.transcribe(turn).await;
    let resolution = match self.controller.resolve(&transcript).await {
      Ok(resolution) => resolution,
      Err(error) => {
        tracing::warn!(%error, "nlu failed");
        no_intent(&transcript, NluStage::NoModel)
      }
    };

    let Resolution {
      mut resolved,
      mut stage,
    } = resolution;
    if let Some(resolver) = self.resolver.as_ref() {
      match resolver.decorate(resolved.clone()).await {
        Ok(decorated) => resolved = decorated,
        Err(error) => tracing::warn!(%error, "catalog resolution failed"),
      }
    }
    if unresolved_catalog_command(&resolved) {
      tracing::info!(intent = %resolved.intent, transcript = %resolved.transcript, "nothing in the catalog matched");
      let refusal = no_intent(&resolved.transcript, NluStage::RejectedNoIntent);
      resolved = refusal.resolved;
      stage = refusal.stage;
    }
    Answer {
      resolved,
      stage,
      asr_confidence: confidence,
    }
  }

  async fn transcribe(&self, turn: &Turn) -> Heard {
    if turn.packets.is_empty() {
      return Heard::nothing();
    }
    let Some(recognizer) = self.recognizer.clone() else {
      return Heard::nothing();
    };

    let packets = turn.packets.clone();
    let format = turn.format;
    let pcm = match tokio::task::spawn_blocking(move || opus::decode(&packets, format)).await {
      Ok(Ok(pcm)) => pcm,
      Ok(Err(error)) => {
        tracing::warn!(%error, stream = %turn.stream_id, "capture failed");
        return Heard::nothing();
      }
      Err(error) => {
        tracing::warn!(%error, stream = %turn.stream_id, "capture failed");
        return Heard::nothing();
      }
    };
    if pcm.is_empty() {
      return Heard::nothing();
    }

    let (sink, spoken) = TranscriptionSink::channel();
    let sample_rate_hz = format.sample_rate_hz;
    if let Err(error) = tokio::task::spawn_blocking(move || recognizer.transcribe(pcm, sample_rate_hz, sink)).await {
      tracing::warn!(%error, stream = %turn.stream_id, "the recognizer did not answer");
      return Heard::nothing();
    }
    match spoken.await {
      Ok(Ok(transcription)) => Heard {
        transcript: transcription.text,
        confidence: transcription.confidence,
      },
      Ok(Err(reason)) => {
        tracing::warn!(%reason, stream = %turn.stream_id, "transcription failed");
        Heard::nothing()
      }
      Err(error) => {
        tracing::warn!(%error, stream = %turn.stream_id, "the recognizer dropped the turn");
        Heard::nothing()
      }
    }
  }
}
