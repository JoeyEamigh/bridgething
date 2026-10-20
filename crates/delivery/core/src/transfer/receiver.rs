use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_sdk_runtime::rt::{self, Instant};
use bytes::Bytes;
use libbridgething::gateway::{TransferAbandon, TransferAck, TransferFragment, TransferRef};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::pacer::ACK_INTERVAL_BYTES;
use crate::{bundle::fetch::sha256_hex, seam::Clock};

pub const MAX_TRANSFER_BYTES: u64 = 1024 * 1024;
pub const RECEIPT_ACK_INTERVAL_BYTES: u32 = ACK_INTERVAL_BYTES as u32;
pub const PREREGISTRATION_BUDGET_BYTES: usize = 512 * 1024;
pub const PREREGISTRATION_TTL: Duration = Duration::from_secs(5);
pub const DEFAULT_COLLECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferReceiveError {
  #[error("transfer {transfer_id} was never registered")]
  NotRegistered { transfer_id: Uuid },
  #[error("transfer {transfer_id} timed out before completing")]
  TimedOut { transfer_id: Uuid },
  #[error("transfer {transfer_id} of {total_size} bytes exceeds the {MAX_TRANSFER_BYTES} byte cap")]
  TooLarge { transfer_id: Uuid, total_size: u32 },
  #[error("transfer {transfer_id} fragment ran past the declared total size")]
  Overflow { transfer_id: Uuid },
  #[error("transfer {transfer_id} non-contiguous fragment: expected offset {expected}, got {got}")]
  Gap { transfer_id: Uuid, expected: u32, got: u32 },
  #[error("transfer {transfer_id} sha256 mismatch: expected {expected}, got {got}")]
  ShaMismatch {
    transfer_id: Uuid,
    expected: String,
    got: String,
  },
  #[error("transfer {transfer_id} abandoned by the sender: {reason}")]
  Abandoned { transfer_id: Uuid, reason: String },
  #[error("transfer {transfer_id} dropped: the receiver stopped")]
  Stopped { transfer_id: Uuid },
}

pub trait AckSink: Send + Sync {
  fn ack(&self, ack: TransferAck);
}

type Collected = Result<Vec<u8>, TransferReceiveError>;

struct Pending {
  total_size: u32,
  sha256: Option<String>,
  buffer: Vec<u8>,
  last_acked: u32,
  waiter: Option<oneshot::Sender<Collected>>,
  terminal: Option<Collected>,
}

struct Buffered {
  fragments: Vec<(u32, Bytes)>,
  bytes: usize,
  expires_at: Instant,
}

#[derive(Default)]
struct State {
  pending: HashMap<Uuid, Pending>,
  buffer: HashMap<Uuid, Buffered>,
  buffered_bytes: usize,
}

pub struct TransferReceiver {
  acks: Arc<dyn AckSink>,
  clock: Arc<dyn Clock>,
  state: Mutex<State>,
}

impl TransferReceiver {
  pub fn new(acks: Arc<dyn AckSink>, clock: Arc<dyn Clock>) -> Arc<Self> {
    Arc::new(Self {
      acks,
      clock,
      state: Mutex::new(State::default()),
    })
  }

  pub fn stop(&self) {
    let mut state = self.state.lock().unwrap();
    for transfer_id in state.pending.keys().copied().collect::<Vec<_>>() {
      settle(
        &mut state,
        transfer_id,
        Err(TransferReceiveError::Stopped { transfer_id }),
      );
    }
    state.buffer.clear();
    state.buffered_bytes = 0;
  }

