use std::{
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
  time::Duration,
};

use bridgething_gateway::Gateway;
use bridgething_test_harness::Harness;
use libbridgething::{
  GatewayCapabilities, GatewayInfo, TunnelAck, TunnelClosed, TunnelData,
  gateway::{BridgeToGatewayMsgData, BridgeToGatewayTunnelMsg, TunnelOpen, TunnelOpenReply},
};
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpStream,
};

const FRAME_BYTES: usize = 1448;
const FRAME_COUNT: u64 = 512;
const SETTLE: Duration = Duration::from_secs(10);

fn frame(seq: u64) -> Vec<u8> {
  let mut buf = Vec::with_capacity(FRAME_BYTES);
  buf.extend_from_slice(&seq.to_be_bytes());
  buf.resize(FRAME_BYTES, seq as u8);
  buf
}

fn decode_seqs(stream: &[u8]) -> Vec<u64> {
  stream
    .chunks_exact(FRAME_BYTES)
    .map(|c| u64::from_be_bytes(c[..8].try_into().expect("8 bytes")))
    .collect()
}

fn corrupt_frames(stream: &[u8]) -> Vec<u64> {
  stream
    .chunks_exact(FRAME_BYTES)
    .filter_map(|c| {
      let seq = u64::from_be_bytes(c[..8].try_into().expect("8 bytes"));
      c[8..].iter().any(|b| *b != seq as u8).then_some(seq)
    })
    .collect()
}

async fn announce(gateway: &Gateway) {
  let caps = GatewayCapabilities {
    gateway: GatewayInfo {
      address: String::new(),
      name: "tunnel-server".into(),
      os_name: "android".into(),
      app_name: "tunnel-server".into(),
      app_version: "0.0.0".into(),
      adapter_version: "harness".into(),
      lib_version: "0.0.0".into(),
      libbridgething_version: format!("v{}", libbridgething::LIBBRIDGETHING_VERSION),
    },
    ..Default::default()
  };
  gateway.capabilities().announce(caps).await.expect("announce");
}

async fn socks_connect(addr: std::net::SocketAddr) -> TcpStream {
  let mut stream = TcpStream::connect(addr).await.expect("connect to SOCKS proxy");
  stream.write_all(&[0x05, 0x01, 0x00]).await.expect("greeting");
  let mut greeting = [0u8; 2];
  stream.read_exact(&mut greeting).await.expect("greeting reply");
  assert_eq!(greeting, [0x05, 0x00], "no-auth accepted");

  let host = b"example.invalid";
  let mut req = vec![0x05, 0x01, 0x00, 0x03, host.len() as u8];
  req.extend_from_slice(host);
  req.extend_from_slice(&80u16.to_be_bytes());
  stream.write_all(&req).await.expect("connect request");

  let mut reply = [0u8; 10];
  stream.read_exact(&mut reply).await.expect("connect reply");
  assert_eq!(reply[1], 0x00, "tunnel open succeeded");
  stream
}

fn spawn_open_responder(companion: &Gateway) -> tokio::sync::oneshot::Receiver<uuid::Uuid> {
  let mut inbound = companion.events();
  let (tx, rx) = tokio::sync::oneshot::channel();
  let companion = companion.clone();
  tokio::spawn(async move {
    loop {
      let msg = inbound.recv().await.expect("gateway channel alive");
      if let BridgeToGatewayMsgData::Tunnel(BridgeToGatewayTunnelMsg::Open(TunnelOpen { tunnel_id, .. })) = &msg.data {
        let id = *tunnel_id;
        companion
          .handle(&msg)
          .respond_to::<TunnelOpen>(TunnelOpenReply {})
          .await
          .expect("respond to TunnelOpen");
        let _ = tx.send(id);
        return;
      }
    }
  });
  rx
}

async fn open_tunnel(harness: &Harness, companion: &Gateway) -> (TcpStream, uuid::Uuid) {
  announce(companion).await;
  assert!(
    harness
      .wait_for(|s| s.peers.connected_companion().is_some(), SETTLE)
      .await,
    "companion registered before the tunnel is opened"
  );

  let opened = spawn_open_responder(companion);
  let stream = socks_connect(harness.proxy_addr()).await;
  let tunnel_id = tokio::time::timeout(SETTLE, opened)
    .await
    .expect("daemon opened a tunnel")
    .expect("responder reported the id");
  (stream, tunnel_id)
}

