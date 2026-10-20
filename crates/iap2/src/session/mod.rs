mod auth;
mod device;
mod ea_transport;
mod external_accessory;
mod file_transfer;
mod hid;
mod identification;
mod mfi_worker;
mod now_playing;
mod telephony;

use async_trait::async_trait;
use auth::AuthFlow;
use bridgething_mfi::{CHALLENGE_LEN, Error as MfiError, RESPONSE_LEN};
use bytes::{Bytes, BytesMut};
use device::DeviceFlow;
#[cfg(feature = "emulator")]
pub(crate) use ea_transport::{EA_LINK_SESSION_ID, EaChunker, split_stream_frame};
pub use ea_transport::{EaSendError, EaStreamSender};
use external_accessory::EaFlow;
#[cfg(feature = "emulator")]
pub(crate) use file_transfer::DeviceFileTransfer;
use file_transfer::FileTransferFlow;
pub use hid::HidCommand;
use hid::HidFlow;
use identification::IdentificationFlow;
pub use mfi_worker::{MfiHandle, WorkerMfiAccess};
use now_playing::NowPlayingFlow;
pub use now_playing::{NowPlayingAuthorityState, NowPlayingCommand};
pub use telephony::TelephonyCommand;
use telephony::TelephonyFlow;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::codec::{Decoder, Encoder};

use crate::{
  csm::{
    CsmCodec, CsmFrame,
    device::{DeviceInformationUpdate, DeviceLanguageUpdate, DeviceTimeUpdate, DeviceUUIDUpdate},
    identification::IdentificationConfig,
    telephony::{CallStateUpdate, CommunicationsUpdate},
  },
  error::{Error, Result},
  frame::Lsp,
  link::{Iap2Command, Iap2Event, sleep_until_or_pending},
};

pub type MfiResult<T> = std::result::Result<T, MfiError>;

#[async_trait]
pub trait MfiAccess: Send + 'static {
  async fn cert(&mut self) -> MfiResult<Bytes>;
  async fn sign(&mut self, challenge: [u8; CHALLENGE_LEN]) -> MfiResult<[u8; RESPONSE_LEN]>;
}

pub(crate) const CONTROL_SESSION_ID: u8 = 1;

#[derive(Debug)]
pub enum SessionEvent {
  LinkEstablished(Lsp),
  LinkRestarting(String),
  Authenticated,
  Identified,
  AuthFailed,
  IdentificationRejected {
    rejected_params: Vec<u16>,
  },
  NowPlayingUpdate(crate::csm::now_playing::NowPlayingUpdate),
  CallStateUpdate(CallStateUpdate),
  CommunicationsUpdate(CommunicationsUpdate),
  DeviceName(DeviceInformationUpdate),
  DeviceLanguage(DeviceLanguageUpdate),
  DeviceTime(DeviceTimeUpdate),
  DeviceUuid(DeviceUUIDUpdate),
  EaStreamOpened {
    stream_id: u16,
    protocol_id: u8,
    inbound_rx: mpsc::Receiver<Bytes>,
    outbound: EaStreamSender,
  },
  EaStreamClosed {
    stream_id: u16,
  },
  ArtworkBytes {
    transfer_id: u8,
    bytes: Bytes,
  },
  QueueSnapshotBytes {
    transfer_id: u8,
    bytes: Bytes,
  },
  LinkDown(String),
}

pub struct Iap2Session<M: MfiAccess> {
  identification: IdentificationConfig,
  app_launch_bundle_id: Option<String>,
  app_launch_command_rx: mpsc::Receiver<String>,
  shutdown_rx: mpsc::Receiver<oneshot::Sender<()>>,
  mfi: M,
  link_command_tx: mpsc::Sender<Iap2Command>,
  link_events_rx: mpsc::Receiver<Iap2Event>,
  session_events_tx: mpsc::Sender<SessionEvent>,
  auth: AuthFlow,
  ident: IdentificationFlow,
  now_playing: NowPlayingFlow,
  now_playing_authority: watch::Receiver<NowPlayingAuthorityState>,
  ea: Option<EaFlow>,
  file_transfer: FileTransferFlow,
  hid: HidFlow,
  telephony: TelephonyFlow,
  device: DeviceFlow,
}

