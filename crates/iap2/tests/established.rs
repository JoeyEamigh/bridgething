mod common;

use std::time::Duration;

use bridgething_iap2::{ControlBits, Error, Iap2Command, Iap2Event, LINK_HEADER_LEN, LinkCodec, LinkPacket};
use bytes::{Bytes, BytesMut};
use common::{
  LspBuilder, PEER_INITIAL_PSN, drive_peer_handshake, fast_link_config, read_link, recv_with_timeout, spawn_link,
  write_link,
};
use tokio::{io::DuplexStream, sync::mpsc, task::JoinHandle};

const SESSION_ID: u8 = 1;

fn accessory_lsp() -> bridgething_iap2::Lsp {
  LspBuilder {
    session_ids: vec![SESSION_ID],
    ..LspBuilder::default()
  }
  .build()
}

#[derive(Debug, Clone)]
struct PeerProposal {
  max_outgoing: u8,
  max_len: u16,
  retransmission_timeout_ms: u16,
  ack_timeout_ms: u16,
  max_retransmissions: u8,
  max_ack: u8,
}

impl Default for PeerProposal {
  fn default() -> Self {
    Self {
      max_outgoing: 5,
      max_len: 2048,
      retransmission_timeout_ms: 6000,
      ack_timeout_ms: 3000,
      max_retransmissions: 30,
      max_ack: 3,
    }
  }
}

impl PeerProposal {
  fn into_lsp(self) -> bridgething_iap2::Lsp {
    LspBuilder {
      max_outgoing: self.max_outgoing,
      max_len: self.max_len,
      retransmission_timeout_ms: self.retransmission_timeout_ms,
      ack_timeout_ms: self.ack_timeout_ms,
      max_retransmissions: self.max_retransmissions,
      max_ack: self.max_ack,
      session_ids: vec![SESSION_ID],
    }
    .build()
  }
}

struct Established {
  events_rx: mpsc::Receiver<Iap2Event>,
  cmd_tx: mpsc::Sender<Iap2Command>,
  peer: DuplexStream,
  peer_buf: BytesMut,
  peer_codec: LinkCodec,
  link: JoinHandle<bridgething_iap2::Result<()>>,
  our_initial_psn: u8,
}

async fn establish(peer: PeerProposal) -> Established {
  let (mut peer_stream, cmd_tx, mut events_rx, link) = spawn_link(fast_link_config(accessory_lsp()));
  let (peer_buf, peer_codec, our_initial_psn) = drive_peer_handshake(&mut peer_stream, peer.into_lsp()).await;

  let event = recv_with_timeout(&mut events_rx, Duration::from_secs(2))
    .await
    .expect("Established");
  assert!(matches!(event, Iap2Event::Established(_)));

  Established {
    events_rx,
    cmd_tx,
    peer: peer_stream,
    peer_buf,
    peer_codec,
    link,
    our_initial_psn,
  }
}

#[tokio::test(flavor = "current_thread")]
async fn send_command_data_round_trips_to_peer() {
  let mut e = establish(PeerProposal::default()).await;
  let our_psn = e.our_initial_psn;

  e.cmd_tx
    .send(Iap2Command::Send {
      session_id: SESSION_ID,
      payload: Bytes::from_static(b"hello"),
    })
    .await
    .unwrap();

  let pkt = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  assert!(pkt.header.control.contains(ControlBits::ACK));
  assert!(!pkt.header.control.contains(ControlBits::SYN));
  assert_eq!(pkt.header.seq, our_psn.wrapping_add(1));
  assert_eq!(pkt.header.ack, PEER_INITIAL_PSN);
  assert_eq!(pkt.header.session_id, SESSION_ID);
  assert_eq!(pkt.payload.as_ref(), b"hello");
}

