use anyhow::Result;
use async_trait::async_trait;
use bridgething::{Address, Iap2Event, Iap2InjectTx};
use bridgething_client::Client as CommandClient;
use bridgething_gateway::Gateway;
#[cfg(target_os = "linux")]
use bridgething_iap2::DeviceEmulatorHandle;
use bridgething_iap2::{SessionEvent, csm::now_playing::NowPlayingUpdate as Iap2NowPlaying};

#[cfg(target_os = "linux")]
use crate::DeviceHarness;
use crate::{FrameObserver, Harness, Iap2OutboundObserver, MockWsClient};

#[async_trait]
pub trait GatewayDriver {
  async fn gateway(&self) -> Result<Gateway>;
}

#[async_trait]
pub trait Iap2SourceDriver {
  type Source: Iap2Source;
  async fn iap2_source(&self) -> Result<Self::Source>;
}

#[async_trait]
pub trait Iap2Source: Send + Sync {
  async fn push_now_playing(&self, update: Iap2NowPlaying) -> Result<()>;
  async fn push_artwork(&self, transfer_id: u8, bytes: Vec<u8>) -> Result<()>;
}

#[async_trait]
pub trait ModernClientDriver {
  async fn modern_client(&self) -> Result<MockWsClient>;
}

#[async_trait]
pub trait FrameObserve {
  async fn frames(&self) -> Result<FrameObserver>;
}

#[async_trait]
pub trait CommandDriver {
  async fn command_client(&self) -> Result<CommandClient>;
}

#[async_trait]
pub trait WebappProvision {
  async fn activate_webapp_declaring(&self, permissions: &[&str]) -> Result<()>;
}

#[async_trait]
pub trait Iap2OutboundObserve {
  async fn iap2_outbound(&self) -> Result<Iap2OutboundObserver>;
}

#[async_trait]
impl GatewayDriver for Harness {
  async fn gateway(&self) -> Result<Gateway> {
    self.connect_android().await
  }
}

#[async_trait]
impl FrameObserve for Harness {
  async fn frames(&self) -> Result<FrameObserver> {
    Ok(self.observe_frames())
  }
}

#[async_trait]
impl ModernClientDriver for Harness {
  async fn modern_client(&self) -> Result<MockWsClient> {
    self.connect_modern_client().await
  }
}

#[async_trait]
impl CommandDriver for Harness {
  async fn command_client(&self) -> Result<CommandClient> {
    self.connect_command_client().await
  }
}

#[async_trait]
impl WebappProvision for Harness {
  async fn activate_webapp_declaring(&self, permissions: &[&str]) -> Result<()> {
    Harness::activate_webapp_declaring(self, permissions).await.map(|_| ())
  }
}

#[async_trait]
impl Iap2OutboundObserve for Harness {
  async fn iap2_outbound(&self) -> Result<Iap2OutboundObserver> {
    Ok(self.observe_iap2_outbound())
  }
}

#[async_trait]
impl Iap2SourceDriver for Harness {
  type Source = HarnessIap2Source;
  async fn iap2_source(&self) -> Result<HarnessIap2Source> {
    let addr = self.iap2_peer();
    self.identify_iphone(addr).await?;
    Ok(HarnessIap2Source {
      iap2: self.inject.iap2.clone(),
      addr,
    })
  }
}

pub struct HarnessIap2Source {
  iap2: Iap2InjectTx,
  addr: Address,
}

#[async_trait]
impl Iap2Source for HarnessIap2Source {
  async fn push_now_playing(&self, update: Iap2NowPlaying) -> Result<()> {
    self
      .iap2
      .send(Iap2Event {
        address: self.addr,
        event: SessionEvent::NowPlayingUpdate(update),
      })
      .await
      .map_err(|_| anyhow::anyhow!("iap2 inject channel closed"))
  }

  async fn push_artwork(&self, transfer_id: u8, bytes: Vec<u8>) -> Result<()> {
    self
      .iap2
      .send(Iap2Event {
        address: self.addr,
        event: SessionEvent::ArtworkBytes {
          transfer_id,
          bytes: bytes.into(),
        },
      })
      .await
      .map_err(|_| anyhow::anyhow!("iap2 inject channel closed"))
  }
}

#[async_trait]
#[cfg(target_os = "linux")]
impl FrameObserve for DeviceHarness {
  async fn frames(&self) -> Result<FrameObserver> {
    self.frame_tap().await
  }
}

#[async_trait]
#[cfg(target_os = "linux")]
impl ModernClientDriver for DeviceHarness {
  async fn modern_client(&self) -> Result<MockWsClient> {
    self.connect_modern_client().await
  }
}

#[async_trait]
#[cfg(target_os = "linux")]
impl Iap2SourceDriver for DeviceHarness {
  type Source = DeviceIap2Source;
  async fn iap2_source(&self) -> Result<DeviceIap2Source> {
    Ok(DeviceIap2Source {
      handle: self.connect_iap2_emulator().await?,
    })
  }
}

#[cfg(target_os = "linux")]
pub struct DeviceIap2Source {
  handle: DeviceEmulatorHandle,
}

#[async_trait]
#[cfg(target_os = "linux")]
impl Iap2Source for DeviceIap2Source {
  async fn push_now_playing(&self, update: Iap2NowPlaying) -> Result<()> {
    self.handle.push_now_playing(update).await?;
    Ok(())
  }

  async fn push_artwork(&self, transfer_id: u8, bytes: Vec<u8>) -> Result<()> {
    self.handle.push_artwork(transfer_id, bytes.into()).await?;
    Ok(())
  }
}

#[derive(Clone, Copy)]
pub enum OverAirTransport {
  Rfcomm,
  Iap2Ea,
}

#[cfg(target_os = "linux")]
pub struct DeviceTier {
  harness: DeviceHarness,
  transport: OverAirTransport,
}

#[cfg(target_os = "linux")]
impl DeviceTier {
  pub fn new(harness: DeviceHarness, transport: OverAirTransport) -> Self {
    Self { harness, transport }
  }
}

#[async_trait]
#[cfg(target_os = "linux")]
impl GatewayDriver for DeviceTier {
  async fn gateway(&self) -> Result<Gateway> {
    match self.transport {
      OverAirTransport::Rfcomm => self.harness.connect_over_air().await,
      OverAirTransport::Iap2Ea => self.harness.connect_over_air_iap2().await,
    }
  }
}

#[async_trait]
#[cfg(target_os = "linux")]
impl FrameObserve for DeviceTier {
  async fn frames(&self) -> Result<FrameObserver> {
    self.harness.frame_tap().await
  }
}

#[async_trait]
#[cfg(target_os = "linux")]
impl ModernClientDriver for DeviceTier {
  async fn modern_client(&self) -> Result<MockWsClient> {
    self.harness.connect_modern_client().await
  }
}
