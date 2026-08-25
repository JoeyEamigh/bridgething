#[cfg(target_os = "linux")]
use std::sync::Arc;

#[cfg(target_os = "linux")]
use bluer::{Adapter, AdapterEvent, AdapterProperty};
#[cfg(target_os = "linux")]
use libbridgething::{client::BridgeToClientBluetoothMsg, wire::MsgMeta};
use tokio::sync::{mpsc, oneshot};

#[cfg(target_os = "linux")]
use super::iap2::Iap2ReconnectHandle;
use super::{Address, BluetoothError, BluetoothResult};
#[cfg(target_os = "linux")]
use crate::{net::WireEventBus, peer::PeerTracker, state::DeviceStore};

pub(crate) const PROFILE_COMMAND_CAPACITY: usize = 16;

#[derive(Debug)]
pub enum ProfileCommand {
  SetAlias {
    alias: String,
    reply: Reply<()>,
  },
  SetDiscoverable {
    discoverable: bool,
    reply: Reply<()>,
  },
  Forget {
    mac: String,
    reply: Reply<()>,
  },
  Reset {
    reply: Reply<()>,
  },
  UpsertPairedDevice {
    mac: Address,
    device_type: libbridgething::DeviceType,
    reply: Reply<libbridgething::Device>,
  },
}

pub type Reply<T> = oneshot::Sender<BluetoothResult<T>>;

impl ProfileCommand {
  fn reject_no_radio(self) {
    match self {
      Self::SetAlias { alias, reply } => {
        tracing::debug!(%alias, "profile set-alias rejected: no radio attached");
        let _ = reply.send(Err(BluetoothError::NoRadio));
      }
      Self::SetDiscoverable { discoverable, reply } => {
        tracing::debug!(discoverable, "profile set-discoverable rejected: no radio attached");
        let _ = reply.send(Err(BluetoothError::NoRadio));
      }
      Self::Forget { mac, reply } => {
        tracing::debug!(%mac, "profile forget rejected: no radio attached");
        let _ = reply.send(Err(BluetoothError::NoRadio));
      }
      Self::Reset { reply } => {
        tracing::debug!("profile reset rejected: no radio attached");
        let _ = reply.send(Err(BluetoothError::NoRadio));
      }
      Self::UpsertPairedDevice {
        mac,
        device_type,
        reply,
      } => {
        tracing::debug!(%mac, ?device_type, "profile upsert-paired rejected: no radio attached");
        let _ = reply.send(Err(BluetoothError::NoRadio));
      }
    }
  }
}

pub(crate) fn spawn_no_radio_actor(mut rx: ProfileCommandRx) -> tokio::task::JoinHandle<()> {
  tokio::spawn(async move {
    while let Some(command) = rx.recv().await {
      command.reject_no_radio();
    }
  })
}
pub type ProfileCommandTx = mpsc::Sender<ProfileCommand>;
pub type ProfileCommandRx = mpsc::Receiver<ProfileCommand>;

#[cfg(target_os = "linux")]
pub type ProfileMan = Arc<ProfileManager>;

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct ProfileManager {
  adapter: Adapter,
  bus: WireEventBus,
  devices: DeviceStore,
  peers: PeerTracker,
  iap2_reconnect: Iap2ReconnectHandle,
}

#[cfg(target_os = "linux")]
impl ProfileManager {
  pub fn init(
    adapter: Adapter,
    bus: WireEventBus,
    devices: DeviceStore,
    peers: PeerTracker,
    iap2_reconnect: Iap2ReconnectHandle,
  ) -> ProfileManager {
    tracing::debug!("initializing bluetooth profile connection manager");

    Self {
      adapter,
      bus,
      devices,
      peers,
      iap2_reconnect,
    }
  }

  pub async fn set_alias(&self, alias: String) -> BluetoothResult<()> {
    tracing::debug!("setting bluetooth adapter alias to {:?}", &alias);
    self.adapter.set_alias(alias).await?;
    Ok(())
  }

  pub async fn set_discoverable(&self, discoverable: bool) -> BluetoothResult<()> {
    tracing::debug!("setting bluetooth discoverable to {:?}", &discoverable);
    self.adapter.set_discoverable(discoverable).await?;
    if discoverable && let Err(err) = super::scan::apply_fast_inquiry_scan(&self.adapter) {
      tracing::warn!(?err, "failed to apply fast inquiry scan params");
    }
    Ok(())
  }

  pub async fn forget(&self, mac: &str) -> BluetoothResult<()> {
    tracing::debug!("attempting to forget device with mac address {:?}", &mac);

    let address: Address = mac.parse()?;
    self.adapter.remove_device(address.into()).await?;

    Ok(())
  }

  pub async fn reset(&self) -> BluetoothResult<()> {
    tracing::debug!("forgetting all devices");
    for mac in self.devices.list(libbridgething::LinkKind::Bluetooth).await?.keys() {
      self.forget(mac).await?;
    }

    Ok(())
  }

