use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
  },
  time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use bridgething_io::{DownloadBody, HttpExecutor, HttpHeader, HttpMethod, HttpRequest};
use serde::{Deserialize, Serialize, Serializer};
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::oneshot;
use url::Url;

use crate::shell::Shell;

const MAX_BODY_BYTES: usize = 1024 * 1024;
const AUTHORIZE_TIMEOUT: Duration = Duration::from_secs(300);
const CALLBACK_PREFIX: &str = "bridgething://oauth/callback";

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
  #[error("network: {0}")]
  Network(String),
  #[error("timeout: {0}")]
  Timeout(String),
  #[error("invalid_url: {0}")]
  InvalidUrl(String),
  #[error("cancelled")]
  Cancelled,
  #[error("busy: an authorization is already in flight")]
  Busy,
  #[error("unsupported: {0}")]
  Unsupported(String),
}

impl Serialize for SettingsError {
  fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&self.to_string())
  }
}

type Answer<T> = Result<T, SettingsError>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BodyKind {
  Text,
  Base64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WireBody {
  pub kind: BodyKind,
  pub data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequest {
  pub url: String,
  pub method: Option<String>,
  pub headers: Option<Vec<(String, String)>>,
  pub body: Option<WireBody>,
  pub timeout_ms: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct FetchReply {
  pub status: u16,
  pub headers: Vec<(String, String)>,
  pub body: WireBody,
}

#[derive(Debug, Serialize)]
pub struct AuthorizeReply {
  pub url: String,
}

struct Claim {
  token: u64,
  tx: oneshot::Sender<Url>,
}

pub struct Claimed {
  pub token: u64,
  pub waiting: oneshot::Receiver<Url>,
}

pub struct Awaiting {
  pub token: u64,
  waiting: oneshot::Receiver<Url>,
  authorize: Arc<Authorize>,
}

impl Awaiting {
  pub async fn settled(self) -> Answer<Url> {
    let Self {
      token,
      waiting,
      authorize,
    } = self;
    match tokio::time::timeout(AUTHORIZE_TIMEOUT, waiting).await {
      Ok(Ok(callback)) => Ok(callback),
      Ok(Err(_)) => Err(SettingsError::Cancelled),
      Err(_) => {
        authorize.release(token);
        Err(SettingsError::Cancelled)
      }
    }
  }
}

#[derive(Default)]
pub struct Authorize {
  pending: Mutex<Option<Claim>>,
  next: AtomicU64,
}

impl Authorize {
  pub fn claim(&self) -> Answer<Claimed> {
    let mut pending = self.pending.lock().unwrap();
    if pending.as_ref().is_some_and(|claim| !claim.tx.is_closed()) {
      return Err(SettingsError::Busy);
    }
    let token = self.next.fetch_add(1, Ordering::Relaxed) + 1;
    let (tx, waiting) = oneshot::channel();
    *pending = Some(Claim { token, tx });
    Ok(Claimed { token, waiting })
  }

  pub fn release(&self, token: u64) {
    let mut pending = self.pending.lock().unwrap();
    if pending.as_ref().is_some_and(|claim| claim.token == token) {
      pending.take();
    }
  }

  pub fn begin(self: &Arc<Self>, url: String, open: impl FnOnce(String) -> Result<(), String>) -> Answer<Awaiting> {
    authorize_url(&url)?;
    let claimed = self.claim()?;
    if let Err(reason) = open(url) {
      self.release(claimed.token);
      return Err(SettingsError::Unsupported(reason));
    }
    Ok(Awaiting {
      token: claimed.token,
      waiting: claimed.waiting,
      authorize: Arc::clone(self),
    })
  }

  pub fn deliver(&self, url: &Url) -> bool {
    if !url.as_str().starts_with(CALLBACK_PREFIX) {
      return false;
    }
    let Some(claim) = self.pending.lock().unwrap().take() else {
      tracing::debug!(%url, "an oauth callback arrived with no authorization waiting for it");
      return false;
    };
    claim.tx.send(url.clone()).is_ok()
  }
}

#[tauri::command]
pub async fn settings_fetch(shell: State<'_, Arc<Shell>>, request: FetchRequest) -> Answer<FetchReply> {
  let FetchRequest {
    url,
    method: verb,
    headers,
    body,
    timeout_ms,
  } = request;
  web_url(&url)?;

  fetch(
    shell.http(),
    HttpRequest {
      method: method(verb.as_deref()),
      url,
      headers: headers
        .unwrap_or_default()
        .into_iter()
        .map(|(name, value)| HttpHeader { name, value })
        .collect(),
      body: body.map(decode).transpose()?.unwrap_or_default(),
      timeout_ms: timeout_ms.unwrap_or(0),
    },
  )
  .await
}

#[tauri::command]
pub async fn settings_authorize<R: Runtime>(
  app: AppHandle<R>,
  authorize: State<'_, Arc<Authorize>>,
  url: String,
) -> Answer<AuthorizeReply> {
  let awaiting = authorize.inner().begin(url, |url| {
    app
      .opener()
      .open_url(url, None::<&str>)
      .map_err(|error| error.to_string())
  })?;

  Ok(AuthorizeReply {
    url: awaiting.settled().await?.to_string(),
  })
}

struct Capped {
  buffer: Vec<u8>,
  refused: Option<u64>,
}

struct CappedBody(Arc<Mutex<Capped>>);

impl DownloadBody for CappedBody {
  fn on_response(&mut self, _status: u16, _headers: &[HttpHeader], content_length: Option<u64>) -> bool {
    let Some(declared) = content_length.filter(|declared| *declared > MAX_BODY_BYTES as u64) else {
      return true;
    };
    self.0.lock().unwrap().refused = Some(declared);
    false
  }

  fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
    let mut held = self.0.lock().unwrap();
    let seen = held.buffer.len() + chunk.len();
    if seen > MAX_BODY_BYTES {
      held.refused = Some(seen as u64);
      return Err(over_cap(seen as u64));
    }
    held.buffer.extend_from_slice(chunk);
    Ok(())
  }
}

fn over_cap(seen: u64) -> String {
  format!("response body is {seen} bytes, over the {MAX_BODY_BYTES} byte cap")
}

async fn fetch(http: &HttpExecutor, request: HttpRequest) -> Answer<FetchReply> {
  let held = Arc::new(Mutex::new(Capped {
    buffer: Vec::new(),
    refused: None,
  }));
  let outcome = http.download(request, Box::new(CappedBody(Arc::clone(&held)))).await;

  let (buffer, refused) = {
    let mut held = held.lock().unwrap();
    (std::mem::take(&mut held.buffer), held.refused)
  };
  if let Some(seen) = refused {
    return Err(SettingsError::Network(over_cap(seen)));
  }
  let outcome = outcome.map_err(|error| transport_failure(&error.to_string()))?;

  let content_type = outcome
    .headers
    .iter()
    .find(|header| header.name.eq_ignore_ascii_case("content-type"))
    .map(|header| header.value.as_str());

  Ok(FetchReply {
    status: outcome.status,
    headers: outcome
      .headers
      .iter()
      .map(|header| (header.name.clone(), header.value.clone()))
      .collect(),
    body: encode(buffer, content_type),
  })
}

pub(crate) fn web_url(raw: &str) -> Answer<()> {
  let url = Url::parse(raw).map_err(|error| SettingsError::InvalidUrl(format!("{raw}: {error}")))?;
  match url.scheme() {
    "http" | "https" => Ok(()),
    scheme => Err(SettingsError::InvalidUrl(format!("{scheme} is not a web scheme"))),
  }
}

pub(crate) fn authorize_url(raw: &str) -> Answer<()> {
  web_url(raw).map_err(|error| match error {
    SettingsError::InvalidUrl(reason) => SettingsError::Unsupported(reason),
    other => other,
  })
}

fn method(raw: Option<&str>) -> HttpMethod {
  let raw = raw.unwrap_or("GET");
  match raw.to_ascii_uppercase().as_str() {
    "GET" => HttpMethod::Get,
    "HEAD" => HttpMethod::Head,
    "POST" => HttpMethod::Post,
    "PUT" => HttpMethod::Put,
    "PATCH" => HttpMethod::Patch,
    "DELETE" => HttpMethod::Delete,
    "OPTIONS" => HttpMethod::Options,
    _ => HttpMethod::Other(raw.to_owned()),
  }
}

fn decode(body: WireBody) -> Answer<Vec<u8>> {
  let bytes = match body.kind {
    BodyKind::Text => body.data.into_bytes(),
    BodyKind::Base64 => STANDARD
      .decode(body.data)
      .map_err(|error| SettingsError::Network(format!("the request body is not base64: {error}")))?,
  };
  if bytes.len() > MAX_BODY_BYTES {
    return Err(SettingsError::Network(format!(
      "request body is {} bytes, over the {MAX_BODY_BYTES} byte cap",
      bytes.len()
    )));
  }
  Ok(bytes)
}

fn encode(bytes: Vec<u8>, content_type: Option<&str>) -> WireBody {
  let texty = content_type.is_some_and(|value| {
    let value = value.to_ascii_lowercase();
    value.contains("json") || value.contains("text") || value.contains("xml") || value.contains("urlencoded")
  });

  let bytes = if texty {
    match String::from_utf8(bytes) {
      Ok(data) => {
        return WireBody {
          kind: BodyKind::Text,
          data,
        };
      }
      Err(error) => error.into_bytes(),
    }
  } else {
    bytes
  };

  WireBody {
    kind: BodyKind::Base64,
    data: STANDARD.encode(bytes),
  }
}

fn transport_failure(reason: &str) -> SettingsError {
  if reason.contains("timed out") || reason.contains("timeout") {
    SettingsError::Timeout(reason.to_owned())
  } else {
    SettingsError::Network(reason.to_owned())
  }
}

#[cfg(test)]
mod tests {
  use bridgething_io::{HttpDownloadSink, HttpSink, HttpTransport};

  use super::*;

  fn callback() -> Url {
    Url::parse("bridgething://oauth/callback?code=abc").expect("a callback url")
  }

  #[test]
  fn a_second_authorization_is_refused_while_one_is_in_flight() {
    let authorize = Authorize::default();
    let held = authorize.claim().expect("the first claim takes the slot");

    assert!(
      matches!(authorize.claim(), Err(SettingsError::Busy)),
      "two browser flows at once would race for the one callback"
    );

    authorize.release(held.token);
    assert!(authorize.claim().is_ok(), "releasing the slot lets the next page in");
  }

  #[test]
  fn a_stale_release_leaves_the_claim_that_replaced_it_alone() {
    let authorize = Authorize::default();
    let first = authorize.claim().expect("the first claim");
    authorize.release(first.token);
    let second = authorize.claim().expect("the second claim");

    authorize.release(first.token);

    assert!(
      matches!(authorize.claim(), Err(SettingsError::Busy)),
      "a timed-out request must not free the slot a newer one holds"
    );
    assert!(
      authorize.deliver(&callback()),
      "the newer request still gets its callback"
    );
    drop(second);
  }

  #[test]
  fn a_dropped_waiter_frees_the_slot_without_a_release() {
    let authorize = Authorize::default();
    let held = authorize.claim().expect("a claim");
    drop(held.waiting);

    assert!(
      authorize.claim().is_ok(),
      "a page that navigated away must not wedge the host for five minutes"
    );
  }

  #[test]
  fn only_the_oauth_callback_scheme_settles_a_pending_authorization() {
    let authorize = Authorize::default();
    let mut held = authorize.claim().expect("a claim");

    for hostile in [
      "https://example.com/oauth/callback?code=abc",
      "bridgething://other/path?code=abc",
    ] {
      let url = Url::parse(hostile).expect("a url");
      assert!(!authorize.deliver(&url), "{hostile} must not answer an authorization");
    }

    assert!(authorize.deliver(&callback()));
    assert_eq!(
      held.waiting.try_recv().expect("the callback arrived"),
      callback(),
      "the page needs the whole callback url, query and all"
    );
  }

  #[test]
  fn a_callback_with_nobody_waiting_is_dropped() {
    let authorize = Authorize::default();
    assert!(!authorize.deliver(&callback()));
  }

  #[test]
  fn only_web_urls_reach_the_network_and_the_browser() {
    assert!(web_url("https://example.com/x").is_ok());
    assert!(web_url("http://example.com/x").is_ok());

    for hostile in ["file:///Users/joey/.ssh/", "javascript:alert(1)", "not a url"] {
      assert!(
        matches!(web_url(hostile), Err(SettingsError::InvalidUrl(_))),
        "{hostile} is not a fetch target"
      );
      assert!(
        matches!(authorize_url(hostile), Err(SettingsError::Unsupported(_))),
        "{hostile} must come back as a kind the sdk's AuthorizeErrorKind carries"
      );
    }
  }

  #[test]
  fn a_body_round_trips_as_text_and_falls_back_to_base64() {
    let text = encode(b"{\"a\":1}".to_vec(), Some("application/json; charset=utf-8"));
    assert!(matches!(text.kind, BodyKind::Text));
    assert_eq!(text.data, "{\"a\":1}");
    assert_eq!(
      decode(text).expect("a text body decodes"),
      b"{\"a\":1}",
      "the text branch is byte-identical in both directions"
    );

    let binary = encode(vec![0x00, 0xff, 0x10], Some("image/png"));
    assert!(matches!(binary.kind, BodyKind::Base64));
    assert_eq!(decode(binary).expect("a base64 body decodes"), vec![0x00, 0xff, 0x10]);

    let broken = encode(vec![0xff, 0xfe, 0xfd], Some("text/plain"));
    assert!(
      matches!(broken.kind, BodyKind::Base64),
      "a texty content-type that is not utf-8 falls back rather than losing bytes"
    );
    assert_eq!(decode(broken).expect("the fallback decodes"), vec![0xff, 0xfe, 0xfd]);
  }

  #[test]
  fn a_request_body_over_the_cap_is_refused_before_it_is_sent() {
    let body = WireBody {
      kind: BodyKind::Text,
      data: "x".repeat(MAX_BODY_BYTES + 1),
    };
    assert!(matches!(decode(body), Err(SettingsError::Network(_))));

    let edge = WireBody {
      kind: BodyKind::Text,
      data: "x".repeat(MAX_BODY_BYTES),
    };
    assert!(decode(edge).is_ok(), "the cap itself is allowed");
  }

  #[test]
  fn a_verb_this_host_does_not_name_still_crosses_to_the_transport() {
    assert_eq!(method(None), HttpMethod::Get);
    assert_eq!(method(Some("post")), HttpMethod::Post);
    for verb in ["PROPFIND", "TRACE", "MKCOL", "REPORT"] {
      assert_eq!(
        method(Some(verb)),
        HttpMethod::Other(verb.to_owned()),
        "{verb} is a real request on the phone, so refusing it here is a host that disagrees with itself"
      );
    }
    assert_eq!(
      method(Some("propfind")),
      HttpMethod::Other("propfind".to_owned()),
      "an unnamed verb keeps the spelling the page wrote, the way the phone forwards it"
    );
  }

  #[derive(Default)]
  struct Serving {
    declared: Option<u64>,
    chunks: Vec<Vec<u8>>,
    seen: Mutex<Option<HttpRequest>>,
  }

  impl HttpTransport for Serving {
    fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
      sink.fail("this test only streams".to_owned());
    }

    fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      *self.seen.lock().unwrap() = Some(request);
      sink.on_response(
        200,
        vec![HttpHeader {
          name: "content-type".to_owned(),
          value: "text/plain".to_owned(),
        }],
        self.declared,
      );
      for chunk in &self.chunks {
        sink.on_chunk(chunk.clone());
      }
      sink.on_finished();
    }
  }

  fn get() -> HttpRequest {
    HttpRequest {
      method: HttpMethod::Get,
      url: "https://example.com/body".to_owned(),
      headers: Vec::new(),
      body: Vec::new(),
      timeout_ms: 0,
    }
  }

  #[tokio::test]
  async fn a_declared_length_over_the_cap_is_refused_before_a_byte_is_read() {
    let http = HttpExecutor::new(Arc::new(Serving {
      declared: Some(MAX_BODY_BYTES as u64 + 1),
      chunks: vec![vec![b'x'; 8]],
      ..Serving::default()
    }));

    let failure = fetch(&http, get()).await.expect_err("an oversized body is refused");
    assert_eq!(
      failure.to_string(),
      format!("network: {}", over_cap(MAX_BODY_BYTES as u64 + 1)),
      "the refusal names the declared size, not what was read"
    );
  }

  #[tokio::test]
  async fn a_chunked_body_stops_at_the_cap_instead_of_buffering_the_whole_thing() {
    let http = HttpExecutor::new(Arc::new(Serving {
      declared: None,
      chunks: vec![vec![b'x'; MAX_BODY_BYTES], vec![b'x'; 4096]],
      ..Serving::default()
    }));

    let failure = fetch(&http, get()).await.expect_err("a body past the cap is refused");
    assert_eq!(
      failure.to_string(),
      format!("network: {}", over_cap(MAX_BODY_BYTES as u64 + 4096)),
      "an undeclared length is caught by the running counter, not after the fact"
    );
  }

  #[tokio::test]
  async fn an_unnamed_verb_reaches_the_transport_unchanged() {
    let serving = Arc::new(Serving {
      declared: Some(2),
      chunks: vec![b"ok".to_vec()],
      ..Serving::default()
    });
    let http = HttpExecutor::new(serving.clone());

    let reply = fetch(
      &http,
      HttpRequest {
        method: method(Some("PROPFIND")),
        ..get()
      },
    )
    .await
    .expect("a webdav request is a request like any other");

    assert_eq!(reply.status, 200);
    let seen = serving.seen.lock().unwrap().clone().expect("the transport saw it");
    assert_eq!(
      seen.method,
      HttpMethod::Other("PROPFIND".to_owned()),
      "the verb reaches reqwest verbatim rather than being relabelled a network failure"
    );
  }

  #[tokio::test]
  async fn a_body_inside_the_cap_arrives_whole() {
    let http = HttpExecutor::new(Arc::new(Serving {
      declared: Some(6),
      chunks: vec![b"hel".to_vec(), b"lo!".to_vec()],
      ..Serving::default()
    }));

    let reply = fetch(&http, get()).await.expect("a small body");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body.data, "hello!", "the chunks are joined in order");
    assert!(matches!(reply.body.kind, BodyKind::Text));
  }
}
