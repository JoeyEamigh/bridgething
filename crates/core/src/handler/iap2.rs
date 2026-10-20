use std::{
  collections::HashMap,
  sync::Arc,
  time::{Duration, Instant},
};

use bridgething_iap2::{
  SessionEvent,
  csm::now_playing::{
    MediaItemAttributes, MediaTypeKind, NowPlayingUpdate as Iap2NowPlayingUpdate, PlaybackAttributes, PlaybackState,
    RepeatMode, ShuffleMode as Iap2ShuffleMode,
  },
};
use libbridgething::{
  DeviceType, MediaItemUpdate, MediaType as LibMediaType, NowPlayingUpdate, PeerCompanionStatus, PeerIap2Status,
  PlaybackUpdate, ShuffleMode as LibShuffleMode, gateway::KeepalivePing,
};
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{
  asset::Retention,
  bluetooth::{
    Address, BluetoothError, BluetoothMan,
    iap2::{EaActivity, Iap2EaGatewayHandle, Iap2Event, Iap2ReconnectHandle, StreamClosed, StreamOpened},
  },
  state::State,
};

const IDLE_PID_HEX: &str = "0000000000000000";
const NONMUSIC_PREFIX: &str = "nonmusic-";

#[derive(Debug, Default, Clone)]
struct NowPlayingCheckpoint {
  track_pid_hex: Option<String>,
  track_title: Option<String>,
  app_bundle: Option<String>,
}

type NowPlayingCheckpointMap = Mutex<HashMap<Address, NowPlayingCheckpoint>>;

#[derive(Debug, Clone)]
struct PendingArtEntry {
  transfer_id: u8,
  track_key: String,
}

#[derive(Debug, Clone, Default)]
pub struct Iap2PendingArt {
  inner: Arc<Mutex<HashMap<Address, PendingArtEntry>>>,
}

impl Iap2PendingArt {
  pub fn new() -> Self {
    Self::default()
  }

  async fn mark(&self, address: Address, transfer_id: u8, track_key: String) {
    self
      .inner
      .lock()
      .await
      .insert(address, PendingArtEntry { transfer_id, track_key });
  }

  async fn take_if_matches(&self, address: Address, transfer_id: u8) -> Option<String> {
    let mut guard = self.inner.lock().await;
    match guard.get(&address) {
      Some(entry) if entry.transfer_id == transfer_id => guard.remove(&address).map(|e| e.track_key),
      _ => None,
    }
  }

  async fn clear(&self, address: Address) {
    self.inner.lock().await.remove(&address);
  }

  pub async fn is_pending(&self, asset_id: &str) -> bool {
    self
      .inner
      .lock()
      .await
      .values()
      .any(|e| iap2_art_prefix_of(asset_id) == Some(e.track_key.as_str()))
  }
}

fn iap2_art_prefix_of(asset_id: &str) -> Option<&str> {
  let rest = asset_id.strip_prefix("iap2/art/")?;
  rest.rsplit_once('/').map(|(key, _)| key)
}

fn iap2_art_asset_id(track_key: &str, bytes: &[u8]) -> String {
  use std::hash::{DefaultHasher, Hash, Hasher};
  let mut hasher = DefaultHasher::new();
  bytes.hash(&mut hasher);
  format!("iap2/art/{track_key}/{:016x}", hasher.finish())
}

#[derive(Debug)]
pub struct Iap2EventRouter {
  state: State,
  bluetooth: BluetoothMan,
  ea_gateway: Iap2EaGatewayHandle,
  reconnect: Iap2ReconnectHandle,
  pending_art: Iap2PendingArt,
  np_checkpoint: NowPlayingCheckpointMap,
  keepalive: Mutex<HashMap<Address, JoinHandle<()>>>,
}

impl Iap2EventRouter {
  pub fn new(
    state: State,
    bluetooth: BluetoothMan,
    ea_gateway: Iap2EaGatewayHandle,
    reconnect: Iap2ReconnectHandle,
    pending_art: Iap2PendingArt,
  ) -> Self {
    Self {
      state,
      bluetooth,
      ea_gateway,
      reconnect,
      pending_art,
      np_checkpoint: Mutex::new(HashMap::new()),
      keepalive: Mutex::new(HashMap::new()),
    }
  }

