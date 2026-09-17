mod bus;
mod connection;
mod connman;

use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
  Router,
  body::Body,
  extract::{ConnectInfo, FromRequest, State as AxumState, WebSocketUpgrade, ws::WebSocket},
  http::Request,
  response::{IntoResponse, Response},
};
pub use bus::WireEventBus;
#[cfg(feature = "test-tap")]
pub use connman::TappedFrame;
pub use connman::{ClientMan, ClientScope, create_client_manager};
use reqwest::StatusCode;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::util::ServiceExt;
use tower_http::services::ServeDir;

use crate::{
  handler::client::{ClientMode, PossibleSendMsg},
  state::State as BridgeThingState,
};

type ServerTx = tokio::sync::mpsc::Sender<(WebSocket, SocketAddr, ClientMode, ClientScope)>;
type ServerRx = tokio::sync::mpsc::Receiver<(WebSocket, SocketAddr, ClientMode, ClientScope)>;

pub struct Server {
  rx: ServerRx,
  cancel_token: CancellationToken,

  _stock_handle: tokio::task::JoinHandle<()>,
  _modern_handle: tokio::task::JoinHandle<()>,
  #[cfg(feature = "test-tap")]
  _frame_tap_handle: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct ModernRouterState {
  state: BridgeThingState,
  tx: ServerTx,
}

pub const STOCK_FD_NAME: &str = "bridgething-stock";
pub const MODERN_FD_NAME: &str = "bridgething-modern";

pub struct Listeners {
  stock: TcpListener,
  modern: TcpListener,
  #[cfg(feature = "test-tap")]
  frame_tap: TcpListener,

  stock_addr: SocketAddr,
  modern_addr: SocketAddr,
  #[cfg(feature = "test-tap")]
  frame_tap_addr: SocketAddr,
}

impl Listeners {
  pub async fn bind(
    stock_bind: SocketAddr,
    modern_bind: SocketAddr,
    #[cfg(feature = "test-tap")] frame_tap_bind: SocketAddr,
  ) -> WSResult<Self> {
    let mut inherited = crate::systemd::socket::inherited_listeners();

    let stock = take_or_bind(&mut inherited, STOCK_FD_NAME, stock_bind).await?;
    let modern = take_or_bind(&mut inherited, MODERN_FD_NAME, modern_bind).await?;
    #[cfg(feature = "test-tap")]
    let frame_tap = TcpListener::bind(frame_tap_bind).await?;

    for name in inherited.keys() {
      tracing::warn!("ignoring unclaimed inherited socket {name:?}");
    }

    let stock_addr = stock.local_addr().unwrap_or(stock_bind);
    let modern_addr = modern.local_addr().unwrap_or(modern_bind);
    #[cfg(feature = "test-tap")]
    let frame_tap_addr = frame_tap.local_addr().unwrap_or(frame_tap_bind);
    tracing::info!("listening on {stock_addr} (stock) and {modern_addr} (modern + file serve)");

    Ok(Self {
      stock,
      modern,
      #[cfg(feature = "test-tap")]
      frame_tap,

      stock_addr,
      modern_addr,
      #[cfg(feature = "test-tap")]
      frame_tap_addr,
    })
  }

  pub fn modern_addr(&self) -> SocketAddr {
    self.modern_addr
  }

  #[cfg(feature = "test-tap")]
  pub fn stock_addr(&self) -> SocketAddr {
    self.stock_addr
  }

