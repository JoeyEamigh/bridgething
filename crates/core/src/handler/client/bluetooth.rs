use libbridgething::{
  LinkKind,
  client::{
    ClientToBridgeBluetoothMsgDispatch, ConnectBluetooth, ForgetBluetooth, ListBluetoothDevices, PairedDevicesMap,
    SetBluetoothAlias,
  },
};

use super::{HandlerResult, MsgHandle};

pub struct BluetoothHandler {
  handle: MsgHandle,
}

impl BluetoothHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl ClientToBridgeBluetoothMsgDispatch for BluetoothHandler {
  type Output = HandlerResult;

  async fn list(&self) -> HandlerResult {
    tracing::debug!("({}) sending list of paired devices", &self.handle.from);
    let devices = self.handle.state.devices.list(LinkKind::Bluetooth).await?;
    tracing::trace!("({}) devices: {:?}", &self.handle.from, &devices);
    Ok(
      self
        .handle
        .respond_to::<ListBluetoothDevices>(PairedDevicesMap(devices.into_iter().collect()))
        .await?,
    )
  }

  async fn connect(&self, params: ConnectBluetooth) -> HandlerResult {
    let ConnectBluetooth { mac } = params;
    tracing::debug!("({}) connecting to device with MAC: {}", &self.handle.from, mac);
    Ok(self.handle.bluetooth.connect(&mac).await?)
  }

  async fn enable_discoverable(&self) -> HandlerResult {
    tracing::debug!("({}) enabling discoverable mode", &self.handle.from);
    Ok(self.handle.bluetooth.profile_man.set_discoverable(true).await?)
  }

  async fn disable_discoverable(&self) -> HandlerResult {
    tracing::debug!("({}) disabling discoverable mode", &self.handle.from);
    Ok(self.handle.bluetooth.profile_man.set_discoverable(false).await?)
  }

  async fn forget(&self, params: ForgetBluetooth) -> HandlerResult {
    let ForgetBluetooth { mac } = params;
    tracing::debug!("({}) forgetting device with MAC: {}", &self.handle.from, mac);

    self.handle.bluetooth.profile_man.forget(&mac).await?;
    self.handle.state.devices.remove(mac).await?;

    let devices = self.handle.state.devices.list(LinkKind::Bluetooth).await?;
    self
      .handle
      .respond_to::<ListBluetoothDevices>(PairedDevicesMap(devices.into_iter().collect()))
      .await?;

    Ok(())
  }

  async fn set_alias(&self, params: SetBluetoothAlias) -> HandlerResult {
    let SetBluetoothAlias { name } = params;
    tracing::debug!("({}) setting adapter alias to: {}", &self.handle.from, name);
    Ok(self.handle.bluetooth.profile_man.set_alias(name).await?)
  }
}
