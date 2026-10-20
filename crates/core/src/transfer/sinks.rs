use std::{
  collections::HashMap,
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
  time::Duration,
};

use tokio::{
  sync::{Notify, mpsc},
  time::timeout,
};
use tokio_util::bytes::{Bytes, BytesMut};
use uuid::Uuid;

pub const MEMORY_SINK_CAP: usize = 256 * 1024;
pub(crate) const FORWARD_ACK_INTERVAL: u32 = 16 * 1024;
pub(crate) const FORWARD_SPOOL_WARN_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const FORWARD_SPOOL_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckPolicy {
  OnReceipt,
  OnDrain,
}

#[derive(Debug)]
pub enum TransferEvent {
  Fragment { offset: u32, bytes: Bytes },
  Abandon { reason: String },
}

#[derive(Debug, Default)]
struct MemoryState {
  buf: BytesMut,
  failed: bool,
}

#[derive(Debug)]
enum Binding {
  Memory(MemoryState),
  Forward(ForwardState),
}

#[derive(Debug)]
struct ForwardState {
  tx: mpsc::UnboundedSender<TransferEvent>,
  queued: Arc<AtomicUsize>,
  last_ack: u32,
  policy: AckPolicy,
  warned: bool,
}

#[derive(Debug)]
pub struct ForwardStream {
  rx: mpsc::UnboundedReceiver<TransferEvent>,
  queued: Arc<AtomicUsize>,
}

impl ForwardStream {
  pub async fn recv(&mut self) -> Option<TransferEvent> {
    let event = self.rx.recv().await?;
    if let TransferEvent::Fragment { bytes, .. } = &event {
      self.queued.fetch_sub(bytes.len(), Ordering::AcqRel);
    }
    Some(event)
  }
}

#[derive(Clone, Debug, Default)]
pub struct TransferSinks {
  inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
  bindings: Mutex<HashMap<Uuid, Binding>>,
  progress: Notify,
}

impl TransferSinks {
  pub fn bind_memory(&self, id: Uuid) {
    self
      .inner
      .bindings
      .lock()
      .unwrap()
      .insert(id, Binding::Memory(MemoryState::default()));
  }

  pub fn bind_forward(&self, id: Uuid, policy: AckPolicy) -> ForwardStream {
    let (tx, rx) = mpsc::unbounded_channel::<TransferEvent>();
    let queued = Arc::new(AtomicUsize::new(0));
    self.inner.bindings.lock().unwrap().insert(
      id,
      Binding::Forward(ForwardState {
        tx,
        queued: queued.clone(),
        last_ack: 0,
        policy,
        warned: false,
      }),
    );
    ForwardStream { rx, queued }
  }

  pub fn unbind(&self, id: Uuid) {
    self.inner.bindings.lock().unwrap().remove(&id);
  }

  pub fn fragment(&self, id: Uuid, offset: u32, bytes: Bytes) -> Option<u32> {
    let received = offset.saturating_add(bytes.len() as u32);
    let mut bindings = self.inner.bindings.lock().unwrap();
    match bindings.get_mut(&id) {
      None => {
        tracing::trace!(%id, "fragment for unbound transfer; dropping");
        None
      }
      Some(Binding::Memory(state)) => {
        let consumed = if state.failed {
          false
        } else if offset as usize != state.buf.len() {
          tracing::warn!(%id, expected = state.buf.len(), got = offset, "transfer fragment out of order");
          state.failed = true;
          false
        } else if state.buf.len() + bytes.len() > MEMORY_SINK_CAP {
          tracing::warn!(%id, "transfer exceeds memory sink cap");
          state.failed = true;
          false
        } else {
          state.buf.extend_from_slice(&bytes);
          true
        };
        drop(bindings);
        self.inner.progress.notify_waiters();
        consumed.then_some(received)
      }
      Some(Binding::Forward(state)) => {
        let len = bytes.len();
        let queued = state.queued.load(Ordering::Acquire);
        if queued.saturating_add(len) > FORWARD_SPOOL_MAX_BYTES {
          tracing::warn!(%id, queued, "forward spool past hard ceiling; abandoning transfer");
          bindings.remove(&id);
          return None;
        }
        if queued.saturating_add(len) > FORWARD_SPOOL_WARN_BYTES && !state.warned {
          state.warned = true;
          tracing::warn!(%id, queued, "forward spool above soft cap; consumer is behind the sender");
        }
        state.queued.fetch_add(len, Ordering::AcqRel);
        if state.tx.send(TransferEvent::Fragment { offset, bytes }).is_err() {
          tracing::debug!(%id, "transfer consumer gone; unbinding");
          bindings.remove(&id);
          return None;
        }
        if state.policy == AckPolicy::OnDrain {
          return None;
        }
        if received.saturating_sub(state.last_ack) >= FORWARD_ACK_INTERVAL {
          state.last_ack = received;
          return Some(received);
        }
        None
      }
    }
  }

