use std::{
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_desktop::{commands, shell::Shell};
use bridgething_io::{DownloadBody, HttpExecutor, HttpHeader, HttpMethod, HttpRequest, ReqwestTransport};
use libbridgething::client::PlayUri;
use live::{client_url_for, drive, now_playing_settles};
use support::{Channel, DRIVE_DEADLINE, Daemon, SETTLE, mock_app, shell_config};
use tauri::Manager;
use tokio::sync::mpsc;

#[path = "support/live.rs"]
mod live;
#[path = "support/mod.rs"]
mod support;

fn main() {
  let Ok(stream_url) = std::env::var("BRIDGETHING_STREAM_LIVE_URL") else {
    eprintln!("skipped: set BRIDGETHING_STREAM_LIVE_URL to an http(s) media url to run the live stream lane");
    return;
  };
  drive(move || lane(stream_url));
}

async fn lane(stream_url: String) {
  let _ = tracing_subscriber::fmt()
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .with_test_writer()
    .try_init();

  let daemon = Daemon::shared();
  let url = daemon.url();

  let spool = tempfile::tempdir().expect("a scratch directory");
  let (tx, _rx) = mpsc::unbounded_channel();
  let shell =
    Shell::create(shell_config(url.clone(), spool.path()), Arc::new(Channel { tx })).expect("the shell builds");
  shell.start().await;
  let app = mock_app(shell);
  let device_id = commands::connect(app.state(), None)
    .await
    .expect("the daemon accepts a link");

  let client = bridgething_client::Client::connect(&client_url_for(&url))
    .await
    .expect("the client wire answers")
    .with_timeout(SETTLE);
  let scheme = stream_url
    .split_once(':')
    .map(|(head, _)| head.to_ascii_lowercase())
    .expect("the stream url carries a scheme");
  let deadline = tokio::time::Instant::now() + DRIVE_DEADLINE;
  loop {
    let announced = client
      .capabilities()
      .get()
      .await
      .expect("the capabilities surface answers")
      .capabilities;
    if announced.gateway.is_some() && announced.uri_schemes.iter().any(|claimed| claimed == &scheme) {
      break;
    }
    assert!(
      tokio::time::Instant::now() < deadline,
      "the shell never claimed the {scheme} scheme; the daemon holds {:?}",
      announced.uri_schemes
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
  }

  client
    .player()
    .play(PlayUri {
      uri: stream_url.clone(),
      context: None,
    })
    .await
    .expect("the daemon takes the stream url");

  let playing = now_playing_settles(&app, |held| {
    held.is_some_and(|now| now.playback.playing && now.track.is_some())
  })
  .await
  .expect("a playing stream");
  eprintln!("stream playing: {:?}", playing.track);

  let titled = named(&app, &stream_url)
    .await
    .expect("the origin named the station before playback started");
  eprintln!("stream title: {titled}");

  let origin = origin_of(&stream_url).await;
  eprintln!("origin: {origin:?}");
  if origin.metaint {
    let song = now_playing_settles(&app, |held| {
      held
        .and_then(|now| now.track.as_ref().and_then(|track| track.title.clone()))
        .is_some_and(|title| Some(title.as_str()) != origin.station.as_deref() && title != host_of(&stream_url))
    })
    .await
    .and_then(|now| now.track.and_then(|track| track.title))
    .expect("a relayed icy origin hands a per-song title to the wire");
    eprintln!("icy title: {song}");
  } else {
    eprintln!("the origin sends no icy-metaint; skipping the per-song title check");
  }

  let live = now_playing_settles(&app, |held| {
    held.is_some_and(|now| {
      now.playback.position_ms > 0 && now.track.as_ref().is_some_and(|track| track.duration_ms.is_none())
    })
  })
  .await
  .expect("a live stream that reports no duration");
  eprintln!(
    "live timing: position {}ms, duration {:?}",
    live.playback.position_ms,
    live.track.as_ref().and_then(|track| track.duration_ms)
  );

  client.player().pause().await.expect("pause");
  now_playing_settles(&app, |held| held.is_some_and(|now| !now.playback.playing)).await;

  client.player().resume().await.expect("resume");
  now_playing_settles(&app, |held| held.is_some_and(|now| now.playback.playing)).await;

  commands::disconnect(app.state(), Some(device_id))
    .await
    .expect("the link drops");
  tokio::time::sleep(SETTLE).await;
  now_playing_settles(&app, |held| held.is_some_and(|now| now.playback.playing))
    .await
    .expect("the stream outlives the link");
}

#[derive(Debug, Default)]
struct Origin {
  metaint: bool,
  station: Option<String>,
}

struct HeaderPeek {
  seen: Arc<Mutex<Origin>>,
}

impl DownloadBody for HeaderPeek {
  fn on_response(&mut self, _status: u16, headers: &[HttpHeader], _content_length: Option<u64>) -> bool {
    let header = |name: &str| {
      headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.trim().to_owned())
        .filter(|value| !value.is_empty())
    };
    *self.seen.lock().unwrap() = Origin {
      metaint: header("icy-metaint").is_some(),
      station: header("icy-name"),
    };
    false
  }

  fn write(&mut self, _chunk: &[u8]) -> Result<(), String> {
    Ok(())
  }
}

async fn origin_of(stream_url: &str) -> Origin {
  let seen = Arc::new(Mutex::new(Origin::default()));
  let http = HttpExecutor::new(Arc::new(ReqwestTransport::default()));
  let _ = tokio::time::timeout(
    SETTLE,
    http.download(
      HttpRequest {
        method: HttpMethod::Get,
        url: stream_url.to_owned(),
        headers: vec![HttpHeader {
          name: "Icy-MetaData".into(),
          value: "1".into(),
        }],
        body: Vec::new(),
        timeout_ms: SETTLE.as_millis() as u32,
      },
      Box::new(HeaderPeek {
        seen: Arc::clone(&seen),
      }),
    ),
  )
  .await;
  std::mem::take(&mut *seen.lock().unwrap())
}

fn host_of(url: &str) -> String {
  url::Url::parse(url)
    .ok()
    .and_then(|parsed| parsed.host_str().map(str::to_owned))
    .unwrap_or_default()
}

async fn named(app: &tauri::App<tauri::test::MockRuntime>, stream_url: &str) -> Option<String> {
  let host = Some(host_of(stream_url));
  let deadline = tokio::time::Instant::now() + DRIVE_DEADLINE;
  loop {
    let held = commands::now_playing(app.state()).await.expect("now playing answers");
    let title = held.and_then(|now| now.track).and_then(|track| track.title);
    if let Some(title) = title.filter(|title| Some(title.as_str()) != host.as_deref()) {
      return Some(title);
    }
    if tokio::time::Instant::now() >= deadline {
      return None;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
  }
}
