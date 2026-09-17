#![cfg(target_arch = "wasm32")]

mod convert;

use std::{collections::BTreeMap, sync::Arc};

use bridgething_delivery::{
  bundle::fetch::{HttpArtifactFetch, fetch_json},
  ota::{
    event::{OtaPollEvent, parse_ota_kind},
    manifest::{OtaArtifactUrls, OtaCompositeVersion, OtaDiscoverManifest},
    service::{BandaidArtifact, IMAGE_SWU_ASSET},
    stream::Artifact,
  },
  seam::SystemClock,
  session::{DeliverySession, SessionDeps, gateway_info},
  transfer::BytesSource,
};
use bridgething_gateway::{
  RequestFailure, TransportError,
  wasm::{ChannelConnector, channel_link},
};
use bridgething_io::{FetchTransport, HttpExecutor};
use bytes::Bytes;
use convert::{Phase, UpdateEvent, install_result, lagged, to_js};
use futures::{StreamExt, channel::mpsc};
use libbridgething::{
  OtaKind,
  gateway::{DeviceSetNickname, LauncherGestureSet, WebappSetSlot, WebappSwitchTo, WebappUninstall},
};
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};

const DEFAULT_DEVICE_ID: &str = "core-wasm";

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &'static str = r#"
export type OtaUpdateKind = "image" | "daemon" | "builtinWebapp" | "installedWebapp" | "wakewordModel";

export type PhaseKind = "idle" | "downloading" | "streaming" | "applying" | "staged" | "completed" | "failed";

export interface Phase {
  kind: PhaseKind;
  asset?: string;
  received?: number;
  sent?: number;
  total?: number;
  ratePerSec?: number;
  etaSeconds?: number;
  writePercent?: number;
  reason?: string;
}

export interface PlanStep {
  id: number;
  kind: "download" | "stream" | "apply" | "reboot";
  label: string;
  bytes: number;
}

export type UpdateEventKind =
  | "manifestPolled"
  | "manifestPollFailed"
  | "updateAvailable"
  | "planned"
  | "progress"
  | "updated"
  | "failed"
  | "lagged";

export interface UpdateEvent {
  kind: UpdateEventKind;
  deviceId?: string;
  updateKind?: OtaUpdateKind;
  release?: string;
  daemonVersion?: string;
  imageVersion?: string;
  channel?: string;
  rootUrl?: string;
  version?: string;
  updatedAt?: string;
  reason?: string;
  stepId?: number;
  steps?: PlanStep[];
  phase?: Phase;
}

export interface InstalledWebapp {
  id: string;
  name: string;
  version: string;
  provenance?: string;
}

export interface CompositeVersion {
  daemon: string;
  image: string;
}

export interface ArtifactUrls {
  imageSwu: string;
  imageZck: string;
  imageBootZck: string;
  daemonBinary: string;
  daemonBinaryZst: string;
}

export interface ArtifactDigest {
  size: number;
  sha256: string;
}

export interface OtaPatchDigest {
  size: number;
  sha256: string;
  sourceSha256: string | null;
}

export interface OtaReleaseArtifacts {
  daemon: ArtifactDigest | null;
  daemonZst: ArtifactDigest | null;
  imageSwu: ArtifactDigest | null;
  imageZck: ArtifactDigest | null;
  imageBootZck: ArtifactDigest | null;
  webapps: Record<string, ArtifactDigest>;
  daemonPatches: Record<string, OtaPatchDigest>;
}

export interface OtaManifestChannel {
  name: string;
  stability: string;
  isDefault: boolean;
  latest: string;
  releases: string[];
}

export interface OtaManifestRelease {
  version: string;
  channel: string;
  yanked: string | null;
  deprecated: boolean;
  builtinWebapps: Record<string, string>;
  artifacts: OtaReleaseArtifacts | null;
}

export interface OtaDiscoverManifest {
  manifestVersion: number;
  updatedAt: string;
  channels: Record<string, OtaManifestChannel>;
  releases: Record<string, OtaManifestRelease>;
}
"#;

#[wasm_bindgen(js_name = init)]
pub fn init() {
  console_error_panic_hook::set_once();
  let _ = tracing_subscriber::fmt()
    .with_writer(ConsoleWriter)
    .without_time()
    .with_ansi(false)
    .try_init();
}

#[derive(Clone, Copy)]
struct ConsoleWriter;

