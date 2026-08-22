use std::{
  collections::{BTreeMap, HashMap, HashSet},
  future::Future,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  thread,
  time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD_PAD_INDIFFERENT};
use bridgething_companion::backend::{
  ImageScaler, MediaArt, MediaArtSink, MediaControl, MediaQueueEntry, MediaRepeatMode, MediaSessionBackend,
  MediaSessionInbox, MediaSessionSnapshot, MediaSnapshotSink,
};
use bridgething_io::{DownloadBody, HttpExecutor, HttpHeader, HttpMethod, HttpRequest, ReqwestTransport};
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use url::Url;
use zbus::{
  Connection, DBusError, MatchRule, Message, MessageStream,
  fdo::{DBusProxy, PropertiesProxy},
  message::Type,
  names::{BusName, InterfaceName},
  proxy::CacheProperties,
  zvariant::{ObjectPath, OwnedValue, Value},
};

use crate::backends::portable::PortableScaler;

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const OBJECT: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const NAMESPACE: &str = "org.mpris.MediaPlayer2";
const BUS: &str = "org.freedesktop.DBus";
const NAME_OWNER_CHANGED: &str = "NameOwnerChanged";
const POSITION: &str = "Position";
const PLAYING: &str = "Playing";
const SETTLE: Duration = Duration::from_millis(150);
const ANSWER: Duration = Duration::from_secs(2);
const RETRY_FLOOR: Duration = Duration::from_millis(500);
const RETRY_CEILING: Duration = Duration::from_secs(30);
const QUEUED: usize = 64;
const MAX_ART_EDGE: u32 = 512;
const ART_JPEG_QUALITY: f32 = 0.6;
const ART_CEILING: usize = 8 * 1024 * 1024;
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
const ABSENT: [&str; 6] = [
  "org.freedesktop.DBus.Error.UnknownInterface",
  "org.freedesktop.DBus.Error.UnknownProperty",
  "org.freedesktop.DBus.Error.UnknownMethod",
  "org.freedesktop.DBus.Error.UnknownObject",
  "org.freedesktop.DBus.Error.NotSupported",
  "org.freedesktop.DBus.Error.InvalidArgs",
];

