use std::{
  collections::HashMap,
  sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
  },
  time::Duration,
};

use bridgething_gateway::{HandlerError, OutboundLink, Reply, TunnelHandler};
use bytes::Bytes;
use libbridgething::{
  Priority, TunnelAck, TunnelClosed, TunnelData, TunnelError,
  gateway::{GatewayToBridgeTunnelMsgEvent, TunnelErrorReply, TunnelOpen, TunnelOpenReply},
  wire::{MsgMeta, WireError},
};
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpStream,
  sync::mpsc,
  task::JoinHandle,
};
use uuid::Uuid;

use crate::{
  seam::Clock,
  transfer::{ACK_INTERVAL_BYTES, ACK_STALL_TIMEOUT, AckWindow, FRAGMENT_BYTES, Pacer},
};

pub const ACK_FLUSH_INTERVAL: Duration = Duration::from_millis(300);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy)]
pub struct TunnelConfig {
  pub connect_timeout: Duration,
  pub ack_stall_timeout: Duration,
  pub ack_flush_interval: Duration,
}

impl Default for TunnelConfig {
  fn default() -> Self {
    Self {
      connect_timeout: CONNECT_TIMEOUT,
      ack_stall_timeout: ACK_STALL_TIMEOUT,
      ack_flush_interval: ACK_FLUSH_INTERVAL,
    }
  }
}

type Tunnels = Arc<Mutex<HashMap<Uuid, Tunnel>>>;

struct Tunnel {
  writes: mpsc::UnboundedSender<Bytes>,
  tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

pub struct TunnelDispatcher {
  link: Arc<dyn OutboundLink>,
  clock: Arc<dyn Clock>,
  config: TunnelConfig,
  acks: Arc<AckWindow>,
  tunnels: Tunnels,
}

impl TunnelDispatcher {
  pub fn new(link: Arc<dyn OutboundLink>, clock: Arc<dyn Clock>) -> Self {
    Self::with_config(link, clock, TunnelConfig::default())
  }

  pub fn with_config(link: Arc<dyn OutboundLink>, clock: Arc<dyn Clock>, config: TunnelConfig) -> Self {
    Self {
      link,
      clock,
      config,
      acks: Arc::new(AckWindow::new()),
      tunnels: Arc::new(Mutex::new(HashMap::new())),
    }
  }

  pub fn stop(&self) {
    let held: Vec<Uuid> = self.tunnels.lock().unwrap().keys().copied().collect();
    for id in held {
      self.teardown(id);
    }
  }

  fn teardown(&self, id: Uuid) {
    teardown(&self.tunnels, &self.acks, id);
  }
}

fn teardown(tunnels: &Tunnels, acks: &AckWindow, id: Uuid) {
  let tunnel = tunnels.lock().unwrap().remove(&id);
  if let Some(tunnel) = tunnel {
    for task in tunnel.tasks.lock().unwrap().drain(..) {
      task.abort();
    }
  }
  acks.finish(id);
}

struct TunnelTasks {
  id: Uuid,
  link: Arc<dyn OutboundLink>,
  acks: Arc<AckWindow>,
  tunnels: Tunnels,
  delivered: Arc<AtomicU64>,
}

impl TunnelTasks {
  async fn emit(&self, event: GatewayToBridgeTunnelMsgEvent, priority: Priority) {
    let _ = self.link.send_data(MsgMeta::Event, event.into(), priority).await;
  }

  fn teardown(&self) {
    teardown(&self.tunnels, &self.acks, self.id);
  }