impl std::io::Write for ConsoleWriter {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    let line = String::from_utf8_lossy(buf);
    web_sys::console::log_1(&JsValue::from_str(line.trim_end()));
    Ok(buf.len())
  }

  fn flush(&mut self) -> std::io::Result<()> {
    Ok(())
  }
}

impl tracing_subscriber::fmt::MakeWriter<'_> for ConsoleWriter {
  type Writer = ConsoleWriter;

  fn make_writer(&self) -> ConsoleWriter {
    *self
  }
}

#[wasm_bindgen(js_name = ByteLink)]
pub struct WasmByteLink {
  inbound: mpsc::UnboundedSender<Result<Bytes, TransportError>>,
  connector: Option<ChannelConnector>,
}

#[wasm_bindgen(js_class = ByteLink)]
impl WasmByteLink {
  #[wasm_bindgen(constructor)]
  pub fn new(
    #[wasm_bindgen(unchecked_param_type = "(chunk: Uint8Array) => Promise<void>")] write: js_sys::Function,
  ) -> Self {
    let (connector, ports) = channel_link();
    let mut outbound = ports.outbound;
    spawn_local(async move {
      while let Some(batch) = outbound.next().await {
        let chunk = js_sys::Uint8Array::from(batch.as_ref());
        let sent = write.call1(&JsValue::NULL, &chunk);
        match sent {
          Ok(value) => match value.dyn_ref::<js_sys::Promise>() {
            Some(promise) => {
              if JsFuture::from(promise.clone()).await.is_err() {
                break;
              }
            }
            None => continue,
          },
          Err(_) => break,
        }
      }
    });

    WasmByteLink {
      inbound: ports.inbound,
      connector: Some(connector),
    }
  }

  #[wasm_bindgen(js_name = push)]
  pub fn push(&self, chunk: &[u8]) {
    let _ = self.inbound.unbounded_send(Ok(Bytes::copy_from_slice(chunk)));
  }

  #[wasm_bindgen(js_name = close)]
  pub fn close(&self) {
    self.inbound.close_channel();
  }
}

#[wasm_bindgen(js_name = DeliverySession)]
pub struct WasmSession {
  session: Arc<DeliverySession>,
  events: Arc<Mutex<broadcast::Receiver<OtaPollEvent>>>,
}

#[wasm_bindgen(js_class = DeliverySession)]
impl WasmSession {
  #[wasm_bindgen(js_name = connect)]
  pub async fn connect(url: String, device_id: Option<String>) -> Result<WasmSession, JsValue> {
    let deps = deps(device_id);
    let session = DeliverySession::connect(&url, deps)
      .await
      .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(WasmSession::hold(session))
  }

  #[wasm_bindgen(js_name = attach)]
  pub async fn attach(link: &mut WasmByteLink, device_id: Option<String>) -> Result<WasmSession, JsValue> {
    let connector = link
      .connector
      .take()
      .ok_or_else(|| JsValue::from_str("this ByteLink already carries a session"))?;
    let session = DeliverySession::spawn(connector, deps(device_id))
      .await
      .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(WasmSession::hold(session))
  }

  #[wasm_bindgen(getter, js_name = deviceId)]
  pub fn device_id(&self) -> String {
    self.session.device_id().to_owned()
  }

  #[wasm_bindgen(js_name = meta)]
  pub async fn meta(&self) -> Result<JsValue, JsValue> {
    match self.session.ota.meta(self.session.device_id()).await {
      Some(meta) => to_js(&meta),
      None => Ok(JsValue::NULL),
    }
  }

