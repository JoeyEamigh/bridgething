mod icy;
mod relay;

use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
  },
  time::{Duration, Instant},
};

use bridgething_gateway::{OutboundLink, OutboundLinkExt};
use bridgething_io::{HttpExecutor, HttpHeader, HttpMethod, HttpRequest, HttpTransport as IoHttpTransport};
use libbridgething::{
  MediaItem, Playback, PlaybackContext, PlaybackState, PlayerError, PlayerState, QueueItem, QueuePosition,
  gateway::{GatewayToBridgePlayerMsgEvent, PlayerErrorReply, QueueSnapshot},
};
use tokio::{
  sync::{mpsc, oneshot},
  task::JoinHandle,
};

use self::{
  icy::IcyMetadata,
  relay::{Feed, OriginBody, Relay, Verdict},
};
use crate::{
  backend::{
    StreamBackend, StreamEvent, StreamMetadata, StreamPresentation, StreamSink, StreamSource, StreamStatus,
    StreamTiming,
  },
  dispatch::{ask, tell},
  provider::{
    ProviderError, ProviderLink,
    art::{ArtCache, ArtResolver},
  },
};

const DEFAULT_HERO_EDGE: u32 = 248;
const DEFAULT_THUMB_EDGE: u32 = 96;
const PROBE_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Presentation {
  pub uri: String,
  pub title: Option<String>,
  pub artist: Option<String>,
  pub artist_uri: Option<String>,
  pub album: Option<String>,
  pub album_uri: Option<String>,
  pub artwork: Option<String>,
  pub duration_ms: Option<u32>,
  pub liked: Option<bool>,
  pub track_number: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
  pub url: String,
  pub presentation: Presentation,
  pub queued: bool,
}

impl QueueEntry {
  pub fn bare(url: &str) -> Self {
    Self {
      url: url.to_owned(),
      presentation: Presentation {
        uri: url.to_owned(),
        ..Presentation::default()
      },
      queued: false,
    }
  }
}

#[derive(Clone)]
struct Program {
  entries: Vec<QueueEntry>,
  index: usize,
  context: Option<PlaybackContext>,
}

struct Current {
  live: bool,
  station: Option<String>,
  status: StreamStatus,
  metadata: StreamMetadata,
  artwork_key: Option<String>,
  icy_title: Option<String>,
  icy_artwork_url: Option<String>,
  timing: StreamTiming,
  timed_at: Instant,
}

impl Current {
  fn starting() -> Self {
    Current {
      live: false,
      station: None,
      status: StreamStatus::Buffering,
      metadata: StreamMetadata::default(),
      artwork_key: None,
      icy_title: None,
      icy_artwork_url: None,
      timing: StreamTiming::default(),
      timed_at: Instant::now(),
    }
  }
}

struct Playing {
  program: Program,
  current: Current,
}

impl Playing {
  fn entry(&self) -> &QueueEntry {
    &self.program.entries[self.program.index]
  }

  fn upcoming(&self) -> &[QueueEntry] {
    &self.program.entries[self.program.index + 1..]
  }
}

#[derive(Default)]
struct Presenter {
  nudge: Option<mpsc::UnboundedSender<()>>,
  shown: Option<StreamPresentation>,
  art: Option<(String, Option<Vec<u8>>)>,
}

struct Core {
  backend: Arc<dyn StreamBackend>,
  source: &'static str,
  http: HttpExecutor,
  art: Arc<ArtCache>,
  resolver: Arc<dyn ArtResolver>,
  app_bundle: Mutex<Option<String>>,
  epoch: AtomicU64,
  state: Mutex<Option<Playing>>,
  link: Mutex<Option<ProviderLink>>,
  edges: Mutex<(u32, u32)>,
  listener: Mutex<Option<JoinHandle<()>>>,
  relay: Mutex<Option<Relay>>,
  presenter: Mutex<Presenter>,
  sent_queue: Mutex<Option<Vec<String>>>,
}

fn host_of(url: &str) -> String {
  url::Url::parse(url)
    .ok()
    .and_then(|parsed| parsed.host_str().map(str::to_owned))
    .unwrap_or_else(|| url.to_owned())
}

fn present(text: &Option<String>) -> Option<String> {
  text.as_ref().filter(|text| !text.trim().is_empty()).cloned()
}

fn title(playing: &Playing) -> String {
  let current = &playing.current;
  let entry = playing.entry();
  present(&current.icy_title)
    .or_else(|| present(&current.metadata.title))
    .or_else(|| present(&entry.presentation.title))
    .or_else(|| present(&current.station))
    .unwrap_or_else(|| host_of(&entry.url))
}