#[zbus::proxy(
  interface = "org.mpris.MediaPlayer2.Player",
  default_path = "/org/mpris/MediaPlayer2"
)]
trait Player {
  fn play(&self) -> zbus::Result<()>;
  fn pause(&self) -> zbus::Result<()>;
  fn next(&self) -> zbus::Result<()>;
  fn previous(&self) -> zbus::Result<()>;
  fn set_position(&self, track: &ObjectPath<'_>, position: i64) -> zbus::Result<()>;
  #[zbus(property)]
  fn set_shuffle(&self, shuffle: bool) -> zbus::Result<()>;
  #[zbus(property)]
  fn set_loop_status(&self, status: &str) -> zbus::Result<()>;
  #[zbus(property)]
  fn set_rate(&self, rate: f64) -> zbus::Result<()>;
}

#[zbus::proxy(
  interface = "org.mpris.MediaPlayer2.TrackList",
  default_path = "/org/mpris/MediaPlayer2"
)]
trait TrackList {
  fn get_tracks_metadata(&self, ids: &[ObjectPath<'_>]) -> zbus::Result<Vec<HashMap<String, OwnedValue>>>;
  fn go_to(&self, id: &ObjectPath<'_>) -> zbus::Result<()>;
  #[zbus(property)]
  fn tracks(&self) -> zbus::Result<Vec<zbus::zvariant::OwnedObjectPath>>;
}

type Players = Arc<Mutex<BTreeMap<String, Known>>>;
type Owners = HashMap<String, HashSet<String>>;

enum Request {
  Control(String, MediaControl),
  Art(String, String, Arc<MediaArtSink>),
}

struct Held {
  stop: oneshot::Sender<()>,
  requests: mpsc::UnboundedSender<Request>,
  players: Players,
  connected: Arc<AtomicBool>,
}

#[derive(Debug, Clone, PartialEq)]
struct Track {
  path: String,
  art_url: Option<String>,
  entry: MediaQueueEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Listing {
  #[default]
  Unknown,
  Absent,
  Present,
}

enum Listed {
  Tracks(Vec<Track>),
  Absent,
  Refused,
}

enum Landing {
  Done,
  Refused,
  Dropped,
}

#[derive(Debug, Clone, PartialEq)]
struct Known {
  package: String,
  status: String,
  title: Option<String>,
  artist: Option<String>,
  album: Option<String>,
  duration_us: Option<i64>,
  art_url: Option<String>,
  track_id: Option<String>,
  shuffle: Option<bool>,
  repeat: Option<MediaRepeatMode>,
  rate: Option<f64>,
  can_seek: bool,
  position_us: i64,
  stamped: Instant,
  answered: Option<Instant>,
  queue: Vec<Track>,
  listing: Listing,
}

#[derive(Default)]
pub struct MprisMedia {
  held: Mutex<Option<Held>>,
}

impl MediaSessionBackend for MprisMedia {
  fn is_access_granted(&self) -> bool {
    self
      .held
      .lock()
      .unwrap()
      .as_ref()
      .is_some_and(|held| held.connected.load(Ordering::Relaxed))
  }

  fn start(&self, inbox: Arc<MediaSessionInbox>) {
    self.stop();

    let (stop, halted) = oneshot::channel();
    let (requests, inflight) = mpsc::unbounded_channel();
    let players = Players::default();
    let connected = Arc::new(AtomicBool::new(false));
    let watched = Watched {
      players: players.clone(),
      connected: connected.clone(),
    };
    match thread::Builder::new()
      .name("bridgething-media-sessions".to_owned())
      .spawn(move || watch(inbox, halted, inflight, watched))
    {
      Ok(_) => {
        *self.held.lock().unwrap() = Some(Held {
          stop,
          requests,
          players,
          connected,
        });
        tracing::debug!("the mpris watcher was asked to read the players on the session bus");
      }
      Err(error) => tracing::warn!(%error, "the mpris watcher could not be started"),
    }
  }

  fn stop(&self) {
    let Some(held) = self.held.lock().unwrap().take() else {
      return;
    };
    held.connected.store(false, Ordering::Relaxed);
    let _ = held.stop.send(());
    tracing::debug!("the mpris watcher was told to stand down");
  }

  fn snapshot_all(&self, sink: Arc<MediaSnapshotSink>) {
    let now = Instant::now();
    let sessions = match self.held.lock().unwrap().as_ref() {
      Some(held) => held
        .players
        .lock()
        .unwrap()
        .values()
        .filter_map(|known| snapshot(known, now))
        .collect(),
      None => Vec::new(),
    };
    tracing::trace!(count = sessions.len(), "handing the players to the companion core");
    sink.complete(sessions);
  }

  fn control(&self, package: String, cmd: MediaControl) {
    let held = self.held.lock().unwrap();
    let Some(held) = held.as_ref() else {
      return tracing::warn!(package, ?cmd, "a control arrived before the mpris watcher was up");
    };
    if held.requests.send(Request::Control(package, cmd)).is_err() {
      tracing::warn!(?cmd, "the mpris watcher is gone, so the control was dropped");
    }
  }

  fn art(&self, package: String, token: String, sink: Arc<MediaArtSink>) {
    let held = self.held.lock().unwrap();
    let Some(held) = held.as_ref() else {
      tracing::warn!(package, "art was asked for before the mpris watcher was up");
      return sink.complete(None);
    };
    if let Err(error) = held.requests.send(Request::Art(package, token, sink))
      && let Request::Art(_, _, sink) = error.0
    {
      tracing::warn!("the mpris watcher is gone, so the art request was dropped");
      sink.complete(None);
    }
  }
}

struct Watched {
  players: Players,
  connected: Arc<AtomicBool>,
}

fn watch(
  inbox: Arc<MediaSessionInbox>,
  halted: oneshot::Receiver<()>,
  requests: mpsc::UnboundedReceiver<Request>,
  watched: Watched,
) {
  match tokio::runtime::Builder::new_current_thread().enable_all().build() {
    Ok(runtime) => runtime.block_on(observe(inbox, halted, requests, watched)),
    Err(error) => tracing::warn!(%error, "the mpris watcher has no runtime"),
  }
}

async fn observe(
  inbox: Arc<MediaSessionInbox>,
  mut halted: oneshot::Receiver<()>,
  mut requests: mpsc::UnboundedReceiver<Request>,
  watched: Watched,
) {
  let art = HttpExecutor::new(Arc::new(ReqwestTransport::default()));
  let mut backoff = RETRY_FLOOR;
  loop {
    let landing = attach(&inbox, &mut halted, &mut requests, &watched, &art).await;
    watched.connected.store(false, Ordering::Relaxed);
    if !watched.players.lock().unwrap().is_empty() {
      watched.players.lock().unwrap().clear();
      inbox.on_sessions_changed();
    }
    match landing {
      Landing::Done => break,
      Landing::Dropped => backoff = RETRY_FLOOR,
      Landing::Refused => backoff = (backoff * 2).min(RETRY_CEILING),
    }
    tracing::debug!(?backoff, "waiting before reaching for the session bus again");
    if !idle(&mut halted, &mut requests, backoff).await {
      break;
    }
  }
  tracing::debug!("the mpris watcher stopped reading the session bus");
}

async fn idle(
  halted: &mut oneshot::Receiver<()>,
  requests: &mut mpsc::UnboundedReceiver<Request>,
  backoff: Duration,
) -> bool {
  let until = tokio::time::Instant::now() + backoff;
  loop {
    tokio::select! {
      _ = &mut *halted => return false,
      _ = tokio::time::sleep_until(until) => return true,
      request = requests.recv() => match request {
        Some(request) => refuse(request),
        None => return false,
      },
    }
  }
}

fn refuse(request: Request) {
  match request {
    Request::Control(package, cmd) => {
      tracing::warn!(package, ?cmd, "there is no session bus to drive a player on")
    }
    Request::Art(package, _, sink) => {
      tracing::warn!(package, "there is no session bus to read cover art from");
      sink.complete(None);
    }
  }
}

async fn attach(
  inbox: &Arc<MediaSessionInbox>,
  halted: &mut oneshot::Receiver<()>,
  requests: &mut mpsc::UnboundedReceiver<Request>,
  watched: &Watched,
  art: &HttpExecutor,
) -> Landing {
  let connection = match Connection::session().await {
    Ok(connection) => connection,
    Err(error) => {
      tracing::warn!(%error, "there is no session bus to read players from");
      return Landing::Refused;
    }
  };
  let (mut frames, mut names) = match subscribe(&connection).await {
    Ok(streams) => streams,
    Err(error) => {
      tracing::warn!(%error, "the session bus refused a subscription to its players");
      return Landing::Refused;
    }
  };
  watched.connected.store(true, Ordering::Relaxed);
  tracing::info!("the session bus is answering for the players on it");

  let mut owners = Owners::new();
  let mut dirty = HashSet::new();
  let mut retry = !discover(&connection, &mut owners, &mut dirty).await;
  let mut deadline = Some(Instant::now());
  let (swept, mut sweeps) = mpsc::unbounded_channel();
  let mut sweeping = false;

  loop {
    tokio::select! {
      _ = &mut *halted => return Landing::Done,
      request = requests.recv() => {
        let Some(request) = request else { return Landing::Done };
        serve(&connection, &watched.players, art.clone(), request);
      }
      frame = frames.next() => {
        let Some(frame) = frame else { return Landing::Dropped };
        match frame {
          Ok(frame) => if let Some(stirring) = stirred(&owners, &frame) {
            dirty.extend(stirring.iter().cloned());
            arm(&mut deadline);
          },
          Err(error) => tracing::debug!(%error, "a player sent a frame that could not be read"),
        }
      }
      frame = names.next() => {
        let Some(frame) = frame else { return Landing::Dropped };
        match frame {
          Ok(frame) => match moved(&frame) {
            Some((name, Some(unique))) => {
              forget(&mut owners, &name);
              owners.entry(unique).or_default().insert(name.clone());
              tracing::debug!(name, "a player took a place on the bus");
              dirty.insert(name);
              arm(&mut deadline);
            }
            Some((name, None)) => {
              forget(&mut owners, &name);
              dirty.remove(&name);
              if watched.players.lock().unwrap().remove(&name).is_some() {
                tracing::debug!(name, "a player left the bus");
                inbox.on_sessions_changed();
              }
            }
            None => tracing::debug!("the bus announced an owner change in a shape the spec does not describe"),
          },
          Err(error) => tracing::debug!(%error, "the bus sent an owner change that could not be read"),
        }
      }
      reads = sweeps.recv() => {
        sweeping = false;
        if let Some(reads) = reads {
          land(&owners, &watched.players, reads);
          inbox.on_sessions_changed();
        }
        if retry {
          retry = false;
          discover(&connection, &mut owners, &mut dirty).await;
          if !dirty.is_empty() {
            arm(&mut deadline);
          }
        }
      }
      _ = settle(deadline), if !sweeping => {
        deadline = None;
        sweeping = true;
        let names: Vec<String> = dirty.drain().collect();
        let connection = connection.clone();
        let players = watched.players.clone();
        let swept = swept.clone();
        tokio::spawn(async move {
          let _ = swept.send(sweep(&connection, &players, names).await);
        });
      }
    }
  }
}

async fn subscribe(connection: &Connection) -> zbus::Result<(MessageStream, MessageStream)> {
  let frames = MessageStream::for_match_rule(
    MatchRule::builder().msg_type(Type::Signal).path(OBJECT)?.build(),
    connection,
    Some(QUEUED),
  )
  .await?;
  let names = MessageStream::for_match_rule(
    MatchRule::builder()
      .msg_type(Type::Signal)
      .sender(BUS)?
      .interface(BUS)?
      .member(NAME_OWNER_CHANGED)?
      .arg0ns(NAMESPACE)?
      .build(),
    connection,
    Some(QUEUED),
  )
  .await?;
  Ok((frames, names))
}

async fn discover(connection: &Connection, owners: &mut Owners, dirty: &mut HashSet<String>) -> bool {
  let bus = match DBusProxy::new(connection).await {
    Ok(bus) => bus,
    Err(error) => {
      tracing::warn!(%error, "the bus will not say which players are already running");
      return false;
    }
  };
  let Some(names) = call("ListNames", bus.list_names()).await else {
    return false;
  };
  let mut whole = true;
  for name in names {
    let name = name.as_str();
    if !name.starts_with(PREFIX) {
      continue;
    }
    let Ok(bus_name) = BusName::try_from(name) else {
      continue;
    };
    match call("GetNameOwner", bus.get_name_owner(bus_name)).await {
      Some(unique) => {
        tracing::debug!(name, "a player was already on the bus");
        owners
          .entry(unique.as_str().to_owned())
          .or_default()
          .insert(name.to_owned());
        dirty.insert(name.to_owned());
      }
      None => whole = false,
    }
  }
  whole
}

fn arm(deadline: &mut Option<Instant>) {
  deadline.get_or_insert_with(|| Instant::now() + SETTLE);
}

async fn settle(deadline: Option<Instant>) {
  match deadline {
    Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
    None => std::future::pending().await,
  }
}

fn stirred<'a>(owners: &'a Owners, frame: &Message) -> Option<&'a HashSet<String>> {
  let header = frame.header();
  let names = owners.get(header.sender()?.as_str())?;
  tracing::trace!(?names, member = ?header.member(), "a player stirred");
  Some(names)
}

fn forget(owners: &mut Owners, name: &str) {
  owners.retain(|_, names| {
    names.remove(name);
    !names.is_empty()
  });
}

fn owned(owners: &Owners, name: &str) -> bool {
  owners.values().any(|names| names.contains(name))
}

fn moved(frame: &Message) -> Option<(String, Option<String>)> {
  let (name, _, new) = frame.body().deserialize::<(String, String, String)>().ok()?;
  Some((name, (!new.is_empty()).then_some(new)))
}

fn serve(connection: &Connection, players: &Players, art: HttpExecutor, request: Request) {
  let players = players.clone();
  match request {
    Request::Control(package, cmd) => {
      let connection = connection.clone();
      tokio::spawn(async move { drive(&connection, &players, &package, cmd).await });
    }
    Request::Art(package, token, sink) => {
      tokio::spawn(async move { sink.complete(paint(&players, &art, &package, &token).await) });
    }
  }
}

async fn sweep(connection: &Connection, players: &Players, names: Vec<String>) -> Vec<(String, Option<Known>)> {
  let mut reads = Vec::with_capacity(names.len());
  for name in names {
    let was = players.lock().unwrap().get(&name).cloned();
    let read = read(connection, &name, was.as_ref()).await;
    reads.push((name, read));
  }
  reads
}

fn land(owners: &Owners, players: &Players, reads: Vec<(String, Option<Known>)>) {
  let mut held = players.lock().unwrap();
  for (name, read) in reads {
    let Some(mut known) = read else {
      tracing::warn!(name, "a player did not answer for itself, so what it last said stands");
      continue;
    };
    if !owned(owners, &name) {
      continue;
    }
    if held.get(&name).map(|was| was.status.as_str()) != Some(known.status.as_str()) {
      tracing::debug!(name, status = known.status, "a player changed what it is doing");
    }
    known.answered = known.answered.max(held.get(&name).and_then(|was| was.answered));
    held.insert(name, known);
  }
}

async fn read(connection: &Connection, name: &str, was: Option<&Known>) -> Option<Known> {
  let package = package_of(name)?;
  let props = properties(connection, name).await?;
  let stamped = Instant::now();
  let mut known = absorb(package, &props, stamped);
  known.answered = match known.status == PLAYING {
    true => Some(stamped),
    false => was.and_then(|was| was.answered),
  };
  known.listing = was.map_or(Listing::Unknown, |was| was.listing);
  if known.listing != Listing::Absent {
    match listed(connection, name).await {
      Listed::Tracks(queue) => {
        known.listing = Listing::Present;
        known.queue = queue;
      }
      Listed::Absent => known.listing = Listing::Absent,
      Listed::Refused => known.queue = was.map(|was| was.queue.clone()).unwrap_or_default(),
    }
  }
  Some(known)
}

async fn properties(connection: &Connection, name: &str) -> Option<HashMap<String, OwnedValue>> {
  let proxy = PropertiesProxy::builder(connection)
    .destination(name)
    .ok()?
    .path(OBJECT)
    .ok()?
    .build()
    .await
    .ok()?;
  let player = InterfaceName::from_static_str(PLAYER).ok()?;
  let mut props = call("GetAll", proxy.get_all(player.clone())).await?;
  if !props.contains_key(POSITION)
    && let Some(position) = call(POSITION, proxy.get(player, POSITION)).await
  {
    props.insert(POSITION.to_owned(), position);
  }
  Some(props)
}

async fn listed(connection: &Connection, name: &str) -> Listed {
  let Some(proxy) = track_list_proxy(connection, name).await else {
    return Listed::Refused;
  };
  let paths = match tokio::time::timeout(ANSWER, proxy.tracks()).await {
    Ok(Ok(paths)) => paths,
    Ok(Err(error)) if absent(&error) => {
      tracing::trace!(name, "this player keeps no track list");
      return Listed::Absent;
    }
    Ok(Err(error)) => {
      tracing::warn!(name, %error, "this player would not open its track list");
      return Listed::Refused;
    }
    Err(_) => {
      tracing::warn!(name, "this player never answered for its track list");
      return Listed::Refused;
    }
  };
  let ids: Vec<ObjectPath<'_>> = paths.iter().map(|path| path.as_ref()).collect();
  let Some(metadata) = call("GetTracksMetadata", proxy.get_tracks_metadata(&ids)).await else {
    return Listed::Refused;
  };
  Listed::Tracks(
    paths
      .iter()
      .zip(metadata)
      .map(|(path, metadata)| track(path.as_str(), &metadata))
      .collect(),
  )
}

fn absent(error: &zbus::Error) -> bool {
  matches!(error, zbus::Error::FDO(error) if ABSENT.contains(&error.name().as_str()))
}

async fn drive(connection: &Connection, players: &Players, package: &str, cmd: MediaControl) {
  let Some((name, known)) = pick(players, package) else {
    return tracing::warn!(package, ?cmd, "no player on the bus answers to this name");
  };
  let Some(proxy) = player_proxy(connection, &name).await else {
    return;
  };
  seat(players, &name);
  tracing::debug!(name, ?cmd, "driving a player over mpris");

  match cmd {
    MediaControl::Play => {
      let _ = call("Play", proxy.play()).await;
    }
    MediaControl::Pause => {
      let _ = call("Pause", proxy.pause()).await;
    }
    MediaControl::SkipNext => {
      let _ = call("Next", proxy.next()).await;
    }
    MediaControl::SkipPrev => {
      let _ = call("Previous", proxy.previous()).await;
    }
    MediaControl::SeekTo { position_ms } => {
      match known.track_id.as_deref().and_then(|id| ObjectPath::try_from(id).ok()) {
        Some(track) => {
          let _ = call("SetPosition", proxy.set_position(&track, position_ms.max(0) * 1000)).await;
        }
        None => tracing::warn!(
          name,
          "this player does not name the track it is on, so it cannot be seeked"
        ),
      }
    }
    MediaControl::SkipToQueueItem { queue_id } => skip(connection, &name, &known, queue_id).await,
    MediaControl::SetShuffle { on } => {
      let _ = call("Shuffle", proxy.set_shuffle(on)).await;
    }
    MediaControl::SetRepeat { mode } => {
      let _ = call("LoopStatus", proxy.set_loop_status(loop_status(mode))).await;
    }
    MediaControl::SetSpeed { speed } => {
      let _ = call("Rate", proxy.set_rate(f64::from(speed))).await;
    }
    MediaControl::SetLiked { .. } => {
      tracing::debug!(name, "mpris carries no rating, so a like cannot be pushed to a player")
    }
  }
}

async fn player_proxy(connection: &Connection, name: &str) -> Option<PlayerProxy<'static>> {
  let built = PlayerProxy::builder(connection)
    .destination(name.to_owned())
    .ok()?
    .path(OBJECT)
    .ok()?
    .cache_properties(CacheProperties::No)
    .build()
    .await;
  match built {
    Ok(proxy) => Some(proxy),
    Err(error) => {
      tracing::warn!(name, %error, "this player cannot be driven");
      None
    }
  }
}

async fn track_list_proxy(connection: &Connection, name: &str) -> Option<TrackListProxy<'static>> {
  let built = TrackListProxy::builder(connection)
    .destination(name.to_owned())
    .ok()?
    .path(OBJECT)
    .ok()?
    .cache_properties(CacheProperties::No)
    .build()
    .await;
  match built {
    Ok(proxy) => Some(proxy),
    Err(error) => {
      tracing::warn!(name, %error, "this player will not open its track list");
      None
    }
  }
}

