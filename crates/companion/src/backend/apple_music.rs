use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AmAuthStatus {
  NotDetermined,
  Authorized,
  Denied,
  Restricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AmRepeatMode {
  Off,
  All,
  One,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AmEntry {
  pub item: Option<AmRef>,
  pub title: String,
  pub artist_name: Option<String>,
  pub album_name: Option<String>,
  pub artwork_url: Option<String>,
  pub duration_ms: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AmPlayerSnapshot {
  pub entry: Option<AmEntry>,
  pub playing: bool,
  pub position_ms: u32,
  pub shuffle: bool,
  pub repeat: AmRepeatMode,
  pub can_seek: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AmKind {
  Song,
  Album,
  Playlist,
  Artist,
  Station,
  Songs,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct AmRef {
  pub kind: AmKind,
  pub id: String,
  pub library: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AmDate {
  pub year: i32,
  pub month: u32,
  pub day: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AmItem {
  pub item: AmRef,
  pub title: String,
  pub subtitle: Option<String>,
  pub artist_name: Option<String>,
  pub album_name: Option<String>,
  pub artwork_url: Option<String>,
  pub duration_ms: Option<u32>,
  pub track_count: Option<u32>,
  pub released: Option<AmDate>,
  pub single: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AmPage {
  pub items: Vec<AmItem>,
  pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AmShelf {
  pub id: String,
  pub title: String,
  pub items: Vec<AmItem>,
  pub total: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AmSearchResults {
  pub top: Vec<AmItem>,
  pub songs: Vec<AmItem>,
  pub albums: Vec<AmItem>,
  pub artists: Vec<AmItem>,
  pub playlists: Vec<AmItem>,
  pub stations: Vec<AmItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct AmRelations {
  pub album: Option<AmRef>,
  pub artist: Option<AmRef>,
  pub station: Option<AmRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum AmPlayerCommand {
  Play,
  Pause,
  SkipNext,
  SkipPrev,
  SeekTo { position_ms: u32 },
  SetShuffle { on: bool },
  SetRepeat { mode: AmRepeatMode },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AmLibraryScope {
  Playlists,
  Albums,
  Artists,
  Songs,
}

#[uniffi::export(with_foreign)]
pub trait AppleMusicBackend: Send + Sync {
  fn start(&self, inbox: Arc<AmInbox>);
  fn stop(&self);
  fn snapshot(&self, sink: Arc<AmSnapshotSink>);
  fn auth_status(&self, sink: Arc<AmAuthSink>);
  fn request_authorization(&self, sink: Arc<AmAuthSink>);
  fn can_play_catalog_content(&self, sink: Arc<AmCatalogSink>);
  fn is_other_audio_playing(&self, sink: Arc<AmFlagSink>);
  fn play_context(&self, context: AmRef, start_at: Option<AmRef>, sink: Arc<AmActionSink>);
  fn queue_insert(&self, item: AmRef, next: bool, sink: Arc<AmActionSink>);
  fn command(&self, cmd: AmPlayerCommand, sink: Arc<AmActionSink>);
  fn library(&self, scope: AmLibraryScope, limit: u32, offset: u32, sink: Arc<AmPageSink>);
  fn library_count(&self, scope: AmLibraryScope, sink: Arc<AmCountSink>);
  fn recently_played(&self, limit: u32, offset: u32, sink: Arc<AmPageSink>);
  fn children(&self, item: AmRef, sink: Arc<AmItemsSink>);
  fn recommendations(&self, sink: Arc<AmShelvesSink>);
  fn resolve(&self, item: AmRef, sink: Arc<AmItemSink>);
  fn relations(&self, item: AmRef, sink: Arc<AmRelationsSink>);
  fn search(&self, query: String, limit: u32, offset: u32, sink: Arc<AmSearchSink>);
  fn is_favorite(&self, items: Vec<AmRef>, sink: Arc<AmFavoritesSink>);
  fn add_favorite(&self, item: AmRef, sink: Arc<AmActionSink>);
  fn artwork(&self, url: String, sink: Arc<AmArtworkSink>);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmSignal {
  Player,
  Library,
}

#[derive(uniffi::Object)]
pub struct AmInbox {
  tx: mpsc::UnboundedSender<AmSignal>,
}

impl AmInbox {
  pub fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<AmSignal>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(Self { tx }), rx)
  }
}

#[uniffi::export]
impl AmInbox {
  pub fn on_player_changed(&self) {
    let _ = self.tx.send(AmSignal::Player);
  }

  pub fn on_library_changed(&self) {
    let _ = self.tx.send(AmSignal::Library);
  }
}

macro_rules! am_sink {
  ($name:ident, $out:ty) => {
    #[derive(uniffi::Object)]
    pub struct $name {
      tx: std::sync::Mutex<Option<oneshot::Sender<Result<$out, String>>>>,
    }

    impl $name {
      pub fn channel() -> (Arc<Self>, oneshot::Receiver<Result<$out, String>>) {
        let (tx, rx) = oneshot::channel();
        (
          Arc::new(Self {
            tx: std::sync::Mutex::new(Some(tx)),
          }),
          rx,
        )
      }
    }

    #[uniffi::export]
    impl $name {
      pub fn complete(&self, value: $out) {
        if let Some(tx) = self.tx.lock().unwrap().take() {
          let _ = tx.send(Ok(value));
        }
      }

      pub fn fail(&self, reason: String) {
        if let Some(tx) = self.tx.lock().unwrap().take() {
          let _ = tx.send(Err(reason));
        }
      }
    }
  };
}

am_sink!(AmSnapshotSink, AmPlayerSnapshot);
am_sink!(AmAuthSink, AmAuthStatus);
am_sink!(AmCatalogSink, Option<bool>);
am_sink!(AmFlagSink, bool);
am_sink!(AmPageSink, AmPage);
am_sink!(AmCountSink, u32);
am_sink!(AmItemsSink, Vec<AmItem>);
am_sink!(AmArtworkSink, Vec<u8>);
am_sink!(AmShelvesSink, Vec<AmShelf>);
am_sink!(AmItemSink, AmItem);
am_sink!(AmSearchSink, AmSearchResults);
am_sink!(AmFavoritesSink, Vec<bool>);
am_sink!(AmRelationsSink, AmRelations);

#[derive(uniffi::Object)]
pub struct AmActionSink {
  tx: std::sync::Mutex<Option<oneshot::Sender<Result<(), String>>>>,
}

impl AmActionSink {
  pub fn channel() -> (Arc<Self>, oneshot::Receiver<Result<(), String>>) {
    let (tx, rx) = oneshot::channel();
    (
      Arc::new(Self {
        tx: std::sync::Mutex::new(Some(tx)),
      }),
      rx,
    )
  }
}

#[uniffi::export]
impl AmActionSink {
  pub fn ok(&self) {
    if let Some(tx) = self.tx.lock().unwrap().take() {
      let _ = tx.send(Ok(()));
    }
  }

  pub fn fail(&self, reason: String) {
    if let Some(tx) = self.tx.lock().unwrap().take() {
      let _ = tx.send(Err(reason));
    }
  }
}
