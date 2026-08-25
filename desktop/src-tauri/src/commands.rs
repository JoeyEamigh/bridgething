use std::{path::PathBuf, sync::Arc};

use bridgething_companion::{
  api::{
    ActiveWebapp, CapabilityFlags, CompanionError, ConfigEntry, DeviceLogLine, DeviceMetaEntry, DocEntry, NowPlaying,
    OtaPollConfig, ProviderInfo, ProviderTokens, SessionHostInfo, SessionPeer, SessionSnapshot, VoiceModelState,
    WebappInfo, WebappSlot, WebappSlots,
    ota::{ArtifactDigest, OtaAvailable, OtaDiscoverManifest, OtaPollStatus, OtaRun},
  },
  provider::ResumeTarget,
};
use bridgething_delivery::{
  discovery::{Discovery, Endpoint},
  ota::{event::OtaPhaseSnapshot, service::WebappInstallResult, stream::FileSource},
  seam::BlobStore,
  transfer::FragmentSource,
};
use libbridgething::gateway::WebappResourceKind;
use serde::Serialize;
use tauri::{AppHandle, Runtime, State};
use uuid::Uuid;

use crate::{
  hints::{self, Hint},
  known_device::KnownDevice,
  logs::Verbosity,
  route::Route,
  shell::{Shell, ShellError},
  sources::Sources,
};

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum CommandError {
  #[error("no link to a daemon")]
  NotConnected,
  #[error("{0}")]
  Link(String),
  #[error("{0}")]
  Device(String),
  #[error("{0}")]
  Artifact(String),
  #[error("{0}")]
  Host(String),
}

impl From<ShellError> for CommandError {
  fn from(error: ShellError) -> Self {
    match error {
      ShellError::NotConnected => Self::NotConnected,
      other => Self::Link(other.to_string()),
    }
  }
}

impl From<CompanionError> for CommandError {
  fn from(error: CompanionError) -> Self {
    match error {
      CompanionError::NotConnected => Self::NotConnected,
      CompanionError::Cancelled => Self::Device("cancelled".to_owned()),
      CompanionError::ResourceNotAvailable => Self::Artifact("resource not available".to_owned()),
      CompanionError::Runtime(reason) => Self::Link(reason),
      CompanionError::Device(reason) => Self::Device(reason),
    }
  }
}

type Answer<T> = Result<T, CommandError>;

fn webapp_id(raw: &str) -> Answer<Uuid> {
  Uuid::parse_str(raw).map_err(|_| CommandError::Device(format!("not a webapp id: {raw}")))
}

fn peer(shell: &Shell) -> Answer<String> {
  shell.peer().ok_or(CommandError::NotConnected)
}

// MARK: pulls

#[tauri::command]
pub async fn session_snapshot(shell: State<'_, Arc<Shell>>) -> Answer<SessionSnapshot> {
  Ok(shell.session().snapshot().await)
}

#[tauri::command]
pub async fn host_info(shell: State<'_, Arc<Shell>>) -> Answer<SessionHostInfo> {
  Ok(shell.session().snapshot().await.host_info)
}

#[tauri::command]
pub async fn capabilities(shell: State<'_, Arc<Shell>>) -> Answer<CapabilityFlags> {
  Ok(shell.session().snapshot().await.capability_flags)
}

#[tauri::command]
pub async fn capability_support(shell: State<'_, Arc<Shell>>) -> Answer<CapabilityFlags> {
  Ok(shell.capability_support())
}

#[tauri::command]
pub async fn providers(shell: State<'_, Arc<Shell>>) -> Answer<Vec<ProviderInfo>> {
  Ok(shell.session().snapshot().await.providers)
}

#[tauri::command]
pub async fn provider_priority(shell: State<'_, Arc<Shell>>) -> Answer<Vec<String>> {
  Ok(shell.session().snapshot().await.provider_priority)
}

#[tauri::command]
pub async fn library_provider(shell: State<'_, Arc<Shell>>) -> Answer<Option<String>> {
  Ok(shell.session().snapshot().await.library_provider)
}

