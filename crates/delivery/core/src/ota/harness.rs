use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
  time::Duration,
};

use bridgething_gateway::{Gateway, HandlerError, Reply};
use bridgething_sdk_runtime::rt;
use libbridgething::{
  BridgeThingMeta, OtaErrorCode, OtaKind, OtaPhase, Priority, RangeSpec, WebappInfo,
  gateway::{
    BridgeToGatewayMsgData, BridgeToGatewaySystemMsg, BridgeToGatewayTransferMsg, BridgeToGatewayWebappMsg,
    GatewayToBridgeMsgData, GatewayToBridgeSystemMsg, GatewayToBridgeTransferMsg, GatewayToBridgeWebappMsg,
    OtaAssetRange, OtaAssetRangeRejected, OtaAssetRangeReply, OtaBegin, OtaBeginAck, OtaBeginRejected, TransferAbandon,
    TransferAck, TransferFragment, WebappList,
  },
  protocol::Compress,
  wire::{MsgMeta, ResponseMeta},
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::broadcast;
use uuid::Uuid;

pub use crate::harness::{FakeDevice, WIRE_TIMEOUT, linked_gateway, pattern};
use crate::{
  bundle::{
    ArtifactDigest,
    fetch::{ArtifactFetch, DownloadRequest, FetchError},
  },
  ota::{
    range::RangeServer,
    service::OtaService,
    stream::{Artifact, FileSource},
  },
  seam::Clock,
  transfer::AckWindow,
};

pub const DEVICE: &str = "AA:BB:CC:DD:EE:FF";
pub const OTHER_DEVICE: &str = "11:22:33:44:55:66";
pub const EPOCH_MS: u64 = 1_700_000_000_000;

pub fn sha256_hex(bytes: &[u8]) -> String {
  let mut hasher = Sha256::new();
  hasher.update(bytes);
  hasher.finalize().iter().fold(String::new(), |mut acc, byte| {
    acc.push_str(&format!("{byte:02x}"));
    acc
  })
}

pub fn digest_of(bytes: &[u8]) -> ArtifactDigest {
  ArtifactDigest {
    size: bytes.len() as u64,
    sha256: sha256_hex(bytes),
  }
}

pub struct TestClock {
  base_ms: u64,
  origin: tokio::time::Instant,
}

impl TestClock {
  pub fn new() -> Arc<Self> {
    Arc::new(Self {
      base_ms: EPOCH_MS,
      origin: tokio::time::Instant::now(),
    })
  }
}

impl Clock for TestClock {
  fn now(&self) -> rt::Instant {
    rt::Instant::now()
  }

  fn unix_millis(&self) -> u64 {
    self.base_ms + self.origin.elapsed().as_millis() as u64
  }
}

pub struct Spool {
  dir: TempDir,
}

impl Spool {
  pub fn new() -> Self {
    Self {
      dir: TempDir::new().expect("a scratch directory"),
    }
  }

  pub fn path(&self) -> &Path {
    self.dir.path()
  }

  pub fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
    let path = self.dir.path().join(name);
    std::fs::write(&path, bytes).expect("the scratch directory is writable");
    path
  }

  pub fn asset(&self, name: &str, bytes: &[u8]) -> Arc<dyn Artifact> {
    Arc::new(FileSource::open(self.write(name, bytes)))
  }
}

#[derive(Default)]
pub struct FakeFetch {
  texts: Mutex<BTreeMap<String, String>>,
  artifacts: Mutex<BTreeMap<String, Vec<u8>>>,
  urls: Mutex<Vec<String>>,
  downloads: AtomicUsize,
  failure: Mutex<Option<String>>,
}

impl FakeFetch {
  pub fn new() -> Arc<Self> {
    Arc::new(Self::default())
  }

  pub fn serve_text(&self, url: &str, body: String) {
    self.texts.lock().unwrap().insert(url.to_owned(), body);
  }

  pub fn serve_artifact(&self, url: &str, body: Vec<u8>) {
    self.artifacts.lock().unwrap().insert(url.to_owned(), body);
  }

  pub fn fail_with(&self, reason: &str) {
    *self.failure.lock().unwrap() = Some(reason.to_owned());
  }