fn artist(playing: &Playing) -> Option<String> {
  present(&playing.current.metadata.artist).or_else(|| present(&playing.entry().presentation.artist))
}

fn album(playing: &Playing) -> Option<String> {
  present(&playing.current.metadata.album).or_else(|| present(&playing.entry().presentation.album))
}

fn artwork(playing: &Playing) -> Option<&str> {
  let current = &playing.current;
  current
    .artwork_key
    .as_deref()
    .or(current.metadata.artwork_url.as_deref())
    .or(current.icy_artwork_url.as_deref())
    .or(playing.entry().presentation.artwork.as_deref())
}

fn queue_item(entry: &QueueEntry, resolver: &dyn ArtResolver, thumb_edge: u32) -> QueueItem {
  let presentation = &entry.presentation;
  QueueItem {
    uri: presentation.uri.clone(),
    title: present(&presentation.title).or_else(|| Some(host_of(&entry.url))),
    artist: presentation.artist.clone(),
    artist_uri: presentation.artist_uri.clone(),
    album: presentation.album.clone(),
    album_uri: presentation.album_uri.clone(),
    artwork_id: presentation
      .artwork
      .as_deref()
      .and_then(|source| resolver.asset_id(source, thumb_edge)),
    duration_ms: presentation.duration_ms,
    persistent_id: None,
    queued: entry.queued,
  }
}

fn player_state(playing: &Playing, resolver: &dyn ArtResolver, hero_edge: u32) -> PlayerState {
  let current = &playing.current;
  let entry = playing.entry();
  let state = match current.status {
    StreamStatus::Buffering | StreamStatus::Playing => PlaybackState::Playing,
    StreamStatus::Paused => PlaybackState::Paused,
    StreamStatus::Ended | StreamStatus::Failed { .. } => PlaybackState::Stopped,
  };
  let position_age_ms = current.timed_at.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
  let many = playing.program.entries.len() > 1;
  PlayerState {
    track: Some(MediaItem {
      uri: Some(entry.presentation.uri.clone()),
      persistent_id: Some(entry.presentation.uri.clone()),
      title: Some(title(playing)),
      artist: artist(playing),
      artist_uri: entry.presentation.artist_uri.clone(),
      album: album(playing),
      album_uri: entry.presentation.album_uri.clone(),
      liked: entry.presentation.liked,
      is_like_supported: entry.presentation.liked.map(|_| true),
      artwork_id: artwork(playing).and_then(|source| resolver.asset_id(source, hero_edge)),
      duration_ms: current
        .timing
        .duration_ms
        .filter(|_| !current.live)
        .or(entry.presentation.duration_ms),
      track_number: entry.presentation.track_number,
      ..Default::default()
    }),
    playback: Playback {
      state,
      position_ms: current.timing.position_ms,
      position_age_ms: Some(position_age_ms),
      set_elapsed_time_available: Some(current.timing.seekable && !current.live),
      queue_index: many.then_some(playing.program.index as u32),
      queue_count: many.then_some(playing.program.entries.len() as u32),
      ..Default::default()
    },
    context: playing.program.context.clone(),
    ..Default::default()
  }
}

impl Core {
  fn outbound(&self) -> Option<Arc<dyn OutboundLink>> {
    self.link.lock().unwrap().as_ref().map(|link| link.outbound.clone())
  }

  fn submit(&self) {
    if let Some(nudge) = self.presenter.lock().unwrap().nudge.as_ref() {
      let _ = nudge.send(());
    }
    let link = self.link.lock().unwrap();
    let Some(link) = link.as_ref() else { return };
    let Some(app_bundle) = self.app_bundle.lock().unwrap().clone() else {
      return;
    };
    let (hero_edge, thumb_edge) = *self.edges.lock().unwrap();
    let state = self.state.lock().unwrap();
    let Some(playing) = state.as_ref() else {
      *self.sent_queue.lock().unwrap() = None;
      link.sink.clear_source(self.source);
      return;
    };
    link.sink.submit_player(
      self.source,
      player_state(playing, self.resolver.as_ref(), hero_edge),
      &app_bundle,
      true,
      false,
    );
    let order: Vec<String> = playing
      .upcoming()
      .iter()
      .map(|entry| entry.presentation.uri.clone())
      .collect();
    let mut sent = self.sent_queue.lock().unwrap();
    if sent.as_ref() == Some(&order) {
      return;
    }
    let items = playing
      .upcoming()
      .iter()
      .map(|entry| queue_item(entry, self.resolver.as_ref(), thumb_edge))
      .collect();
    *sent = Some(order.clone());
    link.sink.submit_queue(self.source, QueueSnapshot { order, items });
  }

