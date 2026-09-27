use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
  Router,
  body::Body,
  extract::{Path, State},
  http::{HeaderMap, HeaderValue, Response, StatusCode, header},
  routing::get,
};
use libbridgething::{
  RangePart, RangeSpec,
  gateway::{OtaAssetRange, OtaAssetRangeReply, TransferBody},
  wire::RequestError,
};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::{bytes::Bytes, sync::CancellationToken};
use uuid::Uuid;

use super::{
  BeginRangeError, RangeProxy, RangeTally,
  cache::{self, AssetLog, FetchReader, FetchWriter},
  layout::{self, EmitStep, MULTIPART_BOUNDARY},
};
use crate::{
  bluetooth::BluetoothMan,
  transfer::sinks::{AckPolicy, ForwardStream, TransferEvent, TransferSinks},
};

const INGEST_IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const BODY_READ_MAX: usize = 64 * 1024;

#[derive(Clone)]
struct AxumState {
  proxy: RangeProxy,
  bluetooth: BluetoothMan,
  sinks: TransferSinks,
}

pub(super) async fn spawn(
  proxy: RangeProxy,
  bluetooth: BluetoothMan,
  sinks: TransferSinks,
  bind: SocketAddr,
  cancel: CancellationToken,
) -> std::io::Result<(SocketAddr, JoinHandle<()>)> {
  let listener = TcpListener::bind(bind).await?;
  let bound = listener.local_addr()?;
  tracing::info!("ota range proxy listening on {bound}");

  let app = Router::new()
    .route("/{asset}", get(handle_range))
    .with_state(AxumState {
      proxy,
      bluetooth,
      sinks,
    });

  let handle = tokio::spawn(async move {
    tokio::select! {
      res = axum::serve(listener, app) => {
        if let Err(err) = res {
          tracing::error!("FATAL: ota range proxy server stopped: {err:?}");
        } else {
          tracing::warn!("ota range proxy server exited cleanly");
        }
      }
      _ = cancel.cancelled() => {
        tracing::debug!("ota range proxy server shutting down");
      }
    }
  });
  Ok((bound, handle))
}

enum Parsed {
  Fresh(Vec<RangeSpec>),
  Resume(u64),
}

async fn handle_range(State(state): State<AxumState>, Path(asset): Path<String>, headers: HeaderMap) -> Response<Body> {
  let range_header = match headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
    Some(v) => v,
    None => {
      return error_response(
        StatusCode::RANGE_NOT_SATISFIABLE,
        "Range header is required for OTA delta fetch",
      );
    }
  };
  let parsed = match parse_range_header(range_header) {
    Ok(p) => p,
    Err(reason) => return error_response(StatusCode::RANGE_NOT_SATISFIABLE, reason),
  };

  let request_id = Uuid::now_v7();
  let body_rx = state.sinks.bind_forward(request_id, AckPolicy::OnReceipt);
  let begin = match state.proxy.begin_range_active(request_id, asset.clone()).await {
    Ok(begin) => begin,
    Err(BeginRangeError::NoActiveOta) => {
      state.sinks.unbind(request_id);
      return error_response(StatusCode::CONFLICT, "no OTA in flight");
    }
    Err(BeginRangeError::ProxyDown) => {
      state.sinks.unbind(request_id);
      return error_response(StatusCode::INTERNAL_SERVER_ERROR, "range proxy unavailable");
    }
    Err(BeginRangeError::Cache(reason)) => {
      state.sinks.unbind(request_id);
      tracing::error!(%asset, %reason, "range cache could not be opened for this request");
      return error_response(StatusCode::INTERNAL_SERVER_ERROR, format!("range cache: {reason}"));
    }
  };

  let remembered = state.proxy.load_ranges(asset.clone()).await;
  let (parts, known_total, start) = match parsed {
    Parsed::Fresh(ranges) => {
      tracing::debug!(%asset, range_count = ranges.len(), "handling fresh OTA range request");
      let parts = ranges
        .into_iter()
        .map(|r| RangePart {
          start: r.start,
          length: r.length,
        })
        .collect::<Vec<_>>();
      (parts, remembered.map(|(_, total)| total), 0u64)
    }
    Parsed::Resume(offset) => {
      tracing::debug!(%asset, offset, "handling OTA range resume");
      match remembered {
        Some((parts, total)) => (parts, Some(total), offset),
        None => {
          state.sinks.unbind(request_id);
          state.proxy.end_range(request_id).await;
          tracing::warn!(%asset, offset, "resume with no remembered ranges; cannot reconstruct body");
          return error_response(StatusCode::RANGE_NOT_SATISFIABLE, "no ranges to resume");
        }
      }
    }
  };

  serve(state, request_id, begin, asset, parts, known_total, start, body_rx).await
}

