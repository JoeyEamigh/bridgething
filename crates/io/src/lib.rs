pub mod http;
pub mod ws;

#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
pub mod native;
#[cfg(all(feature = "web-io", target_arch = "wasm32"))]
pub mod web;

pub use http::{
  DownloadBody, DownloadOutcome, HttpDownloadSink, HttpError, HttpExecutor, HttpHeader, HttpMethod, HttpRequest,
  HttpResponse, HttpSink, HttpTransport,
};
#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
pub use native::{ReqwestConfig, ReqwestTransport, TungsteniteTransport};
#[cfg(all(feature = "web-io", target_arch = "wasm32"))]
pub use web::FetchTransport;
pub use ws::{WsConnect, WsEvent, WsFrame, WsInbox, WsTransport};

pub fn install_crypto_provider() {
  #[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
  native::install_ring();
}