#[tauri::command]
pub async fn peers(shell: State<'_, Arc<Shell>>) -> Answer<Vec<SessionPeer>> {
  Ok(shell.session().snapshot().await.peers)
}

#[tauri::command]
pub async fn now_playing(shell: State<'_, Arc<Shell>>) -> Answer<Option<NowPlaying>> {
  Ok(shell.session().snapshot().await.now_playing)
}

#[tauri::command]
pub async fn device_meta(shell: State<'_, Arc<Shell>>) -> Answer<Vec<DeviceMetaEntry>> {
  Ok(shell.session().snapshot().await.device_meta)
}

#[tauri::command]
pub async fn device_auto_resume(shell: State<'_, Arc<Shell>>) -> Answer<bool> {
  let Some(device_id) = shell.peer() else {
    return Ok(true);
  };
  Ok(
    shell
      .session()
      .companion_debug()
      .auto_resume
      .into_iter()
      .find(|pref| pref.device_id == device_id)
      .map(|pref| pref.enabled)
      .unwrap_or(true),
  )
}

#[tauri::command]
pub async fn device_resume_target(shell: State<'_, Arc<Shell>>) -> Answer<ResumeTarget> {
  let unset = shell.session().default_resume_target();
  let Some(device_id) = shell.peer() else {
    return Ok(unset);
  };
  Ok(
    shell
      .session()
      .companion_debug()
      .resume_targets
      .into_iter()
      .find(|pref| pref.device_id == device_id)
      .map(|pref| pref.target)
      .unwrap_or(unset),
  )
}

#[tauri::command]
pub async fn device_log_streaming(shell: State<'_, Arc<Shell>>) -> Answer<bool> {
  Ok(shell.log_streaming())
}

#[tauri::command]
pub async fn debug_logging(verbosity: State<'_, Arc<Verbosity>>) -> Answer<bool> {
  Ok(verbosity.get())
}

#[tauri::command]
pub async fn voice_model(shell: State<'_, Arc<Shell>>) -> Answer<VoiceModelState> {
  Ok(shell.session().snapshot().await.voice_model)
}

#[tauri::command]
pub async fn ota_runs(shell: State<'_, Arc<Shell>>) -> Answer<Vec<OtaRun>> {
  Ok(shell.session().snapshot().await.ota_runs)
}

#[tauri::command]
pub async fn ota_available(shell: State<'_, Arc<Shell>>) -> Answer<Vec<OtaAvailable>> {
  Ok(shell.session().snapshot().await.ota_available)
}

#[tauri::command]
pub async fn ota_poll(shell: State<'_, Arc<Shell>>) -> Answer<OtaPollStatus> {
  Ok(shell.session().snapshot().await.ota_poll)
}

#[tauri::command]
pub async fn webapps(shell: State<'_, Arc<Shell>>) -> Answer<Vec<WebappInfo>> {
  Ok(shell.session().list_webapps(peer(&shell)?).await?)
}

#[tauri::command]
pub async fn webapp_active(shell: State<'_, Arc<Shell>>) -> Answer<Option<ActiveWebapp>> {
  Ok(shell.session().current_webapp(peer(&shell)?).await?)
}

#[tauri::command]
pub async fn webapp_slots(shell: State<'_, Arc<Shell>>) -> Answer<WebappSlots> {
  Ok(shell.session().webapp_slots(peer(&shell)?).await?)
}

#[tauri::command]
pub async fn webapp_config(shell: State<'_, Arc<Shell>>, id: String) -> Answer<Vec<ConfigEntry>> {
  Ok(shell.session().list_webapp_config(peer(&shell)?, id).await?)
}

#[tauri::command]
pub async fn webapp_doc(shell: State<'_, Arc<Shell>>, id: String) -> Answer<Vec<DocEntry>> {
  Ok(shell.session().list_webapp_doc(peer(&shell)?, id).await?)
}

#[tauri::command]
pub async fn webapp_doc_entry(shell: State<'_, Arc<Shell>>, id: String, key: String) -> Answer<Option<String>> {
  Ok(shell.session().get_webapp_doc(peer(&shell)?, id, key).await?)
}