  fn start_presenting(&self) -> mpsc::UnboundedReceiver<()> {
    let (nudge, nudged) = mpsc::unbounded_channel();
    *self.presenter.lock().unwrap() = Presenter {
      nudge: Some(nudge),
      shown: None,
      art: None,
    };
    nudged
  }

  fn stop_presenting(&self) {
    *self.presenter.lock().unwrap() = Presenter::default();
  }

  async fn present(&self) {
    let (mut presentation, art) = {
      let state = self.state.lock().unwrap();
      let Some(playing) = state.as_ref() else { return };
      (
        StreamPresentation {
          title: title(playing),
          artist: artist(playing),
          album: album(playing),
          artwork: None,
        },
        artwork(playing).map(|source| self.resolver.url(source)),
      )
    };
    if let Some(source) = art {
      let held = self.presenter.lock().unwrap().art.clone();
      presentation.artwork = match held {
        Some((known, bytes)) if known == source => bytes,
        _ => {
          let bytes = self.art.master(&source).await;
          self.presenter.lock().unwrap().art = Some((source, bytes.clone()));
          bytes
        }
      };
    }
    {
      let mut presenter = self.presenter.lock().unwrap();
      if presenter.nudge.is_none() || presenter.shown.as_ref() == Some(&presentation) {
        return;
      }
      presenter.shown = Some(presentation.clone());
    }
    tell(&self.backend, move |backend| backend.present(presentation)).await;
  }

  fn update(&self, apply: impl FnOnce(&mut Playing)) {
    if let Some(playing) = self.state.lock().unwrap().as_mut() {
      apply(playing);
    }
    self.submit();
  }

  fn clear(&self) {
    *self.state.lock().unwrap() = None;
    self.submit();
  }

  fn on_icy(&self, epoch: u64, metadata: IcyMetadata) {
    if self.epoch.load(Ordering::SeqCst) != epoch {
      return;
    }
    let artwork_url = metadata.artwork_url().map(str::to_owned);
    self.update(|playing| match metadata.title {
      Some(title) => {
        playing.current.icy_title = Some(title);
        playing.current.icy_artwork_url = artwork_url;
      }
      None => {
        if artwork_url.is_some() {
          playing.current.icy_artwork_url = artwork_url;
        }
      }
    });
  }

  async fn on_event(self: &Arc<Self>, event: StreamEvent) -> bool {
    match event {
      StreamEvent::Status(StreamStatus::Ended) => {
        self.on_ended();
        return false;
      }
      StreamEvent::Status(StreamStatus::Failed { reason }) => {
        tracing::warn!(%reason, "the stream failed");
        self.clear();
        self.drop_relay();
        if let Some(outbound) = self.outbound() {
          let _ = outbound
            .event(GatewayToBridgePlayerMsgEvent::ErrorEvent(PlayerErrorReply {
              error: PlayerError::PlayFailed { reason },
            }))
            .await;
        }
        return false;
      }
      StreamEvent::Status(status) => self.update(|playing| playing.current.status = status),
      StreamEvent::Metadata(mut metadata) => {
        let artwork_key = metadata.artwork.take().map(|bytes| self.art.adopt(bytes));
        self.update(|playing| {
          playing.current.metadata = metadata;
          playing.current.artwork_key = artwork_key;
        });
      }
      StreamEvent::Timing(timing) => self.update(|playing| {
        playing.current.timing = timing;
        playing.current.timed_at = Instant::now();
      }),
    }
    true
  }

  fn on_ended(self: &Arc<Self>) {
    self.drop_relay();
    self.stop_presenting();
    let next = {
      let mut state = self.state.lock().unwrap();
      let Some(playing) = state.take() else { return };
      let program = playing.program;
      (program.index + 1 < program.entries.len()).then_some(Program {
        index: program.index + 1,
        ..program
      })
    };
    match next {
      Some(program) => {
        let core = self.clone();
        tokio::spawn(async move { core.start(program).await });
      }
      None => self.submit(),
    }
  }

  fn drop_relay(&self) {
    if let Some(relay) = self.relay.lock().unwrap().take() {
      relay.stop();
    }
  }

  async fn stop_current(&self) {
    self.epoch.fetch_add(1, Ordering::SeqCst);
    self.stop_presenting();
    if let Some(task) = self.listener.lock().unwrap().take() {
      task.abort();
    }
    if self.state.lock().unwrap().is_some() {
      tell(&self.backend, |backend| backend.stop()).await;
      self.clear();
    }
    self.drop_relay();
  }

