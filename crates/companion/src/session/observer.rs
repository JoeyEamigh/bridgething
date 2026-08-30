use std::{
  collections::VecDeque,
  sync::{Arc, Mutex},
};

use bridgething_delivery::{
  log::{DeviceLogRing, LogOrigin as RingOrigin},
  ota,
};
use libbridgething::{
  AncsAuthState, BridgeThingMeta, LogEntry, PlaybackState, PlayerState, VoiceCaptureReason,
  gateway::{WebappActiveChanged, WebappDocChanged},
};

use super::LOG_RING_CAPACITY;
use crate::{
  api::{
    ActiveWebapp, AncsAuthStatus, AncsAuthStatusEntry, ConfigField, ConfigKind, DeviceMeta, DeviceMetaEntry,
    DeviceWebappsEntry, ExtensionInfo, LogOrigin, NowPlaying, NowPlayingPlayback, NowPlayingTrack, RepeatMode,
    SessionEvent, SessionEventSink, SessionPeer, VoiceTurn, VoiceTurnPhase, VoiceTurnTrigger, WebappInfo, WebappRole,
    WebappSource,
  },
  backend::{LogLevel, log::ring_level},
  dispatch::{system::DeviceLogSink, webapp::WebappObserver},
  voice::dispatcher::{VoiceTurnObserver, VoiceTurnPhase as DispatchTurnPhase, VoiceTurnUpdate},
};

#[derive(Default)]
struct Held {
  peers: Vec<SessionPeer>,
  device_meta: Vec<DeviceMetaEntry>,
  ancs: Vec<AncsAuthStatusEntry>,
  webapps: Vec<DeviceWebappsEntry>,
  now_playing: Option<NowPlaying>,
}

#[derive(Default)]
struct LogHold {
  depth: usize,
  pending: VecDeque<LogEntry>,
}

impl LogHold {
  fn queue(&mut self, entries: impl IntoIterator<Item = LogEntry>) {
    self.pending.extend(entries);
    while self.pending.len() > LOG_RING_CAPACITY {
      self.pending.pop_front();
    }
  }
}

pub struct SessionObserver {
  events: Arc<dyn SessionEventSink>,
  log_ring: Arc<DeviceLogRing>,
  held: Mutex<Held>,
  log_hold: Mutex<LogHold>,
}

impl SessionObserver {
  pub fn new(events: Arc<dyn SessionEventSink>, log_ring: Arc<DeviceLogRing>) -> Arc<Self> {
    Arc::new(Self {
      events,
      log_ring,
      held: Mutex::new(Held::default()),
      log_hold: Mutex::new(LogHold::default()),
    })
  }

  pub fn emit(&self, event: SessionEvent) {
    self.events.on_event(event);
  }

  pub fn peer_connected(&self, peer: SessionPeer) {
    {
      let mut held = self.held.lock().unwrap();
      held.peers.retain(|known| known.id != peer.id);
      held.peers.push(peer.clone());
    }
    self.emit(SessionEvent::PeerConnected { peer });
  }

  pub fn peer_link_failed(&self, peer: SessionPeer) {
    self.forget(&peer.id);
    self.emit(SessionEvent::PeerLinkFailed { peer });
  }

  pub fn peer_disconnected(&self, device_id: &str) {
    self.forget(device_id);
    self.emit(SessionEvent::PeerDisconnected {
      device_id: device_id.to_owned(),
    });
  }

  fn forget(&self, device_id: &str) {
    let mut held = self.held.lock().unwrap();
    held.peers.retain(|known| known.id != device_id);
    held.device_meta.retain(|entry| entry.device_id != device_id);
    held.ancs.retain(|entry| entry.device_id != device_id);
    held.webapps.retain(|entry| entry.device_id != device_id);
  }

  pub fn peers(&self) -> Vec<SessionPeer> {
    self.held.lock().unwrap().peers.clone()
  }

  pub fn device_meta(&self, device_id: &str, meta: BridgeThingMeta) {
    let device_id = device_id.to_owned();
    let meta = DeviceMeta {
      daemon_version: meta.app_version,
      libbridgething_version: meta.libbridgething_version,
      image_version: meta.image_version,
      app_name: meta.app_name,
      os_name: meta.os_name,
      os_version: meta.os_version,
      channel: meta.channel,
      model_name: meta.model_name,
      serial_number: meta.serial_number,
      nickname: meta.nickname,
    };
    {
      let mut held = self.held.lock().unwrap();
      held.device_meta.retain(|entry| entry.device_id != device_id);
      held.device_meta.push(DeviceMetaEntry {
        device_id: device_id.clone(),
        meta: meta.clone(),
      });
    }
    self.emit(SessionEvent::DeviceMetaChanged { device_id, meta });
  }

