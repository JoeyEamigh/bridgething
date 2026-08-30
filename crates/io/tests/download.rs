use std::sync::{Arc, Mutex};

use bridgething_io::{
  DownloadBody, HttpDownloadSink, HttpError, HttpExecutor, HttpHeader, HttpMethod, HttpRequest, HttpSink, HttpTransport,
};

fn request(url: &str) -> HttpRequest {
  HttpRequest {
    method: HttpMethod::Get,
    url: url.to_string(),
    headers: Vec::new(),
    body: Vec::new(),
    timeout_ms: 0,
  }
}

struct RecordingBody {
  chunks: Arc<Mutex<Vec<Vec<u8>>>>,
  fail_at: Option<usize>,
  takes_any_status: bool,
}

impl DownloadBody for RecordingBody {
  fn on_response(&mut self, status: u16, _headers: &[HttpHeader], _content_length: Option<u64>) -> bool {
    self.takes_any_status || (200..300).contains(&status)
  }

  fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
    let mut chunks = self.chunks.lock().unwrap();
    if self.fail_at == Some(chunks.len()) {
      return Err("no space left on device".to_string());
    }
    chunks.push(chunk.to_vec());
    Ok(())
  }
}

fn body(chunks: &Arc<Mutex<Vec<Vec<u8>>>>) -> Box<dyn DownloadBody> {
  Box::new(RecordingBody {
    chunks: chunks.clone(),
    fail_at: None,
    takes_any_status: false,
  })
}

struct Streaming {
  status: u16,
  content_length: Option<u64>,
  chunks: Vec<Vec<u8>>,
}

impl HttpTransport for Streaming {
  fn execute(&self, _request: HttpRequest, _sink: Arc<HttpSink>) {
    unreachable!("the download suite never takes the whole-body arm");
  }

  fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    sink.on_response(self.status, Vec::new(), self.content_length);
    for chunk in &self.chunks {
      sink.on_chunk(chunk.clone());
    }
    sink.on_finished();
  }
}

struct CutsOff {
  chunks: Vec<Vec<u8>>,
  reason: String,
}

impl HttpTransport for CutsOff {
  fn execute(&self, _request: HttpRequest, _sink: Arc<HttpSink>) {
    unreachable!("the download suite never takes the whole-body arm");
  }

  fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    sink.on_response(200, Vec::new(), None);
    for chunk in &self.chunks {
      sink.on_chunk(chunk.clone());
    }
    sink.on_failed(self.reason.clone());
  }
}

struct Silent;

impl HttpTransport for Silent {
  fn execute(&self, _request: HttpRequest, _sink: Arc<HttpSink>) {
    unreachable!("the download suite never takes the whole-body arm");
  }

  fn download(&self, _request: HttpRequest, _sink: Arc<HttpDownloadSink>) {}
}

fn streaming(chunks: Vec<Vec<u8>>) -> Arc<Streaming> {
  let content_length = chunks.iter().map(|c| c.len() as u64).sum();
  Arc::new(Streaming {
    status: 200,
    content_length: Some(content_length),
    chunks,
  })
}

#[tokio::test]
async fn a_streamed_body_reaches_the_writer_chunk_by_chunk() {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let exec = HttpExecutor::new(streaming(vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]));

  exec
    .download(request("https://ota.example/x.bin"), body(&seen))
    .await
    .unwrap();

  let seen = seen.lock().unwrap();
  assert_eq!(seen.len(), 3, "each chunk is written as it arrives, not coalesced");
  assert_eq!(seen.concat(), b"onetwothree");
}

#[tokio::test]
async fn the_outcome_carries_the_status_the_byte_count_and_the_declared_length() {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let exec = HttpExecutor::new(streaming(vec![vec![0u8; 40], vec![1u8; 60]]));

  let outcome = exec
    .download(request("https://ota.example/x.bin"), body(&seen))
    .await
    .unwrap();

  assert_eq!(outcome.status, 200);
  assert_eq!(outcome.received, 100);
  assert_eq!(outcome.content_length, Some(100));
}

#[tokio::test]
async fn a_length_the_server_never_declared_is_absent_rather_than_zero() {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let exec = HttpExecutor::new(Arc::new(Streaming {
    status: 200,
    content_length: None,
    chunks: vec![vec![7u8; 12]],
  }));

  let outcome = exec
    .download(request("https://ota.example/x.bin"), body(&seen))
    .await
    .unwrap();

  assert_eq!(outcome.content_length, None);
  assert_eq!(outcome.received, 12);
}

#[tokio::test]
async fn a_non_success_status_is_reported_without_writing_the_body() {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let exec = HttpExecutor::new(Arc::new(Streaming {
    status: 404,
    content_length: Some(9),
    chunks: vec![b"not found".to_vec()],
  }));

  let outcome = exec
    .download(request("https://ota.example/x.bin"), body(&seen))
    .await
    .unwrap();

  assert_eq!(outcome.status, 404);
  assert_eq!(outcome.received, 0);
  assert!(
    seen.lock().unwrap().is_empty(),
    "an error page must never reach the artifact writer"
  );
}