  async fn flush_ack(&self) {
    let pending = self.delivered.swap(0, Ordering::SeqCst);
    if pending == 0 {
      return;
    }
    self
      .emit(
        GatewayToBridgeTunnelMsgEvent::Ack(TunnelAck {
          tunnel_id: self.id,
          consumed: pending.min(u64::from(u32::MAX)) as u32,
        }),
        Priority::Normal,
      )
      .await;
  }
}

async fn write_loop(
  tasks: Arc<TunnelTasks>,
  mut socket: tokio::net::tcp::OwnedWriteHalf,
  mut writes: mpsc::UnboundedReceiver<Bytes>,
) {
  while let Some(bytes) = writes.recv().await {
    let len = bytes.len() as u64;
    if socket.write_all(&bytes).await.is_err() || socket.flush().await.is_err() {
      tasks
        .emit(
          GatewayToBridgeTunnelMsgEvent::Closed(TunnelClosed {
            tunnel_id: tasks.id,
            reason: Some("remote write failed".to_string()),
          }),
          Priority::Bulk,
        )
        .await;
      tasks.teardown();
      return;
    }
    if tasks.delivered.fetch_add(len, Ordering::SeqCst) + len >= ACK_INTERVAL_BYTES {
      tasks.flush_ack().await;
    }
  }
}

async fn flush_loop(tasks: Arc<TunnelTasks>, interval: Duration) {
  loop {
    tokio::time::sleep(interval).await;
    tasks.flush_ack().await;
  }
}

async fn pump_loop(
  tasks: Arc<TunnelTasks>,
  mut socket: tokio::net::tcp::OwnedReadHalf,
  clock: Arc<dyn Clock>,
  ack_stall_timeout: Duration,
) {
  let mut pacer = Pacer::new(clock, 0);
  let mut buf = vec![0u8; FRAGMENT_BYTES];
  let mut sent = 0u64;

  let reason = loop {
    pacer.observe(tasks.acks.received_bytes(tasks.id));
    if tasks
      .acks
      .await_window(tasks.id, sent, pacer.window_bytes(), ack_stall_timeout)
      .await
      .is_err()
    {
      break Some("ack window stalled".to_string());
    }

    match socket.read(&mut buf).await {
      Ok(0) => break None,
      Ok(read) => {
        sent += read as u64;
        tasks
          .emit(
            GatewayToBridgeTunnelMsgEvent::Data(TunnelData {
              tunnel_id: tasks.id,
              bytes: Bytes::copy_from_slice(&buf[..read]),
            }),
            Priority::Bulk,
          )
          .await;
      }
      Err(e) => break Some(e.to_string()),
    }
  };

  tasks
    .emit(
      GatewayToBridgeTunnelMsgEvent::Closed(TunnelClosed {
        tunnel_id: tasks.id,
        reason,
      }),
      Priority::Bulk,
    )
    .await;
  tasks.teardown();
}

impl TunnelHandler for TunnelDispatcher {
  async fn open(&self, request: TunnelOpen) -> Result<Reply<TunnelOpenReply>, HandlerError<TunnelErrorReply>> {
    let target = format!("{}:{}", request.host, request.port);
    let connecting = tokio::time::timeout(self.config.connect_timeout, TcpStream::connect(&target));
    let socket = match connecting.await {
      Ok(Ok(socket)) => socket,
      Ok(Err(e)) => return Err(connect_failed(e.to_string())),
      Err(_) => return Err(connect_failed(format!("connect to {target} timed out"))),
    };
    let _ = socket.set_nodelay(true);

    let id = request.tunnel_id;
    self.acks.open(id, 0);
    let (read_half, write_half) = socket.into_split();
    let (writes, write_rx) = mpsc::unbounded_channel::<Bytes>();
    let tasks = Arc::new(TunnelTasks {
      id,
      link: self.link.clone(),
      acks: self.acks.clone(),
      tunnels: self.tunnels.clone(),
      delivered: Arc::new(AtomicU64::new(0)),
    });

    let handles = Arc::new(Mutex::new(Vec::new()));
    self
      .tunnels
      .lock()
      .unwrap()
      .insert(id, Tunnel { writes, tasks: handles });

    let running = vec![
      tokio::spawn(write_loop(tasks.clone(), write_half, write_rx)),
      tokio::spawn(flush_loop(tasks.clone(), self.config.ack_flush_interval)),
      tokio::spawn(pump_loop(
        tasks,
        read_half,
        self.clock.clone(),
        self.config.ack_stall_timeout,
      )),
    ];
    let held = self.tunnels.lock().unwrap();
    match held.get(&id) {
      Some(tunnel) => *tunnel.tasks.lock().unwrap() = running,
      None => {
        drop(held);
        for task in running {
          task.abort();
        }
      }
    }
    Ok(TunnelOpenReply {}.into())
  }

