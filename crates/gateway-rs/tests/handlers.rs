use std::{sync::Arc, time::Duration};

use bridgething_gateway::{
  AssetHandler, AudioHandler, EnvelopeHandler, ForwardHandler, Gateway, GeoHandler, HandlerError, LibraryHandler,
  LyricsHandler, NetHandler, NotificationsHandler, PhoneHandler, PlayerHandler, Reply, SystemHandler, TransferHandler,
  TunnelHandler, VoiceHandler, WebappHandler, route,
};
use libbridgething::{
  gateway::*,
  protocol::{BridgeEndec, DecodedFrame},
  wire::*,
  *,
};
use tokio::sync::{Semaphore, mpsc::UnboundedSender};
use uuid::Uuid;

#[derive(Default)]
struct Unsupported {
  gate: Option<Arc<Semaphore>>,
  seen: Option<UnboundedSender<String>>,
}

impl Unsupported {
  fn saw(&self, what: String) {
    if let Some(seen) = &self.seen {
      let _ = seen.send(what);
    }
  }
}

impl AssetHandler for Unsupported {
  async fn request(&self, _request: AssetRequest) -> Result<Reply<AssetGotReply>, HandlerError<AssetNotFoundReply>> {
    Err(WireError::Unsupported.into())
  }
}