  pub fn register(&self, transfer: &TransferRef) {
    let acks = {
      let mut state = self.state.lock().unwrap();
      self.evict_expired(&mut state);
      if state.pending.contains_key(&transfer.id) {
        return;
      }

      let settled = match transfer.total_size {
        0 => Some(Ok(Vec::new())),
        total if u64::from(total) > MAX_TRANSFER_BYTES => Some(Err(TransferReceiveError::TooLarge {
          transfer_id: transfer.id,
          total_size: total,
        })),
        _ => None,
      };
      let done = settled.is_some();
      state.pending.insert(
        transfer.id,
        Pending {
          total_size: transfer.total_size,
          sha256: transfer.sha256.clone(),
          buffer: Vec::new(),
          last_acked: 0,
          waiter: None,
          terminal: settled,
        },
      );
      if done {
        return;
      }

      let Some(held) = state.buffer.remove(&transfer.id) else {
        return;
      };
      state.buffered_bytes -= held.bytes;
      let mut acks = Vec::new();
      for (offset, bytes) in held.fragments {
        if state.pending.get(&transfer.id).is_none_or(|p| p.terminal.is_some()) {
          break;
        }
        if let Some(received) = ingest(&mut state, transfer.id, offset, &bytes) {
          acks.push(received);
        }
      }
      acks
    };

    for received in acks {
      self.acks.ack(TransferAck {
        transfer_id: transfer.id,
        received,
      });
    }
  }

  pub async fn collect(&self, transfer_id: Uuid, timeout: Duration) -> Result<Vec<u8>, TransferReceiveError> {
    let collected = {
      let mut state = self.state.lock().unwrap();
      let Some(pending) = state.pending.get_mut(&transfer_id) else {
        return Err(TransferReceiveError::NotRegistered { transfer_id });
      };
      match pending.terminal.take() {
        Some(terminal) => {
          state.pending.remove(&transfer_id);
          return terminal;
        }
        None => {
          let (waiter, collected) = oneshot::channel();
          pending.waiter = Some(waiter);
          collected
        }
      }
    };

    match rt::timeout(timeout, collected).await {
      Ok(Ok(outcome)) => outcome,
      Ok(Err(_)) => Err(TransferReceiveError::Stopped { transfer_id }),
      Err(_) => {
        self.state.lock().unwrap().pending.remove(&transfer_id);
        Err(TransferReceiveError::TimedOut { transfer_id })
      }
    }
  }

  pub fn on_fragment(&self, fragment: TransferFragment) {
    let transfer_id = fragment.transfer_id;
    let received = {
      let mut state = self.state.lock().unwrap();
      self.evict_expired(&mut state);
      if state.pending.contains_key(&transfer_id) {
        ingest(&mut state, transfer_id, fragment.offset, &fragment.bytes)
      } else {
        self.hold_unregistered(&mut state, fragment);
        None
      }
    };

    if let Some(received) = received {
      self.acks.ack(TransferAck { transfer_id, received });
    }
  }

  pub fn on_abandon(&self, abandon: TransferAbandon) {
    let mut state = self.state.lock().unwrap();
    if let Some(held) = state.buffer.remove(&abandon.transfer_id) {
      state.buffered_bytes -= held.bytes;
    }
    if state.pending.contains_key(&abandon.transfer_id) {
      settle(
        &mut state,
        abandon.transfer_id,
        Err(TransferReceiveError::Abandoned {
          transfer_id: abandon.transfer_id,
          reason: abandon.reason,
        }),
      );
    }
  }

  fn hold_unregistered(&self, state: &mut State, fragment: TransferFragment) {
    let bytes = fragment.bytes.len();
    if state.buffered_bytes + bytes > PREREGISTRATION_BUDGET_BYTES {
      return;
    }
    state.buffered_bytes += bytes;
    let expires_at = self.clock.now() + PREREGISTRATION_TTL;
    let held = state.buffer.entry(fragment.transfer_id).or_insert_with(|| Buffered {
      fragments: Vec::new(),
      bytes: 0,
      expires_at,
    });
    held.bytes += bytes;
    held.expires_at = expires_at;
    held.fragments.push((fragment.offset, fragment.bytes));
  }

  fn evict_expired(&self, state: &mut State) {
    let now = self.clock.now();
    let mut freed = 0;
    state.buffer.retain(|_, held| {
      if held.expires_at > now {
        return true;
      }
      freed += held.bytes;
      false
    });
    state.buffered_bytes -= freed;
  }
}

