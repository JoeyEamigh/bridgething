use std::{
  collections::HashMap,
  net::SocketAddr,
  sync::Arc,
  time::{Duration, Instant},
};

use bridgething_delivery::discovery::Endpoint;
use tokio::task::JoinSet;

use crate::shell::Shell;

const FIRST_RETRY: Duration = Duration::from_secs(1);
const RETRY_CAP: Duration = Duration::from_secs(30);
const HELD: Duration = Duration::from_secs(30);

pub fn spawn(shell: Arc<Shell>, discovered: impl Fn() -> Vec<Endpoint> + Send + Sync + 'static) {
  tauri::async_runtime::spawn(drive(shell, discovered));
}

async fn drive(shell: Arc<Shell>, discovered: impl Fn() -> Vec<Endpoint>) {
  let wake = shell.wake();
  let mut schedule = HashMap::new();
  loop {
    match sweep(&shell, &discovered(), &mut schedule).await {
      Some(due) => {
        tokio::select! {
          () = wake.notified() => {}
          () = tokio::time::sleep(due) => {}
        }
      }
      None => wake.notified().await,
    }
  }
}

struct Attempt {
  backoff: Duration,
  next: Instant,
  opened: Option<Instant>,
}

impl Attempt {
  fn fresh() -> Self {
    Self {
      backoff: FIRST_RETRY,
      next: Instant::now(),
      opened: None,
    }
  }

  fn gone(&mut self) {
    let Some(opened) = self.opened.take() else {
      return;
    };
    self.backoff = if opened.elapsed() >= HELD {
      FIRST_RETRY
    } else {
      (self.backoff * 2).min(RETRY_CAP)
    };
    self.next = Instant::now() + self.backoff;
  }

  fn refused(&mut self) {
    self.backoff = (self.backoff * 2).min(RETRY_CAP);
    self.next = Instant::now() + self.backoff;
  }

  fn held(&mut self) {
    self.opened = Some(Instant::now());
  }
}

async fn sweep(shell: &Arc<Shell>, found: &[Endpoint], schedule: &mut HashMap<String, Attempt>) -> Option<Duration> {
  let discovered: Vec<String> = found.iter().map(|endpoint| endpoint.url.clone()).collect();
  let wanted = shell.auto_connect_targets(&discovered);
  schedule.retain(|url, _| wanted.contains(url));

  let linked = shell.linked_ids();
  let mut due = Vec::new();
  for url in wanted {
    if linked.contains(&url) {
      continue;
    }
    let attempt = schedule.entry(url.clone()).or_insert_with(Attempt::fresh);
    attempt.gone();
    if attempt.next <= Instant::now() {
      due.push(url);
    }
  }

  let addrs = resolved(due.iter().chain(linked.iter()).cloned().collect()).await;
  let held: Vec<Vec<SocketAddr>> = linked
    .iter()
    .map(|url| addrs.get(url).cloned().unwrap_or_default())
    .collect();
  let candidates: Vec<&Endpoint> = due
    .iter()
    .filter_map(|url| found.iter().find(|endpoint| &endpoint.url == url))
    .collect();

  let mut dials = JoinSet::new();
  for url in distinct(&candidates, &addrs, &held) {
    let label = found
      .iter()
      .find(|endpoint| endpoint.url == url)
      .map(|endpoint| endpoint.nickname.clone().unwrap_or_else(|| endpoint.host.clone()));
    let shell = Arc::clone(shell);
    dials.spawn(async move {
      let outcome = shell.dial(url.clone(), label).await;
      (url, outcome)
    });
  }

  while let Some(Ok((url, outcome))) = dials.join_next().await {
    let Some(attempt) = schedule.get_mut(&url) else {
      continue;
    };
    match outcome {
      Ok(_) => {
        tracing::info!(%url, "an attached device is linked");
        attempt.held();
      }
      Err(error) => {
        tracing::debug!(%url, %error, "an attached device is not answering yet");
        attempt.refused();
      }
    }
  }

  let now = Instant::now();
  schedule
    .values()
    .filter(|attempt| attempt.next > now)
    .map(|attempt| attempt.next - now)
    .min()
}

fn distinct(due: &[&Endpoint], addrs: &HashMap<String, Vec<SocketAddr>>, held: &[Vec<SocketAddr>]) -> Vec<String> {
  let of = |endpoint: &Endpoint| addrs.get(&endpoint.url).map_or(&[][..], Vec::as_slice);
  let overlaps = |a: &[SocketAddr], b: &[SocketAddr]| a.iter().any(|addr| b.contains(addr));
  let same = |a: &Endpoint, b: &Endpoint| match (a.serial.as_deref(), b.serial.as_deref()) {
    (Some(left), Some(right)) => left == right,
    _ => overlaps(of(a), of(b)),
  };

  let mut picked: Vec<&Endpoint> = Vec::new();
  for endpoint in due {
    if held.iter().any(|line| overlaps(of(endpoint), line)) {
      continue;
    }
    match picked.iter().position(|winner| same(endpoint, winner)) {
      Some(seat) if endpoint.browsed && !picked[seat].browsed => picked[seat] = endpoint,
      Some(_) => {}
      None => picked.push(endpoint),
    }
  }
  picked.into_iter().map(|endpoint| endpoint.url.clone()).collect()
}

