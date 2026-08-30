use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{BrightnessState, HardwareState};

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
/// Room brightness, `0` (dark) to `100` (bright).
pub struct AmbientLightUpdate {
  pub ambient_level: u8,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
pub struct HardwareStateReply {
  pub state: HardwareState,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// The display backlight and the ambient light sensor. `displaySetMode` and `displaySetLevel` drive
/// the backlight, and `stateGet` reads the current state.
pub enum BridgeToClientHardwareMsg {
  #[bridge_event]
  AmbientLightUpdate(AmbientLightUpdate),
  #[bridge_event]
  BrightnessChanged(BrightnessState),
  #[bridge_response]
  StateReply(HardwareStateReply),
}
