use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_sdk_runtime::rt;
use tokio::sync::Notify;
use uuid::Uuid;

pub const ACK_STALL_TIMEOUT: Duration = Duration::from_secs(30);
const SLOW_ACK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("transfer {transfer_id} stalled: fragment acks stopped at offset {offset}")]
pub struct TransferStalled {
  pub transfer_id: Uuid,
  pub offset: u64,
}

#[derive(Debug, Default)]
struct Entry {
  received: u64,
  notify: Arc<Notify>,
}

#[derive(Debug, Default)]
pub struct AckWindow {
  entries: Mutex<HashMap<Uuid, Entry>>,
}

impl AckWindow {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn open(&self, transfer_id: Uuid, resume_from: u64) {
    let mut entries = self.entries.lock().unwrap();
    let entry = entries.entry(transfer_id).or_default();
    if resume_from > entry.received {
      entry.received = resume_from;
      entry.notify.notify_waiters();
    }
  }

  pub fn note(&self, transfer_id: Uuid, received: u64) {
    let mut entries = self.entries.lock().unwrap();
    if let Some(entry) = entries.get_mut(&transfer_id)
      && received > entry.received
    {
      entry.received = received;
      entry.notify.notify_waiters();
    }
  }

  pub fn received_bytes(&self, transfer_id: Uuid) -> u64 {
    self
      .entries
      .lock()
      .unwrap()
      .get(&transfer_id)
      .map_or(0, |entry| entry.received)
  }

  pub fn finish(&self, transfer_id: Uuid) {
    let entry = self.entries.lock().unwrap().remove(&transfer_id);
    if let Some(entry) = entry {
      entry.notify.notify_waiters();
    }
  }

  pub async fn wait_for_progress(&self, transfer_id: Uuid, beyond: u64, timeout: Duration) -> bool {
    let notify = {
      let entries = self.entries.lock().unwrap();
      match entries.get(&transfer_id) {
        Some(entry) if entry.received > beyond => return true,
        Some(entry) => entry.notify.clone(),
        None => return false,
      }
    };

    let mut progressed = std::pin::pin!(notify.notified());
    progressed.as_mut().enable();
    {
      let entries = self.entries.lock().unwrap();
      match entries.get(&transfer_id) {
        Some(entry) if entry.received > beyond => return true,
        Some(entry) if Arc::ptr_eq(&entry.notify, &notify) => {}
        _ => return false,
      }
    }

    rt::timeout(timeout, progressed).await.is_ok() && self.received_bytes(transfer_id) > beyond
  }

