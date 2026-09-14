use base64::Engine as _;
use libbridgething::{
  BridgeThingMeta, PhoneCallService, PhoneCallStatus,
  client::{
    AmbientLightUpdate, BridgeToClientAssetMsg, BridgeToClientAudioMsg, BridgeToClientHardwareMsg, BridgeToClientMsg,
    BridgeToClientMsgData, BridgeToClientPhoneMsg, BridgeToClientSystemMsg, VolumeChanged,
  },
  transitive_from,
};
use serde::{Deserialize, Serialize};

use crate::handler::client::{PossibleSendMsg, RecvMsgData};

mod action;
mod bluetooth;
mod configuration;
mod connection;
mod device;
pub mod interapp;
mod messages;
mod permissions;
pub mod presets;
mod settings;
mod setup;
mod version;
mod voice;

pub use action::*;
pub use bluetooth::*;
pub use configuration::*;
pub use connection::*;
pub use device::*;
pub use interapp::*;
pub use messages::*;
pub use permissions::*;
pub use settings::*;
pub use setup::*;
pub use version::*;
pub use voice::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StockRecvMsg {
  Bluetooth(StockBluetoothRecv),
  Voice(StockVoiceRecv),
  Key,
  Action(StockActionRecv),
  #[serde(rename = "settings")]
  Storage(StockStorageRecv),
  Device(StockDeviceRecv),
  Log,
}

impl From<StockRecvMsg> for RecvMsgData {
  fn from(msg: StockRecvMsg) -> Self {
    match msg {
      StockRecvMsg::Bluetooth(data) => RecvMsgData::Bluetooth(data.into()),
      StockRecvMsg::Voice(data) => RecvMsgData::Voice(data.into()),
      StockRecvMsg::Key => RecvMsgData::Hole,
      StockRecvMsg::Action(data) => data.into(),
      StockRecvMsg::Storage(data) => RecvMsgData::Store(data.into()),
      StockRecvMsg::Device(data) => data.into(),
      StockRecvMsg::Log => RecvMsgData::Hole,
    }
  }
}

#[derive(Debug, Clone, Serialize, PartialEq, derive_more::From)]
#[serde(untagged, rename_all = "camelCase")]
pub enum StockSendMsg {
  #[from]
  Bluetooth(StockBluetoothSend),
  #[from]
  Storage(StockStorageSend),
  #[from]
  Setup(StockSetupSend),
  #[from]
  Connection(StockConnectionSend),
  #[from]
  Hardware(StockHardwareSend),
  #[from]
  PhoneCall(StockPhoneCallSend),
  #[from]
  LegacyPhoneCall(StockLegacyPhoneCallSend),
  #[from]
  Permissions(StockPermissionsSend),
  #[from]
  Configuration(StockConfigurationSend),
  #[from]
  Version(StockVersionSend),
  #[from]
  Voice(StockVoiceSend),
  #[from]
  InterApp(StockInterAppSend),
  Unsupported,
}

#[derive(Debug, Default)]
pub struct StockCallSlot(std::sync::Mutex<Option<String>>);

#[derive(Debug, Default)]
pub struct StockPeerPhone(std::sync::atomic::AtomicBool);

impl StockPeerPhone {
  pub fn set(&self, phone: StockDeviceType) {
    self
      .0
      .store(phone == StockDeviceType::Ios, std::sync::atomic::Ordering::Relaxed);
  }

  pub fn get(&self) -> StockDeviceType {
    if self.0.load(std::sync::atomic::Ordering::Relaxed) {
      StockDeviceType::Ios
    } else {
      StockDeviceType::Android
    }
  }
}

