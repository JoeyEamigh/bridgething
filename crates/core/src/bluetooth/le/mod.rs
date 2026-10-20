use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use bluer::{Adapter, Device, gatt::remote::Service};
#[cfg(target_os = "linux")]
use futures::{Stream, StreamExt, stream};
use libbridgething::AncsAuthState;
#[cfg(target_os = "linux")]
use libbridgething::client::VolumeChanged;
use tokio::sync::mpsc;
#[cfg(target_os = "linux")]
use tokio::{task::JoinHandle, time};
#[cfg(target_os = "linux")]
use tokio_util::sync::CancellationToken;
#[cfg(target_os = "linux")]
use uuid::Uuid;
#[cfg(target_os = "linux")]
use zbus::Connection;

#[cfg(target_os = "linux")]
mod advertise;
#[cfg(target_os = "linux")]
mod ams;
#[cfg(target_os = "linux")]
mod ancs;
#[cfg(target_os = "linux")]
mod pair_trigger;

#[cfg(target_os = "linux")]
use advertise::LeAdvertisement;
#[cfg(target_os = "linux")]
use ancs::AuthStateReporter;
#[cfg(target_os = "linux")]
use pair_trigger::{PAIR_TRIGGER_SERVICE, PairTrigger};

use super::Address;
#[cfg(target_os = "linux")]
use super::hci;
#[cfg(target_os = "linux")]
use crate::{bluetooth::BluetoothMan, net::WireEventBus, state::AudioManager};

const COMMAND_MAILBOX_CAP: usize = 16;
#[cfg(target_os = "linux")]
const TRANSIENT_BACKOFF_INITIAL: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const TRANSIENT_BACKOFF_MAX: Duration = Duration::from_secs(60);
#[cfg(target_os = "linux")]
const ANCS_REPROBE_INTERVAL: Duration = Duration::from_secs(15 * 60);
#[cfg(target_os = "linux")]
const LE_PROBE_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
const ACL_DOWN_HEARTBEAT: Duration = Duration::from_secs(60);
#[cfg(target_os = "linux")]
const ADV_CHECK_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(target_os = "linux")]
const ADV_REASSERT_AFTER: Duration = Duration::from_secs(90);
#[cfg(target_os = "linux")]
const GATT_SERVICE_INTERFACE: &str = "org.bluez.GattService1";

pub const ACTION_POSITIVE: u8 = 0x00;
pub const ACTION_NEGATIVE: u8 = 0x01;
pub const ANCS_ID_PREFIX: &str = "ancs:";

const LE_TAP_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeCommand {
  Attach { address: Address },
  Detach { address: Address },
  Invoke { uid: u32, action: u8 },
}

pub type LeTapTx = tokio::sync::broadcast::Sender<LeCommand>;

#[derive(Debug, Clone)]
pub struct LeManager {
  tx: mpsc::Sender<LeCommand>,
  ancs_auth: Arc<std::sync::Mutex<AncsAuthState>>,
}

pub(crate) struct LeBootstrap {
  rx: mpsc::Receiver<LeCommand>,
  tap: LeTapTx,
  #[cfg(target_os = "linux")]
  ancs_auth: Arc<std::sync::Mutex<AncsAuthState>>,
  #[cfg(target_os = "linux")]
  serial_suffix: [char; 4],
}

impl LeBootstrap {
  pub(crate) fn tap(&self) -> LeTapTx {
    self.tap.clone()
  }

  pub(crate) fn spawn_no_radio(mut self) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
      while let Some(command) = self.rx.recv().await {
        tracing::debug!(?command, "LE command dropped: no radio attached");
        let _ = self.tap.send(command);
      }
    })
  }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, PartialEq, Eq)]
enum Attached<S> {
  Kept,
  Started { replaced: Option<S> },
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug)]
struct SessionSlot<S> {
  active: Option<(Address, S)>,
}

#[cfg(any(target_os = "linux", test))]
impl<S> SessionSlot<S> {
  fn new() -> Self {
    Self { active: None }
  }

  fn attach(&mut self, address: Address, start: impl FnOnce() -> S) -> Attached<S> {
    if self.address() == Some(address) {
      return Attached::Kept;
    }
    let replaced = self.active.replace((address, start())).map(|(_, session)| session);
    Attached::Started { replaced }
  }

