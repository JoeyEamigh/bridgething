use std::time::Duration;

use libbridgething::{
  GeoError,
  client::{
    BridgeToClientMsgData, ClientToBridgeGeoMsgDispatch, GeoErrorReply, GeoGetOnce, GeoGetOnceReply, GeoUnwatch,
    GeoWatch, GeoWatchReply,
  },
  gateway::{self, BridgeToGatewayGeoMsgCommand},
  wire::{RequestError, WireRequest},
};
use uuid::Uuid;

use super::{HandlerResult, MsgHandle};
use crate::{
  bluetooth::GatewayMan,
  state::{GEO_PERMISSION, GeoHandover, GeoLease, State},
};

pub struct GeoHandler {
  handle: MsgHandle,
}

impl GeoHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl ClientToBridgeGeoMsgDispatch for GeoHandler {
  type Output = HandlerResult;

  async fn watch(&self, params: GeoWatch) -> HandlerResult {
    let GeoWatch {
      accuracy,
      min_interval_ms,
    } = params;
    if !self.declares_geo().await {
      return self.respond_error::<GeoWatch>(GeoError::NotDeclared).await;
    }
    if !self.has_geo() {
      return self.respond_error::<GeoWatch>(GeoError::Unavailable).await;
    }
    let token = Uuid::now_v7();
    self
      .handle
      .state
      .geo_watchers
      .register(token, self.handle.from, accuracy, min_interval_ms);
    self
      .handle
      .respond_to::<GeoWatch>(GeoWatchReply {
        token: token.to_string(),
      })
      .await?;
    Ok(())
  }

  async fn unwatch(&self, params: GeoUnwatch) -> HandlerResult {
    let GeoUnwatch { token } = params;
    let Ok(uuid) = Uuid::parse_str(&token) else {
      tracing::trace!(%token, "geo.unwatch with malformed token; dropping");
      return Ok(());
    };
    if !self.handle.state.geo_watchers.unregister(uuid) {
      tracing::trace!(%token, "geo.unwatch with unknown token; dropping");
    }
    Ok(())
  }

  async fn get_once(&self, params: GeoGetOnce) -> HandlerResult {
    let GeoGetOnce { accuracy, max_age_s } = params;
    if !self.declares_geo().await {
      return self.respond_error::<GeoGetOnce>(GeoError::NotDeclared).await;
    }
    if !self.has_geo() {
      return self.respond_error::<GeoGetOnce>(GeoError::Unavailable).await;
    }
    if let Some(max_age_s) = max_age_s
      && let Some(position) = self
        .handle
        .state
        .geo_last_fix
        .fresher_than(Duration::from_secs(max_age_s.into()))
    {
      tracing::trace!(max_age_s, "geo.getOnce answered from the held fix; phone not woken");
      self
        .handle
        .respond_to::<GeoGetOnce>(GeoGetOnceReply { position })
        .await?;
      return Ok(());
    }
    let outbound = gateway::GeoGetOnce { accuracy };
    let primary = self.handle.state.capabilities.primary_addr();
    match self.handle.bluetooth.gateway_man.request(primary, outbound).await {
      Ok(reply) => {
        self.handle.state.geo_last_fix.record(reply.position);
        self
          .handle
          .respond_to::<GeoGetOnce>(GeoGetOnceReply {
            position: reply.position,
          })
          .await?;
      }
      Err(err) => self.respond_request_error::<GeoGetOnce>("geo.getOnce", err).await?,
    }
    Ok(())
  }
}

impl GeoHandler {
  async fn declares_geo(&self) -> bool {
    self.handle.state.active_webapp_has_permission(GEO_PERMISSION).await
  }

  fn has_geo(&self) -> bool {
    let snapshot = self.handle.state.capabilities.snapshot();
    snapshot.gateway.is_some() && snapshot.available.geo
  }

  async fn respond_error<R>(&self, error: GeoError) -> HandlerResult
  where
    R: WireRequest<Inbound = BridgeToClientMsgData, DomainError = GeoErrorReply>,
  {
    self
      .handle
      .respond_err::<R>(GeoErrorReply { error })
      .await
      .map_err(Into::into)
  }

  async fn respond_request_error<R>(&self, verb: &str, err: RequestError<gateway::GeoErrorReply>) -> HandlerResult
  where
    R: WireRequest<Inbound = BridgeToClientMsgData, DomainError = GeoErrorReply>,
  {
    let error = match err {
      RequestError::Domain(domain) => domain.error,
      RequestError::Protocol(err) => {
        tracing::warn!(?err, "{verb} protocol error");
        GeoError::Unavailable
      }
      RequestError::ResponseMismatch => {
        tracing::error!("{verb} response did not match expected shape");
        GeoError::Unavailable
      }
    };
    self.respond_error::<R>(error).await
  }
}

pub fn cleanup_owner_watchers(handle: &MsgHandle) {
  handle.state.geo_watchers.drain_for_owner(handle.from);
}

pub async fn hold_watch(state: State, gateway_man: GatewayMan) {
  let mut primary = state.capabilities.primary_subscription_rx();
  let mut watchers = state.geo_watchers.subscribe();
  let mut held = None;
  loop {
    let want = state.geo_watchers.lease_for(state.capabilities.primary_addr());
    if let Some(GeoHandover { unwatch, watch }) = GeoHandover::between(held, want) {
      if let Some(gateway) = unwatch {
        tracing::debug!(%gateway, "geo watch: stopping the companion that held it");
        gateway_man
          .send_command(gateway, BridgeToGatewayGeoMsgCommand::Unwatch)
          .await;
      }
      if let Some(GeoLease { gateway, watch }) = watch {
        tracing::debug!(%gateway, ?watch, "geo watch: running on the primary companion");
        gateway_man
          .send_command(
            gateway,
            BridgeToGatewayGeoMsgCommand::Watch(gateway::GeoWatch {
              accuracy: watch.accuracy,
              min_interval_ms: watch.min_interval_ms,
            }),
          )
          .await;
      }
      held = want;
    }
    let woke = tokio::select! {
      changed = primary.changed() => changed,
      changed = watchers.changed() => changed,
    };
    if woke.is_err() {
      break;
    }
  }
}