  #[expect(clippy::manual_async_fn)]
  pub fn handle_event(
    self: &ProfileMan,
    event: BluetoothConnectionEvent,
  ) -> impl Future<Output = BluetoothResult<()>> + Send {
    async {
      match event {
        // auth/pairing
        BluetoothConnectionEvent::AuthRequest { mac } => {
          tracing::info!("bluetooth auth request from mac address: {:?}", &mac);
          Ok(())
        }
        BluetoothConnectionEvent::ServiceAuthRequest { mac, service } => {
          tracing::info!(
            "bluetooth service auth request from mac address {:?} to service: {:?}",
            &mac,
            &service
          );
          Ok(())
        }
        BluetoothConnectionEvent::PinCode { mac, pin } => {
          tracing::info!(
            "bluetooth device with mac address {:?} pairing pincode: {:?}",
            &mac,
            &pin
          );

          self
            .bus
            .broadcast(
              BridgeToClientBluetoothMsg::Pin(libbridgething::client::BluetoothPin {
                mac: mac.to_string(),
                name: mac.to_string(),
                pin: pin.to_owned(),
              }),
              MsgMeta::Event,
            )
            .await?;

          self.peers.note_pin_shown(mac).await;

          Ok(())
        }

        // adapter
        BluetoothConnectionEvent::DeviceAdded { mac } => {
          tracing::info!("bluetooth device added with mac address: {:?}", &mac);
          let bluez_device = self.adapter.device(mac.into())?;
          if !bluez_device.is_paired().await.unwrap_or(false) {
            tracing::trace!("device added but not yet paired; awaiting Paired property change");
            return Ok(());
          }
          if let Err(err) = self
            .upsert_paired_device(mac, libbridgething::DeviceType::Unknown)
            .await
          {
            tracing::warn!(?err, "failed to register cached paired device");
          }
          Ok(())
        }
        BluetoothConnectionEvent::DeviceRemoved { mac } => {
          tracing::info!("bluetooth device removed with mac address: {:?}", &mac);

          self.peers.remove_bluez(mac).await;
          if let Err(err) = self.devices.remove(mac.to_string()).await {
            tracing::warn!(?err, "failed to remove device store entry on DeviceRemoved");
          }

          Ok(())
        }
        BluetoothConnectionEvent::PairedChanged { mac, paired } => {
          tracing::info!("bluetooth Paired property changed for mac {:?}: {}", &mac, paired);
          if paired {
            if let Err(err) = self
              .upsert_paired_device(mac, libbridgething::DeviceType::Unknown)
              .await
            {
              tracing::warn!(?err, "failed to register newly-paired device");
            }
          } else {
            self.peers.set_paired(mac, false).await;
          }
          Ok(())
        }
        BluetoothConnectionEvent::ConnectedChanged { mac, connected } => {
          tracing::trace!("bluetooth Connected property changed for mac {:?}: {}", &mac, connected);
          if connected {
            self.peers.confirm_pairing(mac).await;
            self.iap2_reconnect.kick(mac).await;
          }
          Ok(())
        }
        BluetoothConnectionEvent::AdapterPropertyChanged(property) => {
          tracing::trace!("adapter property changed: {:?}", &property);
          Ok(())
        }
      }
    }
  }

  pub async fn upsert_paired_device(
    &self,
    mac: Address,
    device_type: libbridgething::DeviceType,
  ) -> BluetoothResult<libbridgething::Device> {
    let bluez = self.adapter.device(mac.into())?;
    if !bluez.is_trusted().await.unwrap_or(false) {
      let _ = bluez.set_trusted(true).await;
    }
    let name = bluez.name().await?.unwrap_or_else(|| mac.to_string());
    let mac_str = mac.to_string();

    let device = libbridgething::Device {
      name,
      device_type,
      id: mac_str.clone(),
      kind: libbridgething::LinkKind::Bluetooth,
      default: true,
    };

    if self.devices.remember(&device).await? {
      self.set_discoverable(false).await?;
    }

    let _ = self.peers.upsert(mac, device.clone()).await;
    let _ = self.peers.set_paired(mac, true).await;
    let _ = self.peers.confirm_pairing(mac).await;

    self.iap2_reconnect.kick(mac).await;

    Ok(device)
  }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub enum BluetoothConnectionEvent {
  // auth/pairing
  AuthRequest { mac: Address },
  ServiceAuthRequest { mac: Address, service: uuid::Uuid },
  PinCode { mac: Address, pin: String },

  // adapter
  DeviceAdded { mac: Address },
  DeviceRemoved { mac: Address },
  AdapterPropertyChanged(AdapterProperty),

  // per-device property changes (from device-level event watcher)
  PairedChanged { mac: Address, paired: bool },
  ConnectedChanged { mac: Address, connected: bool },
}

#[cfg(target_os = "linux")]
impl From<AdapterEvent> for BluetoothConnectionEvent {
  fn from(event: AdapterEvent) -> Self {
    match event {
      AdapterEvent::DeviceAdded(address) => Self::DeviceAdded { mac: address.into() },
      AdapterEvent::DeviceRemoved(address) => Self::DeviceRemoved { mac: address.into() },
      AdapterEvent::PropertyChanged(property) => Self::AdapterPropertyChanged(property),
    }
  }
}

#[cfg(target_os = "linux")]
pub(crate) fn spawn_command_actor(profile_man: ProfileMan, mut rx: ProfileCommandRx) -> tokio::task::JoinHandle<()> {
  tokio::spawn(async move {
    while let Some(command) = rx.recv().await {
      match command {
        ProfileCommand::SetAlias { alias, reply } => {
          let _ = reply.send(profile_man.set_alias(alias).await);
        }
        ProfileCommand::SetDiscoverable { discoverable, reply } => {
          let _ = reply.send(profile_man.set_discoverable(discoverable).await);
        }
        ProfileCommand::Forget { mac, reply } => {
          let _ = reply.send(profile_man.forget(&mac).await);
        }
        ProfileCommand::Reset { reply } => {
          let _ = reply.send(profile_man.reset().await);
        }
        ProfileCommand::UpsertPairedDevice {
          mac,
          device_type,
          reply,
        } => {
          let _ = reply.send(profile_man.upsert_paired_device(mac, device_type).await);
        }
      }
    }
    tracing::debug!("profile command actor exiting");
  })
}
