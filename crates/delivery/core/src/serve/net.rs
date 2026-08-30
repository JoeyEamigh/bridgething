use std::{
  collections::{HashMap, HashSet},
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_gateway::{HandlerError, NetHandler, OutboundLink, Reply};
use bridgething_io::{self as io, DownloadBody, HttpError, HttpExecutor, WsTransport};
use libbridgething::{
  HttpHeader, HttpMethod, NetError, NetFetchRequest, NetFetchResponse, Priority, StreamBegin, StreamChunk, StreamEnd,
  StreamError, WsError, WsFrame,
  gateway::{
    GatewayToBridgeNetMsgEvent, NetFetchErrorReply, NetFetchReply, NetFetchRequestMsg, NetStreamCancel, NetStreamOpen,
    NetWsClose, NetWsClosed, NetWsErrorReply, NetWsMessage, NetWsOpen, NetWsOpenReply, NetWsSend,
  },
  wire::{MsgMeta, WireError},
};
use tokio::{
  sync::{mpsc, oneshot},
  task::JoinHandle,
};
use uuid::Uuid;

pub const STREAM_CHUNK_BYTES: usize = 8 * 1024;
pub const STREAM_BUFFER_BUDGET_BYTES: usize = 4 * 1024 * 1024;
pub const WS_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
pub const BEHIND_REASON: &str = "the link fell too far behind this stream";
const WS_ABNORMAL_CLOSURE: u16 = 1006;

#[derive(Debug, Clone, Copy)]
pub struct NetConfig {
  pub ws_open_timeout: Duration,
  pub stream_chunk_bytes: usize,
  pub stream_buffer_budget_bytes: usize,
}

impl Default for NetConfig {
  fn default() -> Self {
    Self {
      ws_open_timeout: WS_OPEN_TIMEOUT,
      stream_chunk_bytes: STREAM_CHUNK_BYTES,
      stream_buffer_budget_bytes: STREAM_BUFFER_BUDGET_BYTES,
    }
  }
}

#[derive(Default)]
struct Sockets {
  pending: HashMap<Uuid, oneshot::Sender<Result<Option<String>, String>>>,
  live: HashSet<Uuid>,
}

pub struct NetDispatcher {
  link: Arc<dyn OutboundLink>,
  http: HttpExecutor,
  ws: Arc<dyn WsTransport>,
  inbox: Arc<io::WsInbox>,
  config: NetConfig,
  sockets: Arc<Mutex<Sockets>>,
  streams: Streams,
  pending: Mutex<Option<mpsc::UnboundedReceiver<io::WsEvent>>>,
  events: Mutex<Option<JoinHandle<()>>>,
}

type Streams = Arc<Mutex<HashMap<Uuid, Arc<StreamRun>>>>;

struct StreamRun {
  cancel: Mutex<Option<oneshot::Sender<()>>>,
}

impl StreamRun {
  fn cancel(&self) {
    self.cancel.lock().unwrap().take();
  }
}

impl NetDispatcher {
  pub fn new(link: Arc<dyn OutboundLink>, http: HttpExecutor, ws: Arc<dyn WsTransport>) -> Self {
    Self::with_config(link, http, ws, NetConfig::default())
  }

  pub fn with_config(
    link: Arc<dyn OutboundLink>,
    http: HttpExecutor,
    ws: Arc<dyn WsTransport>,
    config: NetConfig,
  ) -> Self {
    let (tx, rx) = mpsc::unbounded_channel();
    let sockets = Arc::new(Mutex::new(Sockets::default()));
    Self {
      link,
      http,
      ws,
      inbox: Arc::new(io::WsInbox::new(tx)),
      config,
      sockets,
      streams: Arc::new(Mutex::new(HashMap::new())),
      pending: Mutex::new(Some(rx)),
      events: Mutex::new(None),
    }
  }

  pub fn start(&self) {
    let Some(rx) = self.pending.lock().unwrap().take() else {
      return;
    };
    let events = tokio::spawn(ws_events(self.link.clone(), rx, self.sockets.clone()));
    *self.events.lock().unwrap() = Some(events);
  }

  pub fn stop(&self) {
    let held: Vec<Uuid> = {
      let mut sockets = self.sockets.lock().unwrap();
      sockets.pending.clear();
      sockets.live.drain().collect()
    };
    for id in held {
      self.ws.disconnect(id, None, None);
    }
    for (_, run) in self.streams.lock().unwrap().drain() {
      run.cancel();
    }
  }
}

impl Drop for NetDispatcher {
  fn drop(&mut self) {
    if let Some(events) = self.events.lock().unwrap().take() {
      events.abort();
    }
  }
}

async fn ws_events(
  link: Arc<dyn OutboundLink>,
  mut rx: mpsc::UnboundedReceiver<io::WsEvent>,
  sockets: Arc<Mutex<Sockets>>,
) {
  while let Some(event) = rx.recv().await {
    match event {
      io::WsEvent::Open { id, accepted_protocol } => {
        let waiting = {
          let mut held = sockets.lock().unwrap();
          held.live.insert(id);
          held.pending.remove(&id)
        };
        if let Some(waiting) = waiting {
          let _ = waiting.send(Ok(accepted_protocol));
        }
      }
      io::WsEvent::Frame { id, frame } => {
        let _ = link
          .send_data(
            MsgMeta::Event,
            GatewayToBridgeNetMsgEvent::WsMessage(NetWsMessage {
              connection_id: id,
              frame: wire_frame(frame),
            })
            .into(),
            Priority::Normal,
          )
          .await;
      }
      io::WsEvent::Closed { id, code, reason } => {
        let (waiting, was_live) = {
          let mut held = sockets.lock().unwrap();
          (held.pending.remove(&id), held.live.remove(&id))
        };
        if let Some(waiting) = waiting {
          let _ = waiting.send(Err(reason));
          continue;
        }
        if !was_live {
          tracing::trace!(connection_id = %id, "ws close for unknown connection; dropping");
          continue;
        }
        let _ = link
          .send_data(
            MsgMeta::Event,
            GatewayToBridgeNetMsgEvent::WsClosed(NetWsClosed {
              connection_id: id,
              code: code.unwrap_or(WS_ABNORMAL_CLOSURE),
              reason,
            })
            .into(),
            Priority::Normal,
          )
          .await;
      }
    }
  }
}

struct StreamWriter {
  chunks: mpsc::UnboundedSender<Vec<u8>>,
  #[allow(clippy::type_complexity)]
  head: Option<oneshot::Sender<(u16, Vec<HttpHeader>, Option<u64>)>>,
  buffer: Vec<u8>,
  chunk_bytes: usize,
  queued: Arc<Mutex<usize>>,
  budget: usize,
  dropped: Arc<Mutex<Option<String>>>,
}

impl StreamWriter {
  fn push(&mut self, chunk: Vec<u8>) -> Result<(), String> {
    let fits = {
      let mut queued = self.queued.lock().unwrap();
      let fits = *queued + chunk.len() <= self.budget;
      if fits {
        *queued += chunk.len();
      }
      fits
    };
    if !fits {
      *self.dropped.lock().unwrap() = Some(BEHIND_REASON.to_string());
      return Err(BEHIND_REASON.to_string());
    }
    let _ = self.chunks.send(chunk);
    Ok(())
  }
}

impl DownloadBody for StreamWriter {
  fn on_response(&mut self, status: u16, headers: &[io::HttpHeader], content_length: Option<u64>) -> bool {
    if let Some(head) = self.head.take() {
      let _ = head.send((status, headers.iter().map(wire_header).collect(), content_length));
    }
    true
  }

  fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
    self.buffer.extend_from_slice(chunk);
    while self.buffer.len() >= self.chunk_bytes {
      let rest = self.buffer.split_off(self.chunk_bytes);
      let full = std::mem::replace(&mut self.buffer, rest);
      self.push(full)?;
    }
    Ok(())
  }
}

