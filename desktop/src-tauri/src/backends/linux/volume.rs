use std::{
  collections::HashSet,
  ffi::CStr,
  io::{BufReader, Cursor, Read, Write},
  net::Shutdown,
  os::unix::net::UnixStream,
  sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
    mpsc::{Receiver, RecvTimeoutError, Sender, channel},
  },
  thread,
  time::{Duration, Instant},
};

use bridgething_companion::backend::{VolumeBackend, VolumeInbox, VolumeLevel};
use pulseaudio::protocol::{self, TagStructRead};

const CLIENT_NAME: &CStr = c"bridgething";
const STEP: f32 = 1.0 / 16.0;
const RETRY_MIN: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(30);

enum Op {
  Set(f32),
  Mute(bool),
  Step(f32),
  ToggleMute,
  Refresh,
  Quit,
}

#[derive(Clone, Copy, Default)]
struct Sink {
  level: f32,
  muted: bool,
  channels: u8,
}

#[derive(Default)]
struct Shared {
  sink: Mutex<Option<Sink>>,
  inbox: Mutex<Option<Arc<VolumeInbox>>>,
}

impl Shared {
  fn publish(&self, sink: Sink) {
    let changed = {
      let mut held = self.sink.lock().unwrap();
      let changed = !matches!(*held, Some(held) if held.level == sink.level && held.muted == sink.muted);
      *held = Some(sink);
      changed
    };
    if !changed {
      return;
    }
    let inbox = self.inbox.lock().unwrap().clone();
    if let Some(inbox) = inbox {
      inbox.on_changed(sink.level, sink.muted);
    }
  }
}

#[derive(Default)]
pub struct PulseVolume {
  shared: Arc<Shared>,
  engine: Mutex<Option<Sender<Op>>>,
}

impl PulseVolume {
  fn send(&self, op: Op) {
    let held = self.engine.lock().unwrap();
    if let Some(engine) = held.as_ref() {
      let _ = engine.send(op);
    }
  }
}

impl VolumeBackend for PulseVolume {
  fn start(&self, inbox: Arc<VolumeInbox>) {
    *self.shared.inbox.lock().unwrap() = Some(inbox);
    let mut held = self.engine.lock().unwrap();
    if held.is_some() {
      return;
    }
    let (tx, rx) = channel();
    let shared = Arc::clone(&self.shared);
    let refresh = tx.clone();
    match thread::Builder::new()
      .name("bridgething-volume".to_owned())
      .spawn(move || run(shared, rx, refresh))
    {
      Ok(_) => *held = Some(tx),
      Err(error) => tracing::warn!(%error, "the volume engine could not be started"),
    }
  }

  fn stop(&self) {
    // the event listener holds a sender too, so the channel only closes on an explicit quit
    let engine = self.engine.lock().unwrap().take();
    if let Some(engine) = engine {
      let _ = engine.send(Op::Quit);
    }
    self.shared.inbox.lock().unwrap().take();
  }

  fn snapshot(&self) -> VolumeLevel {
    let sink = self.shared.sink.lock().unwrap().unwrap_or_default();
    VolumeLevel {
      level: sink.level,
      muted: sink.muted,
    }
  }

  fn set_volume(&self, level: f32) {
    self.send(Op::Set(level));
  }

  fn set_mute(&self, muted: bool) {
    self.send(Op::Mute(muted));
  }

  fn volume_up(&self) {
    self.send(Op::Step(STEP));
  }

  fn volume_down(&self) {
    self.send(Op::Step(-STEP));
  }

  fn mute_toggle(&self) {
    self.send(Op::ToggleMute);
  }
}

enum Served {
  Quit,
  Lost { connected: bool },
}