#[allow(clippy::too_many_arguments)]
async fn serve(
  state: AxumState,
  request_id: Uuid,
  begin: super::RangeBegin,
  asset: String,
  parts: Vec<RangePart>,
  known_total: Option<u32>,
  start: u64,
  body_rx: ForwardStream,
) -> Response<Body> {
  let planned = layout::build(&parts, known_total.unwrap_or(0));
  let planned_plan = layout::plan_from(&planned, start, &begin.log.index());
  let plan = match planned_plan {
    Some(plan) => plan,
    None => {
      state.sinks.unbind(request_id);
      state.proxy.end_range(request_id).await;
      return error_response(StatusCode::RANGE_NOT_SATISFIABLE, "resume offset past end of body");
    }
  };

  if plan.companion_ranges.is_empty() {
    let Some(total) = known_total else {
      state.sinks.unbind(request_id);
      state.proxy.end_range(request_id).await;
      return error_response(StatusCode::BAD_GATEWAY, "no asset size known for a fully cached body");
    };
    tracing::info!(%asset, bytes = plan.cached_bytes, "serving an OTA range entirely from the cache");
    state.sinks.unbind(request_id);
    let tally = state.proxy.tally();
    tally.add_expected(plan.data_bytes());
    tally.add_served(plan.cached_bytes);
    let meta = ResponseMeta {
      is_multipart: planned.is_multipart(),
      single: planned.single,
      resume_offset: start,
      total,
      body_remaining: planned.total_body - start,
    };
    let body = Body::from_stream(emit_stream(plan.steps, begin.log, None, state.proxy, request_id));
    return build_headers(meta).body(body).unwrap();
  }

  let reply = match request_companion(&state, request_id, &begin, &asset, plan.companion_ranges.clone()).await {
    Ok(reply) => reply,
    Err(resp) => {
      state.proxy.end_range(request_id).await;
      return *resp;
    }
  };
  if reply.parts.is_empty() {
    state.proxy.end_range(request_id).await;
    return error_response(StatusCode::BAD_GATEWAY, "companion returned 0 parts");
  }

  let total = reply.total_size;
  let (planned, plan) = if known_total == Some(total) {
    (planned, plan)
  } else {
    let relaid = layout::build(&parts, total);
    let relaid_plan = layout::plan_from(&relaid, start, &begin.log.index());
    let replanned = match relaid_plan {
      Some(replanned) if replanned.companion_ranges == plan.companion_ranges => replanned,
      _ => {
        state.proxy.end_range(request_id).await;
        tracing::warn!(%asset, total, ?known_total, "asset size moved under an in-flight range request");
        return error_response(StatusCode::BAD_GATEWAY, "asset size changed mid-request");
      }
    };
    (relaid, replanned)
  };
  state.proxy.store_ranges(asset, parts, total).await;

  let meta = ResponseMeta {
    is_multipart: planned.is_multipart(),
    single: planned.single,
    resume_offset: start,
    total,
    body_remaining: planned.total_body - start,
  };
  finish_response(state, request_id, begin.log, plan, meta, reply.body, body_rx).await
}

struct ResponseMeta {
  is_multipart: bool,
  single: Option<RangePart>,
  resume_offset: u64,
  total: u32,
  body_remaining: u64,
}

