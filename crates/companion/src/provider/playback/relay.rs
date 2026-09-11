use std::{
  collections::VecDeque,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use bridgething_io::{DownloadBody, HttpHeader};
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::{TcpListener, TcpStream},
  sync::{oneshot, watch},
  task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::icy::{IcyDemux, IcyMetadata};

const WINDOW: usize = 256 * 1024;
const PRIME: usize = 32 * 1024;
const SERVE_CHUNK: usize = 16 * 1024;
const HEAD_LIMIT: usize = 16 * 1024;
const HEAD_DEADLINE: Duration = Duration::from_secs(5);
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);
const DEFAULT_CONTENT_TYPE: &str = "audio/mpeg";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
  pub live: bool,
  pub station: Option<String>,
  pub relay: Option<String>,
}

fn header<'a>(headers: &'a [HttpHeader], name: &str) -> Option<&'a str> {
  headers
    .iter()
    .find(|header| header.name.eq_ignore_ascii_case(name))
    .map(|header| header.value.trim())
    .filter(|value| !value.is_empty())
}

pub fn classify(status: u16, headers: &[HttpHeader]) -> (Verdict, Option<usize>) {
  let station = header(headers, "icy-name").map(str::to_owned);
  let metaint = header(headers, "icy-metaint")
    .and_then(|value| value.parse::<usize>().ok())
    .filter(|metaint| *metaint > 0 && (200..300).contains(&status));
  let content_type = header(headers, "content-type")
    .map(|value| value.to_owned())
    .unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_owned());
  let verdict = Verdict {
    live: station.is_some() || header(headers, "icy-metaint").is_some(),
    station,
    relay: metaint.map(|_| content_type),
  };
  (verdict, metaint)
}

pub struct Feed {
  state: Mutex<FeedState>,
  changed: watch::Sender<()>,
}

struct FeedState {
  buf: VecDeque<u8>,
  base: u64,
  done: Option<Result<(), String>>,
  closed: bool,
}

enum Served {
  Bytes(Vec<u8>),
  Ended,
  Gone,
  Wait,
}

impl Feed {
  pub fn new() -> Arc<Self> {
    Arc::new(Feed {
      state: Mutex::new(FeedState {
        buf: VecDeque::new(),
        base: 0,
        done: None,
        closed: false,
      }),
      changed: watch::Sender::new(()),
    })
  }

  fn push(&self, bytes: &[u8]) {
    let mut state = self.state.lock().unwrap();
    if state.closed || state.done.is_some() {
      return;
    }
    state.buf.extend(bytes);
    if state.buf.len() > WINDOW {
      let drop = state.buf.len() - WINDOW;
      state.buf.drain(..drop);
      state.base += drop as u64;
    }
    drop(state);
    self.changed.send_replace(());
  }

  pub fn finish(&self, outcome: Result<(), String>) {
    let mut state = self.state.lock().unwrap();
    if state.closed || state.done.is_some() {
      return;
    }
    state.done = Some(outcome);
    drop(state);
    self.changed.send_replace(());
  }

  pub fn close(&self) {
    self.state.lock().unwrap().closed = true;
    self.changed.send_replace(());
  }

  pub fn closed(&self) -> bool {
    self.state.lock().unwrap().closed
  }

  fn edge_cursor(&self) -> u64 {
    let state = self.state.lock().unwrap();
    let end = state.base + state.buf.len() as u64;
    end - state.buf.len().min(PRIME) as u64
  }

  fn read_from(&self, cursor: &mut u64) -> Served {
    let state = self.state.lock().unwrap();
    if state.closed {
      return Served::Gone;
    }
    if *cursor < state.base {
      *cursor = state.base;
    }
    let offset = (*cursor - state.base) as usize;
    let available = state.buf.len().saturating_sub(offset);
    if available > 0 {
      let take = available.min(SERVE_CHUNK);
      let bytes: Vec<u8> = state.buf.range(offset..offset + take).copied().collect();
      *cursor += take as u64;
      return Served::Bytes(bytes);
    }
    match &state.done {
      Some(Ok(())) => Served::Ended,
      Some(Err(_)) => Served::Gone,
      None => Served::Wait,
    }
  }
}

pub struct OriginBody {
  feed: Arc<Feed>,
  verdict: Option<oneshot::Sender<Verdict>>,
  abandoned: Arc<AtomicBool>,
  demux: Option<IcyDemux>,
  on_metadata: Box<dyn FnMut(IcyMetadata) + Send>,
}

