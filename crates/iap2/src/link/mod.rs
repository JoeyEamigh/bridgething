mod established;

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use established::{EstablishedState, METER_INTERVAL, RETRANSMIT_GIVE_UP};
use tokio::{
  io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
  sync::mpsc,
  time::Instant,
};
use tokio_util::codec::{Decoder, Encoder};

use crate::{
  error::{Error, Result},
  frame::{ControlBits, DETECT_MARKER, LINK_MAGIC, LinkCodec, LinkPacket, Lsp},
};

const READ_CAPACITY: usize = 16384;
const RESTART_BUDGET_WINDOW: Duration = Duration::from_secs(30);
const RESTART_HEALTHY_RESET: Duration = Duration::from_secs(60);
const RESTART_PAUSE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct LinkConfig {
  pub initial_psn: u8,
  pub our_lsp: Lsp,
  pub detect_interval: Duration,
  pub handshake_timeout: Duration,
  pub retransmit_give_up: Duration,
}

impl LinkConfig {
  pub fn new(our_lsp: Lsp) -> Self {
    Self {
      initial_psn: 99,
      our_lsp,
      detect_interval: Duration::from_secs(1),
      handshake_timeout: Duration::from_secs(30),
      retransmit_give_up: RETRANSMIT_GIVE_UP,
    }
  }
}

#[derive(Debug, Clone)]
pub enum Iap2Event {
  Established(Lsp),
  LinkRestarting { reason: String },
  LinkDown(String),
  DataReceived { session_id: u8, payload: Bytes },
}

#[derive(Debug, Clone)]
pub enum Iap2Command {
  Disconnect,
  Send { session_id: u8, payload: Bytes },
}

pub struct Link;

impl Link {
  pub async fn run<S>(
    stream: S,
    config: LinkConfig,
    events_tx: mpsc::Sender<Iap2Event>,
    mut commands_rx: mpsc::Receiver<Iap2Command>,
  ) -> Result<()>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut buf = BytesMut::with_capacity(READ_CAPACITY);
    let mut codec = LinkCodec;
    let mut restart_window: Option<Instant> = None;