  async fn start_ea_keepalive(&self, address: Address) {
    let handle = spawn_ea_keepalive(
      self.bluetooth.clone(),
      self.state.clone(),
      self.ea_gateway.activity(),
      address,
    );
    if let Some(prev) = self.keepalive.lock().await.insert(address, handle) {
      prev.abort();
    }
  }

  async fn stop_ea_keepalive(&self, address: Address) {
    if let Some(handle) = self.keepalive.lock().await.remove(&address) {
      handle.abort();
    }
  }

  pub async fn route(&self, event: Iap2Event) {
    let Iap2Event { address, event } = event;
    match event {
      SessionEvent::LinkEstablished(lsp) => {
        tracing::info!(
          %address,
          peer_max_outgoing = lsp.max_outgoing,
          peer_max_len = lsp.max_len,
          "iAP2 link Established",
        );
        match self
          .bluetooth
          .profile_man
          .upsert_paired_device(address, DeviceType::Ios)
          .await
        {
          Ok(_) | Err(BluetoothError::NoRadio) => {}
          Err(err) => tracing::warn!(%address, ?err, "failed to upsert peer for iAP2 link"),
        }
        let _ = self.state.peers.set_iap2(address, PeerIap2Status::LinkUp).await;
        self.bluetooth.le.attach(address).await;
        self.start_ea_keepalive(address).await;
      }
      SessionEvent::LinkRestarting(reason) => {
        tracing::info!(%address, %reason, "iAP2 link restarting in place");
        self.stop_ea_keepalive(address).await;
        let _ = self.state.peers.set_iap2(address, PeerIap2Status::None).await;
        self.np_checkpoint.lock().await.remove(&address);
        self.pending_art.clear(address).await;
      }
      SessionEvent::Authenticated => {
        tracing::info!(%address, "iAP2 authenticated");
        let _ = self.state.peers.set_iap2(address, PeerIap2Status::Authenticated).await;
      }
      SessionEvent::Identified => {
        tracing::info!(%address, "iAP2 identified");
        let _ = self.state.peers.set_iap2(address, PeerIap2Status::Identified).await;
      }
      SessionEvent::AuthFailed => tracing::warn!(%address, "iAP2 auth failed"),
      SessionEvent::IdentificationRejected { rejected_params } => {
        tracing::warn!(%address, ?rejected_params, "iAP2 identification rejected");
      }
      SessionEvent::NowPlayingUpdate(update) => {
        let pid_hex = {
          let mut guard = self.np_checkpoint.lock().await;
          let entry = guard.entry(address).or_default();
          if let Some(bundle) = update
            .playback
            .as_ref()
            .and_then(|p| p.app_bundle.as_deref())
            .filter(|b| !b.is_empty())
          {
            if entry.app_bundle.as_deref().is_some_and(|held| held != bundle) {
              entry.track_pid_hex = None;
              entry.track_title = None;
            }
            entry.app_bundle = Some(bundle.to_string());
          }
          match track_delta(update.media_item.as_ref(), entry) {
            TrackDelta::Unchanged => entry.track_pid_hex.clone(),
            TrackDelta::Idle => Some(IDLE_PID_HEX.to_string()),
            TrackDelta::Same { title } => {
              if title.is_some() {
                entry.track_title = title;
              }
              entry.track_pid_hex.clone()
            }
            TrackDelta::Change { key, title } => {
              entry.track_pid_hex = Some(key.clone());
              entry.track_title = title;
              drop(guard);
              self.pending_art.clear(address).await;
              Some(key)
            }
          }
        };
        if let Some(pid_hex) = pid_hex.as_deref()
          && pid_hex != IDLE_PID_HEX
          && let Some(transfer_id) = update.media_item.as_ref().and_then(|m| m.artwork_id)
        {
          self.pending_art.mark(address, transfer_id, pid_hex.to_string()).await;
        }
        let lib_update = translate_now_playing(update, pid_hex.as_deref());
        tracing::debug!(%address, ?lib_update, "iAP2 now-playing delta");
        if let Err(err) = self.state.player.apply_now_playing(lib_update).await {
          tracing::warn!(%address, ?err, "failed to apply iAP2 now-playing delta");
        }
      }
      SessionEvent::ArtworkBytes { transfer_id, bytes } => {
        if bytes.is_empty() {
          tracing::debug!(%address, transfer_id, "iAP2 0-byte artwork; retaining pending entry");
          return;
        }
        let Some(track_key) = self.pending_art.take_if_matches(address, transfer_id).await else {
          tracing::debug!(
            %address,
            transfer_id,
            "iAP2 artwork bytes with no matching pending entry; dropping"
          );
          return;
        };
        let asset_id = iap2_art_asset_id(&track_key, &bytes);
        tracing::debug!(%address, asset_id = %asset_id, bytes = bytes.len(), "iAP2 artwork bytes -> AssetCache");
        if let Err(err) = self
          .state
          .assets
          .insert_internal(
            asset_id.clone(),
            bytes,
            Some("image/jpeg".to_string()),
            Retention::DISK_LRU,
          )
          .await
        {
          tracing::warn!(%address, ?err, "failed to insert iAP2 artwork into asset cache");
          return;
        }
        if let Err(err) = self.state.player.apply_artwork_id(asset_id).await {
          tracing::warn!(%address, ?err, "failed to apply iAP2 artwork id to player");
        }
      }
      SessionEvent::QueueSnapshotBytes { transfer_id, bytes } => {
        tracing::debug!(%address, transfer_id, bytes = bytes.len(), "iAP2 queue snapshot ignored; queue is companion-only");
      }
      SessionEvent::CallStateUpdate(update) => {
        tracing::debug!(%address, ?update, "iAP2 call-state update");
        if let Err(err) = self.state.telephony.apply_iap2_call_state(update).await {
          tracing::warn!(%address, ?err, "failed to apply iAP2 call-state update");
        }
      }
      SessionEvent::CommunicationsUpdate(update) => {
        tracing::debug!(%address, ?update, "iAP2 communications update");
        if let Err(err) = self.state.telephony.apply_iap2_communications(update).await {
          tracing::warn!(%address, ?err, "failed to apply iAP2 communications update");
        }
      }
      SessionEvent::DeviceName(update) => {
        tracing::info!(%address, name = %update.device_name, "iAP2 device name");
        self.state.peers.set_display_name(address, update.device_name).await;
      }
      SessionEvent::DeviceLanguage(update) => {
        tracing::info!(%address, language = %update.language, "iAP2 device language");
        self.state.peers.set_language(address, update.language).await;
      }
      SessionEvent::DeviceTime(update) => {
        tracing::info!(
          %address,
          unix_s = update.seconds_since_reference_date,
          tz_offset_minutes = update.tz_offset_minutes,
          dst_offset_minutes = update.dst_offset_minutes,
          "iAP2 device time"
        );
        if let Err(err) = self
          .state
          .time
          .apply_iap2_update(
            update.seconds_since_reference_date,
            update.tz_offset_minutes,
            update.dst_offset_minutes,
          )
          .await
        {
          tracing::warn!(%address, ?err, "failed to apply iAP2 time update");
        }
      }
      SessionEvent::DeviceUuid(update) => {
        tracing::info!(%address, uuid = %update.uuid, "iAP2 device UUID");
        self.state.peers.set_uuid(address, update.uuid).await;
      }
      SessionEvent::EaStreamOpened {
        stream_id,
        protocol_id,
        inbound_rx,
        outbound,
      } => {
        tracing::info!(%address, stream_id, protocol_id, "iAP2 EA stream opened");
        self
          .ea_gateway
          .notify_open(StreamOpened {
            address,
            stream_id,
            protocol_id,
            inbound_rx,
            outbound,
          })
          .await;
      }
      SessionEvent::EaStreamClosed { stream_id } => {
        tracing::info!(%address, stream_id, "iAP2 EA stream closed");
        self.ea_gateway.notify_closed(StreamClosed { address, stream_id }).await;
      }
      SessionEvent::LinkDown(reason) => {
        tracing::info!(%address, %reason, "iAP2 link down");
        self.stop_ea_keepalive(address).await;
        let _ = self.state.peers.set_iap2(address, PeerIap2Status::None).await;
        self.bluetooth.le.detach(address).await;
        self.np_checkpoint.lock().await.remove(&address);
        self.pending_art.clear(address).await;
        self.reconnect.kick(address).await;
      }
    }
  }
}

