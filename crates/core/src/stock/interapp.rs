use std::collections::HashMap;

use libbridgething::{
  Album, Artist, BrowseEntry, BrowseResult, ItemKind, ItemRef, LibraryItem, MediaItem, PlayContext, Playback,
  PlaybackOptions, PlaybackRestrictions, PlaybackState, PlayerOptions as WirePlayerOptions,
  PlayerState as WirePlayerState, QueueItem, RepeatMode, Track,
  client::{
    BridgeToClientPlayerMsg, ClientLegacyStockCommand, ClientToBridgeAudioMsgCommand, ClientToBridgeLibraryMsg,
    ClientToBridgePhoneMsg, ClientToBridgePlayerMsg, Earcon as ClientEarcon, FavoritesSet as ClientFavoritesSet,
    PhoneCallAction, PlayUri as ClientPlayUri, PlayerQueueReply, PlayerStateReply, SeekTo as ClientSeekTo,
    SetRepeat as ClientSetRepeat, SetShuffle as ClientSetShuffle, SetSpeed as ClientSetSpeed,
    SkipPrev as ClientSkipPrev, SkipToIndex as ClientSkipToIndex,
  },
  stock::{StockPreset, StockSetPreset},
};
use serde::{Deserialize, Serialize};

use crate::handler::client::{PossibleRecvMsg, RecvMsgData};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "method", content = "args", rename_all = "snake_case")]
pub enum StockInterAppRecv {
  #[serde(rename = "com.spotify.superbird.crashes.report")]
  CrashReport(serde_json::Value),
  #[serde(rename = "com.spotify.superbird.earcon")]
  Earcon { earcon: String }, // 'confirmation' | 'listening' | 'error'
  #[serde(rename = "com.spotify.get_available_podcast_playback_speeds")]
  GetAvailablePodcastPlaybackSpeeds {},
  #[serde(rename = "com.spotify.get_capabilities")]
  GetCapabilities {},
  #[serde(rename = "com.spotify.get_children_of_item")]
  GetChildrenOfItem {
    parent_id: String,
    limit: usize,
    offset: Option<usize>,
  },
  #[serde(rename = "com.spotify.superbird.get_home")]
  GetHome {
    limit: usize,
    limit_overrides: HashMap<String, usize>,
  },
  #[serde(rename = "com.spotify.get_crossfade_state")]
  GetCrossfadeState {},
  #[serde(rename = "com.spotify.get_current_context")]
  GetCurrentContext {},
  #[serde(rename = "com.spotify.get_current_track")]
  GetCurrentTrack {},
  #[serde(rename = "com.spotify.get_image")]
  GetImage { id: String },
  #[serde(rename = "com.spotify.get_items_for_uris")]
  GetItemForURI {},
  #[serde(rename = "com.spotify.get_next_tracks")]
  GetNextTracks {},
  #[serde(rename = "com.spotify.superbird.permissions")]
  GetPermissions {},
  #[serde(rename = "com.spotify.get_playback_speed")]
  GetPlaybackSpeed {},
  #[serde(rename = "com.spotify.superbird.player_state")]
  GetPlayerState {},
  #[serde(rename = "com.spotify.superbird.get_podcast")]
  GetPodcast {
    uri: String,
    limit: Option<usize>,
    offset: Option<usize>,
  },
  #[serde(rename = "com.spotify.get_podcast_playback_speed")]
  GetPodcastPlaybackSpeed {},
  #[serde(rename = "com.spotify.superbird.presets.get_presets")]
  GetPresets {},
  #[serde(rename = "com.spotify.get_rating")]
  GetRating {},
  #[serde(rename = "com.spotify.get_recommended_content_for_type")]
  GetRecommendedContentForType {},
  #[serde(rename = "com.spotify.get_repeat")]
  GetRepeat {},
  #[serde(rename = "com.spotify.get_root_item")]
  GetRootItem {},
  #[serde(rename = "com.spotify.get_saved")]
  GetSaved { id: String }, // id is uri
  #[serde(rename = "com.spotify.get_session_state")]
  GetSessionState {},
  #[serde(rename = "com.spotify.get_shuffle")]
  GetShuffle {},
  #[serde(rename = "com.spotify.get_thumbnail_image")]
  GetThumbnailImage { id: String },
  #[serde(rename = "com.spotify.superbird.tipsandtricks.get_tips_and_tricks")]
  GetTips {},
  #[serde(rename = "com.spotify.get_track_elapsed")]
  GetTrackElapsed {},
  #[serde(rename = "com.spotify.superbird.tts.speak")]
  GetTts { file: String },
  #[serde(rename = "com.spotify.superbird.graphql")]
  Graph { payload: String },
  #[serde(rename = "com.spotify.log_message")]
  LogMessage(serde_json::Value),
  #[serde(rename = "com.spotify.superbird.pitstop.log")]
  PitstopLog(serde_json::Value),
  #[serde(rename = "com.spotify.play_item")]
  _PlayItem,
  #[serde(rename = "com.spotify.play_uri")]
  _PlayUri,
  #[serde(rename = "com.spotify.superbird.play_podcast_trailer")]
  PlayPodcastTrailer { uri: String },
  #[serde(rename = "com.spotify.queue_spotify_uri")]
  QueueUri { uri: String },
  #[serde(rename = "com.spotify.search_query")]
  SearchQuery,
  #[serde(rename = "com.spotify.set_playback_position")]
  _SeekToPosition,
  #[serde(rename = "com.spotify.set_playback_speed")]
  _SetPlaybackSpeed,
  #[serde(rename = "com.spotify.set_podcast_playback_speed")]
  SetPodcastPlaybackSpeed { playback_speed: usize },
  #[serde(rename = "com.spotify.superbird.presets.set_preset")]
  SetPreset { presets: Vec<StockSetPreset> },
  #[serde(rename = "com.spotify.set_rating")]
  SetRating,
  #[serde(rename = "com.spotify.set_repeat")]
  _SetRepeat,
  #[serde(rename = "com.spotify.set_saved")]
  SetSaved {
    id: Option<String>, // id is same as uri
    uri: Option<String>,
    saved: bool,
  },
  #[serde(rename = "com.spotify.set_shuffle")]
  _SetShuffle,
  #[serde(rename = "com.spotify.skip_next")]
  _SkipNext,
  #[serde(rename = "com.spotify.skip_previous")]
  _SkipPrevious,
  #[serde(rename = "com.spotify.skip_to_index_in_queue")]
  SkipToIndex { index: usize },
  #[serde(rename = "com.spotify.start_radio")]
  StartRadio,
  #[serde(rename = "com.spotify.superbird.dj.summon")]
  SummonDj,
  #[serde(rename = "com.spotify.superbird.instrumentation.request")]
  RequestLog(serde_json::Value),
  #[serde(rename = "com.spotify.superbird.instrumentation.interaction")]
  SendUbiInteraction(serde_json::Value),
  #[serde(rename = "com.spotify.superbird.instrumentation.impression")]
  SendUbiImpression(serde_json::Value),
  #[serde(rename = "com.spotify.superbird.instrumentation.log")]
  SendUbiBatch(serde_json::Value),
  #[serde(rename = "com.spotify.superbird.phone.answer")]
  PhoneAnswer {},
  #[serde(rename = "com.spotify.superbird.phone.decline")]
  PhoneDecline {},
  #[serde(rename = "com.spotify.superbird.phone.get_image")]
  PhoneCallImage { phone_number: String },
  #[serde(rename = "com.spotify.superbird.phone.send_message")]
  PhoneCallMessage { phone_number: String, message: String },
  #[serde(rename = "com.spotify.superbird.volume.volume_up")]
  IncreaseVolume {},
  #[serde(rename = "com.spotify.superbird.volume.volume_down")]
  DecreaseVolume {},
  #[serde(rename = "com.spotify.superbird.play_uri")]
  PlayUri {
    uri: String,
    feature_identifier: String,
    interaction_id: Option<String>,
    skip_to_uri: Option<String>,
    skip_to_uid: Option<String>,
  },
  #[serde(rename = "com.spotify.superbird.skip_next")]
  SkipNext {},
  #[serde(rename = "com.spotify.superbird.skip_prev")]
  SkipPrev { allow_seeking: bool },
  #[serde(rename = "com.spotify.superbird.seek_to")]
  SeekTo { position: usize },
  #[serde(rename = "com.spotify.superbird.resume")]
  Resume {},
  #[serde(rename = "com.spotify.superbird.pause")]
  Pause {},
  #[serde(rename = "com.spotify.superbird.set_shuffle")]
  SetShuffle { shuffle: bool },
  #[serde(rename = "com.spotify.superbird.set_repeat")]
  SetRepeat { repeat_mode: bool },
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StockInterAppSend {
  #[serde(rename = "msgId")]
  pub msg_id: Option<usize>,
  #[serde(flatten)]
  pub data: StockInterAppSendPayload,
}

#[serde_with::skip_serializing_none]
#[derive(derive_more::Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum StockInterAppSendPayload {
  #[serde(rename = "call_result")]
  Ack {},
  #[serde(rename = "call_error")]
  CallError(String),
  #[serde(rename = "call_result")]
  Permissions { can_use_superbird: bool },
  #[serde(rename = "com.spotify.session_state")]
  SessionState {
    connection_type: StockConnectionType,
    is_in_forced_offline_mode: bool,
    is_logged_in: bool,
    is_offline: bool,
  },
  #[serde(rename = "com.spotify.superbird.player_state")]
  IdlePlayerState {
    context_uri: String,
    is_paused: bool,
    is_paused_bool: bool,
    playback_options: StockPlaybackOptions,
    playback_position: usize,
    playback_restrictions: StockPlaybackRestrictions,
    playback_speed: f64,
  },
  #[serde(rename = "com.spotify.superbird.player_state")]
  SpotifyPlayerState {
    context_uri: String,
    context_title: String,
    is_paused: bool,
    is_paused_bool: bool,
    playback_options: StockPlaybackOptions,
    playback_position: usize,
    playback_restrictions: StockPlaybackRestrictions,
    playback_speed: f64,
    track: StockTrack,
  },
  #[serde(rename = "com.spotify.play_queue")]
  PlayerQueue {
    next: Vec<StockQueueTrack>,
    current: StockQueueTrack,
    previous: Vec<StockQueueTrack>,
  },
  #[serde(rename = "call_result")]
  ItemChildren {
    limit: usize,
    offset: usize,
    total: usize,
    items: Vec<ChildItem>,
  },
  #[serde(rename = "call_result")]
  Home { items: Vec<HomeSection> },
  #[serde(rename = "call_result")]
  Image {
    height: usize,
    width: usize,
    #[debug(skip)]
    image_data: String,
  },
  #[serde(rename = "call_result")]
  Presets { result: Vec<StockPreset>, success: bool },
  #[serde(rename = "call_result")]
  Tips { result: Vec<StockTip> },
  #[serde(rename = "call_result")]
  Saved { saved: bool },
  #[serde(rename = "call_result")]
  Graphql {
    data: Option<serde_json::Value>,
    errors: Option<Vec<GraphqlError>>,
  },
  #[serde(rename = "com.spotify.superbird.volume.volume_state")]
  VolumeState { volume: f64, volume_steps: u8 },
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphqlError {
  pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StockTip {
  pub id: u32,
  pub title: String,
  pub description: String,
  pub action: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct StockPlaybackOptions {
  pub repeat: u32,
  pub shuffle: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StockPlaybackRestrictions {
  pub can_repeat_context: bool,
  pub can_repeat_track: bool,
  pub can_seek: bool,
  pub can_skip_next: bool,
  pub can_skip_prev: bool,
  pub can_toggle_shuffle: bool,
  pub can_like: bool,
  pub can_change_volume: bool,
  pub can_set_output: bool,
}

impl From<PlaybackRestrictions> for StockPlaybackRestrictions {
  fn from(restrictions: PlaybackRestrictions) -> Self {
    Self {
      can_repeat_context: restrictions.can_repeat_context,
      can_repeat_track: restrictions.can_repeat_track,
      can_seek: restrictions.can_seek,
      can_skip_next: restrictions.can_skip_next,
      can_skip_prev: restrictions.can_skip_prev,
      can_toggle_shuffle: restrictions.can_toggle_shuffle,
      can_like: restrictions.can_like,
      can_change_volume: restrictions.can_change_volume,
      can_set_output: restrictions.can_set_output,
    }
  }
}

impl From<PlaybackOptions> for StockPlaybackOptions {
  fn from(options: PlaybackOptions) -> Self {
    Self {
      repeat: match options.repeat {
        RepeatMode::Off => 0,
        RepeatMode::One => 1,
        RepeatMode::All => 2,
      },
      shuffle: options.shuffle,
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChildItem {
  pub id: String,
  pub uri: String,
  pub image_id: String,
  pub title: String,
  pub subtitle: String,
  pub playable: bool,
  pub has_children: bool,
  pub available_offline: bool,
  pub metadata: ChildMeta,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChildMeta {
  pub is_explicit_content: bool,
  pub is_19_plus_content: bool,
  pub duration_ms: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HomeSection {
  pub title: String,
  pub uri: String,
  pub total: usize,
  pub children: Vec<HomeChild>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HomeChild {
  pub image_id: String,
  pub subtitle: String,
  pub title: String,
  pub uri: String,
}

impl From<BridgeToClientPlayerMsg> for StockInterAppSendPayload {
  fn from(data: BridgeToClientPlayerMsg) -> Self {
    match data {
      BridgeToClientPlayerMsg::Snapshot(reply) | BridgeToClientPlayerMsg::StateReply(reply) => {
        player_state_to_stock(reply)
      }
      BridgeToClientPlayerMsg::QueueChanged(reply) | BridgeToClientPlayerMsg::QueueReply(reply) => {
        player_queue_to_stock(reply)
      }
      BridgeToClientPlayerMsg::Delta(_)
      | BridgeToClientPlayerMsg::ErrorEvent(_)
      | BridgeToClientPlayerMsg::ErrorReply(_)
      | BridgeToClientPlayerMsg::TargetsChanged(_)
      | BridgeToClientPlayerMsg::TargetsReply(_) => Self::Ack {},
    }
  }
}

pub fn player_state_to_stock(reply: PlayerStateReply) -> StockInterAppSendPayload {
  let other_media = reply.active_app.is_some();
  let WirePlayerState {
    track,
    playback,
    options,
    context,
    ..
  } = reply.state;
  let is_paused = !matches!(playback.state, PlaybackState::Playing);
  let playback_options = StockPlaybackOptions {
    repeat: match playback.repeat {
      RepeatMode::Off => 0,
      RepeatMode::One => 1,
      RepeatMode::All => 2,
    },
    shuffle: playback.shuffle,
  };
  let playback_position = playback.position_ms as usize;
  let playback_speed = f64::from(player_speed_or_default(&options));

  let (context_uri, context_title, playback_restrictions) = if other_media {
    (
      track.as_ref().and_then(|t| t.persistent_id.clone()).unwrap_or_default(),
      String::new(),
      other_media_restrictions(&playback),
    )
  } else {
    (
      context
        .as_ref()
        .map(|c| c.uri.clone())
        .filter(|u| !u.is_empty())
        .or_else(|| track.as_ref().and_then(|t| t.uri.clone()))
        .unwrap_or_default(),
      context.and_then(|c| c.name).unwrap_or_default(),
      PlaybackRestrictions::all_true().into(),
    )
  };

  StockInterAppSendPayload::SpotifyPlayerState {
    context_uri,
    context_title,
    is_paused,
    is_paused_bool: is_paused,
    playback_options,
    playback_position,
    playback_restrictions,
    playback_speed,
    track: media_item_to_stock_track(track.unwrap_or_default()),
  }
}

fn other_media_restrictions(playback: &Playback) -> StockPlaybackRestrictions {
  StockPlaybackRestrictions {
    can_repeat_context: false,
    can_repeat_track: false,
    can_seek: playback.set_elapsed_time_available.unwrap_or(false),
    can_skip_next: true,
    can_skip_prev: true,
    can_toggle_shuffle: playback.shuffle_mode.is_some(),
    can_like: false,
    can_change_volume: true,
    can_set_output: false,
  }
}

pub fn library_browse_to_stock(result: BrowseResult, limit: u32, offset: u32) -> StockInterAppSendPayload {
  let entries_len = result.entries.len() as u32;
  let total = match result.total {
    Some(t) => t as usize,
    None => {
      let consumed = offset.saturating_add(entries_len);
      let synth = if result.has_more {
        consumed.saturating_add(1)
      } else {
        consumed
      };
      synth as usize
    }
  };
  let items = result.entries.into_iter().map(browse_entry_to_child).collect();
  StockInterAppSendPayload::ItemChildren {
    limit: limit as usize,
    offset: offset as usize,
    total,
    items,
  }
}

pub fn library_browse_to_home(result: BrowseResult) -> StockInterAppSendPayload {
  let items = result
    .entries
    .into_iter()
    .filter_map(browse_entry_to_home_section)
    .collect();
  StockInterAppSendPayload::Home { items }
}

fn browse_entry_to_home_section(entry: BrowseEntry) -> Option<HomeSection> {
  let BrowseEntry::Folder(folder) = entry else {
    return None;
  };
  let total = folder.total.unwrap_or_default() as usize;
  let children = folder
    .preview_children
    .unwrap_or_default()
    .into_iter()
    .map(browse_entry_to_home_child)
    .collect();
  Some(HomeSection {
    title: folder.title,
    uri: folder.node_id,
    total,
    children,
  })
}

fn browse_entry_to_home_child(entry: BrowseEntry) -> HomeChild {
  let child = browse_entry_to_child(entry);
  HomeChild {
    image_id: child.image_id,
    subtitle: child.subtitle,
    title: child.title,
    uri: child.uri,
  }
}

fn browse_entry_to_child(entry: BrowseEntry) -> ChildItem {
  match entry {
    BrowseEntry::Folder(folder) => ChildItem {
      id: folder.node_id.clone(),
      uri: folder.node_id,
      image_id: folder.artwork_id.unwrap_or_default(),
      title: folder.title,
      subtitle: folder.subtitle.unwrap_or_default(),
      playable: false,
      has_children: true,
      available_offline: false,
      metadata: ChildMeta::default(),
    },
    BrowseEntry::Item(item) => library_item_to_child(item),
  }
}

fn library_item_to_child(item: LibraryItem) -> ChildItem {
  match item {
    LibraryItem::Track(t) => ChildItem {
      id: t.id.clone(),
      uri: t.id,
      image_id: t.image_id,
      title: t.name,
      subtitle: t.artist.name,
      playable: true,
      has_children: false,
      available_offline: false,
      metadata: ChildMeta {
        duration_ms: t.duration_ms as usize,
        ..ChildMeta::default()
      },
    },
    LibraryItem::Album(a) => ChildItem {
      id: a.id.clone(),
      uri: a.id,
      image_id: a.artwork_id.unwrap_or_default(),
      title: a.name,
      subtitle: String::new(),
      playable: true,
      has_children: true,
      available_offline: false,
      metadata: ChildMeta::default(),
    },
    LibraryItem::Playlist(p) => ChildItem {
      id: p.uri.clone(),
      uri: p.uri,
      image_id: p.artwork_id.unwrap_or_default(),
      title: p.name,
      subtitle: p.owner_name.unwrap_or_default(),
      playable: true,
      has_children: true,
      available_offline: false,
      metadata: ChildMeta::default(),
    },
    LibraryItem::PodcastEpisode(e) => ChildItem {
      id: e.uri.clone(),
      uri: e.uri,
      image_id: e.artwork_id.unwrap_or_default(),
      title: e.name,
      subtitle: e.show_name.unwrap_or_default(),
      playable: true,
      has_children: false,
      available_offline: false,
      metadata: ChildMeta {
        duration_ms: e.duration_ms.unwrap_or(0) as usize,
        ..ChildMeta::default()
      },
    },
    LibraryItem::Show(s) => ChildItem {
      id: s.uri.clone(),
      uri: s.uri,
      image_id: s.artwork_id.unwrap_or_default(),
      title: s.name,
      subtitle: s.publisher.unwrap_or_default(),
      playable: false,
      has_children: true,
      available_offline: false,
      metadata: ChildMeta::default(),
    },
    LibraryItem::Artist(a) => ChildItem {
      id: a.id.clone(),
      uri: a.id,
      image_id: a.artwork_id.unwrap_or_default(),
      title: a.name,
      subtitle: String::new(),
      playable: false,
      has_children: true,
      available_offline: false,
      metadata: ChildMeta::default(),
    },
    LibraryItem::Station(s) => ChildItem {
      id: s.uri.clone(),
      uri: s.uri,
      image_id: s.artwork_id.unwrap_or_default(),
      title: s.name,
      subtitle: String::new(),
      playable: true,
      has_children: false,
      available_offline: false,
      metadata: ChildMeta::default(),
    },
  }
}

pub fn player_queue_to_stock(reply: PlayerQueueReply) -> StockInterAppSendPayload {
  StockInterAppSendPayload::PlayerQueue {
    next: reply.items.into_iter().map(queue_item_to_stock).collect(),
    current: reply.current.map(queue_item_to_stock).unwrap_or_default(),
    previous: reply.previous.into_iter().map(queue_item_to_stock).collect(),
  }
}

fn player_speed_or_default(options: &WirePlayerOptions) -> f32 {
  if options.speed.is_finite() && options.speed > 0.0 {
    options.speed
  } else {
    1.0
  }
}

fn stock_artist(name: Option<String>, uri: Option<String>) -> StockArtist {
  let name = name.unwrap_or_default();
  let uri = uri
    .filter(|u| !u.is_empty())
    .unwrap_or_else(|| Artist::from(name.clone()).id);
  StockArtist { name, uri }
}

fn stock_album(name: Option<String>, uri: Option<String>) -> StockAlbum {
  let name = name.unwrap_or_default();
  let uri = uri
    .filter(|u| !u.is_empty())
    .unwrap_or_else(|| Album::from(name.clone()).id);
  StockAlbum { name, uri }
}

fn media_item_to_stock_track(item: MediaItem) -> StockTrack {
  let uid = item.persistent_id.clone().unwrap_or_default();
  let uri = item
    .uri
    .filter(|u| !u.is_empty())
    .or(item.persistent_id)
    .unwrap_or_default();
  let artist = stock_artist(item.artist, item.artist_uri);
  StockTrack {
    name: item.title.unwrap_or_default(),
    album: stock_album(item.album, item.album_uri),
    artist: artist.clone(),
    artists: vec![artist],
    duration_ms: item.duration_ms.unwrap_or(0) as usize,
    image_id: item.artwork_id.unwrap_or_default(),
    is_episode: false,
    is_podcast: false,
    saved: item.liked.unwrap_or(false),
    uid,
    uri,
  }
}

fn queue_item_to_stock(item: QueueItem) -> StockQueueTrack {
  let artist_uri = item.artist_uri;
  let artists: Vec<StockArtist> = item
    .artist
    .map(|name| stock_artist(Some(name), artist_uri))
    .into_iter()
    .collect();
  StockQueueTrack {
    uid: item.persistent_id.clone().unwrap_or_else(|| item.uri.clone()),
    uri: item.uri,
    name: item.title.unwrap_or_default(),
    artists,
    image_uri: item.artwork_id.unwrap_or_default(),
    provider: if item.queued { "queue" } else { "context" }.to_string(),
  }
}

impl Default for StockQueueTrack {
  fn default() -> Self {
    Self {
      uid: String::new(),
      uri: String::new(),
      name: String::new(),
      artists: Vec::new(),
      image_uri: String::new(),
      provider: "context".to_string(),
    }
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StockArtist {
  pub name: String,
  pub uri: String,
}

impl From<Artist> for StockArtist {
  fn from(artist: Artist) -> Self {
    Self {
      name: artist.name,
      uri: artist.id,
    }
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StockAlbum {
  pub name: String,
  pub uri: String,
}

impl From<Album> for StockAlbum {
  fn from(album: Album) -> Self {
    Self {
      name: album.name,
      uri: album.id,
    }
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StockTrack {
  pub name: String,
  pub album: StockAlbum,
  pub artist: StockArtist,
  pub artists: Vec<StockArtist>,
  pub duration_ms: usize,
  pub image_id: String,
  pub is_episode: bool,
  pub is_podcast: bool,
  pub saved: bool,
  pub uid: String,
  pub uri: String,
}

impl From<Track> for StockTrack {
  fn from(track: Track) -> Self {
    Self {
      name: track.name,
      album: track.album.into(),
      artist: track.artists.first().cloned().unwrap_or_default().into(),
      artists: track.artists.into_iter().map(|a| a.into()).collect(),
      duration_ms: track.duration_ms as usize,
      image_id: track.image_id,
      is_episode: false,
      is_podcast: false,
      saved: track.saved,
      uid: track.id.clone(),
      uri: track.id,
    }
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StockQueueTrack {
  pub uid: String,
  pub uri: String,
  pub name: String,
  pub artists: Vec<StockArtist>,
  pub image_uri: String,
  pub provider: String,
}

impl From<Track> for StockQueueTrack {
  fn from(track: Track) -> Self {
    Self {
      uid: track.id.clone(),
      uri: track.id,
      name: track.name,
      artists: track.artists.into_iter().map(|a| a.into()).collect(),
      image_uri: track.image_id,
      provider: "context".to_string(),
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StockConnectionType {
  None,
  Wlan,
  #[serde(rename = "4g")]
  FourG,
}

impl StockInterAppSend {
  pub fn new(msg_id: Option<usize>, data: StockInterAppSendPayload) -> Self {
    Self { msg_id, data }
  }

  pub fn make_ack(msg_id: Option<usize>) -> Self {
    Self {
      msg_id,
      data: StockInterAppSendPayload::Ack {},
    }
  }
}

impl RecvMsgData {
  /// this method WILL panic. don't be a dumbass.
  pub fn from_stock_inter_app_possible_recv(recv: PossibleRecvMsg) -> Self {
    let PossibleRecvMsg::StockInterApp { data, .. } = recv else {
      panic!("YOU CAN ONY CALL THIS WITH STOCK INTER APP RECV DATA");
    };

    match data {
      StockInterAppRecv::PlayUri { uri, skip_to_uri, .. } => {
        let play = match skip_to_uri {
          Some(track) => ClientPlayUri {
            uri: track,
            context: Some(PlayContext { context_uri: uri }),
          },
          None => ClientPlayUri { uri, context: None },
        };
        RecvMsgData::Player(ClientToBridgePlayerMsg::Play(play))
      }
      StockInterAppRecv::PlayPodcastTrailer { uri } => {
        RecvMsgData::Player(ClientToBridgePlayerMsg::Play(ClientPlayUri { uri, context: None }))
      }
      StockInterAppRecv::QueueUri { uri } => {
        RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyQueueUri { uri })
      }
      StockInterAppRecv::Pause {} => RecvMsgData::Player(ClientToBridgePlayerMsg::Pause),
      StockInterAppRecv::Resume {} => RecvMsgData::Player(ClientToBridgePlayerMsg::Resume),
      StockInterAppRecv::SkipNext {} => RecvMsgData::Player(ClientToBridgePlayerMsg::SkipNext),
      StockInterAppRecv::SkipPrev { allow_seeking } => {
        RecvMsgData::Player(ClientToBridgePlayerMsg::SkipPrev(ClientSkipPrev { allow_seeking }))
      }
      StockInterAppRecv::SkipToIndex { index } => {
        RecvMsgData::Player(ClientToBridgePlayerMsg::SkipToIndex(ClientSkipToIndex {
          index: u32::try_from(index).unwrap_or(u32::MAX),
        }))
      }
      StockInterAppRecv::SeekTo { position } => RecvMsgData::Player(ClientToBridgePlayerMsg::SeekTo(ClientSeekTo {
        position_ms: u32::try_from(position).unwrap_or(u32::MAX),
      })),
      StockInterAppRecv::SetShuffle { shuffle } => {
        RecvMsgData::Player(ClientToBridgePlayerMsg::SetShuffle(ClientSetShuffle { on: shuffle }))
      }
      StockInterAppRecv::SetRepeat { repeat_mode } => {
        let mode = if repeat_mode { RepeatMode::All } else { RepeatMode::Off };
        RecvMsgData::Player(ClientToBridgePlayerMsg::SetRepeat(ClientSetRepeat { mode }))
      }
      StockInterAppRecv::SetPodcastPlaybackSpeed { playback_speed } => {
        RecvMsgData::Player(ClientToBridgePlayerMsg::SetSpeed(ClientSetSpeed {
          speed: playback_speed as f32 / 100.0,
        }))
      }

      StockInterAppRecv::IncreaseVolume {} => RecvMsgData::Audio(ClientToBridgeAudioMsgCommand::VolumeUp),
      StockInterAppRecv::DecreaseVolume {} => RecvMsgData::Audio(ClientToBridgeAudioMsgCommand::VolumeDown),
      StockInterAppRecv::Earcon { earcon } => {
        RecvMsgData::Audio(ClientToBridgeAudioMsgCommand::Earcon(ClientEarcon { name: earcon }))
      }
      StockInterAppRecv::GetTts { file } => RecvMsgData::Audio(ClientToBridgeAudioMsgCommand::Earcon(ClientEarcon {
        name: format!("spotify-stock:{file}"),
      })),

      StockInterAppRecv::PhoneAnswer {} => RecvMsgData::Phone(ClientToBridgePhoneMsg::Answer(PhoneCallAction {
        call_id: String::new(),
      })),
      StockInterAppRecv::PhoneDecline {} => RecvMsgData::Phone(ClientToBridgePhoneMsg::Decline(PhoneCallAction {
        call_id: String::new(),
      })),

      StockInterAppRecv::SetSaved { uri, id, saved } => match uri.or(id) {
        Some(uri) => RecvMsgData::Library(ClientToBridgeLibraryMsg::FavoritesSet(ClientFavoritesSet {
          item: ItemRef {
            uri,
            kind: ItemKind::Track,
            persistent_id: None,
          },
          liked: saved,
        })),
        None => RecvMsgData::Hole,
      },

      StockInterAppRecv::GetImage { id } => RecvMsgData::LegacyStock(ClientLegacyStockCommand::GetImage { id }),
      StockInterAppRecv::GetThumbnailImage { id } => {
        RecvMsgData::LegacyStock(ClientLegacyStockCommand::GetThumbnailImage { id })
      }
      StockInterAppRecv::GetNextTracks {} => RecvMsgData::LegacyStock(ClientLegacyStockCommand::GetNextTracks),
      StockInterAppRecv::GetPermissions {} => RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetPermissions),
      StockInterAppRecv::GetChildrenOfItem {
        parent_id,
        limit,
        offset,
      } => RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetChildren {
        parent_id,
        limit,
        offset,
      }),
      StockInterAppRecv::GetHome { limit, limit_overrides } => {
        RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetHome { limit, limit_overrides })
      }
      StockInterAppRecv::GetPodcast { uri, limit, offset } => {
        RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetPodcast { uri, limit, offset })
      }
      StockInterAppRecv::GetSaved { id } => RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetSaved { id }),
      StockInterAppRecv::GetPresets {} => RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetPresets),
      StockInterAppRecv::SetPreset { presets } => {
        RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifySetPreset { presets })
      }
      StockInterAppRecv::GetTips {} => RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetTips),
      StockInterAppRecv::SummonDj => RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifySummonDj),
      StockInterAppRecv::Graph { payload } => {
        RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGraphql { payload })
      }
      StockInterAppRecv::PhoneCallImage { phone_number } => {
        RecvMsgData::LegacyStock(ClientLegacyStockCommand::SuperbirdPhoneCallImage { phone_number })
      }

      StockInterAppRecv::PhoneCallMessage { .. } => RecvMsgData::Hole,

      StockInterAppRecv::CrashReport(_)
      | StockInterAppRecv::LogMessage(_)
      | StockInterAppRecv::PitstopLog(_)
      | StockInterAppRecv::RequestLog(_)
      | StockInterAppRecv::SendUbiInteraction(_)
      | StockInterAppRecv::SendUbiImpression(_)
      | StockInterAppRecv::SendUbiBatch(_) => RecvMsgData::Hole,

      StockInterAppRecv::GetCurrentTrack {}
      | StockInterAppRecv::GetCurrentContext {}
      | StockInterAppRecv::GetPlayerState {}
      | StockInterAppRecv::GetTrackElapsed {} => {
        RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetPlayerState)
      }
      StockInterAppRecv::GetSessionState {} => {
        RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetSessionState)
      }

      StockInterAppRecv::GetCapabilities {}
      | StockInterAppRecv::GetShuffle {}
      | StockInterAppRecv::GetRepeat {}
      | StockInterAppRecv::GetPlaybackSpeed {}
      | StockInterAppRecv::GetCrossfadeState {}
      | StockInterAppRecv::GetPodcastPlaybackSpeed {}
      | StockInterAppRecv::GetAvailablePodcastPlaybackSpeeds {}
      | StockInterAppRecv::GetRootItem {}
      | StockInterAppRecv::GetRating {}
      | StockInterAppRecv::GetItemForURI {}
      | StockInterAppRecv::GetRecommendedContentForType {}
      | StockInterAppRecv::SearchQuery
      | StockInterAppRecv::SetRating
      | StockInterAppRecv::StartRadio => RecvMsgData::Hole,

      StockInterAppRecv::_PlayItem
      | StockInterAppRecv::_PlayUri
      | StockInterAppRecv::_SeekToPosition
      | StockInterAppRecv::_SetPlaybackSpeed
      | StockInterAppRecv::_SetRepeat
      | StockInterAppRecv::_SetShuffle
      | StockInterAppRecv::_SkipNext
      | StockInterAppRecv::_SkipPrevious => RecvMsgData::Hole,
    }
  }
}

#[cfg(test)]
mod test {
  use libbridgething::{
    RepeatMode,
    client::{
      ClientLegacyStockCommand, ClientToBridgeAudioMsgCommand, ClientToBridgeLibraryMsg, ClientToBridgeMsg,
      ClientToBridgeMsgData, ClientToBridgePhoneMsg, ClientToBridgePlayerMsg, FavoritesSet, PhoneCallAction,
      PlayUri as ClientPlayUri, SeekTo, SetRepeat, SetShuffle, SkipPrev, SkipToIndex,
    },
    wire::MsgMeta,
  };
  use uuid::Uuid;

  use super::StockInterAppRecv;
  use crate::handler::client::{PossibleRecvMsg, RecvMsgData};

  #[test]
  fn ser_stock_recv() {
    let ser = serde_json::to_string(&StockInterAppRecv::PlayUri {
      uri: "test".to_string(),
      feature_identifier: "test".to_string(),
      interaction_id: None,
      skip_to_uri: None,
      skip_to_uid: None,
    })
    .expect("failed to serialize json");
    println!("{:?}", ser);

    assert_eq!(
      ser,
      r#"{"method":"com.spotify.superbird.play_uri","args":{"uri":"test","feature_identifier":"test"}}"#
    );
  }

  #[test]
  fn de_stock_recv_play_uri() {
    let json = r#"{"msgId":69,"method":"com.spotify.superbird.play_uri","args":{"uri":"test","feature_identifier":"test"},"userAction": true}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::StockInterApp {
        msg_id: 69,
        data: StockInterAppRecv::PlayUri {
          uri: "test".to_string(),
          feature_identifier: "test".to_string(),
          interaction_id: None,
          skip_to_uri: None,
          skip_to_uid: None,
        },
        user_action: true,
      }
    );
  }

  #[test]
  fn de_stock_recv_get_image() {
    let json = r#"{"msgId":1,"method":"com.spotify.get_image","args":{"id":"image_id"},"userAction": false}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::StockInterApp {
        msg_id: 1,
        data: StockInterAppRecv::GetImage {
          id: "image_id".to_string(),
        },
        user_action: false,
      }
    );
  }

  #[test]
  fn de_stock_recv_phone_call_message() {
    let json = r#"{"msgId":2,"method":"com.spotify.superbird.phone.send_message","args":{"phone_number":"1234567890","message":"Hello"},"userAction": false}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::StockInterApp {
        msg_id: 2,
        data: StockInterAppRecv::PhoneCallMessage {
          phone_number: "1234567890".to_string(),
          message: "Hello".to_string(),
        },
        user_action: false,
      }
    );
  }

  #[test]
  fn de_stock_recv_increase_volume() {
    let json = r#"{"msgId":3,"method":"com.spotify.superbird.volume.volume_up","args":{},"userAction": false}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::StockInterApp {
        msg_id: 3,
        data: StockInterAppRecv::IncreaseVolume {},
        user_action: false,
      }
    );
  }

