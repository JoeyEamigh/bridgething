pub mod lyrics;
pub mod nlu;
pub mod ota;
pub mod session;

use std::sync::Arc;

use bridgething_delivery::bundle::fetch::{ArtifactFetch, DownloadRequest, HttpArtifactFetch, sha256_hex};
use bridgething_io::HttpExecutor;
pub use session::*;

use crate::{
  api::ota::{ArtifactDigest, OtaAvailable, OtaPollStatus, OtaRun, OtaRunProgress},
  backend::{
    AppleMusicBackend, AudioBackend, ConnectivityMonitor, DeviceWaker, ExtensionHost, ForeignHttp, ForeignWs,
    GeoProvider, HostEnvironment, HttpTransport, ImageScaler, LinkDevice, LinkTransport, LogInbox, LogLevel, LogSink,
    MediaSessionBackend, ModelArtifactValidator, NluModelRunner, NotificationBackend, PhoneBackend, SecretStore,
    SpeechRecognizer, TransferPolicy, VolumeMonitor, WsTransport,
  },
  provider::ResumeTarget,
  session::Session,
};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct CompanionConfig {
  pub host: HostInfo,
  pub capabilities: CapabilityFlags,
  pub state_dir: String,
  pub cache_dir: String,
  #[uniffi(default = None)]
  pub model_platform: Option<ModelPlatform>,
  #[uniffi(default = None)]
  pub spotify: Option<SpotifyProviderConfig>,
}

