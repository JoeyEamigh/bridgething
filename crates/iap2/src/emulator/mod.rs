mod ea;
pub mod harness;

use bytes::{Bytes, BytesMut};
use ea::DeviceEaFlow;
pub use ea::DeviceEaStream;
use tokio::sync::mpsc;
use tokio_util::codec::{Decoder, Encoder};

use crate::{
  csm::{
    CsmCodec, CsmFrame,
    auth::{
      AuthenticationCertificate, AuthenticationResponse, AuthenticationSucceeded, RequestAuthenticationCertificate,
      RequestAuthenticationChallengeResponse,
    },
    device::{DeviceInformationUpdate, DeviceLanguageUpdate, DeviceTimeUpdate, DeviceUUIDUpdate},
    external_accessory::{RequestAppLaunch, StatusExternalAccessoryProtocolSession},
    hid::{HIDComponentUpdate, StartHID, StartNativeHID},
    identification::{
      EaProtocol, IdentificationAccepted, IdentificationInformation, StartIdentification, parse_ea_protocols,
    },
    now_playing::{MediaItemAttributes, NowPlayingUpdate, PlaybackAttributes, PlaybackState, StartNowPlayingUpdates},
  },
  error::{Error, Result},
  link::{Iap2Command, Iap2Event},
  session::{DeviceFileTransfer, EA_LINK_SESSION_ID},
};

const CONTROL_SESSION_ID: u8 = 1;
const FILE_TRANSFER_SESSION_ID: u8 = 2;
const CHALLENGE_LEN: usize = 32;

#[derive(Debug)]
pub enum EmulatorEvent {
  LinkEstablished,
  Authenticated,
  Identified,
  ArtworkSent(u8),
  AppLaunchRequested(String),
  EaStreamOpened(DeviceEaStream),
  EaStreamClosed(u16),
  LinkDown(String),
}

#[derive(Debug)]
enum EmulatorCommand {
  PushNowPlaying(Box<NowPlayingUpdate>),
  PushArtwork { transfer_id: u8, bytes: Bytes },
  OpenEaStream(String),
}

#[derive(Clone)]
pub struct DeviceEmulatorHandle {
  commands: mpsc::Sender<EmulatorCommand>,
}

impl DeviceEmulatorHandle {
  pub async fn push_now_playing(&self, update: NowPlayingUpdate) -> Result<()> {
    self
      .commands
      .send(EmulatorCommand::PushNowPlaying(Box::new(update)))
      .await
      .map_err(|_| Error::LinkClosed)
  }

  pub async fn push_artwork(&self, transfer_id: u8, bytes: Bytes) -> Result<()> {
    self
      .commands
      .send(EmulatorCommand::PushArtwork { transfer_id, bytes })
      .await
      .map_err(|_| Error::LinkClosed)
  }

  pub async fn open_ea_stream(&self, bundle_id: &str) -> Result<()> {
    self
      .commands
      .send(EmulatorCommand::OpenEaStream(bundle_id.to_owned()))
      .await
      .map_err(|_| Error::LinkClosed)
  }
}

pub struct DeviceEmulator {
  link_command_tx: mpsc::Sender<Iap2Command>,
  link_events_rx: mpsc::Receiver<Iap2Event>,
  events_tx: mpsc::Sender<EmulatorEvent>,
  challenge: Bytes,
  now_playing: Option<NowPlayingUpdate>,
  artwork: Option<(u8, Bytes)>,
  file_transfer: Option<DeviceFileTransfer>,
  ea: Option<DeviceEaFlow>,
  ea_protocols: Vec<EaProtocol>,
  now_playing_pushed: bool,
  command_tx: mpsc::Sender<EmulatorCommand>,
  command_rx: mpsc::Receiver<EmulatorCommand>,
}

impl DeviceEmulator {
  pub fn new(
    link_command_tx: mpsc::Sender<Iap2Command>,
    link_events_rx: mpsc::Receiver<Iap2Event>,
    events_tx: mpsc::Sender<EmulatorEvent>,
  ) -> Self {
    let (command_tx, command_rx) = mpsc::channel(32);
    Self {
      link_command_tx,
      link_events_rx,
      events_tx,
      challenge: Bytes::from_static(&[0x5A; CHALLENGE_LEN]),
      now_playing: Some(default_now_playing()),
      artwork: None,
      file_transfer: None,
      ea: None,
      ea_protocols: Vec::new(),
      now_playing_pushed: false,
      command_tx,
      command_rx,
    }
  }

  pub fn handle(&self) -> DeviceEmulatorHandle {
    DeviceEmulatorHandle {
      commands: self.command_tx.clone(),
    }
  }

  pub fn with_now_playing(mut self, update: NowPlayingUpdate) -> Self {
    self.now_playing = Some(update);
    self
  }

  pub fn without_now_playing(mut self) -> Self {
    self.now_playing = None;
    self
  }