  fn detach(&mut self, address: Address) -> Option<S> {
    self
      .active
      .take_if(|(held, _)| *held == address)
      .map(|(_, session)| session)
  }

  fn evict_if(&mut self, dead: impl FnOnce(&S) -> bool) -> Option<S> {
    self
      .active
      .take_if(|(_, session)| dead(session))
      .map(|(_, session)| session)
  }

  fn address(&self) -> Option<Address> {
    self.active.as_ref().map(|(address, _)| *address)
  }

  #[cfg(target_os = "linux")]
  fn session(&self) -> Option<&S> {
    self.active.as_ref().map(|(_, session)| session)
  }
}

impl LeManager {
  pub(crate) fn allocate(
    #[cfg_attr(not(target_os = "linux"), expect(unused_variables))] serial_suffix: [char; 4],
  ) -> (Self, LeBootstrap) {
    let (tx, rx) = mpsc::channel(COMMAND_MAILBOX_CAP);
    let (tap, _) = tokio::sync::broadcast::channel(LE_TAP_CAPACITY);
    let ancs_auth = Arc::new(std::sync::Mutex::new(AncsAuthState::Unknown));
    (
      Self {
        tx,
        ancs_auth: ancs_auth.clone(),
      },
      LeBootstrap {
        rx,
        tap,
        #[cfg(target_os = "linux")]
        ancs_auth,
        #[cfg(target_os = "linux")]
        serial_suffix,
      },
    )
  }

  pub fn ancs_auth_state(&self) -> AncsAuthState {
    *self.ancs_auth.lock().unwrap()
  }

  pub async fn attach(&self, address: Address) {
    if self.tx.send(LeCommand::Attach { address }).await.is_err() {
      tracing::warn!(%address, "LE dispatcher closed; cannot attach");
    }
  }

  pub async fn detach(&self, address: Address) {
    if self.tx.send(LeCommand::Detach { address }).await.is_err() {
      tracing::trace!(%address, "LE dispatcher closed; detach no-op");
    }
  }

  pub async fn try_invoke_positive(&self, id: &str) -> bool {
    self.try_invoke(id, ACTION_POSITIVE).await
  }

  pub async fn try_invoke_negative(&self, id: &str) -> bool {
    self.try_invoke(id, ACTION_NEGATIVE).await
  }

  async fn try_invoke(&self, id: &str, action: u8) -> bool {
    let Some(rest) = id.strip_prefix(ANCS_ID_PREFIX) else {
      return false;
    };
    let Ok(uid) = rest.parse::<u32>() else {
      tracing::trace!(%id, "ANCS invoke: malformed UID");
      return true;
    };
    if self.tx.send(LeCommand::Invoke { uid, action }).await.is_err() {
      tracing::trace!(%id, "LE dispatcher closed; invoke dropped");
    }
    true
  }
}

#[cfg(target_os = "linux")]
impl LeBootstrap {
  pub(crate) async fn start(
    self,
    adapter: Adapter,
    bus: WireEventBus,
    bluetooth: BluetoothMan,
    audio: AudioManager,
  ) -> JoinHandle<()> {
    let adapter_dbus_path = format!("/org/bluez/{}", adapter.name());
    let pair_trigger = match PairTrigger::register(&adapter).await {
      Ok(handle) => Some(handle),
      Err(err) => {
        tracing::warn!(
          ?err,
          "LE pair-trigger GATT register failed; companion-app LE pair will not work"
        );
        None
      }
    };
    let advertisement =
      match LeAdvertisement::register(self.serial_suffix, &adapter_dbus_path, PAIR_TRIGGER_SERVICE).await {
        Ok(handle) => {
          tracing::info!("LE advertisement registered; companion app drives LE pair via AccessorySetupKit");
          Some(handle)
        }
        Err(err) => {
          tracing::warn!(
            ?err,
            "LE advertisement register failed; iOS notifications + volume state unavailable"
          );
          None
        }
      };
    let dispatcher = LeDispatcher {
      adapter: Arc::new(adapter),
      adapter_dbus_path,
      bus,
      audio,
      sessions: SessionSlot::new(),
      auth_reporter: AuthStateReporter::new(bluetooth, self.ancs_auth),
      advertisement,
      acl_down_since: None,
      serial_suffix: self.serial_suffix,
      _pair_trigger: pair_trigger,
    };
    tokio::spawn(dispatcher.run(self.rx))
  }
}