  pub fn urls(&self) -> Vec<String> {
    self.urls.lock().unwrap().clone()
  }

  pub fn downloads(&self) -> usize {
    self.downloads.load(Ordering::SeqCst)
  }
}

#[async_trait::async_trait]
impl ArtifactFetch for FakeFetch {
  async fn text(&self, url: &str) -> Result<String, FetchError> {
    self.urls.lock().unwrap().push(url.to_owned());
    if let Some(reason) = self.failure.lock().unwrap().clone() {
      return Err(FetchError::Transport(reason));
    }
    self
      .texts
      .lock()
      .unwrap()
      .get(url)
      .cloned()
      .ok_or_else(|| FetchError::Transport(format!("nothing published at {url}")))
  }

  async fn download(&self, request: DownloadRequest) -> Result<PathBuf, FetchError> {
    self.urls.lock().unwrap().push(request.url.clone());
    self.downloads.fetch_add(1, Ordering::SeqCst);
    if let Some(reason) = self.failure.lock().unwrap().clone() {
      return Err(FetchError::Transport(reason));
    }
    let body = self
      .artifacts
      .lock()
      .unwrap()
      .get(&request.url)
      .cloned()
      .ok_or_else(|| FetchError::Transport(format!("nothing published at {}", request.url)))?;

    std::fs::create_dir_all(&request.dir).map_err(|e| FetchError::Io(e.to_string()))?;
    let dest = match &request.expected {
      Some(expected) => request.dir.join(format!("{}-{}", request.filename, expected.sha256)),
      None => request.dir.join(&request.filename),
    };
    std::fs::write(&dest, &body).map_err(|e| FetchError::Io(e.to_string()))?;
    if let Some(progress) = request.progress.clone() {
      progress(body.len() as u64, body.len() as u64);
    }
    Ok(dest)
  }
}