impl Drop for StreamWriter {
  fn drop(&mut self) {
    if !self.buffer.is_empty() {
      let tail = std::mem::take(&mut self.buffer);
      let _ = self.push(tail);
    }
  }
}

impl NetHandler for NetDispatcher {
  async fn fetch(&self, request: NetFetchRequestMsg) -> Result<Reply<NetFetchReply>, HandlerError<NetFetchErrorReply>> {
    let sending = self.http.execute(io_request(&request.request));
    let outcome = match request.request.timeout_ms {
      Some(ms) => match tokio::time::timeout(Duration::from_millis(u64::from(ms)), sending).await {
        Ok(outcome) => outcome,
        Err(_) => return Err(fetch_failed(NetError::Timeout)),
      },
      None => sending.await,
    };

    match outcome {
      Ok(response) => Ok(
        NetFetchReply {
          response: NetFetchResponse {
            status: response.status,
            headers: response.headers.iter().map(wire_header).collect(),
            body: response.body,
          },
        }
        .into(),
      ),
      Err(e) => Err(fetch_failed(net_error(e))),
    }
  }

  async fn ws_open(&self, request: NetWsOpen) -> Result<Reply<NetWsOpenReply>, HandlerError<NetWsErrorReply>> {
    let id = request.connection_id;
    let (tx, rx) = oneshot::channel();
    self.sockets.lock().unwrap().pending.insert(id, tx);
    self.ws.connect(
      io::WsConnect {
        id,
        url: request.url,
        protocols: request.protocols.unwrap_or_default(),
        headers: request.headers.unwrap_or_default().iter().map(io_header).collect(),
      },
      self.inbox.clone(),
    );

    let opened = match tokio::time::timeout(self.config.ws_open_timeout, rx).await {
      Ok(Ok(opened)) => opened,
      Ok(Err(_)) => Err("the websocket transport went away".to_string()),
      Err(_) => Err("connect timed out".to_string()),
    };
    match opened {
      Ok(accepted_protocol) => Ok(NetWsOpenReply { accepted_protocol }.into()),
      Err(reason) => {
        self.sockets.lock().unwrap().pending.remove(&id);
        self.ws.disconnect(id, None, None);
        Err(HandlerError::Domain(NetWsErrorReply {
          error: WsError::ConnectFailed { reason },
        }))
      }
    }
  }

  async fn ws_close(&self, payload: NetWsClose) -> Result<(), WireError> {
    if self.sockets.lock().unwrap().live.contains(&payload.connection_id) {
      self.ws.disconnect(payload.connection_id, payload.code, payload.reason);
    }
    Ok(())
  }

  async fn ws_send(&self, payload: NetWsSend) -> Result<(), WireError> {
    if !self.sockets.lock().unwrap().live.contains(&payload.connection_id) {
      tracing::trace!(connection_id = %payload.connection_id, "ws send for unknown connection; dropping");
      return Ok(());
    }
    self.ws.send(payload.connection_id, io_frame(payload.frame));
    Ok(())
  }

  async fn stream_open(&self, payload: NetStreamOpen) -> Result<(), WireError> {
    let id = payload.stream_id;
    let (cancel, cancelled) = oneshot::channel();
    let run = Arc::new(StreamRun {
      cancel: Mutex::new(Some(cancel)),
    });
    if let Some(previous) = self.streams.lock().unwrap().insert(id, run.clone()) {
      previous.cancel();
    }
    tokio::spawn(run_stream(
      self.link.clone(),
      self.http.clone(),
      self.config,
      id,
      payload.request,
      cancelled,
      self.streams.clone(),
      run,
    ));
    Ok(())
  }

  async fn stream_cancel(&self, payload: NetStreamCancel) -> Result<(), WireError> {
    if let Some(run) = self.streams.lock().unwrap().remove(&payload.stream_id) {
      run.cancel();
    }
    Ok(())
  }
}

