use std::{sync::Arc, time::SystemTime};

use bridgething_io::{
  HttpDownloadSink, HttpExecutor, HttpHeader, HttpMethod, HttpRequest, HttpResponse, HttpSink, HttpTransport,
};
use http_cache_semantics::{AfterResponse, BeforeRequest, CacheOptions, CachePolicy};

use super::disk::DiskCache;

const SCOPE: &str = "http";
const ENTRY_MAX_BYTES: usize = 2 << 20;
const BYPASS_HEADERS: [&str; 5] = [
  "if-none-match",
  "if-modified-since",
  "if-match",
  "if-unmodified-since",
  "range",
];

struct Stored {
  policy: CachePolicy,
  body: Vec<u8>,
}

impl Stored {
  fn encode(policy: &CachePolicy, body: &[u8]) -> Option<Vec<u8>> {
    let policy = serde_json::to_vec(policy).ok()?;
    let mut framed = Vec::with_capacity(4 + policy.len() + body.len());
    framed.extend_from_slice(&u32::try_from(policy.len()).ok()?.to_le_bytes());
    framed.extend_from_slice(&policy);
    framed.extend_from_slice(body);
    Some(framed)
  }

  fn decode(framed: &[u8]) -> Option<Self> {
    let (len, rest) = framed.split_first_chunk::<4>()?;
    let (policy, body) = rest.split_at_checked(u32::from_le_bytes(*len) as usize)?;
    Some(Self {
      policy: serde_json::from_slice(policy).ok()?,
      body: body.to_vec(),
    })
  }
}

pub struct CachingHttp {
  inner: Arc<dyn HttpTransport>,
  core: Arc<Core>,
}

struct Core {
  origin: HttpExecutor,
  store: DiskCache,
}

impl CachingHttp {
  pub fn new(inner: Arc<dyn HttpTransport>, store: DiskCache) -> Self {
    Self {
      core: Arc::new(Core {
        origin: HttpExecutor::new(inner.clone()),
        store,
      }),
      inner,
    }
  }
}

fn cacheable(request: &HttpRequest) -> bool {
  request.method == HttpMethod::Get
    && request.body.is_empty()
    && !request
      .headers
      .iter()
      .any(|header| BYPASS_HEADERS.iter().any(|name| header.name.eq_ignore_ascii_case(name)))
}

fn request_parts(request: &HttpRequest) -> Option<http::request::Parts> {
  let mut builder = http::Request::builder().method(http::Method::GET).uri(&request.url);
  for header in &request.headers {
    builder = builder.header(header.name.as_str(), header.value.as_str());
  }
  builder.body(()).ok().map(|built| built.into_parts().0)
}

fn response_parts(response: &HttpResponse) -> Option<http::response::Parts> {
  let mut builder = http::Response::builder().status(response.status);
  for header in &response.headers {
    builder = builder.header(header.name.as_str(), header.value.as_str());
  }
  builder.body(()).ok().map(|built| built.into_parts().0)
}

fn io_headers(headers: &http::HeaderMap) -> Vec<HttpHeader> {
  headers
    .iter()
    .filter_map(|(name, value)| {
      Some(HttpHeader {
        name: name.as_str().to_owned(),
        value: value.to_str().ok()?.to_owned(),
      })
    })
    .collect()
}

fn served(parts: http::response::Parts, body: Vec<u8>) -> HttpResponse {
  HttpResponse {
    status: parts.status.as_u16(),
    headers: io_headers(&parts.headers),
    body,
  }
}

fn private_policy(request: &http::request::Parts, response: &http::response::Parts) -> CachePolicy {
  CachePolicy::new_options(
    request,
    response,
    SystemTime::now(),
    CacheOptions {
      shared: false,
      ..CacheOptions::default()
    },
  )
}

impl Core {
  async fn send(&self, request: HttpRequest) -> Result<HttpResponse, String> {
    self.origin.execute(request).await.map_err(|error| error.to_string())
  }

  async fn keep(&self, url: &str, policy: CachePolicy, body: &[u8]) {
    if !policy.is_storable() || body.len() > ENTRY_MAX_BYTES {
      self.store.evict(SCOPE, url);
      return;
    }
    if let Some(bytes) = Stored::encode(&policy, body) {
      self.store.put(SCOPE, url, bytes).await;
    }
  }

