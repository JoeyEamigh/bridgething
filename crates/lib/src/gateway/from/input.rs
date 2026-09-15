use bridgething_macros::{BridgeDispatch, BridgeEnum, WireRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::LauncherGesture;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct InputSetGesture {
  pub gesture: LauncherGesture,
}

#[derive(Debug, Clone, Copy, Default, WireRequest)]
#[wire_request(
  direction = GatewayToBridge,
  surface = Input,
  request_variant = GetGesture,
  response = crate::gateway::InputGestureReply,
  response_variant = GetGestureReply,
)]
pub struct InputGetGesture;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum, BridgeDispatch)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[bridge_enum(into = crate::gateway::GatewayToBridgeMsgData)]
/// Which M-button gesture jumps to the launcher. `setGesture` persists the
/// choice on the daemon; `getGesture` reads it back.
pub enum GatewayToBridgeInputMsg {
  #[bridge_command]
  SetGesture(InputSetGesture),
  #[bridge_request]
  GetGesture,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn set_gesture_keeps_its_wire_shape() {
    let msg = GatewayToBridgeInputMsg::SetGesture(InputSetGesture {
      gesture: LauncherGesture::LongPress,
    });
    let json = serde_json::to_value(msg).expect("setGesture serializes");
    assert_eq!(json["event"], "setGesture");
    assert_eq!(json["data"]["gesture"], "longPress");
    assert_eq!(
      serde_json::from_value::<GatewayToBridgeInputMsg>(json).expect("setGesture deserializes"),
      msg
    );
  }

  #[test]
  fn get_gesture_keeps_its_wire_shape() {
    let json = serde_json::to_value(GatewayToBridgeInputMsg::GetGesture).expect("getGesture serializes");
    assert_eq!(json["event"], "getGesture");
    assert_eq!(
      serde_json::from_value::<GatewayToBridgeInputMsg>(json).expect("getGesture deserializes"),
      GatewayToBridgeInputMsg::GetGesture
    );
    let reply = crate::gateway::InputGestureReply {
      gesture: LauncherGesture::FivePress,
    };
    let reply_json = serde_json::to_value(reply).expect("getGesture reply serializes");
    assert_eq!(reply_json["gesture"], "fivePress");
  }
}
