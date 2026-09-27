use std::{
  collections::{BTreeMap, VecDeque},
  time::Duration,
};

use bytes::{Bytes, BytesMut};
use tokio::{
  io::{AsyncWrite, AsyncWriteExt},
  time::Instant,
};

use super::{encode_packet, write_packet};
use crate::{
  error::Result,
  frame::{ControlBits, LINK_FRAME_OVERHEAD, LinkCodec, LinkPacket, Lsp},
};

const RETRANSMIT_STALL_MARKER: u8 = 3;
pub(super) const METER_INTERVAL: Duration = Duration::from_secs(5);

const RTO_MIN: Duration = Duration::from_millis(500);
const RTO_MAX: Duration = Duration::from_secs(8);
pub const RETRANSMIT_GIVE_UP: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct RttEstimator {
  srtt: Option<Duration>,
  rttvar: Duration,
  seed: Duration,
}

impl RttEstimator {
  fn new(seed: Duration) -> Self {
    Self {
      srtt: None,
      rttvar: Duration::ZERO,
      seed,
    }
  }

  fn sample(&mut self, rtt: Duration) {
    match self.srtt {
      None => {
        self.srtt = Some(rtt);
        self.rttvar = rtt / 2;
      }
      Some(srtt) => {
        let err = srtt.abs_diff(rtt);
        self.rttvar = (self.rttvar * 3 + err) / 4;
        self.srtt = Some((srtt * 7 + rtt) / 8);
      }
    }
  }

  fn rto(&self) -> Duration {
    match self.srtt {
      None => self.seed,
      Some(srtt) => (srtt + 4 * self.rttvar).clamp(RTO_MIN, RTO_MAX),
    }
  }
}

#[derive(Debug)]
struct Meter {
  since: Instant,
  tx_bytes: u64,
  tx_packets: u32,
  rx_bytes: u64,
  rx_packets: u32,
  retransmits: u32,
  window_stalls: u32,
}

impl Meter {
  fn new() -> Self {
    Self {
      since: Instant::now(),
      tx_bytes: 0,
      tx_packets: 0,
      rx_bytes: 0,
      rx_packets: 0,
      retransmits: 0,
      window_stalls: 0,
    }
  }

  fn report(&mut self, pending: usize, unacked: usize, srtt_ms: Option<u64>, rto_ms: u64) {
    let elapsed = self.since.elapsed();
    if self.tx_packets == 0 && self.rx_packets == 0 {
      *self = Self::new();
      return;
    }
    let secs = elapsed.as_secs_f64();
    if self.retransmits > 0 {
      tracing::info!(
        tx_kb_s = self.tx_bytes as f64 / 1024.0 / secs,
        rx_kb_s = self.rx_bytes as f64 / 1024.0 / secs,
        retransmits = self.retransmits,
        window_stalls = self.window_stalls,
        srtt_ms,
        rto_ms,
        pending,
        unacked,
        "iap2 link retransmitting"
      );
    }
    tracing::debug!(
      tx_kb_s = self.tx_bytes as f64 / 1024.0 / secs,
      rx_kb_s = self.rx_bytes as f64 / 1024.0 / secs,
      tx_pkt_s = self.tx_packets as f64 / secs,
      rx_pkt_s = self.rx_packets as f64 / secs,
      retransmits = self.retransmits,
      window_stalls = self.window_stalls,
      pending,
      unacked,
      "iap2 link throughput"
    );
    *self = Self::new();
  }
}

#[derive(Debug, Clone, Copy)]
struct LinkParams {
  max_outgoing: u8,
  max_payload_len: u16,
  seed_rto: Duration,
}

impl LinkParams {
  fn from_peer_lsp(lsp: &Lsp) -> Self {
    Self {
      max_outgoing: lsp.max_outgoing.max(1),
      max_payload_len: lsp.max_len.saturating_sub(LINK_FRAME_OVERHEAD as u16).max(1),
      seed_rto: Duration::from_millis(lsp.retransmission_timeout_ms as u64),
    }
  }
}