async fn blast(companion: &Gateway, tunnel_id: uuid::Uuid, frames: u64) {
  for seq in 0..frames {
    companion
      .tunnel()
      .data(TunnelData {
        tunnel_id,
        bytes: frame(seq).into(),
      })
      .await
      .expect("send tunnel data");
  }
}

const DAEMON_MIN_SEND_WINDOW: usize = 64 * 1024;
const DAEMON_MAX_SEND_WINDOW: usize = 256 * 1024;
const DAEMON_READ_CHUNK: usize = 4 * 1024;

fn spawn_ack_tally(companion: &Gateway) -> Arc<AtomicU64> {
  let tally = Arc::new(AtomicU64::new(0));
  let mut inbound = companion.events();
  let out = tally.clone();
  tokio::spawn(async move {
    while let Ok(msg) = inbound.recv().await {
      if let BridgeToGatewayMsgData::Tunnel(BridgeToGatewayTunnelMsg::Ack(ack)) = &msg.data {
        out.fetch_add(u64::from(ack.consumed), Ordering::Relaxed);
      }
    }
  });
  tally
}

fn spawn_upstream_tally(companion: &Gateway, tunnel_id: uuid::Uuid) -> Arc<AtomicU64> {
  let tally = Arc::new(AtomicU64::new(0));
  let mut inbound = companion.events();
  let out = tally.clone();
  let companion = companion.clone();
  tokio::spawn(async move {
    let mut acked_once = false;
    while let Ok(msg) = inbound.recv().await {
      if let BridgeToGatewayMsgData::Tunnel(BridgeToGatewayTunnelMsg::Data(data)) = &msg.data {
        out.fetch_add(data.bytes.len() as u64, Ordering::Relaxed);
        if !acked_once {
          acked_once = true;
          companion
            .tunnel()
            .ack(TunnelAck {
              tunnel_id,
              consumed: data.bytes.len() as u32,
            })
            .await
            .expect("send the one ack");
        }
      }
    }
  });
  tally
}

async fn wait_for_at_least(tally: &AtomicU64, target: u64) -> bool {
  tokio::time::timeout(SETTLE, async {
    while tally.load(Ordering::Relaxed) < target {
      tokio::time::sleep(Duration::from_millis(20)).await;
    }
  })
  .await
  .is_ok()
}

