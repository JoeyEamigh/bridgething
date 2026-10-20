use std::{
  collections::HashMap,
  time::{Duration, Instant},
};

use bytes::Bytes;
use tokio::sync::mpsc;

use super::{
  SessionEvent,
  ea_transport::{EaChunker, split_stream_frame},
  emit, send_csm,
};
use crate::{
  csm::{
    CsmFrame,
    external_accessory::{
      AppLaunchMethod, EaSessionStatus, RequestAppLaunch, StartExternalAccessoryProtocolSession,
      StatusExternalAccessoryProtocolSession, StopExternalAccessoryProtocolSession,
    },
  },
  error::Result,
  link::Iap2Command,
};

const STREAM_INBOUND_CAPACITY: usize = 64;

const MAX_APP_LAUNCH_ATTEMPTS: u32 = 6;
const APP_LAUNCH_RETRY_COOLDOWN: Duration = Duration::from_secs(10);
const ON_DEMAND_LAUNCH_COOLDOWN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppLaunchState {
  Armed,
  Active,
}

async fn send_app_launch(bundle_id: &str, link_command_tx: &mpsc::Sender<Iap2Command>) -> Result<()> {
  send_csm(
    RequestAppLaunch {
      bundle_id: bundle_id.to_string(),
      launch_method: AppLaunchMethod::WithoutUserAlert,
    },
    link_command_tx,
  )
  .await
}

pub(super) struct EaFlow {
  streams: HashMap<u16, mpsc::Sender<Bytes>>,
  chunker: EaChunker,
  app_launch: AppLaunchState,
  relaunch_attempts: u32,
  last_launch_sent: Option<Instant>,
  last_on_demand_sent: Option<Instant>,
  accept_protocol_id: Option<u8>,
}

impl std::fmt::Debug for EaFlow {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("EaFlow")
      .field("streams", &self.streams.keys().collect::<Vec<_>>())
      .field("app_launch", &self.app_launch)
      .field("relaunch_attempts", &self.relaunch_attempts)
      .finish()
  }
}

impl EaFlow {
  pub(super) fn new(
    link_command_tx: mpsc::Sender<Iap2Command>,
    peer_max_len: u16,
    accept_protocol_id: Option<u8>,
  ) -> Self {
    Self {
      streams: HashMap::new(),
      chunker: EaChunker::new(link_command_tx, peer_max_len),
      app_launch: AppLaunchState::Armed,
      relaunch_attempts: 0,
      last_launch_sent: None,
      last_on_demand_sent: None,
      accept_protocol_id,
    }
  }

  pub(super) fn handles(msg_id: u16) -> bool {
    msg_id == StartExternalAccessoryProtocolSession::CSM_MSG_ID
      || msg_id == StopExternalAccessoryProtocolSession::CSM_MSG_ID
  }

  pub(super) async fn ensure_app_launch_requested(
    &mut self,
    bundle_id: &str,
    link_command_tx: &mpsc::Sender<Iap2Command>,
  ) -> Result<()> {
    if matches!(self.app_launch, AppLaunchState::Active) || self.relaunch_attempts >= MAX_APP_LAUNCH_ATTEMPTS {
      return Ok(());
    }
    let now = Instant::now();
    if let Some(last) = self.last_launch_sent
      && now.duration_since(last) < APP_LAUNCH_RETRY_COOLDOWN
    {
      return Ok(());
    }
    tracing::debug!(
      bundle_id,
      attempt = self.relaunch_attempts + 1,
      "iap2 ea: sending RequestAppLaunch"
    );
    send_app_launch(bundle_id, link_command_tx).await?;
    self.relaunch_attempts += 1;
    self.last_launch_sent = Some(now);
    Ok(())
  }