/// Keeps answering across a pulseaudio or pipewire-pulse restart, which takes the socket
/// with it. Backs off so a box with no sound server at all does not spin.
fn run(shared: Arc<Shared>, ops: Receiver<Op>, refresh: Sender<Op>) {
  let mut backoff = RETRY_MIN;
  loop {
    match serve(&shared, &ops, &refresh) {
      Served::Quit => return,
      Served::Lost { connected } => {
        if connected {
          backoff = RETRY_MIN;
        }
      }
    }
    let deadline = Instant::now() + backoff;
    loop {
      let left = deadline.saturating_duration_since(Instant::now());
      if left.is_zero() {
        break;
      }
      match ops.recv_timeout(left) {
        // a quit that lands while the server is away still has to end the thread
        Ok(Op::Quit) | Err(RecvTimeoutError::Disconnected) => return,
        Ok(_) | Err(RecvTimeoutError::Timeout) => {}
      }
    }
    backoff = (backoff * 2).min(RETRY_MAX);
  }
}

fn serve(shared: &Arc<Shared>, ops: &Receiver<Op>, refresh: &Sender<Op>) -> Served {
  let Some((mut socket, reading, version)) = connect() else {
    tracing::debug!("no pulseaudio server answered; this desktop cannot move its own volume yet");
    return Served::Lost { connected: false };
  };

  let seq = AtomicU32::new(2);
  let pending = Arc::new(Mutex::new(HashSet::new()));
  let listening = Arc::clone(&pending);
  let events = Arc::clone(shared);
  let posting = refresh.clone();
  let listener = thread::Builder::new()
    .name("bridgething-volume-events".to_owned())
    .spawn(move || listen(reading, version, events, listening, posting));
  if let Err(error) = listener {
    tracing::warn!(%error, "the volume socket could not be listened to");
    let _ = socket.shutdown(Shutdown::Both);
    return Served::Lost { connected: true };
  }

  let ask = |socket: &mut UnixStream, command: protocol::Command, wants_info: bool| -> bool {
    let id = seq.fetch_add(1, Ordering::Relaxed);
    if wants_info {
      pending.lock().unwrap().insert(id);
    }
    protocol::write_command_message(socket, id, &command, version).is_ok() && socket.flush().is_ok()
  };

  let subscribed = ask(
    &mut socket,
    protocol::Command::Subscribe(protocol::SubscriptionMask::SINK | protocol::SubscriptionMask::SERVER),
    false,
  );
  if !subscribed {
    tracing::warn!("pulseaudio refused a subscription; volume will not follow the host");
  }
  ask(&mut socket, sink_info(), true);

  while let Ok(op) = ops.recv() {
    let wants_info = matches!(op, Op::Refresh);
    let sink = shared.sink.lock().unwrap().unwrap_or_default();
    let command = match op {
      Op::Refresh => sink_info(),
      Op::Set(level) => match volume_of(level, sink.channels) {
        Some(volume) => set_volume(volume),
        None => continue,
      },
      Op::Step(delta) => match volume_of(sink.level + delta, sink.channels) {
        Some(volume) => set_volume(volume),
        None => continue,
      },
      Op::Mute(muted) => set_mute(muted),
      Op::ToggleMute => set_mute(!sink.muted),
      Op::Quit => {
        let _ = socket.shutdown(Shutdown::Both);
        return Served::Quit;
      }
    };
    if !ask(&mut socket, command, wants_info) {
      let _ = socket.shutdown(Shutdown::Both);
      return Served::Lost { connected: true };
    }
  }

  let _ = socket.shutdown(Shutdown::Both);
  Served::Quit
}

fn listen(
  mut reading: BufReader<UnixStream>,
  version: u16,
  shared: Arc<Shared>,
  pending: Arc<Mutex<HashSet<u32>>>,
  refresh: Sender<Op>,
) {
  loop {
    let framed = match message(&mut reading, version) {
      Ok(Some(framed)) => framed,
      Ok(None) => continue,
      Err(_) => break,
    };
    let (tag, seq, mut payload) = framed;
    // any sink or default-sink change can move the number the device is showing
    if matches!(tag, protocol::CommandTag::SubscribeEvent) {
      if refresh.send(Op::Refresh).is_err() {
        break;
      }
      continue;
    }
    if matches!(tag, protocol::CommandTag::Error) {
      pending.lock().unwrap().remove(&seq);
      continue;
    }
    if !matches!(tag, protocol::CommandTag::Reply) || !pending.lock().unwrap().remove(&seq) {
      continue;
    }
    let mut ts = protocol::TagStructReader::new(&mut payload, version);
    let Ok(sink) = protocol::SinkInfo::read(&mut ts, version) else {
      continue;
    };
    shared.publish(Sink {
      level: level_of(&sink.cvolume),
      muted: sink.muted,
      channels: sink.cvolume.channels().len() as u8,
    });
  }
}