async fn request_companion(
  state: &AxumState,
  request_id: Uuid,
  begin: &super::RangeBegin,
  asset: &str,
  ranges: Vec<RangeSpec>,
) -> Result<OtaAssetRangeReply, Box<Response<Body>>> {
  let req = OtaAssetRange {
    update_id: begin.update_id.clone(),
    asset: asset.to_string(),
    ranges,
  };
  match state
    .bluetooth
    .gateway_man
    .request_with_id::<OtaAssetRange>(request_id, begin.peer, req)
    .await
  {
    Ok(reply) => Ok(reply),
    Err(RequestError::Domain(rejected)) => {
      tracing::warn!(update_id = %begin.update_id, reason = %rejected.reason, "companion rejected OtaAssetRange");
      Err(Box::new(error_response(
        StatusCode::BAD_GATEWAY,
        format!("companion rejected: {}", rejected.reason),
      )))
    }
    Err(err) => {
      tracing::warn!(update_id = %begin.update_id, ?err, "OtaAssetRange wire request failed");
      Err(Box::new(error_response(StatusCode::BAD_GATEWAY, "wire request failed")))
    }
  }
}

async fn finish_response(
  state: AxumState,
  request_id: Uuid,
  log: Arc<AssetLog>,
  plan: layout::EmitPlan,
  meta: ResponseMeta,
  reply_body: TransferBody,
  body_rx: ForwardStream,
) -> Response<Body> {
  let expected = plan.companion_bytes;
  let tally = state.proxy.tally();
  let (mut writer, reader) = cache::fetch_channel(log.clone(), plan.companion_ranges.clone());

  let body = match reply_body {
    TransferBody::Inline(bytes) => {
      state.sinks.unbind(request_id);
      if bytes.len() as u64 != expected {
        writer.fail("inline body length does not match requested ranges");
        state.proxy.end_range(request_id).await;
        return error_response(
          StatusCode::BAD_GATEWAY,
          "inline body length does not match requested ranges",
        );
      }
      if let Err(err) = writer.append(&bytes).await {
        tracing::error!(?err, "range cache write failed for an inline body");
        writer.fail(format!("range cache write failed: {err}"));
        state.proxy.end_range(request_id).await;
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "range cache write failed");
      }
      writer.finish();
      tally.add_expected(plan.data_bytes());
      tally.add_served(plan.cached_bytes + expected);
      Body::from_stream(emit_stream(plan.steps, log, Some(reader), state.proxy, request_id))
    }
    TransferBody::Stream(transfer) => {
      if transfer.id != request_id {
        state.proxy.end_range(request_id).await;
        return error_response(StatusCode::BAD_GATEWAY, "stream ref id does not match request id");
      }
      if transfer.total_size as u64 != expected {
        state.proxy.end_range(request_id).await;
        return error_response(StatusCode::BAD_GATEWAY, "stream length does not match requested ranges");
      }
      tally.add_expected(plan.data_bytes());
      tally.add_served(plan.cached_bytes);
      tokio::spawn(ingest_pump(body_rx, writer, expected, request_id, tally));
      Body::from_stream(emit_stream(plan.steps, log, Some(reader), state.proxy, request_id))
    }
  };

  build_headers(meta).body(body).unwrap()
}

async fn ingest_pump(
  mut rx: ForwardStream,
  mut writer: FetchWriter,
  expected: u64,
  request_id: Uuid,
  tally: Arc<RangeTally>,
) {
  let mut received: u64 = 0;
  loop {
    let event = match tokio::time::timeout(INGEST_IDLE_TIMEOUT, rx.recv()).await {
      Ok(ev) => Ok(ev),
      Err(_) => Err(()),
    };
    let (offset, bytes) = match event {
      Err(()) => {
        tracing::warn!(%request_id, received, expected, "range ingest idle timeout; failing pull");
        writer.fail("range ingest idle timeout");
        return;
      }
      Ok(None) => {
        writer.fail("companion fragment stream closed mid-range");
        return;
      }
      Ok(Some(TransferEvent::Abandon { reason })) => {
        tracing::warn!(%request_id, received, expected, %reason, "companion abandoned range stream");
        writer.fail(format!("companion abandoned range stream: {reason}"));
        return;
      }
      Ok(Some(TransferEvent::Fragment { offset, bytes })) => (offset, bytes),
    };
    if offset as u64 != received {
      tracing::warn!(%request_id, offset, received, "range fragment offset out of order; failing pull");
      writer.fail("companion fragment offset out of order");
      return;
    }
    if received + bytes.len() as u64 > expected {
      tracing::warn!(%request_id, received, expected, "range fragment overshoots declared stream length");
      writer.fail("companion fragment overshoots declared stream length");
      return;
    }
    if let Err(err) = writer.append(&bytes).await {
      tracing::warn!(%request_id, ?err, "range cache write failed");
      writer.fail(format!("range cache write failed: {err}"));
      return;
    }
    received += bytes.len() as u64;
    tally.add_served(bytes.len() as u64);
    if received == expected {
      writer.finish();
      return;
    }
  }
}

