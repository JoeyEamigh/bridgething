use std::net::SocketAddr;

use axum::{
  body::Bytes,
  extract::ws::{self, Utf8Bytes, WebSocket},
};
use futures::{
  SinkExt, StreamExt,
  stream::{SplitSink, SplitStream},
};
use libbridgething::{
  client::{BridgeToClientMsg, BridgeToClientMsgData, ClientToBridgeMsg, ClientToBridgeMsgData},
  protocol::{try_probe_envelope_json, try_probe_envelope_msgpack},
  wire::{MsgMeta, ResponseMeta, WireError},
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::handler::client::{ClientMode, PossibleRecvMsg, PossibleSendMsg, RecvMsg, RecvMsgData, RecvTx, SendRx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientEncoding {
  Json,
  Msgpack,
}

pub struct Connection {
  mode: ClientMode,
  encoding: ClientEncoding,
  address: SocketAddr,

  writer: SplitSink<WebSocket, ws::Message>,
  reader: SplitStream<WebSocket>,

  tx: RecvTx,
  rx: SendRx,

  cancel_token: CancellationToken,

  #[cfg(feature = "test-tap")]
  frame_tap: tokio::sync::broadcast::Sender<super::connman::TappedFrame>,
}

impl Connection {
  pub fn spawn(
    address: SocketAddr,
    ws: WebSocket,
    tx: RecvTx,
    rx: SendRx,
    cancel_token: CancellationToken,
    mode: ClientMode,
    #[cfg(feature = "test-tap")] frame_tap: tokio::sync::broadcast::Sender<super::connman::TappedFrame>,
  ) -> JoinHandle<()> {
    tracing::debug!("spawning listener for {address} in {:?} mode", &mode);
    let (writer, reader) = ws.split();

    tokio::spawn(async move {
      Self {
        mode,
        encoding: ClientEncoding::Json,
        address,

        writer,
        reader,

        tx,
        rx,

        cancel_token,

        #[cfg(feature = "test-tap")]
        frame_tap,
      }
      .listen()
      .await
    })
  }

  pub async fn listen(&mut self) {
    let (code, reason) = self.run().await;
    tracing::info!("({}) connection torn down: {:?} {}", &self.address, code, &reason);
    self.forward((code, reason)).await;
  }

  async fn run(&mut self) -> (ws::CloseCode, String) {
    loop {
      tokio::select! {
        ws_msg = self.reader.next() => {
          let Some(ws_msg) = ws_msg else {
            tracing::warn!("({}) connection closed unexpectedly!", &self.address);
            return (ws::close_code::ABNORMAL, "stream ended".to_string());
          };

          let ws_msg = match ws_msg {
            Ok(msg) => msg,
            Err(err) => {
              tracing::warn!("({}) error decoding websocket message: {:?}!", &self.address, &err);
              self.forward(err).await;
              return (ws::close_code::ABNORMAL, "decode error".to_string());
            }
          };

          match ws_msg {
            ws::Message::Text(text) => self.handle_text(text).await,
            ws::Message::Binary(payload) => self.handle_binary(payload).await,
            ws::Message::Ping(payload) => self.handle_ping(payload).await,
            ws::Message::Pong(payload) => self.handle_pong(payload).await,
            ws::Message::Close(frame) => {
              tracing::info!("connection from {} closed with frame {:?}", &self.address, &frame);
              return match frame {
                Some(frame) => (frame.code, frame.reason.as_str().to_string()),
                None => (ws::close_code::ABNORMAL, "no close frame".to_string()),
              };
            }
          };
        }

        Some(server_msg) = self.rx.recv() => {
          tracing::trace!("(outgoing: {}) sending message: {:?}", &self.address, server_msg);
          self.send(server_msg).await;
        }

        _ = self.cancel_token.cancelled() => {
          tracing::debug!("({}) connection was cancelled, shutting down", &self.address);
          self.close().await;
          return (ws::close_code::NORMAL, "cancelled".to_string());
        }
      }
    }
  }

  async fn handle_text(&mut self, text: Utf8Bytes) {
    tracing::trace!("(incoming: {}) new message: {}", &self.address, text.as_str());
    self.encoding = ClientEncoding::Json;
    match serde_json::from_str::<PossibleRecvMsg>(text.as_str()) {
      Ok(msg) => self.handle_decoded(msg).await,
      Err(error) => {
        let reason = match serde_json::from_str::<ClientToBridgeMsg>(text.as_str()) {
          Err(modern) => modern.to_string(),
          Ok(_) => error.to_string(),
        };
        self
          .nack_undecodable(try_probe_envelope_json(text.as_bytes()), &reason)
          .await
      }
    }
  }

  async fn handle_binary(&mut self, payload: Bytes) {
    tracing::trace!(
      "(incoming: {}) new msgpack message: {} bytes",
      &self.address,
      payload.len()
    );
    self.encoding = ClientEncoding::Msgpack;
    match rmp_serde::from_slice::<PossibleRecvMsg>(&payload) {
      Ok(msg) => self.handle_decoded(msg).await,
      Err(error) => {
        let reason = match rmp_serde::from_slice::<ClientToBridgeMsg>(&payload) {
          Err(modern) => modern.to_string(),
          Ok(_) => error.to_string(),
        };
        self
          .nack_undecodable(try_probe_envelope_msgpack(&payload), &reason)
          .await
      }
    }
  }

  async fn nack_undecodable(&mut self, probe: libbridgething::protocol::EnvelopeProbe, error: &str) {
    tracing::warn!(
      target: "bridgething::ws::decode",
      "({}) typed decode failed: surface={:?} event={:?} kind={:?} id={:?}: {error}",
      &self.address, probe.data_type, probe.data_event, probe.meta_kind, probe.id,
    );
    // commands get the nack too, matching how `dispatch` reports a failed command handler
    if (probe.is_request() || probe.is_command())
      && let Some(request_id) = probe.id
    {
      let nack = BridgeToClientMsg {
        id: Uuid::now_v7(),
        meta: MsgMeta::Response(ResponseMeta { request_id }),
        data: BridgeToClientMsgData::Error(WireError::Malformed {
          reason: error.to_owned(),
        }),
      };
      self.send(nack.into()).await;
    }
  }

  async fn handle_decoded(&mut self, msg: PossibleRecvMsg) {
    if self.mode != ClientMode::Stock
      && (matches!(msg, PossibleRecvMsg::Stock(_)) || matches!(msg, PossibleRecvMsg::StockInterApp { .. }))
    {
      tracing::warn!(
        "({}) received a stock message, falling back to stock mode...",
        &self.address
      );
      self.mode = ClientMode::Stock;

      self.forward(ForwardMsg::ChangeMode(ClientMode::Stock)).await;
    };

    if let PossibleRecvMsg::Modern(ClientToBridgeMsg {
      meta: MsgMeta::Response(meta_resp),
      data,
      ..
    }) = msg
    {
      tracing::trace!(
        "(incoming: {}) routing response-meta message to pending request {}",
        &self.address,
        meta_resp.request_id
      );
      self
        .forward(ForwardMsg::Response {
          request_id: meta_resp.request_id,
          data,
        })
        .await;
      return;
    }

    tracing::trace!("(incoming: {}) decoded message: {:?}", &self.address, &msg);
    self.forward(msg).await;
  }

  async fn handle_pong(&self, payload: Bytes) {
    tracing::trace!("({}) pong received? payload: {:?}", &self.address, payload);
  }

  async fn forward(&self, data: impl Into<ForwardMsg>) {
    let data = data.into();

    if let Err(err) = self.tx.send((self.address, data).into()).await {
      tracing::error!("({}) error sending message to connman: {:?}", &self.address, err);
    };
  }

  async fn send(&mut self, msg: PossibleSendMsg) {
    if msg.is_noop() {
      return;
    }

    #[cfg(feature = "test-tap")]
    match serde_json::to_string(&msg) {
      Ok(json) => {
        let _ = self.frame_tap.send(super::connman::TappedFrame {
          to: self.address,
          mode: self.mode,
          json,
        });
      }
      Err(err) => {
        tracing::error!(target: "bridgething::ws::connection::send", "({}) could not tap frame as json: {:?}", &self.address, err)
      }
    }

    let frame = match self.encoding {
      ClientEncoding::Json => match serde_json::to_string(&msg) {
        Ok(json) => {
          tracing::trace!(target: "bridgething::ws::connection::send", "sending json: {:?}", json);
          ws::Message::Text(json.into())
        }
        Err(err) => {
          return tracing::error!(target: "bridgething::ws::connection::send", "({}) error converting message to json!!: {:?}", &self.address, err);
        }
      },
      ClientEncoding::Msgpack => match rmp_serde::to_vec_named(&msg) {
        Ok(packed) => {
          tracing::trace!(target: "bridgething::ws::connection::send", "sending msgpack: {} bytes", packed.len());
          ws::Message::Binary(packed.into())
        }
        Err(err) => {
          return tracing::error!(target: "bridgething::ws::connection::send", "({}) error converting message to msgpack!!: {:?}", &self.address, err);
        }
      },
    };

    if let Err(err) = self.writer.send(frame).await {
      tracing::error!(target: "bridgething::ws::connection::send", "({}) error sending message to websocket!!: {:?}", &self.address, err);
    };
  }

  async fn close(&mut self) {
    if let Err(err) = self
      .writer
      .send(ws::Message::Close(Some(ws::CloseFrame {
        code: ws::close_code::NORMAL,
        reason: "bye".into(),
      })))
      .await
    {
      tracing::error!("({}) error sending message to websocket!!: {:?}", &self.address, err);
    };
  }

  async fn handle_ping(&mut self, payload: Bytes) {
    if let Err(err) = self.writer.send(ws::Message::Pong(payload)).await {
      tracing::error!("({}) error sending message to websocket: {:?}", &self.address, err);
    };
  }
}

#[derive(Debug)]
enum ForwardMsg {
  Msg(Uuid, PossibleRecvMsg),
  Response {
    request_id: Uuid,
    data: ClientToBridgeMsgData,
  },
  ConnectionClosed(ws::CloseCode, String),
  Error(axum::Error),
  ChangeMode(ClientMode),
}

impl From<(ws::CloseCode, String)> for ForwardMsg {
  fn from((close_code, msg): (ws::CloseCode, String)) -> Self {
    Self::ConnectionClosed(close_code, msg)
  }
}

impl From<axum::Error> for ForwardMsg {
  fn from(err: axum::Error) -> Self {
    Self::Error(err)
  }
}

impl From<PossibleRecvMsg> for ForwardMsg {
  fn from(msg: PossibleRecvMsg) -> Self {
    ForwardMsg::Msg(msg.uuid(), msg)
  }
}

impl From<(SocketAddr, ForwardMsg)> for RecvMsg {
  fn from((from, fwd): (SocketAddr, ForwardMsg)) -> Self {
    match fwd {
      ForwardMsg::Msg(id, data) => {
        let stock_msg_id = if let PossibleRecvMsg::StockInterApp { msg_id, .. } = data {
          Some(msg_id)
        } else {
          None
        };

        Self {
          id,
          from,
          data: data.into(),
          stock_msg_id,
        }
      }
      ForwardMsg::Response { request_id, data } => Self {
        id: Uuid::now_v7(),
        from,
        data: RecvMsgData::Response { request_id, data },
        stock_msg_id: None,
      },
      ForwardMsg::ConnectionClosed(close_code, msg) => Self {
        id: Uuid::now_v7(),
        from,
        data: RecvMsgData::ConnectionClosed(close_code, msg),
        stock_msg_id: None,
      },
      ForwardMsg::Error(err) => Self {
        id: Uuid::now_v7(),
        from,
        data: RecvMsgData::Error(err),
        stock_msg_id: None,
      },
      ForwardMsg::ChangeMode(mode) => Self {
        id: Uuid::now_v7(),
        from,
        data: RecvMsgData::ChangeMode(mode),
        stock_msg_id: None,
      },
    }
  }
}