const EA_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const EA_KEEPALIVE_RTT: Duration = Duration::from_secs(7);
const EA_KEEPALIVE_MAX_MISSES: u32 = 3;

fn spawn_ea_keepalive(bluetooth: BluetoothMan, state: State, activity: EaActivity, address: Address) -> JoinHandle<()> {
  tokio::spawn(async move {
    let mut seq: u32 = 0;
    let mut armed = false;
    let mut misses: u32 = 0;
    let mut revoked: Option<PeerCompanionStatus> = None;
    let mut tick = tokio::time::interval(EA_KEEPALIVE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
      tick.tick().await;
      let started = Instant::now();
      let result = bluetooth
        .gateway_man
        .request_with_timeout(Some(address), KeepalivePing { seq }, EA_KEEPALIVE_RTT)
        .await;
      seq = seq.wrapping_add(1);
      match result {
        Ok(_ack) => {
          tracing::debug!(%address, rtt_ms = started.elapsed().as_millis() as u64, "ea keepalive rtt");
          armed = true;
          misses = 0;
          if let Some(restored) = revoked.take() {
            tracing::info!(%address, "ea session answering again; restoring the companion");
            state.peers.set_companion(address, restored).await;
          }
        }
        Err(_) => {
          if activity
            .last_activity(address)
            .is_some_and(|at| at.elapsed() < EA_KEEPALIVE_INTERVAL)
          {
            tracing::debug!(%address, seq, "ea keepalive unanswered while the stream is carrying data; not a miss");
          } else if armed && revoked.is_none() {
            misses += 1;
            if misses >= EA_KEEPALIVE_MAX_MISSES {
              tracing::warn!(%address, "ea session wedged (keepalive unanswered); dropping companion to iap2");
              let previous = state
                .peers
                .snapshot()
                .peers
                .get(&address)
                .map(|peer| peer.companion.clone())
                .unwrap_or_default();
              revoked = Some(previous);
              state.peers.set_companion(address, PeerCompanionStatus::None).await;
            }
          }
        }
      }
    }
  })
}