    loop {
      let handshake = async {
        Self::detect_phase(&mut reader, &mut writer, &mut buf, &config).await?;
        Self::negotiate_phase(&mut reader, &mut writer, &mut buf, &mut codec, &config).await
      };
      let (peer_lsp, peer_initial_psn) = match handshake.await {
        Ok(negotiated) => negotiated,
        Err(Error::PeerReset) if restart_window.is_some_and(|s| s.elapsed() < RESTART_BUDGET_WINDOW) => {
          buf.clear();
          tokio::time::sleep(RESTART_PAUSE).await;
          continue;
        }
        Err(err) => {
          if restart_window.is_some() {
            let _ = events_tx
              .send(Iap2Event::LinkDown(format!("restart failed: {err}")))
              .await;
          }
          return Err(err);
        }
      };

      if events_tx.send(Iap2Event::Established(peer_lsp.clone())).await.is_err() {
        tracing::debug!("iap2 events receiver dropped before Established could be delivered");
      }
      tracing::info!(
        max_outgoing = peer_lsp.max_outgoing,
        max_len = peer_lsp.max_len,
        rt_ms = peer_lsp.retransmission_timeout_ms,
        ack_ms = peer_lsp.ack_timeout_ms,
        max_ack = peer_lsp.max_ack,
        "iap2 link Established"
      );

      let mut state = EstablishedState::new(
        config.initial_psn,
        peer_initial_psn,
        &peer_lsp,
        config.retransmit_give_up,
      );
      let established_at = Instant::now();
      let result = Self::established_phase(
        &mut reader,
        &mut writer,
        &mut buf,
        &mut codec,
        &mut state,
        &events_tx,
        &mut commands_rx,
      )
      .await;

      let err = match result {
        Ok(()) => return Ok(()),
        Err(err @ (Error::PeerReset | Error::RetransmitLimit)) => err,
        Err(err) => {
          let _ = events_tx.send(Iap2Event::LinkDown(format!("link error: {err}"))).await;
          return Err(err);
        }
      };

      let reason = match err {
        Error::PeerReset => "peer RST",
        _ => "retransmit limit",
      };
      if established_at.elapsed() >= RESTART_HEALTHY_RESET {
        restart_window = None;
      }
      match restart_window {
        Some(started) if started.elapsed() >= RESTART_BUDGET_WINDOW => {
          let _ = events_tx.send(Iap2Event::LinkDown(reason.to_string())).await;
          return Err(err);
        }
        Some(_) => {}
        None => restart_window = Some(Instant::now()),
      }

      tracing::info!(reason, "iap2 link reset; restarting detection in place");
      if matches!(err, Error::RetransmitLimit) {
        let rst = LinkPacket::header_only(ControlBits::RST, state.last_sent_psn(), 0);
        let _ = write_packet(&mut writer, &mut codec, rst).await;
      }
      let _ = events_tx
        .send(Iap2Event::LinkRestarting {
          reason: reason.to_string(),
        })
        .await;
      buf.clear();
      tokio::time::sleep(RESTART_PAUSE).await;
    }
  }

  #[cfg(feature = "emulator")]
  pub async fn run_device<S>(
    stream: S,
    config: LinkConfig,
    events_tx: mpsc::Sender<Iap2Event>,
    mut commands_rx: mpsc::Receiver<Iap2Command>,
  ) -> Result<()>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut buf = BytesMut::with_capacity(READ_CAPACITY);
    let mut codec = LinkCodec;

    let (peer_lsp, peer_initial_psn) =
      Self::detect_and_negotiate_device(&mut reader, &mut writer, &mut buf, &mut codec, &config).await?;

    if events_tx.send(Iap2Event::Established(peer_lsp.clone())).await.is_err() {
      tracing::debug!("iap2 events receiver dropped before Established could be delivered");
    }
    tracing::info!("iap2 device link Established");

    let mut state = EstablishedState::new(
      config.initial_psn,
      peer_initial_psn,
      &peer_lsp,
      config.retransmit_give_up,
    );
    Self::established_phase(
      &mut reader,
      &mut writer,
      &mut buf,
      &mut codec,
      &mut state,
      &events_tx,
      &mut commands_rx,
    )
    .await
  }

  async fn detect_phase<R, W>(reader: &mut R, writer: &mut W, buf: &mut BytesMut, config: &LinkConfig) -> Result<()>
  where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
  {
    tracing::debug!("iap2 link entering Detecting state");
    let mut detect_interval = tokio::time::interval(config.detect_interval);
    detect_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let deadline = tokio::time::sleep(config.handshake_timeout);
    tokio::pin!(deadline);

    loop {
      tokio::select! {
        _ = detect_interval.tick() => {
          tracing::trace!("iap2 sending detect marker");
          writer.write_all(&DETECT_MARKER).await?;
          writer.flush().await?;
        }
        read = reader.read_buf(buf) => {
          let n = read?;
          if n == 0 {
            return Err(Error::PeerDisconnectedDuringHandshake);
          }
          if drain_detect_or_link_start(buf) {
            tracing::debug!("iap2 link detected peer; entering Negotiating");
            return Ok(());
          }
        }
        _ = &mut deadline => {
          return Err(Error::HandshakeTimeout("Detecting"));
        }
      }
    }
  }

  async fn negotiate_phase<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    codec: &mut LinkCodec,
    config: &LinkConfig,
  ) -> Result<(Lsp, u8)>
  where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
  {
    let our_seq = config.initial_psn;
    let syn = LinkPacket::with_payload(ControlBits::SYN, our_seq, 0, 0, config.our_lsp.encode());
    write_packet(writer, codec, syn).await?;
    tracing::trace!("iap2 sent SYN");

    let deadline = tokio::time::sleep(config.handshake_timeout);
    tokio::pin!(deadline);

    loop {
      if let Some(pkt) = codec.decode(buf)? {
        tracing::trace!("iap2 negotiating: received {:?}", pkt.header);
        if pkt.header.control.contains(ControlBits::RST) {
          return Err(Error::PeerReset);
        }
        if pkt.header.control.contains(ControlBits::SYN) {
          let lsp = Lsp::decode(&pkt.payload)?;
          let peer_initial_psn = pkt.header.seq;
          let standalone_ack = LinkPacket::header_only(ControlBits::ACK, our_seq, peer_initial_psn);
          write_packet(writer, codec, standalone_ack).await?;
          return Ok((lsp, peer_initial_psn));
        }
        return Err(Error::UnexpectedHandshakePacket(pkt.header.control));
      }

      tokio::select! {
        read = reader.read_buf(buf) => {
          let n = read?;
          if n == 0 {
            return Err(Error::PeerDisconnectedDuringHandshake);
          }
        }
        _ = &mut deadline => {
          return Err(Error::HandshakeTimeout("Negotiating"));
        }
      }
    }
  }

  #[cfg(feature = "emulator")]
  async fn detect_and_negotiate_device<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    codec: &mut LinkCodec,
    config: &LinkConfig,
  ) -> Result<(Lsp, u8)>
  where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
  {
    tracing::debug!("iap2 device link entering Detecting state");
    let mut detect_interval = tokio::time::interval(config.detect_interval);
    detect_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let deadline = tokio::time::sleep(config.handshake_timeout);
    tokio::pin!(deadline);

    loop {
      if let Some(pkt) = codec.decode(buf)? {
        tracing::trace!("iap2 device negotiating: received {:?}", pkt.header);
        if pkt.header.control.contains(ControlBits::RST) {
          return Err(Error::PeerReset);
        }
        if pkt.header.control.contains(ControlBits::SYN) {
          let lsp = Lsp::decode(&pkt.payload)?;
          let peer_initial_psn = pkt.header.seq;
          let syn_ack = LinkPacket::with_payload(
            ControlBits::SYN | ControlBits::ACK,
            config.initial_psn,
            peer_initial_psn,
            0,
            config.our_lsp.encode(),
          );
          write_packet(writer, codec, syn_ack).await?;
          tracing::trace!("iap2 device sent SYN|ACK");
          return Ok((lsp, peer_initial_psn));
        }
        return Err(Error::UnexpectedHandshakePacket(pkt.header.control));
      }

      tokio::select! {
        _ = detect_interval.tick() => {
          tracing::trace!("iap2 device sending detect marker");
          writer.write_all(&DETECT_MARKER).await?;
          writer.flush().await?;
        }
        read = reader.read_buf(buf) => {
          let n = read?;
          if n == 0 {
            return Err(Error::PeerDisconnectedDuringHandshake);
          }
        }
        _ = &mut deadline => {
          return Err(Error::HandshakeTimeout("Negotiating"));
        }
      }
    }
  }

  async fn established_phase<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    codec: &mut LinkCodec,
    state: &mut EstablishedState,
    events_tx: &mpsc::Sender<Iap2Event>,
    commands_rx: &mut mpsc::Receiver<Iap2Command>,
  ) -> Result<()>
  where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
  {
    let mut meter_tick = tokio::time::interval(METER_INTERVAL);
    meter_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    meter_tick.tick().await;

    loop {
      while let Some(pkt) = codec.decode(buf)? {
        if pkt.header.control.contains(ControlBits::RST) {
          return Err(Error::PeerReset);
        }

        if pkt.header.control.contains(ControlBits::ACK) {
          state.handle_inbound_ack(pkt.header.ack);
        }

        if pkt.header.control.contains(ControlBits::EAK) {
          state.handle_inbound_eak(&pkt.payload, writer, codec).await?;
          continue;
        }

        if pkt.header.has_payload() && !pkt.header.control.contains(ControlBits::SYN) {
          let delivered = state.handle_inbound_data(pkt);
          for d in delivered {
            let _ = events_tx
              .send(Iap2Event::DataReceived {
                session_id: d.session_id,
                payload: d.payload,
              })
              .await;
          }
          if state.has_buffered_out_of_order() {
            state.send_eak(writer, codec).await?;
          }
        }
      }

      state.drain_pending_send(writer, codec).await?;
      if state.needs_ack() {
        state.send_standalone_ack(writer, codec).await?;
      }

      let retransmit_deadline = state.next_retransmit_deadline();

      tokio::select! {
        _ = meter_tick.tick() => {
          state.report_meter();
        }
        read = reader.read_buf(buf) => {
          let n = read?;
          if n == 0 {
            return Err(Error::PeerDisconnected);
          }
        }
        cmd = commands_rx.recv() => {
          match cmd {
            Some(Iap2Command::Disconnect) | None => {
              let rst = LinkPacket::header_only(ControlBits::RST, state.last_sent_psn(), 0);
              if let Err(err) = write_packet(writer, codec, rst).await {
                tracing::warn!("iap2 failed to send RST on disconnect: {:?}", err);
              }
              let _ = events_tx.send(Iap2Event::LinkDown("local disconnect".into())).await;
              return Ok(());
            }
            Some(Iap2Command::Send { session_id, payload }) => {
              state.enqueue_send(session_id, payload);
            }
          }
        }
        _ = sleep_until_or_pending(retransmit_deadline) => {
          if state.handle_retransmit_fire(writer).await? {
            return Err(Error::RetransmitLimit);
          }
        }
      }
    }
  }
}

async fn sleep_until_or_pending(deadline: Option<Instant>) {
  match deadline {
    Some(d) => tokio::time::sleep_until(d).await,
    None => std::future::pending::<()>().await,
  }
}

fn drain_detect_or_link_start(buf: &mut BytesMut) -> bool {
  use bytes::Buf;
  let mut drained_any = false;
  while buf.starts_with(&DETECT_MARKER) {
    buf.advance(DETECT_MARKER.len());
    drained_any = true;
  }
  drained_any || (buf.len() >= 2 && buf[0..2] == LINK_MAGIC)
}

fn encode_packet(codec: &mut LinkCodec, packet: LinkPacket) -> Result<Bytes> {
  let mut wire = BytesMut::new();
  codec.encode(packet, &mut wire)?;
  Ok(wire.freeze())
}

async fn write_packet<W: AsyncWrite + Unpin>(writer: &mut W, codec: &mut LinkCodec, packet: LinkPacket) -> Result<()> {
  let wire = encode_packet(codec, packet)?;
  writer.write_all(&wire).await?;
  writer.flush().await?;
  Ok(())
}