  pub fn device_metas(&self) -> Vec<DeviceMetaEntry> {
    self.held.lock().unwrap().device_meta.clone()
  }

  pub fn ancs(&self, device_id: &str, state: AncsAuthState) {
    let device_id = device_id.to_owned();
    let status = match state {
      AncsAuthState::Unknown => AncsAuthStatus::Unknown,
      AncsAuthState::Probing => AncsAuthStatus::Probing,
      AncsAuthState::Authorized => AncsAuthStatus::Authorized,
      AncsAuthState::Unauthorized => AncsAuthStatus::Unauthorized,
    };
    {
      let mut held = self.held.lock().unwrap();
      held.ancs.retain(|entry| entry.device_id != device_id);
      held.ancs.push(AncsAuthStatusEntry {
        device_id: device_id.clone(),
        status,
      });
    }
    self.emit(SessionEvent::AncsAuthStatusChanged { device_id, status });
  }

  pub fn ancs_statuses(&self) -> Vec<AncsAuthStatusEntry> {
    self.held.lock().unwrap().ancs.clone()
  }

  pub fn now_playing_changed(&self, state: Option<PlayerState>) {
    let now_playing = state.map(project);
    self.held.lock().unwrap().now_playing = now_playing.clone();
    self.emit(SessionEvent::NowPlayingChanged { now_playing });
  }

  pub fn now_playing(&self) -> Option<NowPlaying> {
    self.held.lock().unwrap().now_playing.clone()
  }

  pub fn update_store_changed(&self, change: ota::run_store::OtaStoreChange) {
    self.emit(match change {
      ota::run_store::OtaStoreChange::Run(run) => SessionEvent::OtaRunChanged { run: (*run).into() },
      ota::run_store::OtaStoreChange::Available(available) => SessionEvent::OtaAvailableChanged {
        available: available.into(),
      },
      ota::run_store::OtaStoreChange::Poll(status) => SessionEvent::OtaPollChanged { status: status.into() },
    });
  }

  pub fn webapps(&self) -> Vec<DeviceWebappsEntry> {
    self.held.lock().unwrap().webapps.clone()
  }

  pub fn webapps_listed(&self, device_id: &str, webapps: Vec<WebappInfo>, active: Option<ActiveWebapp>) {
    let entry = {
      let mut held = self.held.lock().unwrap();
      let at = self.entry(&mut held, device_id);
      held.webapps[at].webapps = webapps;
      held.webapps[at].active = active;
      held.webapps[at].listed = true;
      held.webapps[at].clone()
    };
    self.emit(SessionEvent::WebappsChanged { entry });
  }

  fn entry(&self, held: &mut Held, device_id: &str) -> usize {
    match held.webapps.iter().position(|entry| entry.device_id == device_id) {
      Some(at) => at,
      None => {
        held.webapps.push(DeviceWebappsEntry {
          device_id: device_id.to_owned(),
          webapps: Vec::new(),
          active: None,
          listed: false,
        });
        held.webapps.len() - 1
      }
    }
  }

  pub fn hold_logs(&self) {
    self.log_hold.lock().unwrap().depth += 1;
  }

  pub fn backfill_logs(&self, tail: Vec<LogEntry>) {
    let spliced = {
      let mut hold = self.log_hold.lock().unwrap();
      hold.depth = hold.depth.saturating_sub(1);
      let live = std::mem::take(&mut hold.pending);
      let spliced = splice(tail, live);
      if hold.depth > 0 {
        hold.queue(spliced);
        return;
      }
      spliced
    };
    for entry in spliced {
      self.ingest(entry);
    }
  }

  fn ingest(&self, entry: LogEntry) {
    let level = level(entry.level);
    self
      .log_ring
      .push(RingOrigin::Device, ring_level(level), &entry.target, &entry.message);
    self.emit(SessionEvent::Log {
      origin: LogOrigin::Device,
      level,
      target: entry.target,
      message: entry.message,
    });
  }
}

