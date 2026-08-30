use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::oneshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpMethod {
  Get,
  Head,
  Post,
  Put,
  Patch,
  Delete,
  Options,
  Other(String),
}

impl HttpMethod {
  pub fn validate(&self) -> Result<(), String> {
    let Self::Other(verb) = self else { return Ok(()) };
    if verb.is_empty() {
      return Err("http method is empty".to_string());
    }
    match verb.bytes().find(|byte| !is_token_byte(*byte)) {
      Some(byte) => Err(format!("http method {verb:?} is not a token: byte {byte:#04x}")),
      None => Ok(()),
    }
  }

  pub fn as_str(&self) -> &str {
    match self {
      Self::Get => "GET",
      Self::Head => "HEAD",
      Self::Post => "POST",
      Self::Put => "PUT",
      Self::Patch => "PATCH",
      Self::Delete => "DELETE",
      Self::Options => "OPTIONS",
      Self::Other(verb) => verb,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpHeader {
  pub name: String,
  pub value: String,
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
  pub method: HttpMethod,
  pub url: String,
  pub headers: Vec<HttpHeader>,
  pub body: Vec<u8>,
  pub timeout_ms: u32,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
  pub status: u16,
  pub headers: Vec<HttpHeader>,
  pub body: Vec<u8>,
}

impl HttpResponse {
  pub fn ok(&self) -> bool {
    (200..300).contains(&self.status)
  }

  pub fn text(&self) -> String {
    String::from_utf8_lossy(&self.body).into_owned()
  }
}

fn is_token_byte(byte: u8) -> bool {
  byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
  #[error("{0}")]
  InvalidRequest(String),
  #[error("{0}")]
  Transport(String),
  #[error("{0}")]
  Body(String),
  #[error("http transport dropped without responding")]
  Dropped,
}

#[derive(Debug, Clone)]
pub struct DownloadOutcome {
  pub status: u16,
  pub headers: Vec<HttpHeader>,
  pub content_length: Option<u64>,
  pub received: u64,
}

impl DownloadOutcome {
  pub fn ok(&self) -> bool {
    (200..300).contains(&self.status)
  }
}

pub trait DownloadBody: Send {
  fn on_response(&mut self, status: u16, headers: &[HttpHeader], content_length: Option<u64>) -> bool;
  fn write(&mut self, chunk: &[u8]) -> Result<(), String>;
}

struct DownloadState {
  body: Option<Box<dyn DownloadBody>>,
  status: u16,
  headers: Vec<HttpHeader>,
  content_length: Option<u64>,
  received: u64,
  failure: Option<HttpError>,
}

pub struct HttpDownloadSink {
  state: Mutex<DownloadState>,
  tx: Mutex<Option<oneshot::Sender<Result<DownloadOutcome, HttpError>>>>,
}

impl HttpDownloadSink {
  pub fn on_response(&self, status: u16, headers: Vec<HttpHeader>, content_length: Option<u64>) {
    let mut state = self.state.lock().unwrap();
    state.status = status;
    state.content_length = content_length;
    let refused = match state.body.as_mut() {
      Some(body) => !body.on_response(status, &headers, content_length),
      None => false,
    };
    state.headers = headers;
    if refused {
      state.body = None;
    }
  }

  pub fn on_chunk(&self, chunk: Vec<u8>) {
    let mut state = self.state.lock().unwrap();
    let Some(body) = state.body.as_mut() else {
      return;
    };
    match body.write(&chunk) {
      Ok(()) => state.received += chunk.len() as u64,
      Err(reason) => {
        state.failure = Some(HttpError::Body(reason));
        state.body = None;
      }
    }
  }

  pub fn on_finished(&self) {
    let result = {
      let mut state = self.state.lock().unwrap();
      state.body = None;
      match state.failure.take() {
        Some(failure) => Err(failure),
        None => Ok(DownloadOutcome {
          status: state.status,
          headers: std::mem::take(&mut state.headers),
          content_length: state.content_length,
          received: state.received,
        }),
      }
    };
    self.settle(result);
  }

  pub fn on_failed(&self, reason: String) {
    self.state.lock().unwrap().body = None;
    self.settle(Err(HttpError::Transport(reason)));
  }

  fn settle(&self, result: Result<DownloadOutcome, HttpError>) {
    if let Some(tx) = self.tx.lock().unwrap().take() {
      let _ = tx.send(result);
    }
  }
}

pub struct HttpSink {
  tx: Mutex<Option<oneshot::Sender<Result<HttpResponse, String>>>>,
}

impl HttpSink {
  pub fn complete(&self, response: HttpResponse) {
    if let Some(tx) = self.tx.lock().unwrap().take() {
      let _ = tx.send(Ok(response));
    }
  }

  pub fn fail(&self, reason: String) {
    if let Some(tx) = self.tx.lock().unwrap().take() {
      let _ = tx.send(Err(reason));
    }
  }
}

fn text_body(resp: &HttpResponse) -> Option<String> {
  let ct = resp
    .headers
    .iter()
    .find(|h| h.name.eq_ignore_ascii_case("content-type"))
    .map(|h| h.value.to_ascii_lowercase())
    .unwrap_or_default();
  let texty = ct.contains("json") || ct.contains("text") || ct.contains("xml") || ct.contains("urlencoded");
  texty.then(|| String::from_utf8_lossy(&resp.body).into_owned())
}

pub trait HttpTransport: Send + Sync {
  fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>);
  fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>);
}

#[derive(Clone)]
pub struct HttpExecutor {
  transport: Arc<RwLock<Arc<dyn HttpTransport>>>,
}

impl HttpExecutor {
  pub fn new(transport: Arc<dyn HttpTransport>) -> Self {
    HttpExecutor {
      transport: Arc::new(RwLock::new(transport)),
    }
  }