fn settle(state: &mut State, transfer_id: Uuid, outcome: Collected) {
  let Some(pending) = state.pending.get_mut(&transfer_id) else {
    return;
  };
  if pending.terminal.is_some() {
    return;
  }
  let Some(waiter) = pending.waiter.take() else {
    pending.buffer = Vec::new();
    pending.terminal = Some(outcome);
    return;
  };
  state.pending.remove(&transfer_id);
  let _ = waiter.send(outcome);
}

fn ingest(state: &mut State, transfer_id: Uuid, offset: u32, bytes: &[u8]) -> Option<u32> {
  enum Step {
    Gap(u32),
    Overflow,
    Complete(Vec<u8>),
    Ack(u32),
    Quiet,
  }

  let step = {
    let pending = state.pending.get_mut(&transfer_id)?;
    if pending.terminal.is_some() {
      return None;
    }
    let have = pending.buffer.len() as u32;
    if offset != have {
      Step::Gap(have)
    } else if pending.buffer.len() as u64 + bytes.len() as u64 > u64::from(pending.total_size) {
      Step::Overflow
    } else {
      pending.buffer.extend_from_slice(bytes);
      let received = pending.buffer.len() as u32;
      if received == pending.total_size {
        Step::Complete(std::mem::take(&mut pending.buffer))
      } else if received - pending.last_acked >= RECEIPT_ACK_INTERVAL_BYTES {
        pending.last_acked = received;
        Step::Ack(received)
      } else {
        Step::Quiet
      }
    }
  };

  match step {
    Step::Gap(expected) => {
      settle(
        state,
        transfer_id,
        Err(TransferReceiveError::Gap {
          transfer_id,
          expected,
          got: offset,
        }),
      );
      None
    }
    Step::Overflow => {
      settle(state, transfer_id, Err(TransferReceiveError::Overflow { transfer_id }));
      None
    }
    Step::Complete(body) => {
      let received = body.len() as u32;
      let want = state.pending.get(&transfer_id).and_then(|p| p.sha256.clone());
      let outcome = match want {
        Some(want) => {
          let got = sha256_hex(&body);
          if got.eq_ignore_ascii_case(&want) {
            Ok(body)
          } else {
            Err(TransferReceiveError::ShaMismatch {
              transfer_id,
              expected: want,
              got,
            })
          }
        }
        None => Ok(body),
      };
      settle(state, transfer_id, outcome);
      Some(received)
    }
    Step::Ack(received) => Some(received),
    Step::Quiet => None,
  }
}

#[cfg(test)]
mod tests {
  use tokio::time::timeout;

  use super::{
    super::fixture::{RecordingAcks, TestClock, ramp},
    *,
  };

  const SHORT: Duration = Duration::from_millis(200);
  const PATIENT: Duration = Duration::from_secs(3);

  fn boot() -> (Arc<TransferReceiver>, Arc<RecordingAcks>, Arc<TestClock>) {
    let acks = RecordingAcks::new();
    let clock = TestClock::new();
    (TransferReceiver::new(acks.clone(), clock.clone()), acks, clock)
  }

  fn transfer_ref(id: Uuid, total: usize, sha256: Option<String>) -> TransferRef {
    TransferRef {
      id,
      total_size: total as u32,
      sha256,
    }
  }

  fn fragment(id: Uuid, offset: usize, bytes: &[u8]) -> TransferFragment {
    TransferFragment {
      transfer_id: id,
      offset: offset as u32,
      bytes: Bytes::copy_from_slice(bytes),
    }
  }

  fn stream_in(receiver: &TransferReceiver, id: Uuid, payload: &[u8], fragment_bytes: usize) {
    let mut offset = 0;
    while offset < payload.len() {
      let end = (offset + fragment_bytes).min(payload.len());
      receiver.on_fragment(fragment(id, offset, &payload[offset..end]));
      offset = end;
    }
  }

