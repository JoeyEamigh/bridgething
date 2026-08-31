use std::{
  collections::BTreeMap,
  net::{TcpStream, ToSocketAddrs},
  sync::{Arc, Mutex, Weak},
  time::Duration,
};

use libbridgething::{BRIDGETHING_MDNS_SERVICE_TYPE, BRIDGETHING_NETWORK_GATEWAY_PORT};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;

const NICKNAME_TXT_KEY: &str = "nickname";
const SERIAL_TXT_KEY: &str = "serial";
const WELL_KNOWN_HOST: &str = "bridgething.local";

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const PROBE_INTERVAL: Duration = Duration::from_secs(5);
const PROBE_INTERVAL_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
  #[error("the mdns responder did not start: {0}")]
  Responder(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
  pub id: String,
  pub url: String,
  pub host: String,
  pub nickname: Option<String>,
  pub serial: Option<String>,
  pub browsed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointChange {
  Found(Endpoint),
  Lost(Endpoint),
}

pub struct Discovery {
  found: Mutex<BTreeMap<String, Endpoint>>,
  probed: Mutex<Option<Endpoint>>,
  daemon: ServiceDaemon,
}

impl Discovery {
  pub fn spawn(on_change: impl Fn(EndpointChange) + Send + Sync + 'static) -> Result<Arc<Self>, DiscoveryError> {
    let daemon = ServiceDaemon::new().map_err(|error| DiscoveryError::Responder(error.to_string()))?;
    let service_type = format!("{BRIDGETHING_MDNS_SERVICE_TYPE}.local.");
    let receiver = daemon
      .browse(&service_type)
      .map_err(|error| DiscoveryError::Responder(error.to_string()))?;

    let discovery = Arc::new(Self {
      found: Mutex::new(BTreeMap::new()),
      probed: Mutex::new(None),
      daemon,
    });

    let on_change = Arc::new(on_change);

    let browsing: Weak<Self> = Arc::downgrade(&discovery);
    let announce = Arc::clone(&on_change);
    std::thread::spawn(move || {
      for event in receiver {
        let change = {
          let Some(discovery) = browsing.upgrade() else { break };
          match event {
            ServiceEvent::ServiceResolved(info) => {
              endpoint_for(&info).and_then(|endpoint| discovery.remember(endpoint))
            }
            ServiceEvent::ServiceRemoved(_, fullname) => discovery.forget(&fullname),
            _ => None,
          }
        };
        if let Some(change) = change {
          announce(change);
        }
      }
    });

    let probing: Weak<Self> = Arc::downgrade(&discovery);
    std::thread::spawn(move || probe_well_known_host(probing, on_change));

    Ok(discovery)
  }

  pub fn endpoints(&self) -> Vec<Endpoint> {
    let browsed: Vec<Endpoint> = self.found.lock().unwrap().values().cloned().collect();
    let probed = self.probed.lock().unwrap().clone();
    offered(browsed, probed)
  }

  fn settle_probe(&self, live: bool) -> Option<EndpointChange> {
    let mut probed = self.probed.lock().unwrap();
    match (live, probed.take()) {
      (true, Some(held)) => {
        *probed = Some(held);
        None
      }
      (true, None) => {
        let fresh = well_known_endpoint();
        *probed = Some(fresh.clone());
        Some(EndpointChange::Found(fresh))
      }
      (false, Some(lost)) => Some(EndpointChange::Lost(lost)),
      (false, None) => None,
    }
  }

  fn remember(&self, endpoint: Endpoint) -> Option<EndpointChange> {
    let mut found = self.found.lock().unwrap();
    if found.get(&endpoint.id) == Some(&endpoint) {
      return None;
    }
    found.insert(endpoint.id.clone(), endpoint.clone());
    Some(EndpointChange::Found(endpoint))
  }

  fn forget(&self, id: &str) -> Option<EndpointChange> {
    self.found.lock().unwrap().remove(id).map(EndpointChange::Lost)
  }
}

impl Drop for Discovery {
  fn drop(&mut self) {
    let _ = self.daemon.shutdown();
  }
}

fn offered(browsed: Vec<Endpoint>, probed: Option<Endpoint>) -> Vec<Endpoint> {
  let mut out = browsed;
  if let Some(probed) = probed
    && !out.iter().any(|held| held.url == probed.url)
  {
    out.push(probed);
  }
  out
}

fn well_known_endpoint() -> Endpoint {
  Endpoint {
    id: format!("{WELL_KNOWN_HOST}:{BRIDGETHING_NETWORK_GATEWAY_PORT}"),
    url: format!("ws://{WELL_KNOWN_HOST}:{BRIDGETHING_NETWORK_GATEWAY_PORT}/"),
    host: WELL_KNOWN_HOST.to_owned(),
    nickname: None,
    serial: None,
    browsed: false,
  }
}

fn reachable(authority: &str, within: Duration) -> bool {
  let Ok(mut addrs) = authority.to_socket_addrs() else {
    return false;
  };
  addrs.any(|addr| TcpStream::connect_timeout(&addr, within).is_ok())
}

fn probe_well_known_host<F>(alive: Weak<Discovery>, on_change: Arc<F>)
where
  F: Fn(EndpointChange) + Send + Sync + 'static,
{
  let authority = format!("{WELL_KNOWN_HOST}:{BRIDGETHING_NETWORK_GATEWAY_PORT}");
  let mut backoff = PROBE_INTERVAL;
  loop {
    let Some(discovery) = alive.upgrade() else { return };
    let live = reachable(&authority, PROBE_TIMEOUT);
    let change = discovery.settle_probe(live);
    drop(discovery);

    backoff = if live {
      PROBE_INTERVAL
    } else {
      (backoff * 2).min(PROBE_INTERVAL_MAX)
    };

    if let Some(change) = change {
      on_change(change);
    }
    if !nap(&alive, backoff) {
      return;
    }
  }
}

fn nap(alive: &Weak<Discovery>, mut left: Duration) -> bool {
  const SLICE: Duration = Duration::from_millis(250);
  while !left.is_zero() {
    if alive.strong_count() == 0 {
      return false;
    }
    let slice = left.min(SLICE);
    std::thread::sleep(slice);
    left -= slice;
  }
  alive.strong_count() > 0
}

fn endpoint_for(service: &mdns_sd::ResolvedService) -> Option<Endpoint> {
  if !service.is_valid() {
    return None;
  }
  let host = service.get_hostname().trim_end_matches('.').to_owned();
  if host.is_empty() {
    return None;
  }
  let port = service.get_port();
  let text = |key| {
    service
      .txt_properties
      .get_property_val_str(key)
      .filter(|value| !value.is_empty())
      .map(str::to_owned)
  };
  Some(Endpoint {
    id: service.get_fullname().to_owned(),
    url: format!("ws://{host}:{port}/"),
    host,
    nickname: text(NICKNAME_TXT_KEY),
    serial: text(SERIAL_TXT_KEY),
    browsed: true,
  })
}

#[cfg(test)]
mod tests {
  use std::{
    sync::mpsc,
    time::{Duration, Instant},
  };

  use mdns_sd::ServiceInfo;

  use super::*;

  static PROBE_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

  fn unique_instance() -> String {
    let seq = PROBE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("probe-{}-{seq}", std::process::id())
  }

  fn probe_host(instance: &str) -> String {
    format!("{instance}.local.")
  }

  fn announce(instance: &str, nickname: &str) -> ServiceDaemon {
    let registrar = ServiceDaemon::new().expect("the registrar starts");
    let info = ServiceInfo::new(
      &format!("{BRIDGETHING_MDNS_SERVICE_TYPE}.local."),
      instance,
      &probe_host(instance),
      "127.0.0.1",
      8892,
      &[(NICKNAME_TXT_KEY, nickname), (SERIAL_TXT_KEY, "8558R481Q61R")][..],
    )
    .expect("the announcement is well formed");
    registrar.register(info).expect("the announcement goes out");
    registrar
  }

  fn changes() -> (
    impl Fn(EndpointChange) + Send + Sync + 'static,
    mpsc::Receiver<EndpointChange>,
  ) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    (
      move |change| {
        let _ = tx.lock().unwrap().send(change);
      },
      rx,
    )
  }

  fn wait_for(rx: &mpsc::Receiver<EndpointChange>, instance: &str, within: Duration) -> Endpoint {
    let fullname = format!("{instance}.{BRIDGETHING_MDNS_SERVICE_TYPE}.local.");
    let deadline = Instant::now() + within;
    loop {
      let remaining = deadline.saturating_duration_since(Instant::now());
      assert!(!remaining.is_zero(), "{instance} never arrived on the link");
      if let Ok(EndpointChange::Found(endpoint)) = rx.recv_timeout(remaining)
        && endpoint.id == fullname
      {
        return endpoint;
      }
    }
  }

  #[test]
  fn an_announcement_on_the_link_becomes_a_dialable_endpoint() {
    let instance = unique_instance();
    let _registrar = announce(&instance, "Headless Probe");

    let (on_change, rx) = changes();
    let discovery = Discovery::spawn(on_change).expect("the responder starts");

    let endpoint = wait_for(&rx, &instance, Duration::from_secs(20));

    assert_eq!(
      endpoint.host,
      probe_host(&instance).trim_end_matches('.'),
      "the announcement names its own probe host, not another test's"
    );
    assert_eq!(
      endpoint.url,
      format!("ws://{}:8892/", probe_host(&instance).trim_end_matches('.')),
      "the endpoint is dialable by hostname"
    );
    assert_eq!(
      endpoint.nickname.as_deref(),
      Some("Headless Probe"),
      "the nickname txt record names the endpoint"
    );
    assert_eq!(
      endpoint.serial.as_deref(),
      Some("8558R481Q61R"),
      "and the serial txt record identifies the device behind it"
    );
    assert!(endpoint.browsed, "a browse hit is not the well-known probe");
    assert!(
      discovery.endpoints().iter().any(|held| held.id == endpoint.id),
      "the browse result is retained for the picker"
    );
  }

  #[test]
  fn a_withdrawn_announcement_is_dropped_from_the_picker() {
    let instance = unique_instance();
    let registrar = announce(&instance, "Departing Probe");

    let (on_change, rx) = changes();
    let discovery = Discovery::spawn(on_change).expect("the responder starts");

    let endpoint = wait_for(&rx, &instance, Duration::from_secs(20));
    registrar.shutdown().expect("the registrar withdraws");

    let deadline = Instant::now() + Duration::from_secs(20);
    while discovery.endpoints().iter().any(|held| held.id == endpoint.id) {
      assert!(
        Instant::now() < deadline,
        "{instance} was never dropped from the picker"
      );
      std::thread::sleep(Duration::from_millis(50));
    }
  }

  #[test]
  fn a_listening_host_is_reachable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port to answer on");
    let authority = listener.local_addr().expect("the bound address").to_string();

    assert!(reachable(&authority, PROBE_TIMEOUT), "a listening gateway answers");
  }

  #[test]
  fn a_resolvable_host_that_refuses_the_port_is_not_reachable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port to answer on");
    let authority = listener.local_addr().expect("the bound address").to_string();
    drop(listener);

    assert!(
      !reachable(&authority, PROBE_TIMEOUT),
      "a refused connect is not a gateway"
    );
  }

  #[test]
  fn an_unresolvable_host_is_not_reachable() {
    let started = Instant::now();

    assert!(
      !reachable("no-such-host.invalid:8892", PROBE_TIMEOUT),
      "a name that does not resolve is not a gateway"
    );
    assert!(
      started.elapsed() < Duration::from_secs(15),
      "an absent device must stay cheap, took {:?}",
      started.elapsed()
    );
  }

  #[test]
  fn the_well_known_host_is_offered_when_no_announcement_covers_it() {
    let offered = offered(Vec::new(), Some(well_known_endpoint()));

    let only = offered.first().expect("the well known host is offered on its own");
    assert_eq!(only.url, "ws://bridgething.local:8892/", "it is dialable as it stands");
    assert_eq!(only.host, "bridgething.local");
    assert_eq!(offered.len(), 1);
  }

  #[test]
  fn the_well_known_host_is_suppressed_when_the_announcement_names_the_same_gateway() {
    let announced = Endpoint {
      id: "bridgething Bridgething Gateway._bridgething._tcp.local.".to_owned(),
      url: "ws://bridgething.local:8892/".to_owned(),
      host: "bridgething.local".to_owned(),
      nickname: Some("UART Superbird".to_owned()),
      serial: Some("8558R481Q61R".to_owned()),
      browsed: true,
    };

    let offered = offered(vec![announced.clone()], Some(well_known_endpoint()));

    assert_eq!(
      offered,
      vec![announced],
      "one gateway is one row, and it keeps its nickname"
    );
  }

  #[test]
  fn a_different_gateway_does_not_suppress_the_well_known_host() {
    let elsewhere = Endpoint {
      id: "other._bridgething._tcp.local.".to_owned(),
      url: "ws://other-thing.local:8892/".to_owned(),
      host: "other-thing.local".to_owned(),
      nickname: None,
      serial: None,
      browsed: true,
    };

    let offered = offered(vec![elsewhere], Some(well_known_endpoint()));

    assert_eq!(offered.len(), 2, "an unrelated announcement leaves the probe standing");
  }

  #[test]
  #[ignore]
  fn a_daemon_on_the_link_is_discovered_and_names_itself() {
    let (on_change, rx) = changes();
    let discovery = Discovery::spawn(on_change).expect("the responder starts");

    let change = rx
      .recv_timeout(Duration::from_secs(15))
      .expect("a daemon announces itself within the browse window");
    assert!(
      matches!(change, EndpointChange::Found(_)),
      "an announcing daemon arrives as a found, got {change:?}"
    );

    let found = discovery.endpoints();
    let endpoint = found.first().expect("the browse result is retained for the picker");
    assert!(
      endpoint.url.starts_with("ws://") && endpoint.url.ends_with('/'),
      "the endpoint is a dialable gateway url, got {}",
      endpoint.url
    );
    assert!(
      endpoint.host.ends_with(".local"),
      "the hostname is preferred over the address, got {}",
      endpoint.host
    );
  }
}
