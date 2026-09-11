use ::http::header::HeaderMap;
use bridgething_io::HttpHeader;

use crate::error::Result;

#[cfg(feature = "native-io")]
pub fn executor() -> bridgething_io::HttpExecutor {
  use std::sync::Arc;

  use bridgething_io::{HttpExecutor, ReqwestConfig, ReqwestTransport};

  use crate::http::{CLIENT_VERSION, HTTP_CONNECT_TIMEOUT, HTTP_REQUEST_TIMEOUT};

  HttpExecutor::new(Arc::new(ReqwestTransport::new(ReqwestConfig {
    user_agent: format!("Spotify/{CLIENT_VERSION} Android/36 (SM-X810)"),
    request_timeout: HTTP_REQUEST_TIMEOUT,
    connect_timeout: HTTP_CONNECT_TIMEOUT,
    ..ReqwestConfig::default()
  })))
}

pub(crate) fn headers_to_vec(map: &HeaderMap) -> Vec<HttpHeader> {
  map
    .iter()
    .map(|(k, v)| HttpHeader {
      name: k.as_str().to_string(),
      value: v.to_str().unwrap_or_default().to_string(),
    })
    .collect()
}

pub(crate) fn with_query(base: String, query: &[(&str, String)]) -> Result<String> {
  if query.is_empty() {
    return Ok(base);
  }
  let url = url::Url::parse_with_params(&base, query.iter().map(|(k, v)| (*k, v.as_str())))?;
  Ok(url.to_string())
}

pub(crate) fn form_urlencode(pairs: &[(&str, &str)]) -> Vec<u8> {
  let mut ser = url::form_urlencoded::Serializer::new(String::new());
  for (k, v) in pairs {
    ser.append_pair(k, v);
  }
  ser.finish().into_bytes()
}
