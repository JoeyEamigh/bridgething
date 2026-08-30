use std::sync::Arc;

use bridgething_io as io;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum HttpMethod {
  Get,
  Head,
  Post,
  Put,
  Patch,
  Delete,
  Options,
  Other { verb: String },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HttpHeader {
  pub name: String,
  pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HttpRequest {
  pub method: HttpMethod,
  pub url: String,
  pub headers: Vec<HttpHeader>,
  pub body: Vec<u8>,
  pub timeout_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HttpResponse {
  pub status: u16,
  pub headers: Vec<HttpHeader>,
  pub body: Vec<u8>,
}

#[uniffi::export(with_foreign)]
pub trait HttpTransport: Send + Sync {
  fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>);
  fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>);
}

#[derive(uniffi::Object)]
pub struct HttpSink {
  inner: Arc<io::HttpSink>,
}

impl HttpSink {
  pub fn wrapping(inner: Arc<io::HttpSink>) -> Arc<Self> {
    Arc::new(Self { inner })
  }

  pub fn inner(&self) -> &Arc<io::HttpSink> {
    &self.inner
  }
}

#[uniffi::export]
impl HttpSink {
  pub fn complete(&self, response: HttpResponse) {
    self.inner.complete(response.into());
  }

  pub fn fail(&self, reason: String) {
    self.inner.fail(reason);
  }
}

#[derive(uniffi::Object)]
pub struct HttpDownloadSink {
  inner: Arc<io::HttpDownloadSink>,
}

impl HttpDownloadSink {
  pub fn wrapping(inner: Arc<io::HttpDownloadSink>) -> Arc<Self> {
    Arc::new(Self { inner })
  }

  pub fn inner(&self) -> &Arc<io::HttpDownloadSink> {
    &self.inner
  }
}

#[uniffi::export]
impl HttpDownloadSink {
  pub fn on_response(&self, status: u16, headers: Vec<HttpHeader>, content_length: Option<u64>) {
    self
      .inner
      .on_response(status, headers.into_iter().map(Into::into).collect(), content_length);
  }

  pub fn on_chunk(&self, chunk: Vec<u8>) {
    self.inner.on_chunk(chunk);
  }

  pub fn on_finished(&self) {
    self.inner.on_finished();
  }

  pub fn on_failed(&self, reason: String) {
    self.inner.on_failed(reason);
  }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WsConnect {
  pub id: String,
  pub url: String,
  pub protocols: Vec<String>,
  pub headers: Vec<HttpHeader>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum WsFrame {
  Text { text: String },
  Binary { bytes: Vec<u8> },
}

#[uniffi::export(with_foreign)]
pub trait WsTransport: Send + Sync {
  fn connect(&self, connect: WsConnect, inbox: Arc<WsInbox>);
  fn send(&self, id: String, frame: WsFrame);
  fn disconnect(&self, id: String, code: Option<u16>, reason: Option<String>);
}

#[derive(uniffi::Object)]
pub struct WsInbox {
  inner: Arc<io::WsInbox>,
}

impl WsInbox {
  pub fn wrapping(inner: Arc<io::WsInbox>) -> Arc<Self> {
    Arc::new(Self { inner })
  }

  pub fn inner(&self) -> &Arc<io::WsInbox> {
    &self.inner
  }
}

#[uniffi::export]
impl WsInbox {
  pub fn on_open(&self, id: String, accepted_protocol: Option<String>) {
    if let Some(id) = connection(&id) {
      self.inner.on_open(id, accepted_protocol);
    }
  }

  pub fn on_text(&self, id: String, text: String) {
    if let Some(id) = connection(&id) {
      self.inner.on_text(id, text);
    }
  }

  pub fn on_binary(&self, id: String, bytes: Vec<u8>) {
    if let Some(id) = connection(&id) {
      self.inner.on_binary(id, bytes);
    }
  }

  pub fn on_closed(&self, id: String, code: Option<u16>, reason: String) {
    if let Some(id) = connection(&id) {
      self.inner.on_closed(id, code, reason);
    }
  }
}

fn connection(id: &str) -> Option<Uuid> {
  match Uuid::parse_str(id) {
    Ok(id) => Some(id),
    Err(_) => {
      tracing::warn!(%id, "ws inbox reported an unparseable connection id; dropping");
      None
    }
  }
}

pub struct ForeignHttp(Arc<dyn HttpTransport>);

impl ForeignHttp {
  pub fn new(transport: Arc<dyn HttpTransport>) -> Self {
    Self(transport)
  }
}

impl io::HttpTransport for ForeignHttp {
  fn execute(&self, request: io::HttpRequest, sink: Arc<io::HttpSink>) {
    self.0.execute(request.into(), HttpSink::wrapping(sink));
  }

  fn download(&self, request: io::HttpRequest, sink: Arc<io::HttpDownloadSink>) {
    self.0.download(request.into(), HttpDownloadSink::wrapping(sink));
  }
}

pub struct ForeignWs(Arc<dyn WsTransport>);

impl ForeignWs {
  pub fn new(transport: Arc<dyn WsTransport>) -> Self {
    Self(transport)
  }
}

impl io::WsTransport for ForeignWs {
  fn connect(&self, connect: io::WsConnect, inbox: Arc<io::WsInbox>) {
    self.0.connect(connect.into(), WsInbox::wrapping(inbox));
  }

  fn send(&self, id: Uuid, frame: io::WsFrame) {
    self.0.send(id.to_string(), frame.into());
  }

  fn disconnect(&self, id: Uuid, code: Option<u16>, reason: Option<String>) {
    self.0.disconnect(id.to_string(), code, reason);
  }
}

impl From<io::HttpMethod> for HttpMethod {
  fn from(method: io::HttpMethod) -> Self {
    match method {
      io::HttpMethod::Get => Self::Get,
      io::HttpMethod::Head => Self::Head,
      io::HttpMethod::Post => Self::Post,
      io::HttpMethod::Put => Self::Put,
      io::HttpMethod::Patch => Self::Patch,
      io::HttpMethod::Delete => Self::Delete,
      io::HttpMethod::Options => Self::Options,
      io::HttpMethod::Other(verb) => Self::Other { verb },
    }
  }
}

impl From<HttpMethod> for io::HttpMethod {
  fn from(method: HttpMethod) -> Self {
    match method {
      HttpMethod::Get => Self::Get,
      HttpMethod::Head => Self::Head,
      HttpMethod::Post => Self::Post,
      HttpMethod::Put => Self::Put,
      HttpMethod::Patch => Self::Patch,
      HttpMethod::Delete => Self::Delete,
      HttpMethod::Options => Self::Options,
      HttpMethod::Other { verb } => Self::Other(verb),
    }
  }
}

impl From<io::WsConnect> for WsConnect {
  fn from(connect: io::WsConnect) -> Self {
    Self {
      id: connect.id.to_string(),
      url: connect.url,
      protocols: connect.protocols,
      headers: connect.headers.into_iter().map(Into::into).collect(),
    }
  }
}

impl From<io::WsFrame> for WsFrame {
  fn from(frame: io::WsFrame) -> Self {
    match frame {
      io::WsFrame::Text(text) => Self::Text { text },
      io::WsFrame::Binary(bytes) => Self::Binary { bytes },
    }
  }
}

impl From<WsFrame> for io::WsFrame {
  fn from(frame: WsFrame) -> Self {
    match frame {
      WsFrame::Text { text } => Self::Text(text),
      WsFrame::Binary { bytes } => Self::Binary(bytes),
    }
  }
}

impl From<io::HttpHeader> for HttpHeader {
  fn from(header: io::HttpHeader) -> Self {
    Self {
      name: header.name,
      value: header.value,
    }
  }
}

impl From<HttpHeader> for io::HttpHeader {
  fn from(header: HttpHeader) -> Self {
    Self {
      name: header.name,
      value: header.value,
    }
  }
}

impl From<io::HttpRequest> for HttpRequest {
  fn from(request: io::HttpRequest) -> Self {
    Self {
      method: request.method.into(),
      url: request.url,
      headers: request.headers.into_iter().map(Into::into).collect(),
      body: request.body,
      timeout_ms: request.timeout_ms,
    }
  }
}

impl From<HttpRequest> for io::HttpRequest {
  fn from(request: HttpRequest) -> Self {
    Self {
      method: request.method.into(),
      url: request.url,
      headers: request.headers.into_iter().map(Into::into).collect(),
      body: request.body,
      timeout_ms: request.timeout_ms,
    }
  }
}

impl From<HttpResponse> for io::HttpResponse {
  fn from(response: HttpResponse) -> Self {
    Self {
      status: response.status,
      headers: response.headers.into_iter().map(Into::into).collect(),
      body: response.body,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_verb_the_enum_does_not_name_survives_the_ffi_hop_in_both_directions() {
    let original = io::HttpMethod::Other("PROPFIND".into());
    let crossed: HttpMethod = original.clone().into();
    assert_eq!(
      crossed,
      HttpMethod::Other {
        verb: "PROPFIND".into()
      }
    );
    assert_eq!(io::HttpMethod::from(crossed), original);
  }

  #[test]
  fn every_named_verb_still_round_trips() {
    for method in [
      io::HttpMethod::Get,
      io::HttpMethod::Head,
      io::HttpMethod::Post,
      io::HttpMethod::Put,
      io::HttpMethod::Patch,
      io::HttpMethod::Delete,
      io::HttpMethod::Options,
    ] {
      let crossed: HttpMethod = method.clone().into();
      assert_eq!(io::HttpMethod::from(crossed), method);
    }
  }
}
