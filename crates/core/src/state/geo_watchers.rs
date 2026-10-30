use std::{
  collections::HashMap,
  net::SocketAddr,
  sync::{Arc, RwLock},
  time::{Duration, Instant},
};

use libbridgething::{GeoAccuracy, Position};
use tokio::sync::watch;
use uuid::Uuid;

use crate::bluetooth::Address;

#[derive(Debug, Clone, Default)]
pub struct GeoLastFix {
  inner: Arc<RwLock<Option<(Position, Instant)>>>,
}

impl GeoLastFix {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn record(&self, position: Position) {
    *self.inner.write().expect("geo last fix poisoned") = Some((position, Instant::now()));
  }

  pub fn fresher_than(&self, max_age: Duration) -> Option<Position> {
    let (position, at) = (*self.inner.read().expect("geo last fix poisoned"))?;
    (at.elapsed() <= max_age).then_some(position)
  }

  #[cfg(test)]
  fn record_aged(&self, position: Position, age: Duration) {
    let at = Instant::now()
      .checked_sub(age)
      .expect("test age precedes process start");
    *self.inner.write().expect("geo last fix poisoned") = Some((position, at));
  }
}

#[derive(Debug, Clone, Copy)]
struct Watcher {
  owner: SocketAddr,
  accuracy: GeoAccuracy,
  min_interval_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchAggregate {
  pub accuracy: GeoAccuracy,
  pub min_interval_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeoLease {
  pub gateway: Address,
  pub watch: WatchAggregate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeoHandover {
  pub unwatch: Option<Address>,
  pub watch: Option<GeoLease>,
}

#[derive(Debug, Clone)]
pub struct GeoWatchers {
  inner: Arc<RwLock<HashMap<Uuid, Watcher>>>,
  changed: Arc<watch::Sender<()>>,
}

impl Default for GeoWatchers {
  fn default() -> Self {
    Self {
      inner: Arc::default(),
      changed: Arc::new(watch::channel(()).0),
    }
  }
}

impl GeoWatchers {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn subscribe(&self) -> watch::Receiver<()> {
    self.changed.subscribe()
  }

  pub fn register(&self, token: Uuid, owner: SocketAddr, accuracy: GeoAccuracy, min_interval_ms: u32) {
    self.inner.write().expect("geo watchers poisoned").insert(
      token,
      Watcher {
        owner,
        accuracy,
        min_interval_ms,
      },
    );
    self.changed.send_replace(());
  }

  pub fn unregister(&self, token: Uuid) -> bool {
    let existed = self
      .inner
      .write()
      .expect("geo watchers poisoned")
      .remove(&token)
      .is_some();
    if existed {
      self.changed.send_replace(());
    }
    existed
  }

  pub fn drain_for_owner(&self, owner: SocketAddr) {
    let drained = {
      let mut guard = self.inner.write().expect("geo watchers poisoned");
      let before = guard.len();
      guard.retain(|_, w| w.owner != owner);
      guard.len() != before
    };
    if drained {
      self.changed.send_replace(());
    }
  }

  pub fn owners(&self) -> Vec<SocketAddr> {
    let guard = self.inner.read().expect("geo watchers poisoned");
    let mut owners: Vec<SocketAddr> = guard.values().map(|w| w.owner).collect();
    owners.sort();
    owners.dedup();
    owners
  }

  pub fn aggregate(&self) -> Option<WatchAggregate> {
    aggregate_of(&self.inner.read().expect("geo watchers poisoned"))
  }

  pub fn lease_for(&self, gateway: Option<Address>) -> Option<GeoLease> {
    gateway
      .zip(self.aggregate())
      .map(|(gateway, watch)| GeoLease { gateway, watch })
  }
}

impl GeoHandover {
  pub fn between(held: Option<GeoLease>, want: Option<GeoLease>) -> Option<Self> {
    (held != want).then(|| Self {
      unwatch: held
        .map(|lease| lease.gateway)
        .filter(|old| want.map(|lease| lease.gateway) != Some(*old)),
      watch: want,
    })
  }
}

fn aggregate_of(map: &HashMap<Uuid, Watcher>) -> Option<WatchAggregate> {
  let mut iter = map.values();
  let first = iter.next()?;
  let mut accuracy = first.accuracy;
  let mut min_interval_ms = first.min_interval_ms;
  for w in iter {
    if matches!(w.accuracy, GeoAccuracy::Fine) {
      accuracy = GeoAccuracy::Fine;
    }
    if w.min_interval_ms < min_interval_ms {
      min_interval_ms = w.min_interval_ms;
    }
  }
  Some(WatchAggregate {
    accuracy,
    min_interval_ms,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
  }

  fn gw(last: u8) -> Address {
    Address([0xAA, 0, 0, 0, 0, last])
  }

  fn aggregate(accuracy: GeoAccuracy, min_interval_ms: u32) -> Option<WatchAggregate> {
    Some(WatchAggregate {
      accuracy,
      min_interval_ms,
    })
  }

  #[test]
  fn first_register_sets_the_aggregate() {
    let w = GeoWatchers::new();
    assert_eq!(w.aggregate(), None);
    w.register(Uuid::now_v7(), addr("127.0.0.1:1"), GeoAccuracy::Coarse, 1000);
    assert_eq!(w.aggregate(), aggregate(GeoAccuracy::Coarse, 1000));
  }

  #[test]
  fn fine_upgrades_aggregate() {
    let w = GeoWatchers::new();
    w.register(Uuid::now_v7(), addr("127.0.0.1:1"), GeoAccuracy::Coarse, 5000);
    w.register(Uuid::now_v7(), addr("127.0.0.1:2"), GeoAccuracy::Fine, 10000);
    assert_eq!(w.aggregate(), aggregate(GeoAccuracy::Fine, 5000));
  }

  #[test]
  fn smaller_interval_wins() {
    let w = GeoWatchers::new();
    w.register(Uuid::now_v7(), addr("127.0.0.1:1"), GeoAccuracy::Coarse, 5000);
    w.register(Uuid::now_v7(), addr("127.0.0.1:2"), GeoAccuracy::Coarse, 1000);
    assert_eq!(w.aggregate().unwrap().min_interval_ms, 1000);
  }

  #[test]
  fn unregister_last_clears_the_aggregate() {
    let w = GeoWatchers::new();
    let token = Uuid::now_v7();
    w.register(token, addr("127.0.0.1:1"), GeoAccuracy::Fine, 1000);
    assert!(w.unregister(token));
    assert_eq!(w.aggregate(), None);
  }

  #[test]
  fn unregister_unknown_is_noop() {
    let w = GeoWatchers::new();
    w.register(Uuid::now_v7(), addr("127.0.0.1:1"), GeoAccuracy::Fine, 1000);
    assert!(!w.unregister(Uuid::now_v7()));
    assert_eq!(w.aggregate(), aggregate(GeoAccuracy::Fine, 1000));
  }

  #[test]
  fn drain_for_owner_clears_only_that_owner() {
    let w = GeoWatchers::new();
    let alice = addr("127.0.0.1:1");
    let bob = addr("127.0.0.1:2");
    w.register(Uuid::now_v7(), alice, GeoAccuracy::Fine, 1000);
    w.register(Uuid::now_v7(), alice, GeoAccuracy::Fine, 5000);
    w.register(Uuid::now_v7(), bob, GeoAccuracy::Coarse, 3000);

    w.drain_for_owner(alice);
    assert_eq!(w.aggregate(), aggregate(GeoAccuracy::Coarse, 3000));
    assert_eq!(w.owners(), vec![bob]);
  }

  #[test]
  fn a_change_to_the_watchers_wakes_the_holder() {
    let w = GeoWatchers::new();
    let mut changed = w.subscribe();
    let token = Uuid::now_v7();
    w.register(token, addr("127.0.0.1:1"), GeoAccuracy::Fine, 1000);
    assert!(changed.has_changed().unwrap());
    changed.borrow_and_update();

    assert!(!w.unregister(Uuid::now_v7()));
    assert!(!changed.has_changed().unwrap(), "an unknown token changed nothing");
    assert!(w.unregister(token));
    assert!(changed.has_changed().unwrap());
  }

  #[test]
  fn the_watch_runs_on_exactly_one_companion() {
    let w = GeoWatchers::new();
    assert_eq!(w.lease_for(Some(gw(1))), None, "no watchers, nothing to start");

    w.register(Uuid::now_v7(), addr("127.0.0.1:1"), GeoAccuracy::Coarse, 1000);
    let lease = w.lease_for(Some(gw(1)));
    assert_eq!(
      GeoHandover::between(None, lease),
      Some(GeoHandover {
        unwatch: None,
        watch: lease,
      })
    );
    assert_eq!(
      GeoHandover::between(lease, lease),
      None,
      "an unchanged lease is not re-sent"
    );
  }

  #[test]
  fn a_new_primary_takes_the_watch_over() {
    let w = GeoWatchers::new();
    w.register(Uuid::now_v7(), addr("127.0.0.1:1"), GeoAccuracy::Fine, 1000);

    let moved = GeoHandover::between(w.lease_for(Some(gw(1))), w.lease_for(Some(gw(2)))).expect("the primary moved");
    assert_eq!(moved.unwatch, Some(gw(1)));
    assert_eq!(moved.watch.map(|lease| lease.gateway), Some(gw(2)));
  }

  #[test]
  fn a_retuned_watch_stays_on_its_companion() {
    let w = GeoWatchers::new();
    w.register(Uuid::now_v7(), addr("127.0.0.1:1"), GeoAccuracy::Coarse, 5000);
    let held = w.lease_for(Some(gw(1)));
    w.register(Uuid::now_v7(), addr("127.0.0.1:2"), GeoAccuracy::Fine, 1000);

    let retuned = GeoHandover::between(held, w.lease_for(Some(gw(1)))).expect("the aggregate changed");
    assert_eq!(retuned.unwatch, None);
    assert_eq!(
      retuned.watch.map(|lease| lease.watch),
      aggregate(GeoAccuracy::Fine, 1000)
    );
  }

  #[test]
  fn the_last_watcher_leaving_stops_the_holder() {
    let w = GeoWatchers::new();
    let token = Uuid::now_v7();
    w.register(token, addr("127.0.0.1:1"), GeoAccuracy::Fine, 1000);
    let held = w.lease_for(Some(gw(1)));
    w.unregister(token);

    assert_eq!(
      GeoHandover::between(held, w.lease_for(Some(gw(1)))),
      Some(GeoHandover {
        unwatch: Some(gw(1)),
        watch: None,
      })
    );
  }

  #[test]
  fn losing_every_companion_drops_the_lease() {
    let w = GeoWatchers::new();
    w.register(Uuid::now_v7(), addr("127.0.0.1:1"), GeoAccuracy::Fine, 1000);
    let held = w.lease_for(Some(gw(1)));

    assert_eq!(
      GeoHandover::between(held, w.lease_for(None)),
      Some(GeoHandover {
        unwatch: Some(gw(1)),
        watch: None,
      })
    );
    assert!(
      GeoHandover::between(None, w.lease_for(Some(gw(2)))).is_some_and(|arrived| arrived.watch.is_some()),
      "a companion arriving later never picked the watch up"
    );
  }

  fn fix() -> Position {
    Position {
      lat: 52.5,
      lon: 6.1,
      alt_m: Some(4.0),
      accuracy_m: 6.7,
      speed_mps: None,
      heading_deg: None,
      ts_unix_s: 1_785_349_738,
    }
  }

  #[test]
  fn last_fix_is_empty_before_anything_arrives() {
    let held = GeoLastFix::new();
    assert_eq!(held.fresher_than(Duration::from_secs(300)), None);
  }

  #[test]
  fn last_fix_serves_inside_tolerance() {
    let held = GeoLastFix::new();
    held.record(fix());
    assert_eq!(held.fresher_than(Duration::from_secs(300)), Some(fix()));
  }

  #[test]
  fn last_fix_refuses_outside_tolerance() {
    let held = GeoLastFix::new();
    held.record_aged(fix(), Duration::from_secs(600));
    assert_eq!(held.fresher_than(Duration::from_secs(300)), None);
  }

  #[test]
  fn zero_tolerance_always_forces_a_fresh_fix() {
    let held = GeoLastFix::new();
    held.record(fix());
    assert_eq!(held.fresher_than(Duration::ZERO), None);
  }

  #[test]
  fn a_newer_fix_replaces_a_stale_one() {
    let held = GeoLastFix::new();
    held.record_aged(fix(), Duration::from_secs(600));
    let mut fresh = fix();
    fresh.lat = 48.0;
    held.record(fresh);
    assert_eq!(held.fresher_than(Duration::from_secs(300)), Some(fresh));
  }

  #[test]
  fn owners_dedups_across_tokens() {
    let w = GeoWatchers::new();
    let a = addr("127.0.0.1:1");
    w.register(Uuid::now_v7(), a, GeoAccuracy::Fine, 1000);
    w.register(Uuid::now_v7(), a, GeoAccuracy::Coarse, 5000);
    assert_eq!(w.owners(), vec![a]);
  }
}