#[derive(Debug)]
struct UnackedPacket {
  seq: u8,
  wire: Bytes,
  sent_at: Instant,
  retry_count: u8,
}

#[derive(Debug)]
pub(super) struct DeliveredData {
  pub(super) session_id: u8,
  pub(super) payload: Bytes,
}

#[derive(Debug)]
pub(super) struct EstablishedState {
  params: LinkParams,

  last_sent_psn: u8,
  unacked: VecDeque<UnackedPacket>,
  pending_send: VecDeque<(u8, Bytes)>,

  last_received_in_sequence_psn: u8,
  out_of_order: BTreeMap<u8, LinkPacket>,
  unacked_delivery: bool,
  must_send_ack: bool,

  rtt: RttEstimator,
  rto: Duration,
  retransmit_at: Option<Instant>,
  last_progress: Instant,
  give_up_after: Duration,

  meter: Meter,
}

impl EstablishedState {
  pub(super) fn new(initial_psn: u8, peer_initial_psn: u8, peer_lsp: &Lsp, give_up_after: Duration) -> Self {
    let params = LinkParams::from_peer_lsp(peer_lsp);
    Self {
      rtt: RttEstimator::new(params.seed_rto),
      rto: params.seed_rto,
      retransmit_at: None,
      last_progress: Instant::now(),
      give_up_after,
      params,
      last_sent_psn: initial_psn,
      unacked: VecDeque::new(),
      pending_send: VecDeque::new(),
      last_received_in_sequence_psn: peer_initial_psn,
      out_of_order: BTreeMap::new(),
      unacked_delivery: false,
      must_send_ack: false,

      meter: Meter::new(),
    }
  }

  pub(super) fn report_meter(&mut self) {
    let (pending, unacked) = (self.pending_send.len(), self.unacked.len());
    let srtt_ms = self.rtt.srtt.map(|srtt| srtt.as_millis() as u64);
    self
      .meter
      .report(pending, unacked, srtt_ms, self.rto.as_millis() as u64);
  }

  pub(super) fn last_sent_psn(&self) -> u8 {
    self.last_sent_psn
  }

  pub(super) fn next_retransmit_deadline(&self) -> Option<Instant> {
    self.retransmit_at
  }

  fn arm_retransmit(&mut self, now: Instant) {
    self.retransmit_at = (!self.unacked.is_empty()).then(|| now + self.rto);
  }

  pub(super) fn has_buffered_out_of_order(&self) -> bool {
    !self.out_of_order.is_empty()
  }

  pub(super) fn needs_ack(&self) -> bool {
    self.must_send_ack || self.unacked_delivery
  }

  pub(super) fn enqueue_send(&mut self, session_id: u8, payload: Bytes) {
    if payload.is_empty() {
      return;
    }
    let max = self.params.max_payload_len as usize;
    if payload.len() <= max {
      self.pending_send.push_back((session_id, payload));
      return;
    }
    let mut remaining = payload;
    while remaining.len() > max {
      let chunk = remaining.split_to(max);
      self.pending_send.push_back((session_id, chunk));
    }
    if !remaining.is_empty() {
      self.pending_send.push_back((session_id, remaining));
    }
  }

  pub(super) async fn drain_pending_send<W>(&mut self, writer: &mut W, codec: &mut LinkCodec) -> Result<()>
  where
    W: AsyncWrite + Unpin,
  {
    while self.window_has_room() {
      let Some((session_id, payload)) = self.pending_send.pop_front() else {
        break;
      };
      self.send_data_packet(session_id, payload, writer, codec).await?;
    }
    if !self.pending_send.is_empty() {
      self.meter.window_stalls += 1;
    }
    Ok(())
  }

  pub(super) async fn send_standalone_ack<W>(&mut self, writer: &mut W, codec: &mut LinkCodec) -> Result<()>
  where
    W: AsyncWrite + Unpin,
  {
    let packet = LinkPacket::header_only(ControlBits::ACK, self.last_sent_psn, self.last_received_in_sequence_psn);
    write_packet(writer, codec, packet).await?;
    self.unacked_delivery = false;
    self.must_send_ack = false;
    Ok(())
  }