  #[wasm_bindgen(js_name = push, unchecked_return_type = "Phase")]
  pub async fn push(
    &self,
    #[wasm_bindgen(unchecked_param_type = "OtaUpdateKind")] kind: String,
    artifact: Vec<u8>,
    label: Option<String>,
  ) -> Result<JsValue, JsValue> {
    let kind = parse_kind(&kind)?;
    let source = Arc::new(BytesSource::new(artifact));
    let terminal = match kind {
      OtaKind::Image => {
        self
          .session
          .ota
          .push_update(self.session.device_id(), source.clone(), BTreeMap::new(), None)
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
              label: label.unwrap_or_else(|| label_for(kind).to_owned()),
              patch: None,
              version: None,
            }],
          )
          .await
      }
    };
    to_js(&Phase::from(terminal))
  }

  #[wasm_bindgen(js_name = pushImage, unchecked_return_type = "Phase")]
  pub async fn push_image(
    &self,
    swu: Vec<u8>,
    #[wasm_bindgen(unchecked_param_type = "Map<string, Uint8Array>")] zcks: js_sys::Map,
    update_url_base: Option<String>,
  ) -> Result<JsValue, JsValue> {
    let mut assets: BTreeMap<String, Arc<dyn Artifact>> = BTreeMap::new();
    for entry in zcks.entries() {
      let entry: js_sys::Array = entry.map_err(|e| JsValue::from_str(&format!("zck map: {e:?}")))?.into();
      let name = entry
        .get(0)
        .as_string()
        .ok_or_else(|| JsValue::from_str("zck names must be strings"))?;
      let bytes = entry
        .get(1)
        .dyn_into::<js_sys::Uint8Array>()
        .map_err(|_| JsValue::from_str(&format!("zck {name} must be a Uint8Array")))?;
      assets.insert(name, Arc::new(BytesSource::new(bytes.to_vec())) as Arc<dyn Artifact>);
    }

    let source = Arc::new(BytesSource::new(swu));
    let terminal = self
      .session
      .ota
      .push_update(self.session.device_id(), source, assets, update_url_base.as_deref())
      .await;
    to_js(&Phase::from(terminal))
  }

  #[wasm_bindgen(js_name = installWebapp, unchecked_return_type = "InstalledWebapp")]
  pub async fn install_webapp(&self, bundle: Vec<u8>, provenance: Option<String>) -> Result<JsValue, JsValue> {
    let source = Arc::new(BytesSource::new(bundle));
    let installed = install_result(
      self
        .session
        .ota
        .install_webapp(self.session.device_id(), source, provenance.as_deref(), None)
        .await,
    )?;
    to_js(&installed)
  }

  #[wasm_bindgen(js_name = webapps)]
  pub async fn webapps(&self) -> Result<JsValue, JsValue> {
    let list = self.session.gateway.webapp().list().await.map_err(failure)?;
    to_js(&list.webapps)
  }

  #[wasm_bindgen(js_name = activeWebapp)]
  pub async fn active_webapp(&self) -> Result<JsValue, JsValue> {
    let active = self.session.gateway.webapp().get_active().await.map_err(failure)?;
    to_js(&active)
  }

  #[wasm_bindgen(js_name = webappSlots)]
  pub async fn webapp_slots(&self) -> Result<JsValue, JsValue> {
    let slots = self.session.gateway.webapp().get_slots().await.map_err(failure)?;
    to_js(&slots)
  }

  #[wasm_bindgen(js_name = switchWebapp)]
  pub async fn switch_webapp(&self, id: String) -> Result<JsValue, JsValue> {
    let id = parse_id("webapp id", &id)?;
    let active = self
      .session
      .gateway
      .webapp()
      .switch_to(WebappSwitchTo { id })
      .await
      .map_err(failure)?;
    to_js(&active)
  }

  #[wasm_bindgen(js_name = uninstallWebapp)]
  pub async fn uninstall_webapp(&self, id: String) -> Result<JsValue, JsValue> {
    let id = parse_id("webapp id", &id)?;
    let active = self
      .session
      .gateway
      .webapp()
      .uninstall(WebappUninstall { id })
      .await
      .map_err(failure)?;
    to_js(&active)
  }

  #[wasm_bindgen(js_name = setWebappSlot)]
  pub async fn set_webapp_slot(
    &self,
    #[wasm_bindgen(unchecked_param_type = "\"launcher\" | \"overlay\"")] slot: String,
    id: Option<String>,
  ) -> Result<JsValue, JsValue> {
    let slot = serde_json::from_value(serde_json::Value::String(slot.clone()))
      .map_err(|_| JsValue::from_str(&format!("unknown webapp slot {slot}")))?;
    let id = id.map(|id| parse_id("webapp id", &id)).transpose()?;
    let slots = self
      .session
      .gateway
      .webapp()
      .set_slot(WebappSetSlot { slot, id })
      .await
      .map_err(failure)?;
    to_js(&slots)
  }

  #[wasm_bindgen(js_name = setLauncherGesture)]
  pub async fn set_launcher_gesture(
    &self,
    #[wasm_bindgen(unchecked_param_type = "\"longPress\" | \"fivePress\"")] gesture: String,
  ) -> Result<(), JsValue> {
    let gesture = serde_json::from_value(serde_json::Value::String(gesture.clone()))
      .map_err(|_| JsValue::from_str(&format!("unknown launcher gesture {gesture}")))?;
    self
      .session
      .gateway
      .system()
      .launcher_gesture_set(LauncherGestureSet { gesture })
      .await
      .map_err(|error| JsValue::from_str(&format!("{error:?}")))?;
    Ok(())
  }

  #[wasm_bindgen(js_name = setNickname)]
  pub async fn set_nickname(&self, nickname: String) -> Result<JsValue, JsValue> {
    let reply = self
      .session
      .gateway
      .system()
      .device_set_nickname(DeviceSetNickname { nickname })
      .await
      .map_err(failure)?;
    self
      .session
      .ota
      .nickname_changed(self.session.device_id(), reply.nickname.clone());
    to_js(&reply)
  }

  #[wasm_bindgen(js_name = nextEvent, unchecked_return_type = "UpdateEvent")]
  pub async fn next_event(&self) -> Result<JsValue, JsValue> {
    let mut events = self.events.lock().await;
    match events.recv().await {
      Ok(event) => to_js(&UpdateEvent::from(event)),
      Err(broadcast::error::RecvError::Lagged(dropped)) => to_js(&lagged(dropped)),
      Err(broadcast::error::RecvError::Closed) => Err(JsValue::from_str("the update feed closed")),
    }
  }

  #[wasm_bindgen(js_name = closed)]
  pub async fn closed(&self) {
    self.session.closed().await;
  }
}

