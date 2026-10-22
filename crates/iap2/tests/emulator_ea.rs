#![cfg(feature = "emulator")]

use std::time::Duration;

use bridgething_iap2::{
  AppLaunch, EmulatorEvent, SessionEvent,
  csm::identification::{EaProtocol, EaProtocolMatchAction, IdentificationConfig},
  emulator::harness::{self as emu, recv_with_timeout},
};
use bytes::Bytes;
use libbridgething::Priority;
use tokio::sync::watch;

const GATEWAY_BUNDLE: &str = "com.bridgething.gateway";
const GATEWAY_EA_PROTOCOL_ID: u8 = 1;

fn gateway_identification() -> IdentificationConfig {
  let mut ident = emu::identification_config();
  ident.supported_external_accessory_protocols = vec![EaProtocol {
    id: GATEWAY_EA_PROTOCOL_ID,
    name: GATEWAY_BUNDLE.into(),
    match_action: EaProtocolMatchAction::NoAlertAction,
    native_transport_component_identifier: None,
  }];
  ident
}

fn gateway_launch(relaunch: bool) -> AppLaunch {
  AppLaunch {
    bundle_id: GATEWAY_BUNDLE.into(),
    relaunch: watch::channel(relaunch).1,
  }
}

#[tokio::test]
async fn emulator_opens_ea_gateway_stream_and_round_trips_bytes() {
  let (mut harness, mut emu_events, _emu_handle) =
    emu::spawn(gateway_identification(), Some(gateway_launch(true)), |e| e);

  let (mut acc_inbound, acc_outbound) = loop {
    match recv_with_timeout(&mut harness.acc_events, Duration::from_secs(10))
      .await
      .expect("accessory event timed out before the EA stream opened")
    {
      SessionEvent::EaStreamOpened {
        protocol_id,
        inbound_rx,
        outbound,
        ..
      } => {
        assert_eq!(
          protocol_id, GATEWAY_EA_PROTOCOL_ID,
          "accessory opens the gateway protocol"
        );
        break (inbound_rx, outbound);
      }
      _ => continue,
    }
  };

  let mut launch_requested = false;
  let mut dev_stream = loop {
    match recv_with_timeout(&mut emu_events, Duration::from_secs(10))
      .await
      .expect("emulator event timed out before the EA stream opened")
    {
      EmulatorEvent::AppLaunchRequested(bundle) => {
        assert_eq!(bundle, GATEWAY_BUNDLE);
        launch_requested = true;
      }
      EmulatorEvent::EaStreamOpened(stream) => break stream,
      _ => continue,
    }
  };
  assert!(launch_requested, "the accessory opened the app to get its stream");
  assert_eq!(
    dev_stream.protocol_id, GATEWAY_EA_PROTOCOL_ID,
    "emulator discovered the protocol id from identification"
  );

  dev_stream
    .outbound
    .send(Priority::Normal, Bytes::from_static(b"hello accessory"))
    .await
    .unwrap();
  let got = recv_with_timeout(&mut acc_inbound, Duration::from_secs(5))
    .await
    .expect("accessory never received the device's gateway bytes");
  assert_eq!(&got[..], b"hello accessory");

  acc_outbound
    .send(Priority::Normal, Bytes::from_static(b"hello device"))
    .await
    .unwrap();
  let got = recv_with_timeout(&mut dev_stream.inbound_rx, Duration::from_secs(5))
    .await
    .expect("device never received the accessory's gateway bytes");
  assert_eq!(&got[..], b"hello device");
}

#[tokio::test]
async fn shutdown_closes_the_ea_session_on_the_device() {
  let (mut harness, mut emu_events, _emu_handle) =
    emu::spawn(gateway_identification(), Some(gateway_launch(true)), |e| e);

  let dev_stream = loop {
    match recv_with_timeout(&mut emu_events, Duration::from_secs(10))
      .await
      .expect("emulator event timed out before the EA stream opened")
    {
      EmulatorEvent::EaStreamOpened(stream) => break stream,
      _ => continue,
    }
  };

  harness.shutdown_session().await;

  let closed = loop {
    match recv_with_timeout(&mut emu_events, Duration::from_secs(5))
      .await
      .expect("device never saw the EA session close before shutdown")
    {
      EmulatorEvent::EaStreamClosed(stream_id) => break stream_id,
      _ => continue,
    }
  };
  assert_eq!(
    closed, dev_stream.stream_id,
    "the device's own stream is the one that closed"
  );

  let mut saw_accessory_close = false;
  while let Some(event) = recv_with_timeout(&mut harness.acc_events, Duration::from_secs(5)).await {
    if matches!(event, SessionEvent::EaStreamClosed { stream_id } if stream_id == dev_stream.stream_id) {
      saw_accessory_close = true;
      break;
    }
  }
  assert!(saw_accessory_close, "the accessory reports its own stream closed");
}

#[tokio::test]
async fn with_relaunch_off_the_accessory_never_opens_the_app_and_still_takes_its_stream() {
  let (mut harness, mut emu_events, emu_handle) =
    emu::spawn(gateway_identification(), Some(gateway_launch(false)), |e| e);

  loop {
    match recv_with_timeout(&mut emu_events, Duration::from_secs(10))
      .await
      .expect("emulator event timed out before identification")
    {
      EmulatorEvent::AppLaunchRequested(bundle) => panic!("the accessory opened {bundle} with relaunch off"),
      EmulatorEvent::Identified => break,
      _ => continue,
    }
  }

  emu_handle
    .open_ea_stream(GATEWAY_BUNDLE)
    .await
    .expect("the app opens its own stream");

  let acc_outbound = loop {
    match recv_with_timeout(&mut harness.acc_events, Duration::from_secs(10))
      .await
      .expect("accessory event timed out before the app's stream opened")
    {
      SessionEvent::EaStreamOpened {
        protocol_id, outbound, ..
      } => {
        assert_eq!(protocol_id, GATEWAY_EA_PROTOCOL_ID);
        break outbound;
      }
      _ => continue,
    }
  };

  let mut dev_stream = loop {
    match recv_with_timeout(&mut emu_events, Duration::from_secs(10))
      .await
      .expect("emulator event timed out before the app's stream opened")
    {
      EmulatorEvent::AppLaunchRequested(bundle) => panic!("the accessory opened {bundle} with relaunch off"),
      EmulatorEvent::EaStreamOpened(stream) => break stream,
      _ => continue,
    }
  };

  acc_outbound
    .send(Priority::Normal, Bytes::from_static(b"after identification"))
    .await
    .unwrap();
  let got = recv_with_timeout(&mut dev_stream.inbound_rx, Duration::from_secs(5))
    .await
    .expect("device never received the accessory's gateway bytes");
  assert_eq!(&got[..], b"after identification");

  while let Ok(event) = emu_events.try_recv() {
    assert!(
      !matches!(event, EmulatorEvent::AppLaunchRequested(_)),
      "everything the accessory sent before these bytes has landed, and none of it opened the app"
    );
  }
}
