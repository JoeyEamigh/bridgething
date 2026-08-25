#![allow(dead_code)]

use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::backend::{
  MediaArt, MediaArtSink, MediaControl, MediaSessionBackend, MediaSessionInbox, MediaSessionSnapshot, MediaSnapshotSink,
};

#[derive(Default)]
pub struct FakeMediaBackend {
  pub granted: Mutex<bool>,
  pub sessions: Mutex<Vec<MediaSessionSnapshot>>,
  pub art: Mutex<HashMap<(String, String), MediaArt>>,
  pub controls: Mutex<Vec<(String, MediaControl)>>,
  pub inbox: Mutex<Option<Arc<MediaSessionInbox>>>,
}

impl FakeMediaBackend {
  pub fn new() -> Arc<Self> {
    let backend = Self::default();
    *backend.granted.lock().unwrap() = true;
    Arc::new(backend)
  }

  pub fn emit(&self, sessions: Vec<MediaSessionSnapshot>) {
    *self.sessions.lock().unwrap() = sessions;
    if let Some(inbox) = self.inbox.lock().unwrap().clone() {
      inbox.on_sessions_changed();
    }
  }

  pub fn controls(&self) -> Vec<(String, MediaControl)> {
    self.controls.lock().unwrap().clone()
  }

  pub async fn wait_control(&self, wanted: MediaControl) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
      if self.controls().iter().any(|(_, cmd)| *cmd == wanted) {
        return;
      }
      assert!(
        tokio::time::Instant::now() < deadline,
        "no {wanted:?} landed; saw {:?}",
        self.controls()
      );
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  }
}

impl MediaSessionBackend for FakeMediaBackend {
  fn is_access_granted(&self) -> bool {
    *self.granted.lock().unwrap()
  }

  fn start(&self, inbox: Arc<MediaSessionInbox>) {
    *self.inbox.lock().unwrap() = Some(inbox);
  }

  fn stop(&self) {
    *self.inbox.lock().unwrap() = None;
  }

  fn snapshot_all(&self, sink: Arc<MediaSnapshotSink>) {
    sink.complete(self.sessions.lock().unwrap().clone());
  }

  fn control(&self, package: String, cmd: MediaControl) {
    self.controls.lock().unwrap().push((package, cmd));
  }

  fn art(&self, package: String, token: String, sink: Arc<MediaArtSink>) {
    sink.complete(self.art.lock().unwrap().get(&(package, token)).cloned());
  }
}

pub fn playing(title: &str, artist: &str, package: &str) -> MediaSessionSnapshot {
  MediaSessionSnapshot {
    package: package.into(),
    title: Some(title.into()),
    artist: Some(artist.into()),
    album: Some("Album".into()),
    duration_ms: Some(1000),
    position_ms: 250,
    playing: true,
    can_seek: true,
    art_token: None,
    queue: Vec::new(),
    active_queue_id: None,
    shuffle: None,
    repeat: None,
    speed: None,
    position_age_ms: None,
    liked: None,
    like_supported: false,
    queue_title: None,
  }
}