  pub(super) async fn request_app_launch(
    &mut self,
    bundle_id: &str,
    link_command_tx: &mpsc::Sender<Iap2Command>,
  ) -> Result<()> {
    let now = Instant::now();
    if let Some(last) = self.last_on_demand_sent
      && now.duration_since(last) < ON_DEMAND_LAUNCH_COOLDOWN
    {
      tracing::debug!(
        bundle_id,
        "iap2 ea: on-demand RequestAppLaunch within cooldown; skipping"
      );
      return Ok(());
    }
    tracing::info!(bundle_id, "iap2 ea: sending on-demand RequestAppLaunch");
    send_app_launch(bundle_id, link_command_tx).await?;
    self.last_on_demand_sent = Some(now);
    Ok(())
  }

  pub(super) async fn handle(
    &mut self,
    frame: CsmFrame,
    link_command_tx: &mpsc::Sender<Iap2Command>,
    session_events_tx: &mpsc::Sender<SessionEvent>,
  ) -> Result<Option<SessionEvent>> {
    match frame.msg_id {
      StartExternalAccessoryProtocolSession::CSM_MSG_ID => {
        let start: StartExternalAccessoryProtocolSession = frame.try_into()?;
        self.handle_start(start, link_command_tx, session_events_tx).await?;
      }
      StopExternalAccessoryProtocolSession::CSM_MSG_ID => {
        let stop: StopExternalAccessoryProtocolSession = frame.try_into()?;
        self.handle_stop(stop, session_events_tx).await;
      }
      _ => {}
    }
    Ok(None)
  }

  async fn handle_start(
    &mut self,
    start: StartExternalAccessoryProtocolSession,
    link_command_tx: &mpsc::Sender<Iap2Command>,
    session_events_tx: &mpsc::Sender<SessionEvent>,
  ) -> Result<()> {
    if let Some(accept) = self.accept_protocol_id
      && start.protocol_id != accept
    {
      tracing::info!(
        stream_id = start.session_id,
        protocol_id = start.protocol_id,
        "iap2 ea: refusing StartES for a declaration-only protocol (we never terminate its stream)"
      );
      send_csm(
        StatusExternalAccessoryProtocolSession {
          session_id: start.session_id,
          status: EaSessionStatus::Close,
        },
        link_command_tx,
      )
      .await?;
      return Ok(());
    }

    if self.streams.contains_key(&start.session_id) {
      tracing::warn!(
        stream_id = start.session_id,
        "iap2 ea: StartExternalAccessoryProtocolSession for a stream id already open; refusing"
      );
      send_csm(
        StatusExternalAccessoryProtocolSession {
          session_id: start.session_id,
          status: EaSessionStatus::Close,
        },
        link_command_tx,
      )
      .await?;
      return Ok(());
    }

    let (inbound_tx, inbound_rx) = mpsc::channel(STREAM_INBOUND_CAPACITY);
    self.streams.insert(start.session_id, inbound_tx);

    send_csm(
      StatusExternalAccessoryProtocolSession {
        session_id: start.session_id,
        status: EaSessionStatus::Ok,
      },
      link_command_tx,
    )
    .await?;
    tracing::info!(
      stream_id = start.session_id,
      protocol_id = start.protocol_id,
      "iap2 ea: stream opened"
    );

    emit(
      session_events_tx,
      SessionEvent::EaStreamOpened {
        stream_id: start.session_id,
        protocol_id: start.protocol_id,
        inbound_rx,
        outbound: self.chunker.sender(start.session_id),
      },
    )
    .await;

    self.app_launch = AppLaunchState::Active;
    self.relaunch_attempts = 0;
    Ok(())
  }

  async fn handle_stop(
    &mut self,
    stop: StopExternalAccessoryProtocolSession,
    session_events_tx: &mpsc::Sender<SessionEvent>,
  ) {
    if self.streams.remove(&stop.session_id).is_some() {
      tracing::info!(stream_id = stop.session_id, "iap2 ea: stream closed by peer");
      if self.streams.is_empty() {
        self.app_launch = AppLaunchState::Armed;
      }
      emit(
        session_events_tx,
        SessionEvent::EaStreamClosed {
          stream_id: stop.session_id,
        },
      )
      .await;
    }
  }