pub fn server_event_to_stock(
  msg: BridgeToClientMsg,
  stock_msg_id: Option<usize>,
  call_slot: &StockCallSlot,
  phone: StockDeviceType,
) -> StockSendMsg {
  match msg.data {
    BridgeToClientMsgData::Bluetooth(data) => StockSendMsg::Bluetooth(data.into()),
    BridgeToClientMsgData::Store(data) => StockSendMsg::Storage(data.into()),
    BridgeToClientMsgData::System(data) => data.into(),
    BridgeToClientMsgData::Player(data) => StockSendMsg::InterApp(StockInterAppSend::new(stock_msg_id, data.into())),
    BridgeToClientMsgData::Hardware(BridgeToClientHardwareMsg::AmbientLightUpdate(AmbientLightUpdate {
      ambient_level,
    })) => StockSendMsg::Hardware(StockHardwareSend::AmbientLightUpdate {
      payload: (100u8.saturating_sub(ambient_level)) as usize,
    }),
    BridgeToClientMsgData::Forward(_) => {
      tracing::warn!("forward message is not supported in stock app!!");
      StockSendMsg::InterApp(StockInterAppSend::make_ack(stock_msg_id))
    }
    BridgeToClientMsgData::Error(err) => {
      tracing::warn!("typed error response is not supported in stock app: {:?}", err);
      StockSendMsg::Unsupported
    }
    BridgeToClientMsgData::Asset(data) => match data {
      BridgeToClientAssetMsg::Got(got) => {
        let image_data = base64::engine::general_purpose::STANDARD.encode(&got.bytes);
        StockSendMsg::InterApp(StockInterAppSend::new(
          stock_msg_id,
          StockInterAppSendPayload::Image {
            height: 0,
            width: 0,
            image_data,
          },
        ))
      }
      BridgeToClientAssetMsg::NotFound(_) => StockSendMsg::InterApp(StockInterAppSend::make_ack(stock_msg_id)),
      BridgeToClientAssetMsg::Ready(_) | BridgeToClientAssetMsg::Cleared(_) => StockSendMsg::Unsupported,
    },
    BridgeToClientMsgData::Phone(data) => phone_event_to_stock(data, call_slot, phone),
    BridgeToClientMsgData::Audio(data) => audio_event_to_stock(data, stock_msg_id),
    BridgeToClientMsgData::Voice(data) => match data {
      libbridgething::client::BridgeToClientVoiceMsg::Intent(intent) => {
        StockSendMsg::Voice(voice::voice_intent_to_stock(&intent))
      }
      _ => StockSendMsg::Unsupported,
    },
    BridgeToClientMsgData::Ack | BridgeToClientMsgData::Done => {
      StockSendMsg::InterApp(StockInterAppSend::make_ack(stock_msg_id))
    }

    BridgeToClientMsgData::Capabilities(_)
    | BridgeToClientMsgData::Config(_)
    | BridgeToClientMsgData::Doc(_)
    | BridgeToClientMsgData::Geo(_)
    | BridgeToClientMsgData::Hardware(_)
    | BridgeToClientMsgData::Input(_)
    | BridgeToClientMsgData::Library(_)
    | BridgeToClientMsgData::Lyrics(_)
    | BridgeToClientMsgData::Net(_)
    | BridgeToClientMsgData::Notifications(_)
    | BridgeToClientMsgData::Peer(_)
    | BridgeToClientMsgData::Time(_)
    | BridgeToClientMsgData::Webapp(_) => StockSendMsg::Unsupported,
  }
}

const STOCK_VOLUME_STEPS: u8 = 16;

fn audio_event_to_stock(event: BridgeToClientAudioMsg, stock_msg_id: Option<usize>) -> StockSendMsg {
  match event {
    BridgeToClientAudioMsg::VolumeChanged(VolumeChanged { level, muted }) => {
      let surfaced = if muted { 0.0 } else { f64::from(level).clamp(0.0, 1.0) };
      StockSendMsg::InterApp(StockInterAppSend::new(
        stock_msg_id,
        StockInterAppSendPayload::VolumeState {
          volume: surfaced,
          volume_steps: STOCK_VOLUME_STEPS,
        },
      ))
    }
    BridgeToClientAudioMsg::TtsStarted(_)
    | BridgeToClientAudioMsg::TtsEnded(_)
    | BridgeToClientAudioMsg::ErrorEvent(_) => StockSendMsg::Unsupported,
  }
}

fn phone_event_to_stock(
  msg: BridgeToClientPhoneMsg,
  call_slot: &StockCallSlot,
  phone: StockDeviceType,
) -> StockSendMsg {
  let call = match msg {
    BridgeToClientPhoneMsg::CallStarted(c) | BridgeToClientPhoneMsg::CallUpdated(c) => c,
    BridgeToClientPhoneMsg::CallEnded(ended) => libbridgething::PhoneCall {
      call_id: ended.call_id,
      remote_id: String::new(),
      display_name: String::new(),
      status: PhoneCallStatus::Disconnected,
      direction: libbridgething::PhoneCallDirection::Incoming,
      started_at_unix_s: None,
      label: None,
      address_book_id: None,
      service: None,
      is_conferenced: None,
      conference_group: None,
    },
    BridgeToClientPhoneMsg::CommunicationsChanged(_)
    | BridgeToClientPhoneMsg::StateReply(_)
    | BridgeToClientPhoneMsg::ErrorEvent(_)
    | BridgeToClientPhoneMsg::ErrorReply(_) => return StockSendMsg::Unsupported,
  };

  let mut slot = call_slot.0.lock().expect("stock call slot poisoned");
  if call.status == PhoneCallStatus::Disconnected {
    if slot.as_deref().is_some_and(|shown| shown != call.call_id) {
      return StockSendMsg::Unsupported;
    }
    *slot = None;
  } else {
    *slot = Some(call.call_id.clone());
  }
  drop(slot);

  let remote_id = if call.service == Some(PhoneCallService::Unknown) {
    call.display_name.clone()
  } else {
    call.remote_id
  };
  let remote_id = format_nanp(&remote_id);

  if phone == StockDeviceType::Android {
    return StockSendMsg::LegacyPhoneCall(StockLegacyPhoneCallSend::PhoneState {
      state: StockLegacyPhoneCallState::from_call(&call.status, &call.direction),
      phone_number: remote_id,
      display_name: call.display_name,
    });
  }

  StockSendMsg::PhoneCall(StockPhoneCallSend::PhoneCallInfo {
    remote_id,
    display_name: call.display_name,
    status: call.status.into(),
    call_dir: call.direction.into(),
    call_id: call.call_id,
  })
}

