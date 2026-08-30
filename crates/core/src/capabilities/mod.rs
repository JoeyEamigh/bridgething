use std::{
  collections::{HashMap, HashSet},
  net::SocketAddr,
  sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
  },
};

use libbridgething::{
  Capabilities, CompanionAuthorityScope, GatewayCapabilities, SurfaceAvailability,
  client::{BridgeToClientCapabilitiesMsgEvent, CapabilitiesSnapshot},
};
use uuid::Uuid;

use crate::{
  authority::AuthorityRegistry,
  bluetooth::Address,
  net::{WSResult, WireEventBus},
};

#[derive(Debug, Clone)]
pub struct CapabilitiesRegistry {
  inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
  snapshot: RwLock<Capabilities>,
  announces: RwLock<HashMap<Address, Announce>>,
  extensions: RwLock<HashMap<Address, HashSet<Uuid>>>,
  active_webapp: RwLock<Option<Uuid>>,
  announce_seq: AtomicU64,
  bus: WireEventBus,
  authority: AuthorityRegistry,
}

#[derive(Debug)]
struct Announce {
  caps: GatewayCapabilities,
  seq: u64,
}

impl CapabilitiesRegistry {
  pub fn new(bus: WireEventBus, authority: AuthorityRegistry) -> Self {
    Self {
      inner: Arc::new(Inner {
        snapshot: RwLock::new(Capabilities::default()),
        announces: RwLock::new(HashMap::new()),
        extensions: RwLock::new(HashMap::new()),
        active_webapp: RwLock::new(None),
        announce_seq: AtomicU64::new(0),
        bus,
        authority,
      }),
    }
  }

  pub fn snapshot(&self) -> Capabilities {
    self.inner.snapshot.read().expect("capabilities lock poisoned").clone()
  }

  pub async fn set_announce(&self, addr: Address, mut caps: GatewayCapabilities) -> WSResult<bool> {
    caps.uri_schemes = normalize_schemes(caps.uri_schemes);
    let provider = caps.music_provider;
    let seq = self.inner.announce_seq.fetch_add(1, Ordering::Relaxed);
    let provider_changed = {
      let mut guard = self.inner.announces.write().expect("announces lock poisoned");
      guard
        .insert(addr, Announce { caps, seq })
        .is_none_or(|prev| prev.caps.music_provider != provider)
    };
    self.rebuild_and_broadcast().await?;
    Ok(provider_changed)
  }

  pub async fn clear_companion(&self, addr: Address) -> WSResult<()> {
    {
      let mut guard = self.inner.announces.write().expect("announces lock poisoned");
      guard.remove(&addr);
    }
    self.drop_extensions(addr);
    self.inner.authority.drop_for(addr);
    self.rebuild_and_broadcast().await
  }

  pub async fn set_extensions_running(&self, addr: Address, webapps: Vec<Uuid>) -> WSResult<()> {
    {
      let mut guard = self.inner.extensions.write().expect("extensions lock poisoned");
      if webapps.is_empty() {
        guard.remove(&addr);
      } else {
        guard.insert(addr, webapps.into_iter().collect());
      }
    }
    self.rebuild_and_broadcast().await
  }

  pub async fn set_active_webapp(&self, active: Option<Uuid>) -> WSResult<()> {
    *self.inner.active_webapp.write().expect("active webapp lock poisoned") = active;
    self.rebuild_and_broadcast().await
  }

  pub async fn forget_extensions(&self, addr: Address) -> WSResult<()> {
    if !self.drop_extensions(addr) {
      return Ok(());
    }
    self.rebuild_and_broadcast().await
  }

  fn drop_extensions(&self, addr: Address) -> bool {
    self
      .inner
      .extensions
      .write()
      .expect("extensions lock poisoned")
      .remove(&addr)
      .is_some()
  }

  fn forward_available(&self) -> bool {
    let Some(active) = *self.inner.active_webapp.read().expect("active webapp lock poisoned") else {
      return false;
    };
    self
      .inner
      .extensions
      .read()
      .expect("extensions lock poisoned")
      .values()
      .any(|running| running.contains(&active))
  }

  pub async fn claim_authority(
    &self,
    addr: Address,
    scope: CompanionAuthorityScope,
    app_bundle: Option<String>,
  ) -> WSResult<()> {
    self.inner.authority.claim(addr, scope);
    self.inner.authority.set_companion_app_bundle(addr, app_bundle);
    self.rebuild_and_broadcast().await
  }

  pub async fn release_authority(&self, addr: Address, scope: CompanionAuthorityScope) -> WSResult<()> {
    self.inner.authority.release(addr, scope);
    self.rebuild_and_broadcast().await
  }