impl FakeDevice {
  pub fn fragment_lanes(&self) -> Vec<Priority> {
    self.lanes_of(|data| match data {
      GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Fragment(fragment)) => Some(fragment.offset),
      _ => None,
    })
  }

  pub async fn await_ota_begin(&mut self) -> (Uuid, OtaBegin) {
    self.await_ota_begin_within(WIRE_TIMEOUT).await
  }

  pub async fn await_ota_begin_within(&mut self, window: Duration) -> (Uuid, OtaBegin) {
    self
      .next_matching_within(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::System(GatewayToBridgeSystemMsg::OtaBegin(begin)) => Some((msg.id, begin.clone())),
        _ => None,
      })
      .await
  }

  pub async fn no_ota_begin(&mut self, window: Duration) -> bool {
    self
      .nothing_matching(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::System(GatewayToBridgeSystemMsg::OtaBegin(begin)) => Some(begin.kind),
        _ => None,
      })
      .await
  }

  pub fn ack_begin(&self, request_id: Uuid, resume_from_offset: u32) {
    self.respond(
      request_id,
      BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaBeginAck(OtaBeginAck {
        resume_from_offset,
      })),
    );
  }

  pub fn reject_begin(&self, request_id: Uuid, reason: &str) {
    self.respond(
      request_id,
      BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaBeginRejected(OtaBeginRejected {
        reason: reason.to_owned(),
      })),
    );
  }

  pub async fn next_fragment(&mut self, transfer_id: Uuid) -> TransferFragment {
    self
      .next_matching(|msg| match &msg.data {
        GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Fragment(fragment))
          if fragment.transfer_id == transfer_id =>
        {
          Some(fragment.clone())
        }
        _ => None,
      })
      .await
  }

  pub async fn no_fragment(&mut self, transfer_id: Uuid, window: Duration) -> bool {
    self
      .nothing_matching(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Fragment(fragment))
          if fragment.transfer_id == transfer_id =>
        {
          Some(fragment.offset)
        }
        _ => None,
      })
      .await
  }

  pub fn ack(&self, transfer_id: Uuid, received: u32) {
    self.event(BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Ack(
      TransferAck { transfer_id, received },
    )));
  }

  pub async fn await_abandon(&mut self, transfer_id: Uuid) -> TransferAbandon {
    self.await_abandon_within(WIRE_TIMEOUT, transfer_id).await
  }

  pub async fn await_abandon_within(&mut self, window: Duration, transfer_id: Uuid) -> TransferAbandon {
    self
      .next_matching_within(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Abandon(abandon))
          if abandon.transfer_id == transfer_id =>
        {
          Some(abandon.clone())
        }
        _ => None,
      })
      .await
  }

  pub async fn await_activate(&mut self) -> Vec<String> {
    self
      .next_matching(|msg| match &msg.data {
        GatewayToBridgeMsgData::System(GatewayToBridgeSystemMsg::OtaActivate(activate)) => {
          Some(activate.expected.clone())
        }
        _ => None,
      })
      .await
  }

  pub async fn no_activate(&mut self, window: Duration) -> bool {
    self
      .nothing_matching(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::System(GatewayToBridgeSystemMsg::OtaActivate(_)) => Some(()),
        _ => None,
      })
      .await
  }

  pub fn progress(&self, phase: OtaPhase, percent: u8) {
    self.progress_full(phase, percent, 0, 0);
  }

  pub fn progress_full(&self, phase: OtaPhase, percent: u8, dwl_percent: u8, dwl_bytes: u32) {
    self.event(BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaProgress(
      libbridgething::OtaProgress {
        phase,
        percent,
        step: 1,
        nsteps: 1,
        dwl_percent,
        dwl_bytes,
        eta_ms: None,
      },
    )));
  }

  pub fn ota_error(&self, code: OtaErrorCode, msg: &str) {
    self.ota_error_full(code, msg, None, false);
  }

  pub fn ota_error_replayed(&self, code: OtaErrorCode, msg: &str, update_id: &str) {
    self.ota_error_full(code, msg, Some(update_id.to_owned()), true);
  }

  fn ota_error_full(&self, code: OtaErrorCode, msg: &str, update_id: Option<String>, replayed: bool) {
    self.event(BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaError(
      libbridgething::OtaError {
        code,
        msg: msg.to_owned(),
        update_id,
        replayed,
      },
    )));
  }

  pub fn ota_finished(&self, kind: OtaKind, update_id: &str) {
    self.event(BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaFinished(
      libbridgething::OtaFinished {
        kind,
        update_id: update_id.to_owned(),
      },
    )));
  }

  pub async fn answer_webapp_list(&mut self, webapps: Vec<WebappInfo>) {
    let request_id = self
      .next_matching(|msg| match &msg.data {
        GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::List) => Some(msg.id),
        _ => None,
      })
      .await;
    self.respond(
      request_id,
      BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::Webapps(WebappList { webapps })),
    );
  }

  pub fn webapp_installed(&self, info: WebappInfo) {
    self.event(BridgeToGatewayMsgData::Webapp(
      BridgeToGatewayWebappMsg::WebappInstalled(info),
    ));
  }

  pub fn announce_meta(&self, meta: BridgeThingMeta) {
    self.event(BridgeToGatewayMsgData::Version(Box::new(meta)));
  }

  pub fn ask_range(&self, asset: &str, ranges: Vec<RangeSpec>) -> Uuid {
    self.request(BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaAssetRange(
      OtaAssetRange {
        update_id: "u1".into(),
        asset: asset.to_owned(),
        ranges,
      },
    )))
  }

  pub async fn await_range_reply(&mut self, request_id: Uuid) -> Result<OtaAssetRangeReply, String> {
    self
      .next_matching(|msg| {
        if !matches!(msg.meta, MsgMeta::Response(ResponseMeta { request_id: id }) if id == request_id) {
          return None;
        }
        match &msg.data {
          GatewayToBridgeMsgData::System(GatewayToBridgeSystemMsg::OtaAssetRangeReply(reply)) => {
            Some(Ok(reply.clone()))
          }
          GatewayToBridgeMsgData::System(GatewayToBridgeSystemMsg::OtaAssetRangeRejected(rejected)) => {
            Some(Err(rejected.reason.clone()))
          }
          _ => None,
        }
      })
      .await
  }
}

pub fn meta(app_version: &str, image_version: &str, channel: &str) -> BridgeThingMeta {
  meta_with_variant(app_version, image_version, channel, "prod", None)
}