fn build_headers(meta: ResponseMeta) -> axum::http::response::Builder {
  let mut builder = Response::builder().status(StatusCode::PARTIAL_CONTENT);
  if meta.is_multipart {
    builder = builder.header(
      header::CONTENT_TYPE,
      HeaderValue::from_str(&format!("multipart/byteranges; boundary={MULTIPART_BOUNDARY}")).unwrap(),
    );
  } else {
    let p = meta.single.expect("single-range response carries its part");
    let start = p.start + meta.resume_offset as u32;
    let end_inclusive = p.start + p.length - 1;
    builder = builder
      .header(header::CONTENT_TYPE, "application/octet-stream")
      .header(header::CONTENT_LENGTH, meta.body_remaining.to_string())
      .header(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes {start}-{end_inclusive}/{}", meta.total)).unwrap(),
      );
  }
  builder
}

fn emit_stream(
  steps: Vec<EmitStep>,
  log: Arc<AssetLog>,
  fetch: Option<FetchReader>,
  proxy: RangeProxy,
  request_id: Uuid,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
  let cleanup = OnDropEnd::new(proxy, request_id);
  async_stream::try_stream! {
    let mut fetch = fetch;
    for step in steps {
      match step {
        EmitStep::Lit(bytes) => {
          yield bytes;
        }
        EmitStep::Cached { log_off, len } => {
          let mut done = 0u64;
          while done < len as u64 {
            let take = (len as u64 - done).min(BODY_READ_MAX as u64) as usize;
            let piece = log.read_at(log_off + done, take).await?;
            done += piece.len() as u64;
            yield piece;
          }
        }
        EmitStep::Fetch(len) => {
          let reader = fetch
            .as_mut()
            .ok_or_else(|| std::io::Error::other("plan needs companion bytes with no fetch in flight"))?;
          let mut remaining = len as u64;
          while remaining > 0 {
            let piece = reader.next(remaining.min(BODY_READ_MAX as u64) as usize).await?;
            remaining -= piece.len() as u64;
            yield piece;
          }
        }
      }
    }
    drop(cleanup);
  }
}

struct OnDropEnd {
  proxy: RangeProxy,
  request_id: Uuid,
}

impl OnDropEnd {
  fn new(proxy: RangeProxy, request_id: Uuid) -> Self {
    Self { proxy, request_id }
  }
}

impl Drop for OnDropEnd {
  fn drop(&mut self) {
    let proxy = self.proxy.clone();
    let request_id = self.request_id;
    tokio::spawn(async move { proxy.end_range(request_id).await });
  }
}

fn error_response(status: StatusCode, body: impl Into<String>) -> Response<Body> {
  Response::builder()
    .status(status)
    .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
    .body(Body::from(body.into()))
    .unwrap()
}

fn parse_range_header(header_value: &str) -> Result<Parsed, &'static str> {
  let trimmed = header_value.trim();
  let payload = trimmed.strip_prefix("bytes=").ok_or("Range must start with bytes=")?;

  if let Some((start, end)) = payload.trim().split_once('-')
    && end.trim().is_empty()
    && !payload.contains(',')
  {
    let start = start.trim();
    if start.is_empty() {
      return Err("suffix ranges 'bytes=-N' are not supported");
    }
    let start: u64 = start.parse().map_err(|_| "resume offset parse failed")?;
    return Ok(Parsed::Resume(start));
  }

  let mut out = Vec::new();
  for piece in payload.split(',') {
    let piece = piece.trim();
    let (start, end) = piece.split_once('-').ok_or("range piece missing '-'")?;
    let start = start.trim();
    let end = end.trim();
    if start.is_empty() || end.is_empty() {
      return Err("only fully-bounded ranges 'a-b' are supported");
    }
    let start: u32 = start.parse().map_err(|_| "range start parse failed")?;
    let end: u32 = end.parse().map_err(|_| "range end parse failed")?;
    if end < start {
      return Err("range end < start");
    }
    let length = end
      .checked_sub(start)
      .and_then(|d| d.checked_add(1))
      .ok_or("range length overflow")?;
    out.push(RangeSpec { start, length });
  }
  if out.is_empty() {
    return Err("Range header parsed to 0 ranges");
  }
  Ok(Parsed::Fresh(out))
}

