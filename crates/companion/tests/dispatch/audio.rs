use std::{
  collections::HashMap,
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
};

use bridgething_companion::{
  backend::{AudioBackend, EarconSink, SpeakSink, VolumeBackend, VolumeInbox, VolumeLevel},
  dispatch::audio::{AudioDispatcher, VolumeAuthority},
};
use bridgething_gateway::AudioHandler;
use libbridgething::{
  AudioError,
  gateway::{
    AudioErrorReply, Earcon, GatewayToBridgeAudioMsg, GatewayToBridgeMsg, GatewayToBridgeMsgData, SetMute, SetVolume,
    Tts, TtsCancel, TtsEnded, TtsStarted, VolumeChanged,
  },
};
use uuid::Uuid;

use crate::support::Peer;

#[derive(Default)]
struct FakeAudio {
  hold_speech: bool,
  earcon_ok: bool,
  earcons: Mutex<Vec<String>>,
  spoken: Mutex<Vec<String>>,
  holding: Mutex<HashMap<String, Arc<SpeakSink>>>,
  cancel_all: AtomicUsize,
  abandon_speech: bool,
}

impl FakeAudio {
  fn new() -> Arc<Self> {
    Arc::new(Self {
      earcon_ok: true,
      ..Default::default()
    })
  }

  fn holding_speech() -> Arc<Self> {
    Arc::new(Self {
      hold_speech: true,
      earcon_ok: true,
      ..Default::default()
    })
  }

  fn refusing_earcons() -> Arc<Self> {
    Arc::new(Self::default())
  }

  fn abandoning_speech() -> Arc<Self> {
    Arc::new(Self {
      abandon_speech: true,
      earcon_ok: true,
      ..Default::default()
    })
  }
}

impl AudioBackend for FakeAudio {
  fn speak(&self, id: String, text: String, _voice: Option<String>, sink: Arc<SpeakSink>) {
    self.spoken.lock().unwrap().push(text);
    if self.abandon_speech {
      return;
    }
    sink.on_start();
    if self.hold_speech {
      self.holding.lock().unwrap().insert(id, sink);
    } else {
      sink.on_finished(true);
    }
  }

  fn cancel(&self, id: String) {
    if let Some(sink) = self.holding.lock().unwrap().remove(&id) {
      sink.on_finished(false);
    }
  }

  fn cancel_all(&self) {
    self.cancel_all.fetch_add(1, Ordering::SeqCst);
    for (_, sink) in self.holding.lock().unwrap().drain() {
      sink.on_finished(false);
    }
  }

  fn play_earcon(&self, name: String, sink: Arc<EarconSink>) {
    self.earcons.lock().unwrap().push(name);
    sink.on_finished(self.earcon_ok);
  }
}

#[derive(Default)]
struct FakeVolume {
  level: Mutex<VolumeLevel>,
  set_volume: Mutex<Vec<f32>>,
  set_mute: Mutex<Vec<bool>>,
  volume_up: AtomicUsize,
  volume_down: AtomicUsize,
  mute_toggle: AtomicUsize,
  inbox: Mutex<Option<Arc<VolumeInbox>>>,
}

impl VolumeBackend for FakeVolume {
  fn start(&self, inbox: Arc<VolumeInbox>) {
    *self.inbox.lock().unwrap() = Some(inbox);
  }

  fn stop(&self) {
    self.inbox.lock().unwrap().take();
  }

  fn snapshot(&self) -> VolumeLevel {
    *self.level.lock().unwrap()
  }

  fn set_volume(&self, level: f32) {
    self.set_volume.lock().unwrap().push(level);
  }

  fn set_mute(&self, muted: bool) {
    self.set_mute.lock().unwrap().push(muted);
  }

  fn volume_up(&self) {
    self.volume_up.fetch_add(1, Ordering::SeqCst);
  }

  fn volume_down(&self) {
    self.volume_down.fetch_add(1, Ordering::SeqCst);
  }

