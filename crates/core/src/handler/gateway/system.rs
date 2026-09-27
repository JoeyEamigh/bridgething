use std::{future::Future, pin::Pin};

use libbridgething::{
  gateway::{
    BridgeToGatewayMsg, BridgeToGatewaySystemMsg, DeviceGetNickname, DeviceNicknameRejected, DeviceNicknameReply,
    DeviceSetNickname, GatewayToBridgeSystemMsgCommandDispatch, GatewayToBridgeSystemMsgRequestDispatch,
    LauncherGestureGet, LauncherGestureReply, LauncherGestureSet, LogsSubscribe, LogsSubscribeReply, LogsTail,
    LogsTailReply, LogsUnsubscribe, OtaAbandon, OtaActivate, OtaBegin,
  },
  wire::MsgMeta,
};
use uuid::Uuid;

use super::handle::MsgHandle;
use crate::{
  bluetooth::OutboundGatewayMessage,
  handler::HandlerResult,
  ota::OtaOrchestrator,
  state::log_tap::{LogOwner, LogSink},
};

const NICKNAME_MAX_LEN: usize = 64;

#[derive(Debug)]
pub struct SystemHandler {
  handle: MsgHandle,
  ota: OtaOrchestrator,
}

impl SystemHandler {
  pub fn new(handle: MsgHandle, ota: OtaOrchestrator) -> Self {
    Self { handle, ota }
  }
}

impl GatewayToBridgeSystemMsgCommandDispatch for SystemHandler {
  type Output = HandlerResult;

  async fn ota_abandon(&self, params: OtaAbandon) -> HandlerResult {
    tracing::info!("({:?}) OtaAbandon update_id={}", &self.handle.address, params.update_id);
    self.ota.abandon(params.update_id).await;
    Ok(())
  }

  async fn ota_activate(&self, params: OtaActivate) -> HandlerResult {
    tracing::info!(
      "({:?}) OtaActivate expected={} staged piece(s)",
      &self.handle.address,
      params.expected.len()
    );
    self.ota.activate(params.expected).await;
    Ok(())
  }

  async fn launcher_gesture_set(&self, params: LauncherGestureSet) -> HandlerResult {
    self.handle.state.meta.set_launcher_gesture(params.gesture).await?;
    Ok(())
  }

  async fn cancel_update(&self) -> HandlerResult {
    tracing::info!("({:?}) CancelUpdate received", &self.handle.address);
    self.ota.cancel().await;
    Ok(())
  }

  async fn logs_unsubscribe(&self, params: LogsUnsubscribe) -> HandlerResult {
    let LogsUnsubscribe { token } = params;
    let Ok(uuid) = Uuid::parse_str(&token) else {
      tracing::trace!(%token, "({:?}) gateway logsUnsubscribe malformed token; dropping", &self.handle.address);
      return Ok(());
    };
    if !self.handle.state.log_tap.unsubscribe(uuid) {
      tracing::trace!(%token, "({:?}) gateway logsUnsubscribe unknown token; dropping", &self.handle.address);
    }
    Ok(())
  }
}

impl GatewayToBridgeSystemMsgRequestDispatch for SystemHandler {
  type Output = HandlerResult;

  async fn ota_begin(&self, params: OtaBegin) -> HandlerResult {
    tracing::info!(
      "({:?}) OtaBegin received: update_id={} transfer_id={} size={}",
      &self.handle.address,
      params.update_id,
      params.transfer.id,
      params.transfer.total_size,
    );
    let peer = self.handle.address;
    match self.ota.begin(params, peer).await {
      Ok(ack) => self.handle.respond_to::<OtaBegin>(ack).await,
      Err(rej) => self.handle.respond_err::<OtaBegin>(rej).await,
    }
    Ok(())
  }

  async fn device_get_nickname(&self) -> HandlerResult {
    let nickname = self.handle.state.meta.nickname();
    self
      .handle
      .respond_to::<DeviceGetNickname>(DeviceNicknameReply { nickname })
      .await;
    Ok(())
  }

  async fn device_set_nickname(&self, params: DeviceSetNickname) -> HandlerResult {
    let trimmed = params.nickname.trim();
    if trimmed.contains('\0') {
      self
        .handle
        .respond_err::<DeviceSetNickname>(DeviceNicknameRejected {
          reason: "nickname contains nul byte".into(),
        })
        .await;
      return Ok(());
    }
    if trimmed.chars().count() > NICKNAME_MAX_LEN {
      self
        .handle
        .respond_err::<DeviceSetNickname>(DeviceNicknameRejected {
          reason: format!("nickname longer than {NICKNAME_MAX_LEN} chars"),
        })
        .await;
      return Ok(());
    }

    let next: Option<String> = if trimmed.is_empty() {
      None
    } else {
      Some(trimmed.to_string())
    };
    self.handle.state.meta.set_nickname(next.clone()).await?;
    self
      .handle
      .respond_to::<DeviceSetNickname>(DeviceNicknameReply { nickname: next })
      .await;
    Ok(())
  }

  async fn launcher_gesture_get(&self) -> HandlerResult {
    let gesture = self.handle.state.meta.launcher_gesture();
    self
      .handle
      .respond_to::<LauncherGestureGet>(LauncherGestureReply { gesture })
      .await;
    Ok(())
  }

  async fn logs_tail(&self, params: LogsTail) -> HandlerResult {
    let entries = self.handle.state.log_tap.tail(
      params.source,
      &params.levels,
      params.filter.as_deref(),
      params.max_lines,
    );
    self.handle.respond_to::<LogsTail>(LogsTailReply { entries }).await;
    Ok(())
  }

  async fn logs_subscribe(&self, params: LogsSubscribe) -> HandlerResult {
    let bluetooth = self.handle.bluetooth.clone();
    let address = self.handle.address;
    let sink: LogSink = Box::new(move |entry| {
      let bluetooth = bluetooth.clone();
      Box::pin(async move {
        bluetooth
          .gateway_man
          .send_all(OutboundGatewayMessage::new(
            address,
            BridgeToGatewayMsg {
              id: Uuid::now_v7(),
              meta: MsgMeta::Event,
              data: BridgeToGatewaySystemMsg::LogEntry((*entry).clone()).into(),
            },
          ))
          .await;
        true
      }) as Pin<Box<dyn Future<Output = bool> + Send>>
    });
    let token = self.handle.state.log_tap.subscribe(
      LogOwner::Gateway(address),
      sink,
      params.source,
      params.levels,
      params.filter,
    );
    self
      .handle
      .respond_to::<LogsSubscribe>(LogsSubscribeReply {
        token: token.to_string(),
      })
      .await;
    Ok(())
  }
}
