use std::{
  collections::HashMap,
  sync::{Arc, Mutex, Once},
  time::Duration,
};

use futures::{SinkExt, StreamExt};
use reqwest::header::HeaderMap;
use tokio::{
  sync::{mpsc, oneshot},
  task::JoinHandle,
};
use tokio_tungstenite::{
  connect_async,
  tungstenite::{
    Message as WsMessage,
    client::IntoClientRequest,
    handshake::client::Request,
    http::{HeaderName, HeaderValue},
    protocol::{CloseFrame, frame::coding::CloseCode},
  },
};
use uuid::Uuid;

use crate::{
  http::{HttpDownloadSink, HttpHeader, HttpMethod, HttpRequest, HttpResponse, HttpSink, HttpTransport},
  ws::{WsConnect, WsFrame, WsInbox, WsTransport},
};

static CRYPTO_PROVIDER: Once = Once::new();

pub(crate) fn install_ring() {
  CRYPTO_PROVIDER.call_once(|| {
    if rustls::crypto::ring::default_provider().install_default().is_err() {
      tracing::warn!("a rustls crypto provider was already installed; leaving it in place");
    }
  });
}

pub struct ReqwestConfig {
  pub user_agent: String,
  pub request_timeout: Duration,
  pub connect_timeout: Duration,
}

impl Default for ReqwestConfig {
  fn default() -> Self {
    ReqwestConfig {
      user_agent: concat!("bridgething/", env!("CARGO_PKG_VERSION")).to_string(),
      request_timeout: Duration::from_secs(15),
      connect_timeout: Duration::from_secs(8),
    }
  }
}

pub struct ReqwestTransport {
  client: reqwest::Client,
}

impl ReqwestTransport {
  pub fn new(config: ReqwestConfig) -> Self {
    install_ring();
    let client = reqwest::Client::builder()
      .user_agent(config.user_agent)
      .timeout(config.request_timeout)
      .connect_timeout(config.connect_timeout)
      .build()
      .expect("reqwest client builds");
    ReqwestTransport { client }
  }
}

impl Default for ReqwestTransport {
  fn default() -> Self {
    Self::new(ReqwestConfig::default())
  }
}

impl HttpTransport for ReqwestTransport {
  fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>) {
    let client = self.client.clone();
    tokio::spawn(async move {
      match reqwest_execute(&client, request).await {
        Ok(resp) => sink.complete(resp),
        Err(e) => sink.fail(e.to_string()),
      }
    });
  }

  fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    let client = self.client.clone();
    tokio::spawn(async move {
      if let Err(reason) = reqwest_download(&client, request, sink.clone()).await {
        sink.on_failed(reason);
      }
    });
  }
}

fn reqwest_builder(client: &reqwest::Client, request: &HttpRequest) -> reqwest::RequestBuilder {
  let method = match request.method {
    HttpMethod::Get => reqwest::Method::GET,
    HttpMethod::Head => reqwest::Method::HEAD,
    HttpMethod::Post => reqwest::Method::POST,
    HttpMethod::Put => reqwest::Method::PUT,
    HttpMethod::Patch => reqwest::Method::PATCH,
    HttpMethod::Delete => reqwest::Method::DELETE,
    HttpMethod::Options => reqwest::Method::OPTIONS,
  };
  let mut rb = client.request(method, request.url.as_str());
  for h in &request.headers {
    rb = rb.header(h.name.as_str(), h.value.as_str());
  }
  if request.timeout_ms > 0 {
    rb = rb.timeout(Duration::from_millis(request.timeout_ms as u64));
  }
  rb
}

async fn reqwest_execute(client: &reqwest::Client, request: HttpRequest) -> Result<HttpResponse, reqwest::Error> {
  let mut rb = reqwest_builder(client, &request);
  if !request.body.is_empty() {
    rb = rb.body(request.body);
  }
  let resp = rb.send().await?;
  let status = resp.status().as_u16();
  let headers = header_vec(resp.headers());
  let body = resp.bytes().await?.to_vec();
  Ok(HttpResponse { status, headers, body })
}

