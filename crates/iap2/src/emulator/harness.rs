use std::time::Duration;

use async_trait::async_trait;
use bridgething_mfi::{CHALLENGE_LEN, Error as MfiError, RESPONSE_LEN};
use bytes::Bytes;
use tokio::{
  sync::{mpsc, oneshot},
  task::JoinHandle,
};

use crate::{
  AppLaunch, DeviceEmulator, DeviceEmulatorHandle, EmulatorEvent, Iap2Command, Iap2Session, Link, LinkConfig, Lsp,
  MfiAccess, NowPlayingAuthorityState, SessionEvent, SessionTriple,
  csm::identification::{CarthingIdentification, IdentificationConfig},
  session::{HidCommand, NowPlayingCommand, TelephonyCommand},
};

pub const ACC_PSN: u8 = 99;
pub const DEV_PSN: u8 = 215;

pub fn link_config(initial_psn: u8, lsp: Lsp) -> LinkConfig {
  let mut config = LinkConfig::new(lsp);
  config.initial_psn = initial_psn;
  config.detect_interval = Duration::from_millis(50);
  config.handshake_timeout = Duration::from_secs(5);
  config
}

pub fn device_lsp() -> Lsp {
  Lsp {
    version: 1,
    max_outgoing: 127,
    max_len: 65535,
    retransmission_timeout_ms: 6000,
    ack_timeout_ms: 3000,
    max_retransmissions: 30,
    max_ack: 3,
    sessions: vec![
      SessionTriple {
        id: 1,
        session_type: 0,
        version: 1,
      },
      SessionTriple {
        id: 2,
        session_type: 1,
        version: 2,
      },
      SessionTriple {
        id: 3,
        session_type: 2,
        version: 1,
      },
    ],
  }
}

pub fn identification_config() -> IdentificationConfig {
  IdentificationConfig::for_carthing(CarthingIdentification {
    serial_number: "BT-TEST-0001".into(),
    firmware_version: "v0.1.0".into(),
    bt_mac: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
  })
}

pub async fn recv_with_timeout<T>(rx: &mut mpsc::Receiver<T>, timeout: Duration) -> Option<T> {
  tokio::time::timeout(timeout, rx.recv()).await.ok().flatten()
}

#[derive(Clone)]
pub struct FakeMfi;

#[async_trait]
impl MfiAccess for FakeMfi {
  async fn cert(&mut self) -> Result<Bytes, MfiError> {
    Ok(Bytes::from_static(b"FAKE-MFI-CERT-DER"))
  }

  async fn sign(&mut self, _challenge: [u8; CHALLENGE_LEN]) -> Result<[u8; RESPONSE_LEN], MfiError> {
    Ok([0xAB; RESPONSE_LEN])
  }
}

pub struct EmuHarness {
  pub acc_events: mpsc::Receiver<SessionEvent>,
  session_shutdown: mpsc::Sender<oneshot::Sender<()>>,
  _keep: Keep,
}

impl EmuHarness {
  pub async fn shutdown_session(&self) {
    let (ack_tx, ack_rx) = oneshot::channel();
    self.session_shutdown.send(ack_tx).await.expect("session alive");
    tokio::time::timeout(Duration::from_secs(2), ack_rx)
      .await
      .expect("session confirmed shutdown")
      .expect("shutdown ack not dropped");
  }
}

struct Keep {
  _links: (JoinHandle<crate::Result<()>>, JoinHandle<crate::Result<()>>),
  _session: JoinHandle<crate::Result<()>>,
  _emulator: JoinHandle<crate::Result<()>>,
  _hid_tx: mpsc::Sender<HidCommand>,
  _np_tx: mpsc::Sender<NowPlayingCommand>,
  _np_authority_tx: tokio::sync::watch::Sender<NowPlayingAuthorityState>,
  _tel_tx: mpsc::Sender<TelephonyCommand>,
}

pub fn spawn<F>(
  ident: IdentificationConfig,
  app_launch: Option<AppLaunch>,
  setup: F,
) -> (EmuHarness, mpsc::Receiver<EmulatorEvent>, DeviceEmulatorHandle)
where
  F: FnOnce(DeviceEmulator) -> DeviceEmulator,
{
  let (acc_io, dev_io) = tokio::io::duplex(8192);

  let (acc_link_ev_tx, acc_link_ev_rx) = mpsc::channel(64);
  let (acc_cmd_tx, acc_cmd_rx) = mpsc::channel::<Iap2Command>(64);
  let acc_link = tokio::spawn(Link::run(
    acc_io,
    link_config(ACC_PSN, Lsp::accessory_default()),
    acc_link_ev_tx,
    acc_cmd_rx,
  ));

  let (sess_ev_tx, acc_events) = mpsc::channel(64);
  let (hid_tx, hid_rx) = mpsc::channel(8);
  let (np_tx, np_rx) = mpsc::channel(8);
  let (np_authority_tx, np_authority_rx) = tokio::sync::watch::channel(NowPlayingAuthorityState::default());
  let (tel_tx, tel_rx) = mpsc::channel(8);
  let (_app_launch_tx, app_launch_rx) = mpsc::channel(8);
  let (session_shutdown_tx, session_shutdown_rx) = mpsc::channel(1);
  let session = Iap2Session::with_app_launch(
    ident,
    app_launch,
    app_launch_rx,
    session_shutdown_rx,
    FakeMfi,
    acc_cmd_tx,
    acc_link_ev_rx,
    sess_ev_tx,
    hid_rx,
    np_rx,
    np_authority_rx,
    tel_rx,
  );
  let session = tokio::spawn(session.run());

  let (dev_link_ev_tx, dev_link_ev_rx) = mpsc::channel(64);
  let (dev_cmd_tx, dev_cmd_rx) = mpsc::channel::<Iap2Command>(64);
  let dev_link = tokio::spawn(Link::run_device(
    dev_io,
    link_config(DEV_PSN, device_lsp()),
    dev_link_ev_tx,
    dev_cmd_rx,
  ));

  let (emu_ev_tx, emu_events) = mpsc::channel(64);
  let emulator = setup(DeviceEmulator::new(dev_cmd_tx, dev_link_ev_rx, emu_ev_tx));
  let emu_handle = emulator.handle();
  let emulator = tokio::spawn(emulator.run());

  let harness = EmuHarness {
    acc_events,
    session_shutdown: session_shutdown_tx,
    _keep: Keep {
      _links: (acc_link, dev_link),
      _session: session,
      _emulator: emulator,
      _hid_tx: hid_tx,
      _np_tx: np_tx,
      _np_authority_tx: np_authority_tx,
      _tel_tx: tel_tx,
    },
  };
  (harness, emu_events, emu_handle)
}