#[cfg(target_os = "linux")]
struct LeDispatcher {
  adapter: Arc<Adapter>,
  adapter_dbus_path: String,
  bus: WireEventBus,
  audio: AudioManager,
  sessions: SessionSlot<ActiveSession>,
  auth_reporter: AuthStateReporter,
  advertisement: Option<LeAdvertisement>,
  acl_down_since: Option<time::Instant>,
  serial_suffix: [char; 4],
  _pair_trigger: Option<PairTrigger>,
}

#[cfg(target_os = "linux")]
struct ActiveSession {
  invoke_tx: mpsc::Sender<(u32, u8)>,
  cancel: CancellationToken,
  handle: JoinHandle<()>,
}

#[cfg(target_os = "linux")]
impl LeDispatcher {
  async fn run(mut self, mut rx: mpsc::Receiver<LeCommand>) {
    let mut adv_check = time::interval(ADV_CHECK_INTERVAL);
    adv_check.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    loop {
      tokio::select! {
        cmd = rx.recv() => match cmd {
          Some(LeCommand::Attach { address }) => self.handle_attach(address).await,
          Some(LeCommand::Detach { address }) => self.handle_detach(address).await,
          Some(LeCommand::Invoke { uid, action }) => self.handle_invoke(uid, action).await,
          None => return,
        },
        _ = adv_check.tick() => self.reassert_advertisement_if_stalled().await,
      }
    }
  }

  async fn reassert_advertisement_if_stalled(&mut self) {
    let Some(address) = self.sessions.address() else {
      self.acl_down_since = None;
      return;
    };
    if le_acl_up(&self.adapter, address) {
      self.acl_down_since = None;
      return;
    }
    let down_since = *self.acl_down_since.get_or_insert_with(time::Instant::now);
    if down_since.elapsed() < ADV_REASSERT_AFTER {
      return;
    }
    self.acl_down_since = None;
    tracing::info!(%address, "LE ACL down with a phone attached; re-registering advertisement");
    if let Some(old) = self.advertisement.take() {
      old.unregister().await;
    }
    match LeAdvertisement::register(self.serial_suffix, &self.adapter_dbus_path, PAIR_TRIGGER_SERVICE).await {
      Ok(handle) => self.advertisement = Some(handle),
      Err(err) => tracing::warn!(?err, "LE advertisement re-register failed; will retry"),
    }
  }

  async fn handle_attach(&mut self, address: Address) {
    if self.sessions.evict_if(|session| session.handle.is_finished()).is_some() {
      tracing::info!(%address, "LE session had exited; starting a fresh one");
    }
    let previous = self.sessions.address();
    let attached = self.sessions.attach(address, || {
      let cancel = CancellationToken::new();
      let (invoke_tx, invoke_rx) = mpsc::channel(COMMAND_MAILBOX_CAP);
      let session = LeSession {
        address,
        adapter: self.adapter.clone(),
        bus: self.bus.clone(),
        audio: self.audio.clone(),
        auth_reporter: self.auth_reporter.clone(),
        invoke_rx,
        cancel: cancel.clone(),
      };
      ActiveSession {
        invoke_tx,
        cancel,
        handle: tokio::spawn(session.run()),
      }
    });
    match attached {
      Attached::Kept => tracing::debug!(%address, "LE session already serving this phone; keeping it"),
      Attached::Started { replaced } => {
        if let Some(replaced) = replaced {
          tracing::info!(prev = ?previous, new = %address, "LE replacing active session");
          replaced.cancel.cancel();
        }
        self.auth_reporter.report(AncsAuthState::Probing).await;
      }
    }
  }

  async fn handle_detach(&mut self, address: Address) {
    if let Some(session) = self.sessions.detach(address) {
      tracing::info!(%address, "LE session released; the LE link stays with the phone");
      session.cancel.cancel();
      self.auth_reporter.report(AncsAuthState::Probing).await;
    } else {
      tracing::trace!(%address, "LE detach for non-active address; ignoring");
    }
  }

  async fn handle_invoke(&self, uid: u32, action: u8) {
    let Some(session) = self.sessions.session() else {
      tracing::trace!(uid, "ANCS invoke: no active session");
      return;
    };
    if session.invoke_tx.send((uid, action)).await.is_err() {
      tracing::trace!(uid, "ANCS invoke: session task closed");
    }
  }
}