impl OriginBody {
  pub fn new(
    feed: Arc<Feed>,
    verdict: oneshot::Sender<Verdict>,
    abandoned: Arc<AtomicBool>,
    on_metadata: Box<dyn FnMut(IcyMetadata) + Send>,
  ) -> Self {
    OriginBody {
      feed,
      verdict: Some(verdict),
      abandoned,
      demux: None,
      on_metadata,
    }
  }
}

impl DownloadBody for OriginBody {
  fn on_response(&mut self, status: u16, headers: &[HttpHeader], _content_length: Option<u64>) -> bool {
    let (mut verdict, metaint) = classify(status, headers);
    if self.abandoned.load(Ordering::SeqCst) {
      verdict.relay = None;
    }
    let relayed = verdict.relay.is_some();
    if let Some(metaint) = metaint.filter(|_| relayed) {
      self.demux = Some(IcyDemux::new(metaint));
    }
    if let Some(verdict_tx) = self.verdict.take() {
      let _ = verdict_tx.send(verdict);
    }
    relayed
  }

  fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
    if self.feed.closed() {
      return Err("the relay stopped".to_owned());
    }
    let Self {
      feed,
      demux,
      on_metadata,
      ..
    } = self;
    let Some(demux) = demux.as_mut() else {
      return Err("the origin was not relayed".to_owned());
    };
    demux.push(chunk, |audio| feed.push(audio), on_metadata);
    Ok(())
  }
}

pub struct Relay {
  pub url: String,
  feed: Arc<Feed>,
  cancel: CancellationToken,
  fetch: JoinHandle<()>,
  accept: JoinHandle<()>,
}

impl Relay {
  pub async fn bind(feed: Arc<Feed>, content_type: String, fetch: JoinHandle<()>) -> std::io::Result<Self> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let token: Arc<str> = uuid::Uuid::now_v7().simple().to_string().into();
    let url = format!("http://127.0.0.1:{port}/{token}");
    let cancel = CancellationToken::new();
    let accept = tokio::spawn(accept_loop(
      listener,
      Arc::clone(&feed),
      token,
      content_type.into(),
      cancel.clone(),
    ));
    Ok(Relay {
      url,
      feed,
      cancel,
      fetch,
      accept,
    })
  }

  pub fn stop(&self) {
    self.feed.close();
    self.cancel.cancel();
    self.accept.abort();
    self.fetch.abort();
  }
}

impl Drop for Relay {
  fn drop(&mut self) {
    self.stop();
  }
}

async fn accept_loop(
  listener: TcpListener,
  feed: Arc<Feed>,
  token: Arc<str>,
  content_type: Arc<str>,
  cancel: CancellationToken,
) {
  loop {
    let accepted = tokio::select! {
      _ = cancel.cancelled() => return,
      accepted = listener.accept() => accepted,
    };
    match accepted {
      Ok((socket, _)) => {
        let cancel = cancel.clone();
        let serving = serve(socket, Arc::clone(&feed), Arc::clone(&token), Arc::clone(&content_type));
        tokio::spawn(async move {
          tokio::select! {
            _ = cancel.cancelled() => {}
            _ = serving => {}
          }
        });
      }
      Err(error) => {
        tracing::debug!(%error, "the stream relay could not accept a player connection");
        tokio::time::sleep(ACCEPT_BACKOFF).await;
      }
    }
  }
}

async fn head(socket: &mut TcpStream) -> Option<String> {
  let mut buffer = Vec::new();
  let read = async {
    let mut byte = [0u8; 1];
    loop {
      if socket.read(&mut byte).await.ok()? == 0 {
        return None;
      }
      buffer.push(byte[0]);
      if buffer.ends_with(b"\r\n\r\n") {
        return Some(String::from_utf8_lossy(&buffer).into_owned());
      }
      if buffer.len() > HEAD_LIMIT {
        return None;
      }
    }
  };
  tokio::time::timeout(HEAD_DEADLINE, read).await.ok().flatten()
}