  pub fn with_artwork(mut self, transfer_id: u8, bytes: Bytes) -> Self {
    self.artwork = Some((transfer_id, bytes));
    let media = self
      .now_playing
      .get_or_insert_with(default_now_playing)
      .media_item
      .get_or_insert_with(MediaItemAttributes::default);
    media.artwork_id = Some(transfer_id);
    self
  }

  pub async fn run(mut self) -> Result<()> {
    let mut control_buf = BytesMut::new();

    loop {
      while let Some(frame) = CsmCodec.decode(&mut control_buf)? {
        self.handle_csm(frame).await?;
      }

      let wake = {
        let link_events = &mut self.link_events_rx;
        let commands = &mut self.command_rx;
        tokio::select! {
          event = link_events.recv() => Wake::Link(event),
          command = commands.recv() => Wake::Command(command),
        }
      };

      match wake {
        Wake::Link(Some(Iap2Event::Established(lsp))) => {
          tracing::info!("emulator: link established; requesting accessory certificate");
          self.file_transfer = Some(DeviceFileTransfer::new(self.link_command_tx.clone(), lsp.max_len));
          self.ea = Some(DeviceEaFlow::new(self.link_command_tx.clone(), lsp.max_len));
          emit(&self.events_tx, EmulatorEvent::LinkEstablished).await;
          self.send_csm(RequestAuthenticationCertificate).await?;
        }
        Wake::Link(Some(Iap2Event::DataReceived { session_id, payload })) => match session_id {
          CONTROL_SESSION_ID => control_buf.extend_from_slice(&payload),
          FILE_TRANSFER_SESSION_ID => self.handle_file_transfer_data(payload).await?,
          EA_LINK_SESSION_ID => {
            if let Some(ea) = self.ea.as_mut() {
              ea.dispatch_link_data(payload).await;
            }
          }
          other => tracing::trace!(session_id = other, "emulator: ignoring data on unhandled session"),
        },
        Wake::Link(Some(Iap2Event::LinkRestarting { reason })) | Wake::Link(Some(Iap2Event::LinkDown(reason))) => {
          tracing::info!(reason = %reason, "emulator: link down");
          emit(&self.events_tx, EmulatorEvent::LinkDown(reason)).await;
          return Ok(());
        }
        Wake::Link(None) => {
          tracing::debug!("emulator: link events channel closed");
          return Ok(());
        }
        Wake::Command(Some(command)) => self.handle_command(command).await?,
        Wake::Command(None) => unreachable!("emulator retains a command sender via handle"),
      }
    }
  }

  async fn handle_csm(&mut self, frame: CsmFrame) -> Result<()> {
    match frame.msg_id {
      AuthenticationCertificate::CSM_MSG_ID => {
        tracing::debug!("emulator: got accessory certificate; issuing challenge (no CA validation)");
        self
          .send_csm(RequestAuthenticationChallengeResponse {
            challenge: self.challenge.clone(),
          })
          .await?;
      }
      AuthenticationResponse::CSM_MSG_ID => {
        tracing::info!("emulator: accepting signed challenge response; authentication succeeded");
        self.send_csm(AuthenticationSucceeded).await?;
        emit(&self.events_tx, EmulatorEvent::Authenticated).await;
        self.send_csm(StartIdentification).await?;
      }
      IdentificationInformation::CSM_MSG_ID => {
        match parse_ea_protocols(&frame) {
          Ok(protocols) => self.ea_protocols = protocols,
          Err(err) => tracing::warn!(?err, "emulator: failed to parse accessory EA protocols"),
        }
        tracing::info!("emulator: accepting accessory identification");
        self.send_csm(IdentificationAccepted).await?;
        emit(&self.events_tx, EmulatorEvent::Identified).await;
        self.push_device_metadata().await?;
      }
      StartNowPlayingUpdates::CSM_MSG_ID => {
        self.push_now_playing().await?;
      }
      RequestAppLaunch::CSM_MSG_ID => {
        let request = RequestAppLaunch::try_from(frame)?;
        emit(
          &self.events_tx,
          EmulatorEvent::AppLaunchRequested(request.bundle_id.clone()),
        )
        .await;
        self.open_ea_stream(&request.bundle_id).await?;
      }
      StatusExternalAccessoryProtocolSession::CSM_MSG_ID => {
        let status = StatusExternalAccessoryProtocolSession::try_from(frame)?;
        let stream_id = status.session_id;
        let closed = self.ea.as_mut().is_some_and(|ea| ea.handle_status(status));
        if closed {
          emit(&self.events_tx, EmulatorEvent::EaStreamClosed(stream_id)).await;
        }
      }
      StartHID::CSM_MSG_ID => {
        let start = StartHID::try_from(frame)?;
        tracing::debug!(component_id = start.component_id, "emulator: enabling accessory HID");
        self
          .send_csm(HIDComponentUpdate {
            component_id: start.component_id,
            component_enabled: true,
          })
          .await?;
        self.send_csm(StartNativeHID).await?;
      }
      other => {
        tracing::trace!(msg_id = format!("{other:#06x}"), "emulator: unhandled CSM");
      }
    }
    Ok(())
  }