  async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, String> {
    let Some(asked) = request_parts(&request) else {
      return self.send(request).await;
    };
    let stored = self
      .store
      .get(SCOPE, &request.url)
      .await
      .and_then(|bytes| Stored::decode(&bytes));
    let Some(stored) = stored else {
      let response = self.send(request.clone()).await?;
      if let Some(parts) = response_parts(&response) {
        self
          .keep(&request.url, private_policy(&asked, &parts), &response.body)
          .await;
      }
      return Ok(response);
    };
    let (revalidation, matches) = match stored.policy.before_request(&asked, SystemTime::now()) {
      BeforeRequest::Fresh(parts) => return Ok(served(parts, stored.body)),
      BeforeRequest::Stale { request, matches } => (request, matches),
    };
    let outgoing = HttpRequest {
      headers: io_headers(&revalidation.headers),
      ..request.clone()
    };
    let response = self.send(outgoing).await?;
    let Some(parts) = response_parts(&response) else {
      return Ok(response);
    };
    if !matches {
      self
        .keep(&request.url, private_policy(&asked, &parts), &response.body)
        .await;
      return Ok(response);
    }
    match stored.policy.after_response(&revalidation, &parts, SystemTime::now()) {
      AfterResponse::NotModified(policy, parts) => {
        self.keep(&request.url, policy, &stored.body).await;
        Ok(served(parts, stored.body))
      }
      AfterResponse::Modified(_, _) if response.status == 304 => {
        self.store.evict(SCOPE, &request.url);
        let response = self.send(request.clone()).await?;
        if let Some(parts) = response_parts(&response) {
          self
            .keep(&request.url, private_policy(&asked, &parts), &response.body)
            .await;
        }
        Ok(response)
      }
      AfterResponse::Modified(policy, _) => {
        self.keep(&request.url, policy, &response.body).await;
        Ok(response)
      }
    }
  }
}

