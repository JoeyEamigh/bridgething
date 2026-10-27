use bridgething_macros::WireEvent;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const LIBBRIDGETHING_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Identity and build information for the device.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, WireEvent)]
#[wire(BridgeToGateway)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct BridgeThingMeta {
  pub bridgething_version: String,
  pub libbridgething_version: String,
  pub app_name: String,
  pub nickname: Option<String>,
  /// The adapter's bluetooth alias, the name phones see when pairing. Absent where the daemon has no radio.
  #[ts(optional)]
  pub bluetooth_name: Option<String>,
  #[serde(default, deserialize_with = "launcher_gesture_or_default")]
  pub launcher_gesture: LauncherGesture,
  /// Whether the device opens the companion app when an iPhone reconnects without it running.
  #[serde(default = "companion_launch_default")]
  pub companion_launch: bool,
  pub app_version: String,
  pub daemon_sha256: Option<String>,
  /// Null when no wake word model is loaded, or the loaded model carries no version.
  pub wakeword_model_version: Option<String>,
  pub os_name: String,
  pub os_version: String,
  pub os_description: String,
  pub bt_mac: String,
  pub serial_number: String,
  pub fcc_id: String,
  pub ic_id: String,
  pub model_name: String,
  pub channel: String,
  pub image_variant: String,
  pub image_version: String,
  pub image_build_id: String,
  pub image_build_date: String,
  pub image_distro: String,
  pub image_machine: String,
  pub discord: String,
  pub credits: String,
}

fn companion_launch_default() -> bool {
  true
}

impl BridgeThingMeta {
  pub fn libbridgething_version() -> String {
    format!("v{}", LIBBRIDGETHING_VERSION)
  }
}

/// Which M-button gesture jumps to the launcher.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum LauncherGesture {
  LongPress,
  #[default]
  FivePress,
  DoublePress,
}

pub(crate) fn launcher_gesture_or_default<'de, D: serde::Deserializer<'de>>(d: D) -> Result<LauncherGesture, D::Error> {
  #[derive(Deserialize)]
  #[serde(untagged)]
  enum Reported {
    Known(LauncherGesture),
    Unknown(serde::de::IgnoredAny),
  }
  Ok(match Reported::deserialize(d)? {
    Reported::Known(gesture) => gesture,
    Reported::Unknown(_) => LauncherGesture::default(),
  })
}

/// What an update installs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum OtaKind {
  Image,
  Daemon,
  BuiltinWebapp,
  InstalledWebapp,
  WakewordModel,
}

