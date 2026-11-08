use std::sync::Arc;

use anyhow::{Context, Result};
use bridgething_delivery::{
  bundle::fetch::HttpArtifactFetch,
  seam::SystemClock,
  session::{DeliverySession, SessionDeps, gateway_info},
};
use bridgething_gateway::{
  Gateway, connect_ws,
  transport::{FramedConnector, WsConnector},
};
use bridgething_io::{HttpExecutor, ReqwestConfig, ReqwestTransport};
use libbridgething::gateway::BridgeToGatewayMsg;
use tokio::{
  io::{AsyncRead, AsyncWrite},
  sync::broadcast,
};

use crate::chaos::{ChaosConfig, ChaosConnector};

pub const DEVICE_ID: &str = "host-gateway";

pub async fn connect(url: &str, chaos: ChaosConfig) -> Result<DeliverySession> {
  tracing::info!(%url, "connecting to daemon network gateway");
  let ws = connect_ws(url).await.with_context(|| format!("ws connect: {url}"))?;
  Ok(DeliverySession::spawn(ChaosConnector::new(WsConnector::new(ws), chaos), deps()).await?)
}

pub async fn connect_gateway(
  url: &str,
  chaos: ChaosConfig,
) -> Result<(Gateway, broadcast::Receiver<BridgeToGatewayMsg>)> {
  tracing::info!(%url, "connecting to daemon network gateway");
  let ws = connect_ws(url).await.with_context(|| format!("ws connect: {url}"))?;
  Ok(Gateway::spawn_subscribed(ChaosConnector::new(
    WsConnector::new(ws),
    chaos,
  )))
}

pub async fn from_io<S>(io: S, chaos: ChaosConfig) -> Result<DeliverySession>
where
  S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
  Ok(DeliverySession::spawn(ChaosConnector::new(FramedConnector::new(io), chaos), deps()).await?)
}

fn deps() -> SessionDeps {
  let transport = Arc::new(ReqwestTransport::new(ReqwestConfig::default()));
  SessionDeps {
    device_id: DEVICE_ID.to_owned(),
    clock: Arc::new(SystemClock),
    fetch: Arc::new(HttpArtifactFetch::new(HttpExecutor::new(transport))),
    cache_dir: std::env::temp_dir().join("bridgething-host-gateway"),
    data_dir: Some(std::env::temp_dir().join("bridgething-host-gateway-state")),
    info: gateway_info(DEVICE_ID, "linux", env!("CARGO_PKG_VERSION")),
  }
}