async fn reqwest_download(
  client: &reqwest::Client,
  request: HttpRequest,
  sink: Arc<HttpDownloadSink>,
) -> Result<(), String> {
  let mut rb = reqwest_builder(client, &request);
  if !request.body.is_empty() {
    rb = rb.body(request.body);
  }
  let mut resp = rb.send().await.map_err(|e| e.to_string())?;
  sink.on_response(
    resp.status().as_u16(),
    header_vec(resp.headers()),
    resp.content_length(),
  );
  while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
    let writing = sink.clone();
    tokio::task::spawn_blocking(move || writing.on_chunk(chunk.to_vec()))
      .await
      .map_err(|e| e.to_string())?;
  }
  sink.on_finished();
  Ok(())
}

fn header_vec(map: &HeaderMap) -> Vec<HttpHeader> {
  map
    .iter()
    .map(|(k, v)| HttpHeader {
      name: k.as_str().to_string(),
      value: v.to_str().unwrap_or_default().to_string(),
    })
    .collect()
}

const WS_ABNORMAL_CLOSURE: u16 = 1006;
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

type Conns = Arc<Mutex<HashMap<Uuid, Conn>>>;

pub struct TungsteniteTransport {
  conns: Conns,
  connect_timeout: Duration,
}

impl Default for TungsteniteTransport {
  fn default() -> Self {
    TungsteniteTransport {
      conns: Conns::default(),
      connect_timeout: WS_CONNECT_TIMEOUT,
    }
  }
}

struct Conn {
  out: mpsc::UnboundedSender<Outgoing>,
  task: JoinHandle<()>,
}

enum Outgoing {
  Frame(WsFrame),
  Close { code: Option<u16>, reason: Option<String> },
}

impl TungsteniteTransport {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn with_connect_timeout(connect_timeout: Duration) -> Self {
    TungsteniteTransport {
      conns: Conns::default(),
      connect_timeout,
    }
  }
}

impl WsTransport for TungsteniteTransport {
  fn connect(&self, connect: WsConnect, inbox: Arc<WsInbox>) {
    install_ring();
    tracing::debug!(id = %connect.id, "ws: connecting (native transport)");
    let id = connect.id;
    let (out, out_rx) = mpsc::unbounded_channel::<Outgoing>();
    let (inserted_tx, inserted_rx) = oneshot::channel();
    let task = tokio::spawn(run(
      connect,
      inbox,
      out_rx,
      self.conns.clone(),
      inserted_rx,
      self.connect_timeout,
    ));
    if let Some(prev) = self.conns.lock().unwrap().insert(id, Conn { out, task }) {
      prev.task.abort();
    }
    let _ = inserted_tx.send(());
  }

  fn send(&self, id: Uuid, frame: WsFrame) {
    tracing::trace!(%id, ?frame, "ws: send");
    if let Some(conn) = self.conns.lock().unwrap().get(&id) {
      let _ = conn.out.send(Outgoing::Frame(frame));
    }
  }

  fn disconnect(&self, id: Uuid, code: Option<u16>, reason: Option<String>) {
    tracing::debug!(%id, "ws: disconnect (native transport)");
    if let Some(conn) = self.conns.lock().unwrap().get(&id) {
      let _ = conn.out.send(Outgoing::Close { code, reason });
    }
  }
}

impl Drop for TungsteniteTransport {
  fn drop(&mut self) {
    for (_, conn) in self.conns.lock().unwrap().drain() {
      conn.task.abort();
    }
  }
}

fn client_request(connect: &WsConnect) -> Result<Request, String> {
  let mut request = connect
    .url
    .as_str()
    .into_client_request()
    .map_err(|e| format!("invalid url: {e}"))?;
  let headers = request.headers_mut();
  for header in &connect.headers {
    let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(|e| format!("invalid header name: {e}"))?;
    let value = HeaderValue::from_str(&header.value).map_err(|e| format!("invalid header value: {e}"))?;
    headers.append(name, value);
  }
  if !connect.protocols.is_empty() {
    let value =
      HeaderValue::from_str(&connect.protocols.join(", ")).map_err(|e| format!("invalid subprotocol: {e}"))?;
    headers.insert("sec-websocket-protocol", value);
  }
  Ok(request)
}