  fn mute_toggle(&self) {
    self.mute_toggle.fetch_add(1, Ordering::SeqCst);
  }
}

struct FakeAuthority {
  owns: bool,
  refuse: Option<String>,
  calls: Mutex<Vec<String>>,
}

impl FakeAuthority {
  fn owning() -> Arc<Self> {
    Arc::new(Self {
      owns: true,
      refuse: None,
      calls: Mutex::new(Vec::new()),
    })
  }

  fn refusing() -> Arc<Self> {
    Arc::new(Self {
      owns: true,
      refuse: Some("device is gone".into()),
      calls: Mutex::new(Vec::new()),
    })
  }

  fn not_owning() -> Arc<Self> {
    Arc::new(Self {
      owns: false,
      refuse: None,
      calls: Mutex::new(Vec::new()),
    })
  }

  fn answer(&self, verb: &str, level: f32) -> Result<f32, String> {
    self.calls.lock().unwrap().push(verb.into());
    match &self.refuse {
      Some(reason) => Err(reason.clone()),
      None => Ok(level),
    }
  }
}

#[async_trait::async_trait]
impl VolumeAuthority for FakeAuthority {
  async fn owns_volume(&self) -> bool {
    self.owns
  }

  async fn volume_up(&self) -> Result<f32, String> {
    self.answer("volumeUp", 0.75)
  }

  async fn volume_down(&self) -> Result<f32, String> {
    self.answer("volumeDown", 0.25)
  }

  async fn set_volume(&self, level: f32) -> Result<f32, String> {
    self.answer("setVolume", level)
  }
}

fn tts_started(msg: &GatewayToBridgeMsg) -> Option<TtsStarted> {
  match msg.data {
    GatewayToBridgeMsgData::Audio(GatewayToBridgeAudioMsg::TtsStarted(started)) => Some(started),
    _ => None,
  }
}

fn tts_ended(msg: &GatewayToBridgeMsg) -> Option<TtsEnded> {
  match msg.data {
    GatewayToBridgeMsgData::Audio(GatewayToBridgeAudioMsg::TtsEnded(ended)) => Some(ended),
    _ => None,
  }
}

fn volume_changed(msg: &GatewayToBridgeMsg) -> Option<VolumeChanged> {
  match msg.data {
    GatewayToBridgeMsgData::Audio(GatewayToBridgeAudioMsg::VolumeChanged(changed)) => Some(changed),
    _ => None,
  }
}

fn audio_error(msg: &GatewayToBridgeMsg) -> Option<AudioErrorReply> {
  match &msg.data {
    GatewayToBridgeMsgData::Audio(GatewayToBridgeAudioMsg::ErrorEvent(reply)) => Some(reply.clone()),
    _ => None,
  }
}

fn started_with(id: Uuid) -> impl Fn(&GatewayToBridgeMsg) -> Option<TtsStarted> {
  move |msg| tts_started(msg).filter(|started| started.id == id)
}

fn ended_with(id: Uuid) -> impl Fn(&GatewayToBridgeMsg) -> Option<TtsEnded> {
  move |msg| tts_ended(msg).filter(|ended| ended.id == id)
}