#[tauri::command]
pub async fn device_logs(shell: State<'_, Arc<Shell>>, limit: u32) -> Answer<Vec<DeviceLogLine>> {
  Ok(shell.session().device_log_snapshot(limit))
}

#[tauri::command]
pub async fn export_logs(path: PathBuf, body: String) -> Answer<()> {
  std::fs::write(&path, body).map_err(|reason| CommandError::Host(format!("{}: {reason}", path.display())))
}

#[tauri::command]
pub async fn ota_manifest(shell: State<'_, Arc<Shell>>, root_url: String) -> Answer<OtaDiscoverManifest> {
  Ok(shell.session().fetch_ota_manifest(root_url).await?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebappResource {
  pub digest: String,
  pub mime: Option<String>,
  pub bytes: Vec<u8>,
}

#[tauri::command]
pub async fn webapp_resource(
  shell: State<'_, Arc<Shell>>,
  id: String,
  kind: WebappResourceKind,
) -> Answer<WebappResource> {
  let id = webapp_id(&id)?;
  let link = shell.link()?;
  let cached = shell
    .resources()?
    .fetch(&link, id, kind)
    .await
    .map_err(|error| CommandError::Device(format!("{error:?}")))?;
  let bytes = shell
    .blobs()
    .get(&cached.digest)
    .map_err(CommandError::Artifact)?
    .ok_or_else(|| CommandError::Artifact(format!("the resource store lost {}", cached.digest)))?;
  Ok(WebappResource {
    digest: cached.digest,
    mime: cached.mime,
    bytes,
  })
}

// MARK: actions

#[tauri::command]
pub async fn endpoints(discovery: State<'_, Arc<Discovery>>) -> Answer<Vec<Endpoint>> {
  Ok(discovery.endpoints())
}

#[tauri::command]
pub async fn default_gateway(shell: State<'_, Arc<Shell>>) -> Answer<String> {
  Ok(shell.gateway_url().to_owned())
}

#[tauri::command]
pub async fn route(route: State<'_, Route>) -> Answer<String> {
  Ok(route.get())
}

#[tauri::command]
pub async fn set_route(route: State<'_, Route>, path: String) -> Answer<()> {
  route.set(path);
  Ok(())
}

#[tauri::command]
pub async fn catalog_sources(sources: State<'_, Sources>) -> Answer<Vec<String>> {
  Ok(sources.list())
}

#[tauri::command]
pub async fn add_catalog_source(sources: State<'_, Sources>, url: String) -> Answer<Vec<String>> {
  Ok(sources.add(url))
}

#[tauri::command]
pub async fn remove_catalog_source(sources: State<'_, Sources>, url: String) -> Answer<Vec<String>> {
  Ok(sources.remove(&url))
}

#[tauri::command]
pub async fn connect(shell: State<'_, Arc<Shell>>, url: Option<String>) -> Answer<String> {
  Ok(shell.connect(url).await?)
}

#[tauri::command]
pub async fn disconnect(shell: State<'_, Arc<Shell>>, device_id: Option<String>) -> Answer<()> {
  shell.disconnect(device_id).await;
  Ok(())
}

#[tauri::command]
pub async fn known_devices(shell: State<'_, Arc<Shell>>) -> Answer<Vec<KnownDevice>> {
  Ok(shell.known_devices())
}

#[tauri::command]
pub async fn forget_known_device(shell: State<'_, Arc<Shell>>, id: String) -> Answer<()> {
  shell.forget_device(&id);
  Ok(())
}

#[tauri::command]
pub async fn selected_device(shell: State<'_, Arc<Shell>>) -> Answer<Option<String>> {
  Ok(shell.peer())
}

#[tauri::command]
pub async fn select_device(shell: State<'_, Arc<Shell>>, device_id: Option<String>) -> Answer<()> {
  shell.select(device_id);
  Ok(())
}

#[tauri::command]
pub async fn set_provider_priority(shell: State<'_, Arc<Shell>>, ids: Vec<String>) -> Answer<()> {
  shell.session().set_provider_priority(ids).await;
  Ok(())
}

#[tauri::command]
pub async fn connect_provider(shell: State<'_, Arc<Shell>>, id: String) -> Answer<()> {
  Ok(shell.session().connect_provider(id).await?)
}

#[tauri::command]
pub async fn disconnect_provider(shell: State<'_, Arc<Shell>>, id: String) -> Answer<()> {
  shell.session().disconnect_provider(id).await;
  Ok(())
}

#[tauri::command]
pub async fn cancel_provider_auth(shell: State<'_, Arc<Shell>>, id: String) -> Answer<()> {
  shell.session().cancel_auth(id).await;
  Ok(())
}

#[tauri::command]
pub async fn complete_provider_auth(shell: State<'_, Arc<Shell>>, id: String, tokens: ProviderTokens) -> Answer<()> {
  Ok(shell.session().complete_provider_auth(id, tokens).await?)
}

#[tauri::command]
pub async fn set_capability_flags(shell: State<'_, Arc<Shell>>, flags: CapabilityFlags) -> Answer<()> {
  shell.set_capability_flags(flags).await;
  shell.announce(Hint::bare(hints::SESSION));
  Ok(())
}

#[tauri::command]
pub async fn set_device_auto_resume(shell: State<'_, Arc<Shell>>, enabled: bool) -> Answer<()> {
  let device_id = peer(&shell)?;
  shell.session().set_device_auto_resume(device_id.clone(), enabled).await;
  shell.announce(Hint::about(hints::DEVICE_META, device_id));
  Ok(())
}

#[tauri::command]
pub async fn set_device_resume_target(shell: State<'_, Arc<Shell>>, target: ResumeTarget) -> Answer<()> {
  let device_id = peer(&shell)?;
  shell
    .session()
    .set_device_resume_target(device_id.clone(), target)
    .await;
  shell.announce(Hint::about(hints::DEVICE_META, device_id));
  Ok(())
}

#[tauri::command]
pub async fn set_device_log_streaming(shell: State<'_, Arc<Shell>>, enabled: bool) -> Answer<()> {
  shell.set_log_streaming(enabled).await;
  shell.announce(Hint::bare(hints::LOGS));
  Ok(())
}

#[tauri::command]
pub async fn set_debug_logging(verbosity: State<'_, Arc<Verbosity>>, enabled: bool) -> Answer<()> {
  verbosity.set(enabled);
  tracing::info!(enabled, "the host log verbosity changed");
  Ok(())
}

#[tauri::command]
pub async fn set_device_nickname(shell: State<'_, Arc<Shell>>, nickname: String) -> Answer<()> {
  Ok(shell.session().device_set_nickname(peer(&shell)?, nickname).await?)
}

#[tauri::command]
pub async fn switch_webapp(shell: State<'_, Arc<Shell>>, id: String) -> Answer<()> {
  Ok(shell.session().switch_webapp(peer(&shell)?, id).await?)
}

#[tauri::command]
pub async fn uninstall_webapp(shell: State<'_, Arc<Shell>>, id: String) -> Answer<()> {
  Ok(shell.session().uninstall_webapp(peer(&shell)?, id).await?)
}

#[tauri::command]
pub async fn set_webapp_slot(
  shell: State<'_, Arc<Shell>>,
  slot: WebappSlot,
  id: Option<String>,
) -> Answer<WebappSlots> {
  Ok(shell.session().set_webapp_slot(peer(&shell)?, slot, id).await?)
}

#[tauri::command]
pub async fn set_webapp_config_field(
  shell: State<'_, Arc<Shell>>,
  id: String,
  key: String,
  value: String,
) -> Answer<()> {
  Ok(
    shell
      .session()
      .set_webapp_config_field(peer(&shell)?, id, key, value)
      .await?,
  )
}

#[tauri::command]
pub async fn delete_webapp_config_field(shell: State<'_, Arc<Shell>>, id: String, key: String) -> Answer<()> {
  Ok(
    shell
      .session()
      .delete_webapp_config_field(peer(&shell)?, id, key)
      .await?,
  )
}

#[tauri::command]
pub async fn set_webapp_doc(shell: State<'_, Arc<Shell>>, id: String, key: String, value: String) -> Answer<()> {
  Ok(shell.session().set_webapp_doc(peer(&shell)?, id, key, value).await?)
}

#[tauri::command]
pub async fn delete_webapp_doc(shell: State<'_, Arc<Shell>>, id: String, key: String) -> Answer<()> {
  Ok(shell.session().delete_webapp_doc(peer(&shell)?, id, key).await?)
}

#[tauri::command]
pub async fn set_ota_poll_config(shell: State<'_, Arc<Shell>>, config: Option<OtaPollConfig>) -> Answer<()> {
  shell.session().set_ota_poll_config(config).await;
  shell.announce(Hint::bare(hints::OTA_POLL));
  Ok(())
}

#[tauri::command]
pub async fn apply_ota_update(
  shell: State<'_, Arc<Shell>>,
  channel: String,
  version: String,
  root_url: String,
) -> Answer<()> {
  shell
    .session()
    .apply_ota_update(peer(&shell)?, channel, version, root_url)
    .await;
  Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OtaOutcome {
  Completed,
  Failed { reason: String },
  Interrupted,
}

impl From<OtaPhaseSnapshot> for OtaOutcome {
  fn from(phase: OtaPhaseSnapshot) -> Self {
    match phase {
      OtaPhaseSnapshot::Completed => Self::Completed,
      OtaPhaseSnapshot::Failed { reason } => Self::Failed { reason },
      _ => Self::Interrupted,
    }
  }
}

#[tauri::command]
pub async fn ota_push_daemon(shell: State<'_, Arc<Shell>>, artifact: PathBuf) -> Answer<OtaOutcome> {
  let device_id = peer(&shell)?;
  Ok(shell.ota().push_daemon(&device_id, spool(artifact)?, None).await.into())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InstallOutcome {
  Installed { id: String },
  Failed { reason: String },
}

#[tauri::command]
pub async fn ota_install_webapp(
  shell: State<'_, Arc<Shell>>,
  bundle: PathBuf,
  provenance: Option<String>,
) -> Answer<InstallOutcome> {
  let device_id = peer(&shell)?;
  let bundle = spool(bundle)?;
  let outcome = shell
    .ota()
    .install_webapp(&device_id, bundle, provenance.as_deref())
    .await;
  Ok(match outcome {
    WebappInstallResult::Installed(info) => InstallOutcome::Installed {
      id: info.id.to_string(),
    },
    WebappInstallResult::Failed { reason } => InstallOutcome::Failed { reason },
  })
}

#[tauri::command]
pub async fn install_webapp_from_url(
  shell: State<'_, Arc<Shell>>,
  url: String,
  expected: Option<ArtifactDigest>,
  provenance: Option<String>,
) -> Answer<WebappInfo> {
  Ok(
    shell
      .session()
      .install_webapp_from_url(peer(&shell)?, url, expected, provenance)
      .await?,
  )
}

#[tauri::command]
pub async fn ota_check_now(shell: State<'_, Arc<Shell>>, root_url: String) -> Answer<()> {
  shell.ota().check_now(&root_url).await;
  Ok(())
}

#[tauri::command]
pub async fn ota_dismiss_run(shell: State<'_, Arc<Shell>>) -> Answer<()> {
  let device_id = peer(&shell)?;
  shell.ota().dismiss_run(&device_id).await;
  Ok(())
}

#[tauri::command]
pub fn restart<R: Runtime>(app: AppHandle<R>) {
  crate::process::restart(&app)
}

#[tauri::command]
pub fn quit<R: Runtime>(app: AppHandle<R>) {
  crate::process::leave(&app)
}

fn spool(path: PathBuf) -> Answer<Arc<FileSource>> {
  let source = Arc::new(FileSource::open(&path));
  let mut probe = [0u8; 1];
  source
    .read_at(0, &mut probe)
    .map_err(|reason| CommandError::Artifact(format!("{}: {reason}", path.display())))?;
  Ok(source)
}
