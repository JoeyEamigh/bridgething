use std::{sync::Arc, time::Duration};

use bridgething_sdk_runtime::{Connector, InboundHalf, OutboundHalf, TransportError, rt};
use bytes::{Bytes, BytesMut};
use libbridgething::{
  gateway::{BridgeToGatewayMsg, GatewayToBridgeMsg},
  protocol::{DecodedFrame, EndecError, GatewayEndec, PrioritizedFrame},
};
use tokio::sync::mpsc;
use tokio_util::codec::{Decoder, Encoder};

use crate::backend::LinkTransport;

fn map_endec(error: EndecError) -> TransportError {
  match error {
    EndecError::Io(io) => TransportError::Io(io),
    other => TransportError::Decode(other.to_string()),
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkWrite {
  Complete,
  Failed,
}

pub(crate) struct LinkFeed {
  pub(crate) bytes: mpsc::UnboundedSender<Vec<u8>>,
  pub(crate) credit: mpsc::UnboundedSender<LinkWrite>,
}

pub(crate) struct LinkConnector {
  device_id: String,
  transport: Arc<dyn LinkTransport>,
  bytes: mpsc::UnboundedReceiver<Vec<u8>>,
  credit: mpsc::UnboundedReceiver<LinkWrite>,
}

impl LinkConnector {
  pub(crate) fn open(device_id: &str, transport: Arc<dyn LinkTransport>) -> (Self, LinkFeed) {
    let (bytes_tx, bytes) = mpsc::unbounded_channel();
    let (credit_tx, credit) = mpsc::unbounded_channel();
    (
      Self {
        device_id: device_id.to_owned(),
        transport,
        bytes,
        credit,
      },
      LinkFeed {
        bytes: bytes_tx,
        credit: credit_tx,
      },
    )
  }
}

pub(crate) struct LinkOut {
  device_id: String,
  transport: Arc<dyn LinkTransport>,
  credit: mpsc::UnboundedReceiver<LinkWrite>,
  in_flight: Option<rt::Instant>,
}

const SLOW_WRITE_CYCLE: Duration = Duration::from_secs(2);

pub(crate) struct LinkIn {
  bytes: mpsc::UnboundedReceiver<Vec<u8>>,
  decoder: GatewayEndec,
  buffer: BytesMut,
}

impl Connector<crate::session::GatewayProtocol> for LinkConnector {
  type Out = LinkOut;
  type In = LinkIn;

  fn split(self) -> (LinkOut, LinkIn) {
    (
      LinkOut {
        device_id: self.device_id,
        transport: self.transport,
        credit: self.credit,
        in_flight: None,
      },
      LinkIn {
        bytes: self.bytes,
        decoder: GatewayEndec::default(),
        buffer: BytesMut::new(),
      },
    )
  }
}

impl OutboundHalf<crate::session::GatewayProtocol> for LinkOut {
  fn max_batch_bytes(&self) -> usize {
    self.transport.max_batch_bytes() as usize
  }

  fn encode(frame: PrioritizedFrame<GatewayToBridgeMsg>) -> Result<Bytes, TransportError> {
    let mut dst = BytesMut::new();
    GatewayEndec::default().encode(frame, &mut dst).map_err(map_endec)?;
    Ok(dst.freeze())
  }

  async fn ready(&mut self) -> Result<(), TransportError> {
    let Some(sent_at) = self.in_flight else {
      return Ok(());
    };
    match self.credit.recv().await {
      Some(LinkWrite::Complete) => {
        self.in_flight = None;
        let cycle = rt::Instant::now().saturating_duration_since(sent_at);
        if cycle >= SLOW_WRITE_CYCLE {
          tracing::warn!(
            device_id = %self.device_id,
            cycle_ms = cycle.as_millis() as u64,
            "link transport took an outbound batch but the peer was slow to make room"
          );
        }
        Ok(())
      }
      Some(LinkWrite::Failed) => {
        tracing::warn!(device_id = %self.device_id, "the link transport dropped a batch; closing the outbound half");
        Err(TransportError::Closed)
      }
      None => Err(TransportError::Closed),
    }
  }

  async fn send_batch(&mut self, batch: Bytes) -> Result<(), TransportError> {
    self.in_flight = Some(rt::Instant::now());
    let transport = self.transport.clone();
    let device_id = self.device_id.clone();
    let batch = batch.to_vec();
    tokio::task::spawn_blocking(move || transport.send(device_id, batch))
      .await
      .map_err(|error| {
        tracing::debug!(device_id = %self.device_id, %error, "an outbound batch never reached the link transport");
        TransportError::Decode(error.to_string())
      })
  }
}

impl InboundHalf<crate::session::GatewayProtocol> for LinkIn {
  async fn recv(&mut self) -> Option<Result<BridgeToGatewayMsg, TransportError>> {
    loop {
      match self.decoder.decode(&mut self.buffer) {
        Ok(Some(DecodedFrame::Frame(frame))) => return Some(Ok(frame.msg)),
        Ok(Some(DecodedFrame::Failed(error))) | Err(error) => {
          tracing::debug!(%error, "an inbound link frame did not decode");
          return Some(Err(map_endec(error)));
        }
        Ok(None) => {}
      }
      match self.bytes.recv().await {
        Some(bytes) => self.buffer.extend_from_slice(&bytes),
        None => {
          tracing::debug!("the inbound link feed closed");
          return None;
        }
      }
    }
  }
}