  pub fn abandon(&self, id: Uuid, reason: String) {
    let mut bindings = self.inner.bindings.lock().unwrap();
    match bindings.get_mut(&id) {
      None => {}
      Some(Binding::Memory(state)) => {
        tracing::debug!(%id, %reason, "transfer abandoned by sender");
        state.failed = true;
        drop(bindings);
        self.inner.progress.notify_waiters();
      }
      Some(Binding::Forward(_)) => {
        let Some(Binding::Forward(state)) = bindings.remove(&id) else {
          return;
        };
        let _ = state.tx.send(TransferEvent::Abandon { reason });
      }
    }
  }

  pub async fn collect_memory(&self, id: Uuid, total_size: u32, dur: Duration) -> Option<Bytes> {
    if total_size as usize > MEMORY_SINK_CAP {
      self.unbind(id);
      return None;
    }
    let gather = async {
      loop {
        let notified = self.inner.progress.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
          let mut bindings = self.inner.bindings.lock().unwrap();
          match bindings.get(&id) {
            Some(Binding::Memory(state)) if state.failed => {
              bindings.remove(&id);
              return None;
            }
            Some(Binding::Memory(state)) if state.buf.len() > total_size as usize => {
              tracing::warn!(%id, got = state.buf.len(), total = total_size, "transfer overshoots declared size");
              bindings.remove(&id);
              return None;
            }
            Some(Binding::Memory(state)) if state.buf.len() == total_size as usize => {
              let Some(Binding::Memory(state)) = bindings.remove(&id) else {
                return None;
              };
              return Some(state.buf.freeze());
            }
            Some(Binding::Memory(_)) => {}
            Some(Binding::Forward(_)) | None => return None,
          }
        }
        notified.await;
      }
    };
    match timeout(dur, gather).await {
      Ok(result) => result,
      Err(_) => {
        self.unbind(id);
        None
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn frag(b: &[u8]) -> Bytes {
    Bytes::copy_from_slice(b)
  }

  #[tokio::test]
  async fn forward_acks_on_receipt_without_the_consumer_draining() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    let _stream = sinks.bind_forward(id, AckPolicy::OnReceipt);

    let chunk = vec![0u8; FORWARD_ACK_INTERVAL as usize];
    let mut acks = Vec::new();
    let mut offset = 0u32;
    for _ in 0..8 {
      if let Some(received) = sinks.fragment(id, offset, frag(&chunk)) {
        acks.push(received);
      }
      offset += chunk.len() as u32;
    }

    assert_eq!(acks.len(), 8, "every ack-interval of receipt must ack, undrained");
    assert_eq!(acks.last().copied(), Some(offset));
  }

  #[tokio::test]
  async fn forward_receipt_acks_respect_the_interval() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    let _stream = sinks.bind_forward(id, AckPolicy::OnReceipt);

    let step = (FORWARD_ACK_INTERVAL / 4) as usize;
    let chunk = vec![0u8; step];
    let mut acks = Vec::new();
    let mut offset = 0u32;
    for _ in 0..8 {
      if let Some(received) = sinks.fragment(id, offset, frag(&chunk)) {
        acks.push(received);
      }
      offset += step as u32;
    }

    assert_eq!(acks, vec![FORWARD_ACK_INTERVAL, 2 * FORWARD_ACK_INTERVAL]);
  }

  #[tokio::test]
  async fn forward_spool_is_bounded_in_bytes_not_messages() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    let _stream = sinks.bind_forward(id, AckPolicy::OnReceipt);

    let tiny = vec![0u8; 64];
    let mut offset = 0u32;
    let mut acks = 0usize;
    for _ in 0..2_000 {
      if sinks.fragment(id, offset, frag(&tiny)).is_some() {
        acks += 1;
      }
      offset += tiny.len() as u32;
    }
    assert_eq!(
      acks,
      (offset / FORWARD_ACK_INTERVAL) as usize,
      "2000 undrained messages must all be accepted and acked on the interval"
    );

