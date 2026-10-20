use std::{
  collections::VecDeque,
  ops::Range,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_sdk_runtime::rt;
#[cfg(target_arch = "wasm32")]
use bridgething_sdk_runtime::rt::Instant;
use bytes::Bytes;
use libbridgething::{
  Priority, WebappError,
  gateway::{
    BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewayTransferMsg, BridgeToGatewayWebappMsg,
    GatewayToBridgeMsg, GatewayToBridgeMsgData, GatewayToBridgeTransferMsg, GatewayToBridgeWebappMsg, TransferAbandon,
    TransferAck, TransferBody, TransferFragment, TransferRef, WebappBlobFailed, WebappBlobPut, WebappBlobPutAck,
    WebappBlobStored, WebappResource, WebappResourceReply,
  },
  wire::{MsgMeta, ResponseMeta, WireError},
};
use tokio::sync::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use tokio::time::Instant;
use uuid::Uuid;

use crate::bundle::fetch::sha256_hex;

pub const WIRE_TIMEOUT: Duration = Duration::from_secs(3);
pub const FRAGMENT: usize = 4096;

pub fn pattern(len: usize) -> Vec<u8> {
  (0..len).map(|i| (i % 251) as u8).collect()
}

type Lanes = Arc<Mutex<Vec<(Priority, GatewayToBridgeMsgData)>>>;

pub struct FakeDevice {
  outbound: mpsc::UnboundedReceiver<GatewayToBridgeMsg>,
  inbound: Option<mpsc::UnboundedSender<BridgeToGatewayMsg>>,
  pending: VecDeque<GatewayToBridgeMsg>,
  lanes: Lanes,
}

pub struct DeviceEar {
  heard: mpsc::UnboundedSender<GatewayToBridgeMsg>,
  lanes: Lanes,
}

impl DeviceEar {
  pub fn hear(&self, priority: Priority, msg: GatewayToBridgeMsg) {
    self.lanes.lock().unwrap().push((priority, msg.data.clone()));
    let _ = self.heard.send(msg);
  }
}

pub fn fake_device() -> (FakeDevice, DeviceEar, mpsc::UnboundedReceiver<BridgeToGatewayMsg>) {
  let (heard, outbound) = mpsc::unbounded_channel();
  let (inbound, said) = mpsc::unbounded_channel();
  let lanes = Lanes::default();
  (
    FakeDevice {
      outbound,
      inbound: Some(inbound),
      pending: VecDeque::new(),
      lanes: lanes.clone(),
    },
    DeviceEar { heard, lanes },
    said,
  )
}

#[cfg(not(target_arch = "wasm32"))]
pub fn linked_gateway() -> (bridgething_gateway::Gateway, FakeDevice) {
  let (companion_io, device) = linked_io();
  (bridgething_gateway::Gateway::from_io(companion_io), device)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn linked_io() -> (tokio::io::DuplexStream, FakeDevice) {
  use futures::{SinkExt, StreamExt};
  use libbridgething::protocol::{BridgeEndec, DecodedFrame};
  use tokio_util::codec::Framed;

  let (companion_io, device_io) = tokio::io::duplex(1024 * 1024);
  let (device, ear, mut said) = fake_device();
  tokio::spawn(async move {
    let mut framed = Framed::new(device_io, BridgeEndec::default());
    loop {
      tokio::select! {
        item = framed.next() => match item {
          Some(Ok(DecodedFrame::Frame(frame))) => ear.hear(frame.priority, frame.msg),
          _ => break,
        },
        msg = said.recv() => match msg {
          Some(msg) => { let _ = framed.send(msg).await; }
          None => break,
        },
      }
    }
  });
  (companion_io, device)
}

impl FakeDevice {
  pub fn lanes_of<T>(&self, pick: impl Fn(&GatewayToBridgeMsgData) -> Option<T>) -> Vec<Priority> {
    self
      .lanes
      .lock()
      .unwrap()
      .iter()
      .filter(|(_, data)| pick(data).is_some())
      .map(|(priority, _)| *priority)
      .collect()
  }

  pub fn fragment_lanes(&self) -> Vec<Priority> {
    self.lanes_of(|data| match data {
      GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Fragment(fragment)) => Some(fragment.offset),
      _ => None,
    })
  }

  pub async fn next_matching<T>(&mut self, pick: impl Fn(&GatewayToBridgeMsg) -> Option<T>) -> T {
    self.next_matching_within(WIRE_TIMEOUT, pick).await
  }

  pub async fn next_matching_within<T>(
    &mut self,
    window: Duration,
    pick: impl Fn(&GatewayToBridgeMsg) -> Option<T>,
  ) -> T {
    if let Some(at) = self.pending.iter().position(|msg| pick(msg).is_some()) {
      let msg = self.pending.remove(at).expect("the position just found");
      return pick(&msg).expect("the predicate that just matched");
    }
    let deadline = Instant::now() + window;
    loop {
      let msg = rt::timeout(deadline.saturating_duration_since(Instant::now()), self.outbound.recv())
        .await
        .expect("the subject went quiet while a message was expected")
        .expect("the link closed");
      if let Some(hit) = pick(&msg) {
        return hit;
      }
      self.pending.push_back(msg);
    }
  }

  pub async fn nothing_matching<T>(
    &mut self,
    window: Duration,
    pick: impl Fn(&GatewayToBridgeMsg) -> Option<T>,
  ) -> bool {
    if self.pending.iter().any(|msg| pick(msg).is_some()) {
      return false;
    }
    let deadline = Instant::now() + window;
    loop {
      match rt::timeout(deadline.saturating_duration_since(Instant::now()), self.outbound.recv()).await {
        Err(_) | Ok(None) => return true,
        Ok(Some(msg)) => {
          if pick(&msg).is_some() {
            return false;
          }
          self.pending.push_back(msg);
        }
      }
    }
  }

  fn send(&self, id: Uuid, meta: MsgMeta, data: BridgeToGatewayMsgData) {
    if let Some(inbound) = &self.inbound {
      let _ = inbound.send(BridgeToGatewayMsg { id, meta, data });
    }
  }

  pub fn event(&self, data: BridgeToGatewayMsgData) {
    self.send(Uuid::now_v7(), MsgMeta::Event, data);
  }

  pub fn request(&self, data: BridgeToGatewayMsgData) -> Uuid {
    let id = Uuid::now_v7();
    self.send(id, MsgMeta::Request, data);
    id
  }

  pub fn respond(&self, request_id: Uuid, data: BridgeToGatewayMsgData) {
    self.send(Uuid::now_v7(), MsgMeta::Response(ResponseMeta { request_id }), data);
  }

  pub fn refuse(&self, request_id: Uuid, error: WireError) {
    self.respond(request_id, BridgeToGatewayMsgData::Error(error));
  }

  pub fn hang_up(&mut self) {
    self.inbound = None;
  }

  pub async fn next_resource_request(&mut self) -> (Uuid, WebappResource) {
    self
      .next_matching(|msg| match &msg.data {
        GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::Resource(request)) => Some((msg.id, request.clone())),
        _ => None,
      })
      .await
  }

  pub fn resource(&self, request_id: Uuid, reply: WebappResourceReply) {
    self.respond(
      request_id,
      BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::Resource(reply)),
    );
  }

  pub fn stream_resource(&self, request_id: Uuid, request: &WebappResource, body: &[u8], transfer_id: Uuid) {
    let digest = sha256_hex(body);
    self.resource(
      request_id,
      WebappResourceReply {
        id: request.id,
        kind: request.kind,
        sha256: digest.clone(),
        mime: Some("text/html".into()),
        body: Some(TransferBody::Stream(TransferRef {
          id: transfer_id,
          total_size: body.len() as u32,
          sha256: Some(digest),
        })),
      },
    );
  }

  pub fn stream(&self, transfer_id: Uuid, body: &[u8], span: Range<usize>, fragment_bytes: usize) {
    let mut offset = span.start;
    while offset < span.end {
      let end = (offset + fragment_bytes).min(span.end);
      self.event(BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Fragment(
        TransferFragment {
          transfer_id,
          offset: offset as u32,
          bytes: Bytes::copy_from_slice(&body[offset..end]),
        },
      )));
      offset = end;
    }
  }

  pub async fn next_fragment(&mut self, transfer_id: Uuid) -> TransferFragment {
    self
      .next_matching(|msg| match &msg.data {
        GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Fragment(fragment))
          if fragment.transfer_id == transfer_id =>
        {
          Some(fragment.clone())
        }
        _ => None,
      })
      .await
  }

  pub async fn no_fragment(&mut self, transfer_id: Uuid, window: Duration) -> bool {
    self
      .nothing_matching(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Fragment(fragment))
          if fragment.transfer_id == transfer_id =>
        {
          Some(fragment.offset)
        }
        _ => None,
      })
      .await
  }

  pub async fn await_ack(&mut self, transfer_id: Uuid, received: u32) {
    self
      .next_matching(|msg| match &msg.data {
        GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Ack(ack))
          if ack.transfer_id == transfer_id && ack.received == received =>
        {
          Some(())
        }
        _ => None,
      })
      .await
  }

  pub fn ack(&self, transfer_id: Uuid, received: u32) {
    self.event(BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Ack(
      TransferAck { transfer_id, received },
    )));
  }

  pub async fn await_abandon(&mut self, transfer_id: Uuid) -> TransferAbandon {
    self.await_abandon_within(WIRE_TIMEOUT, transfer_id).await
  }

  pub async fn await_abandon_within(&mut self, window: Duration, transfer_id: Uuid) -> TransferAbandon {
    self
      .next_matching_within(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Abandon(abandon))
          if abandon.transfer_id == transfer_id =>
        {
          Some(abandon.clone())
        }
        _ => None,
      })
      .await
  }

  pub async fn no_abandon(&mut self, transfer_id: Uuid, window: Duration) -> bool {
    self
      .nothing_matching(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Abandon(abandon))
          if abandon.transfer_id == transfer_id =>
        {
          Some(())
        }
        _ => None,
      })
      .await
  }

  pub async fn await_blob_put(&mut self) -> (Uuid, WebappBlobPut) {
    self.await_blob_put_within(WIRE_TIMEOUT).await
  }

  pub async fn await_blob_put_within(&mut self, window: Duration) -> (Uuid, WebappBlobPut) {
    self
      .next_matching_within(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::BlobPut(put)) => Some((msg.id, put.clone())),
        _ => None,
      })
      .await
  }

  pub async fn no_blob_put(&mut self, window: Duration) -> bool {
    self
      .nothing_matching(window, |msg| match &msg.data {
        GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::BlobPut(put)) => Some(put.transfer.id),
        _ => None,
      })
      .await
  }

  pub fn ack_blob_put(&self, request_id: Uuid, resume_from_offset: u32) {
    self.respond(
      request_id,
      BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::BlobPutAck(WebappBlobPutAck {
        resume_from_offset,
      })),
    );
  }

  pub async fn receive_blob(&mut self, put: &WebappBlobPut) -> Vec<u8> {
    let mut received = Vec::with_capacity(put.transfer.total_size as usize);
    while received.len() < put.transfer.total_size as usize {
      received.extend_from_slice(&self.next_fragment(put.transfer.id).await.bytes);
      self.ack(put.transfer.id, received.len() as u32);
    }
    received
  }

  pub fn refuse_webapp(&self, request_id: Uuid, error: WebappError) {
    self.respond(
      request_id,
      BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::WebappError(error)),
    );
  }

  pub fn blob_stored(&self, put: &WebappBlobPut) {
    self.event(BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::BlobStored(
      WebappBlobStored {
        id: put.id,
        key: put.key.clone(),
        sha256: put.transfer.sha256.clone().expect("a blob put carries its digest"),
        size: put.transfer.total_size,
      },
    )));
  }

  pub fn blob_failed(&self, put: &WebappBlobPut, error: WebappError) {
    self.event(BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::BlobFailed(
      WebappBlobFailed {
        id: put.id,
        key: put.key.clone(),
        sha256: put.transfer.sha256.clone().expect("a blob put carries its digest"),
        error,
      },
    )));
  }
}
