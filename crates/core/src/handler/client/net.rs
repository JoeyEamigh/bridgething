use libbridgething::{
  NetError, WsError,
  client::{
    BridgeToClientNetMsgEvent, ClientToBridgeNetMsgDispatch, NetFetch, NetFetchErrorReply, NetFetchReply,
    NetStreamCancel, NetStreamOpen, NetWsClose, NetWsErrorReply, NetWsOpen, NetWsOpenReply, NetWsSend,
  },
  gateway::{self, BridgeToGatewayNetMsgCommand},
  wire::RequestError,
};

use super::{HandlerResult, MsgHandle};

pub struct NetHandler {
  handle: MsgHandle,
}

impl NetHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl ClientToBridgeNetMsgDispatch for NetHandler {
  type Output = HandlerResult;

  async fn fetch(&self, params: NetFetch) -> HandlerResult {
    let NetFetch { request } = params;
    let snapshot = self.handle.state.capabilities.snapshot();
    if snapshot.gateway.is_none() {
      return self
        .handle
        .respond_err::<NetFetch>(NetFetchErrorReply {
          error: NetError::NoGateway,
        })
        .await
        .map_err(Into::into);
    }
    if !snapshot.available.net_fetch {
      return self
        .handle
        .respond_err::<NetFetch>(NetFetchErrorReply {
          error: NetError::Unavailable,
        })
        .await
        .map_err(Into::into);
    }

    let outbound = gateway::NetFetchRequestMsg { request };
    let primary = self.handle.state.capabilities.primary_addr();
    match self.handle.bluetooth.gateway_man.request(primary, outbound).await {
      Ok(reply) => {
        self
          .handle
          .respond_to::<NetFetch>(NetFetchReply {
            response: reply.response,
          })
          .await?;
      }
      Err(RequestError::Domain(domain)) => {
        self
          .handle
          .respond_err::<NetFetch>(NetFetchErrorReply { error: domain.error })
          .await?;
      }
      Err(RequestError::Protocol(err)) => {
        tracing::warn!(?err, "net.fetch protocol error");
        self
          .handle
          .respond_err::<NetFetch>(NetFetchErrorReply {
            error: NetError::RequestFailed {
              reason: format!("{err:?}"),
            },
          })
          .await?;
      }
      Err(RequestError::ResponseMismatch) => {
        tracing::error!("net.fetch response did not match expected shape");
        self
          .handle
          .respond_err::<NetFetch>(NetFetchErrorReply {
            error: NetError::RequestFailed {
              reason: "response shape mismatch".into(),
            },
          })
          .await?;
      }
    }
    Ok(())
  }

  async fn ws_open(&self, params: NetWsOpen) -> HandlerResult {
    let open = params;
    let Some(primary) = self.handle.state.capabilities.primary_addr() else {
      return self
        .handle
        .respond_err::<NetWsOpen>(NetWsErrorReply {
          error: WsError::ConnectFailed {
            reason: "no gateway connected".into(),
          },
        })
        .await
        .map_err(Into::into);
    };
    if !self.handle.state.capabilities.snapshot().available.net_ws {
      return self
        .handle
        .respond_err::<NetWsOpen>(NetWsErrorReply {
          error: WsError::ConnectFailed {
            reason: "net.ws unavailable".into(),
          },
        })
        .await
        .map_err(Into::into);
    }

    let connection_id = open.connection_id;
    self
      .handle
      .state
      .ws_routes
      .register(connection_id, self.handle.from, primary);

    let outbound = gateway::NetWsOpen {
      connection_id,
      url: open.url,
      protocols: open.protocols,
      headers: open.headers,
    };
    match self.handle.bluetooth.gateway_man.request(Some(primary), outbound).await {
      Ok(reply) => {
        self
          .handle
          .respond_to::<NetWsOpen>(NetWsOpenReply {
            accepted_protocol: reply.accepted_protocol,
          })
          .await?;
      }
      Err(RequestError::Domain(domain)) => {
        self.handle.state.ws_routes.drop_id(connection_id);
        self
          .handle
          .respond_err::<NetWsOpen>(NetWsErrorReply { error: domain.error })
          .await?;
      }
      Err(RequestError::Protocol(err)) => {
        self.handle.state.ws_routes.drop_id(connection_id);
        tracing::warn!(?err, "net.ws.open protocol error");
        self
          .handle
          .respond_err::<NetWsOpen>(NetWsErrorReply {
            error: WsError::ConnectFailed {
              reason: format!("{err:?}"),
            },
          })
          .await?;
      }
      Err(RequestError::ResponseMismatch) => {
        self.handle.state.ws_routes.drop_id(connection_id);
        tracing::error!("net.ws.open response did not match expected shape");
        self
          .handle
          .respond_err::<NetWsOpen>(NetWsErrorReply {
            error: WsError::ProtocolError {
              reason: "response shape mismatch".into(),
            },
          })
          .await?;
      }
    }
    Ok(())
  }