  async fn data(&self, payload: TunnelData) -> Result<(), WireError> {
    let writes = self
      .tunnels
      .lock()
      .unwrap()
      .get(&payload.tunnel_id)
      .map(|tunnel| tunnel.writes.clone());
    let Some(writes) = writes else {
      tracing::trace!(tunnel_id = %payload.tunnel_id, "tunnel data for unknown tunnel; dropping");
      return Ok(());
    };
    let _ = writes.send(payload.bytes);
    Ok(())
  }

  async fn ack(&self, payload: TunnelAck) -> Result<(), WireError> {
    let total = self.acks.received_bytes(payload.tunnel_id) + u64::from(payload.consumed);
    self.acks.note(payload.tunnel_id, total);
    Ok(())
  }

  async fn close(&self, payload: TunnelClosed) -> Result<(), WireError> {
    self.teardown(payload.tunnel_id);
    Ok(())
  }
}

fn connect_failed(reason: String) -> HandlerError<TunnelErrorReply> {
  HandlerError::Domain(TunnelErrorReply {
    error: TunnelError::ConnectFailed { reason },
  })
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use bridgething_gateway::TunnelHandler;
  use libbridgething::{
    Priority, TunnelAck, TunnelClosed, TunnelData, TunnelError,
    gateway::{GatewayToBridgeMsgData, GatewayToBridgeTunnelMsg, TunnelOpen},
  };
  use tokio::io::{AsyncReadExt, AsyncWriteExt};
  use uuid::Uuid;

  use super::*;
  use crate::{
    harness::{FakeDevice, linked_gateway, pattern},
    seam::SystemClock,
    transfer::MIN_WINDOW_BYTES,
  };

  struct Rig {
    dispatcher: TunnelDispatcher,
    device: FakeDevice,
  }

  async fn no_routes_left(dispatcher: &TunnelDispatcher) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !dispatcher.tunnels.lock().unwrap().is_empty() {
      assert!(
        tokio::time::Instant::now() < deadline,
        "a tunnel that ended itself must drop its route rather than leave one nothing can use"
      );
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  }

  fn rig() -> Rig {
    rig_with(TunnelConfig::default())
  }

  fn rig_with(config: TunnelConfig) -> Rig {
    let (gateway, device) = linked_gateway();
    Rig {
      dispatcher: TunnelDispatcher::with_config(Arc::new(gateway), Arc::new(SystemClock), config),
      device,
    }
  }

  struct EchoServer {
    port: u16,
    _task: tokio::task::JoinHandle<()>,
  }

  async fn echo_server() -> EchoServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let port = listener.local_addr().expect("a bound address").port();
    let task = tokio::spawn(async move {
      while let Ok((mut socket, _)) = listener.accept().await {
        tokio::spawn(async move {
          let mut buf = vec![0u8; 64 * 1024];
          loop {
            match socket.read(&mut buf).await {
              Ok(0) | Err(_) => return,
              Ok(read) => {
                if socket.write_all(&buf[..read]).await.is_err() {
                  return;
                }
              }
            }
          }
        });
      }
    });
    EchoServer { port, _task: task }
  }