    let huge = vec![0u8; FORWARD_SPOOL_MAX_BYTES];
    assert_eq!(sinks.fragment(id, offset, frag(&huge)), None);
    assert_eq!(
      sinks.fragment(id, offset, frag(&tiny)),
      None,
      "the hard ceiling must have unbound the transfer"
    );
  }

  #[tokio::test]
  async fn forward_soft_cap_keeps_the_transfer_alive() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    let _stream = sinks.bind_forward(id, AckPolicy::OnReceipt);

    let chunk = vec![0u8; 64 * 1024];
    let mut offset = 0u32;
    while (offset as usize) < FORWARD_SPOOL_WARN_BYTES * 2 {
      sinks.fragment(id, offset, frag(&chunk));
      offset += chunk.len() as u32;
    }

    assert!(
      sinks.fragment(id, offset, frag(&chunk)).is_some(),
      "crossing the soft cap must keep acking, not abandon a transfer mid-write"
    );
  }

  #[tokio::test]
  async fn drain_policy_withholds_receipt_acks() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    let mut stream = sinks.bind_forward(id, AckPolicy::OnDrain);

    let chunk = vec![0u8; FORWARD_ACK_INTERVAL as usize * 4];
    for i in 0..4 {
      assert_eq!(
        sinks.fragment(id, i * chunk.len() as u32, frag(&chunk)),
        None,
        "a drain-paced sender is acked by its consumer, never on arrival"
      );
    }

    assert!(
      stream.recv().await.is_some(),
      "fragments are still delivered downstream"
    );
  }

  #[tokio::test]
  async fn memory_reassembles_in_offset_order() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    sinks.bind_memory(id);
    sinks.fragment(id, 0, frag(b"hello "));
    sinks.fragment(id, 6, frag(b"world"));
    let got = sinks
      .collect_memory(id, 11, Duration::from_secs(1))
      .await
      .expect("collected");
    assert_eq!(&got[..], b"hello world");
  }

  #[tokio::test]
  async fn collect_resolves_when_fragments_lag() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    sinks.bind_memory(id);
    let bg = {
      let sinks = sinks.clone();
      tokio::spawn(async move { sinks.collect_memory(id, 4, Duration::from_secs(1)).await })
    };
    sinks.fragment(id, 0, frag(b"ab"));
    sinks.fragment(id, 2, frag(b"cd"));
    let got = bg.await.unwrap().expect("collected");
    assert_eq!(&got[..], b"abcd");
  }

  #[tokio::test]
  async fn out_of_order_fails_the_stream() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    sinks.bind_memory(id);
    sinks.fragment(id, 2, frag(b"late"));
    assert!(sinks.collect_memory(id, 4, Duration::from_millis(200)).await.is_none());
  }

  #[tokio::test]
  async fn contiguous_overshoot_past_total_fails() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    sinks.bind_memory(id);
    sinks.fragment(id, 0, frag(b"hello"));
    sinks.fragment(id, 5, frag(b" world"));
    assert!(
      sinks.collect_memory(id, 5, Duration::from_millis(200)).await.is_none(),
      "an overshoot past total_size fails the stream"
    );
  }

  #[tokio::test]
  async fn over_cap_total_is_refused() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    sinks.bind_memory(id);
    assert!(
      sinks
        .collect_memory(id, (MEMORY_SINK_CAP + 1) as u32, Duration::from_millis(200))
        .await
        .is_none()
    );
  }

  #[tokio::test]
  async fn abandon_fails_a_pending_collect_immediately() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    sinks.bind_memory(id);
    let bg = {
      let sinks = sinks.clone();
      tokio::spawn(async move { sinks.collect_memory(id, 100, Duration::from_secs(5)).await })
    };
    sinks.fragment(id, 0, frag(b"partial"));
    sinks.abandon(id, "source evicted".into());
    let got = tokio::time::timeout(Duration::from_millis(500), bg)
      .await
      .expect("resolves well before the collect timeout")
      .unwrap();
    assert!(got.is_none());
  }

  #[tokio::test]
  async fn forward_relays_fragments_and_abandon() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    let mut rx = sinks.bind_forward(id, AckPolicy::OnReceipt);
    sinks.fragment(id, 0, frag(b"aa"));
    sinks.abandon(id, "curl gave up".into());

    match rx.recv().await.unwrap() {
      TransferEvent::Fragment { offset, bytes } => {
        assert_eq!(offset, 0);
        assert_eq!(&bytes[..], b"aa");
      }
      other => panic!("expected fragment, got {other:?}"),
    }
    match rx.recv().await.unwrap() {
      TransferEvent::Abandon { reason } => assert_eq!(reason, "curl gave up"),
      other => panic!("expected abandon, got {other:?}"),
    }
    sinks.fragment(id, 2, frag(b"bb"));
    assert!(rx.recv().await.is_none());
  }

  #[tokio::test]
  async fn undrained_consumer_is_not_abandoned() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    let _rx = sinks.bind_forward(id, AckPolicy::OnReceipt);
    let chunk = vec![0u8; 16 * 1024];
    let mut offset = 0u32;
    for _ in 0..64 {
      sinks.fragment(id, offset, frag(&chunk));
      offset += chunk.len() as u32;
      assert!(
        sinks.inner.bindings.lock().unwrap().contains_key(&id),
        "transfer abandoned at offset {offset} despite being far inside the spool bound"
      );
      tokio::task::yield_now().await;
    }
  }

  #[tokio::test]
  async fn dropped_forward_receiver_unbinds() {
    let sinks = TransferSinks::default();
    let id = Uuid::now_v7();
    let rx = sinks.bind_forward(id, AckPolicy::OnReceipt);
    drop(rx);
    sinks.fragment(id, 0, frag(b"aa"));
    for _ in 0..100 {
      if sinks.inner.bindings.lock().unwrap().get(&id).is_none() {
        return;
      }
      tokio::task::yield_now().await;
    }
    panic!("dropped forward receiver did not unbind");
  }

  #[tokio::test]
  async fn unknown_id_fragments_drop() {
    let sinks = TransferSinks::default();
    sinks.fragment(Uuid::now_v7(), 0, frag(b"zz"));
  }
}