fn splice(tail: Vec<LogEntry>, live: VecDeque<LogEntry>) -> Vec<LogEntry> {
  let overlap = (1..=tail.len().min(live.len()))
    .rev()
    .find(|len| tail[tail.len() - len..].iter().eq(live.iter().take(*len)))
    .unwrap_or(0);
  tail.into_iter().chain(live.into_iter().skip(overlap)).collect()
}

fn project(state: PlayerState) -> NowPlaying {
  let track = state.track.map(|item| NowPlayingTrack {
    id: item.uri.or(item.persistent_id),
    title: item.title,
    artist: item.artist,
    artwork_url: None,
    album: item.album,
    duration_ms: item.duration_ms.map(u64::from),
  });
  NowPlaying {
    track,
    playback: NowPlayingPlayback {
      playing: state.playback.state == PlaybackState::Playing,
      position_ms: u64::from(state.playback.position_ms),
      shuffle: state.playback.shuffle,
      repeat_mode: match state.playback.repeat {
        libbridgething::RepeatMode::Off => RepeatMode::Off,
        libbridgething::RepeatMode::One => RepeatMode::One,
        libbridgething::RepeatMode::All => RepeatMode::All,
      },
    },
    app_name: state.context.and_then(|context| context.name),
  }
}

pub(crate) fn webapp(info: libbridgething::WebappInfo) -> WebappInfo {
  WebappInfo {
    id: info.id.to_string(),
    name: info.name,
    source: match info.source {
      libbridgething::WebappSource::Builtin => WebappSource::Builtin,
      libbridgething::WebappSource::Installed => WebappSource::Installed,
    },
    role: match info.role {
      libbridgething::WebappRole::Launcher => WebappRole::Launcher,
      _ => WebappRole::Standard,
    },
    version: info.version,
    provenance: info.provenance,
    description: info.description,
    icon_hash: info.icon_hash,
    settings_hash: info.settings_hash,
    overlay_hash: info.overlay_hash,
    config: info.config.into_iter().map(field).collect(),
    permissions: info.permissions,
    extension: info.extension.map(|extension| ExtensionInfo {
      permissions: extension.permissions.iter().map(ToString::to_string).collect(),
      api: extension.api,
    }),
  }
}

impl DeviceLogSink for SessionObserver {
  fn on_entry(&self, entry: LogEntry) {
    {
      let mut hold = self.log_hold.lock().unwrap();
      if hold.depth > 0 {
        hold.queue([entry]);
        return;
      }
    }
    self.ingest(entry);
  }
}

impl WebappObserver for SessionObserver {
  fn doc_changed(&self, device_id: &str, changed: WebappDocChanged) {
    self.emit(SessionEvent::WebappDocChanged {
      device_id: device_id.to_owned(),
      webapp_id: changed.id.to_string(),
      key: changed.key,
      value: changed.value,
    });
  }

  fn installed(&self, device_id: &str, info: libbridgething::WebappInfo) {
    let info = webapp(info);
    let entry = {
      let mut held = self.held.lock().unwrap();
      let at = self.entry(&mut held, device_id);
      held.webapps[at].webapps.retain(|known| known.id != info.id);
      held.webapps[at].webapps.push(info);
      held.webapps[at].clone()
    };
    self.emit(SessionEvent::WebappsChanged { entry });
  }

  fn active_changed(&self, device_id: &str, changed: WebappActiveChanged) {
    let entry = {
      let mut held = self.held.lock().unwrap();
      let at = self.entry(&mut held, device_id);
      held.webapps[at].active = changed.id.map(|id| ActiveWebapp {
        id: id.to_string(),
        name: changed.name,
      });
      held.webapps[at].clone()
    };
    self.emit(SessionEvent::WebappsChanged { entry });
  }
}

impl VoiceTurnObserver for SessionObserver {
  fn turn_changed(&self, device_id: &str, update: VoiceTurnUpdate<'_>) {
    let (phase, transcript, intent) = match update.phase {
      DispatchTurnPhase::Listening => (VoiceTurnPhase::Listening, None, None),
      DispatchTurnPhase::Cancelled => (VoiceTurnPhase::Cancelled, None, None),
      DispatchTurnPhase::Resolved(resolved) => (
        VoiceTurnPhase::Resolved,
        Some(resolved.transcript.clone()),
        Some(resolved.intent.clone()),
      ),
    };
    self.emit(SessionEvent::VoiceTurnChanged {
      turn: VoiceTurn {
        device_id: device_id.to_owned(),
        stream_id: update.stream_id.to_string(),
        trigger: trigger(update.reason),
        phase,
        transcript,
        intent,
      },
    });
  }
}