async fn skip(connection: &Connection, name: &str, known: &Known, queue_id: i64) {
  let Some(track) = known.queue.iter().find(|track| track.entry.queue_id == queue_id) else {
    return tracing::warn!(name, queue_id, "this player does not hold the track that was asked for");
  };
  let Ok(path) = ObjectPath::try_from(track.path.as_str()) else {
    return tracing::warn!(
      name,
      track = track.path,
      "this player named a track the bus cannot address"
    );
  };
  let Some(proxy) = track_list_proxy(connection, name).await else {
    return;
  };
  let _ = call("GoTo", proxy.go_to(&path)).await;
}

async fn paint(players: &Players, art: &HttpExecutor, package: &str, token: &str) -> Option<MediaArt> {
  let (name, known) = pick(players, package)?;
  let Some(url) = advertised(&known, token) else {
    tracing::warn!(name, "a player was asked for art it never advertised");
    return None;
  };
  let bytes = fetch(art, &url).await?;
  tracing::debug!(name, bytes = bytes.len(), "a player handed over cover art");
  let scaled =
    tokio::task::spawn_blocking(move || PortableScaler.downsample_jpeg(bytes, MAX_ART_EDGE, ART_JPEG_QUALITY))
      .await
      .ok()
      .flatten();
  match scaled {
    Some(bytes) => Some(MediaArt {
      bytes,
      mime: "image/jpeg".to_owned(),
    }),
    None => {
      tracing::warn!(name, "cover art in a shape this desktop cannot re-encode");
      None
    }
  }
}