  pub fn set(&self, transport: Arc<dyn HttpTransport>) {
    *self.transport.write().unwrap() = transport;
  }

  pub async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
    request.method.validate().map_err(HttpError::InvalidRequest)?;
    let transport = self.transport.read().unwrap().clone();
    let (tx, rx) = oneshot::channel();
    let sink = Arc::new(HttpSink {
      tx: Mutex::new(Some(tx)),
    });
    let method = request.method.as_str().to_owned();
    let url = request.url.clone();
    tracing::trace!(%method, %url, bytes = request.body.len(), "http request");
    transport.execute(request, sink);
    match rx.await {
      Ok(Ok(resp)) => {
        match (resp.status >= 400, text_body(&resp)) {
          (true, _) => {
            let body = String::from_utf8_lossy(&resp.body);
            tracing::warn!(%method, %url, status = resp.status, bytes = resp.body.len(), %body, "http response");
          }
          (false, Some(body)) => {
            tracing::debug!(%method, %url, status = resp.status, bytes = resp.body.len(), %body, "http response");
          }
          (false, None) => {
            tracing::debug!(%method, %url, status = resp.status, bytes = resp.body.len(), "http response");
          }
        }
        Ok(resp)
      }
      Ok(Err(reason)) => {
        tracing::warn!(%method, %url, %reason, "http transport error");
        Err(HttpError::Transport(reason))
      }
      Err(_) => {
        tracing::warn!(%method, %url, "http transport dropped without responding");
        Err(HttpError::Dropped)
      }
    }
  }

  pub async fn download(
    &self,
    request: HttpRequest,
    body: Box<dyn DownloadBody>,
  ) -> Result<DownloadOutcome, HttpError> {
    request.method.validate().map_err(HttpError::InvalidRequest)?;
    let transport = self.transport.read().unwrap().clone();
    let (tx, rx) = oneshot::channel();
    let sink = Arc::new(HttpDownloadSink {
      state: Mutex::new(DownloadState {
        body: Some(body),
        status: 0,
        headers: Vec::new(),
        content_length: None,
        received: 0,
        failure: None,
      }),
      tx: Mutex::new(Some(tx)),
    });
    tracing::trace!(url = %request.url, "http download");
    transport.download(request, sink);
    match rx.await {
      Ok(Ok(outcome)) => {
        tracing::debug!(
          status = outcome.status,
          bytes = outcome.received,
          "http download complete"
        );
        Ok(outcome)
      }
      Ok(Err(e)) => {
        tracing::warn!(reason = %e, "http download failed");
        Err(e)
      }
      Err(_) => {
        tracing::warn!("http transport dropped without responding");
        Err(HttpError::Dropped)
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn request(url: &str) -> HttpRequest {
    HttpRequest {
      method: HttpMethod::Get,
      url: url.to_string(),
      headers: Vec::new(),
      body: Vec::new(),
      timeout_ms: 0,
    }
  }

  struct Recording {
    urls: Mutex<Vec<String>>,
  }

  impl HttpTransport for Recording {
    fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>) {
      self.urls.lock().unwrap().push(request.url);
      sink.complete(HttpResponse {
        status: 204,
        headers: vec![HttpHeader {
          name: "x-arm".to_string(),
          value: "recording".to_string(),
        }],
        body: b"body".to_vec(),
      });
    }

    fn download(&self, _request: HttpRequest, _sink: Arc<HttpDownloadSink>) {
      unreachable!("the whole-body suite never takes the streaming arm");
    }
  }

  struct Failing;

  impl HttpTransport for Failing {
    fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
      sink.fail("no route to host".to_string());
    }

    fn download(&self, _request: HttpRequest, _sink: Arc<HttpDownloadSink>) {
      unreachable!("the whole-body suite never takes the streaming arm");
    }
  }

  struct Silent;

  impl HttpTransport for Silent {
    fn execute(&self, _request: HttpRequest, _sink: Arc<HttpSink>) {}

    fn download(&self, _request: HttpRequest, _sink: Arc<HttpDownloadSink>) {}
  }

  #[tokio::test]
  async fn the_installed_transport_answers_the_request() {
    let transport = Arc::new(Recording {
      urls: Mutex::new(Vec::new()),
    });
    let exec = HttpExecutor::new(transport.clone());

    let resp = exec.execute(request("https://example.test/one")).await.unwrap();

    assert_eq!(resp.status, 204);
    assert_eq!(resp.text(), "body");
    assert!(resp.ok());
    assert_eq!(transport.urls.lock().unwrap().as_slice(), ["https://example.test/one"]);
  }

  #[tokio::test]
  async fn a_swapped_transport_takes_over_the_next_request() {
    let first = Arc::new(Recording {
      urls: Mutex::new(Vec::new()),
    });
    let exec = HttpExecutor::new(first.clone());
    exec.execute(request("https://example.test/first")).await.unwrap();

    exec.set(Arc::new(Failing));
    let err = exec.execute(request("https://example.test/second")).await.unwrap_err();

    assert_eq!(err.to_string(), "no route to host");
    assert_eq!(first.urls.lock().unwrap().len(), 1);
  }

  #[tokio::test]
  async fn a_transport_that_drops_the_sink_fails_the_request() {
    let exec = HttpExecutor::new(Arc::new(Silent));

    let err = exec.execute(request("https://example.test/dropped")).await.unwrap_err();

    assert_eq!(err.to_string(), "http transport dropped without responding");
  }
}