  pub(super) async fn teardown(
    &mut self,
    link_command_tx: Option<&mpsc::Sender<Iap2Command>>,
    session_events_tx: &mpsc::Sender<SessionEvent>,
  ) {
    for (stream_id, _inbound) in self.streams.drain() {
      if let Some(tx) = link_command_tx
        && let Err(err) = send_csm(
          StatusExternalAccessoryProtocolSession {
            session_id: stream_id,
            status: EaSessionStatus::Close,
          },
          tx,
        )
        .await
      {
        tracing::warn!(stream_id, ?err, "iap2 ea: close status send failed");
      }
      tracing::info!(stream_id, "iap2 ea: stream torn down");
      emit(session_events_tx, SessionEvent::EaStreamClosed { stream_id }).await;
    }
  }

  pub(super) async fn dispatch_link_data(&mut self, payload: Bytes, session_events_tx: &mpsc::Sender<SessionEvent>) {
    let Some((stream_id, chunk)) = split_stream_frame(&payload) else {
      tracing::warn!(
        len = payload.len(),
        "iap2 ea: link payload too short for stream-id prefix"
      );
      return;
    };
    let Some(state) = self.streams.get(&stream_id) else {
      tracing::trace!(stream_id, "iap2 ea: link payload for unknown stream id");
      return;
    };
    if state.send(chunk).await.is_err() {
      tracing::debug!(stream_id, "iap2 ea: inbound consumer dropped; closing stream");
      self.streams.remove(&stream_id);
      if self.streams.is_empty() {
        self.app_launch = AppLaunchState::Armed;
      }
      emit(session_events_tx, SessionEvent::EaStreamClosed { stream_id }).await;
    }
  }
}

#[cfg(test)]
mod tests {
  use bytes::BytesMut;

  use super::*;
  use crate::{frame::Lsp, session::ea_transport::EA_LINK_SESSION_ID};

  #[tokio::test]
  async fn flow_handles_start_stop_lifecycle() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let lsp = Lsp::accessory_default();
    let mut flow = EaFlow::new(link_tx.clone(), lsp.max_len, None);

    let start_frame: CsmFrame = StartExternalAccessoryProtocolSession {
      protocol_id: 1,
      session_id: 0x0100,
    }
    .into();
    flow.handle(start_frame, &link_tx, &events_tx).await.unwrap();

    let event = events_rx.recv().await.unwrap();
    let opened_outbound = match event {
      SessionEvent::EaStreamOpened {
        stream_id,
        protocol_id,
        outbound,
        ..
      } => {
        assert_eq!(stream_id, 0x0100);
        assert_eq!(protocol_id, 1);
        outbound
      }
      other => panic!("unexpected event: {other:?}"),
    };

    let status_cmd = link_rx.recv().await.unwrap();
    let Iap2Command::Send {
      session_id: status_session,
      ..
    } = status_cmd
    else {
      panic!("expected Send for status reply");
    };
    assert_eq!(
      status_session, 1,
      "status reply rides the control session, not the EA session"
    );