async fn read_exactly(stream: &mut TcpStream, bytes: usize) -> Vec<u8> {
  let mut buf = vec![0u8; bytes];
  tokio::time::timeout(SETTLE, stream.read_exact(&mut buf))
    .await
    .expect("downstream bytes arrived")
    .expect("read");
  buf
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blasted_tunnel_arrives_in_order_and_intact() {
  let harness = Harness::start().await.expect("harness start");
  harness
    .activate_webapp_declaring(&["net.proxy"])
    .await
    .expect("activate a net.proxy webapp");
  let companion = harness.connect_android().await.expect("connect companion");

  let (mut socks, tunnel_id) = open_tunnel(&harness, &companion).await;

  let sender = tokio::spawn({
    let companion = companion.clone();
    async move { blast(&companion, tunnel_id, FRAME_COUNT).await }
  });

  let received = read_exactly(&mut socks, FRAME_BYTES * FRAME_COUNT as usize).await;
  sender.await.expect("sender finished");

  let seqs = decode_seqs(&received);
  let expected: Vec<u64> = (0..FRAME_COUNT).collect();
  assert!(
    corrupt_frames(&received).is_empty(),
    "frame bodies intact: {:?}",
    corrupt_frames(&received)
  );
  assert_eq!(seqs, expected, "every frame present, in the order the gateway sent it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_never_overtakes_data_already_queued_behind_it() {
  let harness = Harness::start().await.expect("harness start");
  harness
    .activate_webapp_declaring(&["net.proxy"])
    .await
    .expect("activate a net.proxy webapp");
  let companion = harness.connect_android().await.expect("connect companion");

  let (mut socks, tunnel_id) = open_tunnel(&harness, &companion).await;

  let sender = tokio::spawn({
    let companion = companion.clone();
    async move {
      blast(&companion, tunnel_id, FRAME_COUNT).await;
      companion
        .tunnel()
        .closed(TunnelClosed {
          tunnel_id,
          reason: None,
        })
        .await
        .expect("close the tunnel");
    }
  });

  let mut received = Vec::new();
  tokio::time::timeout(SETTLE, socks.read_to_end(&mut received))
    .await
    .expect("socket reached EOF rather than hanging")
    .expect("read to EOF");
  sender.await.expect("sender finished");

  assert_eq!(
    received.len(),
    FRAME_BYTES * FRAME_COUNT as usize,
    "close did not amputate the tail"
  );
  assert_eq!(decode_seqs(&received), (0..FRAME_COUNT).collect::<Vec<_>>());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_daemon_acks_downstream_bytes_once_they_reach_the_socket() {
  let harness = Harness::start().await.expect("harness start");
  harness
    .activate_webapp_declaring(&["net.proxy"])
    .await
    .expect("activate a net.proxy webapp");
  let companion = harness.connect_android().await.expect("connect companion");

  let (mut socks, tunnel_id) = open_tunnel(&harness, &companion).await;
  let acked = spawn_ack_tally(&companion);

  let sender = tokio::spawn({
    let companion = companion.clone();
    async move { blast(&companion, tunnel_id, FRAME_COUNT).await }
  });
  let total = (FRAME_BYTES * FRAME_COUNT as usize) as u64;
  read_exactly(&mut socks, total as usize).await;
  sender.await.expect("sender finished");

  assert!(
    wait_for_at_least(&acked, total).await,
    "every delivered byte was acked; saw {} of {total}",
    acked.load(Ordering::Relaxed)
  );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upstream_tunnel_stops_reading_once_the_companion_stops_acking() {
  let harness = Harness::start().await.expect("harness start");
  harness
    .activate_webapp_declaring(&["net.proxy"])
    .await
    .expect("activate a net.proxy webapp");
  let companion = harness.connect_android().await.expect("connect companion");

  let (mut socks, tunnel_id) = open_tunnel(&harness, &companion).await;
  let forwarded = spawn_upstream_tally(&companion, tunnel_id);

  socks
    .write_all(&vec![0xab; DAEMON_READ_CHUNK])
    .await
    .expect("prime the tunnel");
  assert!(
    harness
      .wait_for(|s| s.tunnel_routes.consumed(tunnel_id).is_some_and(|c| c > 0), SETTLE)
      .await,
    "the daemon registered the companion's one ack"
  );

  let offered = DAEMON_MAX_SEND_WINDOW * 4;
  let writer = tokio::spawn(async move {
    let _ = socks.write_all(&vec![0xab; offered]).await;
    socks
  });

  assert!(
    wait_for_at_least(&forwarded, DAEMON_MIN_SEND_WINDOW as u64).await,
    "the daemon forwards at least a floor window past the one ack"
  );
  tokio::time::sleep(Duration::from_secs(1)).await;

  let seen = forwarded.load(Ordering::Relaxed) as usize;
  assert!(
    seen <= DAEMON_MAX_SEND_WINDOW + 2 * DAEMON_READ_CHUNK,
    "a silent-ack tunnel stalls at the window instead of draining {offered} bytes into the link; forwarded {seen}"
  );
  writer.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_the_companion_closes_the_local_socket_instead_of_orphaning_it() {
  let harness = Harness::start().await.expect("harness start");
  harness
    .activate_webapp_declaring(&["net.proxy"])
    .await
    .expect("activate a net.proxy webapp");
  let companion = harness.connect_android().await.expect("connect companion");

  let (mut socks, tunnel_id) = open_tunnel(&harness, &companion).await;

  blast(&companion, tunnel_id, 4).await;
  let head = read_exactly(&mut socks, FRAME_BYTES * 4).await;
  assert_eq!(decode_seqs(&head), vec![0, 1, 2, 3], "transfer was underway");

  drop(companion);

  let mut tail = Vec::new();
  tokio::time::timeout(SETTLE, socks.read_to_end(&mut tail))
    .await
    .expect("socket reached EOF rather than hanging forever")
    .expect("read to EOF");
  assert!(tail.is_empty(), "nothing arrives after the gateway is gone");
}
