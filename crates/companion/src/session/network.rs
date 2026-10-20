use std::{sync::Arc, time::Duration};

use bridgething_gateway::{
  TransportError,
  transport::{SeamWsConnector, connect_seam_ws},
};
use uuid::Uuid;

use super::Session;
use crate::{
  api::CompanionError,
  backend::{ForeignWs, LinkDevice},
};

const REDIAL_FIRST: Duration = Duration::from_secs(1);
const REDIAL_CEILING: Duration = Duration::from_secs(30);

pub(super) struct NetworkLink {
  device: LinkDevice,
  url: String,
  claim: Uuid,
}

impl Session {
  pub async fn connect_network(self: &Arc<Self>, url: String, device: LinkDevice) -> Result<(), CompanionError> {
    let claim = Uuid::now_v7();
    self.networks.lock().unwrap().insert(
      device.id.clone(),
      NetworkLink {
        device: device.clone(),
        url: url.clone(),
        claim,
      },
    );
    match self.dial(&url).await {
      Ok(connector) => {
        self.raise_network(&device.id, claim, connector).await;
        Ok(())
      }
      Err(failure) => {
        self.redial(&device.id);
        Err(CompanionError::Device(format!("{url}: {failure}")))
      }
    }
  }

  pub(super) fn redial(self: &Arc<Self>, device_id: &str) {
    let Some(claim) = self.networks.lock().unwrap().get(device_id).map(|held| held.claim) else {
      return;
    };
    let session = self.clone();
    let device_id = device_id.to_owned();
    tokio::spawn(async move {
      let mut wait = REDIAL_FIRST;
      loop {
        tokio::time::sleep(wait).await;
        let Some(url) = session.held_url(&device_id, claim) else {
          return;
        };
        match session.dial(&url).await {
          Ok(connector) => {
            session.raise_network(&device_id, claim, connector).await;
            return;
          }
          Err(failure) => {
            tracing::debug!(%device_id, %url, %failure, ?wait, "the network daemon did not answer; dialing it again");
            wait = (wait * 2).min(REDIAL_CEILING);
          }
        }
      }
    });
  }

  async fn raise_network(self: &Arc<Self>, device_id: &str, claim: Uuid, connector: SeamWsConnector) {
    let _gate = self.link_gate.lock().await;
    let held = self
      .networks
      .lock()
      .unwrap()
      .get(device_id)
      .filter(|held| held.claim == claim)
      .map(|held| held.device.clone());
    if let Some(device) = held {
      self.raise(&device, connector).await;
    }
  }

  fn held_url(&self, device_id: &str, claim: Uuid) -> Option<String> {
    self
      .networks
      .lock()
      .unwrap()
      .get(device_id)
      .filter(|held| held.claim == claim)
      .map(|held| held.url.clone())
  }

  async fn dial(&self, url: &str) -> Result<SeamWsConnector, TransportError> {
    connect_seam_ws(Arc::new(ForeignWs::new(self.backends.ws.clone())), url).await
  }
}