/// Each kind emits the phases that apply to it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum OtaPhase {
  Streaming,
  Verifying,
  Writing,
  Confirming,
  Reboot,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct OtaProgress {
  pub phase: OtaPhase,
  /// 0 to 100, within the current phase.
  pub percent: u8,
  pub step: u8,
  pub nsteps: u8,
  /// Mirrors `percent`.
  #[serde(default)]
  pub dwl_percent: u8,
  /// Always 0.
  #[serde(default)]
  pub dwl_bytes: u32,
  pub eta_ms: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub enum OtaErrorCode {
  /// The update id does not match an update the device has begun.
  UnknownUpdate,
  /// A fragment arrived at an offset the device was not expecting.
  OffsetMismatch,
  /// The transferred bytes do not match the declared sha256.
  HashMismatch,
  /// The transferred bytes do not match the declared size.
  SizeMismatch,
  Cancelled,
  /// The device rejected the payload while writing it.
  WriteFailed,
  /// The update wrote successfully. The device could not mark the new slot bootable.
  ConfirmFailed,
  /// An unexpected failure.
  Internal,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct OtaError {
  pub code: OtaErrorCode,
  pub msg: String,
  /// A resume of the same artifact reuses the id.
  pub update_id: Option<String>,
  /// The device is redelivering a failure the phone missed.
  #[serde(default, skip_serializing_if = "std::ops::Not::not")]
  pub replayed: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct OtaFinished {
  pub kind: OtaKind,
  pub update_id: String,
}

/// `start` is inclusive, `start + length` is exclusive.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct RangeSpec {
  pub start: u32,
  pub length: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct RangePart {
  pub start: u32,
  pub length: u32,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_meta_from_a_daemon_without_the_companion_launch_setting_reads_as_launching() {
    let meta: BridgeThingMeta = serde_json::from_str(
      r#"{"bridgethingVersion":"v0.13.1","libbridgethingVersion":"v0.13.1","appName":"bridgething",
          "nickname":null,"launcherGesture":"fivePress","appVersion":"0.13.1","daemonSha256":null,
          "wakewordModelVersion":null,"osName":"","osVersion":"","osDescription":"","btMac":"","serialNumber":"",
          "fccId":"","icId":"","modelName":"","channel":"","imageVariant":"","imageVersion":"","imageBuildId":"",
          "imageBuildDate":"","imageDistro":"","imageMachine":"","discord":"","credits":""}"#,
    )
    .expect("an older daemon's meta still decodes");
    assert!(meta.companion_launch, "older daemons always open the companion app");
  }

  fn meta_reporting(gesture: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
      "bridgethingVersion": "v0.15.0", "libbridgethingVersion": "v0.15.0", "appName": "bridgething",
      "nickname": null, "launcherGesture": gesture, "companionLaunch": true, "appVersion": "0.15.0",
      "daemonSha256": null, "wakewordModelVersion": null, "osName": "", "osVersion": "", "osDescription": "",
      "btMac": "", "serialNumber": "", "fccId": "", "icId": "", "modelName": "", "channel": "", "imageVariant": "",
      "imageVersion": "", "imageBuildId": "", "imageBuildDate": "", "imageDistro": "", "imageMachine": "",
      "discord": "", "credits": ""
    })
  }

  #[test]
  fn a_meta_reporting_a_gesture_this_build_does_not_know_still_decodes() {
    for unknown in [serde_json::json!("pressThrice"), serde_json::json!({ "presses": 3 })] {
      let packed = rmp_serde::to_vec_named(&meta_reporting(unknown.clone())).expect("encode meta");
      let meta: BridgeThingMeta = rmp_serde::from_slice(&packed).expect("the frame survives an unknown gesture");
      assert_eq!(meta.launcher_gesture, LauncherGesture::default(), "{unknown}");
      assert_eq!(meta.app_version, "0.15.0");
    }
  }

  #[test]
  fn every_known_gesture_survives_the_lenient_decode() {
    for gesture in [
      LauncherGesture::LongPress,
      LauncherGesture::FivePress,
      LauncherGesture::DoublePress,
    ] {
      let reported = serde_json::to_value(gesture).expect("encode gesture");
      let packed = rmp_serde::to_vec_named(&meta_reporting(reported)).expect("encode meta");
      let meta: BridgeThingMeta = rmp_serde::from_slice(&packed).expect("decode meta");
      assert_eq!(meta.launcher_gesture, gesture);

      let reply: crate::gateway::LauncherGestureReply =
        serde_json::from_value(serde_json::json!({ "gesture": gesture })).expect("decode gateway reply");
      assert_eq!(reply.gesture, gesture);
    }
  }

  #[test]
  fn a_gesture_reply_naming_an_unknown_gesture_still_decodes() {
    let packed = rmp_serde::to_vec_named(&serde_json::json!({ "gesture": "pressThrice" })).expect("encode reply");
    let gateway: crate::gateway::LauncherGestureReply = rmp_serde::from_slice(&packed).expect("gateway reply decodes");
    assert_eq!(gateway.gesture, LauncherGesture::default());
    let client: crate::client::LauncherGestureReply =
      serde_json::from_str(r#"{"gesture":"pressThrice"}"#).expect("client reply decodes");
    assert_eq!(client.gesture, LauncherGesture::default());
  }

  #[test]
  fn setting_an_unknown_gesture_is_refused() {
    let set = serde_json::from_str::<crate::gateway::LauncherGestureSet>(r#"{"gesture":"pressThrice"}"#);
    assert!(set.is_err(), "a set names exactly what to store, so it stays strict");
  }
}
