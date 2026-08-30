use std::sync::Arc;

use bridgething_companion::dispatch::asset::AssetDispatcher;
use bridgething_gateway::{
  AssetHandler, AudioHandler, EnvelopeHandler, ForwardHandler, GeoHandler, HandlerError, LibraryHandler, LyricsHandler,
  NetHandler, NotificationsHandler, PhoneHandler, PlayerHandler, Reply, SystemHandler, TransferHandler, TunnelHandler,
  VoiceHandler, WebappHandler,
};
use libbridgething::{gateway::*, wire::WireError, *};
use uuid::Uuid;

pub struct Routed {
  pub asset: AssetDispatcher,
}

impl Routed {
  pub fn new(asset: AssetDispatcher) -> Arc<Self> {
    Arc::new(Self { asset })
  }
}

impl AssetHandler for Routed {
  async fn request(&self, request: AssetRequest) -> Result<Reply<AssetGotReply>, HandlerError<AssetNotFoundReply>> {
    self.asset.request(request).await
  }
}

impl AudioHandler for Routed {
  async fn volume_up(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn volume_down(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn set_volume(&self, _payload: SetVolume) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn mute_toggle(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn set_mute(&self, _payload: SetMute) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn tts(&self, _payload: Tts) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn tts_cancel(&self, _payload: TtsCancel) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn tts_cancel_all(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn earcon(&self, _payload: Earcon) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl GeoHandler for Routed {
  async fn get_once(&self, _request: GeoGetOnce) -> Result<Reply<GeoGetOnceReply>, HandlerError<GeoErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn watch(&self, _payload: GeoWatch) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn unwatch(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl LibraryHandler for Routed {
  async fn browse(
    &self,
    _request: LibraryBrowseRequest,
  ) -> Result<Reply<BrowseReply>, HandlerError<LibraryErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn resolve_context(
    &self,
    _request: LibraryResolveContextRequest,
  ) -> Result<Reply<ContextResolveReply>, HandlerError<LibraryErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn search(
    &self,
    _request: LibrarySearchRequest,
  ) -> Result<Reply<SearchReply>, HandlerError<LibraryErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn recommendations(
    &self,
    _request: LibraryRecommendationsRequest,
  ) -> Result<Reply<RecommendationsReply>, HandlerError<LibraryErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn favorites_list(
    &self,
    _request: LibraryFavoritesListRequest,
  ) -> Result<Reply<FavoritesListReply>, HandlerError<LibraryErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn favorites_contains(
    &self,
    _request: LibraryFavoritesContainsRequest,
  ) -> Result<Reply<FavoritesContainsReply>, HandlerError<LibraryErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn favorites_toggle(&self, _payload: FavoritesToggle) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn favorites_set(&self, _payload: FavoritesSet) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn favorites_set_many(&self, _payload: FavoritesSetMany) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl LyricsHandler for Routed {
  async fn get(&self, _request: LyricsRequest) -> Result<Reply<LyricsReply>, HandlerError<LyricsErrorReply>> {
    Err(WireError::Unsupported.into())
  }
}

impl NetHandler for Routed {
  async fn fetch(
    &self,
    _request: NetFetchRequestMsg,
  ) -> Result<Reply<NetFetchReply>, HandlerError<NetFetchErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn ws_open(&self, _request: NetWsOpen) -> Result<Reply<NetWsOpenReply>, HandlerError<NetWsErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn ws_close(&self, _payload: NetWsClose) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn ws_send(&self, _payload: NetWsSend) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn stream_open(&self, _payload: NetStreamOpen) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn stream_cancel(&self, _payload: NetStreamCancel) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl NotificationsHandler for Routed {
  async fn invoke_positive(&self, _payload: NotificationInvoke) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn invoke_negative(&self, _payload: NotificationInvoke) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn ancs_auth_state_changed(&self, _payload: AncsAuthState) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl PhoneHandler for Routed {
  async fn state_get(&self) -> Result<Reply<PhoneStateReply>, HandlerError<::core::convert::Infallible>> {
    Err(WireError::Unsupported.into())
  }
  async fn answer(&self, _payload: PhoneCallAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn accept(&self, _payload: PhoneAcceptAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn decline(&self, _payload: PhoneCallAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn end(&self, _payload: PhoneCallAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn end_typed(&self, _payload: PhoneEndAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn hold(&self, _payload: PhoneCallAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn unhold(&self, _payload: PhoneCallAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn initiate(&self, _payload: PhoneInitiateAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn swap(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn merge(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn mute(&self, _payload: PhoneMuteAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn dtmf(&self, _payload: PhoneDtmfAction) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl PlayerHandler for Routed {
  async fn play(&self, _payload: PlayUri) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn queue(&self, _payload: QueueUri) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn pause(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn resume(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn skip_next(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn skip_prev(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn skip_to_index(&self, _payload: SkipToIndex) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn seek_to(&self, _payload: SeekTo) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn set_shuffle(&self, _payload: SetShuffle) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn set_repeat(&self, _payload: SetRepeat) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn set_speed(&self, _payload: SetSpeed) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn set_crossfade(&self, _payload: SetCrossfade) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn snapshot_request(
    &self,
    _request: PlayerSnapshotRequest,
  ) -> Result<Reply<PlayerSnapshotAck>, HandlerError<PlayerErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn transfer_to(&self, _payload: TransferTo) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl SystemHandler for Routed {
  async fn ota_asset_range(
    &self,
    _id: Uuid,
    _request: OtaAssetRange,
  ) -> Result<Reply<OtaAssetRangeReply>, HandlerError<OtaAssetRangeRejected>> {
    Err(WireError::Unsupported.into())
  }
  async fn keepalive(
    &self,
    _request: KeepalivePing,
  ) -> Result<Reply<KeepaliveAck>, HandlerError<::core::convert::Infallible>> {
    Err(WireError::Unsupported.into())
  }
  async fn ota_progress(&self, _payload: OtaProgress) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn ota_error(&self, _payload: OtaError) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn ota_finished(&self, _payload: OtaFinished) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn ota_asset_range_abandon(&self, _payload: OtaAssetRangeAbandon) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn device_nickname_changed(&self, _payload: DeviceNicknameReply) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn log_entry(&self, _payload: LogEntry) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl TransferHandler for Routed {
  async fn ack(&self, _payload: TransferAck) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn fragment(&self, _payload: TransferFragment) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn abandon(&self, _payload: TransferAbandon) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl TunnelHandler for Routed {
  async fn open(&self, _request: TunnelOpen) -> Result<Reply<TunnelOpenReply>, HandlerError<TunnelErrorReply>> {
    Err(WireError::Unsupported.into())
  }
  async fn data(&self, _payload: TunnelData) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn ack(&self, _payload: TunnelAck) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn close(&self, _payload: TunnelClosed) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl VoiceHandler for Routed {
  async fn stream_open(&self, _payload: VoiceStreamOpen) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn frame(&self, _payload: VoiceFrame) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn stream_close(&self, _payload: VoiceStreamClose) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn dispatched(&self, _payload: VoiceDispatched) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn dispatch_failed(&self, _payload: VoiceDispatchFailed) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl WebappHandler for Routed {
  async fn doc_changed(&self, _payload: WebappDocChanged) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn config_changed(&self, _payload: WebappConfigChanged) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn webapp_installed(&self, _payload: WebappInfo) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn active_changed(&self, _payload: WebappActiveChanged) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl ForwardHandler for Routed {
  async fn routed(&self, _payload: ForwardRouted) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl EnvelopeHandler for Routed {
  async fn version(&self, _payload: BridgeThingMeta) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn error(&self, _payload: WireError) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn ack(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn done(&self) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}
