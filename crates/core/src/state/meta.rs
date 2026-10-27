use std::sync::Arc;

use libbridgething::{BridgeThingMeta, LauncherGesture};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::{KvStore, StateResult};

const BRIDGETHING_VERSION: &str = env!("CARGO_PKG_VERSION");
const BRIDGETHING_APP_NAME: &str = env!("CARGO_PKG_NAME");
const NICKNAME_KV_KEY: &str = "nickname";
const LAUNCHER_GESTURE_KV_KEY: &str = "launcher_gesture";
const COMPANION_LAUNCH_KV_KEY: &str = "companion_launch";
#[cfg(feature = "test-tap")]
const ENV_APP_VERSION: &str = "BRIDGETHING_APP_VERSION";

fn app_version() -> String {
  #[cfg(feature = "test-tap")]
  if let Ok(version) = std::env::var(ENV_APP_VERSION) {
    return version;
  }
  BRIDGETHING_VERSION.to_string()
}

fn launcher_gesture_as_storage(gesture: LauncherGesture) -> &'static str {
  match gesture {
    LauncherGesture::LongPress => "longPress",
    LauncherGesture::FivePress => "fivePress",
    LauncherGesture::DoublePress => "doublePress",
  }
}

fn parse_launcher_gesture(stored: Option<&str>) -> LauncherGesture {
  match stored {
    Some("longPress") => LauncherGesture::LongPress,
    Some("fivePress") => LauncherGesture::FivePress,
    Some("doublePress") => LauncherGesture::DoublePress,
    _ => LauncherGesture::default(),
  }
}

