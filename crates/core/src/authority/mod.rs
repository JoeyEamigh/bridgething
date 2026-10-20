use std::{
  collections::HashMap,
  sync::{Arc, RwLock},
  time::{Duration, Instant},
};

use bridgething_iap2::NowPlayingAuthorityState;
use libbridgething::CompanionAuthorityScope;
use tokio::sync::watch;

use crate::bluetooth::Address;

pub const STALE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default)]
pub struct AuthorityRegistry {
  inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
  companions: RwLock<HashMap<Address, CompanionAuthority>>,
  now_playing: watch::Sender<NowPlayingAuthorityState>,
  primary: watch::Sender<Option<Address>>,
}

impl Default for Inner {
  fn default() -> Self {
    let (now_playing, _) = watch::channel(NowPlayingAuthorityState::default());
    let (primary, _) = watch::channel(None);
    Inner {
      companions: RwLock::new(HashMap::new()),
      now_playing,
      primary,
    }
  }
}

#[derive(Debug, Default, Clone)]
struct CompanionAuthority {
  scopes: HashMap<CompanionAuthorityScope, Instant>,
  app_bundle: Option<String>,
}

impl CompanionAuthority {
  fn holds(&self, scope: CompanionAuthorityScope) -> bool {
    self.scopes.get(&scope).is_some_and(|at| is_live(scope, *at))
  }

  fn live_scopes(&self) -> Vec<CompanionAuthorityScope> {
    self
      .scopes
      .iter()
      .filter(|(scope, at)| is_live(**scope, **at))
      .map(|(scope, _)| *scope)
      .collect()
  }

  fn latest_live_claim(&self) -> Option<Instant> {
    self
      .scopes
      .iter()
      .filter(|(scope, at)| is_live(**scope, **at))
      .map(|(_, at)| *at)
      .max()
  }

  fn holds_now_playing(&self) -> bool {
    self.holds(CompanionAuthorityScope::NowPlayingMetadata) || self.holds(CompanionAuthorityScope::NowPlayingPlayback)
  }
}

fn scope_holds_indefinitely(scope: CompanionAuthorityScope) -> bool {
  matches!(
    scope,
    CompanionAuthorityScope::NowPlayingMetadata
      | CompanionAuthorityScope::NowPlayingPlayback
      | CompanionAuthorityScope::Volume
  )
}

fn is_live(scope: CompanionAuthorityScope, claimed_at: Instant) -> bool {
  scope_holds_indefinitely(scope) || claimed_at.elapsed() < STALE_TIMEOUT
}

fn elect(companions: &HashMap<Address, CompanionAuthority>) -> Option<Address> {
  companions
    .iter()
    .filter_map(|(addr, companion)| companion.latest_live_claim().map(|at| (*addr, companion, at)))
    .max_by(|(a_addr, a, a_at), (b_addr, b, b_at)| {
      a.holds_now_playing()
        .cmp(&b.holds_now_playing())
        .then(a_at.cmp(b_at))
        .then(b_addr.cmp(a_addr))
    })
    .map(|(addr, _, _)| addr)
}

impl AuthorityRegistry {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn claim(&self, addr: Address, scope: CompanionAuthorityScope) {
    let mut guard = self.inner.companions.write().expect("authority lock poisoned");
    guard.entry(addr).or_default().scopes.insert(scope, Instant::now());
    self.publish(&guard);
  }

  pub fn release(&self, addr: Address, scope: CompanionAuthorityScope) {
    let mut guard = self.inner.companions.write().expect("authority lock poisoned");
    if let Some(companion) = guard.get_mut(&addr) {
      companion.scopes.remove(&scope);
    }
    self.publish(&guard);
  }

  pub fn drop_for(&self, addr: Address) {
    let mut guard = self.inner.companions.write().expect("authority lock poisoned");
    guard.remove(&addr);
    self.publish(&guard);
  }

  pub fn primary(&self) -> Option<Address> {
    let guard = self.inner.companions.read().expect("authority lock poisoned");
    elect(&guard)
  }

