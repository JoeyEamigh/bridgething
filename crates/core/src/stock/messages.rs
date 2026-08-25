use libbridgething::{Device, NetworkKind};

use crate::{
  capabilities::CapabilitiesRegistry,
  net::{WSError, WireEventBus},
  stock::{
    StockConfigurationSend, StockConnectionSend, StockConnectionType, StockDeviceType, StockInterAppSend,
    StockInterAppSendPayload,
  },
};

pub async fn broadcast_stock_connection(
  bus: &WireEventBus,
  device: &Device,
  capabilities: &CapabilitiesRegistry,
) -> Result<(), Vec<WSError>> {
  let phone_type: StockDeviceType = device.device_type.clone().into();
  bus.set_stock_phone(phone_type);

  bus
    .broadcast_stock(StockConnectionSend::RemoteStatus {
      payload: true,
      mac: device.id.clone(),
      phone_type,
    })
    .await?;
  bus
    .broadcast_stock(StockConnectionSend::TransportStatus { payload: true })
    .await?;
  bus.broadcast_stock(StockConfigurationSend::default()).await?;

  let connection_type = match capabilities.snapshot().network.kind {
    NetworkKind::Wifi | NetworkKind::Ethernet => StockConnectionType::Wlan,
    NetworkKind::Cellular => StockConnectionType::FourG,
    NetworkKind::Unknown => StockConnectionType::Wlan,
  };
  bus
    .broadcast_stock(StockInterAppSend {
      msg_id: None,
      data: StockInterAppSendPayload::SessionState {
        connection_type,
        is_in_forced_offline_mode: false,
        is_logged_in: true,
        is_offline: false,
      },
    })
    .await?;

  bus
    .broadcast_stock(StockConnectionSend::RemoteApp {
      app_id: "com.bridgething".to_string(),
      is_spotify: true,
    })
    .await?;

  Ok(())
}

pub async fn broadcast_stock_disconnection(bus: &WireEventBus) -> Result<(), Vec<WSError>> {
  bus
    .broadcast_stock(StockConnectionSend::TransportStatus { payload: false })
    .await
}