#[tokio::test]
async fn a_writer_that_wants_error_bodies_gets_them() {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let exec = HttpExecutor::new(Arc::new(Streaming {
    status: 404,
    content_length: Some(9),
    chunks: vec![b"not found".to_vec()],
  }));

  let outcome = exec
    .download(
      request("https://api.example/thing"),
      Box::new(RecordingBody {
        chunks: seen.clone(),
        fail_at: None,
        takes_any_status: true,
      }),
    )
    .await
    .unwrap();

  assert_eq!(outcome.status, 404);
  assert_eq!(outcome.received, 9);
  assert_eq!(
    seen.lock().unwrap().concat(),
    b"not found",
    "a proxied response body belongs to its caller whatever the status says"
  );
}

#[tokio::test]
async fn a_writer_error_fails_the_download() {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let exec = HttpExecutor::new(streaming(vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]));

  let err = exec
    .download(
      request("https://ota.example/x.bin"),
      Box::new(RecordingBody {
        chunks: seen.clone(),
        fail_at: Some(1),
        takes_any_status: false,
      }),
    )
    .await
    .unwrap_err();

  assert!(matches!(err, HttpError::Body(ref reason) if reason == "no space left on device"));
  assert_eq!(seen.lock().unwrap().len(), 1, "writing stops at the first failure");
}

#[tokio::test]
async fn a_transport_that_fails_mid_stream_surfaces_the_reason() {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let exec = HttpExecutor::new(Arc::new(CutsOff {
    chunks: vec![b"half".to_vec()],
    reason: "connection reset by peer".to_string(),
  }));

  let err = exec
    .download(request("https://ota.example/x.bin"), body(&seen))
    .await
    .unwrap_err();

  assert_eq!(err.to_string(), "connection reset by peer");
}

#[tokio::test]
async fn a_transport_that_drops_the_download_sink_fails_the_request() {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let exec = HttpExecutor::new(Arc::new(Silent));

  let err = exec
    .download(request("https://ota.example/x.bin"), body(&seen))
    .await
    .unwrap_err();

  assert_eq!(err.to_string(), "http transport dropped without responding");
}

#[tokio::test]
async fn headers_the_response_carried_are_available_on_the_outcome() {
  let seen = Arc::new(Mutex::new(Vec::new()));

  struct WithHeaders;

  impl HttpTransport for WithHeaders {
    fn execute(&self, _request: HttpRequest, _sink: Arc<HttpSink>) {
      unreachable!("the download suite never takes the whole-body arm");
    }

    fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      sink.on_response(
        206,
        vec![HttpHeader {
          name: "content-range".to_string(),
          value: "bytes 0-3/8".to_string(),
        }],
        Some(4),
      );
      sink.on_chunk(vec![0u8; 4]);
      sink.on_finished();
    }
  }

  let exec = HttpExecutor::new(Arc::new(WithHeaders));

  let outcome = exec
    .download(request("https://ota.example/x.bin"), body(&seen))
    .await
    .unwrap();

  assert_eq!(outcome.status, 206);
  assert_eq!(
    outcome.headers,
    vec![HttpHeader {
      name: "content-range".to_string(),
      value: "bytes 0-3/8".to_string(),
    }]
  );
}

struct NeverReached;

impl HttpTransport for NeverReached {
  fn execute(&self, request: HttpRequest, _sink: Arc<HttpSink>) {
    unreachable!("a request the executor should have refused reached the transport: {request:?}");
  }

  fn download(&self, request: HttpRequest, _sink: Arc<HttpDownloadSink>) {
    unreachable!("a request the executor should have refused reached the transport: {request:?}");
  }
}

fn with_verb(verb: &str) -> HttpRequest {
  HttpRequest {
    method: HttpMethod::Other(verb.to_string()),
    ..request("http://127.0.0.1:1/anything")
  }
}

#[tokio::test]
async fn a_verb_that_is_not_a_token_is_refused_rather_than_sent_as_something_else() {
  let exec = HttpExecutor::new(Arc::new(NeverReached));

  for verb in ["PROP FIND", "GET\r\nX-Smuggled: 1", "GET\u{7f}", ""] {
    let refused = exec.execute(with_verb(verb)).await;
    assert!(
      matches!(refused, Err(HttpError::InvalidRequest(_))),
      "{verb:?} must be an invalid request, got {refused:?}"
    );

    let refused = exec
      .download(
        with_verb(verb),
        Box::new(RecordingBody {
          chunks: Arc::new(Mutex::new(Vec::new())),
          fail_at: None,
          takes_any_status: true,
        }),
      )
      .await;
    assert!(
      matches!(refused, Err(HttpError::InvalidRequest(_))),
      "{verb:?} must be an invalid download too, got {refused:?}"
    );
  }
}

#[tokio::test]
async fn a_verb_that_is_a_token_still_reaches_the_transport() {
  let exec = HttpExecutor::new(streaming(vec![vec![1, 2, 3]]));
  let outcome = exec
    .download(
      with_verb("PROPFIND"),
      Box::new(RecordingBody {
        chunks: Arc::new(Mutex::new(Vec::new())),
        fail_at: None,
        takes_any_status: true,
      }),
    )
    .await
    .expect("a well formed custom verb is not the executor's business to refuse");
  assert_eq!(outcome.received, 3);
}
