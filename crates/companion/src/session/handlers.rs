use std::sync::Arc;

use bridgething_delivery::{
  serve::{net::NetDispatcher, tunnel::TunnelDispatcher},
  transfer::TransferReceiver,
};
use bridgething_gateway::{
  AssetHandler, AudioHandler, EnvelopeHandler, ForwardHandler, GeoHandler, HandlerError, LibraryHandler, LyricsHandler,
  NetHandler, NotificationsHandler, PhoneHandler, PlayerHandler, Reply, SystemHandler, TransferHandler, TunnelHandler,
  VoiceHandler, WebappHandler,
};
use libbridgething::{gateway::*, wire::WireError, *};
use uuid::Uuid;

use crate::{
  dispatch::{
    OtaInbound, asset::AssetDispatcher, audio::AudioDispatcher, extension::ExtensionDispatcher, geo::GeoDispatcher,
    library::LibraryDispatcher, lyrics::LyricsDispatcher, notifications::NotificationDispatcher,
    phone::PhoneDispatcher, player::PlayerDispatcher, system::SystemDispatcher, webapp::WebappDispatcher,
  },
  session::{observer::SessionObserver, ota::OtaLink},
  voice::dispatcher::VoiceDispatcher,
};

pub struct Peer {
  pub(crate) device_id: String,
  pub(crate) asset: AssetDispatcher,
  pub(crate) audio: Option<AudioDispatcher>,
  pub(crate) geo: Option<GeoDispatcher>,
  pub(crate) library: LibraryDispatcher,
  pub(crate) lyrics: LyricsDispatcher,
  pub(crate) net: NetDispatcher,
  pub(crate) notifications: Option<NotificationDispatcher>,
  pub(crate) phone: Option<PhoneDispatcher>,
  pub(crate) player: PlayerDispatcher,
  pub(crate) system: SystemDispatcher,
  pub(crate) tunnel: TunnelDispatcher,
  pub(crate) voice: Option<VoiceDispatcher>,
  pub(crate) webapp: WebappDispatcher,
  pub(crate) extensions: Option<Arc<ExtensionDispatcher>>,
  pub(crate) receiver: Arc<TransferReceiver>,
  pub(crate) ota: Arc<OtaLink>,
  pub(crate) observer: Arc<SessionObserver>,
}

impl Peer {
  pub fn receiver(&self) -> &Arc<TransferReceiver> {
    &self.receiver
  }

  fn audio(&self) -> Result<&AudioDispatcher, WireError> {
    self.audio.as_ref().ok_or(WireError::Unsupported)
  }

  fn geo(&self) -> Result<&GeoDispatcher, WireError> {
    self.geo.as_ref().ok_or(WireError::Unsupported)
  }

  fn notifications(&self) -> Result<&NotificationDispatcher, WireError> {
    self.notifications.as_ref().ok_or(WireError::Unsupported)
  }

  fn phone(&self) -> Result<&PhoneDispatcher, WireError> {
    self.phone.as_ref().ok_or(WireError::Unsupported)
  }

  fn voice(&self) -> Result<&VoiceDispatcher, WireError> {
    self.voice.as_ref().ok_or(WireError::Unsupported)
  }
}

impl AssetHandler for Peer {
  async fn request(&self, request: AssetRequest) -> Result<Reply<AssetGotReply>, HandlerError<AssetNotFoundReply>> {
    self.asset.request(request).await
  }
}

impl AudioHandler for Peer {
  async fn volume_up(&self) -> Result<(), WireError> {
    self.audio()?.volume_up().await
  }
  async fn volume_down(&self) -> Result<(), WireError> {
    self.audio()?.volume_down().await
  }
  async fn set_volume(&self, payload: SetVolume) -> Result<(), WireError> {
    self.audio()?.set_volume(payload).await
  }
  async fn mute_toggle(&self) -> Result<(), WireError> {
    self.audio()?.mute_toggle().await
  }
  async fn set_mute(&self, payload: SetMute) -> Result<(), WireError> {
    self.audio()?.set_mute(payload).await
  }
  async fn tts(&self, payload: Tts) -> Result<(), WireError> {
    self.audio()?.tts(payload).await
  }
  async fn tts_cancel(&self, payload: TtsCancel) -> Result<(), WireError> {
    self.audio()?.tts_cancel(payload).await
  }
  async fn tts_cancel_all(&self) -> Result<(), WireError> {
    self.audio()?.tts_cancel_all().await
  }
  async fn earcon(&self, payload: Earcon) -> Result<(), WireError> {
    self.audio()?.earcon(payload).await
  }
}

