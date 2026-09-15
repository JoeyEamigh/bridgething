use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::LauncherGesture;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[bridge_enum(into = crate::gateway::BridgeToGatewayMsgData)]
/// Reports the M-button launcher gesture. `getGesture` reads the current
/// choice and `gestureChanged` fires on every change.
pub enum BridgeToGatewayInputMsg {
  #[bridge_event]
  GestureChanged(InputGestureChanged),
  #[bridge_response]
  GetGestureReply(InputGestureReply),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct InputGestureReply {
  pub gesture: LauncherGesture,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct InputGestureChanged {
  pub gesture: LauncherGesture,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn gesture_changed_keeps_its_wire_shape() {
    let msg = BridgeToGatewayInputMsg::GestureChanged(InputGestureChanged {
      gesture: LauncherGesture::LongPress,
    });
    let json = serde_json::to_value(msg).expect("gestureChanged serializes");
    assert_eq!(json["event"], "gestureChanged");
    assert_eq!(json["data"]["gesture"], "longPress");
    assert_eq!(
      serde_json::from_value::<BridgeToGatewayInputMsg>(json).expect("gestureChanged deserializes"),
      msg
    );
  }

  #[test]
  fn get_gesture_reply_keeps_its_wire_shape() {
    let msg = BridgeToGatewayInputMsg::GetGestureReply(InputGestureReply {
      gesture: LauncherGesture::FivePress,
    });
    let json = serde_json::to_value(msg).expect("getGestureReply serializes");
    assert_eq!(json["event"], "getGestureReply");
    assert_eq!(json["data"]["gesture"], "fivePress");
    assert_eq!(
      serde_json::from_value::<BridgeToGatewayInputMsg>(json).expect("getGestureReply deserializes"),
      msg
    );
  }
}