fn advertised(known: &Known, token: &str) -> Option<String> {
  known
    .art_url
    .iter()
    .chain(known.queue.iter().filter_map(|track| track.art_url.as_ref()))
    .find(|url| art_token(url.as_str()) == token)
    .cloned()
}

async fn fetch(art: &HttpExecutor, url: &str) -> Option<Vec<u8>> {
  if let Some(inline) = url.strip_prefix("data:") {
    return inline_art(inline);
  }
  let parsed = match Url::parse(url) {
    Ok(parsed) => parsed,
    Err(error) => {
      tracing::warn!(%error, "a player named its cover art with something that is not a url");
      return None;
    }
  };
  match parsed.scheme() {
    "file" => on_disk(&parsed).await,
    "http" | "https" => downloaded(art, parsed.as_str()).await,
    scheme => {
      tracing::warn!(scheme, "cover art in a scheme this desktop cannot read");
      None
    }
  }
}

async fn on_disk(url: &Url) -> Option<Vec<u8>> {
  let path = url.to_file_path().ok()?;
  let size = match tokio::fs::metadata(&path).await {
    Ok(found) => found.len(),
    Err(error) => {
      tracing::warn!(%error, path = %path.display(), "cover art on disk could not be measured");
      return None;
    }
  };
  if size > ART_CEILING as u64 {
    tracing::warn!(size, path = %path.display(), "cover art on disk is bigger than this desktop will hold");
    return None;
  }
  match tokio::fs::read(&path).await {
    Ok(bytes) => Some(bytes),
    Err(error) => {
      tracing::warn!(%error, path = %path.display(), "cover art on disk could not be read");
      None
    }
  }
}

async fn downloaded(art: &HttpExecutor, url: &str) -> Option<Vec<u8>> {
  let bytes = Arc::new(Mutex::new(Vec::new()));
  let outcome = art
    .download(
      HttpRequest {
        method: HttpMethod::Get,
        url: url.to_owned(),
        headers: Vec::new(),
        body: Vec::new(),
        timeout_ms: 0,
      },
      Box::new(Capped {
        bytes: bytes.clone(),
        ceiling: ART_CEILING,
      }),
    )
    .await;
  match outcome {
    Ok(outcome) if outcome.ok() && outcome.received > 0 => Some(std::mem::take(&mut bytes.lock().unwrap())),
    Ok(outcome) => {
      tracing::warn!(url, status = outcome.status, "cover art behind a url did not arrive");
      None
    }
    Err(error) => {
      tracing::warn!(url, %error, "cover art behind a url could not be fetched");
      None
    }
  }
}

struct Capped {
  bytes: Arc<Mutex<Vec<u8>>>,
  ceiling: usize,
}

impl DownloadBody for Capped {
  fn on_response(&mut self, status: u16, _headers: &[HttpHeader], content_length: Option<u64>) -> bool {
    (200..300).contains(&status) && content_length.is_none_or(|length| length <= self.ceiling as u64)
  }

  fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
    let mut bytes = self.bytes.lock().unwrap();
    if bytes.len() + chunk.len() > self.ceiling {
      return Err("cover art is bigger than this desktop will hold".to_owned());
    }
    bytes.extend_from_slice(chunk);
    Ok(())
  }
}

fn inline_art(inline: &str) -> Option<Vec<u8>> {
  let (meta, payload) = inline.split_once(',')?;
  if meta.rsplit(';').next() != Some("base64") {
    tracing::warn!("a player inlined cover art in a shape this desktop cannot read");
    return None;
  }
  let size = payload.len() / 4 * 3;
  if size > ART_CEILING {
    tracing::warn!(size, "inlined cover art is bigger than this desktop will hold");
    return None;
  }
  match STANDARD_PAD_INDIFFERENT.decode(payload) {
    Ok(bytes) => Some(bytes),
    Err(error) => {
      tracing::warn!(%error, "a player inlined cover art that is not base64");
      None
    }
  }
}

fn pick(players: &Players, package: &str) -> Option<(String, Known)> {
  let held = players.lock().unwrap();
  let matched = held.iter().filter(|(_, known)| known.package == package);
  matched
    .clone()
    .find(|(_, known)| known.status == PLAYING)
    .or_else(|| matched.max_by_key(|(_, known)| known.answered))
    .map(|(name, known)| (name.clone(), known.clone()))
}

fn seat(players: &Players, name: &str) {
  if let Some(known) = players.lock().unwrap().get_mut(name) {
    known.answered = Some(Instant::now());
  }
}

async fn call<T, E, F>(what: &str, task: F) -> Option<T>
where
  F: Future<Output = Result<T, E>>,
  E: std::fmt::Display,
{
  match tokio::time::timeout(ANSWER, task).await {
    Ok(Ok(value)) => Some(value),
    Ok(Err(error)) => {
      tracing::warn!(what, %error, "a call on the session bus was refused");
      None
    }
    Err(_) => {
      tracing::warn!(what, "a call on the session bus was never answered");
      None
    }
  }
}

fn package_of(bus: &str) -> Option<String> {
  let suffix = bus.strip_prefix(PREFIX)?;
  let base = match suffix.split_once(".instance") {
    Some((head, tail)) if !head.is_empty() && counted(tail) => head,
    _ => suffix,
  };
  (!base.is_empty()).then(|| base.to_owned())
}

fn counted(tail: &str) -> bool {
  let digits = tail.strip_prefix('_').unwrap_or(tail);
  !digits.is_empty() && digits.bytes().all(|digit| digit.is_ascii_digit())
}

fn absorb(package: String, props: &HashMap<String, OwnedValue>, stamped: Instant) -> Known {
  let metadata = props.get("Metadata").and_then(dict).unwrap_or_default();
  Known {
    package,
    status: props.get("PlaybackStatus").and_then(text).unwrap_or_default(),
    title: metadata.get("xesam:title").and_then(text),
    artist: metadata
      .get("xesam:artist")
      .and_then(text)
      .or_else(|| metadata.get("xesam:albumArtist").and_then(text)),
    album: metadata.get("xesam:album").and_then(text),
    duration_us: metadata.get("mpris:length").and_then(number).filter(|us| *us > 0),
    art_url: metadata.get("mpris:artUrl").and_then(text),
    track_id: metadata.get("mpris:trackid").and_then(text).filter(|id| id != NO_TRACK),
    shuffle: props.get("Shuffle").and_then(flag),
    repeat: props.get("LoopStatus").and_then(text).as_deref().and_then(repeat_of),
    rate: props.get("Rate").and_then(real),
    can_seek: props.get("CanSeek").and_then(flag).unwrap_or(false),
    position_us: props.get(POSITION).and_then(number).unwrap_or_default().max(0),
    stamped,
    answered: None,
    queue: Vec::new(),
    listing: Listing::Unknown,
  }
}

fn track(path: &str, metadata: &HashMap<String, OwnedValue>) -> Track {
  let art_url = metadata.get("mpris:artUrl").and_then(text);
  Track {
    path: path.to_owned(),
    entry: MediaQueueEntry {
      queue_id: digest(path),
      title: metadata.get("xesam:title").and_then(text),
      subtitle: metadata.get("xesam:artist").and_then(text),
      art_token: art_url.as_deref().map(art_token),
    },
    art_url,
  }
}

fn snapshot(known: &Known, now: Instant) -> Option<MediaSessionSnapshot> {
  if known.title.is_none() && known.artist.is_none() && known.album.is_none() {
    return None;
  }
  let playing = known.status == PLAYING;
  Some(MediaSessionSnapshot {
    package: known.package.clone(),
    title: known.title.clone(),
    artist: known.artist.clone(),
    album: known.album.clone(),
    duration_ms: known.duration_us.map(|us| us / 1000),
    position_ms: known.position_us / 1000,
    playing,
    can_seek: known.can_seek,
    art_token: known.art_url.as_deref().map(art_token),
    queue: known.queue.iter().map(|track| track.entry.clone()).collect(),
    active_queue_id: known
      .track_id
      .as_deref()
      .filter(|_| !known.queue.is_empty())
      .map(digest),
    shuffle: known.shuffle,
    repeat: known.repeat,
    speed: known.rate.filter(|rate| playing && *rate > 0.0).map(|rate| rate as f32),
    position_age_ms: playing.then(|| now.saturating_duration_since(known.stamped).as_millis() as i64),
    liked: None,
    like_supported: false,
    queue_title: None,
  })
}