  #[test]
  fn de_stock_recv_skip_to_index() {
    let json = r#"{"msgId":4,"method":"com.spotify.skip_to_index_in_queue","args":{"index":5},"userAction": false}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::StockInterApp {
        msg_id: 4,
        data: StockInterAppRecv::SkipToIndex { index: 5 },
        user_action: false,
      }
    );
  }

  #[test]
  fn de_stock_recv_set_repeat() {
    let json =
      r#"{"msgId":5,"method":"com.spotify.superbird.set_repeat","args":{"repeat_mode":true},"userAction": false}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::StockInterApp {
        msg_id: 5,
        data: StockInterAppRecv::SetRepeat { repeat_mode: true },
        user_action: false,
      }
    );
  }

  #[test]
  fn de_stock_recv_permissions() {
    let json = r#"{"msgId":2,"method":"com.spotify.superbird.permissions","args":{},"userAction":false}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);
  }

  #[test]
  fn de_stock_recv_instrumentation() {
    let json = r#"{"msgId":3,"method":"com.spotify.superbird.instrumentation.log","args":{"interactions":[{"action_name":"ui_navigate_back","action_version":1,"annotator_configuration_version":"","annotator_version":"","app":"music","element_path_ids":["","","",""],"element_path_names":["car-settings-carthingos","phone_connection_row","phone_connection_view","hardware_back_button"],"element_path_pos":["","","",""],"element_path_reasons":["","","",""],"element_path_uris":["","","",""],"generator_version":"10.0.2","interaction_id":"a27d88eb-1b5f-4f57-b477-51b3f031b9f1","interaction_type":"key_stroke","parent_modes":[],"parent_path_ids":[],"parent_path_names":[],"parent_path_pos":[],"parent_path_reasons":[],"parent_path_uris":[],"parent_specification_versions":[],"specification_version":"9.0.0","specification_mode":"default","page_instance_id":null,"playback_id":null,"play_context_uri":null},{"action_name":"ui_navigate_back","action_version":1,"annotator_configuration_version":"","annotator_version":"","app":"music","element_path_ids":["",""],"element_path_names":["car-settings-carthingos","hardware_back_button"],"element_path_pos":["",""],"element_path_reasons":["",""],"element_path_uris":["",""],"generator_version":"10.0.2","interaction_id":"4a212c49-76a7-4f34-afad-9c03052c4e4b","interaction_type":"key_stroke","parent_modes":[],"parent_path_ids":[],"parent_path_names":[],"parent_path_pos":[],"parent_path_reasons":[],"parent_path_uris":[],"parent_specification_versions":[],"specification_version":"9.0.0","specification_mode":"default","page_instance_id":null,"playback_id":null,"play_context_uri":null}],"impressions":[{"annotator_configuration_version":"","annotator_version":"","app":"music","element_path_ids":["","","","",""],"element_path_names":["car-settings-carthingos","phone_connection_row","phone_connection_view","existing_phone_row","select_phone_progress_dialog"],"element_path_pos":["","","","",""],"element_path_reasons":["","","","",""],"element_path_uris":["","","","",""],"generator_version":"10.0.2","impression_id":"b858b7aa-bbb9-4816-89ee-25ca1602eb91","parent_modes":[],"parent_path_ids":[],"parent_path_names":[],"parent_path_pos":[],"parent_path_reasons":[],"parent_path_uris":[],"parent_specification_versions":[],"specification_version":"9.0.0","specification_mode":"default","page_instance_id":null,"playback_id":null,"play_context_uri":null},{"annotator_configuration_version":"","annotator_version":"","app":"music","element_path_ids":["","","","",""],"element_path_names":["car-settings-carthingos","phone_connection_row","phone_connection_view","existing_phone_row","select_phone_success_dialog"],"element_path_pos":["","","","",""],"element_path_reasons":["","","","",""],"element_path_uris":["","","","",""],"generator_version":"10.0.2","impression_id":"7eff4cb6-384c-4f43-8ffd-f9e16366ab22","parent_modes":[],"parent_path_ids":[],"parent_path_names":[],"parent_path_pos":[],"parent_path_reasons":[],"parent_path_uris":[],"parent_specification_versions":[],"specification_version":"9.0.0","specification_mode":"default","page_instance_id":null,"playback_id":null,"play_context_uri":null},{"annotator_configuration_version":"","annotator_version":"","app":"music","element_path_ids":[""],"element_path_names":["car-settings-carthingos"],"element_path_pos":[""],"element_path_reasons":[""],"element_path_uris":[""],"generator_version":"10.0.2","impression_id":"50ccf04d-e308-4cf6-a1e0-fd57d7913d63","parent_modes":[],"parent_path_ids":[],"parent_path_names":[],"parent_path_pos":[],"parent_path_reasons":[],"parent_path_uris":[],"parent_specification_versions":[],"specification_version":"9.0.0","specification_mode":"default","page_instance_id":null,"playback_id":null,"play_context_uri":null}],"interaction_timestamps":[{"timestamp":1733803725274,"interaction_id":"a27d88eb-1b5f-4f57-b477-51b3f031b9f1"},{"timestamp":1733803726813,"interaction_id":"4a212c49-76a7-4f34-afad-9c03052c4e4b"}],"impression_timestamps":[{"timestamp":1733803720258,"impression_id":"b858b7aa-bbb9-4816-89ee-25ca1602eb91"},{"timestamp":1733803720263,"impression_id":"7eff4cb6-384c-4f43-8ffd-f9e16366ab22"},{"timestamp":1733803725285,"impression_id":"50ccf04d-e308-4cf6-a1e0-fd57d7913d63"}]},"userAction":false}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);
  }

  #[test]
  fn ser_recv_get_image() {
    let ser = serde_json::to_string(&PossibleRecvMsg::Modern(ClientToBridgeMsg {
      id: Uuid::parse_str("0193ace5-1876-7b2c-8d7b-f63a20d6f316").unwrap(),
      meta: MsgMeta::Request,
      data: ClientToBridgeMsgData::LegacyStock(ClientLegacyStockCommand::GetImage {
        id: "image_id".to_string(),
      }),
    }))
    .expect("failed to serialize json");
    println!("{:?}", ser);

    assert_eq!(
      ser,
      r#"{"id":"0193ace5-1876-7b2c-8d7b-f63a20d6f316","meta":{"kind":"request"},"data":{"type":"legacyStock","data":{"action":"getImage","args":{"id":"image_id"}}}}"#
    );
  }

  #[test]
  fn de_recv_skip_to_index() {
    let json = r#"{"id":"0193ace5-1876-7b2c-8d7b-f63a20d6f316","meta":{"kind":"command"},"data":{"type":"player","data":{"event":"skipToIndex","data":{"index":5}}}}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::Modern(ClientToBridgeMsg {
        id: Uuid::parse_str("0193ace5-1876-7b2c-8d7b-f63a20d6f316").unwrap(),
        meta: MsgMeta::Command,
        data: ClientToBridgeMsgData::Player(ClientToBridgePlayerMsg::SkipToIndex(SkipToIndex { index: 5 }))
      })
    );
  }

  #[test]
  fn de_recv_skip_prev() {
    let json = r#"{"id":"0193ace5-1876-7b2c-8d7b-f63a20d6f316","meta":{"kind":"command"},"data":{"type":"player","data":{"event":"skipPrev","data":{"allowSeeking":true}}}}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::Modern(ClientToBridgeMsg {
        id: Uuid::parse_str("0193ace5-1876-7b2c-8d7b-f63a20d6f316").unwrap(),
        meta: MsgMeta::Command,
        data: ClientToBridgeMsgData::Player(ClientToBridgePlayerMsg::SkipPrev(SkipPrev { allow_seeking: true }))
      })
    );
  }

  #[test]
  fn de_recv_seek_to() {
    let json = r#"{"id":"0193ace5-1876-7b2c-8d7b-f63a20d6f316","meta":{"kind":"command"},"data":{"type":"player","data":{"event":"seekTo","data":{"positionMs":120}}}}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::Modern(ClientToBridgeMsg {
        id: Uuid::parse_str("0193ace5-1876-7b2c-8d7b-f63a20d6f316").unwrap(),
        meta: MsgMeta::Command,
        data: ClientToBridgeMsgData::Player(ClientToBridgePlayerMsg::SeekTo(SeekTo { position_ms: 120 }))
      })
    );
  }

  #[test]
  fn de_recv_set_shuffle() {
    let json = r#"{"id":"0193ace5-1876-7b2c-8d7b-f63a20d6f316","meta":{"kind":"command"},"data":{"type":"player","data":{"event":"setShuffle","data":{"on":true}}}}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::Modern(ClientToBridgeMsg {
        id: Uuid::parse_str("0193ace5-1876-7b2c-8d7b-f63a20d6f316").unwrap(),
        meta: MsgMeta::Command,
        data: ClientToBridgeMsgData::Player(ClientToBridgePlayerMsg::SetShuffle(SetShuffle { on: true }))
      })
    );
  }

  #[test]
  fn de_recv_set_repeat() {
    let json = r#"{"id":"0193ace5-1876-7b2c-8d7b-f63a20d6f316","meta":{"kind":"command"},"data":{"type":"player","data":{"event":"setRepeat","data":{"mode":"all"}}}}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize json");
    println!("{:?}", de);

    assert_eq!(
      de,
      PossibleRecvMsg::Modern(ClientToBridgeMsg {
        id: Uuid::parse_str("0193ace5-1876-7b2c-8d7b-f63a20d6f316").unwrap(),
        meta: MsgMeta::Command,
        data: ClientToBridgeMsgData::Player(ClientToBridgePlayerMsg::SetRepeat(SetRepeat { mode: RepeatMode::All }))
      })
    );
  }

  #[test]
  fn translate_phone_call_message_to_hole() {
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 2,
      data: StockInterAppRecv::PhoneCallMessage {
        phone_number: "1234567890".into(),
        message: "Hello".into(),
      },
      user_action: false,
    };
    assert!(matches!(
      RecvMsgData::from_stock_inter_app_possible_recv(recv),
      RecvMsgData::Hole
    ));
  }

  #[test]
  fn translate_play_uri_to_player_play() {
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 3,
      data: StockInterAppRecv::PlayUri {
        uri: "spotify:track:abc".into(),
        feature_identifier: "test".into(),
        interaction_id: None,
        skip_to_uri: None,
        skip_to_uid: None,
      },
      user_action: true,
    };
    let translated = RecvMsgData::from_stock_inter_app_possible_recv(recv);
    let RecvMsgData::Player(ClientToBridgePlayerMsg::Play(ClientPlayUri { uri, context })) = translated else {
      panic!("expected Player::Play, got {translated:?}");
    };
    assert_eq!(uri, "spotify:track:abc");
    assert!(context.is_none());
  }

  #[test]
  fn translate_play_uri_with_skip_to_plays_track_in_context() {
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 3,
      data: StockInterAppRecv::PlayUri {
        uri: "spotify:playlist:xyz".into(),
        feature_identifier: "test".into(),
        interaction_id: None,
        skip_to_uri: Some("spotify:track:abc".into()),
        skip_to_uid: None,
      },
      user_action: true,
    };
    let translated = RecvMsgData::from_stock_inter_app_possible_recv(recv);
    let RecvMsgData::Player(ClientToBridgePlayerMsg::Play(ClientPlayUri { uri, context })) = translated else {
      panic!("expected Player::Play, got {translated:?}");
    };
    assert_eq!(uri, "spotify:track:abc");
    let context = context.expect("expected a play context");
    assert_eq!(context.context_uri, "spotify:playlist:xyz");
  }

  #[test]
  fn translate_queue_uri_to_legacy_stock() {
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 7,
      data: StockInterAppRecv::QueueUri {
        uri: "spotify:track:abc".into(),
      },
      user_action: true,
    };
    let translated = RecvMsgData::from_stock_inter_app_possible_recv(recv);
    let RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyQueueUri { uri }) = translated else {
      panic!("expected LegacyStock::SpotifyQueueUri, got {translated:?}");
    };
    assert_eq!(uri, "spotify:track:abc");
  }

  #[test]
  fn translate_volume_up_to_audio_volume_up() {
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 4,
      data: StockInterAppRecv::IncreaseVolume {},
      user_action: true,
    };
    assert!(matches!(
      RecvMsgData::from_stock_inter_app_possible_recv(recv),
      RecvMsgData::Audio(ClientToBridgeAudioMsgCommand::VolumeUp)
    ));
  }

  #[test]
  fn translate_phone_answer_to_phone_answer() {
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 5,
      data: StockInterAppRecv::PhoneAnswer {},
      user_action: true,
    };
    let translated = RecvMsgData::from_stock_inter_app_possible_recv(recv);
    let RecvMsgData::Phone(ClientToBridgePhoneMsg::Answer(PhoneCallAction { call_id })) = translated else {
      panic!("expected Phone::Answer, got {translated:?}");
    };
    assert_eq!(call_id, "");
  }

  #[test]
  fn translate_set_saved_to_library_favorites_set() {
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 6,
      data: StockInterAppRecv::SetSaved {
        id: None,
        uri: Some("spotify:track:xyz".into()),
        saved: true,
      },
      user_action: true,
    };
    let translated = RecvMsgData::from_stock_inter_app_possible_recv(recv);
    let RecvMsgData::Library(ClientToBridgeLibraryMsg::FavoritesSet(FavoritesSet { item, liked })) = translated else {
      panic!("expected Library::FavoritesSet, got {translated:?}");
    };
    assert_eq!(item.uri, "spotify:track:xyz");
    assert!(liked);
  }

  #[test]
  fn translate_permissions_to_legacy_stock() {
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 7,
      data: StockInterAppRecv::GetPermissions {},
      user_action: false,
    };
    assert!(matches!(
      RecvMsgData::from_stock_inter_app_possible_recv(recv),
      RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGetPermissions)
    ));
  }

  #[test]
  fn translate_graphql_to_legacy_stock() {
    let payload = "query{shelf(limit:14 overrides:[]){...on Shelf{items{title id total}}}}".to_string();
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 8,
      data: StockInterAppRecv::Graph {
        payload: payload.clone(),
      },
      user_action: false,
    };
    let translated = RecvMsgData::from_stock_inter_app_possible_recv(recv);
    let RecvMsgData::LegacyStock(ClientLegacyStockCommand::SpotifyGraphql { payload: out }) = translated else {
      panic!("expected LegacyStock::SpotifyGraphql, got {translated:?}");
    };
    assert_eq!(out, payload);
  }

  #[test]
  fn translate_phone_call_image_to_legacy_stock() {
    let recv = PossibleRecvMsg::StockInterApp {
      msg_id: 9,
      data: StockInterAppRecv::PhoneCallImage {
        phone_number: "1234567890".into(),
      },
      user_action: false,
    };
    let translated = RecvMsgData::from_stock_inter_app_possible_recv(recv);
    let RecvMsgData::LegacyStock(ClientLegacyStockCommand::SuperbirdPhoneCallImage { phone_number }) = translated
    else {
      panic!("expected LegacyStock::SuperbirdPhoneCallImage, got {translated:?}");
    };
    assert_eq!(phone_number, "1234567890");
  }

  #[test]
  fn de_stock_recv_graphql() {
    let json = r#"{"msgId":42,"method":"com.spotify.superbird.graphql","args":{"payload":"query{tipsOnDemand{tips{id}}}"},"userAction":true}"#;
    let de: PossibleRecvMsg = serde_json::from_str(json).expect("failed to deserialize graphql msg");
    assert_eq!(
      de,
      PossibleRecvMsg::StockInterApp {
        msg_id: 42,
        data: StockInterAppRecv::Graph {
          payload: "query{tipsOnDemand{tips{id}}}".into(),
        },
        user_action: true,
      }
    );
  }

  #[test]
  fn now_playing_track_prefers_provider_uri_over_persistent_id() {
    let item = libbridgething::MediaItem {
      uri: Some("spotify:track:1".to_string()),
      persistent_id: Some("iap2:track:a".to_string()),
      title: Some("Song".to_string()),
      ..Default::default()
    };
    let track = super::media_item_to_stock_track(item);
    assert_eq!(track.uri, "spotify:track:1");
    assert_eq!(track.uid, "iap2:track:a");
  }

  #[test]
  fn now_playing_track_uri_falls_back_to_synthetic_id() {
    let item = libbridgething::MediaItem {
      uri: None,
      persistent_id: Some("iap2:track:a".to_string()),
      ..Default::default()
    };
    let track = super::media_item_to_stock_track(item);
    assert_eq!(track.uri, "iap2:track:a");
    assert_eq!(track.uid, "iap2:track:a");
  }

  fn reply_with(
    active_app: Option<libbridgething::CurrentlyActiveApplication>,
    item: libbridgething::MediaItem,
  ) -> libbridgething::client::PlayerStateReply {
    libbridgething::client::PlayerStateReply {
      active_app,
      state: libbridgething::PlayerState {
        track: Some(item),
        playback: libbridgething::Playback {
          state: libbridgething::PlaybackState::Playing,
          ..Default::default()
        },
        ..Default::default()
      },
    }
  }

  #[test]
  fn other_media_emits_full_player_state_with_other_media_restrictions() {
    let mut reply = reply_with(
      Some(libbridgething::CurrentlyActiveApplication {
        id: "com.google.ios.youtube".to_string(),
        name: "YouTube".to_string(),
      }),
      libbridgething::MediaItem {
        persistent_id: Some("iap2:track:y".to_string()),
        title: Some("Clip".to_string()),
        ..Default::default()
      },
    );
    reply.state.playback.set_elapsed_time_available = Some(true);
    reply.state.playback.shuffle_mode = None;
    match super::player_state_to_stock(reply) {
      super::StockInterAppSendPayload::SpotifyPlayerState {
        context_uri,
        context_title,
        playback_restrictions,
        track,
        ..
      } => {
        assert_eq!(context_uri, "iap2:track:y");
        assert_eq!(context_title, "");
        assert!(playback_restrictions.can_skip_next);
        assert!(playback_restrictions.can_seek);
        assert!(!playback_restrictions.can_like);
        assert!(!playback_restrictions.can_toggle_shuffle);
        assert_eq!(track.uri, "iap2:track:y");
      }
      other => panic!("expected SpotifyPlayerState, got {other:?}"),
    }
  }

  #[test]
  fn player_state_restrictions_serialize_as_snake_case() {
    let reply = reply_with(
      None,
      libbridgething::MediaItem {
        uri: Some("spotify:track:a".to_string()),
        title: Some("Song".to_string()),
        ..Default::default()
      },
    );
    let json = serde_json::to_value(super::player_state_to_stock(reply)).expect("serialize");
    let restrictions = json
      .pointer("/payload/playback_restrictions")
      .and_then(serde_json::Value::as_object)
      .expect("payload carries playback_restrictions");

    for key in [
      "can_repeat_context",
      "can_repeat_track",
      "can_seek",
      "can_skip_next",
      "can_skip_prev",
      "can_toggle_shuffle",
      "can_like",
      "can_change_volume",
      "can_set_output",
    ] {
      assert_eq!(
        restrictions.get(key).and_then(serde_json::Value::as_bool),
        Some(true),
        "stock expects `{key}`; got keys {:?}",
        restrictions.keys().collect::<Vec<_>>()
      );
    }
  }

  #[test]
  fn spotify_now_playing_emits_spotify_player_state() {
    let reply = reply_with(
      None,
      libbridgething::MediaItem {
        uri: Some("spotify:track:x".to_string()),
        persistent_id: Some("spotify:track:x".to_string()),
        title: Some("Song".to_string()),
        ..Default::default()
      },
    );
    match super::player_state_to_stock(reply) {
      super::StockInterAppSendPayload::SpotifyPlayerState { context_uri, .. } => {
        assert_eq!(context_uri, "spotify:track:x");
      }
      other => panic!("expected SpotifyPlayerState, got {other:?}"),
    }
  }
}
