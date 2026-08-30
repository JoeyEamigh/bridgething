use std::sync::Arc;

use bridgething_gateway::{
  AssetHandler, AudioHandler, EnvelopeHandler, ForwardHandler, GeoHandler, HandlerError, LibraryHandler, LyricsHandler,
  NetHandler, NotificationsHandler, PhoneHandler, PlayerHandler, Reply, SystemHandler, TransferHandler, TunnelHandler,
  VoiceHandler, WebappHandler,
};
use libbridgething::{gateway::*, wire::WireError, *};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::ota::service::OtaService;
#[cfg(not(target_arch = "wasm32"))]
use crate::serve::tunnel::TunnelDispatcher;

pub struct DeliveryHandlers {
  device_id: String,
  ota: Arc<OtaService>,
  #[cfg(not(target_arch = "wasm32"))]
  tunnel: TunnelDispatcher,
  pub announced: Notify,
}

impl DeliveryHandlers {
  #[cfg(not(target_arch = "wasm32"))]
  pub fn new(device_id: String, ota: Arc<OtaService>, tunnel: TunnelDispatcher) -> Arc<Self> {
    Arc::new(Self {
      device_id,
      ota,
      tunnel,
      announced: Notify::new(),
    })
  }

  #[cfg(target_arch = "wasm32")]
  pub fn new(device_id: String, ota: Arc<OtaService>) -> Arc<Self> {
    Arc::new(Self {
      device_id,
      ota,
      announced: Notify::new(),
    })
  }
}

impl AssetHandler for DeliveryHandlers {
  async fn request(&self, _request: AssetRequest) -> Result<Reply<AssetGotReply>, HandlerError<AssetNotFoundReply>> {
    Err(WireError::Unsupported.into())
  }
}

impl AudioHandler for DeliveryHandlers {
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

impl GeoHandler for DeliveryHandlers {
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

impl LibraryHandler for DeliveryHandlers {
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

impl LyricsHandler for DeliveryHandlers {
  async fn get(&self, _request: LyricsRequest) -> Result<Reply<LyricsReply>, HandlerError<LyricsErrorReply>> {
    Err(WireError::Unsupported.into())
  }
}

impl NetHandler for DeliveryHandlers {
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

impl NotificationsHandler for DeliveryHandlers {
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

impl PhoneHandler for DeliveryHandlers {
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

impl PlayerHandler for DeliveryHandlers {
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
  async fn transfer_to(&self, _payload: TransferTo) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn snapshot_request(
    &self,
    _request: PlayerSnapshotRequest,
  ) -> Result<Reply<PlayerSnapshotAck>, HandlerError<PlayerErrorReply>> {
    Err(WireError::Unsupported.into())
  }
}

impl SystemHandler for DeliveryHandlers {
  async fn ota_asset_range(
    &self,
    id: Uuid,
    request: OtaAssetRange,
  ) -> Result<Reply<OtaAssetRangeReply>, HandlerError<OtaAssetRangeRejected>> {
    self.ota.asset_range(&self.device_id, id, request).await
  }
  async fn keepalive(
    &self,
    request: KeepalivePing,
  ) -> Result<Reply<KeepaliveAck>, HandlerError<::core::convert::Infallible>> {
    Ok(KeepaliveAck { seq: request.seq }.into())
  }
  async fn ota_progress(&self, payload: OtaProgress) -> Result<(), WireError> {
    self.ota.progress(&self.device_id, payload);
    Ok(())
  }
  async fn ota_error(&self, payload: OtaError) -> Result<(), WireError> {
    self.ota.error(&self.device_id, payload);
    Ok(())
  }
  async fn ota_finished(&self, payload: OtaFinished) -> Result<(), WireError> {
    self.ota.finished(&self.device_id, payload);
    Ok(())
  }
  async fn ota_asset_range_abandon(&self, payload: OtaAssetRangeAbandon) -> Result<(), WireError> {
    self.ota.asset_range_abandon(&self.device_id, payload);
    Ok(())
  }
  async fn device_nickname_changed(&self, payload: DeviceNicknameReply) -> Result<(), WireError> {
    self.ota.nickname_changed(&self.device_id, payload.nickname);
    Ok(())
  }
  async fn log_entry(&self, payload: LogEntry) -> Result<(), WireError> {
    tracing::info!(target: "device", level = ?payload.level, "{}", payload.message);
    Ok(())
  }
}

impl TransferHandler for DeliveryHandlers {
  async fn ack(&self, payload: TransferAck) -> Result<(), WireError> {
    self
      .ota
      .transfer_ack(&self.device_id, payload.transfer_id, u64::from(payload.received));
    Ok(())
  }
  async fn fragment(&self, _payload: TransferFragment) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn abandon(&self, _payload: TransferAbandon) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

#[cfg(not(target_arch = "wasm32"))]
impl TunnelHandler for DeliveryHandlers {
  async fn open(&self, request: TunnelOpen) -> Result<Reply<TunnelOpenReply>, HandlerError<TunnelErrorReply>> {
    self.tunnel.open(request).await
  }
  async fn data(&self, payload: TunnelData) -> Result<(), WireError> {
    self.tunnel.data(payload).await
  }
  async fn ack(&self, payload: TunnelAck) -> Result<(), WireError> {
    self.tunnel.ack(payload).await
  }
  async fn close(&self, payload: TunnelClosed) -> Result<(), WireError> {
    self.tunnel.close(payload).await
  }
}

#[cfg(target_arch = "wasm32")]
impl TunnelHandler for DeliveryHandlers {
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

impl VoiceHandler for DeliveryHandlers {
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

impl WebappHandler for DeliveryHandlers {
  async fn doc_changed(&self, _payload: WebappDocChanged) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn config_changed(&self, _payload: WebappConfigChanged) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
  async fn webapp_installed(&self, payload: WebappInfo) -> Result<(), WireError> {
    self.ota.webapp_installed(&self.device_id, payload);
    Ok(())
  }
  async fn active_changed(&self, _payload: WebappActiveChanged) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl ForwardHandler for DeliveryHandlers {
  async fn routed(&self, _payload: ForwardRouted) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl EnvelopeHandler for DeliveryHandlers {
  async fn version(&self, payload: BridgeThingMeta) -> Result<(), WireError> {
    tracing::info!(image = %payload.image_version, app = %payload.app_version, "daemon announced");
    self.ota.device_meta(&self.device_id, payload);
    self.announced.notify_waiters();
    Ok(())
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