impl<M: MfiAccess> Iap2Session<M> {
  #[allow(clippy::too_many_arguments)]
  pub fn new(
    identification: IdentificationConfig,
    mfi: M,
    link_command_tx: mpsc::Sender<Iap2Command>,
    link_events_rx: mpsc::Receiver<Iap2Event>,
    session_events_tx: mpsc::Sender<SessionEvent>,
    hid_command_rx: mpsc::Receiver<HidCommand>,
    now_playing_command_rx: mpsc::Receiver<NowPlayingCommand>,
    now_playing_authority: watch::Receiver<NowPlayingAuthorityState>,
    telephony_command_rx: mpsc::Receiver<TelephonyCommand>,
  ) -> Self {
    let (_app_launch_tx, app_launch_command_rx) = mpsc::channel(1);
    let (_shutdown_tx, shutdown_rx) = mpsc::channel(1);
    Self::with_app_launch(
      identification,
      None,
      app_launch_command_rx,
      shutdown_rx,
      mfi,
      link_command_tx,
      link_events_rx,
      session_events_tx,
      hid_command_rx,
      now_playing_command_rx,
      now_playing_authority,
      telephony_command_rx,
    )
  }

  #[allow(clippy::too_many_arguments)]
  pub fn with_app_launch(
    identification: IdentificationConfig,
    app_launch_bundle_id: Option<String>,
    app_launch_command_rx: mpsc::Receiver<String>,
    shutdown_rx: mpsc::Receiver<oneshot::Sender<()>>,
    mfi: M,
    link_command_tx: mpsc::Sender<Iap2Command>,
    link_events_rx: mpsc::Receiver<Iap2Event>,
    session_events_tx: mpsc::Sender<SessionEvent>,
    hid_command_rx: mpsc::Receiver<HidCommand>,
    now_playing_command_rx: mpsc::Receiver<NowPlayingCommand>,
    now_playing_authority: watch::Receiver<NowPlayingAuthorityState>,
    telephony_command_rx: mpsc::Receiver<TelephonyCommand>,
  ) -> Self {
    Self {
      auth: AuthFlow::new(),
      ident: IdentificationFlow::new(),
      now_playing: NowPlayingFlow::new(now_playing_command_rx),
      now_playing_authority,
      ea: None,
      file_transfer: FileTransferFlow::new(link_command_tx.clone()),
      hid: HidFlow::new(hid_command_rx),
      telephony: TelephonyFlow::new(telephony_command_rx),
      device: DeviceFlow::new(),
      identification,
      app_launch_bundle_id,
      app_launch_command_rx,
      shutdown_rx,
      mfi,
      link_command_tx,
      link_events_rx,
      session_events_tx,
    }
  }

  pub async fn run(mut self) -> Result<()> {
    match self.run_inner().await {
      Ok(()) => Ok(()),
      Err(err) => {
        tracing::error!(?err, "iap2 session ended in error");
        self.disconnect_link().await;
        emit(
          &self.session_events_tx,
          SessionEvent::LinkDown(format!("session error: {err}")),
        )
        .await;
        Err(err)
      }
    }
  }