  pub(super) async fn send_eak<W>(&mut self, writer: &mut W, codec: &mut LinkCodec) -> Result<()>
  where
    W: AsyncWrite + Unpin,
  {
    let Some(&furthest_buffered) = self.out_of_order.keys().last() else {
      return Ok(());
    };
    let total = furthest_buffered.wrapping_sub(self.last_received_in_sequence_psn);
    let mut payload = BytesMut::with_capacity(total as usize);
    let mut probe = self.last_received_in_sequence_psn.wrapping_add(1);
    for _ in 0..total {
      if !self.out_of_order.contains_key(&probe) {
        payload.extend_from_slice(&[probe]);
      }
      probe = probe.wrapping_add(1);
    }
    if payload.is_empty() {
      return Ok(());
    }
    let packet = LinkPacket::with_payload(
      ControlBits::EAK | ControlBits::ACK,
      self.last_sent_psn,
      self.last_received_in_sequence_psn,
      0,
      payload.freeze(),
    );
    write_packet(writer, codec, packet).await?;
    self.unacked_delivery = false;
    self.must_send_ack = false;
    Ok(())
  }

  pub(super) async fn handle_inbound_eak<W>(
    &mut self,
    payload: &[u8],
    writer: &mut W,
    _codec: &mut LinkCodec,
  ) -> Result<()>
  where
    W: AsyncWrite + Unpin,
  {
    for &missing_seq in payload {
      if let Some(packet) = self.unacked.iter_mut().find(|p| p.seq == missing_seq) {
        packet.retry_count = packet.retry_count.saturating_add(1);
        let wire = packet.wire.clone();
        writer.write_all(&wire).await?;
      }
    }
    writer.flush().await?;
    Ok(())
  }

  pub(super) fn handle_inbound_ack(&mut self, ack_value: u8) {
    let now = Instant::now();
    let mut sample = None;
    let mut advanced = false;
    while let Some(front) = self.unacked.front() {
      if ack_value.wrapping_sub(front.seq) > 127 {
        break;
      }
      let packet = self.unacked.pop_front().expect("front peeked");
      advanced = true;
      if sample.is_none() && packet.retry_count == 0 {
        sample = Some(now.saturating_duration_since(packet.sent_at));
      }
    }
    if !advanced {
      return;
    }
    if let Some(rtt) = sample {
      self.rtt.sample(rtt);
      self.rto = self.rtt.rto();
    }
    self.last_progress = now;
    self.arm_retransmit(now);
  }

  pub(super) fn handle_inbound_data(&mut self, packet: LinkPacket) -> Vec<DeliveredData> {
    let recv_seq = packet.header.seq;
    self.meter.rx_packets += 1;
    self.meter.rx_bytes += packet.payload.len() as u64;
    let delta = recv_seq.wrapping_sub(self.last_received_in_sequence_psn);

    if delta == 0 {
      tracing::trace!("iap2 received duplicate of last delivered seq {}; re-acking", recv_seq);
      self.must_send_ack = true;
      return Vec::new();
    }

    if delta == 1 {
      let mut out = Vec::with_capacity(1);
      out.push(DeliveredData {
        session_id: packet.header.session_id,
        payload: packet.payload,
      });
      self.last_received_in_sequence_psn = recv_seq;
      self.mark_delivered();

      loop {
        let next = self.last_received_in_sequence_psn.wrapping_add(1);
        let Some(buffered) = self.out_of_order.remove(&next) else {
          break;
        };
        out.push(DeliveredData {
          session_id: buffered.header.session_id,
          payload: buffered.payload,
        });
        self.last_received_in_sequence_psn = next;
        self.mark_delivered();
      }
      return out;
    }

    if (delta as usize) < self.params.max_outgoing as usize {
      tracing::trace!("iap2 buffering out-of-order seq {} (delta {})", recv_seq, delta);
      self.out_of_order.insert(recv_seq, packet);
      return Vec::new();
    }

    tracing::trace!("iap2 dropping seq {} (delta {} beyond window)", recv_seq, delta);
    self.must_send_ack = true;
    Vec::new()
  }

