use std::collections::HashMap;

use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Device;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct BluetoothStatus {
  pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct ConnectedDevice {
  pub name: String,
  pub mac: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// The device's own bluetooth adapter.
pub struct BluetoothInterface {
  pub mac: String,
  pub name: String,
  /// For example `hci0`.
  pub interface: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct BluetoothPairingResult {
  pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct BluetoothPin {
  pub mac: String,
  pub name: String,
  pub pin: String,
}

/// Paired devices keyed by MAC address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(transparent)]
#[ts(export, export_to = "client.ts")]
pub struct PairedDevicesMap(pub HashMap<String, Device>);

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Bluetooth pairing and connection state. `onStatus` and `onConnectedDevice` track the connected
/// phone, `onPin` carries a code to show on screen, and `list` returns the paired devices.
pub enum BridgeToClientBluetoothMsg {
  #[bridge_event]
  Status(BluetoothStatus),
  #[bridge_event]
  ConnectedDevice(ConnectedDevice),
  #[bridge_event]
  Interface(BluetoothInterface),
  #[bridge_event]
  PairingResult(BluetoothPairingResult),
  #[bridge_event]
  Pin(BluetoothPin),
  #[bridge_response]
  PairedDevices(PairedDevicesMap),
}
