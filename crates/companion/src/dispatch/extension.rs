use std::{
  collections::{HashMap, HashSet},
  sync::{Arc, Mutex},
};

use bridgething_gateway::Gateway;
use libbridgething::{
  ForwardRouted,
  gateway::{ExtensionsRunning, WebappActiveChanged, WebappConfigList},
};
use tokio::{sync::mpsc, task::JoinHandle};
use uuid::Uuid;

use crate::backend::{ExtensionConfigEntry, ExtensionHost, ExtensionHostInbox, ExtensionOutbound};

#[derive(Clone)]
struct Link {
  name: String,
  gateway: Gateway,
}

struct Active {
  webapp: Option<Uuid>,
  pushed: bool,
}

pub struct ExtensionDispatcher {
  host: Arc<dyn ExtensionHost>,
  links: Mutex<HashMap<String, Link>>,
  active: Mutex<HashMap<String, Active>>,
  running: Mutex<HashSet<Uuid>>,
  pump: Mutex<Option<JoinHandle<()>>>,
}

impl ExtensionDispatcher {
  pub fn new(host: Arc<dyn ExtensionHost>) -> Arc<Self> {
    Arc::new(Self {
      host,
      links: Mutex::new(HashMap::new()),
      active: Mutex::new(HashMap::new()),
      running: Mutex::new(HashSet::new()),
      pump: Mutex::new(None),
    })
  }

  pub fn start(self: &Arc<Self>) {
    let (inbox, rx) = ExtensionHostInbox::channel();
    let me = self.clone();
    let handle = tokio::spawn(async move { me.pump(rx).await });
    if let Some(previous) = self.pump.lock().unwrap().replace(handle) {
      previous.abort();
    }
    let host = self.host.clone();
    tokio::task::spawn_blocking(move || host.start(inbox));
  }

  pub async fn stop(&self) {
    if let Some(pump) = self.pump.lock().unwrap().take() {
      pump.abort();
    }
    let host = self.host.clone();
    let _ = tokio::task::spawn_blocking(move || host.stop()).await;
  }

  pub fn running(&self) -> Vec<Uuid> {
    self.running.lock().unwrap().iter().copied().collect()
  }

  pub async fn peer_connected(&self, device_id: &str, name: &str, gateway: &Gateway) {
    let active = match gateway.webapp().get_active().await {
      Ok(reply) => reply.id,
      Err(failure) => {
        tracing::warn!(%device_id, ?failure, "could not read the active webapp for the extension host");
        None
      }
    };
    self.seed_active(device_id, active);
    self.links.lock().unwrap().insert(
      device_id.to_owned(),
      Link {
        name: name.to_owned(),
        gateway: gateway.clone(),
      },
    );
    self.announce_device(device_id, name, gateway, None).await;
    self.publish_running(gateway).await;
  }

  fn seed_active(&self, device_id: &str, webapp: Option<Uuid>) {
    let mut held = self.active.lock().unwrap();
    if held.get(device_id).is_some_and(|held| held.pushed) {
      return;
    }
    held.insert(device_id.to_owned(), Active { webapp, pushed: false });
  }

  fn active_of(&self, device_id: &str) -> Option<Uuid> {
    self.active.lock().unwrap().get(device_id).and_then(|held| held.webapp)
  }

  pub fn peer_disconnected(&self, device_id: &str) {
    self.links.lock().unwrap().remove(device_id);
    self.active.lock().unwrap().remove(device_id);
    self.host.device_disconnected(device_id.to_owned());
  }

  pub fn active_changed(&self, device_id: &str, changed: &WebappActiveChanged) {
    let previous = self
      .active
      .lock()
      .unwrap()
      .insert(
        device_id.to_owned(),
        Active {
          webapp: changed.id,
          pushed: true,
        },
      )
      .and_then(|held| held.webapp);
    if previous == changed.id {
      return;
    }
    if let Some(previous) = previous {
      self
        .host
        .device_active(device_id.to_owned(), previous.to_string(), false);
    }
    if let Some(next) = changed.id {
      self.host.device_active(device_id.to_owned(), next.to_string(), true);
    }
  }

  pub fn deliver(&self, device_id: &str, routed: ForwardRouted) {
    self
      .host
      .deliver(device_id.to_owned(), routed.webapp.to_string(), routed.message.into());
  }

  pub fn config_changed(&self, device_id: &str, webapp: Uuid, key: &str, value: Option<String>) {
    self
      .host
      .config_changed(device_id.to_owned(), webapp.to_string(), key.to_owned(), value);
  }