impl GeoHandler for Peer {
  async fn get_once(&self, request: GeoGetOnce) -> Result<Reply<GeoGetOnceReply>, HandlerError<GeoErrorReply>> {
    self.geo()?.get_once(request).await
  }
  async fn watch(&self, payload: GeoWatch) -> Result<(), WireError> {
    self.geo()?.watch(payload).await
  }
  async fn unwatch(&self) -> Result<(), WireError> {
    self.geo()?.unwatch().await
  }
}

impl LibraryHandler for Peer {
  async fn browse(&self, request: LibraryBrowseRequest) -> Result<Reply<BrowseReply>, HandlerError<LibraryErrorReply>> {
    self.library.browse(request).await
  }
  async fn resolve_context(
    &self,
    request: LibraryResolveContextRequest,
  ) -> Result<Reply<ContextResolveReply>, HandlerError<LibraryErrorReply>> {
    self.library.resolve_context(request).await
  }
  async fn search(&self, request: LibrarySearchRequest) -> Result<Reply<SearchReply>, HandlerError<LibraryErrorReply>> {
    self.library.search(request).await
  }
  async fn recommendations(
    &self,
    request: LibraryRecommendationsRequest,
  ) -> Result<Reply<RecommendationsReply>, HandlerError<LibraryErrorReply>> {
    self.library.recommendations(request).await
  }
  async fn favorites_list(
    &self,
    request: LibraryFavoritesListRequest,
  ) -> Result<Reply<FavoritesListReply>, HandlerError<LibraryErrorReply>> {
    self.library.favorites_list(request).await
  }
  async fn favorites_contains(
    &self,
    request: LibraryFavoritesContainsRequest,
  ) -> Result<Reply<FavoritesContainsReply>, HandlerError<LibraryErrorReply>> {
    self.library.favorites_contains(request).await
  }
  async fn favorites_toggle(&self, payload: FavoritesToggle) -> Result<(), WireError> {
    self.library.favorites_toggle(payload).await
  }
  async fn favorites_set(&self, payload: FavoritesSet) -> Result<(), WireError> {
    self.library.favorites_set(payload).await
  }
  async fn favorites_set_many(&self, payload: FavoritesSetMany) -> Result<(), WireError> {
    self.library.favorites_set_many(payload).await
  }
}

impl LyricsHandler for Peer {
  async fn get(&self, request: LyricsRequest) -> Result<Reply<LyricsReply>, HandlerError<LyricsErrorReply>> {
    self.lyrics.get(request).await
  }
}

impl NetHandler for Peer {
  async fn fetch(&self, request: NetFetchRequestMsg) -> Result<Reply<NetFetchReply>, HandlerError<NetFetchErrorReply>> {
    self.net.fetch(request).await
  }
  async fn ws_open(&self, request: NetWsOpen) -> Result<Reply<NetWsOpenReply>, HandlerError<NetWsErrorReply>> {
    self.net.ws_open(request).await
  }
  async fn ws_close(&self, payload: NetWsClose) -> Result<(), WireError> {
    self.net.ws_close(payload).await
  }
  async fn ws_send(&self, payload: NetWsSend) -> Result<(), WireError> {
    self.net.ws_send(payload).await
  }
  async fn stream_open(&self, payload: NetStreamOpen) -> Result<(), WireError> {
    self.net.stream_open(payload).await
  }
  async fn stream_cancel(&self, payload: NetStreamCancel) -> Result<(), WireError> {
    self.net.stream_cancel(payload).await
  }
}

impl NotificationsHandler for Peer {
  async fn invoke_positive(&self, payload: NotificationInvoke) -> Result<(), WireError> {
    self.notifications()?.invoke_positive(payload).await
  }
  async fn invoke_negative(&self, payload: NotificationInvoke) -> Result<(), WireError> {
    self.notifications()?.invoke_negative(payload).await
  }
  async fn ancs_auth_state_changed(&self, payload: AncsAuthState) -> Result<(), WireError> {
    self.observer.ancs(&self.device_id, payload);
    Ok(())
  }
}