  pub(super) async fn handle_retransmit_fire<W>(&mut self, writer: &mut W) -> Result<bool>
  where
    W: AsyncWrite + Unpin,
  {
    let now = Instant::now();
    if self.retransmit_at.is_none_or(|at| at > now) {
      return Ok(false);
    }
    let stalled_for = now.saturating_duration_since(self.last_progress);
    let depth = self.unacked.len();
    let Some(front) = self.unacked.front_mut() else {
      self.retransmit_at = None;
      return Ok(false);
    };
    if stalled_for >= self.give_up_after {
      let seq = front.seq;
      tracing::warn!(
        seq,
        stalled_ms = stalled_for.as_millis() as u64,
        unacked = depth,
        "iap2 link gave up: the peer stopped acking"
      );
      return Ok(true);
    }
    front.retry_count += 1;
    self.meter.retransmits += 1;
    let wire = front.wire.clone();
    let seq = front.seq;
    let retry = front.retry_count;
    self.rto = (self.rto * 2).min(RTO_MAX);
    self.arm_retransmit(now);
    if retry >= RETRANSMIT_STALL_MARKER {
      tracing::warn!(
        seq,
        attempt = retry,
        stalled_ms = stalled_for.as_millis() as u64,
        rto_ms = self.rto.as_millis() as u64,
        "iap2 link wedge suspected (retransmit stall)"
      );
    } else {
      tracing::debug!(
        seq,
        attempt = retry,
        rto_ms = self.rto.as_millis() as u64,
        "iap2 retransmitting"
      );
    }
    writer.write_all(&wire).await?;
    writer.flush().await?;
    Ok(false)
  }

  fn window_has_room(&self) -> bool {
    self.unacked.len() < self.params.max_outgoing as usize
  }

  fn mark_delivered(&mut self) {
    self.unacked_delivery = true;
  }