  pub async fn await_window(
    &self,
    transfer_id: Uuid,
    offset: u64,
    window_bytes: u64,
    timeout: Duration,
  ) -> Result<(), TransferStalled> {
    let mut blocked_since = None;
    loop {
      let acked = self.received_bytes(transfer_id);
      if offset < acked.saturating_add(window_bytes) {
        if let Some(since) = blocked_since {
          let blocked = rt::Instant::now().saturating_duration_since(since);
          if blocked >= SLOW_ACK {
            tracing::warn!(
              %transfer_id,
              offset,
              acked,
              window_bytes,
              blocked_ms = blocked.as_millis() as u64,
              "transfer resumed after the peer stopped acking"
            );
          }
        }
        return Ok(());
      }
      let since = *blocked_since.get_or_insert_with(rt::Instant::now);
      if !self.wait_for_progress(transfer_id, acked, timeout).await {
        self.finish(transfer_id);
        tracing::warn!(
          %transfer_id,
          offset,
          acked,
          window_bytes,
          blocked_ms = rt::Instant::now().saturating_duration_since(since).as_millis() as u64,
          timeout_ms = timeout.as_millis() as u64,
          "transfer abandoned: the peer stopped acking inside the window"
        );
        return Err(TransferStalled { transfer_id, offset });
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use tokio::time::{Instant, timeout};

  use super::*;

  const IMMEDIATE: Duration = Duration::from_millis(500);

  #[tokio::test]
  async fn an_unknown_transfer_has_acked_nothing() {
    let window = AckWindow::new();
    assert_eq!(window.received_bytes(Uuid::now_v7()), 0);
  }

  #[tokio::test]
  async fn note_wakes_the_waiter_for_its_own_transfer() {
    let window = Arc::new(AckWindow::new());
    let id = Uuid::now_v7();
    window.open(id, 0);

    let waiter = tokio::spawn({
      let window = window.clone();
      async move { window.wait_for_progress(id, 0, Duration::from_secs(30)).await }
    });
    tokio::task::yield_now().await;

    let at = Instant::now();
    window.note(id, 4 * 1024);
    assert!(
      timeout(IMMEDIATE, waiter).await.expect("waiter woke").unwrap(),
      "an ack past prior must resolve the waiter as progressed"
    );
    assert!(at.elapsed() < IMMEDIATE, "note must wake the waiter immediately");
  }

  #[tokio::test]
  async fn finish_wakes_a_parked_waiter_instead_of_stranding_it_until_timeout() {
    let window = Arc::new(AckWindow::new());
    let id = Uuid::now_v7();
    window.open(id, 16 * 1024);

    let waiter = tokio::spawn({
      let window = window.clone();
      async move { window.wait_for_progress(id, 16 * 1024, Duration::from_secs(30)).await }
    });
    tokio::task::yield_now().await;

    let at = Instant::now();
    window.finish(id);
    assert!(
      !timeout(IMMEDIATE, waiter).await.expect("waiter woke").unwrap(),
      "finish must resolve the waiter as no-progress"
    );
    assert!(at.elapsed() < IMMEDIATE, "finish must wake the waiter immediately");
  }

  #[tokio::test]
  async fn progress_on_one_transfer_never_resolves_another_transfers_waiter() {
    let window = Arc::new(AckWindow::new());
    let watched = Uuid::now_v7();
    let other = Uuid::now_v7();
    window.open(watched, 0);
    window.open(other, 0);

    let waiter = tokio::spawn({
      let window = window.clone();
      async move { window.wait_for_progress(watched, 0, Duration::from_millis(200)).await }
    });
    tokio::task::yield_now().await;
    window.note(other, 8 * 1024);

    assert!(
      !timeout(Duration::from_secs(3), waiter)
        .await
        .expect("waiter ended")
        .unwrap(),
      "acks for another transfer must not count as progress"
    );
  }

  #[tokio::test]
  async fn a_waiter_that_already_has_progress_never_parks() {
    let window = AckWindow::new();
    let id = Uuid::now_v7();
    window.open(id, 32 * 1024);

    assert!(
      timeout(
        IMMEDIATE,
        window.wait_for_progress(id, 16 * 1024, Duration::from_secs(30))
      )
      .await
      .expect("must not park"),
    );
  }

  #[tokio::test]
  async fn a_replayed_ack_is_not_progress() {
    let window = Arc::new(AckWindow::new());
    let id = Uuid::now_v7();
    window.open(id, 16 * 1024);

    let waiter = tokio::spawn({
      let window = window.clone();
      async move {
        window
          .wait_for_progress(id, 16 * 1024, Duration::from_millis(200))
          .await
      }
    });
    tokio::task::yield_now().await;
    window.note(id, 8 * 1024);
    window.note(id, 16 * 1024);

    assert!(
      !timeout(Duration::from_secs(3), waiter)
        .await
        .expect("waiter ended")
        .unwrap(),
      "an ack at or below the prior total must not wake the sender"
    );
    assert_eq!(window.received_bytes(id), 16 * 1024, "a stale total never rewinds");
  }

  #[tokio::test]
  async fn a_sender_inside_the_window_is_never_held() {
    let window = AckWindow::new();
    let id = Uuid::now_v7();

    timeout(
      IMMEDIATE,
      window.await_window(id, 0, 64 * 1024, Duration::from_secs(30)),
    )
    .await
    .expect("must not park")
    .expect("offset 0 is inside any window");
  }

  #[tokio::test]
  async fn a_sender_past_the_window_waits_for_the_ack_that_opens_it() {
    let window = Arc::new(AckWindow::new());
    let id = Uuid::now_v7();
    window.open(id, 0);

    let sender = tokio::spawn({
      let window = window.clone();
      async move {
        window
          .await_window(id, 64 * 1024, 16 * 1024, Duration::from_secs(30))
          .await
      }
    });
    tokio::task::yield_now().await;
    assert!(!sender.is_finished(), "the sender must be parked outside its window");

    window.note(id, 64 * 1024);
    timeout(IMMEDIATE, sender)
      .await
      .expect("sender woke")
      .unwrap()
      .expect("the ack opened the window");
  }

  #[tokio::test]
  async fn await_window_fails_fast_once_the_transfer_finishes() {
    let window = Arc::new(AckWindow::new());
    let id = Uuid::now_v7();
    window.open(id, 0);

    let sender = tokio::spawn({
      let window = window.clone();
      async move {
        window
          .await_window(id, 64 * 1024, 16 * 1024, Duration::from_secs(30))
          .await
      }
    });
    tokio::task::yield_now().await;

    window.finish(id);
    let outcome = timeout(IMMEDIATE, sender).await.expect("sender woke").unwrap();
    assert_eq!(
      outcome,
      Err(TransferStalled {
        transfer_id: id,
        offset: 64 * 1024
      }),
      "a finished transfer must stall its sender rather than hang it"
    );
  }

  #[tokio::test]
  async fn a_silent_peer_stalls_the_sender_at_the_ack_timeout() {
    let window = AckWindow::new();
    let id = Uuid::now_v7();
    window.open(id, 0);

    let at = Instant::now();
    let outcome = timeout(
      Duration::from_secs(3),
      window.await_window(id, 64 * 1024, 16 * 1024, Duration::from_millis(150)),
    )
    .await
    .expect("the gate gave up on its own");
    assert_eq!(
      outcome,
      Err(TransferStalled {
        transfer_id: id,
        offset: 64 * 1024
      })
    );
    assert!(
      at.elapsed() >= Duration::from_millis(150),
      "the gate waits out the ack timeout before giving up"
    );
  }

  #[tokio::test]
  async fn an_ack_for_a_transfer_that_is_not_open_is_dropped() {
    let window = AckWindow::new();
    let never_opened = Uuid::now_v7();
    let finished = Uuid::now_v7();
    window.open(finished, 0);
    window.note(finished, 8 * 1024);
    window.finish(finished);

    window.note(never_opened, 4 * 1024);
    window.note(finished, 16 * 1024);

    assert_eq!(window.received_bytes(never_opened), 0);
    assert_eq!(window.received_bytes(finished), 0);
    assert!(
      window.entries.lock().unwrap().is_empty(),
      "a late ack must not leave bookkeeping behind for a transfer nobody is sending"
    );
  }

  #[tokio::test]
  async fn a_sender_past_the_window_of_a_finished_transfer_stalls_at_once() {
    let window = AckWindow::new();
    let id = Uuid::now_v7();
    window.open(id, 0);
    window.finish(id);

    let outcome = timeout(
      IMMEDIATE,
      window.await_window(id, 64 * 1024, 16 * 1024, Duration::from_secs(30)),
    )
    .await
    .expect("a finished transfer never parks its sender");
    assert_eq!(
      outcome,
      Err(TransferStalled {
        transfer_id: id,
        offset: 64 * 1024
      })
    );
    assert!(window.entries.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn a_stalled_gate_tears_the_transfer_down() {
    let window = AckWindow::new();
    let id = Uuid::now_v7();
    window.open(id, 8 * 1024);

    let _ = window
      .await_window(id, 64 * 1024, 16 * 1024, Duration::from_millis(150))
      .await;
    assert_eq!(
      window.received_bytes(id),
      0,
      "a stall finishes the transfer, so its bookkeeping is gone"
    );
  }

  #[tokio::test]
  async fn a_resume_baseline_is_seeded_when_the_transfer_opens() {
    let window = AckWindow::new();
    let id = Uuid::now_v7();
    window.open(id, 30 * 1024 * 1024);
    assert_eq!(window.received_bytes(id), 30 * 1024 * 1024);

    timeout(
      IMMEDIATE,
      window.await_window(id, 30 * 1024 * 1024, 64 * 1024, Duration::from_secs(30)),
    )
    .await
    .expect("must not park")
    .expect("the resume point is inside the seeded window");
  }

  #[tokio::test]
  async fn every_waiter_on_one_transfer_wakes_together() {
    let window = Arc::new(AckWindow::new());
    let id = Uuid::now_v7();
    window.open(id, 0);

    let waiters: Vec<_> = (0..3)
      .map(|_| {
        let window = window.clone();
        tokio::spawn(async move { window.wait_for_progress(id, 0, Duration::from_secs(30)).await })
      })
      .collect();
    tokio::task::yield_now().await;

    window.note(id, 4 * 1024);
    for waiter in waiters {
      assert!(timeout(IMMEDIATE, waiter).await.expect("waiter woke").unwrap());
    }
  }
}