  async fn run_inner(&mut self) -> Result<()> {
    let mut control_buf = BytesMut::new();
    let mut np_authority_last = NowPlayingAuthorityState::default();
    let mut np_authority_closed = false;
    let mut app_launch_closed = false;
    let mut shutdown_closed = false;

    loop {
      while let Some(frame) = CsmCodec.decode(&mut control_buf)? {
        if let Some(reason) = self.handle_csm(frame).await? {
          self.disconnect_link().await;
          emit(&self.session_events_tx, reason).await;
        }
        if self.ident.is_accepted() {
          self.now_playing.ensure_subscribed(&self.link_command_tx).await?;
          self.hid.ensure_started(&self.link_command_tx).await?;
          self.telephony.ensure_subscribed(&self.link_command_tx).await?;
          if let (Some(ea), Some(bundle)) = (self.ea.as_mut(), self.app_launch_bundle_id.as_deref()) {
            ea.ensure_app_launch_requested(bundle, &self.link_command_tx).await?;
          }
        }
      }

      let np_authority = self.now_playing_authority.borrow_and_update().clone();
      if np_authority != np_authority_last {
        self
          .now_playing
          .reconcile_companion(&np_authority, &self.link_command_tx)
          .await?;
        np_authority_last = np_authority;
      }
      let np_wakeup = self.now_playing.next_wakeup();

      tokio::select! {
        biased;
        link_event = self.link_events_rx.recv() => match link_event {
          Some(Iap2Event::Established(lsp)) => {
            tracing::debug!("iap2 session: link established");
            if self.ea.is_none() {
              let accept_id = self.app_launch_bundle_id.as_deref().and_then(|bundle| {
                self
                  .identification
                  .supported_external_accessory_protocols
                  .iter()
                  .find(|p| p.name == bundle)
                  .map(|p| p.id)
              });
              self.ea = Some(EaFlow::new(self.link_command_tx.clone(), lsp.max_len, accept_id));
            }
            emit(&self.session_events_tx, SessionEvent::LinkEstablished(lsp)).await;
          }
          Some(Iap2Event::DataReceived { session_id, payload }) => {
            if session_id == CONTROL_SESSION_ID {
              control_buf.extend_from_slice(&payload);
            } else if session_id == ea_transport::EA_LINK_SESSION_ID {
              if let Some(ea) = self.ea.as_mut() {
                ea.dispatch_link_data(payload, &self.session_events_tx).await;
              } else {
                tracing::warn!("iap2 session: EA data received before link Established");
              }
            } else if session_id == file_transfer::FILE_TRANSFER_LINK_SESSION_ID {
              if let Err(err) = self
                .file_transfer
                .dispatch_link_data(payload, &self.session_events_tx)
                .await
              {
                tracing::warn!(?err, "iap2 session: file transfer dispatch error");
              }
            } else {
              tracing::trace!(session_id, "iap2 session: ignoring data on non-control session");
            }
          }
          Some(Iap2Event::LinkRestarting { reason }) => {
            tracing::info!(reason = %reason, "iap2 session: link restarting; resetting per-link state");
            control_buf.clear();
            self.auth = AuthFlow::new();
            self.ident = IdentificationFlow::new();
            self.device = DeviceFlow::new();
            self.file_transfer.reset();
            self.now_playing.reset();
            self.hid.reset();
            self.telephony.reset();
            if let Some(mut ea) = self.ea.take() {
              ea.teardown(None, &self.session_events_tx).await;
            }
            emit(&self.session_events_tx, SessionEvent::LinkRestarting(reason)).await;
          }
          Some(Iap2Event::LinkDown(reason)) => {
            tracing::info!(reason = %reason, "iap2 session: link down");
            let _ = self.hid.shutdown(&self.link_command_tx).await;
            emit(&self.session_events_tx, SessionEvent::LinkDown(reason)).await;
            return Ok(());
          }
          None => {
            tracing::debug!("iap2 session: link events channel closed");
            emit(
              &self.session_events_tx,
              SessionEvent::LinkDown("link task exited".into()),
            )
            .await;
            return Ok(());
          }
        },
        Some(hid_cmd) = self.hid.recv() => {
          if let Err(err) = self.hid.handle_command(hid_cmd, &self.link_command_tx).await {
            tracing::warn!(?err, "iap2 session: hid command dispatch failed");
          }
        }
        Some(np_cmd) = self.now_playing.recv() => {
          if let Err(err) = self.now_playing.handle_command(np_cmd, &self.link_command_tx).await {
            tracing::warn!(?err, "iap2 session: now-playing command dispatch failed");
          }
        }
        Some(tel_cmd) = self.telephony.recv() => {
          if let Err(err) = self.telephony.handle_command(tel_cmd, &self.link_command_tx).await {
            tracing::warn!(?err, "iap2 session: telephony command dispatch failed");
          }
        }
        maybe_bundle = self.app_launch_command_rx.recv(), if !app_launch_closed => match maybe_bundle {
          Some(bundle) => {
            if let Some(ea) = self.ea.as_mut() {
              if let Err(err) = ea.request_app_launch(&bundle, &self.link_command_tx).await {
                tracing::warn!(?err, "iap2 session: on-demand app launch dispatch failed");
              }
            } else {
              tracing::warn!(bundle, "iap2 session: on-demand app launch before link Established; dropping");
            }
          }
          None => app_launch_closed = true,
        },
        maybe_ack = self.shutdown_rx.recv(), if !shutdown_closed => match maybe_ack {
          Some(ack) => {
            tracing::info!("iap2 session: closing external accessory sessions for shutdown");
            if let Some(ea) = self.ea.as_mut() {
              ea.teardown(Some(&self.link_command_tx), &self.session_events_tx).await;
            }
            let _ = ack.send(());
            return Ok(());
          }
          None => shutdown_closed = true,
        },
        res = self.now_playing_authority.changed(), if !np_authority_closed => {
          if res.is_err() {
            np_authority_closed = true;
          }
        }
        _ = sleep_until_or_pending(np_wakeup) => {
          self
            .now_playing
            .reconcile_companion(&np_authority_last, &self.link_command_tx)
            .await?;
        }
      }
    }
  }

