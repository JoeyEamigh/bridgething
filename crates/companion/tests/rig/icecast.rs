use std::{
  io::{Read, Write},
  net::{TcpListener, TcpStream},
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
  thread,
  time::Duration,
};

use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpStream as AsyncTcpStream,
};

const METADATA_UNIT: usize = 16;
const TRICKLE: Duration = Duration::from_millis(10);
const FETCH_DEADLINE: Duration = Duration::from_secs(10);

pub struct Station {
  pub name: &'static str,
  pub content_type: &'static str,
  pub metaint: usize,
  pub audio: Vec<u8>,
  pub titles: Vec<Option<&'static str>>,
  pub trickle: bool,
  pub logo: Option<&'static [u8]>,
}

pub struct Icecast {
  pub url: String,
  pub audio: Vec<u8>,
  closed: Arc<AtomicUsize>,
  requests: Arc<Mutex<Vec<String>>>,
}

impl Icecast {
  pub fn serve(station: Station) -> Self {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port for the icecast emulator");
    let port = listener.local_addr().expect("a bound address").port();
    let closed = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let audio = station.audio.clone();
    let station = Arc::new(station);
    let counting = Arc::clone(&closed);
    let recording = Arc::clone(&requests);
    let logo_url = format!("http://127.0.0.1:{port}/logo.jpg");
    thread::spawn(move || {
      for accepted in listener.incoming() {
        let Ok(socket) = accepted else { continue };
        let station = Arc::clone(&station);
        let counting = Arc::clone(&counting);
        let recording = Arc::clone(&recording);
        let logo_url = logo_url.clone();
        thread::spawn(move || answer(socket, &station, &logo_url, &counting, &recording));
      }
    });
    Icecast {
      url: format!("http://127.0.0.1:{port}/live"),
      audio,
      closed,
      requests,
    }
  }

  pub fn closed(&self) -> usize {
    self.closed.load(Ordering::SeqCst)
  }

  pub fn requests(&self) -> Vec<String> {
    self.requests.lock().unwrap().clone()
  }
}

fn metadata_block(title: Option<&str>, logo_url: Option<&str>) -> Vec<u8> {
  let Some(title) = title else {
    return vec![0];
  };
  let mut payload = format!("StreamTitle='{title}';").into_bytes();
  if let Some(logo_url) = logo_url {
    payload.extend_from_slice(format!("StreamUrl='{logo_url}';").as_bytes());
  }
  let units = payload.len().div_ceil(METADATA_UNIT);
  payload.resize(units * METADATA_UNIT, 0);
  let mut block = vec![units as u8];
  block.extend_from_slice(&payload);
  block
}

pub fn framed(audio: &[u8], metaint: usize, titles: &[Option<&str>], logo_url: Option<&str>) -> Vec<u8> {
  let mut out = Vec::new();
  for (index, chunk) in audio.chunks(metaint).enumerate() {
    out.extend_from_slice(chunk);
    if chunk.len() == metaint {
      out.extend_from_slice(&metadata_block(titles.get(index).copied().flatten(), logo_url));
    }
  }
  out
}

fn read_head(socket: &mut TcpStream) -> Option<String> {
  let mut head = Vec::new();
  let mut byte = [0u8; 1];
  while !head.ends_with(b"\r\n\r\n") {
    if socket.read(&mut byte).ok()? == 0 {
      return None;
    }
    head.push(byte[0]);
  }
  Some(String::from_utf8_lossy(&head).into_owned())
}

fn answer(
  mut socket: TcpStream,
  station: &Station,
  logo_url: &str,
  closed: &AtomicUsize,
  requests: &Mutex<Vec<String>>,
) {
  let Some(request) = read_head(&mut socket) else { return };
  let wants_metadata = request
    .lines()
    .any(|line| line.to_ascii_lowercase().replace(' ', "") == "icy-metadata:1");
  let wants_logo = request.starts_with("GET /logo.jpg ");
  requests.lock().unwrap().push(request);
  if wants_logo {
    let (status, body) = match station.logo {
      Some(logo) => ("200 OK", logo),
      None => ("404 Not Found", &b""[..]),
    };
    let head = format!(
      "HTTP/1.0 {status}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
      body.len()
    );
    let _ = socket.write_all(head.as_bytes()).and_then(|()| socket.write_all(body));
    return;
  }
  let mut head = format!(
    "HTTP/1.0 200 OK\r\nContent-Type: {}\r\nicy-name: {}\r\nicy-pub: 0\r\n",
    station.content_type, station.name
  );
  if wants_metadata {
    head.push_str(&format!("icy-metaint: {}\r\n", station.metaint));
  }
  head.push_str("\r\n");
  let body = match wants_metadata {
    true => framed(
      &station.audio,
      station.metaint,
      &station.titles,
      station.logo.map(|_| logo_url),
    ),
    false => station.audio.clone(),
  };
  if socket.write_all(head.as_bytes()).is_err() || socket.write_all(&body).is_err() {
    closed.fetch_add(1, Ordering::SeqCst);
    return;
  }
  if !station.trickle {
    return;
  }
  let mut filler = vec![0u8; station.metaint];
  if wants_metadata {
    filler.push(0);
  }
  loop {
    thread::sleep(TRICKLE);
    if socket.write_all(&filler).is_err() {
      closed.fetch_add(1, Ordering::SeqCst);
      return;
    }
  }
}

