#[path = "voicekit/samples.rs"]
mod samples;
mod voicekit;

use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
  time::Duration,
};

use bridgething_companion::{
  backend::{PrepareSink, SpeechRecognizer, Transcription, TranscriptionSink},
  voice::{
    controller::{ControllerError, VoiceController, VoiceControllerConfig},
    dispatcher::{
      CatalogError, VoiceCatalogResolver, VoiceDispatcher, VoiceDispatcherDeps, VoiceTurnObserver, VoiceTurnPhase,
      VoiceTurnUpdate,
    },
    inference::{InferError, InferenceOutput, NluInference},
    intent_catalog, opus,
  },
};
use bridgething_gateway::{Gateway, OutboundLink, SdkError, VoiceHandler};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use libbridgething::{
  NluPhoneAction, NluPopularityFilter, NluResolvedIntent, NluSlots, NluStage, NluTargetType, Priority,
  VoiceCaptureReason,
  gateway::{
    BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewayVoiceMsg, GatewayToBridgeMsgData,
    GatewayToBridgeVoiceMsg, VoiceCloseReason, VoiceDispatch, VoiceFrame, VoiceStreamClose, VoiceStreamOpen,
  },
  protocol::{BridgeEndec, DecodedFrame},
  wire::MsgMeta,
};
use tokio::sync::mpsc;
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(5);
const CONTENT_UTTERANCE: &str = "play the new mitski album";
const SLOW_UTTERANCE: &str = "play some jazz";

// MARK: doubles