  #[cfg(feature = "test-tap")]
  pub fn frame_tap_addr(&self) -> SocketAddr {
    self.frame_tap_addr
  }
}

async fn take_or_bind(
  inherited: &mut HashMap<String, TcpListener>,
  name: &str,
  fallback: SocketAddr,
) -> WSResult<TcpListener> {
  match inherited.remove(name) {
    Some(listener) => {
      tracing::info!("adopted inherited socket {name:?} from the service manager");
      Ok(listener)
    }
    None => Ok(TcpListener::bind(fallback).await?),
  }
}

impl Server {
  pub fn serve(state: BridgeThingState, listeners: Listeners) -> Self {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let cancel_token = CancellationToken::new();

    #[cfg(feature = "test-tap")]
    let frame_tap_state = state.clone();

    let stock_app = Router::new()
      .fallback(axum::routing::any(stock_ws_handler))
      .with_state(Arc::new(tx.clone()));

    let modern_state = ModernRouterState { state, tx };
    let modern_app = Router::new()
      .fallback(axum::routing::any(modern_handler))
      .with_state(modern_state);

    let Listeners {
      stock: stock_listener,
      modern: modern_listener,
      #[cfg(feature = "test-tap")]
        frame_tap: frame_tap_listener,
      stock_addr,
      modern_addr,
      #[cfg(feature = "test-tap")]
      frame_tap_addr,
    } = listeners;
    tracing::info!("serving on {stock_addr} (stock) and {modern_addr} (modern + file serve)");

    let stock_cancel_token = cancel_token.clone();
    let _stock_handle = tokio::spawn(async move {
      tokio::select! {
        _ = axum::serve(stock_listener, stock_app.into_make_service_with_connect_info::<SocketAddr>()) => {
          tracing::error!("FATAL: stock server stopped");
        }
        _ = stock_cancel_token.cancelled() => {
          tracing::debug!("stock server shutting down");
        }
      }
    });

    let modern_cancel_token = cancel_token.clone();
    let _modern_handle = tokio::spawn(async move {
      tokio::select! {
        _ = axum::serve(modern_listener, modern_app.into_make_service_with_connect_info::<SocketAddr>()) => {
          tracing::error!("FATAL: modern server stopped");
        }
        _ = modern_cancel_token.cancelled() => {
          tracing::debug!("modern server shutting down");
        }
      }
    });

    #[cfg(feature = "test-tap")]
    let _frame_tap_handle = {
      let frame_tap_app = Router::new()
        .fallback(axum::routing::any(frame_tap_ws_handler))
        .with_state(frame_tap_state);
      tracing::info!("serving on {frame_tap_addr} (frame-tap egress mirror)");

      let frame_tap_cancel_token = cancel_token.clone();
      tokio::spawn(async move {
        tokio::select! {
          _ = axum::serve(frame_tap_listener, frame_tap_app.into_make_service()) => {
            tracing::error!("FATAL: frame-tap server stopped");
          }
          _ = frame_tap_cancel_token.cancelled() => {
            tracing::debug!("frame-tap server shutting down");
          }
        }
      })
    };

    Self {
      rx,
      cancel_token,

      _stock_handle,
      _modern_handle,
      #[cfg(feature = "test-tap")]
      _frame_tap_handle,
    }
  }

  /// cancel-safe
  pub async fn listen(&mut self) -> WSResult<(WebSocket, SocketAddr, ClientMode, ClientScope)> {
    self.rx.recv().await.ok_or(WSError::ChannelClosed)
  }