fn trigger(reason: VoiceCaptureReason) -> VoiceTurnTrigger {
  match reason {
    VoiceCaptureReason::PushToTalk => VoiceTurnTrigger::PushToTalk,
    VoiceCaptureReason::Assistant => VoiceTurnTrigger::Assistant,
    VoiceCaptureReason::WakeWord => VoiceTurnTrigger::WakeWord,
  }
}

fn level(level: libbridgething::LogLevel) -> LogLevel {
  match level {
    libbridgething::LogLevel::Trace => LogLevel::Trace,
    libbridgething::LogLevel::Debug => LogLevel::Debug,
    libbridgething::LogLevel::Info => LogLevel::Info,
    libbridgething::LogLevel::Warn => LogLevel::Warn,
    libbridgething::LogLevel::Error => LogLevel::Error,
  }
}

fn field(field: libbridgething::ConfigField) -> ConfigField {
  let bare = |kind, key: String, label: String| ConfigField {
    kind,
    key,
    label,
    pattern: None,
    min_length: None,
    max_length: None,
    min: None,
    max: None,
    step: None,
    choices: Vec::new(),
    default_value: None,
  };
  match field {
    libbridgething::ConfigField::String(text) => ConfigField {
      pattern: text.pattern,
      min_length: text.min_length,
      max_length: text.max_length,
      default_value: text.default,
      ..bare(ConfigKind::String, text.key, text.label)
    },
    libbridgething::ConfigField::Secret(text) => ConfigField {
      pattern: text.pattern,
      min_length: text.min_length,
      max_length: text.max_length,
      default_value: text.default,
      ..bare(ConfigKind::Secret, text.key, text.label)
    },
    libbridgething::ConfigField::Number(number) => ConfigField {
      min: number.min,
      max: number.max,
      step: number.step,
      default_value: number.default.map(|value| value.to_string()),
      ..bare(ConfigKind::Number, number.key, number.label)
    },
    libbridgething::ConfigField::Boolean(flag) => ConfigField {
      default_value: flag.default.map(|value| value.to_string()),
      ..bare(ConfigKind::Boolean, flag.key, flag.label)
    },
    libbridgething::ConfigField::Enum(choice) => ConfigField {
      choices: choice.choices,
      default_value: choice.default,
      ..bare(ConfigKind::Enum, choice.key, choice.label)
    },
  }
}

#[cfg(test)]
mod tests {
  use bridgething_delivery::seam::SystemClock;

  use super::*;

  #[derive(Default)]
  struct Recorded(Mutex<Vec<SessionEvent>>);

  impl SessionEventSink for Recorded {
    fn on_event(&self, event: SessionEvent) {
      self.0.lock().unwrap().push(event);
    }
  }

  fn entries(recorded: &Recorded) -> Vec<DeviceWebappsEntry> {
    recorded
      .0
      .lock()
      .unwrap()
      .iter()
      .filter_map(|event| match event {
        SessionEvent::WebappsChanged { entry } => Some(entry.clone()),
        _ => None,
      })
      .collect()
  }

  #[test]
  fn an_active_webapp_push_before_the_listing_is_not_an_inventory() {
    let recorded = Arc::new(Recorded::default());
    let observer = SessionObserver::new(recorded.clone(), Arc::new(DeviceLogRing::new(8, Arc::new(SystemClock))));

    observer.active_changed(
      "sn-1",
      WebappActiveChanged {
        id: Some(uuid::Uuid::from_u128(1)),
        name: Some("weather".to_owned()),
        art: None,
      },
    );
    observer.webapps_listed("sn-1", Vec::new(), None);

    let seen = entries(&recorded);
    assert_eq!(seen.len(), 2);
    assert!(
      !seen[0].listed,
      "the device's webapps have not been read yet, so an empty list here is ignorance, not an empty device"
    );
    assert!(
      seen[1].listed,
      "the listing is the only event that carries what the device actually holds"
    );
  }
}
