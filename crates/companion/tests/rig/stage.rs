use std::sync::{Arc, Mutex};

use bridgething_companion::{
  api::{CapabilityFlags, CompanionBackends, CompanionConfig, HostInfo},
  backend::{ImageScaler, ImageTarget, NativeHttp, ScaledImage, StreamBackend},
  hub::Hub,
  media::MediaSniff,
  provider::{Provider, ProviderAuthState},
  session::Session,
};
use libbridgething::{
  PlaybackState, PlayerState,
  gateway::{GatewayToBridgeMsg, GatewayToBridgeMsgData, GatewayToBridgePlayerMsg, QueueSnapshot},
};

use crate::{
  art_store,
  backends::{Heard, Offline, RigHost},
  log_sink::Quiet,
  poll::eventually,
  secrets::MemorySecrets,
  stream_fake::{FakeStreamBackend, StreamCall},
  support::Peer,
};

pub struct TagScaler;

impl ImageScaler for TagScaler {
  fn scale(&self, bytes: Vec<u8>, _sniff: MediaSniff, target: ImageTarget) -> Option<ScaledImage> {
    let ImageTarget::Thumbnail { max_edge, .. } = target else {
      return None;
    };
    Some(ScaledImage {
      bytes: format!("{}@{max_edge}", String::from_utf8_lossy(&bytes)).into_bytes(),
      mime: "image/jpeg".into(),
      width: max_edge,
      height: max_edge,
    })
  }
}

pub fn snapshot_of(msg: &GatewayToBridgeMsg) -> Option<PlayerState> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::Snapshot(state)) => Some(state.as_ref().clone()),
    _ => None,
  }
}

pub fn queue_of(msg: &GatewayToBridgeMsg) -> Option<QueueSnapshot> {
  match &msg.data {
    GatewayToBridgeMsgData::Player(GatewayToBridgePlayerMsg::QueueChanged(queue)) => Some(queue.clone()),
    _ => None,
  }
}

pub struct Stage {
  pub hub: Arc<Hub>,
  pub peer: Peer,
  pub backend: Arc<FakeStreamBackend>,
  auth: Arc<Mutex<Vec<ProviderAuthState>>>,
  art: art_store::Art,
}

impl Stage {
  pub fn new(app_name: &str) -> Self {
    let (gateway, peer) = Peer::link();
    let hub = Hub::new(
      Arc::new(gateway),
      HostInfo {
        app_name: app_name.into(),
        app_version: "0.0.1".into(),
        os_name: "test".into(),
        os_version: String::new(),
        host_identifier: String::new(),
      },
      CapabilityFlags {
        geo: false,
        notifications: false,
        net_fetch: true,
        net_ws: true,
        audio_tts: false,
        voice_model: false,
      },
      false,
      false,
    );
    hub.start();
    Stage {
      hub,
      peer,
      backend: FakeStreamBackend::new(),
      auth: Arc::new(Mutex::new(Vec::new())),
      art: art_store::Art::new(Some(Arc::new(TagScaler))),
    }
  }

  pub fn art(&self) -> Arc<bridgething_companion::provider::art::ArtCache> {
    self.art.cache()
  }

  pub async fn attach(&self, provider: Arc<dyn Provider>) {
    let seen = self.auth.clone();
    provider.set_auth_observer(Some(Arc::new(move |state| seen.lock().unwrap().push(state))));
    self.hub.attach(provider).await.expect("the provider attached");
  }

  pub fn auth_states(&self) -> Vec<ProviderAuthState> {
    self.auth.lock().unwrap().clone()
  }

  pub async fn settled(&self, settled: impl Fn(&ProviderAuthState) -> bool) -> ProviderAuthState {
    assert!(
      eventually(|| self.auth.lock().unwrap().iter().any(&settled)).await,
      "the sign-in never settled: {:?}",
      self.auth_states()
    );
    self.auth_states().into_iter().find(settled).expect("the settled state")
  }

  pub async fn authenticated(&self) {
    self
      .settled(|state| matches!(state, ProviderAuthState::Authenticated))
      .await;
  }

  pub async fn snapshot_for(&self, uri: &str) -> PlayerState {
    let wanted = uri.to_owned();
    self
      .peer
      .wait(&format!("a snapshot for {uri}"), |msg| {
        snapshot_of(msg).filter(|state| {
          state.playback.state == PlaybackState::Playing
            && state.track.as_ref().and_then(|track| track.uri.as_deref()) == Some(wanted.as_str())
        })
      })
      .await
  }

  pub async fn queue_with(&self, order: &[&str]) -> QueueSnapshot {
    let wanted: Vec<String> = order.iter().map(|uri| (*uri).to_owned()).collect();
    self
      .peer
      .wait(&format!("a queue of {order:?}"), |msg| {
        queue_of(msg).filter(|queue| queue.order == wanted)
      })
      .await
  }

  pub fn played_urls(&self) -> Vec<String> {
    self
      .backend
      .transport_calls()
      .into_iter()
      .filter_map(|call| match call {
        StreamCall::Play(url) => Some(url),
        _ => None,
      })
      .collect()
  }
}

pub fn boot_session(
  app_name: &str,
  secrets: Arc<MemorySecrets>,
  stream: Option<Arc<FakeStreamBackend>>,
) -> Arc<Session> {
  let spool = std::env::temp_dir().join(format!("{app_name}-{}", uuid::Uuid::now_v7().simple()));
  std::fs::create_dir_all(&spool).expect("a scratch dir");
  let backends = CompanionBackends {
    link: None,
    host: Arc::new(RigHost),
    http: Arc::new(NativeHttp::default()),
    ws: Arc::new(Offline),
    secrets,
    log: Arc::new(Quiet),
    audio: None,
    volume: None,
    geo: None,
    notifications: None,
    phone: None,
    media_sessions: None,
    stream: stream.map(|backend| backend as Arc<dyn StreamBackend>),
    speech: None,
    nlu: None,
    apple_music: None,
    image: None,
    transcoder: None,
    model_validator: None,
    transfer_policy: None,
    connectivity: None,
    device_waker: None,
    extensions: None,
  };
  Session::new(
    CompanionConfig {
      host: HostInfo {
        app_name: app_name.into(),
        app_version: "0.0.0".into(),
        os_name: "linux".into(),
        os_version: String::new(),
        host_identifier: String::new(),
      },
      capabilities: CapabilityFlags {
        geo: false,
        notifications: false,
        net_fetch: false,
        net_ws: false,
        audio_tts: false,
        voice_model: false,
      },
      state_dir: spool.to_string_lossy().into_owned(),
      cache_dir: spool.to_string_lossy().into_owned(),
      model_platform: None,
      spotify: None,
    },
    backends,
    Arc::new(Heard::default()),
    Arc::new(Offline),
  )
}

pub fn stream_session(app_name: &str) -> (Arc<Session>, Arc<MemorySecrets>) {
  let secrets = Arc::new(MemorySecrets::default());
  let session = boot_session(app_name, secrets.clone(), Some(FakeStreamBackend::new()));
  (session, secrets)
}