impl PhoneHandler for Peer {
  async fn state_get(&self) -> Result<Reply<PhoneStateReply>, HandlerError<::core::convert::Infallible>> {
    self.phone()?.state_get().await
  }
  async fn answer(&self, payload: PhoneCallAction) -> Result<(), WireError> {
    self.phone()?.answer(payload).await
  }
  async fn accept(&self, payload: PhoneAcceptAction) -> Result<(), WireError> {
    self.phone()?.accept(payload).await
  }
  async fn decline(&self, payload: PhoneCallAction) -> Result<(), WireError> {
    self.phone()?.decline(payload).await
  }
  async fn end(&self, payload: PhoneCallAction) -> Result<(), WireError> {
    self.phone()?.end(payload).await
  }
  async fn end_typed(&self, payload: PhoneEndAction) -> Result<(), WireError> {
    self.phone()?.end_typed(payload).await
  }
  async fn hold(&self, payload: PhoneCallAction) -> Result<(), WireError> {
    self.phone()?.hold(payload).await
  }
  async fn unhold(&self, payload: PhoneCallAction) -> Result<(), WireError> {
    self.phone()?.unhold(payload).await
  }
  async fn initiate(&self, payload: PhoneInitiateAction) -> Result<(), WireError> {
    self.phone()?.initiate(payload).await
  }
  async fn mute(&self, payload: PhoneMuteAction) -> Result<(), WireError> {
    self.phone()?.mute(payload).await
  }
  async fn dtmf(&self, payload: PhoneDtmfAction) -> Result<(), WireError> {
    self.phone()?.dtmf(payload).await
  }
  async fn swap(&self) -> Result<(), WireError> {
    self.phone()?.swap().await
  }
  async fn merge(&self) -> Result<(), WireError> {
    self.phone()?.merge().await
  }
}

impl PlayerHandler for Peer {
  async fn play(&self, payload: PlayUri) -> Result<(), WireError> {
    self.player.play(payload).await
  }
  async fn queue(&self, payload: QueueUri) -> Result<(), WireError> {
    self.player.queue(payload).await
  }
  async fn skip_to_index(&self, payload: SkipToIndex) -> Result<(), WireError> {
    self.player.skip_to_index(payload).await
  }
  async fn seek_to(&self, payload: SeekTo) -> Result<(), WireError> {
    self.player.seek_to(payload).await
  }
  async fn set_shuffle(&self, payload: SetShuffle) -> Result<(), WireError> {
    self.player.set_shuffle(payload).await
  }
  async fn set_repeat(&self, payload: SetRepeat) -> Result<(), WireError> {
    self.player.set_repeat(payload).await
  }
  async fn set_speed(&self, payload: SetSpeed) -> Result<(), WireError> {
    self.player.set_speed(payload).await
  }
  async fn set_crossfade(&self, payload: SetCrossfade) -> Result<(), WireError> {
    self.player.set_crossfade(payload).await
  }
  async fn transfer_to(&self, payload: TransferTo) -> Result<(), WireError> {
    self.player.transfer_to(payload).await
  }
  async fn pause(&self) -> Result<(), WireError> {
    self.player.pause().await
  }
  async fn resume(&self) -> Result<(), WireError> {
    self.player.resume().await
  }
  async fn skip_next(&self) -> Result<(), WireError> {
    self.player.skip_next().await
  }
  async fn skip_prev(&self) -> Result<(), WireError> {
    self.player.skip_prev().await
  }
  async fn snapshot_request(
    &self,
    request: PlayerSnapshotRequest,
  ) -> Result<Reply<PlayerSnapshotAck>, HandlerError<PlayerErrorReply>> {
    self.player.snapshot_request(request).await
  }
}