type Message = Option<(protocol::CommandTag, u32, Cursor<Vec<u8>>)>;

// reads a whole message up front so an uninteresting reply's payload cannot strand in the socket buffer
fn message(reading: &mut BufReader<UnixStream>, version: u16) -> Result<Message, protocol::ProtocolError> {
  let descriptor = protocol::read_descriptor(reading)?;
  let mut payload = vec![0u8; descriptor.length as usize];
  reading.read_exact(&mut payload)?;
  if descriptor.channel != u32::MAX {
    return Ok(None);
  }
  let mut cursor = Cursor::new(payload);
  let (tag, seq) = {
    let mut ts = protocol::TagStructReader::new(&mut cursor, version);
    (ts.read_enum()?, ts.read_u32()?)
  };
  Ok(Some((tag, seq, cursor)))
}

fn connect() -> Option<(UnixStream, BufReader<UnixStream>, u16)> {
  let path = pulseaudio::socket_path_from_env()?;
  let mut socket = UnixStream::connect(path).ok()?;
  let mut reading = BufReader::new(socket.try_clone().ok()?);
  let cookie = pulseaudio::cookie_path_from_env()
    .and_then(|path| std::fs::read(path).ok())
    .unwrap_or_default();

  let auth = protocol::Command::Auth(protocol::AuthParams {
    version: protocol::MAX_VERSION,
    supports_shm: false,
    supports_memfd: false,
    cookie,
  });
  protocol::write_command_message(&mut socket, 0, &auth, protocol::MAX_VERSION).ok()?;
  socket.flush().ok()?;
  let (_, reply): (u32, protocol::AuthReply) =
    protocol::read_reply_message(&mut reading, protocol::MAX_VERSION).ok()?;
  let version = protocol::MAX_VERSION.min(reply.version);

  let mut props = protocol::Props::new();
  props.set(protocol::Prop::ApplicationName, CLIENT_NAME);
  protocol::write_command_message(&mut socket, 1, &protocol::Command::SetClientName(props), version).ok()?;
  socket.flush().ok()?;
  let _: (u32, protocol::SetClientNameReply) = protocol::read_reply_message(&mut reading, version).ok()?;

  Some((socket, reading, version))
}

fn sink_info() -> protocol::Command {
  protocol::Command::GetSinkInfo(protocol::GetSinkInfo {
    index: None,
    name: Some(protocol::DEFAULT_SINK.to_owned()),
  })
}

fn set_volume(volume: protocol::ChannelVolume) -> protocol::Command {
  protocol::Command::SetSinkVolume(protocol::SetDeviceVolumeParams {
    device_index: None,
    device_name: Some(protocol::DEFAULT_SINK.to_owned()),
    volume,
  })
}

fn set_mute(muted: bool) -> protocol::Command {
  protocol::Command::SetSinkMute(protocol::SetDeviceMuteParams {
    device_index: None,
    device_name: Some(protocol::DEFAULT_SINK.to_owned()),
    mute: muted,
  })
}

// pulse's own to_linear is a cubic curve; the proportion of NORM is what pactl and pavucontrol show
fn level_of(volume: &protocol::ChannelVolume) -> f32 {
  let loudest = volume
    .channels()
    .iter()
    .map(|channel| channel.as_u32())
    .max()
    .unwrap_or_default();
  loudest as f32 / protocol::Volume::NORM.as_u32() as f32
}

fn volume_of(level: f32, channels: u8) -> Option<protocol::ChannelVolume> {
  if channels == 0 {
    return None;
  }
  let raw = (level.clamp(0.0, 1.0) * protocol::Volume::NORM.as_u32() as f32) as u32;
  let mut volume = protocol::ChannelVolume::empty();
  for _ in 0..channels {
    volume.push(protocol::Volume::from_u32_clamped(raw));
  }
  Some(volume)
}