impl WasmSession {
  fn hold(session: DeliverySession) -> Self {
    let events = session.ota.events();
    WasmSession {
      session: Arc::new(session),
      events: Arc::new(Mutex::new(events)),
    }
  }
}

#[wasm_bindgen(js_name = discoverManifest, unchecked_return_type = "OtaDiscoverManifest")]
pub async fn discover_manifest(root_url: String) -> Result<JsValue, JsValue> {
  let fetch = HttpArtifactFetch::new(HttpExecutor::new(Arc::new(FetchTransport::new())));
  let manifest: OtaDiscoverManifest = fetch_json(&fetch, &format!("{}/discover.json", root_url.trim_end_matches('/')))
    .await
    .map_err(|e| JsValue::from_str(&format!("manifest fetch failed: {e}")))?;
  to_js(&manifest)
}

#[wasm_bindgen(js_name = parseCompositeVersion, unchecked_return_type = "CompositeVersion | null")]
pub fn parse_composite_version(raw: String) -> Result<JsValue, JsValue> {
  match OtaCompositeVersion::parse(&raw) {
    Some(parsed) => to_js(&parsed),
    None => Ok(JsValue::NULL),
  }
}

#[wasm_bindgen(js_name = artifactUrls, unchecked_return_type = "ArtifactUrls")]
pub fn artifact_urls(
  root_url: String,
  channel: String,
  daemon_version: String,
  image_version: String,
  image_variant: String,
) -> Result<JsValue, JsValue> {
  to_js(&OtaArtifactUrls::build(
    &root_url,
    &channel,
    &daemon_version,
    &image_version,
    &image_variant,
  ))
}

fn deps(device_id: Option<String>) -> SessionDeps {
  let device_id = device_id.unwrap_or_else(|| DEFAULT_DEVICE_ID.to_owned());
  SessionDeps {
    clock: Arc::new(SystemClock),
    fetch: Arc::new(HttpArtifactFetch::new(HttpExecutor::new(Arc::new(
      FetchTransport::new(),
    )))),
    cache_dir: std::path::PathBuf::new(),
    data_dir: None,
    info: gateway_info(&device_id, "browser", env!("CARGO_PKG_VERSION")),
    device_id,
  }
}

fn parse_id(what: &str, id: &str) -> Result<Uuid, JsValue> {
  Uuid::parse_str(id).map_err(|e| JsValue::from_str(&format!("{what}: {e}")))
}

fn failure<E: std::fmt::Debug>(error: RequestFailure<E>) -> JsValue {
  match error {
    RequestFailure::Domain(error) => JsValue::from_str(&format!("the daemon refused: {error:?}")),
    other => JsValue::from_str(&format!("request failed: {other:?}")),
  }
}

fn parse_kind(kind: &str) -> Result<OtaKind, JsValue> {
  parse_ota_kind(kind).ok_or_else(|| JsValue::from_str(&format!("unknown update kind {kind}")))
}

fn label_for(kind: OtaKind) -> &'static str {
  match kind {
    OtaKind::Image => IMAGE_SWU_ASSET,
    OtaKind::Daemon => "daemon",
    OtaKind::BuiltinWebapp | OtaKind::InstalledWebapp => "webapp",
    OtaKind::WakewordModel => "wakeword",
  }
}