fn tts(id: Uuid, text: &str) -> Tts {
  Tts {
    id,
    text: text.into(),
    voice: None,
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn volume_verbs_route_to_the_host_mixer() {
  let volume = Arc::new(FakeVolume::default());
  let (gateway, _peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(None, Some(volume.clone()), Arc::new(gateway));

  dispatcher
    .set_volume(SetVolume { level: 0.42 })
    .await
    .expect("accepted");
  dispatcher.volume_up().await.expect("accepted");
  dispatcher.volume_down().await.expect("accepted");

  assert_eq!(*volume.set_volume.lock().unwrap(), vec![0.42]);
  assert_eq!(volume.volume_up.load(Ordering::SeqCst), 1);
  assert_eq!(volume.volume_down.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn volume_verbs_report_unavailable_on_a_host_with_no_mixer() {
  let backend = FakeAudio::new();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(Some(backend.clone()), None, Arc::new(gateway));

  dispatcher.volume_up().await.expect("accepted");

  let reply = peer.wait("an audio error", audio_error).await;
  assert_eq!(
    reply.error,
    AudioError::Unavailable {
      verb: "volumeUp".into()
    },
    "a host that cannot move its own volume says so instead of swallowing the verb"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn mute_verbs_route_to_the_host_mixer() {
  let volume = Arc::new(FakeVolume::default());
  let (gateway, _peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(None, Some(volume.clone()), Arc::new(gateway));

  dispatcher.set_mute(SetMute { muted: true }).await.expect("accepted");
  dispatcher.mute_toggle().await.expect("accepted");

  assert_eq!(*volume.set_mute.lock().unwrap(), vec![true]);
  assert_eq!(volume.mute_toggle.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn speech_emits_started_then_ended_completed() {
  let backend = FakeAudio::new();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(Some(backend.clone()), None, Arc::new(gateway));
  let id = Uuid::now_v7();

  dispatcher.tts(tts(id, "hello")).await.expect("accepted");

  peer.wait("a ttsStarted", started_with(id)).await;
  let ended = peer.wait("a ttsEnded", ended_with(id)).await;
  assert!(ended.completed, "uncancelled speech ends completed");
  assert_eq!(*backend.spoken.lock().unwrap(), vec!["hello".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_speech_ends_it_incomplete() {
  let backend = FakeAudio::holding_speech();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(Some(backend.clone()), None, Arc::new(gateway));
  let id = Uuid::now_v7();

  dispatcher.tts(tts(id, "a long sentence")).await.expect("accepted");
  peer.wait("a ttsStarted", started_with(id)).await;

  dispatcher.tts_cancel(TtsCancel { id }).await.expect("accepted");

  let ended = peer.wait("a ttsEnded", ended_with(id)).await;
  assert!(!ended.completed, "cancelled speech ends not-completed");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_all_ends_every_turn_in_flight() {
  let backend = FakeAudio::holding_speech();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(Some(backend.clone()), None, Arc::new(gateway));
  let id = Uuid::now_v7();

  dispatcher.tts(tts(id, "a long sentence")).await.expect("accepted");
  peer.wait("a ttsStarted", started_with(id)).await;

  dispatcher.tts_cancel_all().await.expect("accepted");

  let ended = peer.wait("a ttsEnded", ended_with(id)).await;
  assert!(!ended.completed);
  assert_eq!(backend.cancel_all.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_turn_waits_for_the_first_to_end() {
  let backend = FakeAudio::holding_speech();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(Some(backend.clone()), None, Arc::new(gateway));
  let first = Uuid::now_v7();
  let second = Uuid::now_v7();

  dispatcher.tts(tts(first, "one")).await.expect("accepted");
  peer.wait("a ttsStarted", started_with(first)).await;
  dispatcher.tts(tts(second, "two")).await.expect("accepted");

  peer.quiet("the second turn's ttsStarted", started_with(second)).await;
  assert_eq!(
    *backend.spoken.lock().unwrap(),
    vec!["one".to_string()],
    "the backend is not asked to speak twice at once"
  );

  dispatcher.tts_cancel(TtsCancel { id: first }).await.expect("accepted");
  peer.wait("the second turn's ttsStarted", started_with(second)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_the_backend_abandons_still_ends() {
  let backend = FakeAudio::abandoning_speech();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(Some(backend.clone()), None, Arc::new(gateway));
  let id = Uuid::now_v7();

  dispatcher.tts(tts(id, "hello")).await.expect("accepted");

  let ended = peer.wait("a ttsEnded", ended_with(id)).await;
  assert!(
    !ended.completed,
    "a dropped sink is a failed turn, never a turn that never ends"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn earcons_route_to_the_backend() {
  let backend = FakeAudio::new();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(Some(backend.clone()), None, Arc::new(gateway));

  dispatcher
    .earcon(Earcon { name: "confirm".into() })
    .await
    .expect("accepted");

  peer.quiet("an audio error", audio_error).await;
  assert_eq!(*backend.earcons.lock().unwrap(), vec!["confirm".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_earcon_the_platform_refuses_reports_unavailable() {
  let backend = FakeAudio::refusing_earcons();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(Some(backend.clone()), None, Arc::new(gateway));

  dispatcher
    .earcon(Earcon { name: "confirm".into() })
    .await
    .expect("accepted");

  let reply = peer.wait("an audio error", audio_error).await;
  assert_eq!(reply.error, AudioError::Unavailable { verb: "earcon".into() });
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_cancels_everything_still_speaking() {
  let backend = FakeAudio::holding_speech();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(Some(backend.clone()), None, Arc::new(gateway));
  let id = Uuid::now_v7();

  dispatcher.tts(tts(id, "a long sentence")).await.expect("accepted");
  peer.wait("a ttsStarted", started_with(id)).await;

  dispatcher.stop().await;

  assert_eq!(backend.cancel_all.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn volume_verbs_go_to_the_authority_that_owns_volume() {
  let volume = Arc::new(FakeVolume::default());
  let authority = FakeAuthority::owning();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(None, Some(volume.clone()), Arc::new(gateway));
  dispatcher.set_volume_authority(Some(authority.clone()));

  dispatcher.volume_up().await.expect("accepted");

  let changed = peer.wait("a volumeChanged", volume_changed).await;
  assert_eq!(changed.level, 0.75, "the level the authority landed on is broadcast");
  assert_eq!(*authority.calls.lock().unwrap(), vec!["volumeUp".to_string()]);
  assert_eq!(
    volume.volume_up.load(Ordering::SeqCst),
    0,
    "the host's own volume is not moved when a remote player owns it"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_authority_verb_reports_action_rejected() {
  let authority = FakeAuthority::refusing();
  let (gateway, peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(None, Some(Arc::new(FakeVolume::default())), Arc::new(gateway));
  dispatcher.set_volume_authority(Some(authority.clone()));

  dispatcher.set_volume(SetVolume { level: 0.5 }).await.expect("accepted");

  let reply = peer.wait("an audio error", audio_error).await;
  assert_eq!(
    reply.error,
    AudioError::ActionRejected {
      reason: "setVolume: device is gone".into()
    }
  );
  peer.quiet("a volumeChanged", volume_changed).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mute_verbs_are_dropped_while_the_authority_owns_volume() {
  let volume = Arc::new(FakeVolume::default());
  let (gateway, _peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(None, Some(volume.clone()), Arc::new(gateway));
  dispatcher.set_volume_authority(Some(FakeAuthority::owning()));

  dispatcher.mute_toggle().await.expect("accepted");
  dispatcher.set_mute(SetMute { muted: true }).await.expect("accepted");

  assert_eq!(volume.mute_toggle.load(Ordering::SeqCst), 0);
  assert!(volume.set_mute.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn volume_verbs_fall_back_to_the_host_mixer_when_nothing_owns_volume() {
  let volume = Arc::new(FakeVolume::default());
  let authority = FakeAuthority::not_owning();
  let (gateway, _peer) = Peer::link();
  let dispatcher = AudioDispatcher::new(None, Some(volume.clone()), Arc::new(gateway));
  dispatcher.set_volume_authority(Some(authority.clone()));

  dispatcher.volume_up().await.expect("accepted");
  dispatcher.mute_toggle().await.expect("accepted");

  assert_eq!(volume.volume_up.load(Ordering::SeqCst), 1);
  assert_eq!(volume.mute_toggle.load(Ordering::SeqCst), 1);
  assert!(authority.calls.lock().unwrap().is_empty());
}