#[derive(Clone, uniffi::Record)]
pub struct CompanionBackends {
  pub link: Option<Arc<dyn LinkTransport>>,
  pub host: Arc<dyn HostEnvironment>,
  pub http: Arc<dyn HttpTransport>,
  pub ws: Arc<dyn WsTransport>,
  pub secrets: Arc<dyn SecretStore>,
  pub log: Arc<dyn LogSink>,
  #[uniffi(default = None)]
  pub audio: Option<Arc<dyn AudioBackend>>,
  #[uniffi(default = None)]
  pub volume: Option<Arc<dyn VolumeMonitor>>,
  #[uniffi(default = None)]
  pub geo: Option<Arc<dyn GeoProvider>>,
  #[uniffi(default = None)]
  pub notifications: Option<Arc<dyn NotificationBackend>>,
  #[uniffi(default = None)]
  pub phone: Option<Arc<dyn PhoneBackend>>,
  #[uniffi(default = None)]
  pub media_sessions: Option<Arc<dyn MediaSessionBackend>>,
  #[uniffi(default = None)]
  pub speech: Option<Arc<dyn SpeechRecognizer>>,
  #[uniffi(default = None)]
  pub nlu: Option<Arc<dyn NluModelRunner>>,
  #[uniffi(default = None)]
  pub apple_music: Option<Arc<dyn AppleMusicBackend>>,
  #[uniffi(default = None)]
  pub image: Option<Arc<dyn ImageScaler>>,
  #[uniffi(default = None)]
  pub model_validator: Option<Arc<dyn ModelArtifactValidator>>,
  #[uniffi(default = None)]
  pub transfer_policy: Option<Arc<dyn TransferPolicy>>,
  #[uniffi(default = None)]
  pub connectivity: Option<Arc<dyn ConnectivityMonitor>>,
  #[uniffi(default = None)]
  pub device_waker: Option<Arc<dyn DeviceWaker>>,
  #[uniffi(default = None)]
  pub extensions: Option<Arc<dyn ExtensionHost>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum LogOrigin {
  Device,
  Host,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct DeviceLogLine {
  pub seq: u64,
  pub ts_unix_ms: u64,
  pub origin: LogOrigin,
  pub level: LogLevel,
  pub target: String,
  pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct ProviderTokens {
  pub access_token: String,
  pub refresh_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub struct SpotifyProviderConfig {
  pub worker_base: String,
  pub psk: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, export_to = "companion.ts")]
pub enum SessionEvent {
  ProvidersChanged {
    providers: Vec<ProviderInfo>,
  },
  PeerConnected {
    peer: SessionPeer,
  },
  PeerDisconnected {
    device_id: String,
  },
  PeerLinkFailed {
    peer: SessionPeer,
  },
  NowPlayingChanged {
    now_playing: Option<NowPlaying>,
  },
  AncsAuthStatusChanged {
    device_id: String,
    status: AncsAuthStatus,
  },
  Log {
    origin: LogOrigin,
    level: LogLevel,
    target: String,
    message: String,
  },
  WebappsChanged {
    entry: DeviceWebappsEntry,
  },
  WebappDocChanged {
    device_id: String,
    webapp_id: String,
    key: String,
    value: Option<String>,
  },
  DeviceMetaChanged {
    device_id: String,
    meta: DeviceMeta,
  },
  VoiceModelStateChanged {
    state: VoiceModelState,
  },
  VoiceTurnChanged {
    turn: VoiceTurn,
  },
  OtaRunChanged {
    run: OtaRun,
  },
  OtaAvailableChanged {
    available: OtaAvailable,
  },
  OtaPollChanged {
    status: OtaPollStatus,
  },
  CompanionUpdateProgress {
    received: u64,
    total: u64,
  },
  Resumed,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CompanionError {
  #[error("cancelled")]
  Cancelled,
  #[error("runtime: {0}")]
  Runtime(String),
  #[error("no link to this device")]
  NotConnected,
  #[error("{0}")]
  Device(String),
  #[error("resource not available")]
  ResourceNotAvailable,
}

#[uniffi::export(with_foreign)]
pub trait SessionEventSink: Send + Sync {
  fn on_event(&self, event: SessionEvent);
}

#[uniffi::export(with_foreign)]
pub trait WebappBundleSink: Send + Sync {
  fn installed(&self, bundle: String);
}

#[derive(uniffi::Object)]
pub struct CompanionSession {
  session: Arc<Session>,
  log_inbox: Arc<LogInbox>,
}

#[uniffi::export(async_runtime = "tokio")]
impl CompanionSession {
  #[uniffi::constructor]
  pub fn create(config: CompanionConfig, backends: CompanionBackends, events: Arc<dyn SessionEventSink>) -> Arc<Self> {
    let fetch: Arc<dyn ArtifactFetch> = Arc::new(HttpArtifactFetch::new(HttpExecutor::new(Arc::new(
      ForeignHttp::new(backends.http.clone()),
    ))));
    let session = Session::new(config, backends, events.clone(), fetch);
    let log_inbox = Arc::new(LogInbox::new(session.log_ring().clone(), events));
    Arc::new(Self { session, log_inbox })
  }

  pub fn log_inbox(&self) -> Arc<LogInbox> {
    self.log_inbox.clone()
  }

  pub async fn start(&self) -> Result<(), CompanionError> {
    self.session.start();
    Ok(())
  }

  pub async fn stop(&self) {
    self.session.stop().await;
  }

  pub async fn connect_network(&self, url: String, device: LinkDevice) -> Result<(), CompanionError> {
    let connector = bridgething_gateway::connect_seam_ws(Arc::new(ForeignWs::new(self.session.ws())), &url)
      .await
      .map_err(|failure| CompanionError::Device(format!("{url}: {failure}")))?;
    self.session.connect_direct(device, connector).await;
    Ok(())
  }

  pub async fn disconnect_network(&self, device_id: String) {
    self.session.direct_disconnected(&device_id).await;
  }

  pub async fn snapshot(&self) -> SessionSnapshot {
    self.session.snapshot().await
  }

  pub async fn time_changed(&self) {
    self.session.time_changed().await;
  }

  pub async fn resumed(&self) {
    self.session.hub().resumed().await;
    self.session.ensure_voice_models();
    self.session.observer().emit(SessionEvent::Resumed);
  }

  pub async fn set_provider_priority(&self, ids: Vec<String>) {
    self.session.set_provider_priority(ids).await;
  }

  pub async fn available_providers(&self) -> Vec<ProviderInfo> {
    self.session.provider_infos()
  }

  pub async fn connect_provider(&self, id: String) -> Result<(), CompanionError> {
    self.session.connect_provider(&id).await
  }

  pub async fn disconnect_provider(&self, id: String) {
    self.session.disconnect_provider(&id).await;
  }

  pub async fn cancel_auth(&self, id: String) {
    self.session.cancel_auth(&id).await;
  }

  pub async fn complete_provider_auth(&self, id: String, tokens: ProviderTokens) -> Result<(), CompanionError> {
    self.session.complete_provider_auth(&id, tokens).await
  }

  pub fn device_log_snapshot(&self, limit: u32) -> Vec<DeviceLogLine> {
    self
      .session
      .log_ring()
      .tail(limit as usize)
      .into_iter()
      .map(|record| DeviceLogLine {
        seq: record.seq,
        ts_unix_ms: record.ts_unix_ms,
        origin: match record.origin {
          bridgething_delivery::log::LogOrigin::Device => LogOrigin::Device,
          bridgething_delivery::log::LogOrigin::Host => LogOrigin::Host,
        },
        level: crate::backend::log::api_level(record.level),
        target: record.target,
        message: record.message,
      })
      .collect()
  }

  pub fn voice_model_paths(&self) -> VoiceModelPaths {
    self.session.voice_model_paths()
  }

  pub async fn download_voice_model(&self) {
    self.session.download_voice_models();
  }

  pub fn companion_debug(&self) -> CompanionDebug {
    self.session.debug()
  }
}

#[uniffi::export(async_runtime = "tokio")]
impl CompanionSession {
  pub async fn set_capability_flags(&self, flags: CapabilityFlags) {
    self.session.set_capability_flags(flags).await;
  }

  pub async fn set_device_auto_resume(&self, device_id: String, enabled: bool) {
    self.session.set_device_auto_resume(&device_id, enabled);
  }

  pub fn default_resume_target(&self) -> ResumeTarget {
    self.session.default_resume_target()
  }

  pub async fn set_device_resume_target(&self, device_id: String, target: ResumeTarget) {
    self.session.set_device_resume_target(&device_id, target);
  }

  pub async fn set_device_log_streaming(&self, enabled: bool) {
    self.session.set_device_log_streaming(enabled).await;
  }

  pub async fn device_set_nickname(&self, device_id: String, nickname: String) -> Result<(), CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    gateway
      .system()
      .device_set_nickname(libbridgething::gateway::DeviceSetNickname { nickname })
      .await
      .map(|_| ())
      .map_err(device_error)
  }

  pub async fn list_webapps(&self, device_id: String) -> Result<Vec<WebappInfo>, CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let list = gateway.webapp().list().await.map_err(device_error)?;
    Ok(list.webapps.into_iter().map(crate::session::observer::webapp).collect())
  }

  pub async fn current_webapp(&self, device_id: String) -> Result<Option<ActiveWebapp>, CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let active = gateway.webapp().get_active().await.map_err(device_error)?;
    Ok(active.id.map(|id| ActiveWebapp {
      id: id.to_string(),
      name: active.name,
    }))
  }

  pub async fn install_webapp(
    &self,
    device_id: String,
    archive_path: String,
    provenance: Option<String>,
  ) -> Result<WebappInfo, CompanionError> {
    self.gateway_checked(&device_id)?;
    let source = std::sync::Arc::new(bridgething_delivery::ota::stream::FileSource::open(
      std::path::Path::new(&archive_path),
    ));
    let mut probe = [0u8; 1];
    bridgething_delivery::transfer::FragmentSource::read_at(source.as_ref(), 0, &mut probe)
      .map_err(|reason| CompanionError::Device(format!("{archive_path}: {reason}")))?;
    match self
      .session
      .ota()
      .install_webapp(&device_id, source, provenance.as_deref())
      .await
    {
      bridgething_delivery::ota::service::WebappInstallResult::Installed(info) => {
        self.announce_webapps(&device_id).await;
        Ok(crate::session::observer::webapp(*info))
      }
      bridgething_delivery::ota::service::WebappInstallResult::Failed { reason } => Err(CompanionError::Device(reason)),
    }
  }

  #[uniffi::method(default(sink = None))]
  pub async fn install_webapp_from_url(
    &self,
    device_id: String,
    url: String,
    expected: Option<ArtifactDigest>,
    provenance: Option<String>,
    sink: Option<Arc<dyn WebappBundleSink>>,
  ) -> Result<WebappInfo, CompanionError> {
    self.gateway_checked(&device_id)?;
    let path = self
      .session
      .fetch()
      .download(DownloadRequest {
        filename: format!("{}.zip", sha256_hex(url.as_bytes())),
        dir: self.session.cache_dir().join("webapp-install"),
        asset: url.clone(),
        url,
        expected: expected.map(|digest| bridgething_delivery::bundle::ArtifactDigest {
          size: digest.size,
          sha256: digest.sha256.to_lowercase(),
        }),
        progress: None,
      })
      .await
      .map_err(|failure| CompanionError::Device(failure.to_string()))?;
    let installed = self
      .install_webapp(device_id, path.display().to_string(), provenance)
      .await;
    if let (Ok(_), Some(sink)) = (&installed, sink) {
      let bundle = path.display().to_string();
      let _ = tokio::task::spawn_blocking(move || sink.installed(bundle)).await;
    }
    let _ = std::fs::remove_file(&path);
    installed
  }

  pub async fn download_companion_update(
    &self,
    url: String,
    filename: String,
    expected: ArtifactDigest,
  ) -> Result<String, CompanionError> {
    let observer = self.session.observer().clone();
    let declared = expected.size;
    let reported = Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX));
    let path = self
      .session
      .fetch()
      .download(DownloadRequest {
        filename,
        dir: self.session.companion_update_dir(),
        asset: "companion update".into(),
        url,
        expected: Some(bridgething_delivery::bundle::ArtifactDigest {
          size: expected.size,
          sha256: expected.sha256.to_lowercase(),
        }),
        progress: Some(Arc::new(move |received, seen| {
          let total = if declared > 0 { declared } else { seen };
          let step = if total > 0 { (total / 100).max(1) } else { 512 * 1024 };
          let bucket = if received == total {
            u64::MAX - 1
          } else {
            received / step
          };
          if reported.swap(bucket, std::sync::atomic::Ordering::Relaxed) == bucket {
            return;
          }
          observer.emit(SessionEvent::CompanionUpdateProgress { received, total });
        })),
      })
      .await
      .map_err(|failure| CompanionError::Device(failure.to_string()))?;
    Ok(path.display().to_string())
  }

  pub async fn uninstall_webapp(&self, device_id: String, id: String) -> Result<(), CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    gateway
      .webapp()
      .uninstall(libbridgething::gateway::WebappUninstall { id })
      .await
      .map_err(device_error)?;
    self.announce_webapps(&device_id).await;
    Ok(())
  }

  pub async fn switch_webapp(&self, device_id: String, id: String) -> Result<(), CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    gateway
      .webapp()
      .switch_to(libbridgething::gateway::WebappSwitchTo { id })
      .await
      .map_err(device_error)?;
    self.announce_webapps(&device_id).await;
    Ok(())
  }

  pub async fn webapp_slots(&self, device_id: String) -> Result<WebappSlots, CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    gateway
      .webapp()
      .get_slots()
      .await
      .map(project_slots)
      .map_err(device_error)
  }

  pub async fn set_webapp_slot(
    &self,
    device_id: String,
    slot: WebappSlot,
    id: Option<String>,
  ) -> Result<WebappSlots, CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = id.as_deref().map(webapp_id).transpose()?;
    let slot = match slot {
      WebappSlot::Launcher => libbridgething::gateway::WebappSlot::Launcher,
      WebappSlot::Overlay => libbridgething::gateway::WebappSlot::Overlay,
    };
    let slots = gateway
      .webapp()
      .set_slot(libbridgething::gateway::WebappSetSlot { slot, id })
      .await
      .map(project_slots)
      .map_err(device_error)?;
    self.announce_webapps(&device_id).await;
    Ok(slots)
  }

  pub async fn webapp_resource(
    &self,
    device_id: String,
    id: String,
    kind: WebappResourceKind,
  ) -> Result<WebappResourceFile, CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    let resources = self
      .session
      .resources_for(&device_id)
      .ok_or(CompanionError::NotConnected)?;
    let kind = match kind {
      WebappResourceKind::Icon => libbridgething::gateway::WebappResourceKind::Icon,
      WebappResourceKind::Settings => libbridgething::gateway::WebappResourceKind::Settings,
      WebappResourceKind::Overlay => libbridgething::gateway::WebappResourceKind::Overlay,
    };
    let cached = resources.fetch(&gateway, id, kind).await.map_err(|error| match error {
      bridgething_delivery::webapp::WebappResourceError::Domain(
        libbridgething::WebappError::ResourceNotAvailable { .. },
      ) => CompanionError::ResourceNotAvailable,
      other => CompanionError::Device(format!("{other:?}")),
    })?;
    let path = self
      .session
      .blobs()
      .path_of(&cached.digest)
      .ok_or_else(|| CompanionError::Device(format!("the resource store lost {}", cached.digest)))?;
    Ok(WebappResourceFile {
      path: path.display().to_string(),
      mime: cached.mime,
    })
  }

  pub async fn list_webapp_config(&self, device_id: String, id: String) -> Result<Vec<ConfigEntry>, CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    let reply = gateway
      .webapp()
      .config_list(libbridgething::gateway::WebappConfigList { id })
      .await
      .map_err(device_error)?;
    Ok(
      reply
        .entries
        .into_iter()
        .map(|entry| ConfigEntry {
          key: entry.key,
          value: entry.value,
        })
        .collect(),
    )
  }

  pub async fn set_webapp_config_field(
    &self,
    device_id: String,
    id: String,
    key: String,
    value: String,
  ) -> Result<(), CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    gateway
      .webapp()
      .config_set(libbridgething::gateway::WebappConfigSet { id, key, value })
      .await
      .map_err(device_error)?;
    Ok(())
  }

  pub async fn delete_webapp_config_field(
    &self,
    device_id: String,
    id: String,
    key: String,
  ) -> Result<(), CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    gateway
      .webapp()
      .config_delete(libbridgething::gateway::WebappConfigDelete { id, key })
      .await
      .map_err(device_error)?;
    Ok(())
  }

  pub async fn get_webapp_doc(
    &self,
    device_id: String,
    id: String,
    key: String,
  ) -> Result<Option<String>, CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    gateway
      .webapp()
      .doc_get(libbridgething::gateway::WebappDocGet { id, key })
      .await
      .map(|reply| reply.value)
      .map_err(device_error)
  }

  pub async fn list_webapp_doc(&self, device_id: String, id: String) -> Result<Vec<DocEntry>, CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    let reply = gateway
      .webapp()
      .doc_list(libbridgething::gateway::WebappDocList { id })
      .await
      .map_err(device_error)?;
    Ok(
      reply
        .entries
        .into_iter()
        .map(|entry| DocEntry {
          key: entry.key,
          value: entry.value,
        })
        .collect(),
    )
  }

  pub async fn set_webapp_doc(
    &self,
    device_id: String,
    id: String,
    key: String,
    value: String,
  ) -> Result<(), CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    gateway
      .webapp()
      .doc_set(libbridgething::gateway::WebappDocSet { id, key, value })
      .await
      .map(|_| ())
      .map_err(device_error)
  }

  pub async fn delete_webapp_doc(&self, device_id: String, id: String, key: String) -> Result<(), CompanionError> {
    let gateway = self.gateway_checked(&device_id)?;
    let id = webapp_id(&id)?;
    gateway
      .webapp()
      .doc_delete(libbridgething::gateway::WebappDocDelete { id, key })
      .await
      .map(|_| ())
      .map_err(device_error)
  }

  pub async fn set_ota_poll_config(&self, config: Option<OtaPollConfig>) {
    self.session.set_ota_poll_config(config).await;
  }

  pub async fn check_for_ota_update(&self, root_url: String) {
    self.session.ota().check_now(&root_url).await;
  }

  pub async fn fetch_ota_manifest(&self, root_url: String) -> Result<ota::OtaDiscoverManifest, CompanionError> {
    self
      .session
      .ota()
      .discover_manifest(&root_url)
      .await
      .map(Into::into)
      .map_err(|error| CompanionError::Device(error.to_string()))
  }

  pub async fn apply_ota_update(&self, device_id: String, channel: String, version: String, root_url: String) {
    self
      .session
      .ota()
      .apply_version(&device_id, &channel, &version, &root_url)
      .await;
  }

  pub async fn dismiss_ota_run(&self, device_id: String) {
    self.session.ota().dismiss_run(&device_id).await;
  }

  pub fn ota_run_progress(&self, device_id: String, now_ms: u64) -> Option<OtaRunProgress> {
    self
      .session
      .ota()
      .run_progress(&device_id, now_ms)
      .map(|progress| OtaRunProgress {
        percent: progress.percent,
        step_index: progress.step_index as u32,
        step_count: progress.step_count as u32,
        step_label: progress.step_label,
        eta_seconds: progress.eta_seconds,
      })
  }
}