pub fn meta_with_variant(
  app_version: &str,
  image_version: &str,
  channel: &str,
  image_variant: &str,
  daemon_sha256: Option<&str>,
) -> BridgeThingMeta {
  BridgeThingMeta {
    bridgething_version: app_version.into(),
    libbridgething_version: app_version.into(),
    app_name: "bridgething".into(),
    nickname: None,
    launcher_gesture: libbridgething::LauncherGesture::default(),
    app_version: app_version.into(),
    daemon_sha256: daemon_sha256.map(str::to_owned),
    wakeword_model_version: None,
    os_name: "linux".into(),
    os_version: "1".into(),
    os_description: String::new(),
    bt_mac: String::new(),
    serial_number: String::new(),
    fcc_id: String::new(),
    ic_id: String::new(),
    model_name: "Car Thing".into(),
    channel: channel.into(),
    image_variant: image_variant.into(),
    image_version: image_version.into(),
    image_build_id: String::new(),
    image_build_date: String::new(),
    image_distro: String::new(),
    image_machine: String::new(),
    discord: String::new(),
    credits: String::new(),
  }
}

pub struct ManifestFixture {
  pub channel: String,
  pub latest: String,
  pub yanked: Option<String>,
  pub deprecated: bool,
  pub daemon: Option<ArtifactDigest>,
  pub daemon_zst: Option<ArtifactDigest>,
  pub image_swu: Option<ArtifactDigest>,
  pub image_zck: Option<ArtifactDigest>,
  pub image_boot_zck: Option<ArtifactDigest>,
  pub wakeword_model: Option<String>,
  pub wakeword_runtime: Option<String>,
  pub wakeword_model_digest: Option<ArtifactDigest>,
  pub builtin_webapps: BTreeMap<String, String>,
}

impl ManifestFixture {
  pub fn new(channel: &str, latest: &str) -> Self {
    Self {
      channel: channel.into(),
      latest: latest.into(),
      yanked: None,
      deprecated: false,
      daemon: None,
      daemon_zst: None,
      image_swu: None,
      image_zck: None,
      image_boot_zck: None,
      wakeword_model: None,
      wakeword_runtime: None,
      wakeword_model_digest: None,
      builtin_webapps: BTreeMap::new(),
    }
  }

  pub fn json(&self) -> String {
    let digest = |entry: &Option<ArtifactDigest>| match entry {
      Some(digest) => format!(r#"{{"size":{},"sha256":"{}"}}"#, digest.size, digest.sha256),
      None => "null".into(),
    };
    let webapps = self
      .builtin_webapps
      .iter()
      .map(|(slug, version)| format!(r#""{slug}":"{version}""#))
      .collect::<Vec<_>>()
      .join(",");
    format!(
      r#"{{
        "manifest_version": 1,
        "updated_at": "2026-08-03T00:00:00Z",
        "channels": {{
          "{channel}": {{
            "name": "{channel}",
            "stability": "stable",
            "default": true,
            "latest": "{latest}",
            "releases": ["{latest}"]
          }}
        }},
        "releases": {{
          "{latest}": {{
            "version": "{latest}",
            "channel": "{channel}",
            "yanked": {yanked},
            "deprecated": {deprecated},
            "builtin_webapps": {{{webapps}}},
            "wakeword": {wakeword},
            "artifacts": {{
              "wakeword": {{"model": {wakeword_model}}},
              "daemon": {daemon},
              "daemon_zst": {daemon_zst},
              "image_swu": {swu},
              "image_zck": {zck},
              "image_boot_zck": {boot},
              "webapps": {{}},
              "daemon_patches": {{}}
            }}
          }}
        }}
      }}"#,
      channel = self.channel,
      latest = self.latest,
      yanked = self
        .yanked
        .as_ref()
        .map_or_else(|| "null".to_string(), |reason| format!("\"{reason}\"")),
      deprecated = self.deprecated,
      webapps = webapps,
      wakeword = self.wakeword_model.as_ref().map_or_else(
        || "null".to_string(),
        |model| {
          let runtime = self.wakeword_runtime.as_deref().unwrap_or("0.0.0");
          format!(r#"{{"runtime":"{runtime}","model":"{model}"}}"#)
        }
      ),
      wakeword_model = digest(&self.wakeword_model_digest),
      daemon = digest(&self.daemon),
      daemon_zst = digest(&self.daemon_zst),
      swu = digest(&self.image_swu),
      zck = digest(&self.image_zck),
      boot = digest(&self.image_boot_zck),
    )
  }
}

pub fn route_into(gateway: &Gateway, service: &Arc<OtaService>, device_id: &str) {
  let mut inbound = gateway.events();
  let held = Arc::downgrade(service);
  let device_id = device_id.to_owned();
  let gateway = gateway.clone();

  rt::spawn(async move {
    loop {
      let msg = match inbound.recv().await {
        Ok(msg) => msg,
        Err(broadcast::error::RecvError::Lagged(_)) => continue,
        Err(broadcast::error::RecvError::Closed) => return,
      };
      let Some(service) = held.upgrade() else { return };
      let id = msg.id;
      match msg.data {
        BridgeToGatewayMsgData::Version(meta) => service.device_meta(&device_id, *meta),
        BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Ack(ack)) => {
          service.transfer_ack(&device_id, ack.transfer_id, u64::from(ack.received))
        }
        BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::WebappInstalled(info)) => {
          service.webapp_installed(&device_id, info)
        }
        BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaProgress(tick)) => {
          service.progress(&device_id, tick)
        }
        BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaError(err)) => service.error(&device_id, err),
        BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaFinished(done)) => {
          service.finished(&device_id, done)
        }
        BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::DeviceNicknameChanged(reply)) => {
          service.nickname_changed(&device_id, reply.nickname);
        }
        BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaAssetRangeAbandon(payload)) => {
          service.asset_range_abandon(&device_id, payload)
        }
        BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaAssetRange(request)) => {
          let gateway = gateway.clone();
          let device_id = device_id.clone();
          rt::spawn(
            async move { answer_range(&gateway, service.asset_range(&device_id, id, request).await, id).await },
          );
        }
        _ => {}
      }
    }
  });
}

