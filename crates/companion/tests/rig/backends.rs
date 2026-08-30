use std::{
  path::PathBuf,
  sync::{Arc, Mutex},
};

use bridgething_companion::{
  api::{SessionEvent, SessionEventSink},
  backend::{HostClock, HostEnvironment, HttpDownloadSink, HttpRequest, HttpSink, HttpTransport, WsInbox, WsTransport},
};
use bridgething_delivery::bundle::fetch::{ArtifactFetch, DownloadRequest, FetchError};

pub struct RigHost;

impl HostEnvironment for RigHost {
  fn clock(&self) -> HostClock {
    HostClock {
      tz_iana: "UTC".into(),
      locale: "en-US".into(),
      unix_seconds: 1_700_000_000,
      utc_offset_minutes: 0,
      dst_offset_minutes: 0,
    }
  }
}

pub struct Offline;

impl HttpTransport for Offline {
  fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
    sink.fail("the rig has no network".into());
  }

  fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    sink.on_failed("the rig has no network".into());
  }
}

impl WsTransport for Offline {
  fn connect(&self, connect: bridgething_companion::backend::net::WsConnect, inbox: Arc<WsInbox>) {
    inbox.on_closed(connect.id, None, "the rig has no network".into());
  }
  fn send(&self, _id: String, _frame: bridgething_companion::backend::net::WsFrame) {}
  fn disconnect(&self, _id: String, _code: Option<u16>, _reason: Option<String>) {}
}

#[async_trait::async_trait]
impl ArtifactFetch for Offline {
  async fn text(&self, url: &str) -> Result<String, FetchError> {
    Err(FetchError::Transport(format!("the rig has no network ({url})")))
  }

  async fn download(&self, request: DownloadRequest) -> Result<PathBuf, FetchError> {
    Err(FetchError::Transport(format!(
      "the rig has no network ({})",
      request.url
    )))
  }
}

#[derive(Default)]
pub struct Heard(pub Mutex<Vec<SessionEvent>>);

impl SessionEventSink for Heard {
  fn on_event(&self, event: SessionEvent) {
    self.0.lock().unwrap().push(event);
  }
}