impl HttpTransport for CachingHttp {
  fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>) {
    if !cacheable(&request) {
      self.inner.execute(request, sink);
      return;
    }
    let core = self.core.clone();
    tokio::spawn(async move {
      match core.fetch(request).await {
        Ok(response) => sink.complete(response),
        Err(reason) => sink.fail(reason),
      }
    });
  }

  fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    self.inner.download(request, sink);
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Mutex;

  use super::*;

  struct Origin {
    seen: Mutex<Vec<HttpRequest>>,
    answer: Mutex<HttpResponse>,
    etag: Option<String>,
    etag_on_304: bool,
  }

  impl Origin {
    fn new(headers: &[(&str, &str)], body: &[u8]) -> Arc<Self> {
      Arc::new(Self {
        seen: Mutex::new(Vec::new()),
        answer: Mutex::new(HttpResponse {
          status: 200,
          headers: headers
            .iter()
            .map(|(name, value)| HttpHeader {
              name: (*name).into(),
              value: (*value).into(),
            })
            .collect(),
          body: body.to_vec(),
        }),
        etag: headers
          .iter()
          .find(|(name, _)| *name == "etag")
          .map(|(_, value)| (*value).to_owned()),
        etag_on_304: true,
      })
    }

    fn forgetful(headers: &[(&str, &str)], body: &[u8]) -> Arc<Self> {
      let origin = Arc::try_unwrap(Self::new(headers, body)).ok().unwrap();
      Arc::new(Self {
        etag_on_304: false,
        ..origin
      })
    }

    fn hits(&self) -> usize {
      self.seen.lock().unwrap().len()
    }

    fn last_header(&self, name: &str) -> Option<String> {
      let seen = self.seen.lock().unwrap();
      let last = seen.last()?;
      last
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.clone())
    }
  }

  impl HttpTransport for Origin {
    fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>) {
      let revalidated = request
        .headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("if-none-match") && Some(&header.value) == self.etag.as_ref());
      self.seen.lock().unwrap().push(request);
      if revalidated {
        let headers = match (&self.etag, self.etag_on_304) {
          (Some(etag), true) => vec![HttpHeader {
            name: "etag".into(),
            value: etag.clone(),
          }],
          _ => Vec::new(),
        };
        sink.complete(HttpResponse {
          status: 304,
          headers,
          body: Vec::new(),
        });
      } else {
        sink.complete(self.answer.lock().unwrap().clone());
      }
    }

    fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      sink.on_failed("the net cache suite never streams".into());
    }
  }

  struct Rig {
    dir: tempfile::TempDir,
    origin: Arc<Origin>,
    exec: HttpExecutor,
  }

  fn rig(origin: Arc<Origin>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let caching = CachingHttp::new(origin.clone(), DiskCache::open(dir.path().join("net"), 1 << 20));
    Rig {
      exec: HttpExecutor::new(Arc::new(caching)),
      dir,
      origin,
    }
  }

  impl Rig {
    fn reopened(&self) -> HttpExecutor {
      HttpExecutor::new(Arc::new(CachingHttp::new(
        self.origin.clone(),
        DiskCache::open(self.dir.path().join("net"), 1 << 20),
      )))
    }
  }

  fn get(url: &str) -> HttpRequest {
    HttpRequest {
      method: HttpMethod::Get,
      url: url.into(),
      headers: Vec::new(),
      body: Vec::new(),
      timeout_ms: 0,
    }
  }

  #[tokio::test]
  async fn a_fresh_response_is_served_from_the_phone_without_the_origin() {
    let rig = rig(Origin::new(&[("cache-control", "max-age=600")], b"icons"));

    let first = rig.exec.execute(get("https://cdn.test/icons.png")).await.unwrap();
    let second = rig.exec.execute(get("https://cdn.test/icons.png")).await.unwrap();

    assert_eq!(first.body, b"icons");
    assert_eq!(second.body, b"icons");
    assert_eq!(second.status, 200);
    assert_eq!(rig.origin.hits(), 1);
  }

  #[tokio::test]
  async fn immutable_cdn_assets_outlive_a_restart() {
    let rig = rig(Origin::new(
      &[("cache-control", "public, max-age=31536000, immutable")],
      b"font",
    ));
    rig.exec.execute(get("https://cdn.test/font.woff2")).await.unwrap();

    let served = rig
      .reopened()
      .execute(get("https://cdn.test/font.woff2"))
      .await
      .unwrap();

    assert_eq!(served.body, b"font");
    assert_eq!(rig.origin.hits(), 1);
  }

  #[tokio::test]
  async fn a_stale_entry_revalidates_and_a_304_serves_the_kept_body() {
    let rig = rig(Origin::new(
      &[("cache-control", "no-cache"), ("etag", "\"v1\"")],
      b"weather",
    ));
    rig.exec.execute(get("https://api.test/weather")).await.unwrap();

    let served = rig.exec.execute(get("https://api.test/weather")).await.unwrap();

    assert_eq!(rig.origin.hits(), 2);
    assert_eq!(rig.origin.last_header("if-none-match").as_deref(), Some("\"v1\""));
    assert_eq!(served.status, 200);
    assert_eq!(served.body, b"weather");
  }

  #[tokio::test]
  async fn a_304_without_its_validator_never_leaks_to_the_caller() {
    let rig = rig(Origin::forgetful(
      &[("cache-control", "no-cache"), ("etag", "\"v1\"")],
      b"weather",
    ));
    rig.exec.execute(get("https://api.test/weather")).await.unwrap();

    let served = rig.exec.execute(get("https://api.test/weather")).await.unwrap();

    assert_eq!(served.status, 200);
    assert_eq!(served.body, b"weather");
    assert_eq!(rig.origin.hits(), 3);
  }

  #[tokio::test]
  async fn no_store_is_never_kept() {
    let rig = rig(Origin::new(&[("cache-control", "no-store")], b"secret"));
    rig.exec.execute(get("https://api.test/me")).await.unwrap();
    rig.exec.execute(get("https://api.test/me")).await.unwrap();

    assert_eq!(rig.origin.hits(), 2);
    assert!(rig.origin.last_header("if-none-match").is_none());
  }

  #[tokio::test]
  async fn writes_and_caller_conditionals_pass_straight_through() {
    let rig = rig(Origin::new(&[("cache-control", "max-age=600")], b"ok"));
    let post = HttpRequest {
      method: HttpMethod::Post,
      body: b"{}".to_vec(),
      ..get("https://api.test/thing")
    };
    rig.exec.execute(post.clone()).await.unwrap();
    rig.exec.execute(post).await.unwrap();
    let conditional = HttpRequest {
      headers: vec![HttpHeader {
        name: "If-None-Match".into(),
        value: "\"mine\"".into(),
      }],
      ..get("https://api.test/thing")
    };
    rig.exec.execute(conditional.clone()).await.unwrap();
    rig.exec.execute(conditional).await.unwrap();

    assert_eq!(rig.origin.hits(), 4);
  }

  #[tokio::test]
  async fn a_purged_cache_dir_mid_session_is_a_refetch_not_an_error() {
    let rig = rig(Origin::new(&[("cache-control", "max-age=600")], b"icons"));
    rig.exec.execute(get("https://cdn.test/icons.png")).await.unwrap();
    std::fs::remove_dir_all(rig.dir.path()).unwrap();

    let served = rig.exec.execute(get("https://cdn.test/icons.png")).await.unwrap();

    assert_eq!(served.body, b"icons");
    assert_eq!(rig.origin.hits(), 2);
  }
}