impl CompanionSession {
  fn gateway_checked(&self, device_id: &str) -> Result<bridgething_gateway::Gateway, CompanionError> {
    self.session.gateway_for(device_id).ok_or(CompanionError::NotConnected)
  }

  async fn announce_webapps(&self, device_id: &str) {
    let Ok(gateway) = self.gateway_checked(device_id) else {
      return;
    };
    let Ok(list) = gateway.webapp().list().await else {
      return;
    };
    let active = gateway.webapp().get_active().await.ok().and_then(|active| {
      active.id.map(|id| ActiveWebapp {
        id: id.to_string(),
        name: active.name,
      })
    });
    self.session.observer().webapps_listed(
      device_id,
      list.webapps.into_iter().map(crate::session::observer::webapp).collect(),
      active,
    );
  }
}

fn device_error<E: std::fmt::Debug>(failure: bridgething_sdk_runtime::RequestFailure<E>) -> CompanionError {
  CompanionError::Device(format!("{failure:?}"))
}

fn webapp_id(raw: &str) -> Result<uuid::Uuid, CompanionError> {
  uuid::Uuid::parse_str(raw).map_err(|_| CompanionError::Device(format!("not a webapp id: {raw}")))
}

fn project_slots(slots: libbridgething::gateway::WebappSlots) -> WebappSlots {
  WebappSlots {
    launcher: slots.launcher.map(|id| id.to_string()),
    overlay: slots.overlay.map(|id| id.to_string()),
  }
}

impl CompanionSession {
  pub fn session(&self) -> &Arc<Session> {
    &self.session
  }

  pub fn ota(&self) -> &Arc<bridgething_delivery::ota::service::OtaService> {
    self.session.ota()
  }

  pub async fn connect_direct<C>(&self, device: crate::backend::LinkDevice, connector: C)
  where
    C: bridgething_sdk_runtime::Connector<crate::session::GatewayProtocol> + Send + 'static,
  {
    self.session.connect_direct(device, connector).await;
  }

  pub async fn disconnect_direct(&self, device_id: &str) {
    self.session.direct_disconnected(device_id).await;
  }

  pub async fn add_provider(
    &self,
    provider: Arc<dyn crate::provider::Provider>,
  ) -> Result<(), crate::provider::ProviderError> {
    self.session.add_provider(provider).await
  }

  pub fn gateway_for(&self, device_id: &str) -> Option<bridgething_gateway::Gateway> {
    self.session.gateway_for(device_id)
  }
}