impl AudioHandler for Unsupported {
  async fn volume_up(&self) -> Result<(), WireError> {
    self.saw("volume_up".to_string());
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

impl GeoHandler for Unsupported {
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

impl LibraryHandler for Unsupported {
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

impl LyricsHandler for Unsupported {
  async fn get(&self, _request: LyricsRequest) -> Result<Reply<LyricsReply>, HandlerError<LyricsErrorReply>> {
    Err(WireError::Unsupported.into())
  }
}

impl NetHandler for Unsupported {
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

impl NotificationsHandler for Unsupported {
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

impl PhoneHandler for Unsupported {
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

impl PlayerHandler for Unsupported {
  async fn play(&self, payload: PlayUri) -> Result<(), WireError> {
    if let Some(gate) = &self.gate {
      gate.acquire().await.expect("the gate outlives the handler").forget();
    }
    self.saw(format!("play {}", payload.uri));
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

impl SystemHandler for Unsupported {
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

impl TransferHandler for Unsupported {
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

impl TunnelHandler for Unsupported {
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

impl VoiceHandler for Unsupported {
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

impl WebappHandler for Unsupported {
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

impl ForwardHandler for Unsupported {
  async fn routed(&self, _payload: ForwardRouted) -> Result<(), WireError> {
    Err(WireError::Unsupported)
  }
}

impl EnvelopeHandler for Unsupported {
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

#[tokio::test]
async fn routing_closes_when_the_link_dies() {
  use bridgething_gateway::routing::spawn_routing;

  let (companion_io, daemon_io) = tokio::io::duplex(64 * 1024);
  let gateway = Gateway::from_io(companion_io);
  let inbound = gateway.events();
  let routing = spawn_routing(gateway.clone(), Arc::new(Unsupported::default()), inbound);

  drop(daemon_io);

  tokio::time::timeout(Duration::from_secs(2), routing.closed())
    .await
    .expect("the routing path notices the dead link");
}

#[tokio::test]
async fn a_refused_request_is_answered_rather_than_dropped() {
  use futures::{SinkExt, StreamExt};
  use libbridgething::{HttpMethod, NetFetchRequest, RedirectPolicy};
  use tokio_util::codec::Framed;
  use uuid::Uuid;

  let (companion_io, daemon_io) = tokio::io::duplex(64 * 1024);
  let request_id = Uuid::now_v7();
  let (replies, mut reply_rx) = tokio::sync::mpsc::unbounded_channel();
  tokio::spawn(async move {
    let mut framed = Framed::new(daemon_io, BridgeEndec::default());
    framed
      .send(BridgeToGatewayMsg {
        id: request_id,
        meta: MsgMeta::Request,
        data: BridgeToGatewayMsgData::Net(BridgeToGatewayNetMsg::Fetch(NetFetchRequestMsg {
          request: NetFetchRequest {
            url: "https://example.test".into(),
            method: HttpMethod::Get,
            headers: vec![],
            body: None,
            timeout_ms: None,
            redirect: RedirectPolicy::Follow,
          },
        })),
      })
      .await
      .expect("the request reaches the companion");
    while let Some(Ok(DecodedFrame::Frame(frame))) = framed.next().await {
      let _ = replies.send(frame.msg);
    }
  });

  let gateway = Gateway::from_io(companion_io);
  let mut inbound = gateway.events();
  let msg = tokio::time::timeout(Duration::from_secs(1), inbound.recv())
    .await
    .expect("not timed out")
    .expect("the request");

  route(&Arc::new(Unsupported::default()), msg, gateway.connection())
    .await
    .expect("the refusal goes out");

  let reply = tokio::time::timeout(Duration::from_secs(1), reply_rx.recv())
    .await
    .expect("not timed out")
    .expect("a reply");
  assert!(matches!(reply.meta, MsgMeta::Response(ResponseMeta { request_id: got }) if got == request_id));
  assert!(matches!(
    reply.data,
    GatewayToBridgeMsgData::Error(WireError::Unsupported)
  ));
}

async fn drive_commands(io: tokio::io::DuplexStream, commands: Vec<BridgeToGatewayMsgData>) {
  use futures::SinkExt;
  use tokio_util::codec::Framed;

  let mut framed = Framed::new(io, BridgeEndec::default());
  for data in commands {
    framed
      .send(BridgeToGatewayMsg {
        id: Uuid::now_v7(),
        meta: MsgMeta::Command,
        data,
      })
      .await
      .expect("the command reaches the companion");
  }
  futures::future::pending::<()>().await;
}

fn play(uri: &str) -> BridgeToGatewayMsgData {
  BridgeToGatewayMsgData::Player(BridgeToGatewayPlayerMsg::Play(PlayUri {
    uri: uri.to_string(),
    context: None,
  }))
}

#[tokio::test]
async fn a_surface_waiting_on_the_network_does_not_hold_up_the_others() {
  use bridgething_gateway::routing::spawn_routing;

  let (companion_io, daemon_io) = tokio::io::duplex(64 * 1024);
  tokio::spawn(drive_commands(
    daemon_io,
    vec![
      play("spotify:track:slow"),
      BridgeToGatewayMsgData::Audio(BridgeToGatewayAudioMsg::VolumeUp),
    ],
  ));

  let gate = Arc::new(Semaphore::new(0));
  let (seen, mut ran) = tokio::sync::mpsc::unbounded_channel();
  let gateway = Gateway::from_io(companion_io);
  let inbound = gateway.events();
  let _routing = spawn_routing(
    gateway,
    Arc::new(Unsupported {
      gate: Some(gate.clone()),
      seen: Some(seen),
    }),
    inbound,
  );

  let first = tokio::time::timeout(Duration::from_secs(2), ran.recv())
    .await
    .expect("the wheel is answered while the play round trip is still in flight")
    .expect("a handler ran");
  assert_eq!(first, "volume_up");

  gate.add_permits(1);
  let second = tokio::time::timeout(Duration::from_secs(2), ran.recv())
    .await
    .expect("not timed out")
    .expect("a handler ran");
  assert_eq!(second, "play spotify:track:slow");
}

#[tokio::test]
async fn one_surface_keeps_its_arrival_order() {
  use bridgething_gateway::routing::spawn_routing;

  let (companion_io, daemon_io) = tokio::io::duplex(64 * 1024);
  tokio::spawn(drive_commands(
    daemon_io,
    vec![play("first"), play("second"), play("third")],
  ));

  let (seen, mut ran) = tokio::sync::mpsc::unbounded_channel();
  let gateway = Gateway::from_io(companion_io);
  let inbound = gateway.events();
  let _routing = spawn_routing(
    gateway,
    Arc::new(Unsupported {
      gate: None,
      seen: Some(seen),
    }),
    inbound,
  );

  let mut order = Vec::new();
  for _ in 0..3 {
    order.push(
      tokio::time::timeout(Duration::from_secs(2), ran.recv())
        .await
        .expect("not timed out")
        .expect("a handler ran"),
    );
  }
  assert_eq!(order, ["play first", "play second", "play third"]);
}
