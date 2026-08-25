mod macros;
mod shared;

pub mod client;
pub mod gateway;
pub mod stock;
pub mod wire;

#[cfg(feature = "protocol")]
pub mod protocol;

pub use shared::{
  AcceptCallAction, Album, AncsAuthState, ArtProfile, Artist, AssetRetention, AudioCapabilities, AudioError, BoolField,
  BridgeThingMeta, BrightnessMode, BrightnessState, BrowseEntry, BrowseFolder, BrowseResult, CARTHING_HACKS_LOGO,
  CallEndReason, Capabilities, CommunicationsState, CompanionAuthorityScope, ConfigEntry, ConfigField,
  CurrentlyActiveApplication, Device, DeviceType, Diagnostics, DismissReason, DocEntry, DtmfTone, EndCallAction,
  EnumField, FavoritesPage, ForwardMessage, GatewayCapabilities, GatewayInfo, GeoAccuracy, GeoError, HardwareError,
  HardwareState, HttpHeader, HttpMethod, IMAGE_SIZE, Image, InitiateCallType, ItemKind, ItemRef,
  LIBBRIDGETHING_VERSION, LibraryError, LibraryItem, LinkKind, LogEntry, LogLevel, LogSource, LyricLine, Lyrics,
  MediaItem, MediaItemUpdate, MediaType, MusicProvider, NetError, NetFetchRequest, NetFetchResponse, NetworkInfo,
  NetworkKind, NluAlternate, NluAmount, NluDirection, NluPhoneAction, NluPlaybackSpeed, NluPopularityFilter,
  NluRepeatMode, NluResolvedIntent, NluScope, NluSlots, NluStage, NluTargetType, NluView, Notification,
  NotificationAction, NotificationApp, NotificationCategory, NotificationFlags, NotificationsError, NowPlayingUpdate,
  NumberField, OtaError, OtaErrorCode, OtaFinished, OtaKind, OtaPhase, OtaProgress, OverlayProfile, Peer,
  PeerCompanionStatus, PeerIap2Status, PhoneCall, PhoneCallDirection, PhoneCallService, PhoneCallStatus, PhoneError,
  PhoneState, PlayContext, Playback, PlaybackContext, PlaybackOptions, PlaybackQueue, PlaybackRestrictions,
  PlaybackState, PlaybackTarget, PlaybackTargetKind, PlaybackUpdate, PlayerError, PlayerOptions, PlayerState, Playlist,
  PodcastEpisode, Position, Priority, QueueItem, QueuePosition, RECENTS_NODE_ID, RangePart, RangeSpec,
  RecommendationsResult, RedirectPolicy, RegistrationStatus, RepeatMode, SearchResult, Show, ShuffleMode, Station,
  StreamBegin, StreamChunk, StreamEnd, StreamError, StringField, SurfaceAvailability, THUMBNAIL_SIZE, TimeInfo, Track,
  TtlRetention, TunnelAck, TunnelClosed, TunnelData, TunnelError, VoiceCaptureReason, VoiceDescriptor,
  VoiceDispatchErrorCode, VoiceDispatchTarget, WEBAPP_PROVENANCE_MAX_LEN, WebappError, WebappInfo, WebappManifest,
  WebappRole, WebappSource, WsError, WsFrame, to_slug,
};

pub const BRIDGETHING_DEVICE_CLASS: u32 = 0x7c0000;
pub const BRIDGETHING_PROFILE_UUID: uuid::Uuid = uuid::Uuid::from_u128(0xdead0000_854d_408e_81f0_fb6147f918fd);
pub const BRIDGETHING_RFCOMM_CHANNEL: u8 = 1;

pub const BRIDGETHING_STOCK_WS_PORT: u16 = 8890;
pub const BRIDGETHING_WS_MODERN_PORT: u16 = 8891;
pub const BRIDGETHING_FILE_SERVE_PORT: u16 = 8891;
pub const BRIDGETHING_NETWORK_GATEWAY_PORT: u16 = 8892;
pub const BRIDGETHING_MDNS_SERVICE_TYPE: &str = "_bridgething._tcp";
pub const BRIDGETHING_OTA_RANGE_PROXY_PORT: u16 = 8893;
pub const BRIDGETHING_SOCKS_PROXY_PORT: u16 = 1080;
