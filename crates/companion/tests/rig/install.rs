use std::{
  collections::HashMap,
  path::Path,
  sync::{Arc, Mutex, mpsc},
  time::Duration,
};

use bridgething_companion::{
  api::WebappBundleSink,
  backend::{HttpDownloadSink, HttpRequest, HttpSink, HttpTransport},
};

#[derive(Default)]
pub struct Serving {
  bodies: Mutex<HashMap<String, Vec<u8>>>,
}

impl Serving {
  pub fn new() -> Arc<Self> {
    Arc::new(Self::default())
  }

  pub fn stage(&self, url: &str, body: Vec<u8>) {
    self.bodies.lock().unwrap().insert(url.to_owned(), body);
  }
}

impl HttpTransport for Serving {
  fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
    sink.fail("the rig serves downloads only".into());
  }

  fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    let Some(body) = self.bodies.lock().unwrap().get(&request.url).cloned() else {
      sink.on_failed(format!("nothing staged for {}", request.url));
      return;
    };
    sink.on_response(200, Vec::new(), Some(body.len() as u64));
    sink.on_chunk(body);
    sink.on_finished();
  }
}

#[derive(Default)]
pub struct RecordingSink(Mutex<Vec<(String, bool)>>);

impl RecordingSink {
  pub fn new() -> Arc<Self> {
    Arc::new(Self::default())
  }

  pub fn calls(&self) -> Vec<(String, bool)> {
    self.0.lock().unwrap().clone()
  }
}

impl WebappBundleSink for RecordingSink {
  fn installed(&self, bundle: String) {
    let present = Path::new(&bundle).is_file();
    self.0.lock().unwrap().push((bundle, present));
  }
}

const BLOCKED_GRACE: Duration = Duration::from_secs(5);

pub struct Gate {
  pub entered: mpsc::Receiver<()>,
  pub release: mpsc::Sender<()>,
}

pub struct BlockingSink {
  entered: mpsc::Sender<()>,
  release: Mutex<mpsc::Receiver<()>>,
  freed: Mutex<Vec<bool>>,
}

impl BlockingSink {
  pub fn new() -> (Arc<Self>, Gate) {
    let (entered, heard) = mpsc::channel();
    let (release, waiting) = mpsc::channel();
    (
      Arc::new(Self {
        entered,
        release: Mutex::new(waiting),
        freed: Mutex::new(Vec::new()),
      }),
      Gate {
        entered: heard,
        release,
      },
    )
  }

  pub fn freed(&self) -> Vec<bool> {
    self.freed.lock().unwrap().clone()
  }
}

impl WebappBundleSink for BlockingSink {
  fn installed(&self, _bundle: String) {
    let _ = self.entered.send(());
    let freed = self.release.lock().unwrap().recv_timeout(BLOCKED_GRACE).is_ok();
    self.freed.lock().unwrap().push(freed);
  }
}