  pub async fn send_snapshot_to(&self, to: SocketAddr) -> WSResult<()> {
    let caps = self.snapshot();
    let event = BridgeToClientCapabilitiesMsgEvent::Update(CapabilitiesSnapshot { capabilities: caps });
    self.inner.bus.send_event(to, event).await
  }

  pub fn primary_addr(&self) -> Option<Address> {
    let announces = self.inner.announces.read().expect("announces lock poisoned");
    self.elect_addr(&announces)
  }

  fn elect_addr(&self, announces: &HashMap<Address, Announce>) -> Option<Address> {
    self
      .inner
      .authority
      .primary()
      .filter(|addr| announces.contains_key(addr))
      .or_else(|| announces.iter().max_by_key(|(_, a)| a.seq).map(|(addr, _)| *addr))
  }

  fn build_snapshot(&self) -> Capabilities {
    let forward = self.forward_available();
    let announces = self.inner.announces.read().expect("announces lock poisoned");
    let primary = self
      .elect_addr(&announces)
      .and_then(|addr| announces.get(&addr))
      .map(|a| &a.caps);
    let authority = self.inner.authority.live_scopes();

    match primary {
      Some(caps) => Capabilities {
        gateway: Some(caps.gateway.clone()),
        available: SurfaceAvailability {
          forward,
          ..caps.available
        },
        authority,
        uri_schemes: caps.uri_schemes.clone(),
        network: caps.network,
        audio: caps.audio.clone(),
        music_provider: caps.music_provider,
      },
      None => Capabilities {
        gateway: None,
        available: SurfaceAvailability {
          forward,
          ..Default::default()
        },
        authority,
        uri_schemes: Vec::new(),
        network: Default::default(),
        audio: Default::default(),
        music_provider: Default::default(),
      },
    }
  }

  async fn rebuild_and_broadcast(&self) -> WSResult<()> {
    let snapshot = self.build_snapshot();
    {
      let mut guard = self.inner.snapshot.write().expect("capabilities lock poisoned");
      *guard = snapshot.clone();
    }
    let event = BridgeToClientCapabilitiesMsgEvent::Update(CapabilitiesSnapshot { capabilities: snapshot });
    match self.inner.bus.broadcast_event(event).await {
      Ok(()) => Ok(()),
      Err(errs) => {
        for err in &errs {
          tracing::warn!(?err, "capabilities broadcast partial failure");
        }

        match errs.into_iter().next() {
          Some(e) => Err(e),
          None => Ok(()),
        }
      }
    }
  }
}

fn normalize_schemes(schemes: Vec<String>) -> Vec<String> {
  let mut seen: Vec<String> = Vec::with_capacity(schemes.len());
  for raw in schemes {
    let trimmed = raw.trim().trim_end_matches(':').to_ascii_lowercase();
    if !is_valid_scheme(&trimmed) {
      continue;
    }
    if !seen.iter().any(|s| s == &trimmed) {
      seen.push(trimmed);
    }
  }
  seen
}