async fn run(
  connect: WsConnect,
  inbox: Arc<WsInbox>,
  out_rx: mpsc::UnboundedReceiver<Outgoing>,
  conns: Conns,
  inserted: oneshot::Receiver<()>,
  connect_timeout: Duration,
) {
  let id = connect.id;
  let _ = inserted.await;
  pump(connect, inbox, out_rx, connect_timeout).await;
  conns.lock().unwrap().remove(&id);
}

async fn pump(
  connect: WsConnect,
  inbox: Arc<WsInbox>,
  mut out_rx: mpsc::UnboundedReceiver<Outgoing>,
  connect_timeout: Duration,
) {
  let id = connect.id;
  let request = match client_request(&connect) {
    Ok(request) => request,
    Err(reason) => {
      inbox.on_closed(id, None, reason);
      return;
    }
  };
  let (ws, response) = match tokio::time::timeout(connect_timeout, connect_async(request)).await {
    Ok(Ok(opened)) => opened,
    Ok(Err(e)) => {
      inbox.on_closed(id, None, format!("connect failed: {e}"));
      return;
    }
    Err(_) => {
      inbox.on_closed(
        id,
        None,
        format!("connect timed out after {}ms", connect_timeout.as_millis()),
      );
      return;
    }
  };
  let accepted_protocol = response
    .headers()
    .get("sec-websocket-protocol")
    .and_then(|value| value.to_str().ok())
    .map(str::to_string);
  inbox.on_open(id, accepted_protocol);

  let (mut sink, mut stream) = ws.split();
  loop {
    tokio::select! {
      msg = stream.next() => match msg {
        Some(Ok(WsMessage::Text(t))) => inbox.on_text(id, t.to_string()),
        Some(Ok(WsMessage::Binary(b))) => inbox.on_binary(id, b.to_vec()),
        Some(Ok(WsMessage::Ping(p))) => {
          if sink.send(WsMessage::Pong(p)).await.is_err() {
            inbox.on_closed(id, Some(WS_ABNORMAL_CLOSURE), "write error".to_string());
            return;
          }
        }
        Some(Ok(WsMessage::Close(frame))) => {
          let (code, reason) = match frame {
            Some(frame) => (Some(u16::from(frame.code)), frame.reason.to_string()),
            None => (None, "closed".to_string()),
          };
          inbox.on_closed(id, code, reason);
          return;
        }
        None => {
          inbox.on_closed(id, Some(WS_ABNORMAL_CLOSURE), "closed".to_string());
          return;
        }
        Some(Ok(_)) => {}
        Some(Err(e)) => {
          inbox.on_closed(id, Some(WS_ABNORMAL_CLOSURE), format!("read error: {e}"));
          return;
        }
      },
      out = out_rx.recv() => match out {
        Some(Outgoing::Frame(frame)) => {
          let message = match frame {
            WsFrame::Text(text) => WsMessage::Text(text.into()),
            WsFrame::Binary(bytes) => WsMessage::Binary(bytes.into()),
          };
          if sink.send(message).await.is_err() {
            inbox.on_closed(id, Some(WS_ABNORMAL_CLOSURE), "write error".to_string());
            return;
          }
        }
        Some(Outgoing::Close { code, reason }) => {
          let frame = CloseFrame {
            code: CloseCode::from(code.unwrap_or(1000)),
            reason: reason.unwrap_or_default().into(),
          };
          let _ = sink.send(WsMessage::Close(Some(frame.clone()))).await;
          inbox.on_closed(id, Some(u16::from(frame.code)), frame.reason.to_string());
          return;
        }
        None => return,
      }
    }
  }
}