fn format_nanp(raw: &str) -> String {
  if !raw
    .chars()
    .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '(' | ')' | ' ' | '.'))
  {
    return raw.to_string();
  }
  let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
  let national = match digits.len() {
    11 if digits.starts_with('1') => &digits[1..],
    10 => &digits[..],
    _ => return raw.to_string(),
  };
  format!("({}) {}-{}", &national[..3], &national[3..6], &national[6..])
}

impl From<BridgeToClientSystemMsg> for StockSendMsg {
  fn from(value: BridgeToClientSystemMsg) -> Self {
    match value {
      BridgeToClientSystemMsg::Version(meta) => {
        let BridgeThingMeta {
          serial_number,
          os_version,
          app_version,
          image_version,
          model_name,
          fcc_id,
          ic_id,
          discord,
          credits,
          ..
        } = *meta;
        StockSendMsg::Version(StockVersionSend::Status {
          serial: serial_number,
          os_version,
          app_version,
          fw_version: image_version,
          model_name,
          fcc_id,
          ic_id,
          country: "ThingLabs".to_string(),
          discord,
          credits,
        })
      }
      BridgeToClientSystemMsg::DiagnosticsReply(_)
      | BridgeToClientSystemMsg::LogsTailReply(_)
      | BridgeToClientSystemMsg::LogsSubscribeReply(_)
      | BridgeToClientSystemMsg::LogEntry(_)
      | BridgeToClientSystemMsg::OtaProgress(_)
      | BridgeToClientSystemMsg::OtaError(_)
      | BridgeToClientSystemMsg::OtaFinished(_)
      | BridgeToClientSystemMsg::DeviceNickname(_)
      | BridgeToClientSystemMsg::DeviceNicknameChanged(_) => StockSendMsg::Unsupported,
    }
  }
}

#[cfg(test)]
mod test {
  use libbridgething::{
    PhoneCall, PhoneCallDirection, PhoneCallService, PhoneCallStatus, client::BridgeToClientPhoneMsg,
  };

  use super::{
    StockCallSlot, StockDeviceType, StockLegacyPhoneCallSend, StockLegacyPhoneCallState, StockPhoneCallSend,
    StockSendMsg, format_nanp, phone_event_to_stock,
  };

  const IOS: StockDeviceType = StockDeviceType::Ios;
  const ANDROID: StockDeviceType = StockDeviceType::Android;

  fn call(
    call_id: &str,
    remote_id: &str,
    display_name: &str,
    status: PhoneCallStatus,
    service: Option<PhoneCallService>,
  ) -> PhoneCall {
    PhoneCall {
      call_id: call_id.into(),
      remote_id: remote_id.into(),
      display_name: display_name.into(),
      status,
      direction: PhoneCallDirection::Incoming,
      started_at_unix_s: None,
      label: None,
      address_book_id: None,
      service,
      is_conferenced: None,
      conference_group: None,
    }
  }

  fn remote_id_of(msg: StockSendMsg) -> String {
    match msg {
      StockSendMsg::PhoneCall(StockPhoneCallSend::PhoneCallInfo { remote_id, .. }) => remote_id,
      other => panic!("expected phone_call_info, got {other:?}"),
    }
  }