fn parse_companion_launch(stored: Option<&str>) -> bool {
  stored != Some("false")
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SuperbirdMeta {
  pub name: String,
  pub version: String,
  pub description: String,
  pub bt_mac: String,
  pub serial_number: String,
  pub fcc_id: String,
  pub ic_id: String,
  pub model_name: String,
  pub channel: String,
  pub image_variant: String,
  pub image_version: String,
  pub image_build_id: String,
  pub image_build_date: String,
  pub image_distro: String,
  pub image_machine: String,
}

impl SuperbirdMeta {
  pub async fn read_or_default() -> Self {
    let meta_path = crate::paths::superbird_meta();

    if !meta_path.exists() {
      tracing::warn!(
        path = %meta_path.display(),
        "superbird metadata missing; bridgething is only officially supported on bridgethingOS"
      );
      return Self::default();
    }

    let data = match tokio::fs::read(&meta_path).await {
      Ok(data) => data,
      Err(err) => {
        tracing::warn!(path = %meta_path.display(), %err, "could not read superbird metadata; falling back to defaults");
        return Self::default();
      }
    };

    match serde_json::from_slice(&data) {
      Ok(meta) => meta,
      Err(err) => {
        tracing::warn!(path = %meta_path.display(), %err, "could not parse superbird metadata; struct shape may have drifted from the on-disk template");
        Self::default()
      }
    }
  }
}

#[derive(Debug, Clone)]
pub struct DeviceMeta {
  inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
  static_meta: SuperbirdMeta,
  kv: KvStore,
  nickname_tx: watch::Sender<Option<String>>,
  launcher_gesture_tx: watch::Sender<LauncherGesture>,
  companion_launch_tx: watch::Sender<bool>,
  daemon_sha_tx: watch::Sender<Option<String>>,
  wakeword_version_tx: watch::Sender<Option<String>>,
  bluetooth_name_tx: watch::Sender<Option<String>>,
}

impl DeviceMeta {
  pub async fn init(static_meta: SuperbirdMeta, kv: KvStore) -> Self {
    let initial = kv.device_get(NICKNAME_KV_KEY).await.unwrap_or_else(|err| {
      tracing::warn!(?err, "kv device_get nickname at startup failed; starting empty");
      None
    });
    let (nickname_tx, _rx) = watch::channel(initial);
    let stored_gesture = kv.device_get(LAUNCHER_GESTURE_KV_KEY).await.unwrap_or_else(|err| {
      tracing::warn!(
        ?err,
        "kv device_get launcher gesture at startup failed; starting default"
      );
      None
    });
    let (launcher_gesture_tx, _gesture_rx) = watch::channel(parse_launcher_gesture(stored_gesture.as_deref()));
    let stored_launch = kv.device_get(COMPANION_LAUNCH_KV_KEY).await.unwrap_or_else(|err| {
      tracing::warn!(
        ?err,
        "kv device_get companion launch at startup failed; starting default"
      );
      None
    });
    let (companion_launch_tx, _launch_rx) = watch::channel(parse_companion_launch(stored_launch.as_deref()));
    let (daemon_sha_tx, _sha_rx) = watch::channel(None);
    let (wakeword_version_tx, _ww_rx) = watch::channel(crate::ota::wakeword_model_version().await);
    let (bluetooth_name_tx, _bt_rx) = watch::channel(None);
    let me = Self {
      inner: Arc::new(Inner {
        static_meta,
        kv,
        nickname_tx,
        launcher_gesture_tx,
        companion_launch_tx,
        daemon_sha_tx,
        wakeword_version_tx,
        bluetooth_name_tx,
      }),
    };
    me.spawn_daemon_sha();
    me
  }

  fn spawn_daemon_sha(&self) {
    let tx = self.inner.daemon_sha_tx.clone();
    tokio::spawn(async move {
      let path = crate::ota::daemon_swap::patch_source_path();
      match crate::digest::sha256_file(&path).await {
        Ok(digest) => {
          tracing::info!(path = %path.display(), sha256 = %digest, "daemon binary digest computed");
          tx.send_replace(Some(digest));
        }
        Err(err) => {
          tracing::warn!(path = %path.display(), %err, "could not hash daemon binary; patch sources go unverified")
        }
      }
    });
  }

  pub fn daemon_sha256(&self) -> Option<String> {
    self.inner.daemon_sha_tx.borrow().clone()
  }

  pub fn wakeword_model_version(&self) -> Option<String> {
    self.inner.wakeword_version_tx.borrow().clone()
  }

  pub async fn refresh_wakeword_model_version(&self) {
    let next = crate::ota::wakeword_model_version().await;
    self.inner.wakeword_version_tx.send_replace(next);
  }

  pub fn bluetooth_name(&self) -> Option<String> {
    self.inner.bluetooth_name_tx.borrow().clone()
  }

  pub fn set_bluetooth_name(&self, name: String) {
    self.inner.bluetooth_name_tx.send_replace(Some(name));
  }

  pub fn static_meta(&self) -> &SuperbirdMeta {
    &self.inner.static_meta
  }

  pub fn nickname(&self) -> Option<String> {
    self.inner.nickname_tx.borrow().clone()
  }

  pub fn subscribe(&self) -> watch::Receiver<Option<String>> {
    self.inner.nickname_tx.subscribe()
  }

  pub fn launcher_gesture(&self) -> LauncherGesture {
    *self.inner.launcher_gesture_tx.borrow()
  }

  pub fn subscribe_launcher_gesture(&self) -> watch::Receiver<LauncherGesture> {
    self.inner.launcher_gesture_tx.subscribe()
  }

  pub async fn set_launcher_gesture(&self, next: LauncherGesture) -> StateResult<()> {
    self
      .inner
      .kv
      .device_set(LAUNCHER_GESTURE_KV_KEY, launcher_gesture_as_storage(next).into())
      .await?;
    self.inner.launcher_gesture_tx.send_replace(next);
    Ok(())
  }

  pub fn companion_launch(&self) -> bool {
    *self.inner.companion_launch_tx.borrow()
  }

  pub fn subscribe_companion_launch(&self) -> watch::Receiver<bool> {
    self.inner.companion_launch_tx.subscribe()
  }

  pub async fn set_companion_launch(&self, enabled: bool) -> StateResult<()> {
    self
      .inner
      .kv
      .device_set(COMPANION_LAUNCH_KV_KEY, enabled.to_string())
      .await?;
    self.inner.companion_launch_tx.send_replace(enabled);
    Ok(())
  }

  pub async fn set_nickname(&self, next: Option<String>) -> StateResult<()> {
    match &next {
      Some(value) => self.inner.kv.device_set(NICKNAME_KV_KEY, value.clone()).await?,
      None => self.inner.kv.device_delete(NICKNAME_KV_KEY).await?,
    }
    self.inner.nickname_tx.send_replace(next);
    Ok(())
  }

  pub fn snapshot(&self) -> BridgeThingMeta {
    build_meta(
      &self.inner.static_meta,
      self.nickname(),
      self.bluetooth_name(),
      self.launcher_gesture(),
      self.companion_launch(),
      self.daemon_sha256(),
      self.wakeword_model_version(),
    )
  }
}

fn build_meta(
  meta: &SuperbirdMeta,
  nickname: Option<String>,
  bluetooth_name: Option<String>,
  launcher_gesture: LauncherGesture,
  companion_launch: bool,
  daemon_sha256: Option<String>,
  wakeword_model_version: Option<String>,
) -> BridgeThingMeta {
  let app_version = app_version();
  BridgeThingMeta {
    bridgething_version: format!("v{app_version}"),
    libbridgething_version: BridgeThingMeta::libbridgething_version(),
    app_name: BRIDGETHING_APP_NAME.to_string(),
    nickname,
    bluetooth_name,
    launcher_gesture,
    companion_launch,
    app_version,
    daemon_sha256,
    wakeword_model_version,
    os_name: meta.name.clone(),
    os_version: meta.version.clone(),
    os_description: meta.description.clone(),
    bt_mac: meta.bt_mac.clone(),
    serial_number: meta.serial_number.clone(),
    fcc_id: meta.fcc_id.clone(),
    ic_id: meta.ic_id.clone(),
    model_name: meta.model_name.clone(),
    channel: meta.channel.clone(),
    image_variant: meta.image_variant.clone(),
    image_version: meta.image_version.clone(),
    image_build_id: meta.image_build_id.clone(),
    image_build_date: meta.image_build_date.clone(),
    image_distro: meta.image_distro.clone(),
    image_machine: meta.image_machine.clone(),
    discord: "https://tl.mt/d".to_string(),
    credits: "Joey Eamigh".to_string(),
  }
}

#[cfg(all(test, feature = "test-tap"))]
mod tests {
  use libbridgething::LauncherGesture;

  use super::{ENV_APP_VERSION, SuperbirdMeta, build_meta};

  #[test]
  fn a_test_daemon_told_its_version_announces_it() {
    unsafe { std::env::set_var(ENV_APP_VERSION, "99.0.0") };

    let meta = build_meta(
      &SuperbirdMeta::default(),
      None,
      None,
      LauncherGesture::default(),
      true,
      None,
      None,
    );

    assert_eq!(
      meta.app_version, "99.0.0",
      "a restarted test daemon has to come back on the version it was updated to"
    );
    assert_eq!(meta.bridgething_version, "v99.0.0");
  }
}