  async fn push_now_playing(&mut self) -> Result<()> {
    if self.now_playing_pushed {
      return Ok(());
    }
    self.now_playing_pushed = true;
    let Some(update) = self.now_playing.clone() else {
      tracing::debug!("emulator: accessory subscribed; no canned NowPlaying, awaiting driven pushes");
      return Ok(());
    };
    tracing::info!("emulator: accessory subscribed; pushing NowPlaying delta");
    self.send_csm(update).await?;

    if let (Some((transfer_id, bytes)), Some(ft)) = (self.artwork.clone(), self.file_transfer.as_mut()) {
      ft.begin_artwork(transfer_id, bytes).await?;
    }
    Ok(())
  }

  async fn handle_command(&mut self, command: EmulatorCommand) -> Result<()> {
    match command {
      EmulatorCommand::PushNowPlaying(update) => {
        tracing::info!("emulator: driving NowPlaying delta");
        self.send_csm(*update).await?;
      }
      EmulatorCommand::PushArtwork { transfer_id, bytes } => match self.file_transfer.as_mut() {
        Some(ft) => {
          tracing::info!(transfer_id, "emulator: driving artwork transfer");
          ft.begin_artwork(transfer_id, bytes).await?;
        }
        None => tracing::warn!("emulator: artwork push before link established; dropping"),
      },
      EmulatorCommand::OpenEaStream(bundle_id) => {
        tracing::info!(bundle = %bundle_id, "emulator: app opening its EA stream on its own");
        self.open_ea_stream(&bundle_id).await?;
      }
    }
    Ok(())
  }

  async fn push_device_metadata(&self) -> Result<()> {
    self
      .send_csm(DeviceInformationUpdate {
        device_name: "Emulated iPhone".into(),
      })
      .await?;
    self.send_csm(DeviceLanguageUpdate { language: "en".into() }).await?;
    let now = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|d| d.as_secs() as i64)
      .unwrap_or(0);
    self
      .send_csm(DeviceTimeUpdate {
        seconds_since_reference_date: now,
        tz_offset_minutes: 0,
        dst_offset_minutes: 0,
      })
      .await?;
    self
      .send_csm(DeviceUUIDUpdate {
        uuid: "00000000-0000-4000-8000-000000000000".into(),
      })
      .await?;
    Ok(())
  }

  async fn open_ea_stream(&mut self, bundle_id: &str) -> Result<()> {
    let Some(protocol_id) = self.ea_protocols.iter().find(|p| p.name == bundle_id).map(|p| p.id) else {
      tracing::warn!(bundle = %bundle_id, "emulator: RequestAppLaunch for an undeclared EA protocol; ignoring");
      return Ok(());
    };
    let Some(ea) = self.ea.as_mut() else {
      return Ok(());
    };
    let (start, stream) = ea.open(protocol_id);
    tracing::info!(bundle = %bundle_id, protocol_id, stream_id = stream.stream_id, "emulator: opening EA gateway stream");
    self.send_csm(start).await?;
    emit(&self.events_tx, EmulatorEvent::EaStreamOpened(stream)).await;
    Ok(())
  }

  async fn handle_file_transfer_data(&mut self, payload: Bytes) -> Result<()> {
    let Some(ft) = self.file_transfer.as_mut() else {
      return Ok(());
    };
    if let Some(transfer_id) = ft.on_link_data(payload).await? {
      emit(&self.events_tx, EmulatorEvent::ArtworkSent(transfer_id)).await;
    }
    Ok(())
  }

  async fn send_csm<F: Into<CsmFrame>>(&self, csm: F) -> Result<()> {
    let mut buf = BytesMut::new();
    CsmCodec.encode(csm.into(), &mut buf)?;
    self
      .link_command_tx
      .send(Iap2Command::Send {
        session_id: CONTROL_SESSION_ID,
        payload: buf.freeze(),
      })
      .await
      .map_err(|_| Error::LinkClosed)?;
    Ok(())
  }
}

fn default_now_playing() -> NowPlayingUpdate {
  NowPlayingUpdate {
    media_item: Some(MediaItemAttributes {
      persistent_id: Some(0x0A0B_0C0D_0E0F_1011),
      title: Some("Side of Town".into()),
      album: Some("Capital Soiree".into()),
      artist: Some("Saint Blonde".into()),
      duration_ms: Some(212_000),
      ..MediaItemAttributes::default()
    }),
    playback: Some(PlaybackAttributes {
      state: Some(PlaybackState::Playing),
      position_ms: Some(41_250),
      set_elapsed_time_available: Some(true),
      app_bundle: Some("com.spotify.client".into()),
      ..PlaybackAttributes::default()
    }),
  }
}

enum Wake {
  Link(Option<Iap2Event>),
  Command(Option<EmulatorCommand>),
}

async fn emit(tx: &mpsc::Sender<EmulatorEvent>, event: EmulatorEvent) {
  if tx.send(event).await.is_err() {
    tracing::debug!("emulator: events receiver dropped");
  }
}
