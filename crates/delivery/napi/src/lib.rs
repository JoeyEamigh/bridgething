mod convert;

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use bridgething_delivery::{
  bundle::{
    BundleConfig, BundleKind, BundlePlatform,
    fetch::{ArtifactFetch, HttpArtifactFetch},
  },
  discovery::{Discovery as CoreDiscovery, EndpointChange as CoreEndpointChange},
  ota::{
    event::{OtaPollEvent, parse_ota_kind},
    manifest::OtaCompositeVersion,
    service::{BandaidArtifact, IMAGE_SWU_ASSET},
    stream::{Artifact, FileSource},
  },
  seam::{ArtifactKind, ArtifactValidator, SystemClock, TransferPolicy},
  session::{DeliverySession, SessionDeps, gateway_info},
  transfer::BytesSource,
};
use bridgething_gateway::RequestFailure;
use bridgething_io::{HttpExecutor, ReqwestConfig, ReqwestTransport};
use convert::{
  BundleStatus, Endpoint, EndpointChange, InstalledWebapp, Phase, UpdateEvent, bundle_status, install_result, lagged,
};
use libbridgething::{OtaKind, gateway::WebappSwitchTo};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;

const DEFAULT_DEVICE_ID: &str = "core-node";

#[napi(object)]
pub struct ConnectOptions {
  pub device_id: Option<String>,
  pub cache_dir: Option<String>,
  pub data_dir: Option<String>,
  pub app_name: Option<String>,
  pub app_version: Option<String>,
}

#[napi(object)]
pub struct PushOptions {
  pub update_url_base: Option<String>,
  pub zcks: Option<HashMap<String, String>>,
  pub label: Option<String>,
  pub version: Option<String>,
}

#[napi]
pub struct DeliveryClient {
  session: Arc<DeliverySession>,
  events: Arc<Mutex<broadcast::Receiver<OtaPollEvent>>>,
}

#[napi]
impl DeliveryClient {
  #[napi(factory)]
  pub async fn connect(url: String, options: Option<ConnectOptions>) -> Result<DeliveryClient> {
    let options = options.unwrap_or(ConnectOptions {
      device_id: None,
      cache_dir: None,
      data_dir: None,
      app_name: None,
      app_version: None,
    });
    let name = options.app_name.unwrap_or_else(|| DEFAULT_DEVICE_ID.to_owned());
    let version = options
      .app_version
      .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    let deps = SessionDeps {
      device_id: options.device_id.unwrap_or_else(|| DEFAULT_DEVICE_ID.to_owned()),
      clock: Arc::new(SystemClock),
      fetch: node_fetch(),
      cache_dir: options
        .cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("bridgething-core-node")),
      data_dir: Some(
        options
          .data_dir
          .map(PathBuf::from)
          .unwrap_or_else(|| std::env::temp_dir().join("bridgething-core-node-state")),
      ),
      info: gateway_info(&name, std::env::consts::OS, &version),
    };