  pub async fn shutdown(self) {
    self.cancel_token.cancel();
    if let Err(err) = self._stock_handle.await {
      tracing::error!("failed to shutdown stock server: {:?}", err);
    }
    if let Err(err) = self._modern_handle.await {
      tracing::error!("failed to shutdown modern server: {:?}", err);
    }
    #[cfg(feature = "test-tap")]
    if let Err(err) = self._frame_tap_handle.await {
      tracing::error!("failed to shutdown frame-tap server: {:?}", err);
    }
  }
}

async fn stock_ws_handler(
  ws: WebSocketUpgrade,
  AxumState(tx): AxumState<Arc<ServerTx>>,
  ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
  tracing::info!("new stock port websocket connection from {}", addr);

  let tx = tx.clone();
  ws.on_upgrade(move |socket| async move {
    if let Err(err) = tx
      .send((socket, addr, ClientMode::Stock, ClientScope::ActiveWebapp))
      .await
    {
      tracing::error!("failed to send new connection to server: {:?}", err);
    }
  })
}

async fn modern_handler(
  ConnectInfo(addr): ConnectInfo<SocketAddr>,
  AxumState(state): AxumState<ModernRouterState>,
  req: Request<Body>,
) -> Response {
  if req.headers().contains_key("upgrade") {
    let scope = scope_from_query(req.uri().query());
    return match WebSocketUpgrade::from_request(req, &()).await {
      Ok(ws) => modern_ws_handler(ws, addr, scope, state.tx.clone())
        .await
        .into_response(),
      Err(err) => {
        tracing::error!("failed to upgrade request to websocket: {:?}", err);
        (StatusCode::BAD_REQUEST, err.body_text()).into_response()
      }
    };
  }

  let req = match try_serve_hub(&state.state, req).await {
    Ok(resp) => return resp,
    Err(req) => *req,
  };

  let active_path = match resolve_active_webapp(&state.state).await {
    Some(p) => p,
    None => {
      tracing::error!("no active webapp resolved; cannot serve request");
      return (StatusCode::SERVICE_UNAVAILABLE, "no active webapp").into_response();
    }
  };

  serve_from_dir(active_path, req).await
}

const HUB_PREFIX: &str = "/_hub/";

async fn try_serve_hub(state: &BridgeThingState, req: Request<Body>) -> Result<Response, Box<Request<Body>>> {
  if !req.uri().path().starts_with(HUB_PREFIX) {
    return Err(Box::new(req));
  }
  let Ok(Some(launcher)) = state.launcher_webapp().await else {
    return Err(Box::new(req));
  };
  let Some(hash) = state.webapps.bundle_hash(launcher).await else {
    return Err(Box::new(req));
  };
  let Some(bundle_path) = state.webapps.resolve(launcher).await else {
    return Err(Box::new(req));
  };

  let path = req.uri().path().to_owned();
  let after_prefix = &path[HUB_PREFIX.len()..];
  let (segment_hash, rest) = match after_prefix.find('/') {
    Some(idx) => (&after_prefix[..idx], &after_prefix[idx + 1..]),
    None => (after_prefix, ""),
  };

  if segment_hash.is_empty() {
    let location = format!("{HUB_PREFIX}{hash}/");
    return Ok(
      axum::http::Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(axum::http::header::LOCATION, location)
        .body(Body::empty())
        .expect("static redirect response"),
    );
  }

  if segment_hash != hash {
    let location = format!("{HUB_PREFIX}{hash}/{rest}");
    return Ok(
      axum::http::Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(axum::http::header::LOCATION, location)
        .body(Body::empty())
        .expect("hash redirect response"),
    );
  }

  let inner_path = if rest.is_empty() { "/" } else { rest };
  let new_uri: axum::http::Uri = format!("/{}", inner_path.trim_start_matches('/'))
    .parse()
    .expect("inner uri");
  let (mut parts, body) = req.into_parts();
  parts.uri = new_uri;
  let sub_req = Request::from_parts(parts, body);

  let svc = ServeDir::new(&bundle_path).precompressed_gzip();
  let resp = match svc.oneshot(sub_req).await {
    Ok(r) => r,
    Err(err) => {
      tracing::error!("hub ServeDir error: {:?}", err);
      return Ok((StatusCode::INTERNAL_SERVER_ERROR, "serve error").into_response());
    }
  };
  let mut resp = resp.map(Body::new);
  resp.headers_mut().insert(
    axum::http::header::CACHE_CONTROL,
    axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
  );
  Ok(resp)
}

fn scope_from_query(query: Option<&str>) -> ClientScope {
  match query {
    Some(query) if query.split('&').any(|pair| pair == "scope=overlay") => ClientScope::Overlay,
    _ => ClientScope::ActiveWebapp,
  }
}

async fn modern_ws_handler(
  ws: WebSocketUpgrade,
  addr: SocketAddr,
  scope: ClientScope,
  tx: ServerTx,
) -> impl IntoResponse {
  tracing::info!(?scope, "new modern port websocket connection from {}", addr);

  ws.on_upgrade(move |socket| async move {
    if let Err(err) = tx.send((socket, addr, ClientMode::Modern, scope)).await {
      tracing::error!("failed to send new connection to server: {:?}", err);
    }
  })
}

#[cfg(feature = "test-tap")]
async fn frame_tap_ws_handler(
  ws: WebSocketUpgrade,
  AxumState(state): AxumState<BridgeThingState>,
) -> impl IntoResponse {
  let mut frames = state.client_man.subscribe_frames();
  ws.on_upgrade(move |mut socket| async move {
    loop {
      match frames.recv().await {
        Ok(frame) => {
          let Ok(json) = serde_json::to_string(&frame) else {
            continue;
          };
          if socket
            .send(axum::extract::ws::Message::Text(json.into()))
            .await
            .is_err()
          {
            break;
          }
        }
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
      }
    }
  })
}

async fn resolve_active_webapp(state: &BridgeThingState) -> Option<PathBuf> {
  let id = match state.active_webapp().await {
    Ok(Some(id)) => id,
    Ok(None) => return None,
    Err(err) => {
      tracing::warn!(?err, "failed to read active webapp; treating as no active webapp");
      return None;
    }
  };
  state.webapps.resolve(id).await
}

async fn serve_from_dir(dir: PathBuf, req: Request<Body>) -> Response {
  let path = req.uri().path().to_owned();
  let svc = ServeDir::new(dir).precompressed_gzip();
  match svc.oneshot(req).await {
    Ok(mut resp) => {
      apply_cache_control(&mut resp, &path);
      resp.map(Body::new)
    }
    Err(err) => {
      tracing::error!("ServeDir error: {:?}", err);
      (StatusCode::INTERNAL_SERVER_ERROR, "serve error").into_response()
    }
  }
}

fn apply_cache_control<B>(resp: &mut axum::http::Response<B>, path: &str) {
  let value = if path.starts_with("/assets/") {
    "public, max-age=31536000, immutable"
  } else {
    "no-cache"
  };
  resp.headers_mut().insert(
    axum::http::header::CACHE_CONTROL,
    axum::http::HeaderValue::from_static(value),
  );
}

pub type WSResult<T> = Result<T, WSError>;

#[derive(Debug, thiserror::Error)]
pub enum WSError {
  #[error("failed to bind to port: {0}")]
  Bind(#[from] std::io::Error),
  #[error("websocket error: {0}")]
  Websocket(#[from] axum::Error),
  #[error("requested client to send to is not connected to the server!!")]
  NotConnected,
  #[error("could not send a message to requested client: {0}")]
  MessageSend(Box<tokio::sync::mpsc::error::SendError<PossibleSendMsg>>),
  #[error("could not send a message to requested client: {0}")]
  MessageTrySend(Box<tokio::sync::mpsc::error::TrySendError<PossibleSendMsg>>),
  #[error("channel from connections to server struct has been dropped!!! this is bad.")]
  ChannelClosed,
  #[error("failed to broadcast to all devices. check the logs for more info.")]
  BroadcastFailed,
}

impl From<tokio::sync::mpsc::error::SendError<PossibleSendMsg>> for WSError {
  fn from(err: tokio::sync::mpsc::error::SendError<PossibleSendMsg>) -> Self {
    Self::MessageSend(Box::new(err))
  }
}

impl From<tokio::sync::mpsc::error::TrySendError<PossibleSendMsg>> for WSError {
  fn from(err: tokio::sync::mpsc::error::TrySendError<PossibleSendMsg>) -> Self {
    Self::MessageTrySend(Box::new(err))
  }
}

#[macro_export]
macro_rules! impl_broadcast_failure_from {
  ($err:ty) => {
    impl ::core::convert::From<::std::vec::Vec<$crate::net::WSError>> for $err {
      fn from(errors: ::std::vec::Vec<$crate::net::WSError>) -> Self {
        for error in errors {
          tracing::error!("failed to broadcast message: {:?}", error);
        }
        Self::WS($crate::net::WSError::BroadcastFailed)
      }
    }
  };
}
