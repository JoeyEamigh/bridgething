#![cfg(target_arch = "wasm32")]

mod convert;

use std::{collections::BTreeMap, sync::Arc};

use bridgething_delivery::{
  blob::{MemoryBlobStore, MemorySlotIndex},
  bundle::fetch::{ArtifactFetch, HttpArtifactFetch},
  ota::{
    event::{OtaPollEvent, parse_ota_kind},
    manifest::{OtaArtifactUrls, OtaCompositeVersion, fetch_discover_manifest},
    service::{BandaidArtifact, IMAGE_SWU_ASSET},
    stream::Artifact,
  },
  seam::{BlobStore, SystemClock},
  session::{DeliverySession, SessionDeps, gateway_info},
  transfer::BytesSource,
  webapp::{ResourceOrigin, WebappResourceService, blob::BlobProgress},
};
use bridgething_gateway::{
  RequestFailure, TransportError,
  wasm::{ChannelConnector, channel_link},
};
use bridgething_io::{FetchTransport, HttpExecutor};
use bytes::Bytes;
use convert::{DocChange, Phase, UpdateEvent, install_result, lagged, to_js};
use futures::{StreamExt, channel::mpsc};
use libbridgething::{
  OtaKind,
  gateway::{
    DeviceSetNickname, LauncherGestureSet, WebappConfigDelete, WebappConfigList, WebappConfigSet, WebappDocChanged,
    WebappDocDelete, WebappDocGet, WebappDocList, WebappDocSet, WebappResourceKind, WebappSetSlot, WebappSwitchTo,
    WebappUninstall,
  },
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

export type WebappResourceKind = "icon" | "settings" | "overlay";

export interface ResourceOrigin {
  url: string;
  sha256: string;
  size: number;
  mime: string | null;
}

export interface WebappResourceBody {
  digest: string;
  mime: string | null;
  bytes: Uint8Array;
}

export interface WebappDocChange {
  id: string;
  key: string;
  value: string | null;
}

export interface WebappBlobEntry {
  key: string;
  sha256: string;
  mime: string;
  size: number;
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
  docs: Arc<Mutex<broadcast::Receiver<WebappDocChanged>>>,
  blobs: Arc<MemoryBlobStore>,
  resources: Arc<WebappResourceService>,
}

#[wasm_bindgen(js_class = DeliverySession)]
impl WasmSession {
  #[wasm_bindgen(js_name = connect)]
  pub async fn connect(url: String, device_id: Option<String>) -> Result<WasmSession, JsValue> {
    let fetch = fetch();
    let session = DeliverySession::connect(&url, deps(device_id, fetch.clone()))
      .await
      .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(WasmSession::hold(session, fetch))
  }

  #[wasm_bindgen(js_name = attach)]
  pub async fn attach(link: &mut WasmByteLink, device_id: Option<String>) -> Result<WasmSession, JsValue> {
    let connector = link
      .connector
      .take()
      .ok_or_else(|| JsValue::from_str("this ByteLink already carries a session"))?;
    let fetch = fetch();
    let session = DeliverySession::spawn(connector, deps(device_id, fetch.clone()))
      .await
      .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(WasmSession::hold(session, fetch))
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

  #[wasm_bindgen(js_name = webappResource, unchecked_return_type = "WebappResourceBody")]
  pub async fn webapp_resource(
    &self,
    id: String,
    #[wasm_bindgen(unchecked_param_type = "WebappResourceKind")] kind: String,
    #[wasm_bindgen(unchecked_param_type = "ResourceOrigin | null | undefined")] origin: JsValue,
  ) -> Result<JsValue, JsValue> {
    let id = parse_id("webapp id", &id)?;
    let kind: WebappResourceKind = serde_json::from_value(serde_json::Value::String(kind.clone()))
      .map_err(|_| JsValue::from_str(&format!("unknown webapp resource kind {kind}")))?;
    let origin: Option<ResourceOrigin> = if origin.is_null() || origin.is_undefined() {
      None
    } else {
      Some(serde_wasm_bindgen::from_value(origin).map_err(|e| JsValue::from_str(&format!("resource origin: {e}")))?)
    };
    let resource = self
      .resources
      .fetch(&self.session.gateway, id, kind, origin.as_ref())
      .await
      .map_err(|error| JsValue::from_str(&error.to_string()))?;
    let bytes = self
      .blobs
      .get(&resource.digest)
      .map_err(|error| JsValue::from_str(&error))?
      .ok_or_else(|| JsValue::from_str(&format!("the resource cache lost {}", resource.digest)))?;

    let body = js_sys::Object::new();
    let mime = resource.mime.as_deref().map_or(JsValue::NULL, JsValue::from_str);
    for (key, value) in [
      ("digest", JsValue::from_str(&resource.digest)),
      ("mime", mime),
      ("bytes", js_sys::Uint8Array::from(bytes.as_slice()).into()),
    ] {
      js_sys::Reflect::set(&body, &JsValue::from_str(key), &value)?;
    }
    Ok(body.into())
  }

  #[wasm_bindgen(js_name = webappConfig)]
  pub async fn webapp_config(&self, id: String) -> Result<JsValue, JsValue> {
    let id = parse_id("webapp id", &id)?;
    let reply = self
      .session
      .gateway
      .webapp()
      .config_list(WebappConfigList { id })
      .await
      .map_err(failure)?;
    to_js(&reply.entries)
  }

  #[wasm_bindgen(js_name = setWebappConfig)]
  pub async fn set_webapp_config(&self, id: String, key: String, value: String) -> Result<(), JsValue> {
    let id = parse_id("webapp id", &id)?;
    self
      .session
      .gateway
      .webapp()
      .config_set(WebappConfigSet { id, key, value })
      .await
      .map_err(failure)?;
    Ok(())
  }

  #[wasm_bindgen(js_name = deleteWebappConfig)]
  pub async fn delete_webapp_config(&self, id: String, key: String) -> Result<(), JsValue> {
    let id = parse_id("webapp id", &id)?;
    self
      .session
      .gateway
      .webapp()
      .config_delete(WebappConfigDelete { id, key })
      .await
      .map_err(failure)?;
    Ok(())
  }

  #[wasm_bindgen(js_name = webappDoc)]
  pub async fn webapp_doc(&self, id: String) -> Result<JsValue, JsValue> {
    let id = parse_id("webapp id", &id)?;
    let reply = self
      .session
      .gateway
      .webapp()
      .doc_list(WebappDocList { id })
      .await
      .map_err(failure)?;
    to_js(&reply.entries)
  }

  #[wasm_bindgen(js_name = webappDocEntry)]
  pub async fn webapp_doc_entry(&self, id: String, key: String) -> Result<Option<String>, JsValue> {
    let id = parse_id("webapp id", &id)?;
    let reply = self
      .session
      .gateway
      .webapp()
      .doc_get(WebappDocGet { id, key })
      .await
      .map_err(failure)?;
    Ok(reply.value)
  }

  #[wasm_bindgen(js_name = setWebappDoc)]
  pub async fn set_webapp_doc(&self, id: String, key: String, value: String) -> Result<(), JsValue> {
    let id = parse_id("webapp id", &id)?;
    self
      .session
      .gateway
      .webapp()
      .doc_set(WebappDocSet { id, key, value })
      .await
      .map_err(failure)?;
    Ok(())
  }

  #[wasm_bindgen(js_name = deleteWebappDoc)]
  pub async fn delete_webapp_doc(&self, id: String, key: String) -> Result<(), JsValue> {
    let id = parse_id("webapp id", &id)?;
    self
      .session
      .gateway
      .webapp()
      .doc_delete(WebappDocDelete { id, key })
      .await
      .map_err(failure)?;
    Ok(())
  }

  #[wasm_bindgen(js_name = createBlobHandle)]
  pub fn create_blob_handle(&self, mime: String) -> Result<String, JsValue> {
    self.session.handles.create(mime).map_err(blob_failure)
  }

  #[wasm_bindgen(js_name = appendBlobHandle)]
  pub fn append_blob_handle(&self, handle: String, base64: String) -> Result<f64, JsValue> {
    self
      .session
      .handles
      .append_base64(&handle, &base64)
      .map(|size| size as f64)
      .map_err(blob_failure)
  }

  #[wasm_bindgen(js_name = putWebappBlob, unchecked_return_type = "WebappBlobEntry")]
  pub async fn put_webapp_blob(
    &self,
    id: String,
    key: String,
    handle: String,
    #[wasm_bindgen(unchecked_param_type = "((sent: number, total: number) => void) | null | undefined")]
    on_progress: Option<js_sys::Function>,
  ) -> Result<JsValue, JsValue> {
    let id = blob_webapp(&id)?;
    let progress = on_progress.map(|listener| {
      let (ticks, mut heard) = mpsc::unbounded::<(u64, u64)>();
      spawn_local(async move {
        while let Some((sent, total)) = heard.next().await {
          let _ = listener.call2(
            &JsValue::NULL,
            &JsValue::from_f64(sent as f64),
            &JsValue::from_f64(total as f64),
          );
        }
      });
      Arc::new(move |sent, total| {
        let _ = ticks.unbounded_send((sent, total));
      }) as BlobProgress
    });
    let stored = self
      .session
      .put_blob(id, key, &handle, progress)
      .await
      .map_err(blob_failure)?;
    blob_js(&stored)
  }

  #[wasm_bindgen(js_name = deleteWebappBlob)]
  pub async fn delete_webapp_blob(&self, id: String, key: String) -> Result<(), JsValue> {
    let id = blob_webapp(&id)?;
    self.session.delete_blob(id, key).await.map_err(blob_failure)
  }

  #[wasm_bindgen(js_name = webappBlobs, unchecked_return_type = "WebappBlobEntry[]")]
  pub async fn webapp_blobs(&self, id: String) -> Result<JsValue, JsValue> {
    let id = blob_webapp(&id)?;
    blob_js(&self.session.list_blobs(id).await.map_err(blob_failure)?)
  }

  #[wasm_bindgen(js_name = nextDocChange, unchecked_return_type = "WebappDocChange | null")]
  pub async fn next_doc_change(&self) -> Result<JsValue, JsValue> {
    let mut docs = self.docs.lock().await;
    let closed = self.session.closed();
    futures::pin_mut!(closed);
    loop {
      let heard = {
        let next = docs.recv();
        futures::pin_mut!(next);
        match futures::future::select(next, closed.as_mut()).await {
          futures::future::Either::Left((heard, _)) => heard,
          futures::future::Either::Right(((), _)) => return Ok(JsValue::NULL),
        }
      };
      match heard {
        Ok(change) => return to_js(&DocChange::from(change)),
        Err(broadcast::error::RecvError::Lagged(dropped)) => {
          web_sys::console::warn_1(&JsValue::from_str(&format!("{dropped} document changes were dropped")));
        }
        Err(broadcast::error::RecvError::Closed) => return Ok(JsValue::NULL),
      }
    }
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
  fn hold(session: DeliverySession, fetch: Arc<dyn ArtifactFetch>) -> Self {
    let events = session.ota.events();
    let docs = session.doc_changes();
    let blobs = Arc::new(MemoryBlobStore::new());
    let resources = WebappResourceService::new(
      blobs.clone() as Arc<dyn BlobStore>,
      Arc::new(MemorySlotIndex::new()),
      session.receiver().clone(),
    )
    .with_fetch(fetch);
    WasmSession {
      session: Arc::new(session),
      events: Arc::new(Mutex::new(events)),
      docs: Arc::new(Mutex::new(docs)),
      blobs,
      resources: Arc::new(resources),
    }
  }
}

#[wasm_bindgen(js_name = discoverManifest, unchecked_return_type = "OtaDiscoverManifest")]
pub async fn discover_manifest(root_url: String) -> Result<JsValue, JsValue> {
  let manifest = fetch_discover_manifest(fetch().as_ref(), &root_url)
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

fn fetch() -> Arc<dyn ArtifactFetch> {
  Arc::new(HttpArtifactFetch::new(HttpExecutor::new(Arc::new(
    FetchTransport::new(),
  ))))
}

fn deps(device_id: Option<String>, fetch: Arc<dyn ArtifactFetch>) -> SessionDeps {
  let device_id = device_id.unwrap_or_else(|| DEFAULT_DEVICE_ID.to_owned());
  SessionDeps {
    clock: Arc::new(SystemClock),
    fetch,
    cache_dir: std::path::PathBuf::new(),
    data_dir: None,
    info: gateway_info(&device_id, "browser", env!("CARGO_PKG_VERSION")),
    device_id,
  }
}

fn parse_id(what: &str, id: &str) -> Result<Uuid, JsValue> {
  Uuid::parse_str(id).map_err(|e| JsValue::from_str(&format!("{what}: {e}")))
}

fn blob_failure(error: impl std::fmt::Display) -> JsValue {
  JsValue::from_str(&error.to_string())
}

fn blob_webapp(id: &str) -> Result<Uuid, JsValue> {
  Uuid::parse_str(id).map_err(|e| JsValue::from_str(&format!("invalid: {id} is not a webapp id: {e}")))
}

fn blob_js<T: serde::Serialize>(value: &T) -> Result<JsValue, JsValue> {
  to_js(value).map_err(|e| JsValue::from_str(&format!("protocol: {}", e.as_string().unwrap_or_default())))
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