  #[test]
  fn unknown_service_surfaces_display_name_instead_of_opaque_handle() {
    let slot = StockCallSlot::default();
    let uuid_handle = "3f8a1c2d-9b4e-4a7f-8c1d-2e6b9f0a1c34";

    let named = phone_event_to_stock(
      BridgeToClientPhoneMsg::CallUpdated(call(
        "c1",
        uuid_handle,
        "WhatsApp",
        PhoneCallStatus::Ringing,
        Some(PhoneCallService::Unknown),
      )),
      &slot,
      IOS,
    );
    assert_eq!(remote_id_of(named), "WhatsApp");

    let nameless = phone_event_to_stock(
      BridgeToClientPhoneMsg::CallUpdated(call(
        "c1",
        uuid_handle,
        "",
        PhoneCallStatus::Ringing,
        Some(PhoneCallService::Unknown),
      )),
      &slot,
      IOS,
    );
    assert_eq!(remote_id_of(nameless), "", "no name means empty number, never the uuid");
  }

  #[test]
  fn telephony_and_unreported_service_keep_the_number() {
    let slot = StockCallSlot::default();
    for service in [Some(PhoneCallService::Telephony), None] {
      let msg = phone_event_to_stock(
        BridgeToClientPhoneMsg::CallUpdated(call("c1", "+14081234567", "John", PhoneCallStatus::Active, service)),
        &slot,
        IOS,
      );
      assert_eq!(remote_id_of(msg), "(408) 123-4567");
    }
  }

  #[test]
  fn late_disconnect_for_another_call_never_reaches_stock() {
    let slot = StockCallSlot::default();
    let shown = phone_event_to_stock(
      BridgeToClientPhoneMsg::CallUpdated(call(
        "c2",
        "9154717063",
        "",
        PhoneCallStatus::Ringing,
        Some(PhoneCallService::Telephony),
      )),
      &slot,
      IOS,
    );
    assert!(matches!(shown, StockSendMsg::PhoneCall(_)));

    let stale = phone_event_to_stock(
      BridgeToClientPhoneMsg::CallUpdated(call("c1", "", "", PhoneCallStatus::Disconnected, None)),
      &slot,
      IOS,
    );
    assert_eq!(
      stale,
      StockSendMsg::Unsupported,
      "mismatched disconnect must not clobber the shown call"
    );

    let matching = phone_event_to_stock(
      BridgeToClientPhoneMsg::CallUpdated(call("c2", "", "", PhoneCallStatus::Disconnected, None)),
      &slot,
      IOS,
    );
    assert!(
      matches!(matching, StockSendMsg::PhoneCall(_)),
      "the shown call's own disconnect passes"
    );
  }

  #[test]
  fn disconnect_with_nothing_shown_passes_through() {
    let slot = StockCallSlot::default();
    let msg = phone_event_to_stock(
      BridgeToClientPhoneMsg::CallUpdated(call("c1", "", "", PhoneCallStatus::Disconnected, None)),
      &slot,
      IOS,
    );
    assert!(matches!(msg, StockSendMsg::PhoneCall(_)));
  }

  fn legacy_of(msg: StockSendMsg) -> (StockLegacyPhoneCallState, String, String) {
    match msg {
      StockSendMsg::LegacyPhoneCall(StockLegacyPhoneCallSend::PhoneState {
        state,
        phone_number,
        display_name,
      }) => (state, phone_number, display_name),
      other => panic!("expected com.spotify.superbird.phone.state, got {other:?}"),
    }
  }

  fn directed(call_id: &str, status: PhoneCallStatus, direction: PhoneCallDirection) -> PhoneCall {
    PhoneCall {
      direction,
      ..call(
        call_id,
        "+14081234567",
        "John",
        status,
        Some(PhoneCallService::Telephony),
      )
    }
  }

  #[test]
  fn android_peer_gets_the_legacy_dialect_not_phone_call_info() {
    let slot = StockCallSlot::default();
    let msg = phone_event_to_stock(
      BridgeToClientPhoneMsg::CallStarted(call(
        "c1",
        "+14081234567",
        "John",
        PhoneCallStatus::Ringing,
        Some(PhoneCallService::Telephony),
      )),
      &slot,
      ANDROID,
    );

    let (state, number, name) = legacy_of(msg);
    assert_eq!(state, StockLegacyPhoneCallState::Ringing);
    assert_eq!(
      number, "(408) 123-4567",
      "the legacy dialect still gets a formatted number"
    );
    assert_eq!(name, "John");
  }

