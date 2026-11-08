use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use bridgething_gateway::Gateway;
use libbridgething::gateway::{BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewaySystemMsg, HostState};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::{chaos::ChaosConfig, session};

pub async fn run_host_state(url: &str, chaos: ChaosConfig, state: HostState, hold: Option<Duration>) -> Result<()> {
  let (gateway, inbound) = session::connect_gateway(url, chaos).await?;
  let hold = async {
    match hold {
      Some(hold) => tokio::time::sleep(hold).await,
      None => std::future::pending().await,
    }
  };
  tokio::select! {
    served = serve(&gateway, inbound, state) => served,
    _ = tokio::signal::ctrl_c() => Ok(()),
    () = hold => Ok(()),
  }
}

pub async fn serve(
  gateway: &Gateway,
  mut inbound: broadcast::Receiver<BridgeToGatewayMsg>,
  state: HostState,
) -> Result<()> {
  if state.away.is_some() {
    report(gateway, state).await?;
  }
  loop {
    match inbound.recv().await {
      Ok(BridgeToGatewayMsg {
        data: BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::HostWatch(watch)),
        ..
      }) => {
        tracing::info!(?watch, ?state, "answering the device's host watch");
        report(gateway, state).await?;
      }
      Ok(_) | Err(RecvError::Lagged(_)) => {}
      Err(RecvError::Closed) => bail!("the device closed the link"),
    }
  }
}

async fn report(gateway: &Gateway, state: HostState) -> Result<()> {
  gateway
    .system()
    .host_state(state)
    .await
    .map_err(|err| anyhow!("host state report failed: {err:?}"))?;
  tracing::info!(?state, "the device acked the host state");
  Ok(())
}
