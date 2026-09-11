use std::{
  collections::{HashMap, HashSet},
  time::{Duration, Instant},
};

use libbridgething::{
  CompanionAuthorityScope, CurrentlyActiveApplication, MediaItem, MediaItemUpdate, NowPlayingUpdate, Playback,
  PlaybackContext, PlaybackOptions, PlaybackState, PlaybackTarget, PlaybackUpdate, PlayerOptions,
  PlayerState as WirePlayerState, QueueItem, RepeatMode, Track,
  client::{PlayerQueueReply, PlayerStateReply},
  gateway::QueueSnapshot,
};

use super::is_synthetic_uri;
use crate::{authority::AuthorityRegistry, bluetooth::Address};

const TRANSPORT_INTENT_WINDOW: Duration = Duration::from_millis(1500);
const SEEK_INTENT_WINDOW: Duration = Duration::from_millis(1500);
const POSITION_RESYNC_TOLERANCE_MS: usize = 2000;
const RECENTLY_PLAYED_CAP: usize = 25;
const OPTIMISTIC_MATCH_DEPTH: usize = 3;

#[derive(Debug, Clone)]
pub struct PlayerState {
  authority: AuthorityRegistry,

  pub playing: bool,

  context: Option<PlaybackContext>,
  target: Option<PlaybackTarget>,

  position_anchor: Option<Instant>,
  pub position_ms: usize,
  pub playback_speed: f64,

  pub track: Option<Track>,

  pub options: PlaybackOptions,

  iap2_metadata: MediaItemUpdate,
  iap2_playback: PlaybackUpdate,
  iap2_playback_at: Option<Instant>,
  companion_metadata: MediaItemUpdate,
  companion_playback: PlaybackUpdate,
  companion_playback_at: Option<Instant>,
  companion_source: Option<Address>,

  companion_queue: Vec<QueueItem>,
  recently_played: Vec<QueueItem>,
  previous_track_uri: Option<String>,
  root_browse_gen: u64,

  present_ids: HashSet<String>,

  transport_intent: Option<TransportIntent>,
  seek_intent: Option<SeekIntent>,

  companion_owned: bool,

  position_resync: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PositionAnchor {
  base_ms: usize,
  at: Option<Instant>,
  playing: bool,
  duration_ms: Option<u32>,
}

impl PositionAnchor {
  pub fn position_now_ms(&self) -> usize {
    let Some(at) = self.at.filter(|_| self.playing) else {
      return self.base_ms;
    };
    let elapsed = Instant::now().saturating_duration_since(at).as_millis() as usize;
    let advanced = self.base_ms.saturating_add(elapsed);
    match self.duration_ms.filter(|d| *d > 0) {
      Some(duration) => advanced.min(duration as usize),
      None => advanced,
    }
  }
}

#[derive(Debug, Clone, Copy)]
struct TransportIntent {
  playing: bool,
  expires: Instant,
  mismatches: u8,
}

#[derive(Debug, Clone, Copy)]
struct SeekIntent {
  expires: Instant,
}

impl PlayerState {
  pub fn new(authority: AuthorityRegistry) -> Self {
    Self {
      authority,

      playing: false,

      context: None,
      target: None,

      position_ms: 0,
      position_anchor: None,
      playback_speed: 1.0,

      track: None,

      options: PlaybackOptions::default(),

      iap2_metadata: MediaItemUpdate::default(),
      iap2_playback: PlaybackUpdate::default(),
      iap2_playback_at: None,
      companion_metadata: MediaItemUpdate::default(),
      companion_playback: PlaybackUpdate::default(),
      companion_playback_at: None,
      companion_source: None,

      companion_queue: Vec::new(),
      recently_played: Vec::new(),
      previous_track_uri: None,
      root_browse_gen: 0,

      present_ids: HashSet::new(),

      transport_intent: None,
      seek_intent: None,

      companion_owned: false,

      position_resync: false,
    }
  }

  fn note_ownership(&mut self) -> bool {
    let owns = self.companion_playback_authoritative();
    let edge = owns != self.companion_owned;
    self.companion_owned = owns;
    if edge {
      self.position_resync = true;
    }
    edge
  }

  pub(crate) fn take_position_resync(&mut self) -> bool {
    std::mem::take(&mut self.position_resync)
  }

  pub(crate) fn is_present(&self, id: &str) -> bool {
    self.present_ids.contains(id)
  }

  pub(crate) fn note_asset_ready(&mut self, id: String) {
    self.present_ids.insert(id);
  }

  pub(crate) fn note_asset_cleared(&mut self, id: &str) {
    self.present_ids.remove(id);
  }

  fn accepts_companion_data(&self, addr: Address) -> bool {
    self.authority.primary().is_none_or(|primary| primary == addr)
  }

  fn companion_now_playing_authoritative(&self, scope: CompanionAuthorityScope) -> bool {
    self.authority.is_authoritative(scope) && !self.iap2_foreground_is_other_app()
  }

  pub(crate) fn companion_playback_authoritative(&self) -> bool {
    self.companion_now_playing_authoritative(CompanionAuthorityScope::NowPlayingPlayback)
  }

  fn iap2_foreground_is_other_app(&self) -> bool {
    let Some(iap2) = self.iap2_playback.app_bundle.as_deref().filter(|b| !b.is_empty()) else {
      return false;
    };
    match self.authority.companion_app_bundle() {
      Some(comp) => !comp.eq_ignore_ascii_case(iap2),
      None => false,
    }
  }

  fn iap2_describes_the_companion_item(&self) -> bool {
    match (
      self.companion_metadata.title.as_deref(),
      self.iap2_metadata.title.as_deref(),
    ) {
      (Some(companion), Some(iap2)) => companion.trim().eq_ignore_ascii_case(iap2.trim()),
      _ => false,
    }
  }

  fn iap2_bundle_matches_companion(&self) -> bool {
    let Some(iap2) = self.iap2_playback.app_bundle.as_deref().filter(|b| !b.is_empty()) else {
      return false;
    };
    self
      .authority
      .companion_app_bundle()
      .is_some_and(|comp| comp.eq_ignore_ascii_case(iap2))
  }

  fn try_optimistic_advance(&mut self) {
    if !self.companion_owned || !self.iap2_bundle_matches_companion() {
      return;
    }
    let Some(title) = self.iap2_metadata.title.clone().filter(|t| !t.trim().is_empty()) else {
      return;
    };
    let artist = self.iap2_metadata.artist.clone();
    if let Some(track) = self.effective_track()
      && norm_eq(&track.name, &title)
      && artists_agree(artist.as_deref(), Some(&track.artist.name))
    {
      return;
    }
    let promoted = {
      let current_id = self.track.as_ref().map(|t| t.id.as_str());
      let upcoming = match current_id.and_then(|id| {
        self
          .companion_queue
          .iter()
          .position(|q| (!q.uri.is_empty() && q.uri == id) || q.persistent_id.as_deref() == Some(id))
      }) {
        Some(pos) => &self.companion_queue[pos + 1..],
        None => &self.companion_queue[..],
      };
      upcoming
        .iter()
        .take(OPTIMISTIC_MATCH_DEPTH)
        .filter(|item| self.previous_track_uri.as_deref() != Some(item.uri.as_str()))
        .find(|item| iap2_track_matches_item(&title, artist.as_deref(), item))
        .cloned()
    };
    let Some(item) = promoted else {
      return;
    };
    tracing::info!(uri = %item.uri, %title, "optimistic advance: iap2 track change matches the held queue");
    self.companion_metadata = MediaItemUpdate {
      persistent_id: Some(item.uri),
      title: item.title,
      artist: item.artist,
      artist_uri: item.artist_uri,
      album: item.album,
      album_uri: item.album_uri,
      artwork_id: item.artwork_id,
      duration_ms: item.duration_ms,
      ..MediaItemUpdate::default()
    };
    self.companion_playback.playing = None;
    self.companion_playback.position_ms = None;
    self.companion_playback_at = None;
  }

  pub(crate) fn set_transport_intent(&mut self, playing: bool) {
    self.position_ms = self.current_position_ms();
    self.position_anchor = Some(Instant::now());
    self.playing = playing;
    self.transport_intent = Some(TransportIntent {
      playing,
      expires: Instant::now() + TRANSPORT_INTENT_WINDOW,
      mismatches: 0,
    });
  }

  fn active_transport_intent(&self) -> Option<bool> {
    let intent = self.transport_intent?;
    (Instant::now() < intent.expires).then_some(intent.playing)
  }

  pub(crate) fn set_seek_intent(&mut self, position_ms: u32) {
    self.position_ms = position_ms as usize;
    self.position_anchor = Some(Instant::now());
    self.seek_intent = Some(SeekIntent {
      expires: Instant::now() + SEEK_INTENT_WINDOW,
    });
  }

  pub(crate) fn position_anchor(&self) -> PositionAnchor {
    PositionAnchor {
      base_ms: self.position_ms,
      at: self.position_anchor,
      playing: self.playing,
      duration_ms: self.track.as_ref().map(|t| t.duration_ms),
    }
  }

  pub(crate) fn current_position_ms(&self) -> usize {
    if !self.playing {
      return self.position_ms;
    }
    let Some(anchor) = self.position_anchor else {
      return self.position_ms;
    };
    let elapsed = Instant::now().saturating_duration_since(anchor).as_millis() as usize;
    self.position_ms.saturating_add(elapsed)
  }

  fn seek_intent_active(&self) -> bool {
    self.seek_intent.is_some_and(|i| Instant::now() < i.expires)
  }

  pub(crate) fn apply_companion_queue(&mut self, addr: Address, snapshot: QueueSnapshot) {
    if !self.accepts_companion_data(addr) {
      return;
    }
    self.companion_source = Some(addr);
    let QueueSnapshot { order, items } = snapshot;
    let by_uri: HashMap<&str, &QueueItem> = items.iter().map(|q| (q.uri.as_str(), q)).collect();
    let mut rebuilt = Vec::with_capacity(order.len());
    for uri in &order {
      if let Some(item) = by_uri.get(uri.as_str()) {
        rebuilt.push((*item).clone());
      }
    }
    if rebuilt.len() != order.len() {
      tracing::warn!(
        ordered = order.len(),
        resolved = rebuilt.len(),
        "companion queue: ordered uris without an item in the snapshot were dropped"
      );
    }
    tracing::info!(items = rebuilt.len(), "companion queue applied");
    self.companion_queue = rebuilt;
  }