  async fn pump(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<ExtensionOutbound>) {
    while let Some(outbound) = rx.recv().await {
      match outbound {
        ExtensionOutbound::SendToDevice {
          device,
          webapp,
          message,
        } => {
          let routed = ForwardRouted { webapp, message };
          for (device_id, link) in self.targets(device.as_deref(), webapp) {
            if let Err(failure) = link.gateway.forward().routed(routed.clone()).await {
              tracing::warn!(%device_id, ?failure, "an extension send did not reach the device");
            }
          }
        }
        ExtensionOutbound::RunningChanged { webapps } => {
          let started = {
            let next: HashSet<Uuid> = webapps.into_iter().collect();
            let mut running = self.running.lock().unwrap();
            let started: Vec<Uuid> = next.difference(&running).copied().collect();
            *running = next;
            started
          };
          for (device_id, link) in self.all_links() {
            if !started.is_empty() {
              self
                .announce_device(&device_id, &link.name, &link.gateway, Some(started.clone()))
                .await;
            }
            self.publish_running(&link.gateway).await;
          }
        }
      }
    }
  }

  async fn announce_device(&self, device_id: &str, name: &str, gateway: &Gateway, only: Option<Vec<Uuid>>) {
    let wanted = only.clone().unwrap_or_else(|| self.running());
    let config = self.collect_config(gateway, &wanted).await;
    if let Some(active) = self.active_of(device_id)
      && only.as_ref().is_none_or(|ids| ids.contains(&active))
    {
      self.host.device_active(device_id.to_owned(), active.to_string(), true);
    }
    self.host.device_connected(
      device_id.to_owned(),
      name.to_owned(),
      config,
      wanted.iter().map(Uuid::to_string).collect(),
    );
  }

  async fn publish_running(&self, gateway: &Gateway) {
    let webapps = self.running();
    if let Err(failure) = gateway
      .forward()
      .extensions_running(ExtensionsRunning { webapps })
      .await
    {
      tracing::warn!(?failure, "could not publish the running extension set");
    }
  }

  async fn collect_config(&self, gateway: &Gateway, webapps: &[Uuid]) -> Vec<ExtensionConfigEntry> {
    let mut out = Vec::new();
    for webapp in webapps.iter().copied() {
      match gateway.webapp().config_list(WebappConfigList { id: webapp }).await {
        Ok(reply) => out.extend(reply.entries.into_iter().map(|entry| ExtensionConfigEntry {
          webapp: webapp.to_string(),
          key: entry.key,
          value: entry.value,
        })),
        Err(failure) => tracing::warn!(%webapp, ?failure, "could not read config for an extension"),
      }
    }
    out
  }

  fn all_links(&self) -> Vec<(String, Link)> {
    self
      .links
      .lock()
      .unwrap()
      .iter()
      .map(|(device_id, link)| (device_id.clone(), link.clone()))
      .collect()
  }

