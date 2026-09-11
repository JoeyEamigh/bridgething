use std::{sync::Arc, time::Duration};

use bridgething_companion::api::ProviderCredentials;
use bridgething_desktop::{commands, shell::Shell};
use libbridgething::{
  BrowseEntry, LibraryItem,
  client::{LibraryBrowse, PlayUri, SeekTo, SkipPrev},
};
use live::{client_url_for, drive, now_playing_settles};
use support::{Channel, DRIVE_DEADLINE, Daemon, SETTLE, mock_app, shell_config};
use tauri::Manager;
use tokio::sync::mpsc;

#[path = "support/live.rs"]
mod live;
#[path = "support/mod.rs"]
mod support;

const PROVIDER: &str = "subsonic";
const SEEK_TO_MS: u32 = 30_000;

struct Login {
  server_url: String,
  username: String,
  password: String,
}

fn main() {
  let login = match (
    std::env::var("BRIDGETHING_SUBSONIC_LIVE_URL"),
    std::env::var("BRIDGETHING_SUBSONIC_LIVE_USERNAME"),
    std::env::var("BRIDGETHING_SUBSONIC_LIVE_PASSWORD"),
  ) {
    (Ok(server_url), Ok(username), Ok(password)) => Login {
      server_url,
      username,
      password,
    },
    _ => {
      eprintln!(
        "skipped: set BRIDGETHING_SUBSONIC_LIVE_URL, BRIDGETHING_SUBSONIC_LIVE_USERNAME and BRIDGETHING_SUBSONIC_LIVE_PASSWORD to run the subsonic lane"
      );
      return;
    }
  };
  drive(move || lane(login));
}

fn tracks_of(entries: &[BrowseEntry]) -> Vec<libbridgething::Track> {
  entries
    .iter()
    .filter_map(|entry| match entry {
      BrowseEntry::Item(LibraryItem::Track(track)) => Some(track.clone()),
      _ => None,
    })
    .collect()
}

async fn lane(login: Login) {
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

  commands::complete_provider_auth(
    app.state(),
    PROVIDER.into(),
    ProviderCredentials::ServerLogin {
      server_url: login.server_url.clone(),
      username: login.username,
      password: login.password,
    },
  )
  .await
  .expect("the login is accepted");
  let deadline = tokio::time::Instant::now() + DRIVE_DEADLINE;
  loop {
    let providers = commands::providers(app.state()).await.expect("providers answer");
    let subsonic = providers
      .iter()
      .find(|provider| provider.id == PROVIDER)
      .expect("the desktop offers subsonic");
    if subsonic.connected && subsonic.auth_state.kind == bridgething_companion::api::AuthKind::Authenticated {
      break;
    }
    assert!(
      subsonic.auth_state.kind != bridgething_companion::api::AuthKind::Failed,
      "the sign-in failed: {:?}",
      subsonic.auth_state.message
    );
    assert!(tokio::time::Instant::now() < deadline, "the sign-in never settled: {subsonic:?}");
    tokio::time::sleep(Duration::from_millis(250)).await;
  }
  eprintln!("signed in to {}", login.server_url);

  let device_id = commands::connect(app.state(), None)
    .await
    .expect("the daemon accepts a link");
  let client = bridgething_client::Client::connect(&client_url_for(&url))
    .await
    .expect("the client wire answers")
    .with_timeout(SETTLE);
  let deadline = tokio::time::Instant::now() + DRIVE_DEADLINE;
  loop {
    let announced = client
      .capabilities()
      .get()
      .await
      .expect("the capabilities surface answers")
      .capabilities;
    if announced.gateway.is_some() && announced.uri_schemes.iter().any(|claimed| claimed == PROVIDER) {
      break;
    }
    assert!(
      tokio::time::Instant::now() < deadline,
      "the shell never claimed the subsonic scheme; the daemon holds {:?}",
      announced.uri_schemes
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
  }

  let albums = client
    .library()
    .browse(LibraryBrowse {
      node_id: Some("albums".into()),
      limit: 5,
      offset: 0,
      sections: None,
      preview: None,
    })
    .await
    .expect("the album list comes over the client wire")
    .result;
  let album = albums
    .entries
    .iter()
    .find_map(|entry| match entry {
      BrowseEntry::Item(LibraryItem::Album(album)) => Some(album.clone()),
      _ => None,
    })
    .expect("at least one album");
  let tracks = tracks_of(
    &client
      .library()
      .browse(LibraryBrowse {
        node_id: Some(album.id.clone()),
        limit: 50,
        offset: 0,
        sections: None,
        preview: None,
      })
      .await
      .expect("the album's tracks come over the client wire")
      .result
      .entries,
  );
  assert!(tracks.len() >= 2, "the lane needs an album with two tracks: {}", album.name);
  eprintln!("album: {} ({} tracks)", album.name, tracks.len());

  client
    .player()
    .play(PlayUri {
      uri: album.id.clone(),
      context: None,
    })
    .await
    .expect("the daemon takes the album uri");

  let first = tracks[0].name.clone();
  let playing = now_playing_settles(&app, |held| {
    held.is_some_and(|now| {
      now.playback.playing && now.track.as_ref().and_then(|track| track.title.as_deref()) == Some(first.as_str())
    })
  })
  .await
  .expect("the first track plays");
  eprintln!("playing: {:?}", playing.track);
  let duration = playing
    .track
    .as_ref()
    .and_then(|track| track.duration_ms)
    .expect("a catalog track has a duration");
  assert!(duration > 0);

  let advanced = now_playing_settles(&app, |held| held.is_some_and(|now| now.playback.position_ms > 1_000))
    .await
    .expect("the player advances through the file");
  eprintln!("position after decode: {}ms of {duration}ms", advanced.playback.position_ms);

  client.player().pause().await.expect("pause");
  now_playing_settles(&app, |held| held.is_some_and(|now| !now.playback.playing)).await;
  client.player().resume().await.expect("resume");
  now_playing_settles(&app, |held| held.is_some_and(|now| now.playback.playing)).await;

  if duration > u64::from(SEEK_TO_MS) + 5_000 {
    client
      .player()
      .seek_to(SeekTo {
        position_ms: SEEK_TO_MS,
      })
      .await
      .expect("seek");
    let sought = now_playing_settles(&app, |held| {
      held.is_some_and(|now| now.playback.position_ms >= u64::from(SEEK_TO_MS))
    })
    .await
    .expect("the seek lands");
    eprintln!("after seek: {}ms", sought.playback.position_ms);
  }

  let second = tracks[1].name.clone();
  client.player().skip_next().await.expect("skip next");
  now_playing_settles(&app, |held| {
    held.is_some_and(|now| {
      now.playback.playing && now.track.as_ref().and_then(|track| track.title.as_deref()) == Some(second.as_str())
    })
  })
  .await;
  eprintln!("skipped to: {second}");

  client
    .player()
    .skip_prev(SkipPrev { allow_seeking: false })
    .await
    .expect("skip prev");
  now_playing_settles(&app, |held| {
    held.is_some_and(|now| now.track.as_ref().and_then(|track| track.title.as_deref()) == Some(first.as_str()))
  })
  .await;
  eprintln!("back to: {first}");

  commands::disconnect(app.state(), Some(device_id))
    .await
    .expect("the link drops");
  tokio::time::sleep(SETTLE).await;
  now_playing_settles(&app, |held| held.is_some_and(|now| now.playback.playing))
    .await
    .expect("the album outlives the link");
  eprintln!("the car thing leaving left the album playing on this computer");
}