  pub(crate) fn note_rolled_off(&mut self, outgoing: QueueItem) {
    if is_synthetic_uri(&outgoing.uri) {
      return;
    }
    if self.recently_played.first().map(|q| &q.uri) != Some(&outgoing.uri) {
      self.recently_played.insert(0, outgoing);
      self.recently_played.truncate(RECENTLY_PLAYED_CAP);
    }
  }

  pub(crate) fn root_browse_gen(&self) -> u64 {
    self.root_browse_gen
  }

  pub(crate) fn note_library_changed(&mut self) {
    self.root_browse_gen = self.root_browse_gen.wrapping_add(1);
  }

  pub(crate) fn note_authority_changed(&mut self) {
    let edge = self.note_ownership();
    let merged_meta = self.merged_metadata();
    let merged_play = self.merged_playback();
    self.apply_merged(merged_meta, merged_play, edge);
  }

  pub(crate) fn reset_companion(&mut self, addr: Address) {
    if self.companion_source != Some(addr) {
      return;
    }
    self.companion_source = None;
    self.recently_played.clear();
    self.root_browse_gen = self.root_browse_gen.wrapping_add(1);
    self.companion_metadata = MediaItemUpdate::default();
    self.companion_playback = PlaybackUpdate::default();
    self.companion_playback_at = None;
    self.context = None;
    self.target = None;
    self.note_ownership();
    let merged_meta = self.merged_metadata();
    let merged_play = self.merged_playback();
    self.apply_merged(merged_meta, merged_play, true);
  }

  pub(crate) fn apply_companion_snapshot(&mut self, addr: Address, snapshot: WirePlayerState) {
    if !self.accepts_companion_data(addr) {
      return;
    }
    self.companion_source = Some(addr);
    let WirePlayerState {
      track,
      playback,
      queue: _,
      options,
      context,
      target,
    } = snapshot;

    self.context = context;
    self.target = target;

    self.companion_metadata = match track {
      Some(t) => MediaItemUpdate {
        persistent_id: t.persistent_id,
        title: t.title,
        album: t.album,
        album_uri: t.album_uri,
        album_artist: t.album_artist,
        artist: t.artist,
        artist_uri: t.artist_uri,
        liked: t.liked,
        artwork_id: t.artwork_id,
        duration_ms: t.duration_ms,
        media_types: t.media_types,
        track_number: t.track_number,
        track_count: t.track_count,
        is_like_supported: t.is_like_supported,
        is_ban_supported: t.is_ban_supported,
        is_banned: t.is_banned,
        is_resident_on_device: None,
        chapter_count: t.chapter_count,
      },
      None => MediaItemUpdate::default(),
    };

    let mut position_ms = playback.position_ms;
    if matches!(playback.state, PlaybackState::Playing)
      && let Some(age) = playback.position_age_ms.filter(|age| *age > 0)
    {
      position_ms = position_ms.saturating_add(age);
      if let Some(duration) = self.companion_metadata.duration_ms.filter(|d| *d > 0) {
        position_ms = position_ms.min(duration);
      }
    }

    self.companion_playback = PlaybackUpdate {
      playing: Some(matches!(playback.state, PlaybackState::Playing)),
      position_ms: Some(position_ms),
      shuffle: Some(playback.shuffle),
      shuffle_mode: playback.shuffle_mode,
      repeat: Some(playback.repeat),
      app_bundle: None,
      app_display_name: None,
      queue_index: playback.queue_index,
      queue_count: playback.queue_count,
      queue_chapter_index: playback.queue_chapter_index,
      playback_speed: Some(options.speed),
      set_elapsed_time_available: playback.set_elapsed_time_available,
      queue_list_avail: playback.queue_list_avail,
      apple_music_radio_ad: playback.apple_music_radio_ad,
      apple_music_radio_station_name: None,
    };
    self.companion_playback_at = Some(Instant::now());

    let edge = self.note_ownership();
    let merged_meta = self.merged_metadata();
    let merged_play = self.merged_playback();
    self.apply_merged(merged_meta, merged_play, self.companion_owned || edge);
  }

  pub(crate) fn apply_now_playing(&mut self, update: NowPlayingUpdate) {
    let NowPlayingUpdate {
      mut media_item,
      mut playback,
    } = update;

    if is_idle_sentinel(media_item.as_ref()) {
      let duration = media_item.as_ref().and_then(|m| m.duration_ms).filter(|d| *d > 0);
      playback = None;
      media_item = duration.map(|d| MediaItemUpdate {
        duration_ms: Some(d),
        ..MediaItemUpdate::default()
      });
      if media_item.is_none() {
        return;
      }
    }

    if let Some(media) = media_item {
      if let Some(ref new_pid) = media.persistent_id
        && self.iap2_metadata.persistent_id.as_ref() != Some(new_pid)
      {
        self.iap2_metadata = MediaItemUpdate::default();
        self.iap2_playback.position_ms = Some(0);
        self.iap2_playback_at = Some(Instant::now());
      }
      accumulate_media(&mut self.iap2_metadata, media);
    }
    if let Some(play) = playback {
      if play.position_ms.is_some() {
        self.iap2_playback_at = Some(Instant::now());
      }
      accumulate_playback(&mut self.iap2_playback, play);
    }

    let edge = self.note_ownership();
    self.try_optimistic_advance();
    let merged_meta = self.merged_metadata();
    let mut merged_play = self.merged_playback();
    if self.companion_owned && !edge {
      merged_play = self.companion_fallthrough(merged_play);
    }

    self.apply_merged(merged_meta, merged_play, true);
  }

  fn companion_fallthrough(&self, merged: PlaybackUpdate) -> PlaybackUpdate {
    PlaybackUpdate {
      playing: merged.playing.filter(|_| self.companion_playback.playing.is_none()),
      position_ms: merged
        .position_ms
        .filter(|_| self.companion_playback.position_ms.is_none()),
      shuffle: merged.shuffle.filter(|_| self.companion_playback.shuffle.is_none()),
      repeat: merged.repeat.filter(|_| self.companion_playback.repeat.is_none()),
      ..merged
    }
  }

  pub(crate) fn apply_artwork_id(&mut self, asset_id: String) {
    self.iap2_metadata.artwork_id = Some(asset_id);

    let edge = self.note_ownership();
    let merged_meta = self.merged_metadata();
    let mut merged_play = self.merged_playback();
    if self.companion_owned && !edge {
      merged_play = self.companion_fallthrough(merged_play);
    }
    self.apply_merged(merged_meta, merged_play, true);
  }

  fn merged_metadata(&self) -> MediaItemUpdate {
    if !self.companion_now_playing_authoritative(CompanionAuthorityScope::NowPlayingMetadata) {
      return self.iap2_metadata.clone();
    }
    let companion = self.companion_metadata.clone();
    let iap2 = if self.iap2_describes_the_companion_item() {
      self.iap2_metadata.clone()
    } else {
      MediaItemUpdate::default()
    };
    MediaItemUpdate {
      persistent_id: companion.persistent_id.or(iap2.persistent_id),
      title: companion.title.or(iap2.title),
      album: companion.album.or(iap2.album),
      album_uri: companion.album_uri.or(iap2.album_uri),
      album_artist: companion.album_artist.or(iap2.album_artist),
      artist: companion.artist.or(iap2.artist),
      artist_uri: companion.artist_uri.or(iap2.artist_uri),
      liked: companion.liked.or(iap2.liked),
      artwork_id: companion.artwork_id,
      duration_ms: companion.duration_ms.or(iap2.duration_ms),
      media_types: companion.media_types.or(iap2.media_types),
      track_number: companion.track_number.or(iap2.track_number),
      track_count: companion.track_count.or(iap2.track_count),
      is_like_supported: companion.is_like_supported.or(iap2.is_like_supported),
      is_ban_supported: companion.is_ban_supported.or(iap2.is_ban_supported),
      is_banned: companion.is_banned.or(iap2.is_banned),
      is_resident_on_device: companion.is_resident_on_device.or(iap2.is_resident_on_device),
      chapter_count: companion.chapter_count.or(iap2.chapter_count),
    }
  }

  fn extrapolated(position: Option<u32>, staged_at: Option<Instant>, playing: Option<bool>) -> Option<u32> {
    let position = position?;
    if playing != Some(true) {
      return Some(position);
    }
    let Some(at) = staged_at else {
      return Some(position);
    };
    let aged = at.elapsed().as_millis().min(u32::MAX as u128) as u32;
    Some(position.saturating_add(aged))
  }

  fn merged_playback(&self) -> PlaybackUpdate {
    let companion_authoritative = self.companion_now_playing_authoritative(CompanionAuthorityScope::NowPlayingPlayback);
    let companion_position = Self::extrapolated(
      self.companion_playback.position_ms,
      self.companion_playback_at,
      self.companion_playback.playing,
    );
    let iap2_position = Self::extrapolated(
      self.iap2_playback.position_ms,
      self.iap2_playback_at,
      self.iap2_playback.playing,
    );
    if companion_authoritative {
      PlaybackUpdate {
        playing: self.companion_playback.playing.or(self.iap2_playback.playing),
        position_ms: companion_position.or(iap2_position),
        shuffle: self.companion_playback.shuffle.or(self.iap2_playback.shuffle),
        shuffle_mode: self.companion_playback.shuffle_mode.or(self.iap2_playback.shuffle_mode),
        repeat: self.companion_playback.repeat.or(self.iap2_playback.repeat),
        app_bundle: self
          .companion_playback
          .app_bundle
          .clone()
          .or_else(|| self.iap2_playback.app_bundle.clone()),
        app_display_name: self
          .companion_playback
          .app_display_name
          .clone()
          .or_else(|| self.iap2_playback.app_display_name.clone()),
        queue_index: self.companion_playback.queue_index.or(self.iap2_playback.queue_index),
        queue_count: self.companion_playback.queue_count.or(self.iap2_playback.queue_count),
        queue_chapter_index: self
          .companion_playback
          .queue_chapter_index
          .or(self.iap2_playback.queue_chapter_index),
        playback_speed: self
          .companion_playback
          .playback_speed
          .or(self.iap2_playback.playback_speed),
        set_elapsed_time_available: self
          .companion_playback
          .set_elapsed_time_available
          .or(self.iap2_playback.set_elapsed_time_available),
        queue_list_avail: self
          .companion_playback
          .queue_list_avail
          .or(self.iap2_playback.queue_list_avail),
        apple_music_radio_ad: self
          .companion_playback
          .apple_music_radio_ad
          .or(self.iap2_playback.apple_music_radio_ad),
        apple_music_radio_station_name: self
          .companion_playback
          .apple_music_radio_station_name
          .clone()
          .or_else(|| self.iap2_playback.apple_music_radio_station_name.clone()),
      }
    } else {
      let mut playback = self.iap2_playback.clone();
      playback.position_ms = iap2_position;
      playback
    }
  }