#[cfg(test)]
mod tests {
  use std::sync::{Arc as StdArc, Mutex as StdMutex};

  use libbridgething::gateway::{BridgeToGatewayMsgData, BridgeToGatewaySystemMsg, GatewayToBridgeSystemMsg};

  use super::{
    super::{
      cache::{CacheSeg, RangeCache, tests as cache_tests},
      noop_proxy,
      tests::spawn_broker_only,
    },
    *,
  };
  use crate::{bluetooth::BluetoothManager, transfer::sinks::TransferSinks};

  fn zck_byte(i: u32) -> u8 {
    (i % 251) as u8
  }

  fn serving_companion(total_size: u32) -> (BluetoothMan, StdArc<StdMutex<Vec<Vec<RangeSpec>>>>) {
    let (bluetooth, mut outbound) = BluetoothManager::capturing();
    let asked: StdArc<StdMutex<Vec<Vec<RangeSpec>>>> = StdArc::new(StdMutex::new(Vec::new()));
    let seen = asked.clone();
    let gateway = bluetooth.gateway_man.clone();
    tokio::spawn(async move {
      while let Some(out) = outbound.recv().await {
        let BridgeToGatewayMsgData::System(BridgeToGatewaySystemMsg::OtaAssetRange(req)) = &out.msg.data else {
          continue;
        };
        seen.lock().unwrap().push(req.ranges.clone());
        let reply = OtaAssetRangeReply {
          total_size,
          parts: req
            .ranges
            .iter()
            .map(|r| RangePart {
              start: r.start,
              length: r.length,
            })
            .collect(),
          body: TransferBody::Inline(zck_bytes(&req.ranges)),
        };
        gateway.complete_pending(&out.msg.id, GatewayToBridgeSystemMsg::OtaAssetRangeReply(reply).into());
      }
    });
    (bluetooth, asked)
  }

  fn silent_companion() -> BluetoothMan {
    BluetoothManager::capturing().0
  }

  fn zck_bytes(ranges: &[RangeSpec]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in ranges {
      out.extend((r.start..r.start + r.length).map(zck_byte));
    }
    out
  }

  async fn open_log(dir: &std::path::Path, update_id: &str, asset: &str) -> Arc<AssetLog> {
    let cache = RangeCache::open(dir, update_id, cache_tests::assets().await)
      .await
      .unwrap();
    cache.asset_log(asset).await.unwrap()
  }

  fn multipart_body(parts: &[RangePart], total: u32) -> Vec<u8> {
    let mut want = Vec::new();
    for p in parts {
      want.extend_from_slice(layout::multipart_part_header(p, total).as_bytes());
      want.extend((p.start..p.start + p.length).map(zck_byte));
    }
    want.extend_from_slice(format!("\r\n--{MULTIPART_BOUNDARY}--\r\n").as_bytes());
    want
  }

  async fn collect<E: std::fmt::Debug>(stream: impl futures::Stream<Item = Result<Bytes, E>>) -> Vec<u8> {
    let pieces: Vec<_> = futures::StreamExt::collect::<Vec<_>>(stream).await;
    let mut out = Vec::new();
    for piece in pieces {
      out.extend_from_slice(&piece.unwrap());
    }
    out
  }

