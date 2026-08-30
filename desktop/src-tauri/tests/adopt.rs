use std::{
  io::Write as _,
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
  thread::ThreadId,
};

use bridgething_desktop::extensions::Extensions;
use libbridgething::ExtensionPermission;
use tracing::field::{Field, Visit};
use tracing_subscriber::{
  layer::{Context, Layer, SubscriberExt as _},
  util::SubscriberInitExt as _,
};
use uuid::Uuid;

const ADOPTED: &str = "a webapp brought a native extension";

#[derive(Default, Clone)]
struct Threads(Arc<Mutex<Vec<(String, ThreadId)>>>);

impl Threads {
  fn on(&self, message: &str) -> Vec<ThreadId> {
    self
      .0
      .lock()
      .unwrap()
      .iter()
      .filter(|(said, _)| said == message)
      .map(|(_, thread)| *thread)
      .collect()
  }
}

impl<S: tracing::Subscriber> Layer<S> for Threads {
  fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
    let mut said = Said(String::new());
    event.record(&mut said);
    self.0.lock().unwrap().push((said.0, std::thread::current().id()));
  }
}

struct Said(String);

impl Visit for Said {
  fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
    if field.name() == "message" {
      self.0.push_str(format!("{value:?}").trim_matches('"'));
    }
  }
}

fn bundle(dir: &Path, webapp: Uuid) -> PathBuf {
  let path = dir.join("bundle.zip");
  let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).expect("an archive"));
  let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
  zip.start_file("manifest.json", options).expect("a manifest entry");
  zip
    .write_all(
      format!(
        r#"{{"id":"{webapp}","name":"weather","version":"1.0.0","extension":{{"entry":"extension/desktop.mjs","permissions":["write"],"api":1}}}}"#
      )
      .as_bytes(),
    )
    .expect("the manifest body");
  zip.start_file("extension/desktop.mjs", options).expect("an entry");
  zip.write_all(b"export {}").expect("the entry body");
  zip.finish().expect("a finished archive");
  path
}

#[tokio::test(flavor = "current_thread")]
async fn a_local_install_unpacks_the_extension_off_the_worker_driving_the_links() {
  let seen = Threads::default();
  tracing_subscriber::registry().with(seen.clone()).init();

  let spool = tempfile::tempdir().expect("a scratch directory");
  let state_dir = spool.path().join("state");
  std::fs::create_dir_all(&state_dir).expect("the state directory");
  let webapp = Uuid::now_v7();
  let archive = bundle(spool.path(), webapp);

  let extensions = Extensions::init(&state_dir);
  extensions
    .adopt_off_worker("sn-1".to_owned(), archive, Some(vec![ExtensionPermission::Write(None)]))
    .await;

  let adopted = seen.on(ADOPTED);
  assert_eq!(adopted.len(), 1, "the bundle carried an extension and it was taken out");
  assert_ne!(
    adopted[0],
    std::thread::current().id(),
    "unzipping a bundle on the runtime worker stalls every link the shell is driving for the whole extraction"
  );
}