#[derive(Debug, PartialEq)]
enum TrackDelta {
  Unchanged,
  Idle,
  Same { title: Option<String> },
  Change { key: String, title: Option<String> },
}

fn track_delta(media: Option<&MediaItemAttributes>, current: &NowPlayingCheckpoint) -> TrackDelta {
  let Some(media) = media else {
    return TrackDelta::Unchanged;
  };
  let title = media.title.as_deref().filter(|t| !t.is_empty());
  let current_key = current.track_pid_hex.as_deref();
  match (media.persistent_id, title) {
    (Some(pid), _) if pid != 0 => {
      let key = format!("{pid:016x}");
      let title = title.map(str::to_string);
      if current_key == Some(key.as_str()) {
        TrackDelta::Same { title }
      } else {
        TrackDelta::Change { key, title }
      }
    }
    (pid, Some(t)) => {
      if current.track_title.as_deref() == Some(t) {
        TrackDelta::Same { title: None }
      } else if pid.is_none() && current_key.is_some_and(is_real_pid_key) && current.track_title.is_none() {
        TrackDelta::Same {
          title: Some(t.to_string()),
        }
      } else {
        TrackDelta::Change {
          key: nonmusic_key(t, media.artist.as_deref()),
          title: Some(t.to_string()),
        }
      }
    }
    (Some(0), None) => TrackDelta::Idle,
    _ => TrackDelta::Unchanged,
  }
}

fn is_real_pid_key(key: &str) -> bool {
  key.len() == 16 && key != IDLE_PID_HEX && key.bytes().all(|b| b.is_ascii_hexdigit())
}