#[tokio::test(flavor = "current_thread")]
async fn large_payload_fragments_into_chunks() {
  let mut e = establish(PeerProposal {
    max_len: 60,
    ..PeerProposal::default()
  })
  .await;
  let our_psn = e.our_initial_psn;

  let total = Bytes::from(vec![0xAB; 50 + 50 + 5]);
  e.cmd_tx
    .send(Iap2Command::Send {
      session_id: SESSION_ID,
      payload: total,
    })
    .await
    .unwrap();

  let p1 = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  let p2 = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  let p3 = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;

  assert_eq!(p1.payload.len(), 50);
  assert_eq!(p2.payload.len(), 50);
  assert_eq!(p3.payload.len(), 5);
  assert_eq!(p1.header.seq, our_psn.wrapping_add(1));
  assert_eq!(p2.header.seq, our_psn.wrapping_add(2));
  assert_eq!(p3.header.seq, our_psn.wrapping_add(3));
}

#[tokio::test(flavor = "current_thread")]
async fn inbound_data_delivers_to_events_channel() {
  let mut e = establish(PeerProposal::default()).await;

  let pkt = LinkPacket::with_payload(
    ControlBits::ACK,
    PEER_INITIAL_PSN.wrapping_add(1),
    e.our_initial_psn.wrapping_add(1),
    SESSION_ID,
    Bytes::from_static(b"ping"),
  );
  write_link(&mut e.peer, &mut e.peer_codec, pkt).await;

  let event = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
    .await
    .unwrap();
  match event {
    Iap2Event::DataReceived { session_id, payload } => {
      assert_eq!(session_id, SESSION_ID);
      assert_eq!(payload.as_ref(), b"ping");
    }
    other => panic!("expected DataReceived, got {:?}", other),
  }
}

#[tokio::test(flavor = "current_thread")]
async fn window_backpressures_on_unacked_max_outgoing() {
  let mut e = establish(PeerProposal {
    max_outgoing: 2,
    ..PeerProposal::default()
  })
  .await;
  let our_psn = e.our_initial_psn;

  for c in [b"a", b"b", b"c"] {
    e.cmd_tx
      .send(Iap2Command::Send {
        session_id: SESSION_ID,
        payload: Bytes::copy_from_slice(c),
      })
      .await
      .unwrap();
  }

  let p1 = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  let p2 = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  assert_eq!(p1.payload.as_ref(), b"a");
  assert_eq!(p2.payload.as_ref(), b"b");

  let timeout = tokio::time::timeout(
    Duration::from_millis(75),
    read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec),
  )
  .await;
  assert!(timeout.is_err(), "third packet leaked through closed window");

  let ack = LinkPacket::header_only(ControlBits::ACK, PEER_INITIAL_PSN, our_psn.wrapping_add(2));
  write_link(&mut e.peer, &mut e.peer_codec, ack).await;

  let p3 = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  assert_eq!(p3.payload.as_ref(), b"c");
  assert_eq!(p3.header.seq, our_psn.wrapping_add(3));
}

#[tokio::test(flavor = "current_thread")]
async fn retransmit_resends_unacked_packet_after_timeout() {
  let mut e = establish(PeerProposal {
    retransmission_timeout_ms: 100,
    max_retransmissions: 5,
    ..PeerProposal::default()
  })
  .await;

  e.cmd_tx
    .send(Iap2Command::Send {
      session_id: SESSION_ID,
      payload: Bytes::from_static(b"ouch"),
    })
    .await
    .unwrap();

  let first = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  let resend = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;

  assert_eq!(first.header.seq, resend.header.seq);
  assert_eq!(first.payload, resend.payload);
}

#[tokio::test(flavor = "current_thread")]
async fn a_peer_that_stops_acking_outright_gives_up_on_the_time_budget() {
  let mut e = establish(PeerProposal {
    retransmission_timeout_ms: 30,
    ..PeerProposal::default()
  })
  .await;

  e.cmd_tx
    .send(Iap2Command::Send {
      session_id: SESSION_ID,
      payload: Bytes::from_static(b"doomed"),
    })
    .await
    .unwrap();

  let event = recv_with_timeout(&mut e.events_rx, Duration::from_secs(3))
    .await
    .unwrap();
  match event {
    Iap2Event::LinkRestarting { reason } => assert!(reason.contains("retransmit"), "got reason {:?}", reason),
    other => panic!("expected LinkRestarting, got {:?}", other),
  }

  let mut saw_rst = false;
  for _ in 0..64 {
    let pkt = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
    if pkt.header.control.contains(ControlBits::RST) {
      saw_rst = true;
      break;
    }
  }
  assert!(saw_rst, "giving up must announce a RST");

  let (peer_buf, peer_codec, _psn) = drive_peer_handshake(&mut e.peer, PeerProposal::default().into_lsp()).await;
  e.peer_buf = peer_buf;
  e.peer_codec = peer_codec;
  let event = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
    .await
    .unwrap();
  assert!(matches!(event, Iap2Event::Established(_)));
  assert!(!e.link.is_finished(), "link task must survive the reset");
}