async fn resolved(urls: Vec<String>) -> HashMap<String, Vec<SocketAddr>> {
  let mut lookups = JoinSet::new();
  for url in urls {
    lookups.spawn(async move {
      let addrs = match authority(&url) {
        Some(authority) => tokio::net::lookup_host(authority)
          .await
          .map(Iterator::collect)
          .unwrap_or_default(),
        None => Vec::new(),
      };
      (url, addrs)
    });
  }
  let mut out = HashMap::new();
  while let Some(Ok((url, addrs))) = lookups.join_next().await {
    out.insert(url, addrs);
  }
  out
}

fn authority(url: &str) -> Option<&str> {
  let rest = url.strip_prefix("ws://").or_else(|| url.strip_prefix("wss://"))?;
  let authority = rest.split('/').next().unwrap_or(rest);
  let port = &authority[authority.rfind(':')? + 1..];
  (!port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())).then_some(authority)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn addr(tail: u8) -> SocketAddr {
    SocketAddr::from(([10, 42, 0, tail], 8892))
  }

  fn table(rows: &[(&str, &[SocketAddr])]) -> HashMap<String, Vec<SocketAddr>> {
    rows
      .iter()
      .map(|(url, addrs)| ((*url).to_owned(), addrs.to_vec()))
      .collect()
  }

  fn browsed(host: &str, serial: Option<&str>) -> Endpoint {
    Endpoint {
      id: format!("{host}._bridgething._tcp.local."),
      url: format!("ws://{host}:8892/"),
      host: host.to_owned(),
      nickname: None,
      serial: serial.map(str::to_owned),
      browsed: true,
    }
  }

  fn well_known() -> Endpoint {
    Endpoint {
      id: "bridgething.local:8892".to_owned(),
      url: "ws://bridgething.local:8892/".to_owned(),
      host: "bridgething.local".to_owned(),
      nickname: None,
      serial: None,
      browsed: false,
    }
  }

  #[test]
  fn two_names_for_one_device_are_one_dial_and_the_browsed_name_wins_it() {
    let named = browsed("bridgething-q61r.local", None);
    let probe = well_known();
    let addrs = table(&[
      (named.url.as_str(), &[addr(2)][..]),
      (probe.url.as_str(), &[addr(2)][..]),
    ]);

    assert_eq!(
      distinct(&[&probe, &named], &addrs, &[]),
      vec![named.url.clone()],
      "the well-known host is the one name every device answers to, so it never wins a seat"
    );
  }

  #[test]
  fn a_published_serial_settles_a_duplicate_no_address_would_have() {
    let named = browsed("bridgething-q61r.local", Some("8558R481Q61R"));
    let moved = browsed("bridgething.local", Some("8558R481Q61R"));
    let addrs = table(&[
      (named.url.as_str(), &[addr(2)][..]),
      (moved.url.as_str(), &[addr(9)][..]),
    ]);

    assert_eq!(
      distinct(&[&named, &moved], &addrs, &[]).len(),
      1,
      "one serial is one device however many addresses it is answering on"
    );
  }

  #[test]
  fn two_serials_are_two_dials_even_behind_one_address() {
    let first = browsed("bridgething-q61r.local", Some("8558R481Q61R"));
    let second = browsed("bridgething-a12b.local", Some("1234A56B7A12B"));
    let addrs = table(&[
      (first.url.as_str(), &[addr(2)][..]),
      (second.url.as_str(), &[addr(2)][..]),
    ]);

    assert_eq!(
      distinct(&[&first, &second], &addrs, &[]).len(),
      2,
      "a shared address is a guess and a serial is not, so the serial decides"
    );
  }

  #[test]
  fn two_devices_are_two_dials() {
    let first = browsed("bridgething-q61r.local", None);
    let second = browsed("bridgething-a12b.local", None);
    let addrs = table(&[
      (first.url.as_str(), &[addr(2)][..]),
      (second.url.as_str(), &[addr(10)][..]),
    ]);

    assert_eq!(
      distinct(&[&first, &second], &addrs, &[]).len(),
      2,
      "every attached device gets its own dial"
    );
  }

  #[test]
  fn a_second_name_for_a_device_already_linked_is_left_alone() {
    let probe = well_known();
    let addrs = table(&[(probe.url.as_str(), &[addr(2)][..])]);
    let held = vec![vec![addr(2)]];

    assert!(
      distinct(&[&probe], &addrs, &held).is_empty(),
      "a device holding a link is not dialed again under another name"
    );
  }

  #[test]
  fn a_name_that_does_not_resolve_is_still_dialed() {
    let first = browsed("bridgething-q61r.local", None);
    let second = browsed("bridgething-a12b.local", None);
    let addrs = table(&[(first.url.as_str(), &[][..]), (second.url.as_str(), &[][..])]);

    assert_eq!(
      distinct(&[&first, &second], &addrs, &[]).len(),
      2,
      "an unresolvable name cannot be proven a duplicate, so the dial decides"
    );
  }

  #[test]
  fn the_authority_is_the_dialable_host_and_port() {
    assert_eq!(
      authority("ws://bridgething.local:8892/"),
      Some("bridgething.local:8892")
    );
    assert_eq!(authority("ws://127.0.0.1:8892/"), Some("127.0.0.1:8892"));
    assert_eq!(
      authority("ws://bridgething.local/"),
      None,
      "no port, nothing to resolve"
    );
    assert_eq!(authority("http://x:1/"), None, "not a gateway url shape");
  }
}
