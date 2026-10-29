pub mod chaos;
pub mod install;
pub mod ota;
pub mod presence;
pub mod session;
pub mod webapp;

use anyhow::Result;

use crate::chaos::ChaosConfig;

pub async fn run_open(url: &str, chaos: ChaosConfig) -> Result<()> {
  let session = session::connect(url, chaos).await?;
  tracing::info!("link open; serving every surface the routing path answers");
  session.closed().await;
  tracing::info!("connection closed - exiting");
  Ok(())
}

pub fn init_logging() {
  let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
    tracing_subscriber::EnvFilter::new("bridgething_host_gateway=info,bridgething_delivery=info,info")
  });
  let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