struct FakeInference {
  logits: Vec<(&'static str, f64)>,
  in_domain_logit: f64,
  slots: NluSlots,
  fail_with_call: bool,
  warmed: AtomicUsize,
}

impl FakeInference {
  fn new(logits: &[(&'static str, f64)]) -> Arc<Self> {
    Arc::new(Self {
      logits: logits.to_vec(),
      in_domain_logit: 8.0,
      slots: NluSlots::default(),
      fail_with_call: false,
      warmed: AtomicUsize::new(0),
    })
  }

  fn refusing() -> Arc<Self> {
    Arc::new(Self {
      logits: Vec::new(),
      in_domain_logit: 8.0,
      slots: NluSlots::default(),
      fail_with_call: true,
      warmed: AtomicUsize::new(0),
    })
  }

  fn with_slots(logits: &[(&'static str, f64)], slots: NluSlots) -> Arc<Self> {
    Arc::new(Self {
      logits: logits.to_vec(),
      in_domain_logit: 8.0,
      slots,
      fail_with_call: false,
      warmed: AtomicUsize::new(0),
    })
  }

  fn out_of_domain(logits: &[(&'static str, f64)]) -> Arc<Self> {
    Arc::new(Self {
      logits: logits.to_vec(),
      in_domain_logit: -6.0,
      slots: NluSlots::default(),
      fail_with_call: false,
      warmed: AtomicUsize::new(0),
    })
  }
}

#[async_trait::async_trait]
impl NluInference for FakeInference {
  async fn prewarm(&self) {
    self.warmed.fetch_add(1, Ordering::SeqCst);
  }

  async fn infer(&self, _transcript: &str) -> Result<InferenceOutput, InferError> {
    if self.fail_with_call {
      return Err(InferError::Runtime(
        "the model ran on an utterance the caller should have claimed".into(),
      ));
    }
    let mut intent_logits = vec![0.0; intent_catalog::SURFACE_NAMES.len()];
    for (name, logit) in &self.logits {
      if let Some(index) = intent_catalog::SURFACE_NAMES.iter().position(|n| n == name) {
        intent_logits[index] = *logit;
      }
    }
    Ok(InferenceOutput {
      intent_logits,
      in_domain_logit: self.in_domain_logit,
      slots: self.slots.clone(),
    })
  }
}

struct FakeRecognizer {
  transcript: String,
  fail: bool,
  prepared: AtomicUsize,
  heard: Mutex<Vec<Vec<f32>>>,
}

impl FakeRecognizer {
  fn saying(transcript: &str) -> Arc<Self> {
    Arc::new(Self {
      transcript: transcript.to_owned(),
      fail: false,
      prepared: AtomicUsize::new(0),
      heard: Mutex::new(Vec::new()),
    })
  }

  fn failing() -> Arc<Self> {
    Arc::new(Self {
      transcript: String::new(),
      fail: true,
      prepared: AtomicUsize::new(0),
      heard: Mutex::new(Vec::new()),
    })
  }

  fn heard(&self) -> Vec<Vec<f32>> {
    self.heard.lock().unwrap().clone()
  }
}

impl SpeechRecognizer for FakeRecognizer {
  fn prepare(&self, sink: Arc<PrepareSink>) {
    self.prepared.fetch_add(1, Ordering::SeqCst);
    sink.on_ready();
  }

  fn transcribe(&self, pcm: Vec<f32>, _sample_rate_hz: u32, sink: Arc<TranscriptionSink>) {
    self.heard.lock().unwrap().push(pcm);
    if self.fail {
      sink.fail("analyzer died".into());
      return;
    }
    sink.complete(Transcription {
      text: self.transcript.clone(),
      alternatives: Vec::new(),
      segments: Vec::new(),
      confidence: None,
    });
  }
}

struct GatedRecognizer {
  entered: std::sync::mpsc::Sender<()>,
  release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl SpeechRecognizer for GatedRecognizer {
  fn prepare(&self, sink: Arc<PrepareSink>) {
    sink.on_ready();
  }

  fn transcribe(&self, _pcm: Vec<f32>, _sample_rate_hz: u32, sink: Arc<TranscriptionSink>) {
    let _ = self.entered.send(());
    let _ = self.release.lock().unwrap().recv();
    sink.complete(Transcription {
      text: "pause".into(),
      alternatives: Vec::new(),
      segments: Vec::new(),
      confidence: None,
    });
  }
}

struct StaggeredRecognizer {
  entered: std::sync::mpsc::Sender<()>,
  release: Mutex<std::sync::mpsc::Receiver<()>>,
  calls: AtomicUsize,
}

impl SpeechRecognizer for StaggeredRecognizer {
  fn prepare(&self, sink: Arc<PrepareSink>) {
    sink.on_ready();
  }

  fn transcribe(&self, _pcm: Vec<f32>, _sample_rate_hz: u32, sink: Arc<TranscriptionSink>) {
    let text = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
      let _ = self.entered.send(());
      let _ = self.release.lock().unwrap().recv();
      SLOW_UTTERANCE
    } else {
      "pause"
    };
    sink.complete(Transcription {
      text: text.into(),
      alternatives: Vec::new(),
      segments: Vec::new(),
      confidence: None,
    });
  }
}

type CatalogEntry = (&'static str, &'static str, Option<&'static str>);

const STOCK: &[CatalogEntry] = &[
  ("the strokes", "spotify:album:strokes", None),
  ("hounds of love", "spotify:track:7", Some("spotify:album:2")),
];

struct FakeCatalogResolver {
  stock: &'static [CatalogEntry],
  offline: bool,
  searched: Mutex<Vec<String>>,
}

impl FakeCatalogResolver {
  fn stocked() -> Arc<Self> {
    Arc::new(Self {
      stock: STOCK,
      offline: false,
      searched: Mutex::new(Vec::new()),
    })
  }

  fn empty() -> Arc<Self> {
    Arc::new(Self {
      stock: &[],
      offline: false,
      searched: Mutex::new(Vec::new()),
    })
  }

  fn offline() -> Arc<Self> {
    Arc::new(Self {
      stock: &[],
      offline: true,
      searched: Mutex::new(Vec::new()),
    })
  }

  fn searched(&self) -> Vec<String> {
    self.searched.lock().unwrap().clone()
  }
}

#[async_trait::async_trait]
impl VoiceCatalogResolver for FakeCatalogResolver {
  async fn decorate(&self, mut resolved: NluResolvedIntent) -> Result<NluResolvedIntent, CatalogError> {
    if self.offline {
      return Err(CatalogError::Failed("offline".into()));
    }
    let Some(target) = resolved.slots.target.clone() else {
      return Ok(resolved);
    };
    self.searched.lock().unwrap().push(target.clone());
    if let Some((_, uri, context_uri)) = self
      .stock
      .iter()
      .find(|(name, _, _)| name.eq_ignore_ascii_case(target.trim()))
    {
      resolved.slots.uri = Some((*uri).to_owned());
      resolved.slots.context_uri = context_uri.map(str::to_owned);
    }
    Ok(resolved)
  }
}

struct RecordingSink {
  tx: mpsc::UnboundedSender<VoiceDispatch>,
}

impl RecordingSink {
  fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<VoiceDispatch>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(Self { tx }), rx)
  }
}

#[async_trait::async_trait]
impl OutboundLink for RecordingSink {
  async fn send_data(&self, _meta: MsgMeta, data: GatewayToBridgeMsgData, _priority: Priority) -> Result<(), SdkError> {
    if let GatewayToBridgeMsgData::Voice(GatewayToBridgeVoiceMsg::Dispatch(payload)) = data {
      let _ = self.tx.send(payload);
    }
    Ok(())
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeenTurn {
  device_id: String,
  trigger: VoiceCaptureReason,
  phase: &'static str,
  transcript: Option<String>,
  intent: Option<String>,
}

#[derive(Default)]
struct RecordingTurns {
  seen: Mutex<Vec<SeenTurn>>,
}

impl RecordingTurns {
  fn seen(&self) -> Vec<SeenTurn> {
    self.seen.lock().unwrap().clone()
  }
}

impl VoiceTurnObserver for RecordingTurns {
  fn turn_changed(&self, device_id: &str, update: VoiceTurnUpdate<'_>) {
    let (phase, transcript, intent) = match update.phase {
      VoiceTurnPhase::Listening => ("listening", None, None),
      VoiceTurnPhase::Cancelled => ("cancelled", None, None),
      VoiceTurnPhase::Resolved(resolved) => (
        "resolved",
        Some(resolved.transcript.clone()),
        Some(resolved.intent.clone()),
      ),
    };
    self.seen.lock().unwrap().push(SeenTurn {
      device_id: device_id.to_owned(),
      trigger: update.reason,
      phase,
      transcript,
      intent,
    });
  }
}

// MARK: rig

const RIG_DEVICE: &str = "aa:bb:cc:dd:ee:ff";

struct Rig {
  dispatcher: VoiceDispatcher,
  dispatched: mpsc::UnboundedReceiver<VoiceDispatch>,
  turns: Arc<RecordingTurns>,
  trigger: VoiceCaptureReason,
}

impl Rig {
  fn build(
    recognizer: Option<Arc<dyn SpeechRecognizer>>,
    inference: Option<Arc<dyn NluInference>>,
    use_fast_path: bool,
  ) -> Self {
    Rig::with_catalog(recognizer, inference, use_fast_path, None)
  }

  fn with_catalog(
    recognizer: Option<Arc<dyn SpeechRecognizer>>,
    inference: Option<Arc<dyn NluInference>>,
    use_fast_path: bool,
    resolver: Option<Arc<dyn VoiceCatalogResolver>>,
  ) -> Self {
    let (link, dispatched) = RecordingSink::channel();
    let controller = Arc::new(VoiceController::new(
      inference,
      VoiceControllerConfig {
        use_fast_path,
        ..VoiceControllerConfig::default()
      },
    ));
    let turns = Arc::new(RecordingTurns::default());
    let dispatcher = VoiceDispatcher::new(VoiceDispatcherDeps {
      recognizer,
      controller,
      link,
      resolver,
      observer: turns.clone(),
      device_id: RIG_DEVICE.to_owned(),
    });
    Rig {
      dispatcher,
      dispatched,
      turns,
      trigger: VoiceCaptureReason::PushToTalk,
    }
  }

  fn triggered_by(mut self, trigger: VoiceCaptureReason) -> Self {
    self.trigger = trigger;
    self
  }

  async fn turn(&self, stream_id: Uuid, packets: &[(u32, Bytes)], reason: VoiceCloseReason) {
    self
      .dispatcher
      .stream_open(VoiceStreamOpen {
        stream_id,
        format: voicekit::format(),
        reason: self.trigger,
      })
      .await
      .unwrap();
    for (seq, packet) in packets {
      self
        .dispatcher
        .frame(VoiceFrame {
          stream_id,
          seq: *seq,
          packet: packet.clone(),
        })
        .await
        .unwrap();
    }
    self
      .dispatcher
      .stream_close(VoiceStreamClose { stream_id, reason })
      .await
      .unwrap();
  }

  async fn next_dispatch(&mut self) -> VoiceDispatch {
    tokio::time::timeout(DEADLINE, self.dispatched.recv())
      .await
      .expect("the turn was answered")
      .expect("the sink is live")
  }

  async fn no_dispatch(&mut self) {
    assert!(
      tokio::time::timeout(Duration::from_millis(400), self.dispatched.recv())
        .await
        .is_err(),
      "a turn that was never asked for was answered anyway"
    );
  }
}

#[tokio::test]
async fn a_wake_word_turn_is_reported_to_the_host_as_listening_then_resolved() {
  let mut rig =
    Rig::build(Some(FakeRecognizer::saying("next song")), None, true).triggered_by(VoiceCaptureReason::WakeWord);
  let stream_id = Uuid::now_v7();

  rig
    .turn(
      stream_id,
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;
  rig.next_dispatch().await;

  assert_eq!(
    rig.turns.seen(),
    vec![
      SeenTurn {
        device_id: RIG_DEVICE.to_owned(),
        trigger: VoiceCaptureReason::WakeWord,
        phase: "listening",
        transcript: None,
        intent: None,
      },
      SeenTurn {
        device_id: RIG_DEVICE.to_owned(),
        trigger: VoiceCaptureReason::WakeWord,
        phase: "resolved",
        transcript: Some("next song".to_owned()),
        intent: Some("NEXT".to_owned()),
      },
    ]
  );
}

#[tokio::test]
async fn a_cancelled_turn_is_reported_terminal_rather_than_left_listening() {
  let mut rig =
    Rig::build(Some(FakeRecognizer::saying("next song")), None, true).triggered_by(VoiceCaptureReason::WakeWord);
  let stream_id = Uuid::now_v7();

  rig
    .turn(stream_id, &numbered(&voicekit::packets()), VoiceCloseReason::Cancelled)
    .await;
  rig.no_dispatch().await;

  let phases: Vec<&str> = rig.turns.seen().into_iter().map(|turn| turn.phase).collect();
  assert_eq!(phases, vec!["listening", "cancelled"]);
}

#[tokio::test]
async fn stopping_the_dispatcher_mid_turn_is_reported_terminal_rather_than_left_listening() {
  let mut rig =
    Rig::build(Some(FakeRecognizer::saying("next song")), None, true).triggered_by(VoiceCaptureReason::WakeWord);
  let stream_id = Uuid::now_v7();

  rig
    .dispatcher
    .stream_open(VoiceStreamOpen {
      stream_id,
      format: voicekit::format(),
      reason: rig.trigger,
    })
    .await
    .unwrap();
  rig.dispatcher.stop();
  rig.no_dispatch().await;

  let phases: Vec<&str> = rig.turns.seen().into_iter().map(|turn| turn.phase).collect();
  assert_eq!(phases, vec!["listening", "cancelled"]);
}

#[tokio::test]
async fn reopening_a_stream_cancels_the_turn_it_replaces() {
  let rig =
    Rig::build(Some(FakeRecognizer::saying("next song")), None, true).triggered_by(VoiceCaptureReason::WakeWord);
  let stream_id = Uuid::now_v7();

  for _ in 0..2 {
    rig
      .dispatcher
      .stream_open(VoiceStreamOpen {
        stream_id,
        format: voicekit::format(),
        reason: rig.trigger,
      })
      .await
      .unwrap();
  }

  let phases: Vec<&str> = rig.turns.seen().into_iter().map(|turn| turn.phase).collect();
  assert_eq!(phases, vec!["listening", "cancelled", "listening"]);
}

#[tokio::test]
async fn a_push_to_talk_turn_reports_its_own_trigger() {
  let mut rig = Rig::build(Some(FakeRecognizer::saying("pause")), None, true);
  let stream_id = Uuid::now_v7();

  rig
    .turn(
      stream_id,
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;
  rig.next_dispatch().await;

  assert!(
    rig
      .turns
      .seen()
      .iter()
      .all(|turn| turn.trigger == VoiceCaptureReason::PushToTalk),
    "a push-to-talk turn must never read as a wake word one"
  );
}

fn numbered(packets: &[Bytes]) -> Vec<(u32, Bytes)> {
  packets
    .iter()
    .enumerate()
    .map(|(seq, packet)| (seq as u32, packet.clone()))
    .collect()
}

// MARK: controller

fn controller(inference: Option<Arc<dyn NluInference>>) -> VoiceController {
  VoiceController::new(inference, VoiceControllerConfig::default())
}

#[tokio::test]
async fn the_fast_path_short_circuits_before_the_model_runs() {
  let controller = controller(Some(FakeInference::refusing()));
  let resolution = controller.resolve("Pause.").await.unwrap();
  assert_eq!(resolution.stage, NluStage::FastPath);
  assert_eq!(resolution.resolved.intent, "PAUSE");
}

#[tokio::test]
async fn an_empty_transcript_is_no_intent_without_touching_the_model() {
  let controller = controller(Some(FakeInference::refusing()));
  let resolution = controller.resolve("   ").await.unwrap();
  assert_eq!(resolution.stage, NluStage::RejectedNoIntent);
  assert_eq!(resolution.resolved.intent, intent_catalog::NO_INTENT);
}

#[tokio::test]
async fn with_no_model_configured_the_fast_path_still_resolves() {
  let resolution = controller(None).resolve("turn it up").await.unwrap();
  assert_eq!(resolution.stage, NluStage::FastPath);
  assert_eq!(resolution.resolved.intent, "SET_VOLUME");
}

#[tokio::test]
async fn with_no_model_configured_a_fast_path_miss_says_so_rather_than_guessing() {
  let resolution = controller(None).resolve(CONTENT_UTTERANCE).await.unwrap();
  assert_eq!(resolution.stage, NluStage::NoModel);
  assert_eq!(resolution.resolved.intent, intent_catalog::NO_INTENT);
  assert_eq!(resolution.resolved.transcript, CONTENT_UTTERANCE);
}

#[tokio::test]
async fn an_accepted_intent_carries_the_decoded_slots_through_to_the_wire() {
  let inference = FakeInference::with_slots(
    &[("PLAY", 9.0)],
    NluSlots {
      target: Some("you stupid bitch by girl in red".into()),
      target_type: Some(NluTargetType::Track),
      ..NluSlots::default()
    },
  );
  let resolution = controller(Some(inference))
    .resolve("play you stupid bitch by girl in red")
    .await
    .unwrap();
  assert_eq!(resolution.stage, NluStage::Model);
  assert_eq!(resolution.resolved.intent, "PLAY");
  assert_eq!(
    resolution.resolved.slots.target.as_deref(),
    Some("you stupid bitch by girl in red")
  );
  assert_eq!(resolution.resolved.slots.target_type, Some(NluTargetType::Track));
}

#[tokio::test]
async fn prewarm_reaches_a_client_that_can_be_warmed() {
  let inference = FakeInference::new(&[]);
  controller(Some(inference.clone())).prewarm().await;
  assert_eq!(inference.warmed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn prewarm_is_a_no_op_when_no_model_is_configured() {
  controller(None).prewarm().await;
}

#[tokio::test]
async fn an_out_of_domain_utterance_resolves_to_no_intent() {
  let resolution = controller(Some(FakeInference::out_of_domain(&[("SEARCH", 5.0)])))
    .resolve("what is the capital of peru")
    .await
    .unwrap();
  assert_eq!(resolution.stage, NluStage::RejectedNoIntent);
  assert_eq!(resolution.resolved.intent, intent_catalog::NO_INTENT);
}

#[tokio::test]
async fn an_ambiguous_utterance_resolves_to_clarify_with_alternates_and_no_slots() {
  let inference = FakeInference::with_slots(
    &[("PLAY", 4.0), ("SEARCH", 3.95)],
    NluSlots {
      target: Some("pink".into()),
      ..NluSlots::default()
    },
  );
  let resolution = controller(Some(inference)).resolve("pink").await.unwrap();
  assert_eq!(resolution.stage, NluStage::RejectedClarify);
  assert_eq!(resolution.resolved.intent, intent_catalog::CLARIFY);
  let alternates = resolution.resolved.alternates.expect("clarify carries alternates");
  let mut named: Vec<&str> = alternates.iter().map(|alt| alt.intent.as_str()).collect();
  named.sort_unstable();
  assert_eq!(named, ["PLAY", "SEARCH"]);
  assert!(alternates.iter().all(|alt| alt.slots.is_none()));
}

#[tokio::test]
async fn the_transcript_rides_along_on_every_outcome() {
  let resolution = controller(Some(FakeInference::new(&[("SEARCH", 9.0)])))
    .resolve("search for 90s shoegaze")
    .await
    .unwrap();
  assert_eq!(resolution.resolved.transcript, "search for 90s shoegaze");
}

#[tokio::test]
async fn an_inference_failure_surfaces_as_a_controller_error() {
  let error = controller(Some(FakeInference::refusing()))
    .resolve("play some jazz by miles davis")
    .await
    .expect_err("a broken model is not a silent no-intent");
  assert!(matches!(error, ControllerError::InferenceFailed(_)));
}

#[tokio::test]
async fn disabling_the_fast_path_routes_bare_transport_through_the_model() {
  let controller = VoiceController::new(
    Some(FakeInference::new(&[("PAUSE", 9.0)])),
    VoiceControllerConfig {
      use_fast_path: false,
      ..VoiceControllerConfig::default()
    },
  );
  let resolution = controller.resolve("pause").await.unwrap();
  assert_eq!(resolution.stage, NluStage::Model);
  assert_eq!(resolution.resolved.intent, "PAUSE");
}

#[tokio::test]
async fn a_model_bare_play_with_nothing_said_about_playback_is_refused_rather_than_resumed() {
  let resolution = controller(Some(FakeInference::new(&[("PLAY", 9.0)])))
    .resolve("Are you aware of synthesis?")
    .await
    .unwrap();
  assert_eq!(resolution.stage, NluStage::RejectedNoIntent);
  assert_eq!(resolution.resolved.intent, intent_catalog::NO_INTENT);
}

#[tokio::test]
async fn a_model_bare_play_with_a_playback_cue_still_resumes() {
  let resolution = controller(Some(FakeInference::new(&[("PLAY", 9.0)])))
    .resolve("put something good on")
    .await
    .unwrap();
  assert_eq!(resolution.stage, NluStage::Model);
  assert_eq!(resolution.resolved.intent, "PLAY");
}

#[tokio::test]
async fn a_model_phone_action_with_nothing_said_about_a_call_is_refused() {
  let inference = FakeInference::with_slots(
    &[("PHONE_ACTION", 9.0)],
    NluSlots {
      phone_action: Some(NluPhoneAction::Answer),
      ..NluSlots::default()
    },
  );
  let resolution = controller(Some(inference)).resolve("turn left here").await.unwrap();
  assert_eq!(resolution.stage, NluStage::RejectedNoIntent);
  assert_eq!(resolution.resolved.intent, intent_catalog::NO_INTENT);
}

#[tokio::test]
async fn a_model_phone_action_naming_the_call_is_accepted() {
  let inference = FakeInference::with_slots(
    &[("PHONE_ACTION", 9.0)],
    NluSlots {
      phone_action: Some(NluPhoneAction::Answer),
      ..NluSlots::default()
    },
  );
  let resolution = controller(Some(inference)).resolve("answer the call").await.unwrap();
  assert_eq!(resolution.stage, NluStage::Model);
  assert_eq!(resolution.resolved.intent, "PHONE_ACTION");
}

// MARK: dispatcher

#[tokio::test]
async fn a_fast_path_turn_resolves_and_dispatches_on_the_wire() {
  let mut rig = Rig::build(Some(FakeRecognizer::saying("pause")), None, true);
  rig
    .turn(
      Uuid::now_v7(),
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;

  let dispatch = rig.next_dispatch().await;
  assert_eq!(dispatch.resolved.intent, "PAUSE");
  assert_eq!(dispatch.stage, Some(NluStage::FastPath));
}

#[tokio::test]
async fn every_dispatch_names_the_turn_it_answers() {
  let mut rig = Rig::build(Some(FakeRecognizer::saying("pause")), None, true);
  let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
  for stream_id in [first, second] {
    rig
      .turn(
        stream_id,
        &numbered(&voicekit::packets()),
        VoiceCloseReason::EndOfSpeech,
      )
      .await;
  }

  assert_eq!(
    [rig.next_dispatch().await.stream_id, rig.next_dispatch().await.stream_id],
    [first, second],
    "the daemon can only settle a turn once if each answer says which turn it is"
  );
}

#[tokio::test]
async fn a_fast_path_miss_resolves_through_the_injected_model() {
  let mut rig = Rig::with_catalog(
    Some(FakeRecognizer::saying(CONTENT_UTTERANCE)),
    Some(naming("hounds of love")),
    true,
    Some(FakeCatalogResolver::stocked()),
  );
  rig
    .turn(
      Uuid::now_v7(),
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;

  let dispatch = rig.next_dispatch().await;
  assert_eq!(dispatch.resolved.intent, "PLAY");
  assert_eq!(dispatch.stage, Some(NluStage::Model));
  assert_eq!(dispatch.resolved.slots.target.as_deref(), Some("hounds of love"));
}

fn asking_to_play(slots: NluSlots) -> Arc<FakeInference> {
  FakeInference::with_slots(&[("PLAY", 9.0)], slots)
}

fn naming(target: &str) -> Arc<FakeInference> {
  asking_to_play(NluSlots {
    target: Some(target.into()),
    target_type: Some(NluTargetType::Album),
    ..NluSlots::default()
  })
}

async fn catalog_turn(inference: Arc<FakeInference>, resolver: Option<Arc<dyn VoiceCatalogResolver>>) -> VoiceDispatch {
  let mut rig = Rig::with_catalog(
    Some(FakeRecognizer::saying(CONTENT_UTTERANCE)),
    Some(inference),
    true,
    resolver,
  );
  rig
    .turn(
      Uuid::now_v7(),
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;
  rig.next_dispatch().await
}

fn assert_refused(dispatch: &VoiceDispatch) {
  assert_eq!(
    dispatch.resolved.intent,
    intent_catalog::NO_INTENT,
    "an unresolvable catalog play must never reach the daemon as a play"
  );
  assert_eq!(dispatch.stage, Some(NluStage::RejectedNoIntent));
  assert_eq!(dispatch.resolved.slots.uri, None);
  assert_eq!(
    dispatch.resolved.transcript, CONTENT_UTTERANCE,
    "the refusal still tells the user what was heard"
  );
}

#[tokio::test]
async fn a_bare_kind_play_the_anchor_cannot_place_is_refused_not_resumed() {
  let dispatch = catalog_turn(
    asking_to_play(NluSlots {
      target_type: Some(NluTargetType::Album),
      ..NluSlots::default()
    }),
    Some(FakeCatalogResolver::empty()),
  )
  .await;
  assert_refused(&dispatch);
}

#[tokio::test]
async fn a_catalog_play_carries_the_uri_the_provider_search_found() {
  let resolver = FakeCatalogResolver::stocked();
  let dispatch = catalog_turn(naming("The Strokes"), Some(resolver.clone())).await;

  assert_eq!(dispatch.resolved.intent, "PLAY");
  assert_eq!(dispatch.resolved.slots.uri.as_deref(), Some("spotify:album:strokes"));
  assert_eq!(resolver.searched(), ["The Strokes"], "the provider was really asked");
}

#[tokio::test]
async fn a_resolved_track_carries_the_context_it_was_found_in() {
  let dispatch = catalog_turn(naming("hounds of love"), Some(FakeCatalogResolver::stocked())).await;

  assert_eq!(dispatch.resolved.slots.uri.as_deref(), Some("spotify:track:7"));
  assert_eq!(dispatch.resolved.slots.context_uri.as_deref(), Some("spotify:album:2"));
}

#[tokio::test]
async fn a_catalog_play_nothing_matched_is_refused_rather_than_dispatched() {
  let resolver = FakeCatalogResolver::empty();
  let dispatch = catalog_turn(naming("a record nobody pressed"), Some(resolver.clone())).await;

  assert_eq!(resolver.searched(), ["a record nobody pressed"]);
  assert_refused(&dispatch);
}

#[tokio::test]
async fn a_catalog_play_is_refused_when_the_provider_could_not_be_reached() {
  assert_refused(&catalog_turn(naming("The Strokes"), Some(FakeCatalogResolver::offline())).await);
}

#[tokio::test]
async fn a_catalog_play_is_refused_when_no_provider_can_resolve_it() {
  assert_refused(&catalog_turn(naming("The Strokes"), None).await);
}

#[tokio::test]
async fn a_play_naming_only_a_popularity_filter_is_refused_when_nothing_resolves() {
  let inference = asking_to_play(NluSlots {
    popularity_filter: Some(NluPopularityFilter::New),
    ..NluSlots::default()
  });
  assert_refused(&catalog_turn(inference, Some(FakeCatalogResolver::stocked())).await);
}

#[tokio::test]
async fn a_queue_add_that_never_resolved_is_refused() {
  let inference = FakeInference::with_slots(
    &[("ADD_TO_QUEUE", 9.0)],
    NluSlots {
      target: Some("a record nobody pressed".into()),
      ..NluSlots::default()
    },
  );
  assert_refused(&catalog_turn(inference, Some(FakeCatalogResolver::empty())).await);
}

#[tokio::test]
async fn a_bare_play_needs_no_catalog_resolution_to_reach_the_daemon() {
  let dispatch = catalog_turn(asking_to_play(NluSlots::default()), None).await;

  assert_eq!(
    dispatch.resolved.intent, "PLAY",
    "resuming what is already loaded never needs a uri"
  );
  assert_eq!(dispatch.stage, Some(NluStage::Model));
}

#[tokio::test]
async fn a_failing_recognizer_dispatches_a_no_intent_turn_rather_than_silence() {
  let recognizer = FakeRecognizer::failing();
  let mut rig = Rig::build(
    Some(recognizer.clone()),
    Some(FakeInference::new(&[("PLAY", 9.0)])),
    true,
  );
  rig
    .turn(
      Uuid::now_v7(),
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;

  let dispatch = rig.next_dispatch().await;
  assert_eq!(dispatch.resolved.intent, intent_catalog::NO_INTENT);
  assert_eq!(dispatch.stage, Some(NluStage::RejectedNoIntent));
  assert_eq!(recognizer.heard().len(), 1, "the recognizer really was asked");
}

#[tokio::test]
async fn a_failed_decode_still_answers_the_turn() {
  let recognizer = FakeRecognizer::saying("pause");
  let mut rig = Rig::build(Some(recognizer.clone()), None, true);
  let garbage = vec![(0u32, Bytes::from_static(&[0xff, 0x00]))];
  rig.turn(Uuid::now_v7(), &garbage, VoiceCloseReason::EndOfSpeech).await;

  let dispatch = rig.next_dispatch().await;
  assert_eq!(dispatch.resolved.intent, intent_catalog::NO_INTENT);
  assert_eq!(dispatch.stage, Some(NluStage::RejectedNoIntent));
  assert!(
    recognizer.heard().is_empty(),
    "a turn that would not decode has nothing to transcribe"
  );
}

#[tokio::test]
async fn a_capture_that_carried_no_packets_answers_no_intent() {
  let recognizer = FakeRecognizer::saying("pause");
  let mut rig = Rig::build(Some(recognizer.clone()), None, true);
  rig.turn(Uuid::now_v7(), &[], VoiceCloseReason::EndOfSpeech).await;

  let dispatch = rig.next_dispatch().await;
  assert_eq!(dispatch.resolved.intent, intent_catalog::NO_INTENT);
  assert_eq!(dispatch.stage, Some(NluStage::RejectedNoIntent));
  assert!(dispatch.resolved.transcript.is_empty());
  assert!(recognizer.heard().is_empty(), "there was nothing to transcribe");
}

#[tokio::test]
async fn a_failed_model_still_answers_the_turn() {
  let mut rig = Rig::build(
    Some(FakeRecognizer::saying(CONTENT_UTTERANCE)),
    Some(FakeInference::refusing()),
    true,
  );
  rig
    .turn(
      Uuid::now_v7(),
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;

  let dispatch = rig.next_dispatch().await;
  assert_eq!(dispatch.resolved.intent, intent_catalog::NO_INTENT);
  assert_eq!(dispatch.stage, Some(NluStage::NoModel));
  assert_eq!(
    dispatch.resolved.transcript, CONTENT_UTTERANCE,
    "the transcript survives a model that failed"
  );
}

#[tokio::test]
async fn a_cancelled_capture_drops_without_answering() {
  let mut rig = Rig::build(Some(FakeRecognizer::saying("pause")), None, true);
  rig
    .turn(
      Uuid::now_v7(),
      &numbered(&voicekit::packets()),
      VoiceCloseReason::Cancelled,
    )
    .await;
  rig.no_dispatch().await;
}

#[tokio::test]
async fn a_close_for_a_stream_that_was_never_opened_answers_nothing() {
  let mut rig = Rig::build(Some(FakeRecognizer::saying("pause")), None, true);
  rig
    .dispatcher
    .stream_close(VoiceStreamClose {
      stream_id: Uuid::now_v7(),
      reason: VoiceCloseReason::EndOfSpeech,
    })
    .await
    .unwrap();
  rig.no_dispatch().await;
}

#[tokio::test]
async fn packets_reach_the_recognizer_in_sequence_order_however_they_arrived() {
  let recognizer = FakeRecognizer::saying("pause");
  let mut rig = Rig::build(Some(recognizer.clone()), None, true);

  let packets = voicekit::packets();
  let mut arrival = numbered(&packets);
  arrival.reverse();
  rig.turn(Uuid::now_v7(), &arrival, VoiceCloseReason::EndOfSpeech).await;
  rig.next_dispatch().await;

  let ordered = opus::decode(&packets, voicekit::format()).expect("the capture decodes");
  let heard = recognizer.heard();
  assert_eq!(heard.len(), 1);
  assert_eq!(
    heard[0], ordered,
    "reassembly by seq has to reproduce the capture order exactly"
  );
  assert_eq!(heard[0].len(), packets.len() * samples::SAMPLES_PER_PACKET);
}

#[tokio::test]
async fn prewarm_fires_once_on_the_first_stream_open_not_once_per_turn() {
  let inference = FakeInference::new(&[]);
  let mut rig = Rig::build(Some(FakeRecognizer::saying("pause")), Some(inference.clone()), true);

  rig
    .turn(
      Uuid::now_v7(),
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;
  rig.next_dispatch().await;
  rig
    .turn(
      Uuid::now_v7(),
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;
  rig.next_dispatch().await;

  assert_eq!(
    inference.warmed.load(Ordering::SeqCst),
    1,
    "a warm model is not re-warmed by the next turn"
  );
}

#[tokio::test]
async fn stopping_drops_the_captures_in_flight() {
  let mut rig = Rig::build(Some(FakeRecognizer::saying("pause")), None, true);
  let stream_id = Uuid::now_v7();
  rig
    .dispatcher
    .stream_open(VoiceStreamOpen {
      stream_id,
      format: voicekit::format(),
      reason: VoiceCaptureReason::PushToTalk,
    })
    .await
    .unwrap();
  rig.dispatcher.stop();
  rig
    .dispatcher
    .stream_close(VoiceStreamClose {
      stream_id,
      reason: VoiceCloseReason::EndOfSpeech,
    })
    .await
    .unwrap();
  rig.no_dispatch().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_drops_the_turn_already_in_flight() {
  let (entered, arrived) = std::sync::mpsc::channel();
  let (release, held) = std::sync::mpsc::channel();
  let mut rig = Rig::build(
    Some(Arc::new(GatedRecognizer {
      entered,
      release: Mutex::new(held),
    })),
    None,
    true,
  );

  rig
    .turn(
      Uuid::now_v7(),
      &numbered(&voicekit::packets()),
      VoiceCloseReason::EndOfSpeech,
    )
    .await;
  arrived.recv_timeout(DEADLINE).expect("the turn reached the recognizer");
  rig.dispatcher.stop();
  release.send(()).expect("the recognizer is still waiting");

  rig.no_dispatch().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_spoken_second_waits_for_the_slow_turn_spoken_first() {
  let (entered, arrived) = std::sync::mpsc::channel();
  let (release, held) = std::sync::mpsc::channel();
  let mut rig = Rig::build(
    Some(Arc::new(StaggeredRecognizer {
      entered,
      release: Mutex::new(held),
      calls: AtomicUsize::new(0),
    })),
    None,
    true,
  );

  let packets = numbered(&voicekit::packets());
  rig.turn(Uuid::now_v7(), &packets, VoiceCloseReason::EndOfSpeech).await;
  arrived
    .recv_timeout(DEADLINE)
    .expect("the first turn reached the recognizer");
  rig.turn(Uuid::now_v7(), &packets, VoiceCloseReason::EndOfSpeech).await;

  assert!(
    tokio::time::timeout(Duration::from_millis(400), rig.dispatched.recv())
      .await
      .is_err(),
    "the second turn resolved first, but dispatching it first is what makes the device play something \
     right after the user asked it to stop"
  );

  release.send(()).expect("the recognizer is still waiting");
  let first = rig.next_dispatch().await;
  let second = rig.next_dispatch().await;
  assert_eq!(
    (first.resolved.transcript.as_str(), second.resolved.transcript.as_str()),
    (SLOW_UTTERANCE, "pause"),
    "a fast second turn that overtakes a slow first one plays what the user just asked to stop"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_turns_interleaved_on_the_wire_both_dispatch_with_every_packet() {
  let recognizer = FakeRecognizer::saying("pause");
  let mut rig = Rig::build(Some(recognizer.clone()), None, true);

  let packets = voicekit::packets();
  let first = Uuid::now_v7();
  let second = Uuid::now_v7();
  let format = voicekit::format();

  for stream_id in [first, second] {
    rig
      .dispatcher
      .stream_open(VoiceStreamOpen {
        stream_id,
        format: voicekit::format(),
        reason: VoiceCaptureReason::PushToTalk,
      })
      .await
      .unwrap();
  }
  for (seq, packet) in packets.iter().enumerate() {
    for stream_id in [first, second] {
      rig
        .dispatcher
        .frame(VoiceFrame {
          stream_id,
          seq: seq as u32,
          packet: packet.clone(),
        })
        .await
        .unwrap();
    }
  }
  for stream_id in [first, second] {
    rig
      .dispatcher
      .stream_close(VoiceStreamClose {
        stream_id,
        reason: VoiceCloseReason::EndOfSpeech,
      })
      .await
      .unwrap();
  }

  rig.next_dispatch().await;
  rig.next_dispatch().await;

  let ordered = opus::decode(&packets, format).expect("the capture decodes");
  let heard = recognizer.heard();
  assert_eq!(heard.len(), 2, "both turns have to reach the recognizer");
  assert!(
    heard.iter().all(|pcm| *pcm == ordered),
    "an interleaved turn lost packets: {:?}",
    heard.iter().map(Vec::len).collect::<Vec<usize>>()
  );
}

// MARK: the real link

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_arriving_over_the_link_is_answered_over_the_link() {
  let (companion_io, daemon_io) = tokio::io::duplex(256 * 1024);
  let (dispatched_tx, mut dispatched_rx) = mpsc::unbounded_channel();

  let stream_id = Uuid::now_v7();
  let packets = voicekit::packets();
  let mut said: Vec<BridgeToGatewayMsgData> = vec![BridgeToGatewayMsgData::Voice(BridgeToGatewayVoiceMsg::StreamOpen(
    VoiceStreamOpen {
      stream_id,
      format: voicekit::format(),
      reason: VoiceCaptureReason::WakeWord,
    },
  ))];
  for (seq, packet) in packets.iter().enumerate() {
    said.push(BridgeToGatewayMsgData::Voice(BridgeToGatewayVoiceMsg::Frame(
      VoiceFrame {
        stream_id,
        seq: seq as u32,
        packet: packet.clone(),
      },
    )));
  }
  said.push(BridgeToGatewayMsgData::Voice(BridgeToGatewayVoiceMsg::StreamClose(
    VoiceStreamClose {
      stream_id,
      reason: VoiceCloseReason::EndOfSpeech,
    },
  )));

  tokio::spawn(async move {
    let mut framed = tokio_util::codec::Framed::new(daemon_io, BridgeEndec::default());
    for data in said {
      framed
        .send(BridgeToGatewayMsg {
          id: Uuid::now_v7(),
          meta: MsgMeta::Event,
          data,
        })
        .await
        .expect("the burst reaches the companion");
    }
    while let Some(Ok(DecodedFrame::Frame(frame))) = framed.next().await {
      if let GatewayToBridgeMsgData::Voice(GatewayToBridgeVoiceMsg::Dispatch(dispatch)) = frame.msg.data {
        let _ = dispatched_tx.send(dispatch);
      }
    }
  });

  let (gateway, mut inbound) =
    Gateway::spawn_subscribed(bridgething_gateway::transport::FramedConnector::new(companion_io));
  let controller = Arc::new(VoiceController::new(None, VoiceControllerConfig::default()));
  let turns = Arc::new(RecordingTurns::default());
  let dispatcher = VoiceDispatcher::new(VoiceDispatcherDeps {
    recognizer: Some(FakeRecognizer::saying("pause")),
    controller,
    link: Arc::new(gateway.clone()),
    resolver: None,
    observer: turns.clone(),
    device_id: RIG_DEVICE.to_owned(),
  });

  tokio::spawn(async move {
    while let Ok(msg) = inbound.recv().await {
      if let BridgeToGatewayMsgData::Voice(surface) = msg.data {
        let _ = match surface {
          BridgeToGatewayVoiceMsg::StreamOpen(payload) => dispatcher.stream_open(payload).await,
          BridgeToGatewayVoiceMsg::Frame(payload) => dispatcher.frame(payload).await,
          BridgeToGatewayVoiceMsg::StreamClose(payload) => dispatcher.stream_close(payload).await,
          BridgeToGatewayVoiceMsg::Dispatched(payload) => dispatcher.dispatched(payload).await,
          BridgeToGatewayVoiceMsg::DispatchFailed(payload) => dispatcher.dispatch_failed(payload).await,
        };
      }
    }
  });

  let dispatch = tokio::time::timeout(DEADLINE, dispatched_rx.recv())
    .await
    .expect("the turn was answered over the link")
    .expect("the daemon side is live");
  assert_eq!(dispatch.resolved.intent, "PAUSE");
  assert_eq!(dispatch.stage, Some(NluStage::FastPath));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_capture_delivered_out_of_order_keeps_every_packet() {
  let recognizer = FakeRecognizer::saying("pause");
  let mut rig = Rig::build(Some(recognizer.clone()), None, true);

  let packets: Vec<Bytes> = std::iter::repeat_n(voicekit::packets(), 8).flatten().collect();
  let mut arrival: Vec<(u32, Bytes)> = numbered(&packets);
  let rotation = arrival.len() / 3;
  arrival.rotate_left(rotation);
  rig.turn(Uuid::now_v7(), &arrival, VoiceCloseReason::EndOfSpeech).await;
  rig.next_dispatch().await;

  let heard = recognizer.heard();
  assert_eq!(heard.len(), 1);
  assert_eq!(
    heard[0].len(),
    packets.len() * samples::SAMPLES_PER_PACKET,
    "the turn lost packets"
  );
}

#[tokio::test]
async fn a_repeated_sequence_number_replaces_rather_than_lengthening_the_turn() {
  let recognizer = FakeRecognizer::saying("pause");
  let mut rig = Rig::build(Some(recognizer.clone()), None, true);

  let packets = voicekit::packets();
  let mut arrival = numbered(&packets);
  arrival.push((0, packets[0].clone()));
  rig.turn(Uuid::now_v7(), &arrival, VoiceCloseReason::EndOfSpeech).await;
  rig.next_dispatch().await;

  assert_eq!(recognizer.heard()[0].len(), packets.len() * samples::SAMPLES_PER_PACKET);
}

#[tokio::test]
async fn sequence_numbers_past_ten_sort_numerically() {
  let recognizer = FakeRecognizer::saying("pause");
  let mut rig = Rig::build(Some(recognizer.clone()), None, true);

  let packets = voicekit::packets();
  let mut arrival = numbered(&packets);
  arrival.swap(1, 11);
  let (first, second) = (arrival[1].0, arrival[11].0);
  assert_eq!((first, second), (11, 1), "the fixture no longer exercises the sort");
  rig.turn(Uuid::now_v7(), &arrival, VoiceCloseReason::EndOfSpeech).await;
  rig.next_dispatch().await;

  let ordered = opus::decode(&packets, voicekit::format()).expect("the capture decodes");
  assert_eq!(recognizer.heard()[0], ordered);
}