async fn serve(mut socket: TcpStream, feed: Arc<Feed>, token: Arc<str>, content_type: Arc<str>) {
  let Some(request) = head(&mut socket).await else {
    return;
  };
  let mut line = request.lines().next().unwrap_or_default().split_whitespace();
  let method = line.next().unwrap_or_default();
  let target = line.next().unwrap_or_default();
  if target.strip_prefix('/') != Some(&*token) {
    let _ = socket
      .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
      .await;
    return;
  }
  let headers = format!(
    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nAccept-Ranges: none\r\nCache-Control: no-cache, no-store\r\nConnection: close\r\n\r\n"
  );
  if socket.write_all(headers.as_bytes()).await.is_err() || method == "HEAD" {
    return;
  }
  let mut cursor = feed.edge_cursor();
  let mut changed = feed.changed.subscribe();
  loop {
    match feed.read_from(&mut cursor) {
      Served::Bytes(bytes) => {
        let framed = [format!("{:x}\r\n", bytes.len()).into_bytes(), bytes, b"\r\n".to_vec()].concat();
        if socket.write_all(&framed).await.is_err() {
          return;
        }
      }
      Served::Ended => {
        let _ = socket.write_all(b"0\r\n\r\n").await;
        let _ = socket.shutdown().await;
        return;
      }
      Served::Gone => return,
      Served::Wait => {
        if changed.changed().await.is_err() {
          return;
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn drained(feed: &Feed, cursor: &mut u64) -> Vec<u8> {
    let mut out = Vec::new();
    while let Served::Bytes(bytes) = feed.read_from(cursor) {
      out.extend_from_slice(&bytes);
    }
    out
  }

  #[test]
  fn the_window_never_holds_more_than_its_bound() {
    let feed = Feed::new();
    for _ in 0..8 {
      feed.push(&[7u8; 100 * 1024]);
    }
    let state = feed.state.lock().unwrap();
    assert_eq!(state.buf.len(), WINDOW);
    assert_eq!(state.base, 800 * 1024 - WINDOW as u64);
  }

  #[test]
  fn a_stalled_player_skips_ahead_to_what_is_still_buffered() {
    let feed = Feed::new();
    feed.push(&[1u8; 1024]);
    let mut cursor = feed.edge_cursor();
    assert_eq!(cursor, 0);
    assert_eq!(drained(&feed, &mut cursor).len(), 1024);
    for _ in 0..6 {
      feed.push(&[2u8; 100 * 1024]);
    }
    let base = feed.state.lock().unwrap().base;
    assert!(base > cursor, "the player's cursor fell out of the window");
    let caught_up = drained(&feed, &mut cursor);
    assert_eq!(caught_up.len(), WINDOW);
    assert!(caught_up.iter().all(|byte| *byte == 2));
    assert_eq!(cursor, base + WINDOW as u64);
  }

  #[test]
  fn a_fresh_player_starts_a_short_way_behind_the_live_edge() {
    let feed = Feed::new();
    feed.push(&[3u8; 200 * 1024]);
    let cursor = feed.edge_cursor();
    assert_eq!(cursor, (200 * 1024 - PRIME) as u64);
  }

  #[test]
  fn a_finished_feed_reports_its_end_after_the_last_byte() {
    let feed = Feed::new();
    feed.push(b"tail");
    feed.finish(Ok(()));
    let mut cursor = 0;
    assert_eq!(drained(&feed, &mut cursor), b"tail");
    assert!(matches!(feed.read_from(&mut cursor), Served::Ended));
    feed.push(b"late");
    assert!(matches!(feed.read_from(&mut cursor), Served::Ended));
  }

  #[test]
  fn a_failed_or_closed_feed_reports_gone() {
    let failed = Feed::new();
    failed.finish(Err("reset".into()));
    assert!(matches!(failed.read_from(&mut 0), Served::Gone));

    let closed = Feed::new();
    closed.push(b"bytes");
    closed.close();
    assert!(matches!(closed.read_from(&mut 0), Served::Gone));
    assert!(closed.closed());
  }

  #[test]
  fn only_a_metaint_origin_is_relayed() {
    let named = vec![HttpHeader {
      name: "icy-name".into(),
      value: "Groove Salad".into(),
    }];
    let (verdict, metaint) = classify(200, &named);
    assert_eq!(
      verdict,
      Verdict {
        live: true,
        station: Some("Groove Salad".into()),
        relay: None
      }
    );
    assert_eq!(metaint, None);

    let framed = vec![
      HttpHeader {
        name: "Icy-MetaInt".into(),
        value: "16000".into(),
      },
      HttpHeader {
        name: "Content-Type".into(),
        value: "audio/aac".into(),
      },
    ];
    let (verdict, metaint) = classify(200, &framed);
    assert_eq!(
      verdict,
      Verdict {
        live: true,
        station: None,
        relay: Some("audio/aac".into())
      }
    );
    assert_eq!(metaint, Some(16000));

    let (refused, metaint) = classify(404, &framed);
    assert_eq!(refused.relay, None);
    assert_eq!(metaint, None);

    let (plain, _) = classify(200, &[]);
    assert!(!plain.live);
    assert_eq!(plain.relay, None);
  }
}