  async fn source_server(body: Vec<u8>) -> EchoServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let port = listener.local_addr().expect("a bound address").port();
    let task = tokio::spawn(async move {
      while let Ok((mut socket, _)) = listener.accept().await {
        let body = body.clone();
        tokio::spawn(async move {
          let _ = socket.write_all(&body).await;
          let _ = socket.shutdown().await;
        });
      }
    });
    EchoServer { port, _task: task }
  }

  fn open_at(port: u16) -> TunnelOpen {
    TunnelOpen {
      tunnel_id: Uuid::now_v7(),
      host: "127.0.0.1".to_string(),
      port,
    }
  }

  impl FakeDevice {
    async fn next_tunnel_data(&mut self, id: Uuid) -> TunnelData {
      self
        .next_matching(|msg| match &msg.data {
          GatewayToBridgeMsgData::Tunnel(GatewayToBridgeTunnelMsg::Data(data)) if data.tunnel_id == id => {
            Some(data.clone())
          }
          _ => None,
        })
        .await
    }

    async fn next_tunnel_ack(&mut self, id: Uuid) -> TunnelAck {
      self
        .next_matching(|msg| match &msg.data {
          GatewayToBridgeMsgData::Tunnel(GatewayToBridgeTunnelMsg::Ack(ack)) if ack.tunnel_id == id => {
            Some(ack.clone())
          }
          _ => None,
        })
        .await
    }

    async fn next_tunnel_closed(&mut self, id: Uuid) -> TunnelClosed {
      self
        .next_matching(|msg| match &msg.data {
          GatewayToBridgeMsgData::Tunnel(GatewayToBridgeTunnelMsg::Closed(closed)) if closed.tunnel_id == id => {
            Some(closed.clone())
          }
          _ => None,
        })
        .await
    }

    async fn no_tunnel_data(&mut self, id: Uuid, window: Duration) -> bool {
      self
        .nothing_matching(window, |msg| match &msg.data {
          GatewayToBridgeMsgData::Tunnel(GatewayToBridgeTunnelMsg::Data(data)) if data.tunnel_id == id => Some(()),
          _ => None,
        })
        .await
    }
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn an_open_to_a_listening_port_succeeds() {
    let echo = echo_server().await;
    let rig = rig();

    rig
      .dispatcher
      .open(open_at(echo.port))
      .await
      .expect("a listening port opens");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn an_open_to_a_closed_port_answers_with_the_connect_failure() {
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let port = dead.local_addr().expect("a bound address").port();
    drop(dead);
    let rig = rig();

    let err = rig
      .dispatcher
      .open(open_at(port))
      .await
      .expect_err("nothing is listening");

    assert!(
      matches!(
        err,
        HandlerError::Domain(TunnelErrorReply {
          error: TunnelError::ConnectFailed { .. }
        })
      ),
      "a refused connect is a domain error the webapp can read, got {err:?}"
    );
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn an_open_that_never_completes_gives_up_rather_than_holding_the_request() {
    let rig = rig_with(TunnelConfig {
      connect_timeout: Duration::from_millis(150),
      ..TunnelConfig::default()
    });

    let err = rig
      .dispatcher
      .open(TunnelOpen {
        tunnel_id: Uuid::now_v7(),
        host: "10.255.255.1".to_string(),
        port: 9,
      })
      .await
      .expect_err("an unanswered connect must not hang the request");

    assert!(matches!(err, HandlerError::Domain(_)), "got {err:?}");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn bytes_from_the_device_reach_the_remote_and_come_back() {
    let echo = echo_server().await;
    let mut rig = rig();
    let open = open_at(echo.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the echo server is up");

    let payload = Bytes::from_static(b"ping-through-the-phone");
    rig
      .dispatcher
      .data(TunnelData {
        tunnel_id: id,
        bytes: payload.clone(),
      })
      .await
      .expect("a write to a live tunnel");

    let echoed = rig.device.next_tunnel_data(id).await;
    assert_eq!(echoed.tunnel_id, id);
    assert_eq!(echoed.bytes, payload);
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn inbound_data_reaches_the_remote_in_arrival_order() {
    let echo = echo_server().await;
    let mut rig = rig();
    let open = open_at(echo.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the echo server is up");

    let chunks: Vec<Bytes> = (0u8..16).map(|i| Bytes::from(vec![i; 512])).collect();
    for chunk in &chunks {
      rig
        .dispatcher
        .data(TunnelData {
          tunnel_id: id,
          bytes: chunk.clone(),
        })
        .await
        .expect("a write to a live tunnel");
    }

    let expected: Vec<u8> = chunks.iter().flat_map(|c| c.to_vec()).collect();
    let mut seen: Vec<u8> = Vec::new();
    while seen.len() < expected.len() {
      let data = rig.device.next_tunnel_data(id).await;
      seen.extend_from_slice(&data.bytes);
      rig
        .dispatcher
        .ack(TunnelAck {
          tunnel_id: id,
          consumed: data.bytes.len() as u32,
        })
        .await
        .expect("acking is infallible");
    }

    assert_eq!(
      seen, expected,
      "a tunnel's bytes must reach the socket in the order the link delivered them"
    );
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn a_short_write_is_still_acked_so_the_window_recovers() {
    let echo = echo_server().await;
    let mut rig = rig();
    let open = open_at(echo.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the echo server is up");

    let payload = Bytes::from(vec![0x7a; 1024]);
    rig
      .dispatcher
      .data(TunnelData {
        tunnel_id: id,
        bytes: payload.clone(),
      })
      .await
      .expect("a write to a live tunnel");

    let ack = rig.device.next_tunnel_ack(id).await;
    assert_eq!(ack.tunnel_id, id);
    assert_eq!(
      ack.consumed,
      payload.len() as u32,
      "a write below the ack threshold still has to be reported"
    );
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn a_write_past_the_threshold_is_acked_without_waiting_for_the_flush() {
    let echo = echo_server().await;
    let mut rig = rig_with(TunnelConfig {
      ack_flush_interval: Duration::from_secs(60),
      ..TunnelConfig::default()
    });
    let open = open_at(echo.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the echo server is up");

    let payload = Bytes::from(pattern(ACK_INTERVAL_BYTES as usize));
    rig
      .dispatcher
      .data(TunnelData {
        tunnel_id: id,
        bytes: payload.clone(),
      })
      .await
      .expect("a write to a live tunnel");

    let ack = rig.device.next_tunnel_ack(id).await;
    assert_eq!(ack.consumed, payload.len() as u32);
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn acks_from_the_device_accumulate_as_deltas() {
    let echo = echo_server().await;
    let rig = rig();
    let open = open_at(echo.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the echo server is up");

    rig
      .dispatcher
      .ack(TunnelAck {
        tunnel_id: id,
        consumed: 4096,
      })
      .await
      .expect("acking is infallible");
    rig
      .dispatcher
      .ack(TunnelAck {
        tunnel_id: id,
        consumed: 4096,
      })
      .await
      .expect("acking is infallible");

    assert_eq!(
      rig.dispatcher.acks.received_bytes(id),
      8192,
      "the wire carries a delta, so two of them are a running total"
    );
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn the_pump_stops_at_its_window_until_the_device_acks() {
    let body = pattern(MIN_WINDOW_BYTES as usize * 4);
    let source = source_server(body.clone()).await;
    let mut rig = rig();
    let open = open_at(source.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the source server is up");

    let mut streamed = 0u64;
    while streamed < MIN_WINDOW_BYTES {
      streamed += rig.device.next_tunnel_data(id).await.bytes.len() as u64;
    }
    assert!(
      rig.device.no_tunnel_data(id, Duration::from_millis(400)).await,
      "the pump ran past the unacked window"
    );

    rig
      .dispatcher
      .ack(TunnelAck {
        tunnel_id: id,
        consumed: streamed as u32,
      })
      .await
      .expect("acking is infallible");

    let mut seen = streamed;
    while seen < body.len() as u64 {
      let data = rig.device.next_tunnel_data(id).await;
      seen += data.bytes.len() as u64;
      rig
        .dispatcher
        .ack(TunnelAck {
          tunnel_id: id,
          consumed: data.bytes.len() as u32,
        })
        .await
        .expect("acking is infallible");
    }
    assert_eq!(seen, body.len() as u64, "the whole body streams once the window opens");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn an_ack_window_that_never_opens_closes_the_tunnel_rather_than_hanging() {
    let body = pattern(MIN_WINDOW_BYTES as usize * 4);
    let source = source_server(body).await;
    let mut rig = rig_with(TunnelConfig {
      ack_stall_timeout: Duration::from_millis(150),
      ..TunnelConfig::default()
    });
    let open = open_at(source.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the source server is up");

    let closed = rig.device.next_tunnel_closed(id).await;
    assert_eq!(
      closed.reason.as_deref(),
      Some("ack window stalled"),
      "a device that stops acking must end the tunnel, not park the pump"
    );
    no_routes_left(&rig.dispatcher).await;
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn a_remote_that_hangs_up_closes_the_tunnel_towards_the_device() {
    let source = source_server(b"short body".to_vec()).await;
    let mut rig = rig();
    let open = open_at(source.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the source server is up");

    let data = rig.device.next_tunnel_data(id).await;
    assert_eq!(data.bytes.as_ref(), b"short body");

    let closed = rig.device.next_tunnel_closed(id).await;
    assert_eq!(closed.reason, None, "a clean eof carries no failure reason");
    no_routes_left(&rig.dispatcher).await;
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn a_close_from_the_device_drops_the_write_route() {
    let echo = echo_server().await;
    let mut rig = rig();
    let open = open_at(echo.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the echo server is up");

    rig
      .dispatcher
      .close(TunnelClosed {
        tunnel_id: id,
        reason: None,
      })
      .await
      .expect("closing is infallible");

    assert!(
      rig.dispatcher.tunnels.lock().unwrap().is_empty(),
      "a closed tunnel keeps no route"
    );
    rig
      .dispatcher
      .data(TunnelData {
        tunnel_id: id,
        bytes: Bytes::from_static(b"after the close"),
      })
      .await
      .expect("a write to a dead tunnel is not an error");
    assert!(
      rig.device.no_tunnel_data(id, Duration::from_millis(300)).await,
      "nothing may come back through a torn-down tunnel"
    );
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn a_remote_that_stops_reading_never_holds_the_route_every_surface_shares() {
    let listener = tokio::net::TcpSocket::new_v4().expect("a socket");
    listener.set_recv_buffer_size(2048).expect("a small receive buffer");
    listener
      .bind("127.0.0.1:0".parse().expect("a loopback address"))
      .expect("a free port");
    let listener = listener.listen(1).expect("a listening socket");
    let port = listener.local_addr().expect("a bound address").port();
    let _accepting = tokio::spawn(async move {
      let _held = listener.accept().await.expect("the dispatcher dials");
      std::future::pending::<()>().await;
    });

    let rig = rig();
    let open = open_at(port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the listener is up");

    let chunk = Bytes::from(vec![0x5a; 4096]);
    let feeding = async {
      for _ in 0..2048 {
        rig
          .dispatcher
          .data(TunnelData {
            tunnel_id: id,
            bytes: chunk.clone(),
          })
          .await
          .expect("a write to a live tunnel");
      }
    };

    tokio::time::timeout(Duration::from_secs(5), feeding)
      .await
      .expect("one stalled remote must never park the inbound route the whole session shares");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn data_for_an_unknown_tunnel_is_dropped_rather_than_failing() {
    let rig = rig();

    rig
      .dispatcher
      .data(TunnelData {
        tunnel_id: Uuid::now_v7(),
        bytes: Bytes::from_static(b"nowhere to go"),
      })
      .await
      .expect("an unknown tunnel is not a protocol error");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn stopping_the_dispatcher_drops_every_tunnel() {
    let echo = echo_server().await;
    let rig = rig();
    let first = open_at(echo.port);
    let second = open_at(echo.port);
    rig.dispatcher.open(first).await.expect("the echo server is up");
    rig.dispatcher.open(second).await.expect("the echo server is up");

    rig.dispatcher.stop();

    assert!(rig.dispatcher.tunnels.lock().unwrap().is_empty());
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn tunnel_body_rides_the_bulk_lane_and_acks_ride_the_normal_one() {
    let echo = echo_server().await;
    let mut rig = rig();
    let open = open_at(echo.port);
    let id = open.tunnel_id;
    rig.dispatcher.open(open).await.expect("the echo server is up");

    rig
      .dispatcher
      .data(TunnelData {
        tunnel_id: id,
        bytes: Bytes::from(vec![0x11; 2048]),
      })
      .await
      .expect("a write to a live tunnel");
    rig.device.next_tunnel_data(id).await;
    rig.device.next_tunnel_ack(id).await;

    let body = rig.device.lanes_of(|data| match data {
      GatewayToBridgeMsgData::Tunnel(GatewayToBridgeTunnelMsg::Data(_)) => Some(()),
      _ => None,
    });
    let acks = rig.device.lanes_of(|data| match data {
      GatewayToBridgeMsgData::Tunnel(GatewayToBridgeTunnelMsg::Ack(_)) => Some(()),
      _ => None,
    });

    assert!(
      body.iter().all(|lane| *lane == Priority::Bulk),
      "proxied bytes must never share a lane with what the screen is doing, got {body:?}"
    );
    assert!(
      acks.iter().all(|lane| *lane == Priority::Normal),
      "an ack that queues behind the bytes it is meant to unblock is useless, got {acks:?}"
    );
  }
}