fn digest(text: &str) -> i64 {
  text
    .bytes()
    .fold(0i64, |id, byte| id.wrapping_mul(31).wrapping_add(i64::from(byte)))
}

fn art_token(url: &str) -> String {
  format!("u{:016x}", digest(url) as u64)
}

fn repeat_of(status: &str) -> Option<MediaRepeatMode> {
  match status {
    "None" => Some(MediaRepeatMode::Off),
    "Track" => Some(MediaRepeatMode::One),
    "Playlist" => Some(MediaRepeatMode::All),
    _ => None,
  }
}

fn loop_status(mode: MediaRepeatMode) -> &'static str {
  match mode {
    MediaRepeatMode::Off => "None",
    MediaRepeatMode::One => "Track",
    MediaRepeatMode::All => "Playlist",
  }
}

fn plain<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
  match value {
    Value::Value(held) => plain(held),
    held => held,
  }
}

fn dict(value: &OwnedValue) -> Option<HashMap<String, OwnedValue>> {
  match plain(value) {
    Value::Dict(held) => held.try_clone().ok().and_then(|held| HashMap::try_from(held).ok()),
    _ => None,
  }
}

fn text(value: &OwnedValue) -> Option<String> {
  let joined = match plain(value) {
    Value::Str(held) => held.to_string(),
    Value::ObjectPath(held) => held.to_string(),
    Value::Array(held) => held
      .iter()
      .filter_map(|held| match plain(held) {
        Value::Str(held) => Some(held.to_string()),
        _ => None,
      })
      .collect::<Vec<_>>()
      .join(", "),
    _ => return None,
  };
  (!joined.is_empty()).then_some(joined)
}

fn number(value: &OwnedValue) -> Option<i64> {
  match plain(value) {
    Value::I64(held) => Some(*held),
    Value::U64(held) => i64::try_from(*held).ok(),
    Value::I32(held) => Some(i64::from(*held)),
    Value::U32(held) => Some(i64::from(*held)),
    Value::I16(held) => Some(i64::from(*held)),
    Value::U16(held) => Some(i64::from(*held)),
    Value::U8(held) => Some(i64::from(*held)),
    Value::F64(held) => Some(*held as i64),
    _ => None,
  }
}

fn real(value: &OwnedValue) -> Option<f64> {
  match plain(value) {
    Value::F64(held) => Some(*held),
    Value::I64(held) => Some(*held as f64),
    Value::I32(held) => Some(f64::from(*held)),
    Value::U32(held) => Some(f64::from(*held)),
    _ => None,
  }
}