#[allow(clippy::too_many_arguments)]
async fn run_stream(
  link: Arc<dyn OutboundLink>,
  http: HttpExecutor,
  config: NetConfig,
  id: Uuid,
  request: NetFetchRequest,
  cancelled: oneshot::Receiver<()>,
  streams: Streams,
  run: Arc<StreamRun>,
) {
  let (head_tx, head_rx) = oneshot::channel();
  let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<Vec<u8>>();
  let queued = Arc::new(Mutex::new(0usize));
  let dropped = Arc::new(Mutex::new(None));
  let writer = StreamWriter {
    chunks: chunk_tx,
    head: Some(head_tx),
    buffer: Vec::new(),
    chunk_bytes: config.stream_chunk_bytes,
    queued: queued.clone(),
    budget: config.stream_buffer_budget_bytes,
    dropped: dropped.clone(),
  };

  let emitting = async {
    let Ok((status, headers, content_length)) = head_rx.await else {
      return;
    };
    let _ = link
      .send_data(
        MsgMeta::Event,
        GatewayToBridgeNetMsgEvent::StreamBegin(StreamBegin {
          stream_id: id,
          status,
          headers,
          total_size: content_length.map(|len| len.min(u64::from(u32::MAX)) as u32),
        })
        .into(),
        Priority::Bulk,
      )
      .await;

    let mut offset: u32 = 0;
    while let Some(chunk) = chunk_rx.recv().await {
      *queued.lock().unwrap() -= chunk.len();
      let len = chunk.len() as u32;
      let _ = link
        .send_data(
          MsgMeta::Event,
          GatewayToBridgeNetMsgEvent::StreamChunk(StreamChunk {
            stream_id: id,
            offset,
            bytes: chunk,
          })
          .into(),
          Priority::Bulk,
        )
        .await;
      offset = offset.saturating_add(len);
    }
  };

  let downloading = http.download(io_request(&request), Box::new(writer));
  let outcome = tokio::select! {
    _ = cancelled => return,
    (outcome, ()) = futures::future::join(downloading, emitting) => outcome,
  };

  let truncated = dropped.lock().unwrap().take();
  let terminal = match (outcome, truncated) {
    (Ok(_), None) => GatewayToBridgeNetMsgEvent::StreamEnd(StreamEnd { stream_id: id }),
    (Ok(_), Some(reason)) => GatewayToBridgeNetMsgEvent::StreamError(StreamError {
      stream_id: id,
      error: NetError::RequestFailed { reason },
    }),
    (Err(e), _) => GatewayToBridgeNetMsgEvent::StreamError(StreamError {
      stream_id: id,
      error: net_error(e),
    }),
  };
  let _ = link.send_data(MsgMeta::Event, terminal.into(), Priority::Bulk).await;

  let mut held = streams.lock().unwrap();
  if held.get(&id).is_some_and(|current| Arc::ptr_eq(current, &run)) {
    held.remove(&id);
  }
}

fn fetch_failed(error: NetError) -> HandlerError<NetFetchErrorReply> {
  HandlerError::Domain(NetFetchErrorReply { error })
}

fn net_error(e: HttpError) -> NetError {
  match e {
    HttpError::InvalidRequest(reason) | HttpError::Transport(reason) | HttpError::Body(reason) => {
      NetError::RequestFailed { reason }
    }
    HttpError::Dropped => NetError::Unavailable,
  }
}

fn io_request(request: &NetFetchRequest) -> io::HttpRequest {
  io::HttpRequest {
    method: io_method(request.method),
    url: request.url.clone(),
    headers: request.headers.iter().map(io_header).collect(),
    body: request.body.clone().unwrap_or_default(),
    timeout_ms: request.timeout_ms.unwrap_or(0),
  }
}

fn io_method(method: HttpMethod) -> io::HttpMethod {
  match method {
    HttpMethod::Get => io::HttpMethod::Get,
    HttpMethod::Head => io::HttpMethod::Head,
    HttpMethod::Post => io::HttpMethod::Post,
    HttpMethod::Put => io::HttpMethod::Put,
    HttpMethod::Patch => io::HttpMethod::Patch,
    HttpMethod::Delete => io::HttpMethod::Delete,
    HttpMethod::Options => io::HttpMethod::Options,
  }
}

fn io_header(header: &HttpHeader) -> io::HttpHeader {
  io::HttpHeader {
    name: header.name.clone(),
    value: header.value.clone(),
  }
}

fn wire_header(header: &io::HttpHeader) -> HttpHeader {
  HttpHeader {
    name: header.name.clone(),
    value: header.value.clone(),
  }
}

fn io_frame(frame: WsFrame) -> io::WsFrame {
  match frame {
    WsFrame::Text(text) => io::WsFrame::Text(text),
    WsFrame::Binary(bytes) => io::WsFrame::Binary(bytes),
  }
}

fn wire_frame(frame: io::WsFrame) -> WsFrame {
  match frame {
    io::WsFrame::Text(text) => WsFrame::Text(text),
    io::WsFrame::Binary(bytes) => WsFrame::Binary(bytes),
  }
}