impl SystemHandler for Peer {
  async fn ota_asset_range(
    &self,
    id: Uuid,
    request: OtaAssetRange,
  ) -> Result<Reply<OtaAssetRangeReply>, HandlerError<OtaAssetRangeRejected>> {
    self.system.ota_asset_range(id, request).await
  }
  async fn keepalive(
    &self,
    request: KeepalivePing,
  ) -> Result<Reply<KeepaliveAck>, HandlerError<::core::convert::Infallible>> {
    self.system.keepalive(request).await
  }
  async fn ota_progress(&self, payload: OtaProgress) -> Result<(), WireError> {
    self.system.ota_progress(payload).await
  }
  async fn ota_error(&self, payload: OtaError) -> Result<(), WireError> {
    self.system.ota_error(payload).await
  }
  async fn ota_finished(&self, payload: OtaFinished) -> Result<(), WireError> {
    self.system.ota_finished(payload).await
  }
  async fn ota_asset_range_abandon(&self, payload: OtaAssetRangeAbandon) -> Result<(), WireError> {
    self.system.ota_asset_range_abandon(payload).await
  }
  async fn device_nickname_changed(&self, payload: DeviceNicknameReply) -> Result<(), WireError> {
    if let Some(meta) = self.ota.nickname_changed(payload.nickname) {
      self.observer.device_meta(&self.device_id, meta);
    }
    Ok(())
  }
  async fn log_entry(&self, payload: LogEntry) -> Result<(), WireError> {
    self.system.log_entry(payload).await
  }
}

impl TransferHandler for Peer {
  async fn ack(&self, payload: TransferAck) -> Result<(), WireError> {
    self.asset.acks().note(payload.transfer_id, u64::from(payload.received));
    self.ota.transfer_ack(payload);
    Ok(())
  }
  async fn fragment(&self, payload: TransferFragment) -> Result<(), WireError> {
    self.receiver.on_fragment(payload);
    Ok(())
  }
  async fn abandon(&self, payload: TransferAbandon) -> Result<(), WireError> {
    self.receiver.on_abandon(payload);
    Ok(())
  }
}

impl TunnelHandler for Peer {
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

impl VoiceHandler for Peer {
  async fn stream_open(&self, payload: VoiceStreamOpen) -> Result<(), WireError> {
    self.voice()?.stream_open(payload).await
  }
  async fn frame(&self, payload: VoiceFrame) -> Result<(), WireError> {
    self.voice()?.frame(payload).await
  }
  async fn stream_close(&self, payload: VoiceStreamClose) -> Result<(), WireError> {
    self.voice()?.stream_close(payload).await
  }
  async fn dispatched(&self, payload: VoiceDispatched) -> Result<(), WireError> {
    self.voice()?.dispatched(payload).await
  }
  async fn dispatch_failed(&self, payload: VoiceDispatchFailed) -> Result<(), WireError> {
    self.voice()?.dispatch_failed(payload).await
  }
}

impl WebappHandler for Peer {
  async fn doc_changed(&self, payload: WebappDocChanged) -> Result<(), WireError> {
    self.webapp.doc_changed(payload).await
  }
  async fn webapp_installed(&self, payload: WebappInfo) -> Result<(), WireError> {
    self.ota.webapp_installed(payload.clone());
    self.webapp.webapp_installed(payload).await
  }
  async fn active_changed(&self, payload: WebappActiveChanged) -> Result<(), WireError> {
    if let Some(extensions) = &self.extensions {
      extensions.active_changed(&self.device_id, &payload);
    }
    self.webapp.active_changed(payload).await
  }
  async fn config_changed(&self, payload: WebappConfigChanged) -> Result<(), WireError> {
    if let Some(extensions) = &self.extensions {
      extensions.config_changed(&self.device_id, payload.id, &payload.key, payload.value);
    }
    Ok(())
  }
}

impl ForwardHandler for Peer {
  async fn routed(&self, payload: ForwardRouted) -> Result<(), WireError> {
    let Some(extensions) = &self.extensions else {
      return Err(WireError::Unsupported);
    };
    extensions.deliver(&self.device_id, payload);
    Ok(())
  }
}

impl EnvelopeHandler for Peer {
  async fn version(&self, payload: BridgeThingMeta) -> Result<(), WireError> {
    self.ota.device_meta(payload.clone());
    self.observer.device_meta(&self.device_id, payload);
    Ok(())
  }
  async fn error(&self, payload: WireError) -> Result<(), WireError> {
    tracing::warn!(?payload, "the device reported a protocol error");
    Ok(())
  }
  async fn ack(&self) -> Result<(), WireError> {
    Ok(())
  }
  async fn done(&self) -> Result<(), WireError> {
    Ok(())
  }
}