fn nonmusic_key(title: &str, artist: Option<&str>) -> String {
  use std::hash::{DefaultHasher, Hash, Hasher};
  let mut hasher = DefaultHasher::new();
  title.hash(&mut hasher);
  if let Some(a) = artist {
    a.hash(&mut hasher);
  }
  format!("{NONMUSIC_PREFIX}{:016x}", hasher.finish())
}

fn translate_now_playing(update: Iap2NowPlayingUpdate, track_key: Option<&str>) -> NowPlayingUpdate {
  NowPlayingUpdate {
    media_item: update.media_item.map(|m| translate_media_item(m, track_key)),
    playback: update.playback.map(translate_playback),
  }
}

fn translate_media_item(media: MediaItemAttributes, track_key: Option<&str>) -> MediaItemUpdate {
  MediaItemUpdate {
    persistent_id: track_key.map(|key| format!("iap2:track:{key}")),
    title: media.title,
    album: media.album,
    album_uri: None,
    album_artist: media.album_artist,
    artist: media.artist,
    artist_uri: None,
    liked: media.liked,
    artwork_id: None,
    duration_ms: media.duration_ms,
    media_types: media.media_types.map(translate_media_types),
    track_number: media.track_number,
    track_count: media.track_count,
    is_like_supported: media.like_supported,
    is_ban_supported: media.ban_supported,
    is_banned: media.banned,
    is_resident_on_device: media.resident_on_device,
    chapter_count: media.chapter_count,
  }
}

fn translate_playback(play: PlaybackAttributes) -> PlaybackUpdate {
  PlaybackUpdate {
    playing: play.state.map(|s| matches!(s, PlaybackState::Playing)),
    position_ms: play.position_ms,
    shuffle: play.shuffle_mode.map(|m| m.is_on()),
    shuffle_mode: play.shuffle_mode.map(translate_shuffle),
    repeat: play.repeat.map(translate_repeat),
    app_bundle: play.app_bundle,
    app_display_name: play.app_display_name,
    queue_index: play.queue_index,
    queue_count: play.queue_count,
    queue_chapter_index: play.queue_chapter_index,
    playback_speed: play.playback_speed_hundredths.map(|h| f32::from(h) / 100.0),
    set_elapsed_time_available: play.set_elapsed_time_available,
    queue_list_avail: None,
    apple_music_radio_ad: play.apple_music_radio_ad,
    apple_music_radio_station_name: play.apple_music_radio_station_name,
  }
}

fn translate_repeat(mode: RepeatMode) -> libbridgething::RepeatMode {
  match mode {
    RepeatMode::Off => libbridgething::RepeatMode::Off,
    RepeatMode::Track => libbridgething::RepeatMode::One,
    RepeatMode::All => libbridgething::RepeatMode::All,
  }
}

fn translate_shuffle(mode: Iap2ShuffleMode) -> LibShuffleMode {
  match mode {
    Iap2ShuffleMode::Off => LibShuffleMode::Off,
    Iap2ShuffleMode::Songs => LibShuffleMode::Songs,
    Iap2ShuffleMode::Albums => LibShuffleMode::Albums,
  }
}

