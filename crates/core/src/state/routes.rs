use std::{
  collections::HashMap,
  net::SocketAddr,
  sync::{Arc, RwLock},
};

use uuid::Uuid;

use crate::bluetooth::Address;

#[derive(Debug, Clone, Copy)]
struct Route {
  owner: SocketAddr,
  gateway: Address,
}

#[derive(Debug, Clone, Default)]
pub struct RouteTable {
  inner: Arc<RwLock<HashMap<Uuid, Route>>>,
}

impl RouteTable {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn register(&self, id: Uuid, owner: SocketAddr, gateway: Address) {
    self
      .inner
      .write()
      .expect("route table poisoned")
      .insert(id, Route { owner, gateway });
  }

  pub fn lookup(&self, id: Uuid) -> Option<SocketAddr> {
    self
      .inner
      .read()
      .expect("route table poisoned")
      .get(&id)
      .map(|route| route.owner)
  }

  pub fn gateway_of(&self, id: Uuid) -> Option<Address> {
    self
      .inner
      .read()
      .expect("route table poisoned")
      .get(&id)
      .map(|route| route.gateway)
  }

  pub fn drop_id(&self, id: Uuid) -> Option<SocketAddr> {
    self
      .inner
      .write()
      .expect("route table poisoned")
      .remove(&id)
      .map(|route| route.owner)
  }

  pub fn drain_for_owner(&self, owner: SocketAddr) -> Vec<(Uuid, Address)> {
    self
      .drain_where(|route| route.owner == owner)
      .into_iter()
      .map(|(id, route)| (id, route.gateway))
      .collect()
  }

  pub fn drain_for_gateway(&self, gateway: Address) -> Vec<(Uuid, SocketAddr)> {
    self
      .drain_where(|route| route.gateway == gateway)
      .into_iter()
      .map(|(id, route)| (id, route.owner))
      .collect()
  }

  fn drain_where(&self, pred: impl Fn(&Route) -> bool) -> Vec<(Uuid, Route)> {
    let mut guard = self.inner.write().expect("route table poisoned");
    let hit: Vec<Uuid> = guard
      .iter()
      .filter(|(_, route)| pred(route))
      .map(|(id, _)| *id)
      .collect();
    hit
      .into_iter()
      .filter_map(|id| guard.remove(&id).map(|route| (id, route)))
      .collect()
  }
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

  #[test]
  fn register_lookup_drop() {
    let table = RouteTable::new();
    let id = Uuid::now_v7();
    let a = addr("127.0.0.1:1000");
    table.register(id, a, gw(1));
    assert_eq!(table.lookup(id), Some(a));
    assert_eq!(table.drop_id(id), Some(a));
    assert_eq!(table.lookup(id), None);
  }

  #[test]
  fn drain_for_owner_returns_only_matching() {
    let table = RouteTable::new();
    let alice = addr("127.0.0.1:1");
    let bob = addr("127.0.0.1:2");
    let a1 = Uuid::now_v7();
    let a2 = Uuid::now_v7();
    let b1 = Uuid::now_v7();
    table.register(a1, alice, gw(1));
    table.register(a2, alice, gw(2));
    table.register(b1, bob, gw(1));

    let mut drained = table.drain_for_owner(alice);
    drained.sort();
    let mut expected = vec![(a1, gw(1)), (a2, gw(2))];
    expected.sort();
    assert_eq!(drained, expected);
    assert_eq!(table.lookup(a1), None);
    assert_eq!(table.lookup(b1), Some(bob));
  }

  #[test]
  fn drain_for_gateway_spares_other_companions() {
    let table = RouteTable::new();
    let client = addr("127.0.0.1:1");
    let phone_route = Uuid::now_v7();
    let desktop_route = Uuid::now_v7();
    table.register(phone_route, client, gw(1));
    table.register(desktop_route, client, gw(2));

    let drained = table.drain_for_gateway(gw(2));
    assert_eq!(drained, vec![(desktop_route, client)]);
    assert_eq!(
      table.lookup(phone_route),
      Some(client),
      "one companion leaving tore down another's route"
    );
  }
}