pub(crate) fn is_valid_scheme(s: &str) -> bool {
  let mut chars = s.chars();
  let Some(first) = chars.next() else { return false };
  if !first.is_ascii_alphabetic() {
    return false;
  }
  chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
  use libbridgething::{AudioCapabilities, GatewayInfo, MusicProvider, NetworkInfo, NetworkKind, SurfaceAvailability};

  use super::*;

  fn caps_with(schemes: Vec<&str>, available: SurfaceAvailability) -> GatewayCapabilities {
    GatewayCapabilities {
      gateway: GatewayInfo {
        address: "00:11:22:33:44:55".into(),
        name: "test".into(),
        os_name: "ios".into(),
        ..Default::default()
      },
      uri_schemes: schemes.into_iter().map(String::from).collect(),
      network: NetworkInfo {
        kind: NetworkKind::Wifi,
        metered: false,
      },
      available,
      audio: AudioCapabilities::default(),
      music_provider: MusicProvider::Spotify,
    }
  }

  #[test]
  fn normalizes_schemes() {
    let out = normalize_schemes(vec![
      "Spotify:".into(),
      "  apple-music: ".into(),
      "spotify".into(), // dedup
      "".into(),
      "1bad".into(), // invalid
      "ok+v.2".into(),
    ]);
    assert_eq!(out, vec!["spotify", "apple-music", "ok+v.2"]);
  }

  #[tokio::test]
  async fn snapshot_empty_without_announce() {
    let (client_man, _listener) = crate::net::create_client_manager();
    let bus = WireEventBus::new(client_man);
    let auth = AuthorityRegistry::new();
    let reg = CapabilitiesRegistry::new(bus, auth);
    let snap = reg.snapshot();
    assert!(snap.gateway.is_none());
    assert!(snap.uri_schemes.is_empty());
    assert!(snap.authority.is_empty());
    assert_eq!(snap.available, SurfaceAvailability::default());
  }

  #[tokio::test]
  async fn announce_populates_snapshot_with_companion_availability() {
    let (client_man, _listener) = crate::net::create_client_manager();
    let bus = WireEventBus::new(client_man);
    let auth = AuthorityRegistry::new();
    let reg = CapabilitiesRegistry::new(bus, auth);

    let addr: Address = "00:11:22:33:44:55".parse().unwrap();
    let claimed = SurfaceAvailability {
      geo: true,
      notifications: true,
      net_fetch: true,
      net_ws: true,
      audio_tts: true,
      lyrics: true,
      playback_targets: true,
      forward: false,
    };
    let caps = caps_with(vec!["spotify:", "Apple-Music"], claimed);
    let _ = reg.set_announce(addr, caps).await;

    let snap = reg.snapshot();
    assert!(snap.gateway.is_some());
    assert_eq!(snap.uri_schemes, vec!["spotify", "apple-music"]);
    assert_eq!(snap.available, claimed);
  }

  #[tokio::test]
  async fn authority_mutations_appear_in_snapshot() {
    let (client_man, _listener) = crate::net::create_client_manager();
    let bus = WireEventBus::new(client_man);
    let auth = AuthorityRegistry::new();
    let reg = CapabilitiesRegistry::new(bus, auth.clone());

    let addr: Address = "00:11:22:33:44:55".parse().unwrap();
    let _ = reg
      .claim_authority(addr, CompanionAuthorityScope::NowPlayingMetadata, None)
      .await;
    let snap = reg.snapshot();
    assert_eq!(snap.authority, vec![CompanionAuthorityScope::NowPlayingMetadata]);
    assert!(auth.is_authoritative(CompanionAuthorityScope::NowPlayingMetadata));

    let _ = reg
      .release_authority(addr, CompanionAuthorityScope::NowPlayingMetadata)
      .await;
    assert!(reg.snapshot().authority.is_empty());
  }

  #[tokio::test]
  async fn a_running_set_report_never_moves_the_webapp_forward_is_derived_against() {
    let (client_man, _listener) = crate::net::create_client_manager();
    let bus = WireEventBus::new(client_man);
    let auth = AuthorityRegistry::new();
    let reg = CapabilitiesRegistry::new(bus, auth);

    let addr: Address = "00:11:22:33:44:55".parse().unwrap();
    let stale = Uuid::now_v7();
    let active = Uuid::now_v7();
    let _ = reg.set_active_webapp(Some(active)).await;

    let _ = reg.set_extensions_running(addr, vec![stale]).await;
    assert!(
      !reg.snapshot().available.forward,
      "a report about another webapp must not become the webapp forward is derived against"
    );

    let _ = reg.set_extensions_running(addr, vec![stale, active]).await;
    assert!(
      reg.snapshot().available.forward,
      "the active webapp is in the running set now"
    );
  }

  #[tokio::test]
  async fn a_running_set_dies_with_its_gateway_link_without_an_announce() {
    let (client_man, _listener) = crate::net::create_client_manager();
    let bus = WireEventBus::new(client_man);
    let auth = AuthorityRegistry::new();
    let reg = CapabilitiesRegistry::new(bus, auth);

    let addr: Address = "00:11:22:33:44:55".parse().unwrap();
    let active = Uuid::now_v7();
    let _ = reg.set_active_webapp(Some(active)).await;
    let _ = reg.set_extensions_running(addr, vec![active]).await;
    assert!(reg.snapshot().available.forward);

    let _ = reg.forget_extensions(addr).await;
    assert!(
      !reg.snapshot().available.forward,
      "nothing announced, so nothing but the link teardown can clear this"
    );
  }

  #[tokio::test]
  async fn clear_companion_drops_announce_and_authority() {
    let (client_man, _listener) = crate::net::create_client_manager();
    let bus = WireEventBus::new(client_man);
    let auth = AuthorityRegistry::new();
    let reg = CapabilitiesRegistry::new(bus, auth.clone());

    let addr: Address = "00:11:22:33:44:55".parse().unwrap();
    let _ = reg
      .set_announce(addr, caps_with(vec!["spotify"], SurfaceAvailability::default()))
      .await;
    let _ = reg
      .claim_authority(addr, CompanionAuthorityScope::NowPlayingPlayback, None)
      .await;
    assert!(reg.snapshot().gateway.is_some());
    assert!(!reg.snapshot().authority.is_empty());

    let _ = reg.clear_companion(addr).await;
    let snap = reg.snapshot();
    assert!(snap.gateway.is_none());
    assert!(snap.uri_schemes.is_empty());
    assert!(snap.authority.is_empty());
    assert!(!auth.is_authoritative(CompanionAuthorityScope::NowPlayingPlayback));
  }
}