fn flag(value: &OwnedValue) -> Option<bool> {
  match plain(value) {
    Value::Bool(held) => Some(*held),
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use tokio::sync::mpsc::UnboundedReceiver;

  use super::*;

  const FAKE: &str = "org.mpris.MediaPlayer2.fake";
  const FAKE_PACKAGE: &str = "fake";
  const LATER: &str = "org.mpris.MediaPlayer2.later.instance7";
  const LATER_PACKAGE: &str = "later";
  const ARMED: Duration = Duration::from_secs(10);
  const TICK: Duration = Duration::from_millis(250);
  const TRACK: &str = "/org/mpris/MediaPlayer2/Track/1";
  const COVER: &str = "https://i.scdn.co/image/cover";
  const LIVE: &str = "BRIDGETHING_MPRIS_LIVE";
  const VLC_PACKAGE: &str = "vlc";
  const VLC_BUS: &str = "org.mpris.MediaPlayer2.vlc";
  const MPV_PACKAGE: &str = "mpv";
  const MPV_BUS: &str = "org.mpris.MediaPlayer2.mpv";
  const STAGING: &str = concat!(
    "set BRIDGETHING_MPRIS_LIVE=1 and run this test on a session bus that already carries two players: ",
    "cvlc --intf dummy --control dbus --aout dummy --vout dummy --no-video --loop one.mp3 two.mp3 three.mp3, and ",
    "mpv --no-video --ao=null --loop-playlist --script=<libdir>/mpv/mpris.so four.mp3, where the three vlc files ",
    "carry tags and embedded cover art and four.mp3 sits in a directory with no cover image beside it"
  );

  fn held(value: impl Into<Value<'static>>) -> OwnedValue {
    OwnedValue::try_from(value.into()).expect("a value the bus could carry")
  }

  fn metadata() -> HashMap<String, OwnedValue> {
    HashMap::from([
      (
        "mpris:trackid".to_owned(),
        held(ObjectPath::try_from(TRACK).expect("a track path")),
      ),
      ("mpris:length".to_owned(), held(213_000_000i64)),
      ("mpris:artUrl".to_owned(), held(COVER)),
      ("xesam:title".to_owned(), held("Ceremony")),
      (
        "xesam:artist".to_owned(),
        held(vec!["New Order".to_owned(), "Joy Division".to_owned()]),
      ),
      ("xesam:album".to_owned(), held("Substance")),
    ])
  }

  fn props(status: &str) -> HashMap<String, OwnedValue> {
    HashMap::from([
      ("PlaybackStatus".to_owned(), held(status.to_owned())),
      ("Metadata".to_owned(), OwnedValue::from(metadata())),
      (POSITION.to_owned(), held(42_000_000i64)),
      ("Shuffle".to_owned(), held(true)),
      ("LoopStatus".to_owned(), held("Playlist".to_owned())),
      ("Rate".to_owned(), held(1.25f64)),
      ("CanSeek".to_owned(), held(true)),
    ])
  }

  #[test]
  fn what_a_player_publishes_becomes_the_snapshot_the_core_reads() {
    let now = Instant::now();
    let known = absorb("spotify".to_owned(), &props(PLAYING), now - Duration::from_millis(400));
    let snapshot = snapshot(&known, now).expect("a player with a track loaded is a session");

    assert_eq!(snapshot.package, "spotify");
    assert_eq!(snapshot.title.as_deref(), Some("Ceremony"));
    assert_eq!(
      snapshot.artist.as_deref(),
      Some("New Order, Joy Division"),
      "every artist on the track belongs on the one line the device shows"
    );
    assert_eq!(snapshot.album.as_deref(), Some("Substance"));
    assert_eq!(snapshot.duration_ms, Some(213_000));
    assert_eq!(snapshot.position_ms, 42_000);
    assert!(snapshot.playing && snapshot.can_seek);
    assert_eq!(
      snapshot.art_token.as_deref(),
      Some(art_token(COVER).as_str()),
      "the url a player names is digested so a local path never crosses the link"
    );
    assert!(
      !snapshot.art_token.as_deref().unwrap_or_default().contains(COVER),
      "the raw url is not the token"
    );
    assert_eq!(snapshot.shuffle, Some(true));
    assert_eq!(snapshot.repeat, Some(MediaRepeatMode::All));
    assert_eq!(snapshot.speed, Some(1.25));
    assert_eq!(
      snapshot.position_age_ms,
      Some(400),
      "a position read 400ms ago is 400ms stale"
    );
    assert!(snapshot.liked.is_none() && !snapshot.like_supported);
    assert!(
      snapshot.queue.is_empty() && snapshot.active_queue_id.is_none(),
      "a player with no track list has nothing queued"
    );
  }

  #[test]
  fn a_paused_player_ages_nothing_and_runs_at_no_speed() {
    let now = Instant::now();
    let known = absorb("vlc".to_owned(), &props("Paused"), now - Duration::from_millis(400));
    let snapshot = snapshot(&known, now).expect("a paused player still holds a track");

    assert!(!snapshot.playing);
    assert_eq!(snapshot.position_ms, 42_000);
    assert_eq!(
      snapshot.position_age_ms, None,
      "a position that is not moving does not go stale"
    );
    assert_eq!(snapshot.speed, None);
  }

  #[test]
  fn a_player_with_nothing_loaded_is_not_a_session() {
    let props = HashMap::from([("PlaybackStatus".to_owned(), held("Stopped".to_owned()))]);
    let known = absorb("firefox".to_owned(), &props, Instant::now());

    assert!(
      snapshot(&known, Instant::now()).is_none(),
      "a player holding no title, artist or album has nothing the device could show"
    );
  }

  #[test]
  fn a_player_that_says_nothing_about_shuffle_or_repeat_leaves_them_unanswered() {
    let props = HashMap::from([
      ("PlaybackStatus".to_owned(), held(PLAYING.to_owned())),
      ("Metadata".to_owned(), OwnedValue::from(metadata())),
    ]);
    let known = absorb("chromium".to_owned(), &props, Instant::now());
    let snapshot = snapshot(&known, Instant::now()).expect("a browser tab with a track is a session");

    assert_eq!(snapshot.shuffle, None);
    assert_eq!(snapshot.repeat, None);
    assert_eq!(snapshot.speed, None);
    assert!(!snapshot.can_seek);
    assert_eq!(snapshot.position_ms, 0);
  }

  #[test]
  fn a_track_list_becomes_the_queue_the_device_scrolls() {
    let now = Instant::now();
    let mut known = absorb("vlc".to_owned(), &props(PLAYING), now);
    known.queue = vec![
      track(TRACK, &metadata()),
      track(
        "/org/mpris/MediaPlayer2/Track/2",
        &HashMap::from([
          ("xesam:title".to_owned(), held("Temptation")),
          ("xesam:artist".to_owned(), held(vec!["New Order".to_owned()])),
        ]),
      ),
    ];
    let snapshot = snapshot(&known, now).expect("a player with a track loaded is a session");

    assert_eq!(snapshot.queue.len(), 2);
    assert_eq!(snapshot.queue[1].title.as_deref(), Some("Temptation"));
    assert_eq!(snapshot.queue[1].subtitle.as_deref(), Some("New Order"));
    assert!(snapshot.queue[1].art_token.is_none());
    assert_eq!(
      snapshot.active_queue_id,
      Some(snapshot.queue[0].queue_id),
      "the track the player names is the one the queue is sitting on"
    );
  }

  #[test]
  fn the_bus_name_a_player_owns_is_the_name_the_device_knows_it_by() {
    assert_eq!(package_of("org.mpris.MediaPlayer2.spotify").as_deref(), Some("spotify"));
    assert_eq!(package_of("org.mpris.MediaPlayer2.vlc").as_deref(), Some("vlc"));
    assert_eq!(
      package_of("org.mpris.MediaPlayer2.chromium.instance12345").as_deref(),
      Some("chromium"),
      "a browser that restarts must not read as a different app"
    );
    assert_eq!(
      package_of("org.mpris.MediaPlayer2.firefox.instance_1729").as_deref(),
      Some("firefox")
    );
    assert_eq!(
      package_of("org.mpris.MediaPlayer2.kdeconnect.mpris_1").as_deref(),
      Some("kdeconnect.mpris_1"),
      "only a pid tail is a tail worth dropping"
    );
    assert_eq!(package_of("org.freedesktop.Notifications"), None);
    assert_eq!(package_of("org.mpris.MediaPlayer2."), None);
  }

  #[test]
  fn the_loop_a_player_reports_is_the_repeat_the_device_shows() {
    assert_eq!(repeat_of("None"), Some(MediaRepeatMode::Off));
    assert_eq!(repeat_of("Track"), Some(MediaRepeatMode::One));
    assert_eq!(repeat_of("Playlist"), Some(MediaRepeatMode::All));
    assert_eq!(
      repeat_of("Album"),
      None,
      "a loop mode the spec does not name is one the device cannot show"
    );

    for mode in [MediaRepeatMode::Off, MediaRepeatMode::One, MediaRepeatMode::All] {
      assert_eq!(
        repeat_of(loop_status(mode)),
        Some(mode),
        "what the device asks for is what it reads back"
      );
    }
  }

  #[test]
  fn cover_art_a_player_inlines_is_decoded_where_it_stands() {
    assert_eq!(
      inline_art("image/jpeg;base64,YWJj").as_deref(),
      Some(b"abc".as_slice()),
      "base64 art rides inline"
    );
    assert_eq!(
      inline_art("image/svg+xml,<svg/>").as_deref(),
      None,
      "a payload that is not base64 is percent encoded text no decoder here reads"
    );
    assert_eq!(inline_art("image/jpeg;base64,not base64").as_deref(), None);
    assert_eq!(inline_art("image/jpeg").as_deref(), None);
  }

  #[test]
  fn an_interface_a_player_never_exported_is_one_it_is_never_asked_for_twice() {
    for refusal in [
      zbus::fdo::Error::UnknownInterface("no such interface".to_owned()),
      zbus::fdo::Error::UnknownProperty("no such property".to_owned()),
      zbus::fdo::Error::UnknownMethod("no such method".to_owned()),
      zbus::fdo::Error::UnknownObject("no such object".to_owned()),
      zbus::fdo::Error::NotSupported("not supported".to_owned()),
      zbus::fdo::Error::InvalidArgs("No such interface “org.mpris.MediaPlayer2.TrackList”".to_owned()),
    ] {
      assert!(
        absent(&zbus::Error::from(refusal.clone())),
        "{refusal} is a player saying it keeps no track list"
      );
    }

    for refusal in [
      zbus::fdo::Error::AccessDenied("not for you".to_owned()),
      zbus::fdo::Error::NoReply("nothing came back".to_owned()),
      zbus::fdo::Error::Failed("something went wrong".to_owned()),
    ] {
      assert!(
        !absent(&zbus::Error::from(refusal.clone())),
        "{refusal} is a player refusing once, not a player without a track list"
      );
    }
  }

  fn seated(package: &str, status: &str, answered: Option<Instant>) -> Known {
    let mut known = absorb(package.to_owned(), &props(status), Instant::now());
    known.answered = answered;
    known
  }

  #[test]
  fn a_control_lands_on_the_instance_that_last_answered_for_the_package() {
    let now = Instant::now();
    let players = Players::default();
    players.lock().unwrap().extend([
      (
        "org.mpris.MediaPlayer2.chromium.instance1".to_owned(),
        seated("chromium", "Paused", Some(now - Duration::from_secs(30))),
      ),
      (
        "org.mpris.MediaPlayer2.chromium.instance2".to_owned(),
        seated("chromium", "Paused", Some(now - Duration::from_secs(1))),
      ),
    ]);

    let (name, _) = pick(&players, "chromium").expect("one of the two instances answers");
    assert_eq!(
      name, "org.mpris.MediaPlayer2.chromium.instance2",
      "the instance that was playing most recently is the one a resume belongs to"
    );

    players.lock().unwrap().insert(
      "org.mpris.MediaPlayer2.chromium.instance1".to_owned(),
      seated("chromium", PLAYING, Some(now - Duration::from_secs(30))),
    );
    let (name, _) = pick(&players, "chromium").expect("one of the two instances answers");
    assert_eq!(
      name, "org.mpris.MediaPlayer2.chromium.instance1",
      "a playing instance outranks whichever one answered last"
    );
  }

  #[test]
  fn reading_a_player_for_its_art_does_not_move_where_a_control_lands() {
    let now = Instant::now();
    let players = Players::default();
    players.lock().unwrap().extend([
      (
        "org.mpris.MediaPlayer2.chromium.instance1".to_owned(),
        seated("chromium", "Paused", Some(now - Duration::from_secs(30))),
      ),
      (
        "org.mpris.MediaPlayer2.chromium.instance2".to_owned(),
        seated("chromium", "Paused", Some(now - Duration::from_secs(1))),
      ),
    ]);

    let (looked, _) = pick(&players, "chromium").expect("one of the two instances answers");
    assert_eq!(looked, "org.mpris.MediaPlayer2.chromium.instance2");
    assert_eq!(
      players
        .lock()
        .unwrap()
        .get("org.mpris.MediaPlayer2.chromium.instance2")
        .and_then(|known| known.answered),
      Some(now - Duration::from_secs(1)),
      "looking a player up to pull its art is not the player answering"
    );

    seat(&players, "org.mpris.MediaPlayer2.chromium.instance1");
    let (driven, _) = pick(&players, "chromium").expect("one of the two instances answers");
    assert_eq!(
      driven, "org.mpris.MediaPlayer2.chromium.instance1",
      "the instance a control was issued on is the one the next control belongs to"
    );
  }

  #[test]
  fn a_read_that_began_before_a_control_does_not_unseat_the_player_it_was_driven_on() {
    let now = Instant::now();
    let players = Players::default();
    let stale = seated("chromium", "Paused", Some(now - Duration::from_secs(30)));
    players.lock().unwrap().extend([
      ("org.mpris.MediaPlayer2.chromium.instance1".to_owned(), stale.clone()),
      (
        "org.mpris.MediaPlayer2.chromium.instance2".to_owned(),
        seated("chromium", "Paused", Some(now - Duration::from_secs(1))),
      ),
    ]);
    let owners = Owners::from([(
      ":1.7".to_owned(),
      HashSet::from(["org.mpris.MediaPlayer2.chromium.instance1".to_owned()]),
    )]);

    seat(&players, "org.mpris.MediaPlayer2.chromium.instance1");
    land(
      &owners,
      &players,
      vec![("org.mpris.MediaPlayer2.chromium.instance1".to_owned(), Some(stale))],
    );

    let (driven, _) = pick(&players, "chromium").expect("one of the two instances answers");
    assert_eq!(
      driven, "org.mpris.MediaPlayer2.chromium.instance1",
      "a sweep that read a player before it was driven does not carry the older answer back over the control"
    );
  }

  #[test]
  fn the_art_a_player_advertises_is_the_only_art_it_will_be_asked_for() {
    let now = Instant::now();
    let mut known = absorb("spotify".to_owned(), &props(PLAYING), now);
    known.queue = vec![track(
      "/org/mpris/MediaPlayer2/Track/2",
      &HashMap::from([("mpris:artUrl".to_owned(), held("file:///run/user/1000/art.png"))]),
    )];

    assert_eq!(
      advertised(&known, &art_token(COVER)).as_deref(),
      Some(COVER),
      "the token the device holds resolves back to the url the player named"
    );
    assert_eq!(
      advertised(&known, &art_token("file:///run/user/1000/art.png")).as_deref(),
      Some("file:///run/user/1000/art.png"),
      "a queued track's art is reachable by its own token"
    );
    assert_eq!(
      advertised(&known, COVER),
      None,
      "a raw url is not a token any player advertised"
    );
    assert_eq!(advertised(&known, "u0000000000000000"), None);
  }

  #[test]
  fn a_player_holding_no_track_names_none() {
    let mut props = props(PLAYING);
    props.insert(
      "Metadata".to_owned(),
      OwnedValue::from(HashMap::from([
        (
          "mpris:trackid".to_owned(),
          held(ObjectPath::try_from(NO_TRACK).expect("the sentinel path")),
        ),
        ("xesam:title".to_owned(), held("Ceremony")),
      ])),
    );
    let known = absorb("vlc".to_owned(), &props, Instant::now());

    assert_eq!(
      known.track_id, None,
      "the spec's no-track sentinel is not a track that can be seeked or skipped to"
    );
  }

  #[test]
  fn inlined_cover_art_larger_than_this_desktop_holds_is_refused() {
    let payload = "A".repeat(ART_CEILING / 3 * 4 + 8);
    assert_eq!(
      inline_art(&format!("image/jpeg;base64,{payload}")),
      None,
      "art that would decode past the ceiling is refused before it is decoded"
    );
  }

  #[test]
  fn a_connection_holding_two_player_names_is_read_for_both() {
    let mut owners = Owners::new();
    owners.entry(":1.42".to_owned()).or_default().insert(FAKE.to_owned());
    owners.entry(":1.42".to_owned()).or_default().insert(LATER.to_owned());

    assert!(owned(&owners, FAKE) && owned(&owners, LATER));
    assert_eq!(
      owners.get(":1.42").map(HashSet::len),
      Some(2),
      "one process can hold more than one player name and both have to be read"
    );

    forget(&mut owners, FAKE);
    assert!(!owned(&owners, FAKE) && owned(&owners, LATER));
    forget(&mut owners, LATER);
    assert!(owners.is_empty(), "a connection holding no player names is not tracked");
  }

  #[derive(Clone)]
  struct FakePlayer {
    called: Arc<Mutex<Vec<String>>>,
    shuffle: Arc<AtomicBool>,
    repeat: Arc<Mutex<String>>,
  }

  impl FakePlayer {
    fn new() -> Self {
      Self {
        called: Arc::default(),
        shuffle: Arc::new(AtomicBool::new(true)),
        repeat: Arc::new(Mutex::new("Playlist".to_owned())),
      }
    }
  }

  #[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
  impl FakePlayer {
    fn play(&self) {
      self.called.lock().unwrap().push("Play".to_owned());
    }

    fn pause(&self) {
      self.called.lock().unwrap().push("Pause".to_owned());
    }

    #[zbus(property)]
    fn playback_status(&self) -> String {
      PLAYING.to_owned()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
      metadata()
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
      42_000_000
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
      self.shuffle.load(Ordering::Relaxed)
    }

    #[zbus(property)]
    fn set_shuffle(&self, on: bool) {
      self.shuffle.store(on, Ordering::Relaxed);
    }

    #[zbus(property)]
    fn loop_status(&self) -> String {
      self.repeat.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn set_loop_status(&self, status: String) {
      *self.repeat.lock().unwrap() = status;
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
      1.0
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
      true
    }
  }

  async fn sessions(backend: &MprisMedia) -> Vec<MediaSessionSnapshot> {
    let (sink, sessions) = MediaSnapshotSink::channel();
    backend.snapshot_all(sink);
    sessions.await.expect("the sink settles")
  }

  async fn watched(
    backend: &MprisMedia,
    ticks: &mut UnboundedReceiver<()>,
    package: &str,
    ready: impl Fn(&MediaSessionSnapshot) -> bool,
  ) -> MediaSessionSnapshot {
    let armed = tokio::time::Instant::now() + ARMED;
    loop {
      assert!(
        tokio::time::Instant::now() < armed,
        "{package} never reached the snapshot this test was waiting for"
      );
      let _ = tokio::time::timeout(TICK, ticks.recv()).await;
      if let Some(found) = sessions(backend)
        .await
        .into_iter()
        .find(|found| found.package == package && ready(found))
      {
        return found;
      }
    }
  }

  async fn dropped(backend: &MprisMedia, ticks: &mut UnboundedReceiver<()>, package: &str) {
    let armed = tokio::time::Instant::now() + ARMED;
    loop {
      assert!(
        tokio::time::Instant::now() < armed,
        "{package} was still being read after it left the bus"
      );
      let _ = tokio::time::timeout(TICK, ticks.recv()).await;
      if !sessions(backend).await.iter().any(|found| found.package == package) {
        return;
      }
    }
  }

  #[tokio::test]
  #[ignore = "needs a session bus: dbus-run-session -- cargo test -p bridgething-desktop"]
  async fn a_player_on_the_session_bus_is_read_and_driven_over_mpris() {
    let fake = FakePlayer::new();
    let _served = zbus::connection::Builder::session()
      .expect("a session bus")
      .name(FAKE)
      .expect("the fake player name is free on a private bus")
      .serve_at(OBJECT, fake.clone())
      .expect("the player object")
      .build()
      .await
      .expect("a stand-in player");

    let backend = MprisMedia::default();
    let (inbox, mut ticks) = MediaSessionInbox::channel();
    backend.start(inbox);

    let snapshot = watched(&backend, &mut ticks, FAKE_PACKAGE, |_| true).await;
    assert!(backend.is_access_granted(), "a bus in hand is all the access there is");
    assert_eq!(snapshot.title.as_deref(), Some("Ceremony"));
    assert_eq!(snapshot.artist.as_deref(), Some("New Order, Joy Division"));
    assert_eq!(snapshot.position_ms, 42_000);
    assert!(snapshot.playing && snapshot.can_seek);
    assert_eq!(snapshot.shuffle, Some(true));
    assert_eq!(snapshot.repeat, Some(MediaRepeatMode::All));
    assert!(
      snapshot.queue.is_empty(),
      "a player with no track list interface has nothing queued"
    );

    backend.control(FAKE_PACKAGE.to_owned(), MediaControl::Play);
    let armed = tokio::time::Instant::now() + ARMED;
    loop {
      if fake.called.lock().unwrap().iter().any(|call| call == "Play") {
        break;
      }
      assert!(
        tokio::time::Instant::now() < armed,
        "the play never reached the player on the bus"
      );
      tokio::time::sleep(Duration::from_millis(10)).await;
    }

    backend.control(FAKE_PACKAGE.to_owned(), MediaControl::SetShuffle { on: false });
    backend.control(
      FAKE_PACKAGE.to_owned(),
      MediaControl::SetRepeat {
        mode: MediaRepeatMode::One,
      },
    );
    let turned = watched(&backend, &mut ticks, FAKE_PACKAGE, |found| {
      found.shuffle == Some(false) && found.repeat == Some(MediaRepeatMode::One)
    })
    .await;
    assert_eq!(
      (turned.shuffle, turned.repeat),
      (Some(false), Some(MediaRepeatMode::One)),
      "a write to a player property comes back through the change the player announces"
    );

    let later = zbus::connection::Builder::session()
      .expect("a session bus")
      .name(LATER)
      .expect("the second player name is free on a private bus")
      .serve_at(OBJECT, fake.clone())
      .expect("the player object")
      .build()
      .await
      .expect("a second stand-in player");

    let appeared = watched(&backend, &mut ticks, LATER_PACKAGE, |_| true).await;
    assert_eq!(
      appeared.title.as_deref(),
      Some("Ceremony"),
      "a player that starts after the watcher is read the same way"
    );

    drop(later);
    dropped(&backend, &mut ticks, LATER_PACKAGE).await;

    backend.stop();
    assert!(
      !backend.is_access_granted(),
      "a stopped watcher holds no bus for the core to read"
    );

    let (inbox, mut ticks) = MediaSessionInbox::channel();
    backend.start(inbox);
    let again = watched(&backend, &mut ticks, FAKE_PACKAGE, |_| true).await;
    assert_eq!(again.title, snapshot.title, "a restarted watcher reads the same bus");

    backend.stop();
  }

  #[tokio::test]
  #[ignore = "needs BRIDGETHING_MPRIS_LIVE=1 and vlc plus mpv already playing on the session bus"]
  async fn a_real_player_over_the_session_bus_is_read_and_driven() {
    if std::env::var_os(LIVE).is_none() {
      eprintln!("skipping the live player read: {STAGING}");
      return;
    }

    let backend = MprisMedia::default();
    let (inbox, mut ticks) = MediaSessionInbox::channel();
    backend.start(inbox);

    let vlc = watched(&backend, &mut ticks, VLC_PACKAGE, |found| {
      found.playing && !found.queue.is_empty()
    })
    .await;
    assert!(backend.is_access_granted(), "a bus in hand is all the access there is");
    let title = vlc.title.clone().expect("vlc names the track it is on");
    assert!(vlc.artist.is_some_and(|artist| !artist.is_empty()));
    assert!(vlc.album.is_some_and(|album| !album.is_empty()));
    let duration = vlc.duration_ms.expect("vlc measures the file it opened");
    assert!(duration > 0 && vlc.can_seek);
    assert_eq!(
      vlc.repeat,
      Some(MediaRepeatMode::All),
      "a playlist vlc was told to loop reads back as repeat all"
    );
    assert_eq!(vlc.speed, Some(1.0));
    assert!(
      vlc.queue.len() >= 2,
      "vlc answers the track list interface, so the playlist it was handed is the queue"
    );
    let active = vlc.active_queue_id.expect("vlc names the track it is sitting on");
    let sitting = vlc
      .queue
      .iter()
      .find(|entry| entry.queue_id == active)
      .expect("the track vlc is on is one of the tracks it queued");
    assert_eq!(
      sitting.title.as_deref(),
      Some(title.as_str()),
      "the queue entry the player is sitting on is the track it is playing"
    );

    let token = vlc.art_token.clone().expect("vlc unpacks embedded art onto disk");
    let (sink, painted) = MediaArtSink::channel();
    backend.art(VLC_PACKAGE.to_owned(), token, sink);
    let art = painted
      .await
      .expect("the sink settles")
      .expect("art vlc left on disk is art this desktop can read");
    assert_eq!(art.mime, "image/jpeg");
    assert!(
      art.bytes.starts_with(&[0xff, 0xd8, 0xff]),
      "art fetched over file:// comes back re-encoded as jpeg"
    );

    backend.control(VLC_PACKAGE.to_owned(), MediaControl::Pause);
    let paused = watched(&backend, &mut ticks, VLC_PACKAGE, |found| !found.playing).await;
    assert_eq!(
      paused.position_age_ms, None,
      "a position that is not moving does not go stale"
    );
    backend.control(VLC_PACKAGE.to_owned(), MediaControl::Play);
    let resumed = watched(&backend, &mut ticks, VLC_PACKAGE, |found| found.playing).await;
    assert!(resumed.position_age_ms.is_some());

    let landing = duration / 2;
    let departed = resumed.position_ms;
    assert!(departed < landing, "the seek has somewhere to travel to");
    let issued = Instant::now();
    backend.control(VLC_PACKAGE.to_owned(), MediaControl::SeekTo { position_ms: landing });
    let seeked = watched(&backend, &mut ticks, VLC_PACKAGE, |found| found.position_ms >= landing).await;
    let waited = issued.elapsed().as_millis() as i64;
    assert!(
      seeked.position_ms - departed > waited + 1_000,
      "vlc honors SetPosition, so the position jumped further than playing on could have carried it"
    );
    assert!(
      seeked.position_ms < landing + 10_000,
      "vlc honors SetPosition, so the position lands where it was sent"
    );

    let target = vlc
      .queue
      .iter()
      .rev()
      .find(|entry| entry.queue_id != active)
      .expect("a playlist of more than one has somewhere else to go")
      .clone();
    backend.control(
      VLC_PACKAGE.to_owned(),
      MediaControl::SkipToQueueItem {
        queue_id: target.queue_id,
      },
    );
    let landed = watched(&backend, &mut ticks, VLC_PACKAGE, |found| {
      found.active_queue_id == Some(target.queue_id)
    })
    .await;
    assert_eq!(
      landed.title, target.title,
      "the queued track the device asked for is the one vlc moved to"
    );

    let mpv = watched(&backend, &mut ticks, MPV_PACKAGE, |found| found.playing).await;
    assert!(mpv.title.is_some_and(|title| !title.is_empty()));
    assert!(mpv.artist.is_some_and(|artist| !artist.is_empty()));
    assert!(mpv.album.is_some_and(|album| !album.is_empty()));
    assert!(mpv.duration_ms.is_some_and(|ms| ms > 0));
    assert!(
      mpv.art_token.is_none(),
      "mpv names no artUrl for art buried in the file, and a track without art is still a session"
    );
    assert!(
      mpv.queue.is_empty(),
      "mpv keeps no track list, and the interface-absent answer it gives is not a queue"
    );

    backend.control(MPV_PACKAGE.to_owned(), MediaControl::Pause);
    let held = watched(&backend, &mut ticks, MPV_PACKAGE, |found| !found.playing).await;
    backend.control(MPV_PACKAGE.to_owned(), MediaControl::Play);
    let running = watched(&backend, &mut ticks, MPV_PACKAGE, |found| found.playing).await;
    assert!(
      held.queue.is_empty() && running.queue.is_empty(),
      "a player that answered once for having no track list is read on without being asked again"
    );
    assert_eq!(
      running.title, held.title,
      "a player with no track list keeps being read like any other"
    );
    assert_eq!(
      probed(&backend, MPV_BUS),
      Listing::Absent,
      "the interface-absent answer mpv gives latches, so it is not asked for a track list on every read"
    );
    assert_eq!(
      probed(&backend, VLC_BUS),
      Listing::Present,
      "a player that does answer for its track list keeps being asked"
    );

    backend.stop();
  }

  fn probed(backend: &MprisMedia, name: &str) -> Listing {
    backend
      .held
      .lock()
      .unwrap()
      .as_ref()
      .expect("a watcher holding the bus")
      .players
      .lock()
      .unwrap()
      .get(name)
      .expect("a player that was read at least once")
      .listing
  }
}