pub struct Fetched {
  pub status: u16,
  pub headers: Vec<(String, String)>,
  pub body: Vec<u8>,
}

impl Fetched {
  pub fn header(&self, name: &str) -> Option<&str> {
    self
      .headers
      .iter()
      .find(|(held, _)| held.eq_ignore_ascii_case(name))
      .map(|(_, value)| value.as_str())
  }
}

pub fn authority(url: &str) -> &str {
  let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
  rest.split('/').next().unwrap_or(rest)
}

pub async fn fetch(url: &str, extra_headers: &[(&str, &str)]) -> Result<Fetched, String> {
  tokio::time::timeout(FETCH_DEADLINE, fetch_now(url, extra_headers))
    .await
    .map_err(|_| "the relay fetch timed out".to_owned())?
}

async fn fetch_now(url: &str, extra_headers: &[(&str, &str)]) -> Result<Fetched, String> {
  let authority = authority(url);
  let path = url
    .split_once("://")
    .and_then(|(_, rest)| rest.find('/').map(|at| &rest[at..]))
    .unwrap_or("/");
  let mut socket = AsyncTcpStream::connect(authority)
    .await
    .map_err(|error| error.to_string())?;
  let mut request = format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\n");
  for (name, value) in extra_headers {
    request.push_str(&format!("{name}: {value}\r\n"));
  }
  request.push_str("\r\n");
  socket
    .write_all(request.as_bytes())
    .await
    .map_err(|error| error.to_string())?;

  let mut buffer = Vec::new();
  let head_end = loop {
    if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
      break at + 4;
    }
    let mut chunk = [0u8; 4096];
    let read = socket.read(&mut chunk).await.map_err(|error| error.to_string())?;
    if read == 0 {
      return Err("the relay closed before the head".to_owned());
    }
    buffer.extend_from_slice(&chunk[..read]);
  };
  let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
  let mut lines = head.lines();
  let status = lines
    .next()
    .and_then(|line| line.split_whitespace().nth(1))
    .and_then(|code| code.parse::<u16>().ok())
    .ok_or_else(|| format!("no status line in {head:?}"))?;
  let headers: Vec<(String, String)> = lines
    .filter_map(|line| line.split_once(':'))
    .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
    .collect();
  let chunked = headers
    .iter()
    .any(|(name, value)| name.eq_ignore_ascii_case("transfer-encoding") && value.eq_ignore_ascii_case("chunked"));
  let mut rest = buffer[head_end..].to_vec();
  let body = match chunked {
    true => dechunk(&mut socket, &mut rest).await?,
    false => {
      let mut chunk = [0u8; 4096];
      loop {
        let read = socket.read(&mut chunk).await.map_err(|error| error.to_string())?;
        if read == 0 {
          break rest;
        }
        rest.extend_from_slice(&chunk[..read]);
      }
    }
  };
  Ok(Fetched { status, headers, body })
}

async fn dechunk(socket: &mut AsyncTcpStream, buffer: &mut Vec<u8>) -> Result<Vec<u8>, String> {
  let mut body = Vec::new();
  loop {
    let line_end = loop {
      if let Some(at) = buffer.windows(2).position(|window| window == b"\r\n") {
        break at;
      }
      fill(socket, buffer).await?;
    };
    let size = usize::from_str_radix(String::from_utf8_lossy(&buffer[..line_end]).trim(), 16)
      .map_err(|error| format!("bad chunk size: {error}"))?;
    buffer.drain(..line_end + 2);
    if size == 0 {
      return Ok(body);
    }
    while buffer.len() < size + 2 {
      fill(socket, buffer).await?;
    }
    body.extend_from_slice(&buffer[..size]);
    buffer.drain(..size + 2);
  }
}

async fn fill(socket: &mut AsyncTcpStream, buffer: &mut Vec<u8>) -> Result<(), String> {
  let mut chunk = [0u8; 4096];
  let read = socket.read(&mut chunk).await.map_err(|error| error.to_string())?;
  if read == 0 {
    return Err("the relay closed mid body".to_owned());
  }
  buffer.extend_from_slice(&chunk[..read]);
  Ok(())
}