  async fn ws_send(&self, params: NetWsSend) -> HandlerResult {
    let send = params;
    let owner = self.handle.state.ws_routes.lookup(send.connection_id);
    let gateway = self.handle.state.ws_routes.gateway_of(send.connection_id);
    let Some(gateway) = gateway.filter(|_| owner == Some(self.handle.from)) else {
      tracing::warn!(
        connection_id = %send.connection_id,
        ?owner,
        from = %self.handle.from,
        "net.ws.send for unknown or non-owned connection; dropping"
      );
      return Ok(());
    };
    let outbound = BridgeToGatewayNetMsgCommand::WsSend(gateway::NetWsSend {
      connection_id: send.connection_id,
      frame: send.frame,
    });
    self.handle.bluetooth.gateway_man.command_bulk(gateway, outbound).await;
    Ok(())
  }

  async fn ws_close(&self, params: NetWsClose) -> HandlerResult {
    let close = params;
    let gateway = self.handle.state.ws_routes.gateway_of(close.connection_id);
    let owner = self.handle.state.ws_routes.drop_id(close.connection_id);
    let Some(gateway) = gateway.filter(|_| owner == Some(self.handle.from)) else {
      tracing::warn!(
        connection_id = %close.connection_id,
        ?owner,
        from = %self.handle.from,
        "net.ws.close for unknown or non-owned connection; dropping"
      );
      return Ok(());
    };
    let outbound = BridgeToGatewayNetMsgCommand::WsClose(gateway::NetWsClose {
      connection_id: close.connection_id,
      code: close.code,
      reason: close.reason,
    });
    self.handle.bluetooth.gateway_man.command_bulk(gateway, outbound).await;
    Ok(())
  }

  async fn stream_open(&self, params: NetStreamOpen) -> HandlerResult {
    let open = params;
    let primary = match self.handle.state.capabilities.primary_addr() {
      Some(primary) if self.handle.state.capabilities.snapshot().available.net_fetch => primary,
      unusable => {
        let error = match unusable {
          Some(_) => NetError::Unavailable,
          None => NetError::NoGateway,
        };
        let event = BridgeToClientNetMsgEvent::StreamError(libbridgething::StreamError {
          stream_id: open.stream_id,
          error,
        });
        let _ = self.handle.state.bus.send_event(self.handle.from, event).await;
        return Ok(());
      }
    };

    self
      .handle
      .state
      .stream_routes
      .register(open.stream_id, self.handle.from, primary);
    let outbound = BridgeToGatewayNetMsgCommand::StreamOpen(gateway::NetStreamOpen {
      stream_id: open.stream_id,
      request: open.request,
    });
    self.handle.bluetooth.gateway_man.command_bulk(primary, outbound).await;
    Ok(())
  }

  async fn stream_cancel(&self, params: NetStreamCancel) -> HandlerResult {
    let cancel = params;
    let gateway = self.handle.state.stream_routes.gateway_of(cancel.stream_id);
    let owner = self.handle.state.stream_routes.drop_id(cancel.stream_id);
    let Some(gateway) = gateway.filter(|_| owner == Some(self.handle.from)) else {
      tracing::warn!(
        stream_id = %cancel.stream_id,
        ?owner,
        from = %self.handle.from,
        "net.stream.cancel for unknown or non-owned stream; dropping"
      );
      return Ok(());
    };
    let outbound = BridgeToGatewayNetMsgCommand::StreamCancel(gateway::NetStreamCancel {
      stream_id: cancel.stream_id,
    });
    self.handle.bluetooth.gateway_man.command_bulk(gateway, outbound).await;
    Ok(())
  }
}

pub async fn cleanup_owner_routes(handle: &MsgHandle) {
  for (connection_id, gateway) in handle.state.ws_routes.drain_for_owner(handle.from) {
    let outbound = BridgeToGatewayNetMsgCommand::WsClose(gateway::NetWsClose {
      connection_id,
      code: Some(1001),
      reason: Some("webapp disconnected".into()),
    });
    handle.bluetooth.gateway_man.command_bulk(gateway, outbound).await;
  }

  for (stream_id, gateway) in handle.state.stream_routes.drain_for_owner(handle.from) {
    let outbound = BridgeToGatewayNetMsgCommand::StreamCancel(gateway::NetStreamCancel { stream_id });
    handle.bluetooth.gateway_man.command_bulk(gateway, outbound).await;
  }
}