    let session = DeliverySession::connect(&url, deps)
      .await
      .map_err(|e| Error::from_reason(e.to_string()))?;
    let events = session.ota.events();
    Ok(DeliveryClient {
      session: Arc::new(session),
      events: Arc::new(Mutex::new(events)),
    })
  }

  #[napi]
  pub fn device_id(&self) -> String {
    self.session.device_id().to_owned()
  }

  #[napi]
  pub async fn meta(&self) -> Result<Option<serde_json::Value>> {
    match self.session.ota.meta(self.session.device_id()).await {
      Some(meta) => serde_json::to_value(meta)
        .map(Some)
        .map_err(|e| Error::from_reason(e.to_string())),
      None => Ok(None),
    }
  }

  #[napi]
  pub async fn discover_manifest(&self, root_url: String) -> Result<serde_json::Value> {
    let manifest = self
      .session
      .ota
      .discover_manifest(&root_url)
      .await
      .map_err(|e| Error::from_reason(e.to_string()))?;
    serde_json::to_value(manifest).map_err(|e| Error::from_reason(e.to_string()))
  }

  #[napi]
  pub async fn apply_version(&self, channel: String, version: String, root_url: String) {
    self
      .session
      .ota
      .apply_version(self.session.device_id(), &channel, &version, &root_url)
      .await;
  }

  #[napi]
  pub async fn push(
    &self,
    kind: String,
    artifact: Either<String, Buffer>,
    options: Option<PushOptions>,
  ) -> Result<Phase> {
    let kind = parse_kind(&kind)?;
    let options = options.unwrap_or(PushOptions {
      update_url_base: None,
      zcks: None,
      label: None,
      version: None,
    });
    let source = artifact_of(artifact);

    let terminal = match kind {
      OtaKind::Image => {
        let zcks = options
          .zcks
          .unwrap_or_default()
          .into_iter()
          .map(|(asset, path)| (asset, Arc::new(FileSource::open(path)) as Arc<dyn Artifact>))
          .collect();
        self
          .session
          .ota
          .push_update(
            self.session.device_id(),
            source.clone(),
            zcks,
            options.update_url_base.as_deref(),
          )
          .await
      }
      kind => {
        self
          .session
          .ota
          .push_bandaid_batch(
            self.session.device_id(),
            vec![BandaidArtifact {
              kind,
              artifact: source,
              label: options.label.unwrap_or_else(|| label_for(kind).to_owned()),
              patch: None,
              version: options.version,
            }],
          )
          .await
      }
    };
    Ok(terminal.into())
  }

  #[napi]
  pub async fn install_webapp(
    &self,
    bundle: Either<String, Buffer>,
    provenance: Option<String>,
  ) -> Result<InstalledWebapp> {
    let source = artifact_of(bundle);
    install_result(
      self
        .session
        .ota
        .install_webapp(self.session.device_id(), source, provenance.as_deref(), None)
        .await,
    )
  }

  #[napi]
  pub async fn switch_webapp(&self, id: String) -> Result<String> {
    let id = Uuid::parse_str(&id).map_err(|e| Error::from_reason(format!("webapp id: {e}")))?;
    match self.session.gateway.webapp().switch_to(WebappSwitchTo { id }).await {
      Ok(active) => Ok(active.id.map(|id| id.to_string()).unwrap_or_default()),
      Err(RequestFailure::Domain(error)) => Err(Error::from_reason(format!("daemon rejected switch: {error:?}"))),
      Err(other) => Err(Error::from_reason(format!("switch failed: {other:?}"))),
    }
  }

  #[napi]
  pub async fn next_event(&self) -> Result<UpdateEvent> {
    let mut events = self.events.lock().await;
    match events.recv().await {
      Ok(event) => Ok(event.into()),
      Err(broadcast::error::RecvError::Lagged(dropped)) => Ok(lagged(dropped)),
      Err(broadcast::error::RecvError::Closed) => Err(Error::from_reason("the update feed closed")),
    }
  }

  #[napi]
  pub async fn closed(&self) {
    self.session.closed().await;
  }
}

const DISCOVERY_CHANGE_BACKLOG: usize = 64;

#[napi]
pub struct Discovery {
  inner: Arc<CoreDiscovery>,
  changes: Mutex<broadcast::Receiver<CoreEndpointChange>>,
}

#[napi]
impl Discovery {
  #[napi(constructor)]
  pub fn new() -> Result<Self> {
    let (sender, changes) = broadcast::channel(DISCOVERY_CHANGE_BACKLOG);
    let inner = CoreDiscovery::spawn(move |change| {
      let _ = sender.send(change);
    })
    .map_err(|e| Error::from_reason(e.to_string()))?;
    Ok(Discovery {
      inner,
      changes: Mutex::new(changes),
    })
  }

  #[napi]
  pub fn endpoints(&self) -> Vec<Endpoint> {
    self.inner.endpoints().into_iter().map(Endpoint::from).collect()
  }