  pub fn now_playing_subscription_rx(&self) -> watch::Receiver<NowPlayingAuthorityState> {
    self.inner.now_playing.subscribe()
  }

  pub fn primary_subscription_rx(&self) -> watch::Receiver<Option<Address>> {
    self.inner.primary.subscribe()
  }

  fn publish(&self, companions: &HashMap<Address, CompanionAuthority>) {
    let elected = elect(companions);
    let primary = elected.and_then(|addr| companions.get(&addr));
    let state = NowPlayingAuthorityState {
      companion_metadata: primary.is_some_and(|c| c.holds(CompanionAuthorityScope::NowPlayingMetadata)),
      companion_playback: primary.is_some_and(|c| c.holds(CompanionAuthorityScope::NowPlayingPlayback)),
      app_bundle: primary.and_then(|c| c.app_bundle.clone()),
    };
    self.inner.now_playing.send_if_modified(|cur| {
      let changed = *cur != state;
      *cur = state;
      changed
    });
    self.inner.primary.send_if_modified(|cur| {
      let changed = *cur != elected;
      *cur = elected;
      changed
    });
  }

  pub fn set_companion_app_bundle(&self, addr: Address, bundle: Option<String>) {
    if let Some(bundle) = bundle {
      let mut guard = self.inner.companions.write().expect("authority lock poisoned");
      guard.entry(addr).or_default().app_bundle = Some(bundle);
      self.publish(&guard);
    }
  }

  pub fn companion_app_bundle(&self) -> Option<String> {
    let guard = self.inner.companions.read().expect("authority lock poisoned");
    elect(&guard).and_then(|addr| guard.get(&addr)?.app_bundle.clone())
  }

  pub fn is_authoritative(&self, scope: CompanionAuthorityScope) -> bool {
    let guard = self.inner.companions.read().expect("authority lock poisoned");
    elect(&guard).is_some_and(|addr| guard.get(&addr).is_some_and(|c| c.holds(scope)))
  }