  fn apply_merged(&mut self, media: MediaItemUpdate, playback: PlaybackUpdate, apply_time: bool) {
    let has_identity = media.persistent_id.is_some() || media.title.is_some();
    let mut same_track = false;
    if self.track.is_some() || has_identity {
      same_track = match (
        self.track.as_ref().map(|t| t.id.as_str()),
        media.persistent_id.as_deref(),
      ) {
        (Some(existing), Some(new)) => existing == new,
        (None, _) | (_, None) => false,
      };
      if !same_track
        && media.persistent_id.is_some()
        && let Some(existing) = self.track.as_ref().map(|t| t.id.clone())
      {
        self.previous_track_uri = Some(existing);
      }
      let mut track = if same_track {
        self.track.clone().unwrap_or_default()
      } else {
        Track::default()
      };

      if let Some(id) = media.persistent_id {
        track.id = id;
      }
      if let Some(title) = media.title {
        track.name = title;
      }
      if let Some(album) = media.album {
        track.album = album.into();
      }
      if let Some(artist) = media.artist {
        track.artist = artist.clone().into();
        track.artists = vec![artist.into()];
      }
      track.image_id = media.artwork_id.unwrap_or_default();
      if let Some(duration) = media.duration_ms {
        track.duration_ms = duration;
      }
      if let Some(liked) = media.liked {
        track.saved = liked;
      }
      self.track = Some(track);
    }

    if !apply_time {
      return;
    }

    let mut accept_position = true;
    if let Some(playing) = playback.playing {
      match self.active_transport_intent() {
        Some(expected) if playing == expected => {
          self.playing = playing;
          self.transport_intent = None;
        }
        Some(_) => {
          let sustained = self
            .transport_intent
            .as_mut()
            .map(|i| {
              i.mismatches = i.mismatches.saturating_add(1);
              i.mismatches >= 2
            })
            .unwrap_or(true);
          if sustained {
            self.playing = playing;
            self.transport_intent = None;
          } else {
            accept_position = false;
          }
        }
        None => {
          self.playing = playing;
        }
      }
    } else if self.active_transport_intent().is_some() {
      accept_position = false;
    }
    if accept_position && let Some(position) = playback.position_ms {
      if self.seek_intent_active() {
        // Stale pre-seek position from iOS
      } else {
        let position = position as usize;
        let current = self.current_position_ms();
        let duration_known = self.track.as_ref().is_some_and(|t| t.duration_ms > 0);
        let riding = same_track
          && self.playing
          && duration_known
          && self.position_anchor.is_some()
          && position.abs_diff(current) <= POSITION_RESYNC_TOLERANCE_MS;

        let frozen_duplicate = same_track
          && self.playing
          && self.position_anchor.is_some()
          && position == self.position_ms
          && position < current;

        if (riding && position < current) || frozen_duplicate {
          // stale backward tick; ignore
        } else {
          if !riding {
            self.position_resync = true;
          }
          self.seek_intent = None;
          self.position_ms = position;
          self.position_anchor = Some(Instant::now());
        }
      }
    }
    if let Some(shuffle) = playback.shuffle {
      self.options.shuffle = shuffle;
    }
    if let Some(repeat) = playback.repeat {
      self.options.repeat = repeat;
    }
  }

  pub fn iap2_shuffle(&self) -> Option<bool> {
    self.iap2_playback.shuffle
  }

  pub fn iap2_app_bundle(&self) -> Option<String> {
    self.iap2_playback.app_bundle.clone().filter(|b| !b.is_empty())
  }

  pub fn iap2_playing(&self) -> Option<bool> {
    self.iap2_playback.playing
  }

  pub fn iap2_repeat_mode(&self) -> Option<RepeatMode> {
    self.iap2_playback.repeat
  }

  pub fn iap2_set_elapsed_time_available(&self) -> Option<bool> {
    self.iap2_playback.set_elapsed_time_available
  }

  pub fn replies(&self) -> (PlayerStateReply, PlayerQueueReply) {
    let merged_meta = self.merged_metadata();
    let merged_play = self.merged_playback();
    let companion_authoritative = self.companion_now_playing_authoritative(CompanionAuthorityScope::NowPlayingMetadata);
    let head_art = merged_meta.artwork_id.clone().filter(|s| !s.is_empty());
    let raw_upcoming = if companion_authoritative {
      self.companion_queue.clone()
    } else {
      Vec::new()
    };
    let effective = self.effective_track();
    let context = effective.and_then(|_| companion_authoritative.then(|| self.context.clone()).flatten());
    let target = effective.and_then(|_| companion_authoritative.then(|| self.target.clone()).flatten());

    let queue_current = effective.map(|t| build_queue_item(t, &merged_meta, head_art.clone()));
    let next = derive_next(raw_upcoming, queue_current.as_ref());

    let media_item = effective.map(|t| build_media_item(t, &merged_meta, head_art.clone()));
    let playback = Playback {
      state: if self.playing {
        PlaybackState::Playing
      } else {
        PlaybackState::Paused
      },
      position_ms: u32::try_from(self.current_position_ms()).unwrap_or(u32::MAX),
      position_age_ms: None,
      shuffle: self.options.shuffle,
      shuffle_mode: merged_play.shuffle_mode,
      repeat: self.options.repeat,
      queue_index: merged_play.queue_index,
      queue_count: merged_play.queue_count,
      queue_chapter_index: merged_play.queue_chapter_index,
      set_elapsed_time_available: merged_play.set_elapsed_time_available,
      queue_list_avail: merged_play.queue_list_avail,
      apple_music_radio_ad: merged_play.apple_music_radio_ad,
    };
    let options = PlayerOptions {
      speed: merged_play.playback_speed.unwrap_or(self.playback_speed as f32),
      crossfade_ms: None,
    };

    let state = PlayerStateReply {
      active_app: self.active_app(),
      state: WirePlayerState {
        track: media_item,
        playback,
        queue: next.clone(),
        options,
        context,
        target,
      },
    };

    let queue = PlayerQueueReply {
      current: queue_current,
      items: next,
      previous: self.recently_played.clone(),
    };

    (state, queue)
  }

  pub fn current_artwork_id(&self) -> Option<String> {
    self.effective_track()?;
    self.merged_metadata().artwork_id.filter(|s| !s.is_empty())
  }

  pub fn active_app(&self) -> Option<CurrentlyActiveApplication> {
    let track = self.effective_track()?;
    if !is_synthetic_uri(&track.id) {
      return None;
    }
    let play = self.merged_playback();
    let bundle = play.app_bundle.clone();
    let name = play
      .app_display_name
      .clone()
      .or_else(|| bundle.clone())
      .unwrap_or_else(|| "phone".to_string());
    Some(CurrentlyActiveApplication {
      id: bundle.unwrap_or_default(),
      name,
    })
  }

  fn effective_track(&self) -> Option<&Track> {
    let track = self.track.as_ref()?;
    if track.id.ends_with("0000000000000000") && track.name.is_empty() {
      return None;
    }
    Some(track)
  }