  #[tokio::test]
  async fn acks_flow_on_receipt_while_reader_stalls() {
    let sinks = TransferSinks::default();
    let request_id = Uuid::now_v7();
    let body_rx = sinks.bind_forward(request_id, AckPolicy::OnReceipt);

    let expected: u64 = 64 * 1024;
    let log = open_log(&cache_tests::temp_dir(), "u1", "a.zck").await;
    let (writer, mut reader) = cache::fetch_channel(
      log,
      vec![RangeSpec {
        start: 0,
        length: expected as u32,
      }],
    );
    let tally = Arc::new(RangeTally::default());
    let pump = tokio::spawn(ingest_pump(body_rx, writer, expected, request_id, tally.clone()));

    let body: Vec<u8> = (0..expected).map(|i| (i % 251) as u8).collect();
    let mut acks = Vec::new();
    for (i, chunk) in body.chunks(4096).enumerate() {
      if let Some(received) = sinks.fragment(request_id, (i * 4096) as u32, Bytes::copy_from_slice(chunk)) {
        acks.push(received);
      }
      tokio::task::yield_now().await;
    }
    tokio::time::timeout(Duration::from_secs(5), pump)
      .await
      .expect("pump completes without any reader draining the cache")
      .unwrap();

    assert!(!acks.is_empty(), "receipt acks must flow while the reader stalls");
    assert_eq!(acks.last().copied(), Some(expected as u32));
    assert_eq!(tally.snapshot().0, expected);

    let mut drained = Vec::new();
    while (drained.len() as u64) < expected {
      let piece = reader.next(BODY_READ_MAX).await.unwrap();
      drained.extend_from_slice(&piece);
    }
    assert_eq!(drained, body);
  }

  #[tokio::test]
  async fn emit_stream_interleaves_headers_with_fetched_data() {
    let parts = vec![
      RangePart { start: 100, length: 50 },
      RangePart {
        start: 1000,
        length: 300,
      },
    ];
    let log = open_log(&cache_tests::temp_dir(), "u1", "a.zck").await;
    let bl = layout::build(&parts, 100_000);
    let plan = layout::plan_from(&bl, 0, &log.index()).unwrap();
    let companion = zck_bytes(&plan.companion_ranges);
    let (mut writer, reader) = cache::fetch_channel(log.clone(), plan.companion_ranges.clone());
    let feeder = tokio::spawn(async move {
      for chunk in companion.chunks(64) {
        writer.append(chunk).await.unwrap();
        tokio::task::yield_now().await;
      }
      writer.finish();
    });

    let stream = emit_stream(plan.steps, log.clone(), Some(reader), noop_proxy(), Uuid::now_v7());
    let got = collect(stream).await;
    feeder.await.unwrap();
    assert_eq!(got, multipart_body(&parts, 100_000));
  }

  #[tokio::test]
  async fn a_fully_cached_request_serves_without_a_wire_request() {
    let sinks = TransferSinks::default();
    let proxy = spawn_broker_only(sinks.clone(), cache_tests::temp_dir()).await;
    proxy.activate("u1".into(), None).await;
    let parts = vec![
      RangePart { start: 100, length: 50 },
      RangePart {
        start: 1000,
        length: 300,
      },
    ];

    let request_id = Uuid::now_v7();
    let begin = proxy.begin_range_active(request_id, "a.zck".into()).await.unwrap();
    for p in &parts {
      let bytes: Vec<u8> = (p.start..p.start + p.length).map(zck_byte).collect();
      begin.log.append(p.start, &bytes).await.unwrap();
    }
    proxy.store_ranges("a.zck".into(), parts.clone(), 100_000).await;
    proxy.end_range(request_id).await;

    let state = AxumState {
      proxy: proxy.clone(),
      bluetooth: silent_companion(),
      sinks,
    };
    let response = handle_range(
      State(state),
      Path("a.zck".to_string()),
      range_header("bytes=100-149,1000-1299"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);

    let got = collect(response.into_body().into_data_stream()).await;
    assert_eq!(
      got,
      multipart_body(&parts, 100_000),
      "a fully cached body is reproduced byte for byte with no companion answering"
    );
    assert_eq!(proxy.tally().snapshot(), (350, 350));
  }

  #[tokio::test]
  async fn a_partially_cached_request_only_fetches_the_gaps() {
    let sinks = TransferSinks::default();
    let proxy = spawn_broker_only(sinks.clone(), cache_tests::temp_dir()).await;
    proxy.activate("u1".into(), None).await;

    let request_id = Uuid::now_v7();
    let begin = proxy.begin_range_active(request_id, "a.zck".into()).await.unwrap();
    let cached: Vec<u8> = (0..40u32).map(zck_byte).collect();
    begin.log.append(0, &cached).await.unwrap();
    proxy.end_range(request_id).await;

    let (bluetooth, asked) = serving_companion(100_000);
    let state = AxumState {
      proxy: proxy.clone(),
      bluetooth,
      sinks,
    };
    let response = handle_range(State(state), Path("a.zck".to_string()), range_header("bytes=0-99")).await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);

    let got = collect(response.into_body().into_data_stream()).await;
    assert_eq!(got, (0..100u32).map(zck_byte).collect::<Vec<_>>());
    assert_eq!(
      *asked.lock().unwrap(),
      vec![vec![RangeSpec { start: 40, length: 60 }]],
      "only the uncovered gap crosses the wire"
    );
    assert_eq!(proxy.tally().snapshot(), (100, 100));
  }