pub fn route_acks(gateway: &Gateway, acks: &Arc<AckWindow>) {
  let mut inbound = gateway.events();
  let held = Arc::downgrade(acks);

  rt::spawn(async move {
    loop {
      let msg = match inbound.recv().await {
        Ok(msg) => msg,
        Err(broadcast::error::RecvError::Lagged(_)) => continue,
        Err(broadcast::error::RecvError::Closed) => return,
      };
      let Some(acks) = held.upgrade() else { return };
      if let BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Ack(ack)) = msg.data {
        acks.note(ack.transfer_id, u64::from(ack.received));
      }
    }
  });
}

pub fn route_ranges(gateway: &Gateway, server: &Arc<RangeServer>) {
  let mut inbound = gateway.events();
  let held = Arc::downgrade(server);
  let gateway = gateway.clone();

  rt::spawn(async move {
    loop {
      let msg = match inbound.recv().await {
        Ok(msg) => msg,
        Err(broadcast::error::RecvError::Lagged(_)) => continue,
        Err(broadcast::error::RecvError::Closed) => return,
      };
      let Some(server) = held.upgrade() else { return };
      let id = msg.id;
      match msg.data {
        BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Ack(ack)) => {
          server.acks().note(ack.transfer_id, u64::from(ack.received))
        }
        BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaAssetRange(request)) => {
          let gateway = gateway.clone();
          rt::spawn(async move { answer_range(&gateway, server.answer(id, request).await, id).await });
        }
        _ => {}
      }
    }
  });
}

async fn answer_range(
  gateway: &Gateway,
  answer: Result<Reply<OtaAssetRangeReply>, HandlerError<OtaAssetRangeRejected>>,
  id: Uuid,
) {
  let conn = gateway.connection();
  match answer {
    Ok(reply) => {
      let _ = conn
        .respond_to_with::<OtaAssetRange>(id, reply.response, Priority::Normal, Compress::Auto)
        .await;
      if let Some(after) = reply.after {
        after.await;
      }
    }
    Err(HandlerError::Domain(error)) => {
      let _ = conn.respond_err::<OtaAssetRange>(id, error).await;
    }
    Err(HandlerError::Wire(error)) => {
      let _ = conn.respond(id, error.into()).await;
    }
  }
}