  fn targets(&self, device: Option<&str>, webapp: Uuid) -> Vec<(String, Link)> {
    let links = self.links.lock().unwrap();
    match device {
      Some(device) => links
        .get_key_value(device)
        .map(|(id, link)| (id.clone(), link.clone()))
        .into_iter()
        .collect(),
      None => {
        let active = self.active.lock().unwrap();
        links
          .iter()
          .filter(|(device_id, _)| active.get(*device_id).and_then(|held| held.webapp) == Some(webapp))
          .map(|(device_id, link)| (device_id.clone(), link.clone()))
          .collect()
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use futures::{SinkExt as _, StreamExt as _};
  use libbridgething::{
    gateway::{
      BridgeToGatewayMsg, BridgeToGatewayMsgData, BridgeToGatewayWebappMsg, GatewayToBridgeForwardMsg,
      GatewayToBridgeMsgData, GatewayToBridgeWebappMsg, WebappActive, WebappConfigListReply,
    },
    protocol::{BridgeEndec, DecodedFrame},
    wire::{MsgMeta, ResponseMeta},
  };
  use tokio::sync::oneshot;
  use tokio_util::codec::Framed;

  use super::*;
  use crate::backend::{ExtensionConfigEntry, ExtensionMessage};

  const WEATHER: Uuid = Uuid::from_u128(0x2f0c_1a4b_0000_4000_8000_0000_0000_0001);
  const CLOCK: Uuid = Uuid::from_u128(0x2f0c_1a4b_0000_4000_8000_0000_0000_0002);
  const SETTLE: Duration = Duration::from_millis(400);

  struct Silent;

  impl ExtensionHost for Silent {
    fn start(&self, _inbox: Arc<ExtensionHostInbox>) {}
    fn stop(&self) {}
    fn deliver(&self, _device: String, _webapp: String, _message: ExtensionMessage) {}
    fn device_connected(
      &self,
      _device: String,
      _name: String,
      _config: Vec<ExtensionConfigEntry>,
      _webapps: Vec<String>,
    ) {
    }
    fn device_disconnected(&self, _device: String) {}
    fn device_active(&self, _device: String, _webapp: String, _active: bool) {}
    fn config_changed(&self, _device: String, _webapp: String, _key: String, _value: Option<String>) {}
  }

  fn switched(id: Option<Uuid>) -> WebappActiveChanged {
    WebappActiveChanged {
      id,
      name: None,
      art: None,
    }
  }

  #[test]
  fn a_pushed_active_webapp_outranks_a_get_active_reply_still_in_flight() {
    let dispatcher = ExtensionDispatcher::new(Arc::new(Silent));

    dispatcher.active_changed("sn-1", &switched(Some(CLOCK)));
    dispatcher.seed_active("sn-1", Some(WEATHER));

    assert_eq!(
      dispatcher.active_of("sn-1"),
      Some(CLOCK),
      "the user switched webapps while the link was coming up, so the reply that was already out is the stale one"
    );
  }

  #[test]
  fn a_get_active_reply_still_seeds_a_device_the_daemon_has_said_nothing_about() {
    let dispatcher = ExtensionDispatcher::new(Arc::new(Silent));

    dispatcher.seed_active("sn-1", Some(WEATHER));
    assert_eq!(dispatcher.active_of("sn-1"), Some(WEATHER));

    dispatcher.seed_active("sn-1", Some(CLOCK));
    assert_eq!(
      dispatcher.active_of("sn-1"),
      Some(CLOCK),
      "a second link-up reads the device again and nothing has overruled it"
    );
  }

  #[test]
  fn a_relinked_device_reads_its_active_webapp_again() {
    let dispatcher = ExtensionDispatcher::new(Arc::new(Silent));

    dispatcher.active_changed("sn-1", &switched(Some(CLOCK)));
    dispatcher.peer_disconnected("sn-1");
    dispatcher.seed_active("sn-1", Some(WEATHER));

    assert_eq!(
      dispatcher.active_of("sn-1"),
      Some(WEATHER),
      "what was active before the link died says nothing about what is active now"
    );
  }

  #[derive(Default, Clone)]
  struct Timeline(Arc<Mutex<Vec<String>>>);

  impl Timeline {
    fn note(&self, step: impl Into<String>) {
      self.0.lock().unwrap().push(step.into());
    }

    fn seen(&self) -> Vec<String> {
      self.0.lock().unwrap().clone()
    }

    fn connects(&self) -> usize {
      self.seen().iter().filter(|step| step.starts_with("connected")).count()
    }
  }

  struct Watching {
    timeline: Timeline,
    inbox: Mutex<Option<Arc<ExtensionHostInbox>>>,
  }

  impl Watching {
    fn new(timeline: Timeline) -> Arc<Self> {
      Arc::new(Self {
        timeline,
        inbox: Mutex::new(None),
      })
    }

    async fn inbox(&self) -> Arc<ExtensionHostInbox> {
      for _ in 0..200 {
        if let Some(inbox) = self.inbox.lock().unwrap().clone() {
          return inbox;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
      }
      panic!("the dispatcher never handed the host an inbox")
    }
  }

  impl ExtensionHost for Watching {
    fn start(&self, inbox: Arc<ExtensionHostInbox>) {
      *self.inbox.lock().unwrap() = Some(inbox);
    }
    fn stop(&self) {}
    fn deliver(&self, _device: String, _webapp: String, _message: ExtensionMessage) {}
    fn device_connected(
      &self,
      device: String,
      _name: String,
      _config: Vec<ExtensionConfigEntry>,
      webapps: Vec<String>,
    ) {
      self
        .timeline
        .note(format!("connected {device} for [{}]", webapps.join(" ")));
    }
    fn device_disconnected(&self, device: String) {
      self.timeline.note(format!("disconnected {device}"));
    }
    fn device_active(&self, device: String, webapp: String, active: bool) {
      self.timeline.note(format!("active {device} {webapp} {active}"));
    }
    fn config_changed(&self, _device: String, _webapp: String, _key: String, _value: Option<String>) {}
  }

  struct Daemon {
    asked: mpsc::UnboundedReceiver<()>,
    release: Option<oneshot::Sender<()>>,
  }

  fn answer(request: Uuid, data: BridgeToGatewayWebappMsg) -> BridgeToGatewayMsg {
    BridgeToGatewayMsg {
      id: Uuid::now_v7(),
      meta: MsgMeta::Response(ResponseMeta { request_id: request }),
      data: BridgeToGatewayMsgData::Webapp(data),
    }
  }

  fn fake_daemon(active: Option<Uuid>, timeline: Timeline) -> (Gateway, Daemon) {
    let (near, far) = tokio::io::duplex(64 * 1024);
    let gateway = Gateway::from_io(near);
    let (mut sink, mut stream) = Framed::new(far, BridgeEndec::default()).split();
    let (out, mut outbound) = mpsc::unbounded_channel::<BridgeToGatewayMsg>();
    tokio::spawn(async move {
      while let Some(msg) = outbound.recv().await {
        if sink.send(msg).await.is_err() {
          break;
        }
      }
    });

    let (asked, waiting) = mpsc::unbounded_channel();
    let (release, gate) = oneshot::channel::<()>();
    tokio::spawn(async move {
      let mut gate = Some(gate);
      while let Some(Ok(DecodedFrame::Frame(frame))) = stream.next().await {
        let msg = frame.msg;
        let request = msg.id;
        match msg.data {
          GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::GetActive) => {
            let _ = asked.send(());
            let out = out.clone();
            let gate = gate.take();
            tokio::spawn(async move {
              if let Some(gate) = gate {
                let _ = gate.await;
              }
              let _ = out.send(answer(
                request,
                BridgeToGatewayWebappMsg::Active(WebappActive { id: active, name: None }),
              ));
            });
          }
          GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::ConfigList(_)) => {
            let _ = out.send(answer(
              request,
              BridgeToGatewayWebappMsg::ConfigList(WebappConfigListReply { entries: Vec::new() }),
            ));
          }
          GatewayToBridgeMsgData::Forward(GatewayToBridgeForwardMsg::ExtensionsRunning(_)) => {
            timeline.note("running");
          }
          _ => {}
        }
      }
    });

    (
      gateway,
      Daemon {
        asked: waiting,
        release: Some(release),
      },
    )
  }

  async fn running_taken(dispatcher: &Arc<ExtensionDispatcher>, webapp: Uuid) {
    for _ in 0..200 {
      if dispatcher.running().contains(&webapp) {
        return;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the pump never took the running set");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn an_extension_that_starts_while_the_active_webapp_is_being_read_hears_the_device_once() {
    let timeline = Timeline::default();
    let host = Watching::new(timeline.clone());
    let dispatcher = ExtensionDispatcher::new(host.clone() as Arc<dyn ExtensionHost>);
    dispatcher.start();
    let inbox = host.inbox().await;

    let (gateway, mut daemon) = fake_daemon(Some(WEATHER), timeline.clone());
    let linking = {
      let dispatcher = Arc::clone(&dispatcher);
      let gateway = gateway.clone();
      tokio::spawn(async move { dispatcher.peer_connected("sn-1", "car thing", &gateway).await })
    };

    daemon
      .asked
      .recv()
      .await
      .expect("the link reads the device's active webapp");
    inbox.running_changed(vec![WEATHER.to_string()]);
    running_taken(&dispatcher, WEATHER).await;
    tokio::time::sleep(SETTLE).await;

    assert_eq!(
      timeline.connects(),
      0,
      "a device whose active webapp has not come back yet is announced as nobody's, and the announce \
       that carries the answer is then a second connect with no disconnect between them, got {:?}",
      timeline.seen()
    );

    daemon
      .release
      .take()
      .expect("the gate")
      .send(())
      .expect("the reply goes out");
    linking.await.expect("the link comes up");
    tokio::time::sleep(SETTLE).await;

    assert_eq!(
      timeline.connects(),
      1,
      "and the link-up announce is the only connect the host is ever given, got {:?}",
      timeline.seen()
    );
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn a_link_tells_the_host_the_active_webapp_before_the_device_and_the_daemon_last() {
    let timeline = Timeline::default();
    let host = Watching::new(timeline.clone());
    let dispatcher = ExtensionDispatcher::new(host.clone() as Arc<dyn ExtensionHost>);
    dispatcher.start();
    host.inbox().await.running_changed(vec![WEATHER.to_string()]);
    running_taken(&dispatcher, WEATHER).await;

    let (gateway, mut daemon) = fake_daemon(Some(WEATHER), timeline.clone());
    daemon.release = None;
    dispatcher.peer_connected("sn-1", "car thing", &gateway).await;
    tokio::time::sleep(SETTLE).await;

    assert_eq!(
      timeline.seen(),
      vec![
        format!("active sn-1 {WEATHER} true"),
        format!("connected sn-1 for [{WEATHER}]"),
        "running".to_owned(),
      ],
      "the flag rides the connect rather than a message after it, and forward only becomes available \
       on the daemon once the host has been told the device it will carry"
    );
  }
}