  #[test]
  fn android_call_lifecycle_maps_onto_the_three_legacy_states() {
    let slot = StockCallSlot::default();
    let state_for = |status| {
      legacy_of(phone_event_to_stock(
        BridgeToClientPhoneMsg::CallUpdated(directed("c1", status, PhoneCallDirection::Incoming)),
        &slot,
        ANDROID,
      ))
      .0
    };

    assert_eq!(state_for(PhoneCallStatus::Ringing), StockLegacyPhoneCallState::Ringing);
    assert_eq!(state_for(PhoneCallStatus::Active), StockLegacyPhoneCallState::Offhook);
    assert_eq!(state_for(PhoneCallStatus::Held), StockLegacyPhoneCallState::Offhook);
    assert_eq!(
      state_for(PhoneCallStatus::Disconnected),
      StockLegacyPhoneCallState::Idle,
      "the android store only clears its overlay on IDLE"
    );
  }

  #[test]
  fn android_outgoing_call_stays_hidden_until_it_connects() {
    let slot = StockCallSlot::default();
    let state_for = |status| {
      legacy_of(phone_event_to_stock(
        BridgeToClientPhoneMsg::CallUpdated(directed("c1", status, PhoneCallDirection::Outgoing)),
        &slot,
        ANDROID,
      ))
      .0
    };

    assert_eq!(state_for(PhoneCallStatus::Sending), StockLegacyPhoneCallState::Idle);
    assert_eq!(state_for(PhoneCallStatus::Ringing), StockLegacyPhoneCallState::Idle);
    assert_eq!(state_for(PhoneCallStatus::Active), StockLegacyPhoneCallState::Offhook);
  }

  #[test]
  fn legacy_phone_state_serializes_the_shape_the_android_store_reads() {
    let json = serde_json::to_value(StockLegacyPhoneCallSend::PhoneState {
      state: StockLegacyPhoneCallState::Ringing,
      phone_number: "(408) 123-4567".into(),
      display_name: "John".into(),
    })
    .expect("serializes");

    assert_eq!(json["type"], "com.spotify.superbird.phone.state");
    assert_eq!(json["payload"]["state"], "RINGING");
    assert_eq!(json["payload"]["phone_number"], "(408) 123-4567");
    assert_eq!(json["payload"]["display_name"], "John");
  }

  #[test]
  fn format_nanp_covers_expected_shapes() {
    assert_eq!(format_nanp("+14081234567"), "(408) 123-4567");
    assert_eq!(format_nanp("14081234567"), "(408) 123-4567");
    assert_eq!(format_nanp("4081234567"), "(408) 123-4567");
    assert_eq!(format_nanp("9154717063"), "(915) 471-7063");
    assert_eq!(format_nanp("+442071234567"), "+442071234567");
    assert_eq!(format_nanp("123"), "123");
    assert_eq!(format_nanp(""), "");
    assert_eq!(
      format_nanp("3f8a1c2d-9b4e-4a7f-8c1d-2e6b9f0a1c34"),
      "3f8a1c2d-9b4e-4a7f-8c1d-2e6b9f0a1c34"
    );
    assert_eq!(format_nanp("a1b2c3d4e5-f6a7-8b9c-0d"), "a1b2c3d4e5-f6a7-8b9c-0d");
  }
}

transitive_from! {
  StockBluetoothSend     => PossibleSendMsg: |v| StockSendMsg::Bluetooth(v).into(),
  StockStorageSend       => PossibleSendMsg: |v| StockSendMsg::Storage(v).into(),
  StockSetupSend         => PossibleSendMsg: |v| StockSendMsg::Setup(v).into(),
  StockConnectionSend    => PossibleSendMsg: |v| StockSendMsg::Connection(v).into(),
  StockHardwareSend      => PossibleSendMsg: |v| StockSendMsg::Hardware(v).into(),
  StockPhoneCallSend     => PossibleSendMsg: |v| StockSendMsg::PhoneCall(v).into(),
  StockLegacyPhoneCallSend => PossibleSendMsg: |v| StockSendMsg::LegacyPhoneCall(v).into(),
  StockPermissionsSend   => PossibleSendMsg: |v| StockSendMsg::Permissions(v).into(),
  StockConfigurationSend => PossibleSendMsg: |v| StockSendMsg::Configuration(v).into(),
  StockVersionSend       => PossibleSendMsg: |v| StockSendMsg::Version(v).into(),
  StockVoiceSend         => PossibleSendMsg: |v| StockSendMsg::Voice(v).into(),
  StockInterAppSend      => PossibleSendMsg: |v| StockSendMsg::InterApp(v).into(),
  StockStoragePayload    => PossibleSendMsg: |payload| StockSendMsg::Storage(StockStorageSend::Response { payload }).into(),
  Vec<StockDevice>       => PossibleSendMsg: |payload| StockSendMsg::Bluetooth(StockBluetoothSend::DeviceList { payload }).into(),
}