#[cfg(test)]
mod tests {
  use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
  };

  use bridgething_io::{HttpDownloadSink, HttpRequest, HttpResponse, HttpSink, HttpTransport};
  use libbridgething::{
    RedirectPolicy,
    gateway::{GatewayToBridgeMsgData, GatewayToBridgeNetMsg},
  };

  use super::*;
  use crate::harness::{FakeDevice, linked_gateway, pattern};

  struct Silent;

  #[async_trait::async_trait]
  impl OutboundLink for Silent {
    async fn send_data(
      &self,
      _meta: libbridgething::wire::MsgMeta,
      _data: GatewayToBridgeMsgData,
      _priority: libbridgething::Priority,
    ) -> Result<(), bridgething_gateway::SdkError> {
      Ok(())
    }
  }

  #[test]
  fn constructing_a_dispatcher_outside_a_runtime_does_not_panic() {
    let dispatcher = NetDispatcher::new(
      Arc::new(Silent),
      HttpExecutor::new(FakeHttp::scripted(Vec::new())),
      FakeWs::new(WsMode::Manual),
    );
    dispatcher.stop();
  }

  enum Script {
    Reply(HttpResponse),
    Fail(String),
    Silent,
    Holds,
    Stream {
      status: u16,
      headers: Vec<io::HttpHeader>,
      content_length: Option<u64>,
      chunks: Vec<Vec<u8>>,
      failure: Option<String>,
    },
  }

  #[derive(Default)]
  struct FakeHttp {
    script: Mutex<VecDeque<Script>>,
    seen: Mutex<Vec<HttpRequest>>,
    held: Mutex<Vec<Box<dyn std::any::Any + Send>>>,
  }

  impl FakeHttp {
    fn scripted(steps: Vec<Script>) -> Arc<Self> {
      Arc::new(Self {
        script: Mutex::new(steps.into()),
        seen: Mutex::new(Vec::new()),
        held: Mutex::new(Vec::new()),
      })
    }

    fn next(&self, request: HttpRequest) -> Script {
      self.seen.lock().unwrap().push(request);
      self.script.lock().unwrap().pop_front().expect("a scripted response")
    }
  }

  impl HttpTransport for FakeHttp {
    fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>) {
      match self.next(request) {
        Script::Reply(response) => sink.complete(response),
        Script::Fail(reason) => sink.fail(reason),
        Script::Silent => {}
        Script::Holds => self.held.lock().unwrap().push(Box::new(sink)),
        Script::Stream { .. } => unreachable!("a fetch never takes the streaming arm"),
      }
    }

    fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      match self.next(request) {
        Script::Stream {
          status,
          headers,
          content_length,
          chunks,
          failure,
        } => {
          sink.on_response(status, headers, content_length);
          for chunk in chunks {
            sink.on_chunk(chunk);
          }
          match failure {
            Some(reason) => sink.on_failed(reason),
            None => sink.on_finished(),
          }
        }
        Script::Silent => {}
        Script::Holds => self.held.lock().unwrap().push(Box::new(sink)),
        _ => unreachable!("a stream never takes the whole-body arm"),
      }
    }
  }

  #[derive(Clone)]
  enum WsMode {
    Opens { accepted_protocol: Option<String> },
    Refuses { reason: String },
    Manual,
  }

  struct FakeWs {
    mode: WsMode,
    inbox: Mutex<Option<Arc<io::WsInbox>>>,
    connects: Mutex<Vec<io::WsConnect>>,
    sent: Mutex<Vec<(Uuid, io::WsFrame)>>,
    #[allow(clippy::type_complexity)]
    disconnects: Mutex<Vec<(Uuid, Option<u16>, Option<String>)>>,
  }

  impl FakeWs {
    fn new(mode: WsMode) -> Arc<Self> {
      Arc::new(Self {
        mode,
        inbox: Mutex::new(None),
        connects: Mutex::new(Vec::new()),
        sent: Mutex::new(Vec::new()),
        disconnects: Mutex::new(Vec::new()),
      })
    }

    fn inbox(&self) -> Arc<io::WsInbox> {
      self
        .inbox
        .lock()
        .unwrap()
        .clone()
        .expect("the dispatcher dialled first")
    }
  }

  impl WsTransport for FakeWs {
    fn connect(&self, connect: io::WsConnect, inbox: Arc<io::WsInbox>) {
      let id = connect.id;
      self.connects.lock().unwrap().push(connect);
      *self.inbox.lock().unwrap() = Some(inbox.clone());
      match &self.mode {
        WsMode::Opens { accepted_protocol } => inbox.on_open(id, accepted_protocol.clone()),
        WsMode::Refuses { reason } => inbox.on_closed(id, None, reason.clone()),
        WsMode::Manual => {}
      }
    }

    fn send(&self, id: Uuid, frame: io::WsFrame) {
      self.sent.lock().unwrap().push((id, frame));
    }

    fn disconnect(&self, id: Uuid, code: Option<u16>, reason: Option<String>) {
      self.disconnects.lock().unwrap().push((id, code, reason));
    }
  }

  struct Rig {
    dispatcher: Arc<NetDispatcher>,
    http: Arc<FakeHttp>,
    ws: Arc<FakeWs>,
    device: FakeDevice,
  }

  fn rig(steps: Vec<Script>, mode: WsMode) -> Rig {
    rig_with(steps, mode, NetConfig::default())
  }

  fn rig_with(steps: Vec<Script>, mode: WsMode, config: NetConfig) -> Rig {
    let (gateway, device) = linked_gateway();
    let http = FakeHttp::scripted(steps);
    let ws = FakeWs::new(mode);
    let dispatcher = Arc::new(NetDispatcher::with_config(
      Arc::new(gateway),
      HttpExecutor::new(http.clone()),
      ws.clone(),
      config,
    ));
    dispatcher.start();
    Rig {
      dispatcher,
      http,
      ws,
      device,
    }
  }

  fn get(url: &str) -> NetFetchRequest {
    NetFetchRequest {
      url: url.to_string(),
      method: HttpMethod::Get,
      headers: Vec::new(),
      body: None,
      timeout_ms: None,
      redirect: RedirectPolicy::Follow,
    }
  }

  fn ok_response(body: &[u8]) -> HttpResponse {
    HttpResponse {
      status: 200,
      headers: vec![io::HttpHeader {
        name: "content-type".to_string(),
        value: "text/plain".to_string(),
      }],
      body: body.to_vec(),
    }
  }

  fn streaming(chunks: Vec<Vec<u8>>) -> Script {
    let content_length = chunks.iter().map(|c| c.len() as u64).sum();
    Script::Stream {
      status: 200,
      headers: Vec::new(),
      content_length: Some(content_length),
      chunks,
      failure: None,
    }
  }

  impl FakeDevice {
    async fn next_stream_begin(&mut self, id: Uuid) -> StreamBegin {
      self
        .next_matching(|msg| match &msg.data {
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::StreamBegin(begin)) if begin.stream_id == id => {
            Some(begin.clone())
          }
          _ => None,
        })
        .await
    }

    async fn next_stream_chunk(&mut self, id: Uuid) -> StreamChunk {
      self
        .next_matching(|msg| match &msg.data {
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::StreamChunk(chunk)) if chunk.stream_id == id => {
            Some(chunk.clone())
          }
          _ => None,
        })
        .await
    }

    async fn next_stream_terminal(&mut self, id: Uuid) -> Result<(), NetError> {
      self
        .next_matching(|msg| match &msg.data {
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::StreamEnd(end)) if end.stream_id == id => Some(Ok(())),
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::StreamError(error)) if error.stream_id == id => {
            Some(Err(error.error.clone()))
          }
          _ => None,
        })
        .await
    }

    async fn no_stream_traffic(&mut self, id: Uuid, window: Duration) -> bool {
      self
        .nothing_matching(window, |msg| match &msg.data {
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::StreamBegin(begin)) if begin.stream_id == id => Some(()),
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::StreamChunk(chunk)) if chunk.stream_id == id => Some(()),
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::StreamEnd(end)) if end.stream_id == id => Some(()),
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::StreamError(error)) if error.stream_id == id => Some(()),
          _ => None,
        })
        .await
    }

    async fn next_ws_message(&mut self, id: Uuid) -> NetWsMessage {
      self
        .next_matching(|msg| match &msg.data {
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::WsMessage(message)) if message.connection_id == id => {
            Some(message.clone())
          }
          _ => None,
        })
        .await
    }

    async fn next_ws_closed(&mut self, id: Uuid) -> NetWsClosed {
      self
        .next_matching(|msg| match &msg.data {
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::WsClosed(closed)) if closed.connection_id == id => {
            Some(closed.clone())
          }
          _ => None,
        })
        .await
    }

    async fn no_ws_traffic(&mut self, id: Uuid, window: Duration) -> bool {
      self
        .nothing_matching(window, |msg| match &msg.data {
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::WsMessage(message)) if message.connection_id == id => {
            Some(())
          }
          GatewayToBridgeMsgData::Net(GatewayToBridgeNetMsg::WsClosed(closed)) if closed.connection_id == id => {
            Some(())
          }
          _ => None,
        })
        .await
    }
  }

  #[tokio::test]
  async fn a_fetch_answers_with_the_status_headers_and_body() {
    let rig = rig(vec![Script::Reply(ok_response(b"hello"))], WsMode::Manual);

    let reply = rig
      .dispatcher
      .fetch(NetFetchRequestMsg {
        request: get("https://example.test/thing"),
      })
      .await
      .expect("a scripted 200");

    assert_eq!(reply.response.response.status, 200);
    assert_eq!(reply.response.response.body, b"hello");
    assert_eq!(
      reply.response.response.headers,
      vec![HttpHeader {
        name: "content-type".to_string(),
        value: "text/plain".to_string(),
      }]
    );
  }

  #[tokio::test]
  async fn a_fetch_carries_every_method_the_wire_can_name() {
    let methods = [
      HttpMethod::Get,
      HttpMethod::Head,
      HttpMethod::Post,
      HttpMethod::Put,
      HttpMethod::Patch,
      HttpMethod::Delete,
      HttpMethod::Options,
    ];
    let rig = rig(
      methods.iter().map(|_| Script::Reply(ok_response(b""))).collect(),
      WsMode::Manual,
    );

    for method in methods {
      let mut request = get("https://example.test/verb");
      request.method = method;
      rig
        .dispatcher
        .fetch(NetFetchRequestMsg { request })
        .await
        .expect("a scripted 200");
    }

    let seen: Vec<io::HttpMethod> = rig.http.seen.lock().unwrap().iter().map(|r| r.method.clone()).collect();
    assert_eq!(
      seen,
      vec![
        io::HttpMethod::Get,
        io::HttpMethod::Head,
        io::HttpMethod::Post,
        io::HttpMethod::Put,
        io::HttpMethod::Patch,
        io::HttpMethod::Delete,
        io::HttpMethod::Options,
      ],
      "a method the wire can name must survive the seam rather than being coerced"
    );
  }

  #[tokio::test]
  async fn a_fetch_carries_its_headers_and_body_to_the_transport() {
    let rig = rig(vec![Script::Reply(ok_response(b""))], WsMode::Manual);
    let mut request = get("https://example.test/post");
    request.method = HttpMethod::Post;
    request.headers = vec![HttpHeader {
      name: "content-type".to_string(),
      value: "application/json".to_string(),
    }];
    request.body = Some(b"{\"a\":1}".to_vec());

    rig
      .dispatcher
      .fetch(NetFetchRequestMsg { request })
      .await
      .expect("a scripted 200");

    let seen = rig.http.seen.lock().unwrap();
    let sent = seen.first().expect("one request");
    assert_eq!(sent.url, "https://example.test/post");
    assert_eq!(sent.body, b"{\"a\":1}");
    assert_eq!(sent.headers.len(), 1);
    assert_eq!(sent.headers[0].name, "content-type");
  }

  #[tokio::test]
  async fn a_transport_failure_answers_with_a_reason_the_webapp_can_read() {
    let rig = rig(vec![Script::Fail("no route to host".to_string())], WsMode::Manual);

    let err = rig
      .dispatcher
      .fetch(NetFetchRequestMsg {
        request: get("https://example.test/thing"),
      })
      .await
      .expect_err("a failed transport");

    assert!(
      matches!(
        err,
        HandlerError::Domain(NetFetchErrorReply {
          error: NetError::RequestFailed { ref reason }
        }) if reason == "no route to host"
      ),
      "got {err:?}"
    );
  }

  #[tokio::test]
  async fn a_fetch_past_its_own_timeout_answers_timeout_rather_than_hanging() {
    let rig = rig(vec![Script::Holds], WsMode::Manual);
    let mut request = get("https://example.test/slow");
    request.timeout_ms = Some(100);

    let err = rig
      .dispatcher
      .fetch(NetFetchRequestMsg { request })
      .await
      .expect_err("a transport that never answers");

    assert!(
      matches!(
        err,
        HandlerError::Domain(NetFetchErrorReply {
          error: NetError::Timeout
        })
      ),
      "a bounded request must be classified as a timeout, not a generic failure; got {err:?}"
    );
  }

  #[tokio::test]
  async fn a_transport_that_drops_the_sink_answers_rather_than_hanging() {
    let rig = rig(vec![Script::Silent], WsMode::Manual);

    let err = rig
      .dispatcher
      .fetch(NetFetchRequestMsg {
        request: get("https://example.test/dropped"),
      })
      .await
      .expect_err("a dropped sink");

    assert!(
      matches!(
        err,
        HandlerError::Domain(NetFetchErrorReply {
          error: NetError::Unavailable
        })
      ),
      "got {err:?}"
    );
  }

  #[tokio::test]
  async fn a_stream_begins_with_the_head_then_chunks_then_ends() {
    let body = pattern(20 * 1024);
    let mut rig = rig(vec![streaming(vec![body.clone()])], WsMode::Manual);
    let id = Uuid::now_v7();

    rig
      .dispatcher
      .stream_open(NetStreamOpen {
        stream_id: id,
        request: get("https://example.test/video"),
      })
      .await
      .expect("opening a stream is infallible");

    let begin = rig.device.next_stream_begin(id).await;
    assert_eq!(begin.status, 200);
    assert_eq!(begin.total_size, Some(body.len() as u32));

    let mut assembled: Vec<u8> = Vec::new();
    while assembled.len() < body.len() {
      let chunk = rig.device.next_stream_chunk(id).await;
      assert_eq!(chunk.offset as usize, assembled.len(), "chunks address contiguously");
      assert!(
        chunk.bytes.len() <= STREAM_CHUNK_BYTES,
        "a chunk must stay inside the wire size, got {}",
        chunk.bytes.len()
      );
      assembled.extend_from_slice(&chunk.bytes);
    }
    assert_eq!(assembled, body, "the streamed bytes are the body");
    rig.device.next_stream_terminal(id).await.expect("a clean stream ends");
    assert!(
      rig.dispatcher.streams.lock().unwrap().is_empty(),
      "a finished stream must drop its own entry rather than accumulate one per request"
    );
  }

  #[tokio::test]
  async fn a_body_that_arrives_in_odd_pieces_is_re_chunked_to_the_wire_size() {
    let pieces: Vec<Vec<u8>> = vec![pattern(100), pattern(9_000), pattern(3), pattern(7_000)];
    let total: usize = pieces.iter().map(Vec::len).sum();
    let mut rig = rig(vec![streaming(pieces)], WsMode::Manual);
    let id = Uuid::now_v7();

    rig
      .dispatcher
      .stream_open(NetStreamOpen {
        stream_id: id,
        request: get("https://example.test/odd"),
      })
      .await
      .expect("opening a stream is infallible");
    rig.device.next_stream_begin(id).await;

    let mut sizes: Vec<usize> = Vec::new();
    let mut seen = 0usize;
    while seen < total {
      let chunk = rig.device.next_stream_chunk(id).await;
      assert_eq!(chunk.offset as usize, seen);
      seen += chunk.bytes.len();
      sizes.push(chunk.bytes.len());
    }

    assert_eq!(seen, total);
    let (last, full) = sizes.split_last().expect("at least one chunk");
    assert!(
      full.iter().all(|size| *size == STREAM_CHUNK_BYTES),
      "every chunk but the tail is a full wire chunk, got {sizes:?}"
    );
    assert!(*last <= STREAM_CHUNK_BYTES);
  }

  #[tokio::test]
  async fn a_stream_delivers_the_body_of_an_error_response() {
    let mut rig = rig(
      vec![Script::Stream {
        status: 404,
        headers: Vec::new(),
        content_length: Some(9),
        chunks: vec![b"not found".to_vec()],
        failure: None,
      }],
      WsMode::Manual,
    );
    let id = Uuid::now_v7();

    rig
      .dispatcher
      .stream_open(NetStreamOpen {
        stream_id: id,
        request: get("https://example.test/missing"),
      })
      .await
      .expect("opening a stream is infallible");

    let begin = rig.device.next_stream_begin(id).await;
    assert_eq!(begin.status, 404, "the status line reaches the webapp unchanged");
    let chunk = rig.device.next_stream_chunk(id).await;
    assert_eq!(
      chunk.bytes, b"not found",
      "a proxied error body belongs to the webapp that asked for it"
    );
    rig
      .device
      .next_stream_terminal(id)
      .await
      .expect("the stream still ends");
  }

  #[tokio::test]
  async fn a_stream_that_fails_mid_body_ends_with_the_reason() {
    let mut rig = rig(
      vec![Script::Stream {
        status: 200,
        headers: Vec::new(),
        content_length: None,
        chunks: vec![pattern(9_000)],
        failure: Some("connection reset by peer".to_string()),
      }],
      WsMode::Manual,
    );
    let id = Uuid::now_v7();

    rig
      .dispatcher
      .stream_open(NetStreamOpen {
        stream_id: id,
        request: get("https://example.test/cut"),
      })
      .await
      .expect("opening a stream is infallible");
    rig.device.next_stream_begin(id).await;

    let error = rig
      .device
      .next_stream_terminal(id)
      .await
      .expect_err("a cut stream fails");
    assert!(
      matches!(error, NetError::RequestFailed { ref reason } if reason == "connection reset by peer"),
      "got {error:?}"
    );
  }

  #[tokio::test]
  async fn a_stream_whose_link_falls_behind_fails_rather_than_buffering_without_bound() {
    let mut rig = rig_with(
      vec![Script::Stream {
        status: 200,
        headers: Vec::new(),
        content_length: None,
        chunks: vec![pattern(64 * 1024)],
        failure: None,
      }],
      WsMode::Manual,
      NetConfig {
        stream_buffer_budget_bytes: 4 * 1024,
        ..NetConfig::default()
      },
    );
    let id = Uuid::now_v7();

    rig
      .dispatcher
      .stream_open(NetStreamOpen {
        stream_id: id,
        request: get("https://example.test/firehose"),
      })
      .await
      .expect("opening a stream is infallible");

    let error = rig
      .device
      .next_stream_terminal(id)
      .await
      .expect_err("a stream past its budget fails");
    assert!(matches!(error, NetError::RequestFailed { .. }), "got {error:?}");
  }

  #[tokio::test]
  async fn a_body_whose_tail_does_not_fit_the_budget_fails_rather_than_ending_short() {
    let body = pattern(4 * 1024 + 100);
    let mut rig = rig_with(
      vec![streaming(vec![body.clone()])],
      WsMode::Manual,
      NetConfig {
        stream_chunk_bytes: 1_024,
        stream_buffer_budget_bytes: 4 * 1024,
        ..NetConfig::default()
      },
    );
    let id = Uuid::now_v7();

    rig
      .dispatcher
      .stream_open(NetStreamOpen {
        stream_id: id,
        request: get("https://example.test/tail"),
      })
      .await
      .expect("opening a stream is infallible");

    let begin = rig.device.next_stream_begin(id).await;
    assert_eq!(begin.total_size, Some(body.len() as u32));
    let error = rig
      .device
      .next_stream_terminal(id)
      .await
      .expect_err("a body missing its tail is not a clean end");
    assert!(
      matches!(error, NetError::RequestFailed { ref reason } if reason == BEHIND_REASON),
      "the webapp was promised total_size bytes; a short body has to arrive as an error, got {error:?}"
    );
  }

  #[tokio::test]
  async fn a_cancelled_stream_stops_saying_anything() {
    let mut rig = rig(vec![Script::Holds], WsMode::Manual);
    let id = Uuid::now_v7();

    rig
      .dispatcher
      .stream_open(NetStreamOpen {
        stream_id: id,
        request: get("https://example.test/long"),
      })
      .await
      .expect("opening a stream is infallible");
    rig
      .dispatcher
      .stream_cancel(NetStreamCancel { stream_id: id })
      .await
      .expect("cancelling is infallible");

    assert!(
      rig.device.no_stream_traffic(id, Duration::from_millis(300)).await,
      "the device already dropped its route, so a cancelled stream is silent"
    );
    assert!(rig.dispatcher.streams.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn every_stream_event_rides_the_bulk_lane() {
    let mut rig = rig(vec![streaming(vec![pattern(20 * 1024)])], WsMode::Manual);
    let id = Uuid::now_v7();

    rig
      .dispatcher
      .stream_open(NetStreamOpen {
        stream_id: id,
        request: get("https://example.test/video"),
      })
      .await
      .expect("opening a stream is infallible");
    rig.device.next_stream_terminal(id).await.expect("a clean stream ends");

    let lanes = rig.device.lanes_of(|data| match data {
      GatewayToBridgeMsgData::Net(
        GatewayToBridgeNetMsg::StreamBegin(_)
        | GatewayToBridgeNetMsg::StreamChunk(_)
        | GatewayToBridgeNetMsg::StreamEnd(_)
        | GatewayToBridgeNetMsg::StreamError(_),
      ) => Some(()),
      _ => None,
    });

    assert!(!lanes.is_empty());
    assert!(
      lanes.iter().all(|lane| *lane == Priority::Bulk),
      "a terminal on another lane could overtake the chunks it ends, got {lanes:?}"
    );
  }

  #[tokio::test]
  async fn a_ws_open_answers_once_the_handshake_lands() {
    let rig = rig(
      Vec::new(),
      WsMode::Opens {
        accepted_protocol: Some("chat".to_string()),
      },
    );
    let id = Uuid::now_v7();

    let reply = rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: Some(vec!["chat".to_string()]),
        headers: None,
      })
      .await
      .expect("a socket that opens");

    assert_eq!(
      reply.response.accepted_protocol.as_deref(),
      Some("chat"),
      "the subprotocol the server picked is the webapp's answer, not a guess"
    );
    let connects = rig.ws.connects.lock().unwrap();
    assert_eq!(connects[0].id, id);
    assert_eq!(connects[0].protocols, vec!["chat".to_string()]);
  }

  #[tokio::test]
  async fn a_ws_open_waits_for_the_handshake_before_answering() {
    let rig = rig(Vec::new(), WsMode::Manual);
    let id = Uuid::now_v7();
    let dispatcher = rig.dispatcher.clone();

    let opening = tokio::spawn(async move {
      dispatcher
        .ws_open(NetWsOpen {
          connection_id: id,
          url: "wss://example.test/socket".to_string(),
          protocols: None,
          headers: None,
        })
        .await
    });
    tokio::task::yield_now().await;
    assert!(!opening.is_finished(), "a dialled socket is not yet an open one");

    rig.ws.inbox().on_open(id, None);
    opening.await.expect("the open task").expect("the handshake landed");
  }

  #[tokio::test]
  async fn a_ws_open_that_is_refused_answers_connect_failed() {
    let rig = rig(
      Vec::new(),
      WsMode::Refuses {
        reason: "connection refused".to_string(),
      },
    );
    let id = Uuid::now_v7();

    let err = rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: None,
        headers: None,
      })
      .await
      .expect_err("a refused socket");

    assert!(
      matches!(
        err,
        HandlerError::Domain(NetWsErrorReply {
          error: WsError::ConnectFailed { ref reason }
        }) if reason == "connection refused"
      ),
      "a socket that never opened must not be reported as open; got {err:?}"
    );
  }

  #[tokio::test]
  async fn a_refused_open_reports_no_close_event_for_a_connection_that_never_existed() {
    let mut rig = rig(
      Vec::new(),
      WsMode::Refuses {
        reason: "connection refused".to_string(),
      },
    );
    let id = Uuid::now_v7();

    let _ = rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: None,
        headers: None,
      })
      .await;

    assert!(
      rig.device.no_ws_traffic(id, Duration::from_millis(200)).await,
      "the failure was the open's answer, so there is nothing further to report"
    );
  }

  #[tokio::test]
  async fn a_ws_open_that_never_completes_gives_up() {
    let rig = rig_with(
      Vec::new(),
      WsMode::Manual,
      NetConfig {
        ws_open_timeout: Duration::from_millis(120),
        ..NetConfig::default()
      },
    );
    let id = Uuid::now_v7();

    let err = rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: None,
        headers: None,
      })
      .await
      .expect_err("a handshake that never lands");

    assert!(matches!(err, HandlerError::Domain(_)), "got {err:?}");
    assert!(
      rig.dispatcher.sockets.lock().unwrap().pending.is_empty(),
      "a timed-out open leaves nothing behind to answer later"
    );
  }

  #[tokio::test]
  async fn frames_from_the_socket_reach_the_device_with_their_connection_id() {
    let mut rig = rig(
      Vec::new(),
      WsMode::Opens {
        accepted_protocol: None,
      },
    );
    let id = Uuid::now_v7();
    rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: None,
        headers: None,
      })
      .await
      .expect("a socket that opens");

    rig.ws.inbox().on_text(id, "hello".to_string());
    rig.ws.inbox().on_binary(id, vec![0xde, 0xad]);

    let text = rig.device.next_ws_message(id).await;
    assert!(matches!(text.frame, WsFrame::Text(ref t) if t == "hello"));
    let binary = rig.device.next_ws_message(id).await;
    assert!(matches!(binary.frame, WsFrame::Binary(ref b) if b == &[0xde, 0xad]));
  }

  #[tokio::test]
  async fn two_sockets_never_cross_their_frames() {
    let mut rig = rig(
      Vec::new(),
      WsMode::Opens {
        accepted_protocol: None,
      },
    );
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    for id in [first, second] {
      rig
        .dispatcher
        .ws_open(NetWsOpen {
          connection_id: id,
          url: "wss://example.test/socket".to_string(),
          protocols: None,
          headers: None,
        })
        .await
        .expect("a socket that opens");
    }

    rig.ws.inbox().on_text(first, "one".to_string());
    rig.ws.inbox().on_text(second, "two".to_string());

    assert!(matches!(rig.device.next_ws_message(first).await.frame, WsFrame::Text(ref t) if t == "one"));
    assert!(matches!(rig.device.next_ws_message(second).await.frame, WsFrame::Text(ref t) if t == "two"));
  }

  #[tokio::test]
  async fn a_send_reaches_the_socket_it_names() {
    let rig = rig(
      Vec::new(),
      WsMode::Opens {
        accepted_protocol: None,
      },
    );
    let id = Uuid::now_v7();
    rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: None,
        headers: None,
      })
      .await
      .expect("a socket that opens");

    rig
      .dispatcher
      .ws_send(NetWsSend {
        connection_id: id,
        frame: WsFrame::Binary(vec![1, 2, 3]),
      })
      .await
      .expect("sending is infallible");

    let sent = rig.ws.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, id);
    assert!(matches!(sent[0].1, io::WsFrame::Binary(ref b) if b == &[1, 2, 3]));
  }

  #[tokio::test]
  async fn a_send_on_an_unknown_connection_is_dropped() {
    let rig = rig(Vec::new(), WsMode::Manual);

    rig
      .dispatcher
      .ws_send(NetWsSend {
        connection_id: Uuid::now_v7(),
        frame: WsFrame::Text("nowhere".to_string()),
      })
      .await
      .expect("an unknown connection is not a protocol error");

    assert!(rig.ws.sent.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn a_close_from_the_device_carries_its_code_to_the_socket() {
    let rig = rig(
      Vec::new(),
      WsMode::Opens {
        accepted_protocol: None,
      },
    );
    let id = Uuid::now_v7();
    rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: None,
        headers: None,
      })
      .await
      .expect("a socket that opens");

    rig
      .dispatcher
      .ws_close(NetWsClose {
        connection_id: id,
        code: Some(1001),
        reason: Some("going away".to_string()),
      })
      .await
      .expect("closing is infallible");

    let disconnects = rig.ws.disconnects.lock().unwrap();
    assert_eq!(disconnects.len(), 1);
    assert_eq!(disconnects[0].0, id);
    assert_eq!(disconnects[0].1, Some(1001));
    assert_eq!(disconnects[0].2.as_deref(), Some("going away"));
  }

  #[tokio::test]
  async fn a_socket_that_dies_reports_closed_with_the_abnormal_code() {
    let mut rig = rig(
      Vec::new(),
      WsMode::Opens {
        accepted_protocol: None,
      },
    );
    let id = Uuid::now_v7();
    rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: None,
        headers: None,
      })
      .await
      .expect("a socket that opens");

    rig.ws.inbox().on_closed(id, None, "read error".to_string());

    let closed = rig.device.next_ws_closed(id).await;
    assert_eq!(
      closed.code, WS_ABNORMAL_CLOSURE,
      "a socket that died rather than shut down carries the close the peer never sent"
    );
    assert_eq!(closed.reason, "read error");
  }

  #[tokio::test]
  async fn a_closed_socket_stops_taking_sends() {
    let rig = rig(
      Vec::new(),
      WsMode::Opens {
        accepted_protocol: None,
      },
    );
    let id = Uuid::now_v7();
    rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: None,
        headers: None,
      })
      .await
      .expect("a socket that opens");
    rig.ws.inbox().on_closed(id, Some(1000), "bye".to_string());
    tokio::time::sleep(Duration::from_millis(50)).await;

    rig
      .dispatcher
      .ws_send(NetWsSend {
        connection_id: id,
        frame: WsFrame::Text("too late".to_string()),
      })
      .await
      .expect("a send to a dead socket is not an error");

    assert!(rig.ws.sent.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn stopping_the_dispatcher_drops_every_socket_and_stream() {
    let rig = rig(
      vec![Script::Holds],
      WsMode::Opens {
        accepted_protocol: None,
      },
    );
    let id = Uuid::now_v7();
    rig
      .dispatcher
      .ws_open(NetWsOpen {
        connection_id: id,
        url: "wss://example.test/socket".to_string(),
        protocols: None,
        headers: None,
      })
      .await
      .expect("a socket that opens");
    rig
      .dispatcher
      .stream_open(NetStreamOpen {
        stream_id: Uuid::now_v7(),
        request: get("https://example.test/long"),
      })
      .await
      .expect("opening a stream is infallible");

    rig.dispatcher.stop();

    assert!(rig.dispatcher.sockets.lock().unwrap().live.is_empty());
    assert!(rig.dispatcher.streams.lock().unwrap().is_empty());
    assert_eq!(rig.ws.disconnects.lock().unwrap().len(), 1);
  }
}
