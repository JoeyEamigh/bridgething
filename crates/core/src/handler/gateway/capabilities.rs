use libbridgething::{
  Device, DeviceType, GatewayCapabilities, LinkKind, PeerCompanionStatus,
  gateway::{BridgeToGatewayNotificationsMsgEvent, GatewayToBridgeCapabilitiesMsgEventDispatch},
};

use super::{HandlerResult, MsgHandle};
use crate::bluetooth::{BluetoothError, GatewayType};

pub struct CapabilitiesHandler {
  handle: MsgHandle,
}

impl CapabilitiesHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl GatewayToBridgeCapabilitiesMsgEventDispatch for CapabilitiesHandler {
  type Output = HandlerResult;

  async fn announce(&self, params: GatewayCapabilities) -> HandlerResult {
    if let Some(mac) = self.handle.address {
      match self.handle.protocol {
        GatewayType::Network => {
          let device = Device {
            name: params.gateway.name.clone(),
            device_type: device_type_from_os(&params.gateway.os_name),
            id: params.gateway.address.clone(),
            kind: LinkKind::Network,
            default: false,
          };
          if device.id.is_empty() {
            tracing::debug!("a network companion announced no host identifier; not persisting it");
          } else if let Err(err) = self.handle.state.devices.remember(&device).await {
            tracing::warn!(?err, "failed to persist a network companion on capabilities announce");
          }
          self.handle.state.peers.upsert(mac, device).await;
        }
        GatewayType::Rfcomm | GatewayType::Iap2Ea => {
          let device = Device {
            name: params.gateway.name.clone(),
            device_type: device_type_from_os(&params.gateway.os_name),
            id: mac.to_string(),
            kind: LinkKind::Bluetooth,
            default: false,
          };
          match self
            .handle
            .bluetooth
            .profile_man
            .upsert_paired_device(mac, device.device_type.clone())
            .await
          {
            Ok(_) | Err(BluetoothError::NoRadio) => {}
            Err(err) => tracing::warn!(?err, "failed to upsert paired device on capabilities announce"),
          }
          self.handle.state.peers.ensure_exists(mac, device).await;
        }
      }
      self
        .handle
        .state
        .peers
        .set_companion(mac, PeerCompanionStatus::Connected(params.gateway.clone()))
        .await;
      match self.handle.state.capabilities.set_announce(mac, params).await {
        Ok(true) => {
          if let Err(err) = self.handle.state.player.note_library_changed().await {
            tracing::warn!(?err, "failed to invalidate browse caches on provider change");
          }
        }
        Ok(false) => {}
        Err(err) => tracing::warn!(?err, "failed to publish capabilities snapshot"),
      }
      let ancs = self.handle.bluetooth.le.ancs_auth_state();
      self
        .handle
        .bluetooth
        .gateway_man
        .send_event(mac, BridgeToGatewayNotificationsMsgEvent::AncsAuthStateChanged(ancs))
        .await;
    }
    Ok(())
  }
}

fn device_type_from_os(os_name: &str) -> DeviceType {
  match os_name.to_ascii_lowercase().as_str() {
    "android" => DeviceType::Android,
    "ios" => DeviceType::Ios,
    "linux" => DeviceType::Linux,
    "macos" | "darwin" => DeviceType::MacOS,
    "windows" => DeviceType::Windows,
    _ => DeviceType::Unknown,
  }
}