  pub fn live_scopes(&self) -> Vec<CompanionAuthorityScope> {
    let guard = self.inner.companions.read().expect("authority lock poisoned");
    elect(&guard)
      .and_then(|addr| guard.get(&addr))
      .map(CompanionAuthority::live_scopes)
      .unwrap_or_default()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn addr(last: u8) -> Address {
    Address([0xAA, 0, 0, 0, 0, last])
  }

  #[test]
  fn claim_and_release_round_trip() {
    let r = AuthorityRegistry::new();
    assert!(!r.is_authoritative(CompanionAuthorityScope::NowPlayingMetadata));
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    assert!(r.is_authoritative(CompanionAuthorityScope::NowPlayingMetadata));
    r.release(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    assert!(!r.is_authoritative(CompanionAuthorityScope::NowPlayingMetadata));
  }

  #[test]
  fn drop_for_clears_every_scope_of_that_companion() {
    let r = AuthorityRegistry::new();
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingPlayback);
    r.drop_for(addr(1));
    assert!(!r.is_authoritative(CompanionAuthorityScope::NowPlayingMetadata));
    assert!(!r.is_authoritative(CompanionAuthorityScope::NowPlayingPlayback));
  }

  #[test]
  fn dropping_a_non_primary_leaves_the_primary_holding() {
    let r = AuthorityRegistry::new();
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    r.claim(addr(2), CompanionAuthorityScope::Volume);
    assert_eq!(
      r.primary(),
      Some(addr(1)),
      "now-playing claimant should outrank volume-only"
    );

    r.drop_for(addr(2));
    assert_eq!(r.primary(), Some(addr(1)));
    assert!(r.is_authoritative(CompanionAuthorityScope::NowPlayingMetadata));
  }

  #[test]
  fn a_non_primary_claim_does_not_answer_for_the_primary() {
    let r = AuthorityRegistry::new();
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    r.claim(addr(2), CompanionAuthorityScope::Volume);
    assert!(
      !r.is_authoritative(CompanionAuthorityScope::Volume),
      "a non-primary's volume claim leaked into the primary's answer"
    );
  }

  #[test]
  fn losing_the_primary_promotes_the_remaining_claimant() {
    let r = AuthorityRegistry::new();
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    r.claim(addr(2), CompanionAuthorityScope::NowPlayingMetadata);
    assert_eq!(r.primary(), Some(addr(2)), "last claim should win between equals");

    r.drop_for(addr(2));
    assert_eq!(r.primary(), Some(addr(1)));
    assert!(r.is_authoritative(CompanionAuthorityScope::NowPlayingMetadata));
  }

  #[test]
  fn scopes_are_independent() {
    let r = AuthorityRegistry::new();
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    assert!(r.is_authoritative(CompanionAuthorityScope::NowPlayingMetadata));
    assert!(!r.is_authoritative(CompanionAuthorityScope::NowPlayingPlayback));
  }

  #[test]
  fn app_bundle_follows_the_primary() {
    let r = AuthorityRegistry::new();
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    r.set_companion_app_bundle(addr(1), Some("com.spotify.client".into()));
    r.set_companion_app_bundle(addr(2), Some("com.example.dev".into()));
    assert_eq!(r.companion_app_bundle().as_deref(), Some("com.spotify.client"));
  }

  #[test]
  fn now_playing_subscription_mirrors_primary_scope_claims() {
    let r = AuthorityRegistry::new();
    let rx = r.now_playing_subscription_rx();
    assert_eq!(*rx.borrow(), NowPlayingAuthorityState::default());

    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    assert_eq!(
      *rx.borrow(),
      NowPlayingAuthorityState {
        companion_metadata: true,
        companion_playback: false,
        app_bundle: None,
      }
    );

    r.claim(addr(1), CompanionAuthorityScope::NowPlayingPlayback);
    assert!(rx.borrow().companion_playback);

    r.claim(addr(1), CompanionAuthorityScope::Volume);
    assert!(rx.borrow().companion_metadata && rx.borrow().companion_playback);

    r.drop_for(addr(1));
    assert_eq!(*rx.borrow(), NowPlayingAuthorityState::default());
  }

  #[test]
  fn a_secondary_companion_leaving_does_not_disturb_the_subscription() {
    let r = AuthorityRegistry::new();
    let rx = r.now_playing_subscription_rx();
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingPlayback);
    r.drop_for(addr(2));
    assert_eq!(
      *rx.borrow(),
      NowPlayingAuthorityState {
        companion_metadata: true,
        companion_playback: true,
        app_bundle: None,
      }
    );
  }

  #[test]
  fn now_playing_subscription_carries_the_bundle_the_primary_claims() {
    let r = AuthorityRegistry::new();
    let rx = r.now_playing_subscription_rx();
    r.set_companion_app_bundle(addr(1), Some("com.apple.Music".into()));
    r.claim(addr(1), CompanionAuthorityScope::NowPlayingMetadata);
    assert_eq!(
      rx.borrow().app_bundle.as_deref(),
      Some("com.apple.Music"),
      "the claim publishes with its bundle, never a bundle-less interim state"
    );

    r.set_companion_app_bundle(addr(1), Some("com.spotify.client".into()));
    assert_eq!(
      rx.borrow().app_bundle.as_deref(),
      Some("com.spotify.client"),
      "a handover to another provider re-publishes"
    );

    r.drop_for(addr(1));
    assert_eq!(rx.borrow().app_bundle, None, "no companion, no claimed bundle");
  }

  #[test]
  fn volume_holds_past_stale_timeout() {
    assert!(scope_holds_indefinitely(CompanionAuthorityScope::Volume));
    let r = AuthorityRegistry::new();
    r.inner
      .companions
      .write()
      .unwrap()
      .entry(addr(1))
      .or_default()
      .scopes
      .insert(CompanionAuthorityScope::Volume, Instant::now() - STALE_TIMEOUT * 4);
    assert!(r.is_authoritative(CompanionAuthorityScope::Volume));
  }
}