  async fn handle_csm(&mut self, frame: CsmFrame) -> Result<Option<SessionEvent>> {
    let msg_id = frame.msg_id;
    if AuthFlow::handles(msg_id) {
      return self
        .auth
        .handle(frame, &mut self.mfi, &self.link_command_tx, &self.session_events_tx)
        .await;
    }
    if IdentificationFlow::handles(msg_id) {
      if !self.auth.is_authenticated() && msg_id == crate::csm::identification::StartIdentification::CSM_MSG_ID {
        tracing::warn!("iap2 session: StartIdentification before AuthenticationSucceeded");
      }
      return self
        .ident
        .handle(
          frame,
          &self.identification,
          &self.link_command_tx,
          &self.session_events_tx,
        )
        .await;
    }
    if NowPlayingFlow::handles(msg_id) {
      let authority = self.now_playing_authority.borrow().clone();
      return self
        .now_playing
        .handle(frame, &authority, &self.link_command_tx, &self.session_events_tx)
        .await;
    }
    if EaFlow::handles(msg_id) {
      if let Some(ea) = self.ea.as_mut() {
        return ea.handle(frame, &self.link_command_tx, &self.session_events_tx).await;
      }
      tracing::warn!(
        msg_id = format!("{msg_id:#06x}"),
        "iap2 session: EA CSM before link Established"
      );
      return Ok(None);
    }
    if TelephonyFlow::handles(msg_id) {
      return self.telephony.handle(frame, &self.session_events_tx).await;
    }
    if DeviceFlow::handles(msg_id) {
      return self.device.handle(frame, &self.session_events_tx).await;
    }
    if HidFlow::handles(msg_id) {
      return self.hid.handle(frame, &self.session_events_tx).await;
    }
    tracing::trace!(msg_id = format!("{msg_id:#06x}"), "iap2 session: unhandled CSM");
    Ok(None)
  }

  async fn disconnect_link(&self) {
    if self.link_command_tx.send(Iap2Command::Disconnect).await.is_err() {
      tracing::debug!("iap2 session: link command channel closed before Disconnect could be sent");
    }
  }
}

pub(super) async fn send_csm<F>(csm: F, link_command_tx: &mpsc::Sender<Iap2Command>) -> Result<()>
where
  F: Into<CsmFrame>,
{
  let frame: CsmFrame = csm.into();
  let mut buf = BytesMut::new();
  CsmCodec.encode(frame, &mut buf)?;
  link_command_tx
    .send(Iap2Command::Send {
      session_id: CONTROL_SESSION_ID,
      payload: buf.freeze(),
    })
    .await
    .map_err(|_| Error::LinkClosed)?;
  Ok(())
}

pub(super) async fn emit(tx: &mpsc::Sender<SessionEvent>, event: SessionEvent) {
  if tx.send(event).await.is_err() {
    tracing::debug!("iap2 session: events receiver dropped");
  }
}
