use std::time::Duration;

use anyhow::{Result, bail};
use libbridgething::{DisplayState, HostPresence};

use crate::{chaos::ChaosConfig, session};

const HEARTBEAT: Duration = Duration::from_secs(10);

fn parse(state: &str) -> Result<DisplayState> {
  match state {
    "active" => Ok(DisplayState::Active),
    "dim" => Ok(DisplayState::Dim),
    "off" => Ok(DisplayState::Off),
    other => bail!("unknown display state {other:?}; use active, dim or off"),
  }
}

pub async fn run_presence(url: &str, chaos: ChaosConfig, state: &str, hold_s: u64) -> Result<()> {
  let screen = parse(state)?;
  let session = session::connect(url, chaos).await?;
  let report = HostPresence {
    display: screen,
    idle_s: None,
    heartbeat_s: (hold_s > 0).then_some(HEARTBEAT.as_secs() as u16),
  };
  let deadline = tokio::time::Instant::now() + Duration::from_secs(hold_s);
  loop {
    session
      .gateway
      .presence()
      .update(report.clone())
      .await
      .map_err(|err| anyhow::anyhow!("presence update failed: {err:?}"))?;
    tracing::info!(?screen, "reported presence");
    if tokio::time::Instant::now() + HEARTBEAT > deadline {
      break;
    }
    tokio::time::sleep(HEARTBEAT).await;
  }
  tokio::time::sleep(Duration::from_millis(500)).await;
  Ok(())
}