#[tokio::test(flavor = "current_thread")]
async fn peer_rst_restarts_detection_in_place_and_reestablishes() {
  let mut e = establish(PeerProposal::default()).await;

  let rst = LinkPacket::header_only(ControlBits::RST, PEER_INITIAL_PSN, 0);
  write_link(&mut e.peer, &mut e.peer_codec, rst).await;

  let event = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
    .await
    .unwrap();
  match event {
    Iap2Event::LinkRestarting { reason } => assert!(reason.contains("RST"), "got reason {:?}", reason),
    other => panic!("expected LinkRestarting, got {:?}", other),
  }

  let (peer_buf, peer_codec, our_psn) = drive_peer_handshake(&mut e.peer, PeerProposal::default().into_lsp()).await;
  e.peer_buf = peer_buf;
  e.peer_codec = peer_codec;
  let event = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
    .await
    .unwrap();
  assert!(matches!(event, Iap2Event::Established(_)));

  e.cmd_tx
    .send(Iap2Command::Send {
      session_id: SESSION_ID,
      payload: Bytes::from_static(b"alive"),
    })
    .await
    .unwrap();
  let pkt = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  assert_eq!(pkt.header.seq, our_psn.wrapping_add(1));
  assert_eq!(pkt.payload.as_ref(), b"alive");

  let inbound = LinkPacket::with_payload(
    ControlBits::ACK,
    PEER_INITIAL_PSN.wrapping_add(1),
    pkt.header.seq,
    SESSION_ID,
    Bytes::from_static(b"hello again"),
  );
  write_link(&mut e.peer, &mut e.peer_codec, inbound).await;
  let event = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
    .await
    .unwrap();
  match event {
    Iap2Event::DataReceived { session_id, payload } => {
      assert_eq!(session_id, SESSION_ID);
      assert_eq!(payload.as_ref(), b"hello again");
    }
    other => panic!("expected DataReceived, got {:?}", other),
  }
  assert!(!e.link.is_finished(), "link task must survive the reset");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn sustained_rst_flood_exhausts_the_restart_budget() {
  let mut e = establish(PeerProposal::default()).await;

  let rst = LinkPacket::header_only(ControlBits::RST, PEER_INITIAL_PSN, 0);
  write_link(&mut e.peer, &mut e.peer_codec, rst).await;
  let event = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
    .await
    .unwrap();
  assert!(matches!(event, Iap2Event::LinkRestarting { .. }));

  let feeder = {
    let mut peer = e.peer;
    let mut codec = e.peer_codec;
    tokio::spawn(async move {
      loop {
        let rst = LinkPacket::header_only(ControlBits::RST, PEER_INITIAL_PSN, 0);
        let mut wire = BytesMut::new();
        use tokio_util::codec::Encoder;
        if codec.encode(rst, &mut wire).is_err() {
          return;
        }
        use tokio::io::AsyncWriteExt;
        if peer.write_all(&wire).await.is_err() || peer.flush().await.is_err() {
          return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
      }
    })
  };

  let down = recv_with_timeout(&mut e.events_rx, Duration::from_secs(60))
    .await
    .expect("LinkDown after the restart budget closes");
  assert!(matches!(down, Iap2Event::LinkDown(_)), "got {:?}", down);
  let result = tokio::time::timeout(Duration::from_secs(5), e.link)
    .await
    .expect("link task exits after budget exhaustion")
    .expect("link task must not panic");
  assert!(matches!(result, Err(Error::PeerReset)));
  feeder.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn ack_delay_fires_standalone_ack_when_no_outbound_to_piggyback() {
  let mut e = establish(PeerProposal {
    ack_timeout_ms: 100,
    max_ack: 100,
    ..PeerProposal::default()
  })
  .await;

  let inbound = LinkPacket::with_payload(
    ControlBits::ACK,
    PEER_INITIAL_PSN.wrapping_add(1),
    e.our_initial_psn.wrapping_add(1),
    SESSION_ID,
    Bytes::from_static(b"ping"),
  );
  write_link(&mut e.peer, &mut e.peer_codec, inbound).await;

  let _ = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
    .await
    .unwrap();

  let ack = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  assert!(ack.header.control.contains(ControlBits::ACK));
  assert!(!ack.header.has_payload());
  assert_eq!(ack.header.length as usize, LINK_HEADER_LEN);
  assert_eq!(ack.header.ack, PEER_INITIAL_PSN.wrapping_add(1));
}

#[tokio::test(flavor = "current_thread")]
async fn cumulative_max_ack_threshold_fires_standalone_ack() {
  let mut e = establish(PeerProposal {
    ack_timeout_ms: 5000,
    max_ack: 2,
    ..PeerProposal::default()
  })
  .await;

  for i in 1..=2u8 {
    let pkt = LinkPacket::with_payload(
      ControlBits::ACK,
      PEER_INITIAL_PSN.wrapping_add(i),
      e.our_initial_psn.wrapping_add(1),
      SESSION_ID,
      Bytes::from_static(b"x"),
    );
    write_link(&mut e.peer, &mut e.peer_codec, pkt).await;
  }

  for _ in 0..2 {
    let _ = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
      .await
      .unwrap();
  }

  let ack = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  assert!(ack.header.control.contains(ControlBits::ACK));
  assert!(!ack.header.has_payload());
  assert_eq!(ack.header.ack, PEER_INITIAL_PSN.wrapping_add(2));
}

#[tokio::test(flavor = "current_thread")]
async fn out_of_order_inbound_triggers_eak_listing_missing_psns() {
  let mut e = establish(PeerProposal::default()).await;

  let gap = LinkPacket::with_payload(
    ControlBits::ACK,
    PEER_INITIAL_PSN.wrapping_add(2),
    e.our_initial_psn.wrapping_add(1),
    SESSION_ID,
    Bytes::from_static(b"future"),
  );
  write_link(&mut e.peer, &mut e.peer_codec, gap).await;

  let eak = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  assert!(eak.header.control.contains(ControlBits::EAK));
  assert_eq!(eak.payload.as_ref(), &[PEER_INITIAL_PSN.wrapping_add(1)]);
}

#[tokio::test(flavor = "current_thread")]
async fn out_of_order_drains_in_order_when_gap_arrives() {
  let mut e = establish(PeerProposal::default()).await;
  let our_psn = e.our_initial_psn;

  let p2 = LinkPacket::with_payload(
    ControlBits::ACK,
    PEER_INITIAL_PSN.wrapping_add(2),
    our_psn.wrapping_add(1),
    SESSION_ID,
    Bytes::from_static(b"two"),
  );
  write_link(&mut e.peer, &mut e.peer_codec, p2).await;

  let eak = read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec).await;
  assert!(eak.header.control.contains(ControlBits::EAK));

  let p1 = LinkPacket::with_payload(
    ControlBits::ACK,
    PEER_INITIAL_PSN.wrapping_add(1),
    our_psn.wrapping_add(1),
    SESSION_ID,
    Bytes::from_static(b"one"),
  );
  write_link(&mut e.peer, &mut e.peer_codec, p1).await;

  let first = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
    .await
    .unwrap();
  match first {
    Iap2Event::DataReceived { payload, .. } => assert_eq!(payload.as_ref(), b"one"),
    other => panic!("expected DataReceived 'one', got {:?}", other),
  }
  let second = recv_with_timeout(&mut e.events_rx, Duration::from_secs(2))
    .await
    .unwrap();
  match second {
    Iap2Event::DataReceived { payload, .. } => assert_eq!(payload.as_ref(), b"two"),
    other => panic!("expected DataReceived 'two', got {:?}", other),
  }
}

async fn read_link_within(e: &mut Established, dur: Duration) -> Option<LinkPacket> {
  tokio::time::timeout(dur, read_link(&mut e.peer, &mut e.peer_buf, &mut e.peer_codec))
    .await
    .ok()
}

#[tokio::test(flavor = "current_thread")]
async fn a_slow_but_healthy_peer_costs_a_constant_not_a_retransmit_per_packet() {
  const RTO_MS: u64 = 200;
  const ACK_LATENCY_MS: u64 = 300;
  const PACKETS: u8 = 20;
  const STARTUP_COST: usize = 1;

  let mut e = establish(PeerProposal {
    max_outgoing: 127,
    max_len: 65535,
    retransmission_timeout_ms: RTO_MS as u16,
    max_retransmissions: 30,
    ..PeerProposal::default()
  })
  .await;
  let last_seq = e.our_initial_psn.wrapping_add(PACKETS);

  for i in 0..PACKETS {
    e.cmd_tx
      .send(Iap2Command::Send {
        session_id: SESSION_ID,
        payload: Bytes::from(vec![i; 64]),
      })
      .await
      .unwrap();
  }

  let mut wire = Vec::new();
  let mut highest = e.our_initial_psn;
  while let Some(pkt) = read_link_within(&mut e, Duration::from_millis(800)).await {
    wire.push(pkt.header.seq);
    assert!(wire.len() <= 200, "the link never stopped resending");
    if pkt.header.seq == highest.wrapping_add(1) {
      highest = pkt.header.seq;
    }
    tokio::time::sleep(Duration::from_millis(ACK_LATENCY_MS)).await;
    let ack = LinkPacket::header_only(ControlBits::ACK, PEER_INITIAL_PSN, highest);
    write_link(&mut e.peer, &mut e.peer_codec, ack).await;
  }

  assert_eq!(highest, last_seq, "every payload must arrive");
  let mut duplicates = wire.clone();
  duplicates.sort_unstable();
  duplicates.dedup();
  assert!(
    wire.len() <= PACKETS as usize + STARTUP_COST,
    "nothing was lost and everything was acked, just later than the {RTO_MS}ms RTO. \
     {} wire packets carried {PACKETS} payloads ({:.1}x amplification), seqs {:?}",
    wire.len(),
    wire.len() as f64 / PACKETS as f64,
    wire
  );
}

#[tokio::test(flavor = "current_thread")]
async fn late_acks_cost_bandwidth_but_a_still_advancing_peer_keeps_the_link() {
  const RTO_MS: u64 = 100;
  const DRAIN_MS_PER_PACKET: u64 = 40;
  const PACKETS: u8 = 24;

  let mut e = establish(PeerProposal {
    max_outgoing: 127,
    max_len: 65535,
    retransmission_timeout_ms: RTO_MS as u16,
    max_retransmissions: 4,
    ..PeerProposal::default()
  })
  .await;

  for i in 0..PACKETS {
    e.cmd_tx
      .send(Iap2Command::Send {
        session_id: SESSION_ID,
        payload: Bytes::from(vec![i; 64]),
      })
      .await
      .unwrap();
  }

  let mut wire = 0usize;
  let mut highest = e.our_initial_psn;
  while wire < 400 {
    let Some(pkt) = read_link_within(&mut e, Duration::from_millis(600)).await else {
      break;
    };
    wire += 1;
    if pkt.header.seq == highest.wrapping_add(1) {
      highest = pkt.header.seq;
    }
    tokio::time::sleep(Duration::from_millis(DRAIN_MS_PER_PACKET)).await;
    let ack = LinkPacket::header_only(ControlBits::ACK, PEER_INITIAL_PSN, highest);
    write_link(&mut e.peer, &mut e.peer_codec, ack).await;
  }

  assert_eq!(
    highest,
    e.our_initial_psn.wrapping_add(PACKETS),
    "every payload must arrive"
  );
  assert!(
    matches!(
      tokio::time::timeout(Duration::from_millis(50), e.events_rx.recv()).await,
      Err(_) | Ok(None)
    ),
    "a cumulative ack that keeps advancing pops the head before it can exhaust its retries, \
     so ack LAG only costs bandwidth ({wire} wire packets for {PACKETS} payloads). \
     killing the link needs the ack stream to STALL, which is a different fault"
  );
}