  #[napi]
  pub async fn next_change(&self) -> Result<EndpointChange> {
    let mut changes = self.changes.lock().await;
    loop {
      match changes.recv().await {
        Ok(change) => return Ok(change.into()),
        Err(broadcast::error::RecvError::Lagged(_)) => continue,
        Err(broadcast::error::RecvError::Closed) => return Err(Error::from_reason("the mdns browser stopped")),
      }
    }
  }
}

#[napi]
pub struct BundleStore {
  inner: Arc<bridgething_delivery::bundle::BundleStore>,
}

#[napi]
impl BundleStore {
  #[napi(constructor)]
  pub fn new(kind: String, platform: String, storage_dir: String) -> Result<Self> {
    let kind = match kind.as_str() {
      "nlu" => BundleKind::Nlu,
      "asr" => BundleKind::Asr,
      other => return Err(Error::from_reason(format!("unknown bundle kind {other}"))),
    };
    let platform = match platform.as_str() {
      "ios" => BundlePlatform::Ios,
      "android" => BundlePlatform::Android,
      other => return Err(Error::from_reason(format!("unknown bundle platform {other}"))),
    };
    Ok(BundleStore {
      inner: Arc::new(bridgething_delivery::bundle::BundleStore::new(
        kind,
        BundleConfig::new(PathBuf::from(storage_dir), platform),
        true,
        node_fetch(),
        Arc::new(UnmeteredLink),
        Arc::new(ShapeOnly),
      )),
    })
  }

  #[napi]
  pub async fn ensure(&self) -> BundleStatus {
    self.inner.ensure().await;
    self.status()
  }

  #[napi]
  pub fn status(&self) -> BundleStatus {
    bundle_status(
      self.inner.state(),
      self.inner.live().map(|path| path.display().to_string()),
    )
  }
}

#[napi(object)]
pub struct CompositeVersion {
  pub daemon: String,
  pub image: String,
}

#[napi]
pub fn parse_composite_version(raw: String) -> Option<CompositeVersion> {
  OtaCompositeVersion::parse(&raw).map(|parsed| CompositeVersion {
    daemon: parsed.daemon,
    image: parsed.image,
  })
}

#[napi]
pub fn init_logging(directive: Option<String>) {
  let filter = match directive {
    Some(directive) => tracing_subscriber::EnvFilter::new(directive),
    None => tracing_subscriber::EnvFilter::try_from_default_env()
      .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("bridgething_delivery=info,info")),
  };
  let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

fn node_fetch() -> Arc<dyn ArtifactFetch> {
  let transport = Arc::new(ReqwestTransport::new(ReqwestConfig::default()));
  Arc::new(HttpArtifactFetch::new(HttpExecutor::new(transport)))
}

fn artifact_of(artifact: Either<String, Buffer>) -> Arc<dyn Artifact> {
  match artifact {
    Either::A(path) => Arc::new(FileSource::open(path)),
    Either::B(bytes) => Arc::new(BytesSource::new(bytes.to_vec())),
  }
}

fn parse_kind(kind: &str) -> Result<OtaKind> {
  parse_ota_kind(kind).ok_or_else(|| Error::from_reason(format!("unknown update kind {kind}")))
}

fn label_for(kind: OtaKind) -> &'static str {
  match kind {
    OtaKind::Image => IMAGE_SWU_ASSET,
    OtaKind::Daemon => "daemon",
    OtaKind::BuiltinWebapp | OtaKind::InstalledWebapp => "webapp",
    OtaKind::WakewordModel => "wakeword",
  }
}

struct UnmeteredLink;

impl TransferPolicy for UnmeteredLink {
  fn allows_large_transfer(&self) -> bool {
    true
  }
}

struct ShapeOnly;

impl ArtifactValidator for ShapeOnly {
  fn validate(&self, _kind: ArtifactKind, _staged: &std::path::Path) -> std::result::Result<(), String> {
    Ok(())
  }
}