  #[tokio::test]
  async fn fetched_bytes_land_in_the_cache_for_the_next_attempt() {
    let sinks = TransferSinks::default();
    let proxy = spawn_broker_only(sinks.clone(), cache_tests::temp_dir()).await;
    proxy.activate("u1".into(), None).await;

    let (bluetooth, asked) = serving_companion(100_000);
    let state = AxumState {
      proxy: proxy.clone(),
      bluetooth,
      sinks,
    };
    let response = handle_range(
      State(state.clone()),
      Path("a.zck".to_string()),
      range_header("bytes=0-99"),
    )
    .await;
    let first = collect(response.into_body().into_data_stream()).await;

    proxy.deactivate().await;
    proxy.activate("u1".into(), None).await;

    let request_id = Uuid::now_v7();
    let begin = proxy.begin_range_active(request_id, "a.zck".into()).await.unwrap();
    assert_eq!(
      begin.log.index().segments(0, 100),
      vec![CacheSeg::Cached { log_off: 8, len: 100 }],
      "the fetched range is cached across a deactivate"
    );
    proxy.end_range(request_id).await;

    let response = handle_range(State(state), Path("a.zck".to_string()), range_header("bytes=0-99")).await;
    let second = collect(response.into_body().into_data_stream()).await;
    assert_eq!(first, second, "the replayed body is byte-identical");
    assert_eq!(
      asked.lock().unwrap().len(),
      1,
      "the replay makes no second wire request"
    );
  }

  #[tokio::test]
  async fn a_restart_refetches_only_what_the_cache_is_missing() {
    let dir = cache_tests::temp_dir();
    let sinks = TransferSinks::default();
    let proxy = spawn_broker_only(sinks.clone(), dir.clone()).await;
    proxy.activate("u1".into(), None).await;

    let (bluetooth, asked) = serving_companion(100_000);
    let state = AxumState {
      proxy,
      bluetooth,
      sinks: sinks.clone(),
    };
    let response = handle_range(State(state), Path("a.zck".to_string()), range_header("bytes=0-99")).await;
    let prefix = collect(response.into_body().into_data_stream()).await;
    assert_eq!(*asked.lock().unwrap(), vec![vec![RangeSpec { start: 0, length: 100 }]]);

    let restarted = spawn_broker_only(sinks.clone(), dir).await;
    restarted.activate("u1".into(), None).await;
    let (bluetooth, asked_after) = serving_companion(100_000);
    let state = AxumState {
      proxy: restarted.clone(),
      bluetooth,
      sinks,
    };
    let response = handle_range(State(state), Path("a.zck".to_string()), range_header("bytes=0-199")).await;
    let full = collect(response.into_body().into_data_stream()).await;

    assert_eq!(full[..100], prefix[..], "the cached prefix is replayed verbatim");
    assert_eq!(full, (0..200u32).map(zck_byte).collect::<Vec<_>>());
    assert_eq!(
      *asked_after.lock().unwrap(),
      vec![vec![RangeSpec {
        start: 100,
        length: 100
      }]],
      "only the bytes the cache is missing cross the wire after a restart"
    );
    assert_eq!(restarted.tally().snapshot(), (200, 200));
  }