  #[tokio::test]
  async fn a_streamed_body_reassembles_and_acks_coalesce_on_the_interval() {
    let (receiver, acks, _clock) = boot();
    let payload = ramp(40 * 1024);
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, payload.len(), Some(sha256_hex(&payload))));

    stream_in(&receiver, id, &payload, 4 * 1024);
    let got = timeout(PATIENT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect("the transfer completed");

    assert_eq!(got, payload, "reassembled bytes must match the streamed payload");
    assert_eq!(
      acks.received(),
      vec![16384, 32768, 40960],
      "acks must land on 16 KiB boundaries and then on the final byte, never one per fragment"
    );
    assert!(
      acks.all().iter().all(|ack| ack.transfer_id == id),
      "every ack must name its own transfer"
    );
  }

  #[tokio::test]
  async fn a_collect_already_waiting_is_handed_the_bytes_as_they_complete() {
    let (receiver, _acks, _clock) = boot();
    let payload = ramp(8 * 1024);
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, payload.len(), None));

    let waiting = tokio::spawn({
      let receiver = receiver.clone();
      async move { receiver.collect(id, DEFAULT_COLLECT_TIMEOUT).await }
    });
    tokio::task::yield_now().await;

    stream_in(&receiver, id, &payload, 4 * 1024);
    let got = timeout(PATIENT, waiting)
      .await
      .expect("collect resolved")
      .unwrap()
      .expect("the transfer completed");
    assert_eq!(got, payload);
  }

  #[tokio::test]
  async fn fragments_that_beat_registration_are_replayed_from_the_buffer() {
    let (receiver, acks, _clock) = boot();
    let payload = ramp(16 * 1024);
    let id = Uuid::now_v7();

    stream_in(&receiver, id, &payload, 4 * 1024);
    assert!(
      acks.received().is_empty(),
      "a buffered fragment is not acked until someone registers the transfer"
    );

    receiver.register(&transfer_ref(id, payload.len(), Some(sha256_hex(&payload))));
    let got = timeout(PATIENT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect("a stream buffered before registration must still reassemble");

    assert_eq!(got, payload);
    assert_eq!(acks.received(), vec![16384], "the final byte is acked on replay");
  }

  #[tokio::test]
  async fn a_gap_fails_the_collect() {
    let (receiver, _acks, _clock) = boot();
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, 40 * 1024, None));

    receiver.on_fragment(fragment(id, 0, &[0u8; 4096]));
    receiver.on_fragment(fragment(id, 8192, &[0u8; 4096]));

    let err = timeout(PATIENT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect_err("a gap must fail the collect");
    assert_eq!(
      err,
      TransferReceiveError::Gap {
        transfer_id: id,
        expected: 4096,
        got: 8192
      }
    );
  }

  #[tokio::test]
  async fn an_overrun_past_the_declared_size_fails_the_collect() {
    let (receiver, _acks, _clock) = boot();
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, 4096, None));

    receiver.on_fragment(fragment(id, 0, &[0u8; 8192]));

    let err = timeout(PATIENT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect_err("a fragment past the declared size must fail the collect");
    assert_eq!(err, TransferReceiveError::Overflow { transfer_id: id });
  }

  #[tokio::test]
  async fn a_sha256_mismatch_fails_the_collect() {
    let (receiver, _acks, _clock) = boot();
    let payload = ramp(8 * 1024);
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, payload.len(), Some("0".repeat(64))));

    stream_in(&receiver, id, &payload, 4 * 1024);

    let err = timeout(PATIENT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect_err("a digest mismatch must fail the collect");
    assert!(
      matches!(err, TransferReceiveError::ShaMismatch { transfer_id, ref expected, .. }
        if transfer_id == id && expected == &"0".repeat(64)),
      "expected a sha mismatch, got {err}"
    );
  }

  #[tokio::test]
  async fn a_declared_digest_is_verified_before_the_bytes_are_handed_over() {
    let (receiver, _acks, _clock) = boot();
    let payload = ramp(8 * 1024);
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(
      id,
      payload.len(),
      Some(sha256_hex(&payload).to_uppercase()),
    ));

    stream_in(&receiver, id, &payload, 4 * 1024);
    let got = timeout(PATIENT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect("a digest is compared case-insensitively");
    assert_eq!(got, payload);
  }

  #[tokio::test]
  async fn an_abandon_fails_an_inflight_collect_promptly() {
    let (receiver, _acks, _clock) = boot();
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, 40 * 1024, None));

    let waiting = tokio::spawn({
      let receiver = receiver.clone();
      async move { receiver.collect(id, DEFAULT_COLLECT_TIMEOUT).await }
    });
    tokio::task::yield_now().await;

    receiver.on_fragment(fragment(id, 0, &[0u8; 4096]));
    receiver.on_abandon(TransferAbandon {
      transfer_id: id,
      reason: "upstream gone".into(),
    });

    let err = timeout(SHORT, waiting)
      .await
      .expect("abandon must fail the collect without waiting out its timeout")
      .unwrap()
      .expect_err("abandon must fail the collect");
    assert_eq!(
      err,
      TransferReceiveError::Abandoned {
        transfer_id: id,
        reason: "upstream gone".into()
      }
    );
  }

  #[tokio::test]
  async fn a_zero_byte_transfer_completes_without_any_fragment() {
    let (receiver, acks, _clock) = boot();
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, 0, None));

    let got = timeout(SHORT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("an empty transfer must resolve immediately, not hang")
      .expect("an empty transfer is a successful transfer");
    assert!(got.is_empty());
    assert!(acks.received().is_empty(), "there is nothing to ack receipt of");
  }

  #[tokio::test]
  async fn an_oversize_transfer_fails_its_collect() {
    let (receiver, _acks, _clock) = boot();
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, 2 * 1024 * 1024, None));

    let err = timeout(SHORT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("an oversize transfer is refused at registration, so its collect never waits")
      .expect_err("an oversize transfer must fail");
    assert_eq!(
      err,
      TransferReceiveError::TooLarge {
        transfer_id: id,
        total_size: 2 * 1024 * 1024
      }
    );
  }

  #[tokio::test]
  async fn a_transfer_exactly_at_the_cap_is_accepted() {
    let (receiver, _acks, _clock) = boot();
    let payload = ramp(MAX_TRANSFER_BYTES as usize);
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, payload.len(), None));

    stream_in(&receiver, id, &payload, 64 * 1024);
    let got = timeout(PATIENT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect("the cap is inclusive");
    assert_eq!(got.len(), MAX_TRANSFER_BYTES as usize);
  }

  #[tokio::test]
  async fn collect_of_an_unregistered_transfer_fails_immediately() {
    let (receiver, _acks, _clock) = boot();
    let id = Uuid::now_v7();

    let err = timeout(SHORT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("an unknown transfer must not park the caller")
      .expect_err("collecting an unknown transfer must fail");
    assert_eq!(err, TransferReceiveError::NotRegistered { transfer_id: id });
  }

  #[tokio::test]
  async fn collect_gives_up_when_the_sender_goes_silent() {
    let (receiver, _acks, _clock) = boot();
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, 32 * 1024, None));
    receiver.on_fragment(fragment(id, 0, &[0u8; 4096]));

    let err = timeout(PATIENT, receiver.collect(id, SHORT))
      .await
      .expect("the collect timed itself out")
      .expect_err("a silent sender must time the collect out");
    assert_eq!(err, TransferReceiveError::TimedOut { transfer_id: id });
  }

  #[tokio::test]
  async fn a_collected_transfer_is_forgotten() {
    let (receiver, _acks, _clock) = boot();
    let payload = ramp(4 * 1024);
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, payload.len(), None));
    stream_in(&receiver, id, &payload, 4 * 1024);

    timeout(PATIENT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect("the transfer completed");

    let err = timeout(SHORT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("a second collect must not park")
      .expect_err("a collected transfer is gone");
    assert_eq!(err, TransferReceiveError::NotRegistered { transfer_id: id });
  }

  #[tokio::test]
  async fn stop_fails_every_parked_collect() {
    let (receiver, _acks, _clock) = boot();
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    receiver.register(&transfer_ref(first, 32 * 1024, None));
    receiver.register(&transfer_ref(second, 32 * 1024, None));

    let waiting: Vec<_> = [first, second]
      .into_iter()
      .map(|id| {
        let receiver = receiver.clone();
        tokio::spawn(async move { receiver.collect(id, DEFAULT_COLLECT_TIMEOUT).await })
      })
      .collect();
    tokio::task::yield_now().await;

    receiver.stop();
    for (id, task) in [first, second].into_iter().zip(waiting) {
      let err = timeout(SHORT, task)
        .await
        .expect("stop must not leave a caller waiting out its timeout")
        .unwrap()
        .expect_err("a stopped receiver fails its collects");
      assert_eq!(err, TransferReceiveError::Stopped { transfer_id: id });
    }
  }

  #[tokio::test]
  async fn stop_drops_what_the_preregistration_buffer_was_holding() {
    let (receiver, _acks, _clock) = boot();
    let payload = ramp(16 * 1024);
    let id = Uuid::now_v7();
    stream_in(&receiver, id, &payload, 4 * 1024);

    receiver.stop();
    receiver.register(&transfer_ref(id, payload.len(), None));

    let err = timeout(PATIENT, receiver.collect(id, SHORT))
      .await
      .expect("collect resolved")
      .expect_err("nothing survives a stop");
    assert_eq!(err, TransferReceiveError::TimedOut { transfer_id: id });
  }

  #[tokio::test]
  async fn the_preregistration_buffer_is_bounded_in_bytes() {
    let (receiver, _acks, _clock) = boot();
    let held = Uuid::now_v7();
    let starved = Uuid::now_v7();

    let payload = ramp(PREREGISTRATION_BUDGET_BYTES + 128 * 1024);
    stream_in(&receiver, held, &payload, 16 * 1024);
    receiver.on_fragment(fragment(starved, 0, &[0u8; 4096]));

    receiver.register(&transfer_ref(held, payload.len(), None));
    let err = timeout(PATIENT, receiver.collect(held, SHORT))
      .await
      .expect("collect resolved")
      .expect_err("the dropped tail can never complete the transfer");
    assert_eq!(err, TransferReceiveError::TimedOut { transfer_id: held });

    receiver.register(&transfer_ref(starved, 4096, None));
    let err = timeout(PATIENT, receiver.collect(starved, SHORT))
      .await
      .expect("collect resolved")
      .expect_err("a fragment arriving against a full budget is dropped too");
    assert_eq!(err, TransferReceiveError::TimedOut { transfer_id: starved });
  }

  #[tokio::test]
  async fn buffered_fragments_expire_after_the_ttl() {
    let (receiver, _acks, clock) = boot();
    let stale = Uuid::now_v7();
    let fresh = Uuid::now_v7();
    let payload = ramp(4 * 1024);

    receiver.on_fragment(fragment(stale, 0, &payload));
    clock.advance(PREREGISTRATION_TTL + Duration::from_millis(1));
    receiver.on_fragment(fragment(fresh, 0, &payload));

    receiver.register(&transfer_ref(stale, payload.len(), None));
    let err = timeout(PATIENT, receiver.collect(stale, SHORT))
      .await
      .expect("collect resolved")
      .expect_err("a registration that never came must not pin bytes forever");
    assert_eq!(err, TransferReceiveError::TimedOut { transfer_id: stale });

    receiver.register(&transfer_ref(fresh, payload.len(), None));
    let got = timeout(PATIENT, receiver.collect(fresh, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect("a fragment inside the ttl is still there");
    assert_eq!(got, payload);
  }

  #[tokio::test]
  async fn a_fragment_arriving_after_a_failure_is_dropped_rather_than_rebuffered() {
    let (receiver, _acks, _clock) = boot();
    let id = Uuid::now_v7();
    receiver.register(&transfer_ref(id, 40 * 1024, None));

    receiver.on_fragment(fragment(id, 0, &[0u8; 4096]));
    receiver.on_fragment(fragment(id, 8192, &[0u8; 4096]));
    receiver.on_fragment(fragment(id, 4096, &[0u8; 4096]));

    let err = timeout(PATIENT, receiver.collect(id, DEFAULT_COLLECT_TIMEOUT))
      .await
      .expect("collect resolved")
      .expect_err("the failure stands");
    assert!(
      matches!(err, TransferReceiveError::Gap { .. }),
      "a late fragment must not resurrect a failed transfer, got {err}"
    );
  }
}