  async fn start(self: &Arc<Self>, program: Program) {
    self.stop_current().await;
    let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
    let url = program.entries[program.index].url.clone();
    *self.state.lock().unwrap() = Some(Playing {
      program,
      current: Current::starting(),
    });
    self.submit();
    let Some(source) = self.resolve(epoch, url).await else {
      return;
    };
    let (sink, mut rx) = StreamSink::channel();
    let mut nudged = self.start_presenting();
    let core = self.clone();
    let task = tokio::spawn(async move {
      loop {
        tokio::select! {
          event = rx.recv() => match event {
            Some(event) => {
              if !core.on_event(event).await {
                break;
              }
            }
            None => break,
          },
          Some(()) = nudged.recv() => core.present().await,
        }
      }
    });
    if let Some(previous) = self.listener.lock().unwrap().replace(task) {
      previous.abort();
    }
    tell(&self.backend, move |backend| backend.play(source, sink)).await;
    self.submit();
  }

  async fn resolve(self: &Arc<Self>, epoch: u64, url: String) -> Option<StreamSource> {
    let feed = Feed::new();
    let (verdict_tx, verdict_rx) = oneshot::channel();
    let abandoned = Arc::new(AtomicBool::new(false));
    let core = self.clone();
    let body = Box::new(OriginBody::new(
      Arc::clone(&feed),
      verdict_tx,
      Arc::clone(&abandoned),
      Box::new(move |metadata| core.on_icy(epoch, metadata)),
    ));
    let request = HttpRequest {
      method: HttpMethod::Get,
      url: url.clone(),
      headers: vec![HttpHeader {
        name: "Icy-MetaData".into(),
        value: "1".into(),
      }],
      body: Vec::new(),
      timeout_ms: 0,
    };
    let http = self.http.clone();
    let feeding = Arc::clone(&feed);
    let fetch = tokio::spawn(async move {
      let outcome = http.download(request, body).await;
      feeding.finish(outcome.map(|_| ()).map_err(|error| error.to_string()));
    });

    let verdict = tokio::time::timeout(PROBE_DEADLINE, verdict_rx)
      .await
      .ok()
      .and_then(Result::ok)
      .unwrap_or_else(|| {
        abandoned.store(true, Ordering::SeqCst);
        Verdict {
          live: false,
          station: None,
          relay: None,
        }
      });
    if self.epoch.load(Ordering::SeqCst) != epoch {
      abandoned.store(true, Ordering::SeqCst);
      feed.close();
      fetch.abort();
      return None;
    }
    let Verdict { live, station, relay } = verdict;
    self.update(|playing| {
      playing.current.live = live;
      playing.current.station = station.clone();
    });

    let Some(content_type) = relay else {
      fetch.abort();
      return Some(StreamSource { url, live, station });
    };
    let relay = match Relay::bind(Arc::clone(&feed), content_type, fetch).await {
      Ok(relay) => relay,
      Err(error) => {
        tracing::warn!(%error, "the stream relay could not listen; playing the origin directly");
        feed.close();
        return Some(StreamSource { url, live, station });
      }
    };
    let relayed = relay.url.clone();
    let mut held = self.relay.lock().unwrap();
    if self.epoch.load(Ordering::SeqCst) != epoch {
      drop(relay);
      return None;
    }
    *held = Some(relay);
    Some(StreamSource {
      url: relayed,
      live,
      station,
    })
  }
}

pub struct StreamPlayback {
  core: Arc<Core>,
}

impl StreamPlayback {
  pub fn new(
    backend: Arc<dyn StreamBackend>,
    http: Arc<dyn IoHttpTransport>,
    art: Arc<ArtCache>,
    source: &'static str,
    resolver: Arc<dyn ArtResolver>,
  ) -> Self {
    Self {
      core: Arc::new(Core {
        backend,
        source,
        http: HttpExecutor::new(http),
        art,
        resolver,
        app_bundle: Mutex::new(None),
        epoch: AtomicU64::new(0),
        state: Mutex::new(None),
        link: Mutex::new(None),
        edges: Mutex::new((DEFAULT_HERO_EDGE, DEFAULT_THUMB_EDGE)),
        listener: Mutex::new(None),
        relay: Mutex::new(None),
        presenter: Mutex::new(Presenter::default()),
        sent_queue: Mutex::new(None),
      }),
    }
  }

  pub async fn attach(&self, link: ProviderLink) {
    if let Some(bundle) = ask(&self.core.backend, |backend| backend.app_bundle()).await {
      *self.core.app_bundle.lock().unwrap() = Some(bundle);
    }
    *self.core.link.lock().unwrap() = Some(link);
  }