  #[tokio::test]
  async fn an_http_resume_replays_the_cached_prefix_across_a_restart() {
    let dir = cache_tests::temp_dir();
    let sinks = TransferSinks::default();
    let proxy = spawn_broker_only(sinks.clone(), dir.clone()).await;
    proxy.activate("u1".into(), None).await;
    let parts = vec![
      RangePart { start: 100, length: 50 },
      RangePart {
        start: 1000,
        length: 300,
      },
    ];

    let (bluetooth, asked) = serving_companion(100_000);
    let state = AxumState {
      proxy: proxy.clone(),
      bluetooth,
      sinks: sinks.clone(),
    };
    let response = handle_range(
      State(state),
      Path("a.zck".to_string()),
      range_header("bytes=100-149,1000-1299"),
    )
    .await;
    let full = collect(response.into_body().into_data_stream()).await;
    assert_eq!(full, multipart_body(&parts, 100_000));

    let restarted = spawn_broker_only(sinks.clone(), dir).await;
    restarted.activate("u1".into(), None).await;
    let (bluetooth, asked_after) = serving_companion(100_000);
    let state = AxumState {
      proxy: restarted,
      bluetooth,
      sinks,
    };
    let response = handle_range(State(state), Path("a.zck".to_string()), range_header("bytes=200-")).await;
    let resumed = collect(response.into_body().into_data_stream()).await;

    assert_eq!(
      resumed,
      full[200..],
      "a resume after a restart reproduces the body suffix"
    );
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(
      asked_after.lock().unwrap().is_empty(),
      "the whole resume comes out of the cache"
    );
  }

  #[tokio::test]
  async fn abandon_mid_stream_fails_the_body() {
    let sinks = TransferSinks::default();
    let request_id = Uuid::now_v7();
    let body_rx = sinks.bind_forward(request_id, AckPolicy::OnReceipt);

    let log = open_log(&cache_tests::temp_dir(), "u1", "a.zck").await;
    let parts = vec![RangePart { start: 0, length: 1024 }];
    let bl = layout::build(&parts, 4096);
    let plan = layout::plan_from(&bl, 0, &log.index()).unwrap();
    let (writer, reader) = cache::fetch_channel(log.clone(), plan.companion_ranges.clone());
    tokio::spawn(ingest_pump(
      body_rx,
      writer,
      1024,
      request_id,
      Arc::new(RangeTally::default()),
    ));

    sinks.fragment(request_id, 0, Bytes::from_static(&[0u8; 512]));
    sinks.abandon(request_id, "link died".into());

    let stream = emit_stream(plan.steps, log, Some(reader), noop_proxy(), request_id);
    let collected: Vec<_> = futures::StreamExt::collect::<Vec<_>>(stream).await;
    assert!(
      collected.iter().any(|r| r.is_err()),
      "an abandoned pull must error the HTTP body, not hang"
    );
  }

  fn range_header(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_str(value).unwrap());
    headers
  }

  #[tokio::test]
  async fn a_resume_without_remembered_ranges_is_not_satisfiable() {
    let sinks = TransferSinks::default();
    let proxy = spawn_broker_only(sinks.clone(), cache_tests::temp_dir()).await;
    proxy.activate("u1".into(), None).await;
    let state = AxumState {
      proxy,
      bluetooth: silent_companion(),
      sinks,
    };
    let response = handle_range(State(state), Path("a.zck".to_string()), range_header("bytes=12-")).await;
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
  }

  fn fresh(header: &str) -> Vec<RangeSpec> {
    match parse_range_header(header).unwrap() {
      Parsed::Fresh(r) => r,
      Parsed::Resume(_) => panic!("expected fresh"),
    }
  }

  #[test]
  fn parses_single_range() {
    assert_eq!(fresh("bytes=0-99"), vec![RangeSpec { start: 0, length: 100 }]);
  }

  #[test]
  fn parses_multi_range() {
    assert_eq!(
      fresh("bytes=0-99,200-299"),
      vec![
        RangeSpec { start: 0, length: 100 },
        RangeSpec {
          start: 200,
          length: 100
        },
      ]
    );
  }

  #[test]
  fn open_ended_range_parses_as_resume() {
    assert!(matches!(
      parse_range_header("bytes=134321-"),
      Ok(Parsed::Resume(134321))
    ));
  }

  #[test]
  fn rejects_suffix_range() {
    assert!(parse_range_header("bytes=-100").is_err());
  }

  #[test]
  fn rejects_inverted_range() {
    assert!(parse_range_header("bytes=10-5").is_err());
  }

  #[test]
  fn rejects_missing_prefix() {
    assert!(parse_range_header("0-99").is_err());
  }
}