#[cfg(target_os = "linux")]
struct LeSession {
  address: Address,
  adapter: Arc<Adapter>,
  bus: WireEventBus,
  audio: AudioManager,
  auth_reporter: AuthStateReporter,
  invoke_rx: mpsc::Receiver<(u32, u8)>,
  cancel: CancellationToken,
}

#[cfg(target_os = "linux")]
enum LoopExit {
  Cancelled,
  ConnectionLost,
  ReprobeAncs,
}

#[cfg(target_os = "linux")]
impl LeSession {
  async fn run(self) {
    let LeSession {
      address,
      adapter,
      bus,
      audio,
      auth_reporter,
      mut invoke_rx,
      cancel,
    } = self;

    let conn = match Connection::system().await {
      Ok(conn) => conn,
      Err(err) => {
        tracing::error!(%address, ?err, "LE session: system bus connect failed; aborting session");
        return;
      }
    };

    let mut backoff = TRANSIENT_BACKOFF_INITIAL;
    let mut absent_logged = false;
    let mut acl_watch = AclWatch::default();

    loop {
      if cancel.is_cancelled() {
        return;
      }
      let outcome = attempt(
        &conn,
        &adapter,
        address,
        &bus,
        &audio,
        &auth_reporter,
        &mut invoke_rx,
        &cancel,
        &mut absent_logged,
        &mut acl_watch,
        &mut backoff,
      )
      .await;
      match outcome {
        Ok(LoopExit::Cancelled) => return,
        Ok(LoopExit::ReprobeAncs) => {}
        Ok(LoopExit::ConnectionLost) => {
          tokio::select! {
            _ = cancel.cancelled() => return,
            _ = time::sleep(LE_PROBE_INTERVAL) => {}
          }
        }
        Err(err) if cancel.is_cancelled() => {
          tracing::debug!(%address, ?err, "LE session ended after detach");
          return;
        }
        Err(err) => {
          tracing::warn!(%address, ?err, "LE session error; will retry");
          let d = backoff;
          backoff = (backoff * 2).min(TRANSIENT_BACKOFF_MAX);
          tokio::select! {
            _ = cancel.cancelled() => return,
            _ = time::sleep(d) => {}
          }
        }
      }
    }
  }
}

#[cfg(target_os = "linux")]
type NotifyStream = std::pin::Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>;

#[cfg(target_os = "linux")]
fn pending_stream() -> NotifyStream {
  Box::pin(stream::pending())
}

