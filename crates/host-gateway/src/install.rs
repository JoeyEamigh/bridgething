use std::{path::Path, sync::Arc};

use anyhow::{Result, anyhow};
use bridgething_delivery::{
  ota::{service::WebappInstallResult, stream::FileSource},
  session::DeliverySession,
};

use crate::{
  chaos::ChaosConfig,
  session::{self, DEVICE_ID},
};

pub async fn run_install(url: &str, chaos: ChaosConfig, bundle: &Path, provenance: Option<&str>) -> Result<()> {
  let session = session::connect(url, chaos).await?;
  install(&session, bundle, provenance).await
}

pub async fn install(session: &DeliverySession, bundle: &Path, provenance: Option<&str>) -> Result<()> {
  match session
    .ota
    .install_webapp(DEVICE_ID, Arc::new(FileSource::open(bundle)), provenance, None)
    .await
  {
    WebappInstallResult::Installed(info) => {
      tracing::info!(id = %info.id, name = %info.name, version = %info.version, "webapp installed");
      println!("installed: id={} name={} version={}", info.id, info.name, info.version);
      Ok(())
    }
    WebappInstallResult::Failed { reason } => Err(anyhow!("install failed: {reason}")),
  }
}