  pub async fn detach(&self) {
    self.core.stop_current().await;
    *self.core.link.lock().unwrap() = None;
  }

  pub fn app_bundles(&self) -> Vec<String> {
    self.core.app_bundle.lock().unwrap().iter().cloned().collect()
  }

  pub async fn play(&self, entries: Vec<QueueEntry>, start: usize, context: Option<PlaybackContext>) {
    if entries.is_empty() {
      self.stop().await;
      return;
    }
    let index = start.min(entries.len() - 1);
    self
      .core
      .start(Program {
        entries,
        index,
        context,
      })
      .await;
  }

  pub async fn enqueue(&self, entries: Vec<QueueEntry>, position: QueuePosition) {
    let idle = {
      let mut state = self.core.state.lock().unwrap();
      match state.as_mut() {
        Some(playing) => {
          let program = &mut playing.program;
          let after_current = program.index + 1;
          let at = match position {
            QueuePosition::Append => program.entries.len(),
            QueuePosition::Next => after_current,
            QueuePosition::Index(slot) => (after_current + slot as usize).min(program.entries.len()),
          };
          let tail = program.entries.split_off(at);
          program.entries.extend(entries);
          program.entries.extend(tail);
          None
        }
        None => Some(entries),
      }
    };
    match idle {
      Some(entries) => self.play(entries, 0, None).await,
      None => self.core.submit(),
    }
  }

  pub async fn pause(&self) {
    if self.has_item() {
      tell(&self.core.backend, |backend| backend.pause()).await;
    }
  }

  pub async fn resume(&self) {
    if self.has_item() {
      tell(&self.core.backend, |backend| backend.resume()).await;
    }
  }

  pub async fn seek_to(&self, position_ms: u32) -> Result<(), ProviderError> {
    let seekable = self
      .core
      .state
      .lock()
      .unwrap()
      .as_ref()
      .is_some_and(|playing| playing.current.timing.seekable && !playing.current.live);
    if !seekable {
      return Err(ProviderError::NotImplemented);
    }
    tell(&self.core.backend, move |backend| backend.seek_to(position_ms)).await;
    Ok(())
  }

  pub async fn stop(&self) {
    self.core.stop_current().await;
  }

  fn program(&self) -> Option<(usize, usize)> {
    self
      .core
      .state
      .lock()
      .unwrap()
      .as_ref()
      .map(|playing| (playing.program.index, playing.program.entries.len()))
  }

  async fn jump(&self, index: usize) {
    let program = {
      let state = self.core.state.lock().unwrap();
      let Some(playing) = state.as_ref() else { return };
      Program {
        index,
        ..playing.program.clone()
      }
    };
    self.core.start(program).await;
  }

  pub async fn skip_next(&self) {
    let Some((index, len)) = self.program() else { return };
    if index + 1 < len {
      self.jump(index + 1).await;
    } else {
      self.stop().await;
    }
  }

  pub async fn skip_prev(&self) {
    let Some((index, _)) = self.program() else { return };
    self.jump(index.saturating_sub(1)).await;
  }

  pub async fn skip_to_index(&self, upcoming: u32) -> Result<(), ProviderError> {
    let Some((index, len)) = self.program() else {
      return Err(ProviderError::Failed("nothing is playing".into()));
    };
    let target = index + 1 + upcoming as usize;
    if target >= len {
      return Err(ProviderError::Failed(format!("index {upcoming} is past the queue")));
    }
    self.jump(target).await;
    Ok(())
  }

  pub fn has_item(&self) -> bool {
    self.core.state.lock().unwrap().is_some()
  }

  pub fn current_uri(&self) -> Option<String> {
    self
      .core
      .state
      .lock()
      .unwrap()
      .as_ref()
      .map(|playing| playing.entry().presentation.uri.clone())
  }

  pub fn set_liked(&self, uri: &str, liked: bool) {
    self.core.update(|playing| {
      for entry in &mut playing.program.entries {
        if entry.presentation.uri == uri {
          entry.presentation.liked = Some(liked);
        }
      }
    });
  }

  pub fn set_art_edges(&self, hero_px: u32, thumb_px: u32) {
    *self.core.edges.lock().unwrap() = (hero_px.max(1), thumb_px.max(1));
    *self.core.sent_queue.lock().unwrap() = None;
    self.core.submit();
  }
}

impl Drop for StreamPlayback {
  fn drop(&mut self) {
    if let Some(task) = self.core.listener.lock().unwrap().take() {
      task.abort();
    }
    self.core.drop_relay();
  }
}