    opened_outbound
      .send(libbridgething::Priority::Normal, Bytes::from_static(&[0xCA, 0xFE]))
      .await
      .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let chunk_cmd = link_rx.recv().await.unwrap();
    let Iap2Command::Send {
      session_id: ea_session,
      payload,
    } = chunk_cmd
    else {
      panic!("expected Send for EA chunk");
    };
    assert_eq!(ea_session, EA_LINK_SESSION_ID);
    assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 0x0100);
    assert_eq!(&payload[2..], &[0xCA, 0xFE]);

    let stop_frame: CsmFrame = StopExternalAccessoryProtocolSession { session_id: 0x0100 }.into();
    flow.handle(stop_frame, &link_tx, &events_tx).await.unwrap();
    let event = events_rx.recv().await.unwrap();
    assert!(matches!(event, SessionEvent::EaStreamClosed { stream_id: 0x0100 }));
  }

  #[tokio::test]
  async fn a_torn_down_flow_stops_feeding_the_link() {
    let (link_tx, mut link_rx) = mpsc::channel(2);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut flow = EaFlow::new(link_tx.clone(), 64, None);

    let start_frame: CsmFrame = StartExternalAccessoryProtocolSession {
      protocol_id: 1,
      session_id: 0x0100,
    }
    .into();
    flow.handle(start_frame, &link_tx, &events_tx).await.unwrap();
    let outbound = match events_rx.recv().await.unwrap() {
      SessionEvent::EaStreamOpened { outbound, .. } => outbound,
      other => panic!("unexpected event: {other:?}"),
    };
    assert!(matches!(
      link_rx.recv().await.unwrap(),
      Iap2Command::Send { session_id: 1, .. }
    ));

    outbound
      .send(libbridgething::Priority::Bulk, Bytes::from(vec![0xAB; 4096]))
      .await
      .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    flow.teardown(None, &events_tx).await;
    drop(flow);
    tokio::task::yield_now().await;

    let mut backlog = 0;
    while link_rx.try_recv().is_ok() {
      backlog += 1;
    }
    assert!(backlog <= 2, "only what already sat in the link queue may remain");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(
      link_rx.try_recv().is_err(),
      "a dead flow's chunker kept pushing queued EA bytes onto the link"
    );
    assert!(
      outbound
        .send(libbridgething::Priority::Normal, Bytes::from_static(&[1]))
        .await
        .is_err(),
      "a sender for a torn-down flow must fail instead of queueing into the void"
    );
  }

  #[tokio::test]
  async fn dispatch_routes_inbound_payload_into_stream_channel() {
    let (link_tx, _link_rx) = mpsc::channel(64);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut flow = EaFlow::new(link_tx.clone(), Lsp::accessory_default().max_len, None);

    let start_frame: CsmFrame = StartExternalAccessoryProtocolSession {
      protocol_id: 1,
      session_id: 0x0100,
    }
    .into();
    flow.handle(start_frame, &link_tx, &events_tx).await.unwrap();

    let mut inbound_rx = match events_rx.recv().await.unwrap() {
      SessionEvent::EaStreamOpened { inbound_rx, .. } => inbound_rx,
      other => panic!("unexpected event: {other:?}"),
    };

    let mut wire = BytesMut::new();
    wire.extend_from_slice(&0x0100u16.to_be_bytes());
    wire.extend_from_slice(&[0xDE, 0xAD]);
    flow.dispatch_link_data(wire.freeze(), &events_tx).await;

    let chunk = inbound_rx.recv().await.unwrap();
    assert_eq!(&chunk[..], &[0xDE, 0xAD]);
  }

  #[tokio::test]
  async fn ensure_app_launch_is_idempotent() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let mut flow = EaFlow::new(link_tx.clone(), Lsp::accessory_default().max_len, None);

    flow
      .ensure_app_launch_requested("com.bridgething.gateway", &link_tx)
      .await
      .unwrap();
    flow
      .ensure_app_launch_requested("com.bridgething.gateway", &link_tx)
      .await
      .unwrap();

    let mut launches = 0;
    while let Ok(cmd) = link_rx.try_recv() {
      if matches!(cmd, Iap2Command::Send { session_id: 1, .. }) {
        launches += 1;
      }
    }
    assert_eq!(launches, 1, "RequestAppLaunch sent exactly once");
  }

  #[tokio::test]
  async fn request_app_launch_dedupes_within_cooldown() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let mut flow = EaFlow::new(link_tx.clone(), Lsp::accessory_default().max_len, Some(1));

    flow.request_app_launch("com.spotify.client", &link_tx).await.unwrap();
    flow.request_app_launch("com.spotify.client", &link_tx).await.unwrap();

    let mut launches = 0;
    while let Ok(cmd) = link_rx.try_recv() {
      if matches!(cmd, Iap2Command::Send { session_id: 1, .. }) {
        launches += 1;
      }
    }
    assert_eq!(
      launches, 1,
      "on-demand RequestAppLaunch deduped within the cooldown window"
    );
  }

  #[tokio::test]
  async fn refuses_start_for_declaration_only_protocol() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut flow = EaFlow::new(link_tx.clone(), Lsp::accessory_default().max_len, Some(1));

    let start: CsmFrame = StartExternalAccessoryProtocolSession {
      protocol_id: 2,
      session_id: 0x0200,
    }
    .into();
    flow.handle(start, &link_tx, &events_tx).await.unwrap();

    assert!(matches!(
      link_rx.recv().await.unwrap(),
      Iap2Command::Send { session_id: 1, .. }
    ));
    assert!(flow.streams.is_empty(), "refused protocol must not open a stream");
    assert!(
      events_rx.try_recv().is_err(),
      "no EaStreamOpened emitted for a refused protocol"
    );
    assert!(matches!(flow.app_launch, AppLaunchState::Armed));
  }

  #[tokio::test]
  async fn app_launch_rearms_after_stream_close() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut flow = EaFlow::new(link_tx.clone(), Lsp::accessory_default().max_len, None);

    let start: CsmFrame = StartExternalAccessoryProtocolSession {
      protocol_id: 1,
      session_id: 0x0100,
    }
    .into();
    flow.handle(start, &link_tx, &events_tx).await.unwrap();
    assert!(matches!(
      events_rx.recv().await.unwrap(),
      SessionEvent::EaStreamOpened { stream_id: 0x0100, .. }
    ));
    assert!(matches!(
      link_rx.recv().await.unwrap(),
      Iap2Command::Send { session_id: 1, .. }
    ));
    flow
      .ensure_app_launch_requested("com.bridgething.gateway", &link_tx)
      .await
      .unwrap();
    assert!(link_rx.try_recv().is_err(), "no relaunch while the EA stream is open");

    let stop: CsmFrame = StopExternalAccessoryProtocolSession { session_id: 0x0100 }.into();
    flow.handle(stop, &link_tx, &events_tx).await.unwrap();
    assert!(matches!(
      events_rx.recv().await.unwrap(),
      SessionEvent::EaStreamClosed { stream_id: 0x0100 }
    ));
    flow
      .ensure_app_launch_requested("com.bridgething.gateway", &link_tx)
      .await
      .unwrap();
    assert!(
      matches!(link_rx.try_recv(), Ok(Iap2Command::Send { session_id: 1, .. })),
      "RequestAppLaunch re-requested after the companion stream closed"
    );
  }

  #[tokio::test]
  async fn app_launch_rearms_after_consumer_drop() {
    let (link_tx, mut link_rx) = mpsc::channel(64);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut flow = EaFlow::new(link_tx.clone(), Lsp::accessory_default().max_len, None);

    let start: CsmFrame = StartExternalAccessoryProtocolSession {
      protocol_id: 1,
      session_id: 0x0100,
    }
    .into();
    flow.handle(start, &link_tx, &events_tx).await.unwrap();
    let inbound_rx = match events_rx.recv().await.unwrap() {
      SessionEvent::EaStreamOpened { inbound_rx, .. } => inbound_rx,
      other => panic!("unexpected event: {other:?}"),
    };
    assert!(matches!(
      link_rx.recv().await.unwrap(),
      Iap2Command::Send { session_id: 1, .. }
    ));

    drop(inbound_rx);
    let mut wire = BytesMut::new();
    wire.extend_from_slice(&0x0100u16.to_be_bytes());
    wire.extend_from_slice(&[0xDE, 0xAD]);
    flow.dispatch_link_data(wire.freeze(), &events_tx).await;
    assert!(matches!(
      events_rx.recv().await.unwrap(),
      SessionEvent::EaStreamClosed { stream_id: 0x0100 }
    ));

    flow
      .ensure_app_launch_requested("com.bridgething.gateway", &link_tx)
      .await
      .unwrap();
    assert!(
      matches!(link_rx.try_recv(), Ok(Iap2Command::Send { session_id: 1, .. })),
      "a dropped inbound consumer must re-arm the relaunch"
    );
  }
}