#[derive(Debug, thiserror::Error)]
#[cfg(target_os = "linux")]
enum ServiceEnumError {
  #[error(transparent)]
  Zbus(#[from] zbus::Error),
  #[error(transparent)]
  ZbusFdo(#[from] zbus::fdo::Error),
  #[error(transparent)]
  Bluer(#[from] bluer::Error),
}

#[cfg(target_os = "linux")]
async fn enumerate_services(
  conn: &Connection,
  device: &Device,
  adapter_name: &str,
  address: Address,
) -> Result<Vec<Service>, ServiceEnumError> {
  let om = zbus::fdo::ObjectManagerProxy::builder(conn)
    .destination("org.bluez")?
    .path("/")?
    .build()
    .await?;
  let objects = om.get_managed_objects().await?;

  let prefix = format!(
    "/org/bluez/{adapter_name}/dev_{}/service",
    address.to_string().replace(':', "_")
  );

  let mut services = Vec::new();
  for (path, ifaces) in &objects {
    let Some(rest) = path.as_str().strip_prefix(&prefix) else {
      continue;
    };
    if rest.contains('/') || !ifaces.keys().any(|i| i.as_str() == GATT_SERVICE_INTERFACE) {
      continue;
    }
    if let Ok(id) = u16::from_str_radix(rest, 16) {
      services.push(device.service(id).await?);
    }
  }
  Ok(services)
}

#[cfg(target_os = "linux")]
async fn find_service(services: &[Service], uuid: Uuid) -> Option<Service> {
  for svc in services {
    if let Ok(u) = svc.uuid().await
      && u == uuid
    {
      return Some(svc.clone());
    }
  }
  None
}

#[cfg(target_os = "linux")]
fn le_acl_up(adapter: &Adapter, address: Address) -> bool {
  hci::le_acl_connected(adapter, address.into()).unwrap_or_else(|err| {
    tracing::warn!(%address, ?err, "LE ACL query failed; treating LE as down");
    false
  })
}

#[derive(Default)]
#[cfg(target_os = "linux")]
struct AclWatch {
  down: Option<(time::Instant, time::Instant)>,
}

#[cfg(target_os = "linux")]
impl AclWatch {
  fn note_down(&mut self, address: Address) {
    let now = time::Instant::now();
    match &mut self.down {
      None => {
        tracing::warn!(%address, "LE ACL down; polling for iOS to reconnect");
        self.down = Some((now, now));
      }
      Some((since, last_log)) => {
        if now.duration_since(*last_log) >= ACL_DOWN_HEARTBEAT {
          tracing::info!(%address, outage_s = now.duration_since(*since).as_secs(), "LE ACL still down");
          *last_log = now;
        }
      }
    }
  }

  fn note_up(&mut self, address: Address, services_resolved: bool) {
    if let Some((since, _)) = self.down.take() {
      tracing::info!(
        %address,
        outage_s = since.elapsed().as_secs(),
        services_resolved,
        "LE ACL restored; discovering"
      );
    }
  }
}

#[allow(clippy::too_many_arguments)]
#[cfg(target_os = "linux")]
async fn attempt(
  conn: &Connection,
  adapter: &Adapter,
  address: Address,
  bus: &WireEventBus,
  audio: &AudioManager,
  auth_reporter: &AuthStateReporter,
  invoke_rx: &mut mpsc::Receiver<(u32, u8)>,
  cancel: &CancellationToken,
  absent_logged: &mut bool,
  acl_watch: &mut AclWatch,
  backoff: &mut Duration,
) -> Result<LoopExit, bluer::Error> {
  if !le_acl_up(adapter, address) {
    acl_watch.note_down(address);
    return Ok(LoopExit::ConnectionLost);
  }
  let device = adapter.device(address.into())?;
  let services_resolved = device.is_services_resolved().await.unwrap_or(false);
  acl_watch.note_up(address, services_resolved);

  let services = match enumerate_services(conn, &device, adapter.name(), address).await {
    Ok(services) => services,
    Err(err) => {
      tracing::warn!(%address, ?err, "LE GATT enumeration failed; will retry");
      return Ok(LoopExit::ConnectionLost);
    }
  };

  let ancs_svc = find_service(&services, ancs::ANCS_SERVICE).await;
  let ams_svc = find_service(&services, ams::AMS_SERVICE).await;

  if ancs_svc.is_none() && ams_svc.is_none() {
    tracing::debug!(%address, "LE attempt: no ANCS/AMS services resolved yet");
    return Ok(LoopExit::ConnectionLost);
  }

  let (mut ancs, mut ns, mut ds) = match &ancs_svc {
    Some(svc) => match ancs::Ancs::subscribe(svc).await {
      Ok((a, streams)) => {
        tracing::info!(%address, "LE session: ANCS subscribed");
        *absent_logged = false;
        (Some(a), streams.notification_source, streams.data_source)
      }
      Err(err) => {
        tracing::warn!(%address, ?err, "ANCS subscribe failed; serving AMS only this session");
        (None, pending_stream(), pending_stream())
      }
    },
    None => (None, pending_stream(), pending_stream()),
  };

  let ancs_present = ancs.is_some();
  if !ancs_present {
    if !*absent_logged {
      tracing::warn!(
        %address,
        "ANCS unavailable (notifications disabled / unauthorized); serving AMS only. Run the companion \
         iOS app's \"Enable notifications\" flow to LE-pair the device and accept the ANCS prompt"
      );
      *absent_logged = true;
    }
    auth_reporter.report(AncsAuthState::Unauthorized).await;
  }

  let mut ams_eu = match &ams_svc {
    Some(svc) => match ams::subscribe(svc).await {
      Ok(s) => {
        tracing::info!(%address, "LE session: AMS subscribed");
        s
      }
      Err(err) => {
        tracing::warn!(%address, ?err, "AMS subscribe failed; volume state unavailable this session");
        pending_stream()
      }
    },
    None => {
      tracing::debug!(%address, "AMS service absent");
      pending_stream()
    }
  };

  *backoff = TRANSIENT_BACKOFF_INITIAL;

  let reprobe = time::sleep(ANCS_REPROBE_INTERVAL);
  tokio::pin!(reprobe);
  let probe = time::sleep(LE_PROBE_INTERVAL);
  tokio::pin!(probe);

  loop {
    if cancel.is_cancelled() {
      return Ok(LoopExit::Cancelled);
    }
    if let Some(a) = &mut ancs
      && a.pump_allowed()
      && !a.pump().await
      && !le_acl_up(adapter, address)
    {
      tracing::debug!(%address, "LE ACL gone after GNA write error; connection lost");
      return Ok(LoopExit::ConnectionLost);
    }

    tokio::select! {
      _ = cancel.cancelled() => return Ok(LoopExit::Cancelled),
      ns_item = ns.next() => match ns_item {
        Some(v) => { if let Some(a) = &mut ancs { a.on_notification_source(&v, bus).await; } }
        None => return Ok(LoopExit::ConnectionLost),
      },
      ds_item = ds.next() => match ds_item {
        Some(v) => {
          if let Some(a) = &mut ancs
            && a.on_data_source(&v, bus).await
          {
            auth_reporter.report(AncsAuthState::Authorized).await;
          }
        }
        None => return Ok(LoopExit::ConnectionLost),
      },
      cmd = invoke_rx.recv() => match cmd {
        Some((uid, action)) => { if let Some(a) = &ancs { a.on_invoke(uid, action).await; } }
        None => return Ok(LoopExit::Cancelled),
      },
      _ = time::sleep(ancs::ATTRIBUTE_FETCH_TIMEOUT), if ancs.as_ref().is_some_and(ancs::Ancs::has_in_flight) => {
        if let Some(a) = &mut ancs
          && a.on_fetch_timeout()
        {
          auth_reporter.report(AncsAuthState::Unauthorized).await;
        }
      }
      ams_item = ams_eu.next() => match ams_item {
        Some(v) => {
          if let Some(level) = ams::parse_volume(&v)
            && let Err(err) = audio.apply_ams(VolumeChanged { level, muted: false }).await
          {
            tracing::warn!(?err, "failed to apply AMS volume");
          }
        }
        None => return Ok(LoopExit::ConnectionLost),
      },
      _ = &mut reprobe, if !ancs_present => {
        tracing::debug!(%address, "ANCS re-probe interval elapsed; re-discovering");
        return Ok(LoopExit::ReprobeAncs);
      }
      _ = &mut probe => {
        if !le_acl_up(adapter, address) {
          tracing::debug!(%address, "LE ACL dropped; connection lost");
          return Ok(LoopExit::ConnectionLost);
        }
        probe.as_mut().reset(time::Instant::now() + LE_PROBE_INTERVAL);
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn phone(last: u8) -> Address {
    Address::new([0xAA, 0, 0, 0, 0, last])
  }

  #[test]
  fn reattaching_the_serving_phone_keeps_its_session() {
    let mut slot = SessionSlot::new();
    let mut starts = 0;
    assert_eq!(
      slot.attach(phone(1), || {
        starts += 1;
        starts
      }),
      Attached::Started { replaced: None }
    );
    assert_eq!(
      slot.attach(phone(1), || {
        starts += 1;
        starts
      }),
      Attached::Kept,
      "an iAP2 generation coming back up must not churn the LE session it never owned"
    );
    assert_eq!(starts, 1);
  }

  #[test]
  fn a_different_phone_replaces_the_session() {
    let mut slot = SessionSlot::new();
    slot.attach(phone(1), || 1);
    assert_eq!(slot.attach(phone(2), || 2), Attached::Started { replaced: Some(1) });
    assert_eq!(slot.address(), Some(phone(2)));
  }

  #[test]
  fn a_dead_session_is_evicted_so_the_next_attach_restarts_it() {
    let mut slot = SessionSlot::new();
    slot.attach(phone(1), || false);
    assert_eq!(slot.evict_if(|finished| *finished), None, "a live session stays");
    slot.attach(phone(2), || true);
    assert_eq!(slot.evict_if(|finished| *finished), Some(true));
    assert_eq!(slot.attach(phone(2), || false), Attached::Started { replaced: None });
  }

  #[test]
  fn detach_only_releases_the_matching_phone() {
    let mut slot = SessionSlot::new();
    slot.attach(phone(1), || 1);
    assert_eq!(slot.detach(phone(2)), None);
    assert_eq!(slot.detach(phone(1)), Some(1));
    assert_eq!(slot.address(), None);
  }
}