  #[cfg(test)]
  fn age_clocks(&mut self, by: Duration) {
    if let Some(anchor) = self.position_anchor.as_mut() {
      *anchor -= by;
    }
    if let Some(at) = self.companion_playback_at.as_mut() {
      *at -= by;
    }
    if let Some(at) = self.iap2_playback_at.as_mut() {
      *at -= by;
    }
  }
}

fn derive_next(upcoming: Vec<QueueItem>, current: Option<&QueueItem>) -> Vec<QueueItem> {
  let Some(cur) = current else {
    return upcoming;
  };
  match upcoming.iter().position(|q| queue_items_match(q, cur)) {
    Some(pos) => upcoming[pos + 1..].to_vec(),
    None => upcoming,
  }
}

fn queue_items_match(a: &QueueItem, b: &QueueItem) -> bool {
  (!a.uri.is_empty() && a.uri == b.uri) || (a.persistent_id.is_some() && a.persistent_id == b.persistent_id)
}

fn norm_eq(a: &str, b: &str) -> bool {
  a.trim().eq_ignore_ascii_case(b.trim())
}

fn artists_agree(a: Option<&str>, b: Option<&str>) -> bool {
  match (
    a.map(str::trim).filter(|s| !s.is_empty()),
    b.map(str::trim).filter(|s| !s.is_empty()),
  ) {
    (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
    _ => true,
  }
}

fn iap2_track_matches_item(title: &str, artist: Option<&str>, item: &QueueItem) -> bool {
  item.title.as_deref().is_some_and(|t| norm_eq(t, title)) && artists_agree(artist, item.artist.as_deref())
}

fn build_queue_item(track: &Track, merged: &MediaItemUpdate, art_id: Option<String>) -> QueueItem {
  QueueItem {
    uri: track.id.clone(),
    title: merged.title.clone(),
    artist: merged.artist.clone(),
    artist_uri: merged.artist_uri.clone(),
    album: merged.album.clone(),
    album_uri: merged.album_uri.clone(),
    artwork_id: art_id,
    duration_ms: merged.duration_ms,
    persistent_id: Some(track.id.clone()),
    queued: false,
  }
}

fn build_media_item(track: &Track, merged: &MediaItemUpdate, art_id: Option<String>) -> MediaItem {
  let uri = if is_synthetic_uri(&track.id) {
    None
  } else {
    Some(track.id.clone())
  };
  MediaItem {
    uri,
    persistent_id: Some(track.id.clone()),
    title: merged.title.clone(),
    album: merged.album.clone(),
    album_uri: merged.album_uri.clone(),
    album_artist: merged.album_artist.clone(),
    artist: merged.artist.clone(),
    artist_uri: merged.artist_uri.clone(),
    liked: merged.liked,
    artwork_id: art_id,
    duration_ms: merged.duration_ms,
    media_types: merged.media_types.clone(),
    track_number: merged.track_number,
    track_count: merged.track_count,
    is_like_supported: merged.is_like_supported,
    is_ban_supported: merged.is_ban_supported,
    is_banned: merged.is_banned,
    chapter_count: merged.chapter_count,
  }
}

fn is_idle_sentinel(media: Option<&MediaItemUpdate>) -> bool {
  media.is_some_and(|m| {
    m.persistent_id
      .as_deref()
      .is_some_and(|p| p.ends_with("0000000000000000"))
      && m.title.as_deref() == Some("")
  })
}

fn accumulate_media(target: &mut MediaItemUpdate, src: MediaItemUpdate) {
  if src.persistent_id.is_some() {
    target.persistent_id = src.persistent_id;
  }
  if src.title.is_some() {
    target.title = src.title;
  }
  if src.album.is_some() {
    target.album = src.album;
  }
  if src.album_uri.is_some() {
    target.album_uri = src.album_uri;
  }
  if src.album_artist.is_some() {
    target.album_artist = src.album_artist;
  }
  if src.artist.is_some() {
    target.artist = src.artist;
  }
  if src.artist_uri.is_some() {
    target.artist_uri = src.artist_uri;
  }
  if src.liked.is_some() {
    target.liked = src.liked;
  }
  if src.artwork_id.is_some() {
    target.artwork_id = src.artwork_id;
  }
  if src.duration_ms.is_some() {
    target.duration_ms = src.duration_ms;
  }
  if src.media_types.is_some() {
    target.media_types = src.media_types;
  }
  if src.track_number.is_some() {
    target.track_number = src.track_number;
  }
  if src.track_count.is_some() {
    target.track_count = src.track_count;
  }
  if src.is_like_supported.is_some() {
    target.is_like_supported = src.is_like_supported;
  }
  if src.is_ban_supported.is_some() {
    target.is_ban_supported = src.is_ban_supported;
  }
  if src.is_banned.is_some() {
    target.is_banned = src.is_banned;
  }
  if src.is_resident_on_device.is_some() {
    target.is_resident_on_device = src.is_resident_on_device;
  }
  if src.chapter_count.is_some() {
    target.chapter_count = src.chapter_count;
  }
}

fn accumulate_playback(target: &mut PlaybackUpdate, src: PlaybackUpdate) {
  if src.playing.is_some() {
    target.playing = src.playing;
  }
  if src.position_ms.is_some() {
    target.position_ms = src.position_ms;
  }
  if src.shuffle.is_some() {
    target.shuffle = src.shuffle;
  }
  if src.shuffle_mode.is_some() {
    target.shuffle_mode = src.shuffle_mode;
  }
  if src.repeat.is_some() {
    target.repeat = src.repeat;
  }
  if src.app_bundle.is_some() {
    target.app_bundle = src.app_bundle;
  }
  if src.app_display_name.is_some() {
    target.app_display_name = src.app_display_name;
  }
  if src.queue_index.is_some() {
    target.queue_index = src.queue_index;
  }
  if src.queue_count.is_some() {
    target.queue_count = src.queue_count;
  }
  if src.queue_chapter_index.is_some() {
    target.queue_chapter_index = src.queue_chapter_index;
  }
  if src.playback_speed.is_some() {
    target.playback_speed = src.playback_speed;
  }
  if src.set_elapsed_time_available.is_some() {
    target.set_elapsed_time_available = src.set_elapsed_time_available;
  }
  if src.queue_list_avail.is_some() {
    target.queue_list_avail = src.queue_list_avail;
  }
  if src.apple_music_radio_ad.is_some() {
    target.apple_music_radio_ad = src.apple_music_radio_ad;
  }
  if src.apple_music_radio_station_name.is_some() {
    target.apple_music_radio_station_name = src.apple_music_radio_station_name;
  }
}

#[cfg(test)]
mod tests {
  fn anchored(base_ms: usize, ago: Duration, playing: bool, duration_ms: Option<u32>) -> PositionAnchor {
    PositionAnchor {
      base_ms,
      at: Instant::now().checked_sub(ago),
      playing,
      duration_ms,
    }
  }

  #[test]
  fn a_read_between_pushes_advances_the_playhead() {
    let anchor = anchored(30_000, Duration::from_secs(40), true, Some(300_000));
    let position = anchor.position_now_ms();
    assert!(
      (69_000..=71_000).contains(&position),
      "forty quiet seconds should carry 30s to about 70s, got {position}"
    );
  }

  #[test]
  fn a_paused_playhead_does_not_run() {
    let anchor = anchored(30_000, Duration::from_secs(40), false, Some(300_000));
    assert_eq!(anchor.position_now_ms(), 30_000);
  }

  #[test]
  fn the_playhead_never_runs_past_the_track() {
    let anchor = anchored(30_000, Duration::from_secs(600), true, Some(180_000));
    assert_eq!(anchor.position_now_ms(), 180_000);
  }

  #[test]
  fn an_unknown_duration_still_advances() {
    let anchor = anchored(1_000, Duration::from_secs(10), true, None);
    assert!(anchor.position_now_ms() >= 10_500);
  }

  #[test]
  fn a_playhead_with_no_anchor_stands_still() {
    let anchor = PositionAnchor {
      base_ms: 4_242,
      at: None,
      playing: true,
      duration_ms: Some(300_000),
    };
    assert_eq!(anchor.position_now_ms(), 4_242);
  }

  use libbridgething::CompanionAuthorityScope;

  use super::*;

  fn iap2_track(persistent_id: &str, title: &str) -> NowPlayingUpdate {
    NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some(persistent_id.to_string()),
        title: Some(title.to_string()),
        ..MediaItemUpdate::default()
      }),
      playback: None,
    }
  }

  fn companion_snapshot(persistent_id: &str, title: &str, artwork_id: Option<&str>, playing: bool) -> WirePlayerState {
    WirePlayerState {
      track: Some(MediaItem {
        persistent_id: Some(persistent_id.to_string()),
        title: Some(title.to_string()),
        artwork_id: artwork_id.map(str::to_string),
        ..MediaItem::default()
      }),
      playback: Playback {
        state: if playing {
          PlaybackState::Playing
        } else {
          PlaybackState::Paused
        },
        ..Playback::default()
      },
      ..WirePlayerState::default()
    }
  }

  fn iap2_app(pid: &str, title: &str, app_bundle: &str, playing: bool) -> NowPlayingUpdate {
    NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some(pid.to_string()),
        title: Some(title.to_string()),
        ..MediaItemUpdate::default()
      }),
      playback: Some(PlaybackUpdate {
        playing: Some(playing),
        app_bundle: Some(app_bundle.to_string()),
        ..PlaybackUpdate::default()
      }),
    }
  }

  const COMPANION: Address = Address([0xC0, 0, 0, 0, 0, 1]);

  fn artwork_id_of(state: &PlayerState) -> Option<String> {
    state.replies().0.state.track.and_then(|t| t.artwork_id)
  }

  #[test]
  fn iap2_now_playing_does_not_emit_artwork_id_until_apply_artwork_id() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(iap2_track("iap2:track:abc", "Heart of Glass"));
    assert_eq!(artwork_id_of(&state), None);

    state.apply_artwork_id("iap2/art/abc/5".to_string());
    assert_eq!(artwork_id_of(&state), Some("iap2/art/abc/5".to_string()));
  }

  #[test]
  fn idle_persistent_id_zero_suppresses_track_emission() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some("iap2:track:0000000000000000".to_string()),
        title: Some(String::new()),
        ..MediaItemUpdate::default()
      }),
      playback: None,
    });
    assert!(state.replies().0.state.track.is_none());
    assert_eq!(state.current_artwork_id(), None);
  }

  #[test]
  fn pid_zero_with_real_title_emits_track() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(iap2_track(
      "iap2:track:0000000000000000",
      "99.9% Of Elden Ring Players CAN'T Beat This Mod",
    ));
    let track = state.replies().0.state.track.expect("track present");
    assert_eq!(
      track.title.as_deref(),
      Some("99.9% Of Elden Ring Players CAN'T Beat This Mod")
    );
    assert_eq!(track.persistent_id.as_deref(), Some("iap2:track:0000000000000000"));
  }

  #[test]
  fn track_change_clears_stale_iap2_art() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(iap2_track("iap2:track:a", "Track A"));
    state.apply_artwork_id("iap2/art/a/5".to_string());
    assert_eq!(artwork_id_of(&state), Some("iap2/art/a/5".to_string()));

    state.apply_now_playing(iap2_track("iap2:track:b", "Track B"));
    assert_eq!(artwork_id_of(&state), None);
  }

  #[test]
  fn companion_authoritative_clears_iap2_art_on_wire() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    state.apply_now_playing(iap2_track("track:a", "A"));
    state.apply_artwork_id("iap2/art/a/5".to_string());
    assert_eq!(artwork_id_of(&state), Some("iap2/art/a/5".to_string()));

    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    state.apply_companion_snapshot(COMPANION, companion_snapshot("track:a", "A", None, true));
    assert_eq!(
      artwork_id_of(&state),
      None,
      "companion authoritative without artwork_id must NOT leak iap2 art onto the wire"
    );
  }

  fn iap2_full(pid: &str, title: &str, app_bundle: &str) -> NowPlayingUpdate {
    NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some(pid.to_string()),
        title: Some(title.to_string()),
        artist: Some("Phone Artist".to_string()),
        album: Some("Phone Album".to_string()),
        duration_ms: Some(240_000),
        ..MediaItemUpdate::default()
      }),
      playback: Some(PlaybackUpdate {
        playing: Some(true),
        app_bundle: Some(app_bundle.to_string()),
        ..PlaybackUpdate::default()
      }),
    }
  }

  #[test]
  fn another_item_s_iap2_fields_never_fill_in_a_companion_track() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    state.apply_now_playing(iap2_full("iap2:track:1", "Spotify Song", "com.bridgething.gateway"));

    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.set_companion_app_bundle(COMPANION, Some("com.bridgething.gateway".into()));
    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot("https://radio.example/live", "Groove Salad", None, true),
    );

    let track = media(&state);
    assert_eq!(track.title.as_deref(), Some("Groove Salad"));
    assert_eq!(track.artist, None, "a stream has no artist to borrow from spotify");
    assert_eq!(track.album, None);
    assert_eq!(track.duration_ms, None);
  }

  #[test]
  fn the_same_item_s_iap2_fields_still_fill_in_a_companion_track() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    state.apply_now_playing(iap2_full("iap2:track:1", "Song", "com.spotify.client"));

    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));
    state.apply_companion_snapshot(COMPANION, companion_snapshot("spotify:track:1", "Song", None, true));

    let track = media(&state);
    assert_eq!(track.artist.as_deref(), Some("Phone Artist"));
    assert_eq!(track.album.as_deref(), Some("Phone Album"));
    assert_eq!(track.duration_ms, Some(240_000));
  }

  #[test]
  fn companion_release_restores_iap2_art() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    state.apply_now_playing(iap2_track("track:a", "A"));
    state.apply_artwork_id("iap2/art/a/5".to_string());

    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot("track:a", "A", Some("spotify/track/a/image"), true),
    );
    assert_eq!(artwork_id_of(&state), Some("spotify/track/a/image".to_string()));

    auth.release(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    state.apply_now_playing(iap2_track("track:a", "A"));
    assert_eq!(artwork_id_of(&state), Some("iap2/art/a/5".to_string()));
  }

  #[test]
  fn transport_gate_follows_the_audible_app() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".to_string()));
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    assert!(
      state.companion_playback_authoritative(),
      "companion owns playback with no other foreground"
    );

    state.apply_now_playing(iap2_app("vid", "Clip", "com.google.ios.youtube", true));
    assert!(!state.companion_playback_authoritative());

    state.apply_now_playing(iap2_app("vid", "", "", true));
    assert!(state.companion_playback_authoritative());

    state.apply_now_playing(iap2_app("track:a", "A", "com.spotify.client", true));
    assert!(state.companion_playback_authoritative());
  }

  #[test]
  fn idle_sentinel_does_not_clobber_held_track_or_art() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(iap2_track("iap2:track:a", "Track A"));
    state.apply_artwork_id("iap2/art/a/5".to_string());
    assert_eq!(artwork_id_of(&state), Some("iap2/art/a/5".to_string()));

    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some("iap2:track:0000000000000000".to_string()),
        title: Some(String::new()),
        ..MediaItemUpdate::default()
      }),
      playback: None,
    });

    assert_eq!(
      artwork_id_of(&state),
      Some("iap2/art/a/5".to_string()),
      "idle sentinel must not wipe held iap2 art"
    );
    let track = state.replies().0.state.track.expect("held track preserved");
    assert_eq!(track.persistent_id.as_deref(), Some("iap2:track:a"));
  }

  #[test]
  fn build_media_item_emits_none_for_empty_image_id() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(iap2_track("track:a", "A"));
    let track = state.replies().0.state.track.expect("track present");
    assert_eq!(track.artwork_id, None);
  }

  fn qitem(uri: &str, title: &str, artist: &str, art: Option<&str>, duration_ms: Option<u32>) -> QueueItem {
    QueueItem {
      uri: uri.to_string(),
      title: Some(title.to_string()),
      artist: Some(artist.to_string()),
      artist_uri: None,
      album: None,
      album_uri: None,
      artwork_id: art.map(str::to_string),
      duration_ms,
      persistent_id: None,
      queued: false,
    }
  }

  fn qsnap(items: Vec<QueueItem>) -> QueueSnapshot {
    QueueSnapshot {
      order: items.iter().map(|i| i.uri.clone()).collect(),
      items,
    }
  }

  fn media(state: &PlayerState) -> MediaItem {
    state.replies().0.state.track.expect("track present")
  }

  #[test]
  fn idle_sentinel_real_duration_is_absorbed_into_held_track() {
    let mut state = PlayerState::new(AuthorityRegistry::new());

    state.apply_now_playing(iap2_track(
      "iap2:track:nonmusic-57052159de5875d2",
      "What Happened To All The Ads?",
    ));
    assert_eq!(media(&state).duration_ms, None, "title delta carries no duration");

    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some("iap2:track:0000000000000000".to_string()),
        title: Some(String::new()),
        duration_ms: Some(1_044_700),
        ..MediaItemUpdate::default()
      }),
      playback: None,
    });

    let track = media(&state);
    assert_eq!(
      track.persistent_id.as_deref(),
      Some("iap2:track:nonmusic-57052159de5875d2"),
      "held track identity survives the sentinel"
    );
    assert_eq!(
      track.title.as_deref(),
      Some("What Happened To All The Ads?"),
      "held title is not clobbered by the sentinel's empty title"
    );
    assert_eq!(
      track.duration_ms,
      Some(1_044_700),
      "real duration carried on the zero-pid sentinel is absorbed into the held track"
    );
  }

  #[test]
  fn idle_sentinel_without_real_data_is_still_dropped() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(iap2_track("iap2:track:nonmusic-abc", "A Video"));

    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some("iap2:track:0000000000000000".to_string()),
        title: Some(String::new()),
        duration_ms: Some(0),
        ..MediaItemUpdate::default()
      }),
      playback: None,
    });

    assert_eq!(
      media(&state).persistent_id.as_deref(),
      Some("iap2:track:nonmusic-abc"),
      "pure idle sentinel leaves the held track untouched"
    );
  }

  #[test]
  fn synthetic_iap2_identity_without_companion_is_non_actionable() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(iap2_track("iap2:track:a", "Song"));
    let m = media(&state);
    assert_eq!(m.uri, None, "a synthetic iap2 identity exposes no actionable uri");
    assert_eq!(m.persistent_id.as_deref(), Some("iap2:track:a"));
    assert_eq!(m.artwork_id, None);
    assert_eq!(m.is_like_supported, None);
  }

  #[test]
  fn non_synthetic_companion_identity_is_actionable() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    state.apply_companion_snapshot(COMPANION, companion_snapshot("spotify:track:x", "Song", None, true));
    let m = media(&state);
    assert_eq!(
      m.uri.as_deref(),
      Some("spotify:track:x"),
      "a real provider uri is actionable"
    );
  }

  #[test]
  fn synthetic_uri_predicate() {
    assert!(is_synthetic_uri("iap2:track:abc"));
    assert!(!is_synthetic_uri("spotify:track:abc"));
  }

  #[test]
  fn companion_snapshot_drives_now_playing_when_authoritative() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));
    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot("spotify:track:x", "Spotify Song", Some("spotify/img/x"), true),
    );

    let m = media(&state);
    assert_eq!(m.uri.as_deref(), Some("spotify:track:x"));
    assert_eq!(m.title.as_deref(), Some("Spotify Song"));
    assert_eq!(m.artwork_id.as_deref(), Some("spotify/img/x"));
    assert_eq!(state.replies().0.state.playback.state, PlaybackState::Playing);
  }

  #[test]
  fn iap2_other_foreground_app_overrides_companion_authority() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));
    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot("spotify:track:x", "Spotify Song", Some("spotify/img/x"), true),
    );

    state.apply_now_playing(iap2_app("iap2:track:s", "Spotify Song", "com.spotify.client", true));
    assert_eq!(
      media(&state).uri.as_deref(),
      Some("spotify:track:x"),
      "same-app iap2 does not override"
    );

    state.apply_now_playing(iap2_app(
      "iap2:track:y",
      "YouTube Video",
      "com.google.ios.youtube",
      true,
    ));
    let m = media(&state);
    assert_eq!(
      m.title.as_deref(),
      Some("YouTube Video"),
      "a diverging foreground bundle hands now-playing to iap2"
    );
    assert_eq!(m.persistent_id.as_deref(), Some("iap2:track:y"));
    assert_eq!(m.uri, None, "the iap2 identity is synthetic");

    state.apply_now_playing(iap2_app("iap2:track:s2", "Spotify Song", "com.spotify.client", true));
    assert_eq!(
      media(&state).uri.as_deref(),
      Some("spotify:track:x"),
      "companion re-takes on the bundle returning"
    );
  }

  #[test]
  fn active_app_surfaces_only_for_non_spotify_now_playing() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));
    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot("spotify:track:x", "Spotify Song", Some("spotify/img/x"), true),
    );
    assert!(state.active_app().is_none(), "spotify now-playing is not other-media");

    state.apply_now_playing(iap2_app(
      "iap2:track:y",
      "YouTube Video",
      "com.google.ios.youtube",
      true,
    ));
    let app = state.active_app().expect("other-media app present");
    assert_eq!(app.id, "com.google.ios.youtube");
  }

  fn position_tick(position_ms: u32) -> NowPlayingUpdate {
    NowPlayingUpdate {
      media_item: None,
      playback: Some(PlaybackUpdate {
        position_ms: Some(position_ms),
        ..PlaybackUpdate::default()
      }),
    }
  }

  fn playing_track(pid: &str, position_ms: u32) -> NowPlayingUpdate {
    NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some(pid.into()),
        title: Some("Song".into()),
        duration_ms: Some(180_000),
        ..MediaItemUpdate::default()
      }),
      playback: Some(PlaybackUpdate {
        playing: Some(true),
        position_ms: Some(position_ms),
        ..PlaybackUpdate::default()
      }),
    }
  }

  #[test]
  fn routine_progress_tick_does_not_force_resync() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(playing_track("iap2:track:a", 1_000));
    assert!(state.take_position_resync(), "first position of a track resyncs");

    state.apply_now_playing(position_tick(1_200));
    assert!(
      !state.take_position_resync(),
      "a tick near the extrapolated playhead rides without a broadcast"
    );
  }

  #[test]
  fn position_jump_forces_resync() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(playing_track("iap2:track:a", 1_000));
    state.take_position_resync();

    state.apply_now_playing(position_tick(120_000));
    assert!(
      state.take_position_resync(),
      "a phone-side seek beyond tolerance resyncs the webapp"
    );
  }

  #[test]
  fn frozen_duplicate_position_resend_does_not_rewind() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(playing_track("iap2:track:a", 277));
    state.take_position_resync();

    state.age_clocks(Duration::from_secs(40));
    let extrapolated = state.replies().0.state.playback.position_ms;
    assert!(extrapolated >= 40_000, "playhead extrapolated while playing");

    state.apply_now_playing(position_tick(277));
    assert!(
      !state.take_position_resync(),
      "a frozen duplicate is not a seek and must not resync"
    );
    let after = state.replies().0.state.playback.position_ms;
    assert!(
      after >= extrapolated,
      "playhead never rewinds on a frozen duplicate: {after} < {extrapolated}"
    );
  }

  #[test]
  fn aged_companion_position_extrapolates_instead_of_rewinding() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));

    state.apply_companion_snapshot(COMPANION, companion_snapshot_pos("spotify:track:x", "X", true, 40_000));
    state.take_position_resync();
    state.age_clocks(Duration::from_secs(40));
    let live = state.replies().0.state.playback.position_ms;
    assert!(live >= 80_000, "playhead extrapolated while playing: {live}");

    let mut stale = companion_snapshot_pos("spotify:track:x", "X", true, 40_000);
    stale.playback.position_age_ms = Some(40_000);
    state.apply_companion_snapshot(COMPANION, stale);
    assert!(
      !state.take_position_resync(),
      "an age-anchored resend lands on live time and is not a seek"
    );
    let after = state.replies().0.state.playback.position_ms;
    assert!(
      after >= live.saturating_sub(POSITION_RESYNC_TOLERANCE_MS as u32),
      "playhead never rewinds on an aged resend: {after} < {live}"
    );
  }

  #[test]
  fn aged_position_clamps_to_duration() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));

    let mut stale = companion_snapshot_pos("spotify:track:x", "X", true, 190_000);
    stale.playback.position_age_ms = Some(600_000);
    state.apply_companion_snapshot(COMPANION, stale);
    let position = state.replies().0.state.playback.position_ms;
    assert!(position <= 200_000, "aged position clamps to duration: {position}");
  }

  #[test]
  fn backward_position_jitter_while_riding_is_ignored() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(playing_track("iap2:track:a", 1_000));
    state.take_position_resync();

    state.apply_now_playing(position_tick(1_500));
    let forward = state.replies().0.state.playback.position_ms;

    state.apply_now_playing(position_tick(900));
    assert!(!state.take_position_resync(), "backward jitter does not force a resync");
    let after = state.replies().0.state.playback.position_ms;
    assert!(
      after >= forward,
      "playhead never regresses on backward jitter: {after} < {forward}"
    );
  }

  #[test]
  fn position_tick_while_paused_resyncs() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(playing_track("iap2:track:a", 1_000));
    state.take_position_resync();
    state.apply_now_playing(NowPlayingUpdate {
      media_item: None,
      playback: Some(PlaybackUpdate {
        playing: Some(false),
        ..PlaybackUpdate::default()
      }),
    });
    state.take_position_resync();

    state.apply_now_playing(position_tick(5_000));
    assert!(state.take_position_resync(), "paused scrub resyncs");
  }

  #[test]
  fn companion_queue_rebuilds_order_from_snapshot_items() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    let a = qitem("spotify:track:a", "A", "X", Some("img/a"), Some(1000));
    let b = qitem("spotify:track:b", "B", "X", Some("img/b"), Some(2000));
    let c = qitem("spotify:track:c", "C", "X", Some("img/c"), Some(3000));

    state.apply_companion_queue(COMPANION, qsnap(vec![a.clone(), b.clone(), c.clone()]));
    assert_eq!(state.companion_queue, vec![a, b.clone(), c.clone()]);

    let d = qitem("spotify:track:d", "D", "X", Some("img/d"), Some(4000));
    state.apply_companion_queue(COMPANION, qsnap(vec![b.clone(), c.clone(), d.clone()]));
    assert_eq!(state.companion_queue, vec![b, c, d]);
  }

  #[test]
  fn derive_next_strips_current_when_held_queue_still_contains_it() {
    let b = qitem("spotify:track:b", "B", "X", None, None);
    let c = qitem("spotify:track:c", "C", "X", None, None);
    let d = qitem("spotify:track:d", "D", "X", None, None);
    let next = derive_next(vec![b.clone(), c.clone(), d.clone()], Some(&b));
    assert_eq!(next, vec![c, d]);
  }

  #[test]
  fn derive_next_returns_all_when_current_absent() {
    let b = qitem("spotify:track:b", "B", "X", None, None);
    let c = qitem("spotify:track:c", "C", "X", None, None);
    let current = qitem("spotify:track:a", "A", "X", None, None);
    let next = derive_next(vec![b.clone(), c.clone()], Some(&current));
    assert_eq!(next, vec![b, c]);
  }

  #[test]
  fn rolled_off_feeds_queue_previous_without_bumping_home_gen() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    let a = qitem("spotify:track:a", "A", "X", None, None);
    let b = qitem("spotify:track:b", "B", "X", None, None);

    state.note_rolled_off(a.clone());
    assert_eq!(state.recently_played, vec![a.clone()]);
    assert_eq!(state.root_browse_gen(), 0, "a roll-off never bumps the home cache gen");

    state.note_rolled_off(b.clone());
    assert_eq!(state.recently_played, vec![b, a], "most-recent-first");
    assert_eq!(state.root_browse_gen(), 0, "still no gen bump");
  }

  #[test]
  fn reset_companion_clears_recents_and_bumps_home_gen_but_retains_queue() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_companion_queue(COMPANION, qsnap(vec![qitem("spotify:track:a", "A", "X", None, None)]));
    state.note_rolled_off(qitem("spotify:track:b", "B", "X", None, None));
    let before = state.root_browse_gen();

    state.reset_companion(COMPANION);
    assert!(
      !state.companion_queue.is_empty(),
      "the held queue survives a companion blip"
    );
    assert!(state.recently_played.is_empty());
    assert_eq!(state.root_browse_gen(), before + 1);
  }

  #[test]
  fn queue_survives_companion_blip_and_serves_suffix_on_reconnect() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));
    state.apply_companion_snapshot(COMPANION, companion_snapshot("spotify:track:x", "X", None, true));
    state.apply_companion_queue(
      COMPANION,
      qsnap(vec![
        qitem("spotify:track:y", "Y", "A", None, None),
        qitem("spotify:track:z", "Z", "A", None, None),
      ]),
    );
    assert_eq!(state.replies().1.items.len(), 2);

    auth.drop_for(COMPANION);
    state.reset_companion(COMPANION);
    assert!(
      state.replies().1.items.is_empty(),
      "no companion authority means no companion queue view"
    );

    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));
    state.apply_companion_snapshot(COMPANION, companion_snapshot("spotify:track:y", "Y", None, true));
    let items = state.replies().1.items;
    assert_eq!(items.len(), 1, "the retained queue serves the derived suffix");
    assert_eq!(items[0].uri, "spotify:track:z");
  }

  #[test]
  fn iap2_rolled_off_stays_out_of_recents() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.note_rolled_off(qitem("iap2:track:x", "Foreign", "App", None, None));
    assert!(
      state.recently_played.is_empty(),
      "synthetic uris never enter the previous ring"
    );

    state.note_rolled_off(qitem("spotify:track:a", "A", "X", None, None));
    assert_eq!(state.recently_played.len(), 1);
  }

  #[test]
  fn note_library_changed_bumps_home_gen_without_clearing_queue() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_companion_queue(COMPANION, qsnap(vec![qitem("spotify:track:a", "A", "X", None, None)]));
    state.note_rolled_off(qitem("spotify:track:b", "B", "X", None, None));
    let before = state.root_browse_gen();

    state.note_library_changed();
    assert_eq!(
      state.root_browse_gen(),
      before + 1,
      "a phone-side library mutation invalidates the home cache"
    );
    assert!(
      !state.companion_queue.is_empty(),
      "a library change must not disturb the live queue"
    );
    assert_eq!(
      state.recently_played.len(),
      1,
      "a library change must not clear recents"
    );
  }

  #[test]
  fn reset_companion_falls_back_to_iap2_view_immediately() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());

    state.apply_now_playing(iap2_track("iap2:track:fallback", "iAP2 Song"));

    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));
    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot("spotify:track:x", "Spotify Song", Some("img/x"), true),
    );
    state.apply_companion_queue(COMPANION, qsnap(vec![qitem("spotify:track:y", "Y", "Z", None, None)]));
    assert_eq!(media(&state).uri.as_deref(), Some("spotify:track:x"));

    auth.drop_for(COMPANION);
    state.reset_companion(COMPANION);

    let m = media(&state);
    assert_eq!(
      m.persistent_id.as_deref(),
      Some("iap2:track:fallback"),
      "the forced post-disconnect broadcast reflects the iap2-only fallback, not the stale companion track"
    );
    assert_eq!(m.title.as_deref(), Some("iAP2 Song"));
    let queue = state.replies().1;
    assert!(queue.items.is_empty(), "the stale companion queue is cleared");
  }

  fn companion_snapshot_pos(pid: &str, title: &str, playing: bool, position_ms: u32) -> WirePlayerState {
    WirePlayerState {
      track: Some(MediaItem {
        persistent_id: Some(pid.to_string()),
        title: Some(title.to_string()),
        artist: Some("Artist".to_string()),
        album: Some("Album".to_string()),
        duration_ms: Some(200_000),
        ..MediaItem::default()
      }),
      playback: Playback {
        state: if playing {
          PlaybackState::Playing
        } else {
          PlaybackState::Paused
        },
        position_ms,
        ..Playback::default()
      },
      ..WirePlayerState::default()
    }
  }

  fn spotify_owned_state(auth: &AuthorityRegistry) -> PlayerState {
    let mut state = PlayerState::new(auth.clone());
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));
    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot_pos("spotify:track:x", "Spotify Song", true, 10_000),
    );
    state.take_position_resync();
    state
  }

  fn iap2_playback_delta(bundle: &str, playing: Option<bool>, position_ms: Option<u32>) -> NowPlayingUpdate {
    NowPlayingUpdate {
      media_item: None,
      playback: Some(PlaybackUpdate {
        playing,
        position_ms,
        app_bundle: Some(bundle.to_string()),
        ..PlaybackUpdate::default()
      }),
    }
  }

  fn view_position(state: &PlayerState) -> u32 {
    state.replies().0.state.playback.position_ms
  }

  #[test]
  fn staged_iap2_chatter_never_moves_a_companion_playhead() {
    let auth = AuthorityRegistry::new();
    let mut state = spotify_owned_state(&auth);
    state.age_clocks(Duration::from_secs(10));
    let before = view_position(&state);
    assert!(before >= 19_000, "aged playhead extrapolates: {before}");

    state.apply_now_playing(iap2_playback_delta("com.spotify.client", Some(true), None));
    assert!(
      !state.take_position_resync(),
      "staged iap2 chatter must not force a resync broadcast"
    );
    let after = view_position(&state);
    assert!(
      after >= before,
      "playhead snapped backward on staged iap2 chatter: {after} < {before}"
    );
  }

  #[test]
  fn iap2_song_change_burst_stages_without_disturbing_companion_view() {
    let auth = AuthorityRegistry::new();
    let mut state = spotify_owned_state(&auth);
    state.age_clocks(Duration::from_secs(10));
    let before = view_position(&state);

    let burst = [
      NowPlayingUpdate {
        media_item: Some(MediaItemUpdate {
          persistent_id: Some("iap2:track:b".into()),
          ..MediaItemUpdate::default()
        }),
        playback: Some(PlaybackUpdate {
          playing: Some(true),
          app_bundle: Some("com.spotify.client".into()),
          ..PlaybackUpdate::default()
        }),
      },
      NowPlayingUpdate {
        media_item: Some(MediaItemUpdate {
          title: Some("Track B".into()),
          ..MediaItemUpdate::default()
        }),
        playback: None,
      },
      NowPlayingUpdate {
        media_item: Some(MediaItemUpdate {
          artist: Some("Artist B".into()),
          ..MediaItemUpdate::default()
        }),
        playback: None,
      },
    ];
    for delta in burst {
      state.apply_now_playing(delta);
      assert!(
        !state.take_position_resync(),
        "burst fragment forced a resync broadcast"
      );
      let m = media(&state);
      assert_eq!(
        m.persistent_id.as_deref(),
        Some("spotify:track:x"),
        "companion track identity lost mid-burst"
      );
      assert_eq!(m.title.as_deref(), Some("Spotify Song"));
      let now = view_position(&state);
      assert!(now >= before, "playhead regressed mid-burst: {now} < {before}");
    }
  }

  #[test]
  fn staged_iap2_play_state_does_not_burn_the_transport_intent() {
    let auth = AuthorityRegistry::new();
    let mut state = spotify_owned_state(&auth);

    state.set_transport_intent(false);
    assert!(!state.playing);

    state.apply_now_playing(iap2_playback_delta("com.spotify.client", Some(true), None));
    state.apply_now_playing(iap2_playback_delta("com.spotify.client", Some(true), None));
    assert!(
      !state.playing,
      "staged iap2 play-state burned the optimistic transport intent"
    );

    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot_pos("spotify:track:x", "Spotify Song", false, 12_000),
    );
    assert!(!state.playing);
    state.apply_now_playing(iap2_playback_delta("com.spotify.client", Some(true), None));
    assert!(!state.playing, "post-confirm iap2 chatter flipped the play state");
  }

  #[test]
  fn iap2_artwork_ready_does_not_resync_a_companion_playhead() {
    let auth = AuthorityRegistry::new();
    let mut state = spotify_owned_state(&auth);
    state.age_clocks(Duration::from_secs(10));
    let before = view_position(&state);

    state.apply_artwork_id("iap2/art/a/5".to_string());
    assert!(
      !state.take_position_resync(),
      "iap2 artwork arrival forced a resync broadcast"
    );
    let after = view_position(&state);
    assert!(after >= before, "playhead regressed on artwork arrival");
    assert_eq!(
      artwork_id_of(&state),
      None,
      "companion-authoritative wire art must not fall back to iap2 art"
    );
  }

  #[test]
  fn bundle_divergence_hard_cuts_to_the_iap2_view() {
    let auth = AuthorityRegistry::new();
    let mut state = spotify_owned_state(&auth);

    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some("iap2:track:y".into()),
        title: Some("YouTube Video".into()),
        ..MediaItemUpdate::default()
      }),
      playback: Some(PlaybackUpdate {
        playing: Some(true),
        position_ms: Some(5_000),
        app_bundle: Some("com.google.ios.youtube".into()),
        ..PlaybackUpdate::default()
      }),
    });

    assert!(
      state.take_position_resync(),
      "an ownership flip is a hard cut and must broadcast"
    );
    let m = media(&state);
    assert_eq!(m.persistent_id.as_deref(), Some("iap2:track:y"));
    assert_eq!(m.title.as_deref(), Some("YouTube Video"));
    let pos = view_position(&state);
    assert!((4_500..=6_500).contains(&pos), "cut lands on the iap2 playhead: {pos}");
  }

  #[test]
  fn companion_snapshot_while_iap2_owns_stages_only() {
    let auth = AuthorityRegistry::new();
    let mut state = spotify_owned_state(&auth);
    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some("iap2:track:y".into()),
        title: Some("YouTube Video".into()),
        ..MediaItemUpdate::default()
      }),
      playback: Some(PlaybackUpdate {
        playing: Some(true),
        position_ms: Some(5_000),
        app_bundle: Some("com.google.ios.youtube".into()),
        ..PlaybackUpdate::default()
      }),
    });
    state.take_position_resync();
    state.age_clocks(Duration::from_secs(3));
    let before = view_position(&state);
    assert!(before >= 7_500, "aged iap2 playhead extrapolates: {before}");

    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot_pos("spotify:track:z", "Next Song", true, 0),
    );
    assert!(
      !state.take_position_resync(),
      "staged companion snapshot must not force a resync broadcast"
    );
    let m = media(&state);
    assert_eq!(
      m.title.as_deref(),
      Some("YouTube Video"),
      "companion snapshot clobbered the iap2-owned view"
    );
    let after = view_position(&state);
    assert!(
      after >= before,
      "playhead snapped backward on a staged companion snapshot: {after} < {before}"
    );
  }

  #[test]
  fn companion_claim_without_data_falls_through_to_iap2_time_fields() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);

    state.apply_now_playing(NowPlayingUpdate {
      media_item: None,
      playback: Some(PlaybackUpdate {
        repeat: Some(RepeatMode::One),
        ..PlaybackUpdate::default()
      }),
    });
    assert_eq!(state.options.repeat, RepeatMode::One);

    state.apply_now_playing(NowPlayingUpdate {
      media_item: None,
      playback: Some(PlaybackUpdate {
        repeat: Some(RepeatMode::Off),
        ..PlaybackUpdate::default()
      }),
    });
    assert_eq!(
      state.options.repeat,
      RepeatMode::Off,
      "companion-unset fields track the live fallback source past the claim edge"
    );
  }

  #[test]
  fn out_of_band_claim_surfaces_staged_companion_state_on_next_event() {
    let auth = AuthorityRegistry::new();
    let mut state = PlayerState::new(auth.clone());

    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot_pos("spotify:track:x", "Song", true, 3_000),
    );
    assert!(
      state.replies().0.state.track.is_none(),
      "an unclaimed companion snapshot must not surface"
    );

    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingMetadata);
    auth.claim(COMPANION, CompanionAuthorityScope::NowPlayingPlayback);
    auth.set_companion_app_bundle(COMPANION, Some("com.spotify.client".into()));
    state.apply_now_playing(iap2_playback_delta("com.spotify.client", Some(true), None));

    assert!(state.take_position_resync(), "the lazy claim edge is a hard cut");
    let m = media(&state);
    assert_eq!(m.uri.as_deref(), Some("spotify:track:x"));
    assert!(state.playing, "staged companion play state surfaces on the cut");
  }

  #[test]
  fn bundle_return_hard_cuts_to_the_staged_companion_view() {
    let auth = AuthorityRegistry::new();
    let mut state = spotify_owned_state(&auth);
    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some("iap2:track:y".into()),
        title: Some("YouTube Video".into()),
        ..MediaItemUpdate::default()
      }),
      playback: Some(PlaybackUpdate {
        playing: Some(true),
        position_ms: Some(5_000),
        app_bundle: Some("com.google.ios.youtube".into()),
        ..PlaybackUpdate::default()
      }),
    });
    state.take_position_resync();
    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot_pos("spotify:track:z", "Next Song", true, 0),
    );
    state.take_position_resync();

    state.age_clocks(Duration::from_secs(5));
    state.apply_now_playing(iap2_playback_delta("com.spotify.client", Some(true), None));

    assert!(
      state.take_position_resync(),
      "an ownership flip back is a hard cut and must broadcast"
    );
    let m = media(&state);
    assert_eq!(
      m.uri.as_deref(),
      Some("spotify:track:z"),
      "staged companion view surfaced"
    );
    assert_eq!(m.title.as_deref(), Some("Next Song"));
    let pos = view_position(&state);
    assert!(
      (5_000..=6_500).contains(&pos),
      "cut-back extrapolates the staged position by its age: {pos}"
    );
  }

  #[test]
  fn sustained_transport_mismatch_accepts_source_play_state() {
    let mut state = PlayerState::new(AuthorityRegistry::new());
    state.apply_now_playing(playing_track("iap2:track:a", 1_000));

    state.set_transport_intent(false);
    assert!(!state.playing);

    state.apply_now_playing(NowPlayingUpdate {
      media_item: None,
      playback: Some(PlaybackUpdate {
        playing: Some(true),
        ..PlaybackUpdate::default()
      }),
    });
    assert!(
      !state.playing,
      "a single stale mismatch rides over the optimistic state"
    );

    state.apply_now_playing(NowPlayingUpdate {
      media_item: None,
      playback: Some(PlaybackUpdate {
        playing: Some(true),
        ..PlaybackUpdate::default()
      }),
    });
    assert!(
      state.playing,
      "a sustained mismatch cancels the intent and accepts the source play state"
    );
  }

  fn queued_state(auth: &AuthorityRegistry) -> PlayerState {
    let mut state = spotify_owned_state(auth);
    state.apply_companion_queue(
      COMPANION,
      qsnap(vec![
        qitem(
          "spotify:track:b",
          "Track B",
          "Artist B",
          Some("spotify/img/248/b"),
          Some(201_000),
        ),
        qitem(
          "spotify:track:c",
          "Track C",
          "Artist C",
          Some("spotify/img/248/c"),
          Some(202_000),
        ),
        qitem(
          "spotify:track:d",
          "Track D",
          "Artist D",
          Some("spotify/img/248/d"),
          Some(203_000),
        ),
      ]),
    );
    state
  }

  fn upcoming_uris(state: &PlayerState) -> Vec<String> {
    state.replies().1.items.into_iter().map(|q| q.uri).collect()
  }

  #[test]
  fn optimistic_advance_promotes_the_predicted_next_track() {
    let auth = AuthorityRegistry::new();
    let mut state = queued_state(&auth);
    state.age_clocks(Duration::from_secs(5));
    state.take_position_resync();

    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some("iap2:track:b".into()),
        ..MediaItemUpdate::default()
      }),
      playback: Some(PlaybackUpdate {
        playing: Some(true),
        app_bundle: Some("com.spotify.client".into()),
        ..PlaybackUpdate::default()
      }),
    });
    assert_eq!(
      media(&state).persistent_id.as_deref(),
      Some("spotify:track:x"),
      "no promotion before the title lands"
    );

    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        title: Some("Track B".into()),
        ..MediaItemUpdate::default()
      }),
      playback: None,
    });
    let m = media(&state);
    assert_eq!(
      m.uri.as_deref(),
      Some("spotify:track:b"),
      "promoted card carries the real uri"
    );
    assert_eq!(
      m.artwork_id.as_deref(),
      Some("spotify/img/248/b"),
      "promoted card reuses the held queue art"
    );
    assert_eq!(m.duration_ms, Some(201_000));
    assert!(
      state.take_position_resync(),
      "a promoted track change is a hard broadcast"
    );
    assert!(
      state.playing,
      "play state rides iap2 once the companion time half is vacated"
    );
    let pos = view_position(&state);
    assert!(
      pos <= 2_000,
      "iap2 drives time for the promoted track from track start: {pos}"
    );
    assert_eq!(
      upcoming_uris(&state),
      vec!["spotify:track:c", "spotify:track:d"],
      "the promoted track leaves the derived upcoming"
    );
  }

  #[test]
  fn optimistic_advance_noops_when_the_dealer_already_advanced() {
    let auth = AuthorityRegistry::new();
    let mut state = queued_state(&auth);

    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot_pos("spotify:track:b", "Track B", true, 4_000),
    );
    state.take_position_resync();

    state.apply_now_playing(iap2_app("iap2:track:b", "Track B", "com.spotify.client", true));
    assert!(
      !state.take_position_resync(),
      "an already-current track change must not re-broadcast"
    );
    assert!(
      state.companion_playback.position_ms.is_some(),
      "the companion time half stays authoritative"
    );
    let pos = view_position(&state);
    assert!(pos >= 4_000, "the playhead must not reset to track start: {pos}");
    assert_eq!(
      upcoming_uris(&state),
      vec!["spotify:track:c", "spotify:track:d"],
      "no promotion past the already-current track"
    );
  }

  #[test]
  fn optimistic_advance_absorbs_a_missed_advance() {
    let auth = AuthorityRegistry::new();
    let mut state = queued_state(&auth);

    state.apply_now_playing(iap2_app("iap2:track:c", "Track C", "com.spotify.client", true));
    assert_eq!(
      media(&state).uri.as_deref(),
      Some("spotify:track:c"),
      "a missed boundary still lands on the right queue item"
    );
    assert_eq!(upcoming_uris(&state), vec!["spotify:track:d"]);
  }

  #[test]
  fn optimistic_advance_chains_across_a_multi_song_outage() {
    let auth = AuthorityRegistry::new();
    let mut state = queued_state(&auth);

    state.apply_now_playing(iap2_app("iap2:track:b", "Track B", "com.spotify.client", true));
    state.apply_now_playing(iap2_app("iap2:track:c", "Track C", "com.spotify.client", true));
    assert_eq!(media(&state).uri.as_deref(), Some("spotify:track:c"));
    assert_eq!(upcoming_uris(&state), vec!["spotify:track:d"]);
  }

  #[test]
  fn optimistic_advance_ignores_an_off_queue_jump() {
    let auth = AuthorityRegistry::new();
    let mut state = queued_state(&auth);

    state.apply_now_playing(iap2_app(
      "iap2:track:z",
      "Something Else Entirely",
      "com.spotify.client",
      true,
    ));
    assert_eq!(
      media(&state).persistent_id.as_deref(),
      Some("spotify:track:x"),
      "an off-queue jump stages and waits for the dealer"
    );
    assert_eq!(state.replies().1.items.len(), 3, "the held queue is untouched");
  }

  #[test]
  fn optimistic_advance_requires_artist_agreement_when_both_carry_one() {
    let auth = AuthorityRegistry::new();
    let mut state = queued_state(&auth);

    state.apply_now_playing(NowPlayingUpdate {
      media_item: Some(MediaItemUpdate {
        persistent_id: Some("iap2:track:cover".into()),
        title: Some("Track B".into()),
        artist: Some("A Cover Band".into()),
        ..MediaItemUpdate::default()
      }),
      playback: Some(PlaybackUpdate {
        playing: Some(true),
        app_bundle: Some("com.spotify.client".into()),
        ..PlaybackUpdate::default()
      }),
    });
    assert_eq!(
      media(&state).persistent_id.as_deref(),
      Some("spotify:track:x"),
      "a title collision with a different artist must not promote"
    );
  }

  #[test]
  fn optimistic_advance_never_promotes_for_a_foreign_bundle() {
    let auth = AuthorityRegistry::new();
    let mut state = queued_state(&auth);

    state.apply_now_playing(iap2_app("iap2:track:vid", "Track B", "com.google.ios.youtube", true));
    assert_eq!(
      state.companion_metadata.persistent_id.as_deref(),
      Some("spotify:track:x"),
      "the companion buffer holds the last real snapshot untouched"
    );
    assert_eq!(
      media(&state).persistent_id.as_deref(),
      Some("iap2:track:vid"),
      "the view is the iap2 hard cut, not a promotion"
    );
  }

  #[test]
  fn optimistic_advance_requires_an_explicit_bundle_match() {
    let auth = AuthorityRegistry::new();
    let mut state = queued_state(&auth);

    state.apply_now_playing(iap2_track("iap2:track:b", "Track B"));
    assert_eq!(media(&state).persistent_id.as_deref(), Some("spotify:track:x"));
  }

  #[test]
  fn optimistic_advance_does_not_double_promote_duplicate_queue_titles() {
    let auth = AuthorityRegistry::new();
    let mut state = spotify_owned_state(&auth);
    state.apply_companion_queue(
      COMPANION,
      qsnap(vec![
        qitem("spotify:track:b", "Track B", "Artist B", None, None),
        qitem("spotify:track:b2", "Track B", "Artist B", None, None),
        qitem("spotify:track:c", "Track C", "Artist C", None, None),
      ]),
    );

    state.apply_now_playing(iap2_app("iap2:track:b", "Track B", "com.spotify.client", true));
    assert_eq!(
      media(&state).uri.as_deref(),
      Some("spotify:track:b"),
      "the first duplicate promotes"
    );

    state.apply_now_playing(iap2_app("iap2:track:b", "Track B", "com.spotify.client", true));
    assert_eq!(media(&state).uri.as_deref(), Some("spotify:track:b"));
    assert_eq!(upcoming_uris(&state), vec!["spotify:track:b2", "spotify:track:c"]);
  }

  #[test]
  fn dealer_snapshot_corrects_a_wrong_promotion() {
    let auth = AuthorityRegistry::new();
    let mut state = queued_state(&auth);

    state.apply_now_playing(iap2_app("iap2:track:b", "Track B", "com.spotify.client", true));
    assert_eq!(media(&state).uri.as_deref(), Some("spotify:track:b"));

    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot_pos("spotify:track:elsewhere", "Elsewhere", true, 30_000),
    );
    state.apply_companion_queue(
      COMPANION,
      qsnap(vec![qitem("spotify:track:w", "W", "Artist W", None, None)]),
    );
    assert_eq!(media(&state).uri.as_deref(), Some("spotify:track:elsewhere"));
    let pos = view_position(&state);
    assert!(pos >= 30_000, "the corrected playhead is the dealer's: {pos}");
    assert_eq!(upcoming_uris(&state), vec!["spotify:track:w"]);
  }

  #[test]
  fn stale_iap2_echo_after_a_context_switch_never_reverts_the_card() {
    let auth = AuthorityRegistry::new();
    let mut state = spotify_owned_state(&auth);
    state.apply_companion_queue(
      COMPANION,
      qsnap(vec![
        qitem("spotify:track:x", "Spotify Song", "Artist", None, Some(200_000)),
        qitem("spotify:track:y", "Track Y", "Artist Y", None, None),
      ]),
    );
    state.apply_now_playing(iap2_app("iap2:track:x", "Spotify Song", "com.spotify.client", true));
    assert_eq!(media(&state).persistent_id.as_deref(), Some("spotify:track:x"));

    state.apply_companion_snapshot(
      COMPANION,
      companion_snapshot_pos("spotify:track:t", "Fresh Pick", true, 0),
    );
    assert_eq!(media(&state).persistent_id.as_deref(), Some("spotify:track:t"));

    state.apply_now_playing(iap2_playback_delta("com.spotify.client", Some(true), None));
    assert_eq!(
      media(&state).persistent_id.as_deref(),
      Some("spotify:track:t"),
      "a stale iap2 echo must not promote the just-played track back onto the card"
    );
  }
}