  async fn send_data_packet<W>(
    &mut self,
    session_id: u8,
    payload: Bytes,
    writer: &mut W,
    codec: &mut LinkCodec,
  ) -> Result<()>
  where
    W: AsyncWrite + Unpin,
  {
    let seq = self.last_sent_psn.wrapping_add(1);
    let packet = LinkPacket::with_payload(
      ControlBits::ACK,
      seq,
      self.last_received_in_sequence_psn,
      session_id,
      payload,
    );
    let wire = encode_packet(codec, packet)?;
    writer.write_all(&wire).await?;
    writer.flush().await?;

    self.meter.tx_packets += 1;
    self.meter.tx_bytes += wire.len() as u64;
    self.last_sent_psn = seq;
    self.unacked_delivery = false;
    self.must_send_ack = false;
    let now = Instant::now();
    let opening = self.unacked.is_empty();
    self.unacked.push_back(UnackedPacket {
      seq,
      wire,
      sent_at: now,
      retry_count: 0,
    });
    if opening {
      self.last_progress = now;
      self.arm_retransmit(now);
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::frame::SessionTriple;

  fn test_lsp(max_outgoing: u8, max_len: u16, max_ack: u8) -> Lsp {
    Lsp {
      version: 1,
      max_outgoing,
      max_len,
      retransmission_timeout_ms: 6000,
      ack_timeout_ms: 3000,
      max_retransmissions: 30,
      max_ack,
      sessions: vec![SessionTriple {
        id: 1,
        session_type: 0,
        version: 1,
      }],
    }
  }

  fn data_packet(seq: u8, ack: u8, session_id: u8, payload: &[u8]) -> LinkPacket {
    LinkPacket::with_payload(ControlBits::ACK, seq, ack, session_id, Bytes::copy_from_slice(payload))
  }

  #[test]
  fn enqueue_send_chunks_at_max_payload_len() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 60, 3), RETRANSMIT_GIVE_UP);
    let max_payload = state.params.max_payload_len as usize;
    assert_eq!(max_payload, 60 - LINK_FRAME_OVERHEAD);
    let total = max_payload * 2 + 5;
    let payload = Bytes::from(vec![0xABu8; total]);
    state.enqueue_send(1, payload);
    assert_eq!(state.pending_send.len(), 3);
    let chunks: Vec<usize> = state.pending_send.iter().map(|(_, b)| b.len()).collect();
    assert_eq!(chunks, vec![max_payload, max_payload, 5]);
  }

  #[test]
  fn handle_inbound_data_in_sequence_delivers_and_drains_buffered() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 65535, 3), RETRANSMIT_GIVE_UP);
    let buffered = data_packet(52, 100, 1, b"two");
    state.out_of_order.insert(52, buffered);
    let next = data_packet(51, 100, 1, b"one");
    let delivered = state.handle_inbound_data(next);
    assert_eq!(delivered.len(), 2);
    assert_eq!(delivered[0].payload.as_ref(), b"one");
    assert_eq!(delivered[1].payload.as_ref(), b"two");
    assert_eq!(state.last_received_in_sequence_psn, 52);
    assert!(state.out_of_order.is_empty());
  }

  #[test]
  fn handle_inbound_data_buffers_out_of_order() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 65535, 3), RETRANSMIT_GIVE_UP);
    let pkt = data_packet(52, 100, 1, b"hello");
    let delivered = state.handle_inbound_data(pkt);
    assert!(delivered.is_empty());
    assert!(state.out_of_order.contains_key(&52));
    assert_eq!(state.last_received_in_sequence_psn, 50);
  }

  #[tokio::test]
  async fn single_in_sequence_delivery_owes_ack_immediately() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 65535, 8), RETRANSMIT_GIVE_UP);
    assert!(!state.needs_ack(), "fresh state owes no ack");
    let delivered = state.handle_inbound_data(data_packet(51, 100, 1, b"art-chunk"));
    assert_eq!(delivered.len(), 1);
    assert!(
      state.needs_ack(),
      "one in-sequence packet must owe an ack without waiting for max_ack or a timer"
    );
    let mut sink = Vec::new();
    let mut codec = LinkCodec;
    state.send_standalone_ack(&mut sink, &mut codec).await.unwrap();
    assert!(!state.needs_ack(), "sending the ack clears the debt");
  }

  #[test]
  fn handle_inbound_data_duplicate_forces_ack() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 65535, 3), RETRANSMIT_GIVE_UP);
    let pkt = data_packet(50, 100, 1, b"dup");
    let delivered = state.handle_inbound_data(pkt);
    assert!(delivered.is_empty());
    assert!(state.must_send_ack);
  }

  #[test]
  fn handle_inbound_ack_drains_unacked() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 65535, 3), RETRANSMIT_GIVE_UP);
    state.unacked.push_back(UnackedPacket {
      seq: 100,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 0,
    });
    state.unacked.push_back(UnackedPacket {
      seq: 101,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 0,
    });
    state.handle_inbound_ack(102);
    assert!(state.unacked.is_empty());
  }

  #[test]
  fn handle_inbound_ack_partial_drains() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 65535, 3), RETRANSMIT_GIVE_UP);
    state.unacked.push_back(UnackedPacket {
      seq: 100,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 0,
    });
    state.unacked.push_back(UnackedPacket {
      seq: 101,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 0,
    });
    state.handle_inbound_ack(101);
    assert!(state.unacked.is_empty());
  }

  #[test]
  fn handle_inbound_ack_handles_psn_wrap() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 65535, 3), RETRANSMIT_GIVE_UP);
    state.unacked.push_back(UnackedPacket {
      seq: 254,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 0,
    });
    state.unacked.push_back(UnackedPacket {
      seq: 255,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 0,
    });
    state.unacked.push_back(UnackedPacket {
      seq: 0,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 0,
    });
    state.handle_inbound_ack(1);
    assert!(state.unacked.is_empty());
  }

  #[test]
  fn an_unsampled_estimator_uses_the_peers_advertised_timeout() {
    let rtt = RttEstimator::new(Duration::from_millis(6000));
    assert_eq!(rtt.rto(), Duration::from_millis(6000));
  }

  #[test]
  fn the_first_sample_seeds_srtt_and_variance() {
    let mut rtt = RttEstimator::new(Duration::from_millis(6000));
    rtt.sample(Duration::from_millis(800));
    assert_eq!(rtt.srtt, Some(Duration::from_millis(800)));
    assert_eq!(rtt.rttvar, Duration::from_millis(400));
    assert_eq!(rtt.rto(), Duration::from_millis(2400));
  }

  #[test]
  fn a_steady_round_trip_converges_and_the_variance_decays() {
    let mut rtt = RttEstimator::new(Duration::from_millis(6000));
    for _ in 0..40 {
      rtt.sample(Duration::from_millis(800));
    }
    let srtt = rtt.srtt.expect("sampled");
    assert!(
      srtt.abs_diff(Duration::from_millis(800)) < Duration::from_millis(1),
      "got {srtt:?}"
    );
    assert!(rtt.rttvar < Duration::from_millis(10), "got {:?}", rtt.rttvar);
    assert!(rtt.rto().abs_diff(Duration::from_millis(800)) < Duration::from_millis(50));
  }

  #[test]
  fn a_jittery_round_trip_widens_the_timeout_past_the_mean() {
    let mut steady = RttEstimator::new(Duration::from_millis(6000));
    let mut jittery = RttEstimator::new(Duration::from_millis(6000));
    for i in 0..40 {
      steady.sample(Duration::from_millis(800));
      jittery.sample(Duration::from_millis(if i % 2 == 0 { 300 } else { 1300 }));
    }
    assert!(
      jittery.rto() > steady.rto(),
      "jitter must buy headroom: {:?} vs {:?}",
      jittery.rto(),
      steady.rto()
    );
  }

  #[test]
  fn the_timeout_stays_inside_its_bounds() {
    let mut fast = RttEstimator::new(Duration::from_millis(6000));
    fast.sample(Duration::from_micros(200));
    assert_eq!(fast.rto(), RTO_MIN);

    let mut slow = RttEstimator::new(Duration::from_millis(6000));
    slow.sample(Duration::from_secs(90));
    assert_eq!(slow.rto(), RTO_MAX);
  }

  #[test]
  fn a_retransmitted_packet_contributes_no_sample() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 2048, 3), RETRANSMIT_GIVE_UP);
    state.unacked.push_back(UnackedPacket {
      seq: 100,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 1,
    });
    state.handle_inbound_ack(100);
    assert_eq!(
      state.rtt.srtt, None,
      "an ack for a retransmitted packet is ambiguous and always reads short"
    );
    assert!(state.unacked.is_empty(), "it is still acknowledged");
  }

  #[test]
  fn karn_holds_the_backed_off_timeout_until_an_unambiguous_sample() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(127, 2048, 3), RETRANSMIT_GIVE_UP);
    state.rto = (state.rto * 2).min(RTO_MAX);
    let backed_off = state.rto;

    state.unacked.push_back(UnackedPacket {
      seq: 100,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 1,
    });
    state.handle_inbound_ack(100);
    assert_eq!(state.rto, backed_off, "an ambiguous ack must not restore it");

    state.unacked.push_back(UnackedPacket {
      seq: 101,
      wire: Bytes::new(),
      sent_at: Instant::now(),
      retry_count: 0,
    });
    state.handle_inbound_ack(101);
    assert_eq!(state.rto, RTO_MIN, "a clean sample recomputes it from the estimator");
  }

  #[test]
  fn window_has_room_respects_max_outgoing() {
    let mut state = EstablishedState::new(99, 50, &test_lsp(2, 65535, 3), RETRANSMIT_GIVE_UP);
    assert!(state.window_has_room());
    for seq in 100..102 {
      state.unacked.push_back(UnackedPacket {
        seq,
        wire: Bytes::new(),
        sent_at: Instant::now(),
        retry_count: 0,
      });
    }
    assert!(!state.window_has_room());
  }
}