fn translate_media_types(types: Vec<MediaTypeKind>) -> Vec<LibMediaType> {
  types
    .into_iter()
    .map(|t| match t {
      MediaTypeKind::Music => LibMediaType::Music,
      MediaTypeKind::Podcast => LibMediaType::Podcast,
      MediaTypeKind::AudioBook => LibMediaType::AudioBook,
    })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn addr() -> Address {
    "AA:BB:CC:DD:EE:FF".parse().unwrap()
  }

  fn checkpoint(key: Option<&str>, title: Option<&str>) -> NowPlayingCheckpoint {
    NowPlayingCheckpoint {
      track_pid_hex: key.map(str::to_string),
      track_title: title.map(str::to_string),
      app_bundle: None,
    }
  }

  fn changed_key(delta: TrackDelta) -> String {
    match delta {
      TrackDelta::Change { key, .. } => key,
      other => panic!("expected a track change, got {other:?}"),
    }
  }

  #[tokio::test]
  async fn pending_take_matches_transfer_id() {
    let pending = Iap2PendingArt::new();
    pending.mark(addr(), 5, "abcd".to_string()).await;
    assert!(pending.is_pending("iap2/art/abcd/00ff00ff00ff00ff").await);
    assert_eq!(pending.take_if_matches(addr(), 5).await, Some("abcd".to_string()));
    assert!(!pending.is_pending("iap2/art/abcd/00ff00ff00ff00ff").await);
  }

  #[tokio::test]
  async fn pending_rejects_mismatched_transfer_id() {
    let pending = Iap2PendingArt::new();
    pending.mark(addr(), 5, "abcd".to_string()).await;
    assert_eq!(pending.take_if_matches(addr(), 4).await, None);
    assert!(pending.is_pending("iap2/art/abcd/1").await);
    assert!(
      !pending.is_pending("iap2/art/abcdef/1").await,
      "prefix must match a whole key"
    );
  }

  #[tokio::test]
  async fn mark_replaces_prior_entry_for_same_address() {
    let pending = Iap2PendingArt::new();
    pending.mark(addr(), 5, "old".to_string()).await;
    pending.mark(addr(), 7, "new".to_string()).await;
    assert!(!pending.is_pending("iap2/art/old/5").await);
    assert!(pending.is_pending("iap2/art/new/7").await);
    assert_eq!(pending.take_if_matches(addr(), 7).await, Some("new".to_string()));
  }

  #[tokio::test]
  async fn clear_drops_all_for_address() {
    let pending = Iap2PendingArt::new();
    pending.mark(addr(), 5, "abcd".to_string()).await;
    pending.clear(addr()).await;
    assert!(!pending.is_pending("iap2/art/abcd/5").await);
    assert_eq!(pending.take_if_matches(addr(), 5).await, None);
  }

  #[test]
  fn asset_id_is_track_key_plus_content_hash() {
    let a = iap2_art_asset_id("abcd", &[1, 2, 3]);
    assert!(a.starts_with("iap2/art/abcd/"));
    assert_eq!(a, iap2_art_asset_id("abcd", &[1, 2, 3]), "same bytes, same id");
    assert_ne!(a, iap2_art_asset_id("abcd", &[4, 5, 6]), "new bytes, new id");
  }

  #[test]
  fn pid_round_trips_canonically_single_prefix() {
    let media = MediaItemAttributes {
      persistent_id: Some(0x3242_5b9c_9dd6_28f8),
      title: Some("Side of Town".to_string()),
      ..Default::default()
    };
    let key = changed_key(track_delta(Some(&media), &checkpoint(None, None)));
    assert_eq!(key, "32425b9c9dd628f8", "key is bare hex, no prefix");

    let lib_update = translate_now_playing(
      Iap2NowPlayingUpdate {
        media_item: Some(media),
        playback: None,
      },
      Some(&key),
    );
    let lib_pid = lib_update.media_item.as_ref().and_then(|m| m.persistent_id.clone());
    assert_eq!(lib_pid.as_deref(), Some("iap2:track:32425b9c9dd628f8"), "single prefix");
  }

  #[test]
  fn pidless_title_fragment_inherits_active_real_pid() {
    let pid_frag = MediaItemAttributes {
      persistent_id: Some(13687273467863668569),
      duration_ms: Some(183127),
      album: Some("Feel".to_string()),
      track_number: Some(1),
      ..Default::default()
    };
    let key1 = changed_key(track_delta(Some(&pid_frag), &checkpoint(None, None)));
    assert_eq!(key1, "bdf2f6b363a49759");

    let title_frag = MediaItemAttributes {
      persistent_id: None,
      title: Some("Feel Real Pretty".to_string()),
      artist: Some("Pretty".to_string()),
      ..Default::default()
    };
    let delta = track_delta(Some(&title_frag), &checkpoint(Some(&key1), None));
    assert_eq!(
      delta,
      TrackDelta::Same {
        title: Some("Feel Real Pretty".to_string())
      },
      "pid-less title fragment sticks to the active real pid while its title is unknown"
    );
  }

  #[test]
  fn pidless_title_change_after_titled_pid_track_is_a_new_track() {
    let title_frag = MediaItemAttributes {
      persistent_id: None,
      title: Some("Sanguirush".to_string()),
      artist: Some("Cult of the Sun".to_string()),
      ..Default::default()
    };
    let key = changed_key(track_delta(
      Some(&title_frag),
      &checkpoint(Some("bdf2f6b363a49759"), Some("Feel Real Pretty")),
    ));
    assert!(
      key.starts_with(NONMUSIC_PREFIX),
      "a differing title is a track change, not sticky"
    );
  }

  #[test]
  fn pid_zero_with_title_never_attaches_to_real_pid() {
    let media = MediaItemAttributes {
      persistent_id: Some(0),
      title: Some("Big Buck Bunny".to_string()),
      ..Default::default()
    };
    let key = changed_key(track_delta(Some(&media), &checkpoint(Some("bdf2f6b363a49759"), None)));
    assert!(key.starts_with(NONMUSIC_PREFIX), "pid 0 declares a non-library source");
  }

  #[test]
  fn same_title_with_artist_arriving_later_keeps_the_key() {
    let first = MediaItemAttributes {
      title: Some("Some Episode".to_string()),
      ..Default::default()
    };
    let key = changed_key(track_delta(Some(&first), &checkpoint(None, None)));

    let second = MediaItemAttributes {
      title: Some("Some Episode".to_string()),
      artist: Some("Host".to_string()),
      ..Default::default()
    };
    let delta = track_delta(Some(&second), &checkpoint(Some(&key), Some("Some Episode")));
    assert_eq!(
      delta,
      TrackDelta::Same { title: None },
      "a re-sent title is not a new track"
    );
  }

  #[test]
  fn idle_shaped_delta_leaves_the_checkpoint_alone() {
    let idle = MediaItemAttributes {
      persistent_id: Some(0),
      title: Some(String::new()),
      duration_ms: Some(596_000),
      ..Default::default()
    };
    let delta = track_delta(Some(&idle), &checkpoint(Some("nonmusic-abc"), Some("Big Buck Bunny")));
    assert_eq!(delta, TrackDelta::Idle);
  }

  #[test]
  fn pidless_title_with_no_active_pid_is_nonmusic() {
    let title_frag = MediaItemAttributes {
      persistent_id: None,
      title: Some("Some Episode".to_string()),
      artist: Some("Host".to_string()),
      ..Default::default()
    };
    let key = changed_key(track_delta(Some(&title_frag), &checkpoint(None, None)));
    assert!(key.starts_with(NONMUSIC_PREFIX), "no active pid -> nonmusic identity");
  }

  #[test]
  fn pidless_source_distinguishes_tracks_by_title() {
    let first = changed_key(track_delta(
      Some(&MediaItemAttributes {
        title: Some("Ep 1".to_string()),
        ..Default::default()
      }),
      &checkpoint(None, None),
    ));
    let second = changed_key(track_delta(
      Some(&MediaItemAttributes {
        title: Some("Ep 2".to_string()),
        ..Default::default()
      }),
      &checkpoint(Some(&first), Some("Ep 1")),
    ));
    assert_ne!(first, second, "pid-less title change is a new track, not sticky");
  }

  #[test]
  fn real_pid_change_overrides_active_pid() {
    let a = changed_key(track_delta(
      Some(&MediaItemAttributes {
        persistent_id: Some(0xaaaa),
        ..Default::default()
      }),
      &checkpoint(None, None),
    ));
    let b = changed_key(track_delta(
      Some(&MediaItemAttributes {
        persistent_id: Some(0xbbbb),
        title: Some("New".to_string()),
        ..Default::default()
      }),
      &checkpoint(Some(&a), None),
    ));
    assert_ne!(a, b, "a genuine new pid is always a track change");
  }

  #[test]
  fn idle_pid_is_not_treated_as_a_real_pid_anchor() {
    assert!(!is_real_pid_key(IDLE_PID_HEX));
    let title_frag = MediaItemAttributes {
      title: Some("After Idle".to_string()),
      ..Default::default()
    };
    let key = changed_key(track_delta(Some(&title_frag), &checkpoint(Some(IDLE_PID_HEX), None)));
    assert!(
      key.starts_with(NONMUSIC_PREFIX),
      "idle is not a real-pid anchor to stick to"
    );
  }
}
