use std::{
  sync::{Arc, Mutex},
  time::Duration,
};

use bridgething_companion::backend::{ExtensionConfigEntry, ExtensionHost, ExtensionHostInbox, ExtensionMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCall {
  Started,
  Stopped,
  Delivered {
    device: String,
    webapp: String,
    message: ExtensionMessage,
  },
  DeviceConnected {
    device: String,
    name: String,
    config: Vec<ExtensionConfigEntry>,
    webapps: Vec<String>,
  },
  DeviceDisconnected {
    device: String,
  },
  DeviceActive {
    device: String,
    webapp: String,
    active: bool,
  },
  ConfigChanged {
    device: String,
    webapp: String,
    key: String,
    value: Option<String>,
  },
}

#[derive(Default)]
pub struct FakeExtensionHost {
  inbox: Mutex<Option<Arc<ExtensionHostInbox>>>,
  calls: Mutex<Vec<HostCall>>,
}

impl FakeExtensionHost {
  pub fn calls(&self) -> Vec<HostCall> {
    self.calls.lock().unwrap().clone()
  }

  pub async fn inbox(&self, within: Duration) -> Arc<ExtensionHostInbox> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
      if let Some(inbox) = self.inbox.lock().unwrap().clone() {
        return inbox;
      }
      assert!(
        tokio::time::Instant::now() < deadline,
        "the session never started the extension host"
      );
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
  }

  pub async fn await_call(&self, within: Duration, mut matches: impl FnMut(&HostCall) -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
      if self.calls.lock().unwrap().iter().any(&mut matches) {
        return true;
      }
      if tokio::time::Instant::now() >= deadline {
        return false;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
  }

  fn note(&self, call: HostCall) {
    self.calls.lock().unwrap().push(call);
  }
}

impl ExtensionHost for FakeExtensionHost {
  fn start(&self, inbox: Arc<ExtensionHostInbox>) {
    *self.inbox.lock().unwrap() = Some(inbox);
    self.note(HostCall::Started);
  }

  fn stop(&self) {
    self.note(HostCall::Stopped);
  }

  fn deliver(&self, device: String, webapp: String, message: ExtensionMessage) {
    self.note(HostCall::Delivered {
      device,
      webapp,
      message,
    });
  }

  fn device_connected(&self, device: String, name: String, config: Vec<ExtensionConfigEntry>, webapps: Vec<String>) {
    self.note(HostCall::DeviceConnected {
      device,
      name,
      config,
      webapps,
    });
  }

  fn device_disconnected(&self, device: String) {
    self.note(HostCall::DeviceDisconnected { device });
  }

  fn device_active(&self, device: String, webapp: String, active: bool) {
    self.note(HostCall::DeviceActive { device, webapp, active });
  }

  fn config_changed(&self, device: String, webapp: String, key: String, value: Option<String>) {
    self.note(HostCall::ConfigChanged {
      device,
      webapp,
      key,
      value,
    });
  }
}
