use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::Duration,
};

use libbridgething::{
  gateway::{BridgeToGatewayTransferMsgEvent, TransferAbandon, TransferFragment},
  protocol::Compress,
};
use tokio::sync::watch;
use tokio_util::bytes::Bytes;
use uuid::Uuid;

use crate::bluetooth::{Address, BluetoothMan};

pub const OUTBOUND_FRAGMENT_LEN: usize = 4 * 1024;
pub const OUTBOUND_WINDOW: u32 = 64 * 1024;
pub const OUTBOUND_ACK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default)]
pub struct TransferOutbound {
  acks: Arc<Mutex<HashMap<Uuid, watch::Sender<u32>>>>,
}

impl TransferOutbound {
  pub fn register(&self, id: Uuid) -> watch::Receiver<u32> {
    let (tx, rx) = watch::channel(0);
    self.acks.lock().unwrap().insert(id, tx);
    rx
  }

  pub fn unregister(&self, id: Uuid) {
    self.acks.lock().unwrap().remove(&id);
  }

  pub fn note_ack(&self, id: Uuid, received: u32) {
    if let Some(tx) = self.acks.lock().unwrap().get(&id) {
      tx.send_if_modified(|acked| {
        if received > *acked {
          *acked = received;
          true
        } else {
          false
        }
      });
    }
  }

  pub async fn send_stream(
    &self,
    bluetooth: &BluetoothMan,
    address: Address,
    id: Uuid,
    bytes: Bytes,
    compress: Compress,
  ) -> bool {
    let mut ack_rx = self.register(id);
    let total = bytes.len();
    let mut offset: usize = 0;

    while offset < total {
      let window_base = *ack_rx.borrow() as usize;
      if offset >= window_base + OUTBOUND_WINDOW as usize {
        let waited = tokio::time::timeout(OUTBOUND_ACK_TIMEOUT, ack_rx.changed()).await;
        if waited.is_err() {
          tracing::warn!(%id, offset, "outbound transfer ack window stalled; abandoning");
          let abandon = BridgeToGatewayTransferMsgEvent::Abandon(TransferAbandon {
            transfer_id: id,
            reason: "ack window stalled".into(),
          });
          bluetooth.gateway_man.send_event(address, abandon).await;
          self.unregister(id);
          return false;
        }
        continue;
      }

      let len = OUTBOUND_FRAGMENT_LEN.min(total - offset);
      let fragment = BridgeToGatewayTransferMsgEvent::Fragment(TransferFragment {
        transfer_id: id,
        offset: offset as u32,
        bytes: bytes.slice(offset..offset + len),
      });
      bluetooth.gateway_man.send_event_bulk(address, fragment, compress).await;
      offset += len;
    }

    self.unregister(id);
    true
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn acks_advance_monotonically() {
    let outbound = TransferOutbound::default();
    let id = Uuid::now_v7();
    let rx = outbound.register(id);
    outbound.note_ack(id, 4096);
    outbound.note_ack(id, 1024);
    assert_eq!(*rx.borrow(), 4096);
    outbound.note_ack(id, 8192);
    assert_eq!(*rx.borrow(), 8192);
    outbound.unregister(id);
    outbound.note_ack(id, 100_000);
    assert_eq!(*rx.borrow(), 8192);
  }
}
