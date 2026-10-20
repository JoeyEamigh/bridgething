use std::{
  collections::{HashMap, HashSet},
  future::Future,
  sync::{Arc, Mutex},
  time::Duration,
};

use base64::Engine;
use bridgething_sdk_runtime::rt::{self, Instant};
use bytes::Bytes;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
  ota::stream::Artifact,
  seam::{Clock, SystemClock},
  transfer::FragmentSource,
};

pub const HANDLE_TTL: Duration = Duration::from_secs(30 * 60);
pub const HANDLE_SWEEP_PERIOD: Duration = Duration::from_secs(60);
pub const HANDLE_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const HANDLE_BUDGET_BYTES: u64 = 4 * HANDLE_MAX_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandleError {
  #[error("unknown_handle: {0} is not a live handle")]
  Unknown(String),
  #[error("too_large: a handle holds at most {HANDLE_MAX_BYTES} bytes")]
  TooLarge,
  #[error("busy: picked files waiting to upload already fill the {budget} byte handle budget")]
  Busy { budget: u64 },
  #[error("invalid: the chunk is not base64: {0}")]
  Encoding(String),
  #[error("io: {0}")]
  Io(String),
}

pub trait HandleStore: Send + Sync + 'static {
  fn create(&self, handle: &str) -> Result<(), HandleError>;
  fn append(&self, handle: &str, bytes: &[u8]) -> Result<(), HandleError>;
  fn artifact(&self, handle: &str) -> Result<Arc<dyn Artifact>, HandleError>;
  fn remove(&self, handle: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldBlob {
  pub mime: String,
  pub size: u64,
}

type PutTarget = (Uuid, String);

struct Held {
  blob: HeldBlob,
  touched: Instant,
  pins: usize,
  stored: bool,
  retrying: HashSet<PutTarget>,
}

impl Held {
  fn spent(&self) -> bool {
    self.stored && self.retrying.is_empty()
  }

  fn takes(&self, target: Option<&PutTarget>) -> bool {
    match target {
      Some(target) if self.stored => self.retrying.contains(target),
      _ => !self.spent(),
    }
  }
}

struct Registry<S> {
  store: S,
  clock: Arc<dyn Clock>,
  ttl: Duration,
  budget: u64,
  held: Mutex<HashMap<String, Held>>,
}

impl<S: HandleStore> Registry<S> {
  fn sweep(&self) {
    let now = self.clock.now();
    let mut held = self.held.lock().unwrap();
    let expired: Vec<String> = held
      .iter()
      .filter(|(_, entry)| entry.pins == 0 && now.saturating_duration_since(entry.touched) >= self.ttl)
      .map(|(handle, _)| handle.clone())
      .collect();
    for handle in expired {
      held.remove(&handle);
      self.store.remove(&handle);
    }
  }

  fn fits(&self, held: &HashMap<String, Held>, incoming: u64) -> Result<(), HandleError> {
    let total: u64 = held.values().map(|entry| entry.blob.size).sum();
    if total + incoming > self.budget {
      return Err(HandleError::Busy { budget: self.budget });
    }
    Ok(())
  }

  fn settle(&self, handle: &str, outcome: impl FnOnce(&mut Held)) {
    let mut held = self.held.lock().unwrap();
    let Some(entry) = held.get_mut(handle) else {
      return;
    };
    outcome(entry);
    entry.pins -= 1;
    entry.touched = self.clock.now();
    if entry.spent() && entry.pins == 0 {
      held.remove(handle);
      self.store.remove(handle);
    }
  }
}

trait Pins: Send + Sync {
  fn unpin(&self, handle: &str);
  fn end(&self, handle: &str, target: &PutTarget, stored: bool);
}

impl<S: HandleStore> Pins for Registry<S> {
  fn unpin(&self, handle: &str) {
    self.settle(handle, |_| {});
  }

  fn end(&self, handle: &str, target: &PutTarget, stored: bool) {
    self.settle(handle, |entry| {
      if stored {
        entry.retrying.remove(target);
        entry.stored = true;
      } else {
        entry.retrying.insert(target.clone());
      }
    });
  }
}

struct HandlePin {
  pins: Arc<dyn Pins>,
  handle: String,
}

impl Drop for HandlePin {
  fn drop(&mut self) {
    self.pins.unpin(&self.handle);
  }
}

pub struct HandleLease {
  pins: Arc<dyn Pins>,
  handle: String,
  target: PutTarget,
  stored: bool,
}

impl HandleLease {
  pub fn stored(mut self) {
    self.stored = true;
  }
}

impl Drop for HandleLease {
  fn drop(&mut self) {
    self.pins.end(&self.handle, &self.target, self.stored);
  }
}

struct PinnedArtifact {
  artifact: Arc<dyn Artifact>,
  _pin: HandlePin,
}

impl FragmentSource for PinnedArtifact {
  fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
    self.artifact.read_at(offset, buf)
  }
}

impl Artifact for PinnedArtifact {
  fn size(&self) -> Result<u64, String> {
    self.artifact.size()
  }

  fn sha256(&self) -> Result<String, String> {
    self.artifact.sha256()
  }
}

pub struct BlobHandles<S> {
  registry: Arc<Registry<S>>,
}

impl<S: HandleStore> BlobHandles<S> {
  pub fn new(store: S) -> Self {
    Self::with_limits(store, Arc::new(SystemClock), HANDLE_TTL, HANDLE_BUDGET_BYTES)
  }

  pub fn with_limits(store: S, clock: Arc<dyn Clock>, ttl: Duration, budget: u64) -> Self {
    Self {
      registry: Arc::new(Registry {
        store,
        clock,
        ttl,
        budget,
        held: Mutex::new(HashMap::new()),
      }),
    }
  }

  pub fn store(&self) -> &S {
    &self.registry.store
  }

  pub fn sweeper(&self) -> impl Future<Output = ()> + Send + 'static {
    let registry = Arc::downgrade(&self.registry);
    async move {
      loop {
        rt::sleep(HANDLE_SWEEP_PERIOD).await;
        let Some(registry) = registry.upgrade() else {
          return;
        };
        registry.sweep();
      }
    }
  }

  pub fn create(&self, mime: String) -> Result<String, HandleError> {
    let (handle, _pin) = self.reserve(HeldBlob { mime, size: 0 })?;
    if let Err(error) = self.store().create(&handle) {
      self.forget(&handle);
      return Err(error);
    }
    Ok(handle)
  }

  pub fn append(&self, handle: &str, bytes: &[u8]) -> Result<u64, HandleError> {
    self.registry.sweep();
    let mut held = self.registry.held.lock().unwrap();
    let size = live(&held, handle, None)?.blob.size;
    let grown = size + bytes.len() as u64;
    if grown > HANDLE_MAX_BYTES {
      return Err(HandleError::TooLarge);
    }
    self.registry.fits(&held, bytes.len() as u64)?;
    self.store().append(handle, bytes)?;
    if let Some(entry) = held.get_mut(handle) {
      entry.blob.size = grown;
      entry.touched = self.registry.clock.now();
    }
    Ok(grown)
  }

  pub fn append_base64(&self, handle: &str, chunk: &str) -> Result<u64, HandleError> {
    let bytes = base64::engine::general_purpose::STANDARD
      .decode(chunk.as_bytes())
      .map_err(|error| HandleError::Encoding(error.to_string()))?;
    self.append(handle, &bytes)
  }

  pub fn resolve(&self, handle: &str) -> Result<(HeldBlob, Arc<dyn Artifact>), HandleError> {
    let (blob, pin) = self.pin(handle)?;
    let artifact = self.store().artifact(handle)?;
    Ok((blob, Arc::new(PinnedArtifact { artifact, _pin: pin })))
  }

  pub fn lease(
    &self,
    handle: &str,
    webapp: Uuid,
    key: &str,
  ) -> Result<(HeldBlob, Arc<dyn Artifact>, HandleLease), HandleError> {
    let target = (webapp, key.to_owned());
    self.hold(handle, Some(&target))?;
    let lease = HandleLease {
      pins: self.registry.clone(),
      handle: handle.to_owned(),
      target,
      stored: false,
    };
    let (blob, artifact) = self.resolve(handle)?;
    Ok((blob, artifact, lease))
  }

  fn pin(&self, handle: &str) -> Result<(HeldBlob, HandlePin), HandleError> {
    let blob = self.hold(handle, None)?;
    Ok((blob, self.pinned(handle)))
  }

  fn hold(&self, handle: &str, target: Option<&PutTarget>) -> Result<HeldBlob, HandleError> {
    self.registry.sweep();
    let mut held = self.registry.held.lock().unwrap();
    live(&held, handle, target)?;
    let entry = held.get_mut(handle).expect("checked live above");
    entry.pins += 1;
    entry.touched = self.registry.clock.now();
    Ok(entry.blob.clone())
  }

  fn pinned(&self, handle: &str) -> HandlePin {
    HandlePin {
      pins: self.registry.clone(),
      handle: handle.to_owned(),
    }
  }

  fn reserve(&self, blob: HeldBlob) -> Result<(String, HandlePin), HandleError> {
    self.registry.sweep();
    let handle = uuid::Uuid::now_v7().to_string();
    let mut held = self.registry.held.lock().unwrap();
    self.registry.fits(&held, blob.size)?;
    held.insert(
      handle.clone(),
      Held {
        blob,
        touched: self.registry.clock.now(),
        pins: 1,
        stored: false,
        retrying: HashSet::new(),
      },
    );
    let pin = self.pinned(&handle);
    Ok((handle, pin))
  }

  pub fn forget(&self, handle: &str) {
    self.registry.held.lock().unwrap().remove(handle);
    self.store().remove(handle);
  }
}

fn live<'a>(
  held: &'a HashMap<String, Held>,
  handle: &str,
  target: Option<&PutTarget>,
) -> Result<&'a Held, HandleError> {
  held
    .get(handle)
    .filter(|entry| entry.takes(target))
    .ok_or_else(|| HandleError::Unknown(handle.to_owned()))
}

#[derive(Default, Clone)]
struct Segments {
  chunks: Vec<Bytes>,
  ends: Vec<u64>,
}

impl Segments {
  fn len(&self) -> u64 {
    self.ends.last().copied().unwrap_or(0)
  }

  fn push(&mut self, bytes: &[u8]) {
    if bytes.is_empty() {
      return;
    }
    let end = self.len() + bytes.len() as u64;
    self.chunks.push(Bytes::copy_from_slice(bytes));
    self.ends.push(end);
  }
}

impl FragmentSource for Segments {
  fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
    let mut at = self.ends.partition_point(|&end| end <= offset);
    let mut offset = offset;
    let mut filled = 0;
    while filled < buf.len() && at < self.chunks.len() {
      let start = self.ends[at] - self.chunks[at].len() as u64;
      let within = &self.chunks[at][(offset - start) as usize..];
      let take = within.len().min(buf.len() - filled);
      buf[filled..filled + take].copy_from_slice(&within[..take]);
      filled += take;
      offset += take as u64;
      at += 1;
    }
    Ok(filled)
  }
}

impl Artifact for Segments {
  fn size(&self) -> Result<u64, String> {
    Ok(self.len())
  }

  fn sha256(&self) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for chunk in &self.chunks {
      hasher.update(chunk);
    }
    Ok(hex::encode(hasher.finalize()))
  }
}

#[derive(Default)]
pub struct MemoryHandleStore {
  held: Mutex<HashMap<String, Arc<Segments>>>,
}

impl HandleStore for MemoryHandleStore {
  fn create(&self, handle: &str) -> Result<(), HandleError> {
    self.held.lock().unwrap().insert(handle.to_owned(), Arc::default());
    Ok(())
  }

  fn append(&self, handle: &str, bytes: &[u8]) -> Result<(), HandleError> {
    let mut held = self.held.lock().unwrap();
    let segments = held
      .get_mut(handle)
      .ok_or_else(|| HandleError::Unknown(handle.to_owned()))?;
    Arc::make_mut(segments).push(bytes);
    Ok(())
  }

  fn artifact(&self, handle: &str) -> Result<Arc<dyn Artifact>, HandleError> {
    let segments = self
      .held
      .lock()
      .unwrap()
      .get(handle)
      .cloned()
      .ok_or_else(|| HandleError::Unknown(handle.to_owned()))?;
    Ok(segments)
  }

  fn remove(&self, handle: &str) {
    self.held.lock().unwrap().remove(handle);
  }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs::FsHandleStore;

#[cfg(not(target_arch = "wasm32"))]
mod fs {
  use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
  };

  use super::{BlobHandles, HANDLE_MAX_BYTES, HandleError, HandleStore, HeldBlob, live};
  use crate::ota::stream::{Artifact, FileSource};

  pub struct FsHandleStore {
    dir: PathBuf,
  }

  impl FsHandleStore {
    pub fn new(dir: PathBuf) -> Self {
      let _ = std::fs::remove_dir_all(&dir);
      Self { dir }
    }

    pub fn dir(&self) -> &Path {
      &self.dir
    }

    fn path(&self, handle: &str) -> Result<PathBuf, HandleError> {
      std::fs::create_dir_all(&self.dir).map_err(io)?;
      Ok(self.dir.join(handle))
    }
  }

  fn io(error: std::io::Error) -> HandleError {
    HandleError::Io(error.to_string())
  }

  #[cfg(unix)]
  fn owner_writable(permissions: &mut std::fs::Permissions) {
    std::os::unix::fs::PermissionsExt::set_mode(permissions, 0o600);
  }

  #[cfg(not(unix))]
  #[allow(clippy::permissions_set_readonly_false)]
  fn owner_writable(permissions: &mut std::fs::Permissions) {
    permissions.set_readonly(false);
  }

  fn copy_owned(source: &Path, held: &Path) -> std::io::Result<()> {
    std::fs::copy(source, held)?;
    let mut permissions = std::fs::metadata(held)?.permissions();
    owner_writable(&mut permissions);
    std::fs::set_permissions(held, permissions)
  }

  impl HandleStore for FsHandleStore {
    fn create(&self, handle: &str) -> Result<(), HandleError> {
      File::create(self.path(handle)?).map_err(io)?;
      Ok(())
    }

    fn append(&self, handle: &str, bytes: &[u8]) -> Result<(), HandleError> {
      OpenOptions::new()
        .append(true)
        .open(self.dir.join(handle))
        .and_then(|mut file| file.write_all(bytes))
        .map_err(io)
    }

    fn artifact(&self, handle: &str) -> Result<Arc<dyn Artifact>, HandleError> {
      Ok(Arc::new(FileSource::open(self.dir.join(handle))))
    }

    fn remove(&self, handle: &str) {
      let _ = std::fs::remove_file(self.dir.join(handle));
    }
  }

  impl BlobHandles<FsHandleStore> {
    pub fn adopt(&self, path: &Path, mime: String) -> Result<(String, HeldBlob), HandleError> {
      let adopted = self.land(path, mime, |scratch, held| std::fs::rename(scratch, held));
      if adopted.is_err() {
        let _ = std::fs::remove_file(path);
      }
      adopted
    }

    pub fn import(&self, source: &Path, mime: String) -> Result<(String, HeldBlob), HandleError> {
      self.land(source, mime, copy_owned)
    }

    fn land(
      &self,
      source: &Path,
      mime: String,
      place: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<(String, HeldBlob), HandleError> {
      let size = std::fs::metadata(source).map_err(io)?.len();
      if size > HANDLE_MAX_BYTES {
        return Err(HandleError::TooLarge);
      }
      let (handle, _pin) = self.reserve(HeldBlob { mime, size })?;
      match self.placed(&handle, source, place) {
        Ok(blob) => Ok((handle, blob)),
        Err(error) => {
          self.forget(&handle);
          Err(error)
        }
      }
    }

    fn placed(
      &self,
      handle: &str,
      source: &Path,
      place: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<HeldBlob, HandleError> {
      let held = self.store().path(handle)?;
      place(source, &held).map_err(io)?;
      let size = std::fs::metadata(&held).map_err(io)?.len();
      if size > HANDLE_MAX_BYTES {
        return Err(HandleError::TooLarge);
      }
      let mut registered = self.registry.held.lock().unwrap();
      live(&registered, handle, None)?;
      let entry = registered.get_mut(handle).expect("checked live above");
      entry.blob.size = size;
      Ok(entry.blob.clone())
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::transfer::fixture::TestClock;

  const MINUTE: Duration = Duration::from_secs(60);
  const WEBAPP: Uuid = Uuid::from_u128(1);

  struct Rig<S> {
    handles: BlobHandles<S>,
    clock: Arc<TestClock>,
  }

  fn memory() -> Rig<MemoryHandleStore> {
    memory_within(HANDLE_BUDGET_BYTES)
  }

  fn memory_within(budget: u64) -> Rig<MemoryHandleStore> {
    let clock = TestClock::new();
    Rig {
      handles: BlobHandles::with_limits(MemoryHandleStore::default(), clock.clone(), HANDLE_TTL, budget),
      clock,
    }
  }

  fn on_disk(dir: &std::path::Path, budget: u64) -> Rig<FsHandleStore> {
    let clock = TestClock::new();
    Rig {
      handles: BlobHandles::with_limits(
        FsHandleStore::new(dir.join("handles")),
        clock.clone(),
        HANDLE_TTL,
        budget,
      ),
      clock,
    }
  }

  fn filled<S: HandleStore>(rig: &Rig<S>, bytes: &[u8]) -> String {
    let handle = rig.handles.create("application/octet-stream".into()).unwrap();
    rig.handles.append(&handle, bytes).unwrap();
    handle
  }

  fn read(artifact: &Arc<dyn Artifact>) -> Vec<u8> {
    let mut buf = vec![0u8; artifact.size().unwrap() as usize];
    artifact.read_at(0, &mut buf).unwrap();
    buf
  }

  fn live<S: HandleStore>(rig: &Rig<S>, handle: &str) -> bool {
    rig.handles.resolve(handle).is_ok()
  }

  fn holds(rig: &Rig<MemoryHandleStore>, handle: &str) -> bool {
    rig.handles.store().held.lock().unwrap().contains_key(handle)
  }

  #[test]
  fn appended_chunks_land_in_order() {
    let rig = memory();
    let handle = rig.handles.create("image/png".into()).unwrap();
    assert_eq!(rig.handles.append(&handle, b"abc").unwrap(), 3);
    assert_eq!(rig.handles.append_base64(&handle, "ZGVm").unwrap(), 6);

    let (blob, artifact) = rig.handles.resolve(&handle).unwrap();
    assert_eq!(
      blob,
      HeldBlob {
        mime: "image/png".into(),
        size: 6
      }
    );
    assert_eq!(read(&artifact), b"abcdef");
  }

  #[test]
  fn a_chunk_that_is_not_base64_is_refused_and_the_handle_keeps_what_it_had() {
    let rig = memory();
    let handle = filled(&rig, b"kept");

    assert!(matches!(
      rig.handles.append_base64(&handle, "not base64!"),
      Err(HandleError::Encoding(_))
    ));
    assert_eq!(read(&rig.handles.resolve(&handle).unwrap().1), b"kept");
  }

  #[test]
  fn a_handle_refuses_bytes_past_the_device_cap_and_keeps_what_it_had() {
    let rig = memory();
    let handle = filled(&rig, &[7u8; 1024]);

    assert!(matches!(
      rig.handles.append(&handle, &vec![0u8; HANDLE_MAX_BYTES as usize]),
      Err(HandleError::TooLarge)
    ));
    assert_eq!(rig.handles.resolve(&handle).unwrap().0.size, 1024);
  }

  #[test]
  fn an_idle_handle_expires_and_its_bytes_go_with_it() {
    let rig = memory();
    let handle = filled(&rig, b"idle");

    rig.clock.advance(HANDLE_TTL - MINUTE);
    assert!(live(&rig, &handle), "a handle inside its idle window is live");

    rig.clock.advance(HANDLE_TTL);
    assert_eq!(
      rig.handles.resolve(&handle).err(),
      Some(HandleError::Unknown(handle.clone()))
    );
    assert!(rig.handles.store().held.lock().unwrap().is_empty());
  }

  #[test]
  fn a_handle_a_put_is_reading_outlives_its_idle_window() {
    let rig = memory();
    let handle = filled(&rig, b"a long upload");
    let (_, uploading) = rig.handles.resolve(&handle).unwrap();

    rig.clock.advance(HANDLE_TTL * 3);
    let other = filled(&rig, b"another pick");

    assert_eq!(read(&uploading), b"a long upload");
    assert!(
      live(&rig, &handle),
      "a put running past the idle window keeps its handle"
    );
    assert!(live(&rig, &other));
  }

  #[test]
  fn the_idle_window_restarts_when_the_put_ends() {
    let rig = memory();
    let handle = filled(&rig, b"retry me");
    let (_, uploading) = rig.handles.resolve(&handle).unwrap();
    rig.clock.advance(HANDLE_TTL * 3);
    drop(uploading);

    rig.clock.advance(HANDLE_TTL - MINUTE);
    let (_, retry) = rig
      .handles
      .resolve(&handle)
      .expect("a put that failed after a long upload retries with the same handle");
    assert_eq!(read(&retry), b"retry me");
    drop(retry);

    rig.clock.advance(HANDLE_TTL);
    assert!(
      !live(&rig, &handle),
      "once no put holds it the handle idles out as usual"
    );
  }

  #[test]
  fn a_pick_past_the_budget_is_refused_and_every_pick_not_yet_uploaded_survives() {
    let rig = memory_within(300);
    let picks: Vec<String> = (1..=3).map(|n| filled(&rig, &[n; 100])).collect();
    rig.clock.advance(MINUTE);

    let incoming = rig.handles.create("image/png".into()).unwrap();
    let refused = rig.handles.append(&incoming, &[4u8; 100]).unwrap_err();

    assert_eq!(refused, HandleError::Busy { budget: 300 });
    assert!(refused.to_string().starts_with("busy:"), "got {refused}");
    for (n, pick) in picks.iter().enumerate() {
      assert_eq!(read(&rig.handles.resolve(pick).unwrap().1), [n as u8 + 1; 100]);
    }
  }

  #[test]
  fn a_stored_put_releases_its_handle_and_frees_its_bytes() {
    let rig = memory_within(200);
    let uploaded = filled(&rig, &[1u8; 100]);
    let waiting = filled(&rig, &[2u8; 100]);
    let (_, artifact, lease) = rig.handles.lease(&uploaded, WEBAPP, "bg").unwrap();
    drop(artifact);

    lease.stored();

    assert_eq!(
      rig.handles.resolve(&uploaded).err(),
      Some(HandleError::Unknown(uploaded.clone()))
    );
    assert!(!holds(&rig, &uploaded));
    assert!(live(&rig, &waiting));
    let next = rig.handles.create("image/png".into()).unwrap();
    rig
      .handles
      .append(&next, &[3u8; 100])
      .expect("the released bytes make room");
  }

  #[test]
  fn a_stored_handle_another_put_still_reads_goes_once_that_put_ends() {
    let rig = memory();
    let handle = filled(&rig, b"two keys");
    let (_, other_put) = rig.handles.resolve(&handle).unwrap();
    let (_, artifact, lease) = rig.handles.lease(&handle, WEBAPP, "bg").unwrap();
    drop(artifact);

    lease.stored();

    assert!(!live(&rig, &handle), "a stored handle takes no new puts");
    assert_eq!(read(&other_put), b"two keys");
    assert!(holds(&rig, &handle));
    drop(other_put);
    assert!(!holds(&rig, &handle));
  }

  #[test]
  fn a_failed_put_leaves_its_handle_for_the_retry() {
    let rig = memory();
    let handle = filled(&rig, b"retry me");

    drop(rig.handles.lease(&handle, WEBAPP, "bg").unwrap());

    assert_eq!(read(&rig.handles.resolve(&handle).unwrap().1), b"retry me");
  }

  #[test]
  fn a_retry_that_stores_uses_the_handle_up() {
    let rig = memory();
    let handle = filled(&rig, b"retry me");
    drop(rig.handles.lease(&handle, WEBAPP, "bg").unwrap());

    rig.handles.lease(&handle, WEBAPP, "bg").unwrap().2.stored();

    assert!(!live(&rig, &handle));
    assert!(!holds(&rig, &handle));
  }

  #[test]
  fn a_put_that_fails_after_another_key_stored_the_handle_can_still_retry() {
    let rig = memory();
    let handle = filled(&rig, b"shared pick");
    let (_, _, bg) = rig.handles.lease(&handle, WEBAPP, "bg").unwrap();
    let (_, _, thumb) = rig.handles.lease(&handle, WEBAPP, "thumb").unwrap();

    bg.stored();
    assert!(
      !live(&rig, &handle),
      "a stored pick takes no new puts while one still runs"
    );
    drop(thumb);

    let (_, retry, lease) = rig
      .handles
      .lease(&handle, WEBAPP, "thumb")
      .expect("the put that failed retries with its handle");
    assert_eq!(read(&retry), b"shared pick");
    drop(retry);
    lease.stored();
    assert!(!holds(&rig, &handle), "the retry that stores uses the handle up");
  }

  #[test]
  fn a_put_that_failed_before_another_key_stored_the_handle_can_still_retry() {
    let rig = memory();
    let handle = filled(&rig, b"shared pick");
    let (_, _, bg) = rig.handles.lease(&handle, WEBAPP, "bg").unwrap();
    let (_, _, thumb) = rig.handles.lease(&handle, WEBAPP, "thumb").unwrap();

    drop(thumb);
    bg.stored();

    assert!(live(&rig, &handle), "the failed put still owes its retry");
    rig.handles.lease(&handle, WEBAPP, "thumb").unwrap().2.stored();
    assert!(!holds(&rig, &handle));
  }

  #[test]
  fn a_handle_owed_a_retry_takes_only_that_retry() {
    let rig = memory();
    let handle = filled(&rig, b"shared pick");
    let (_, _, bg) = rig.handles.lease(&handle, WEBAPP, "bg").unwrap();
    let (_, _, thumb) = rig.handles.lease(&handle, WEBAPP, "thumb").unwrap();
    drop(thumb);
    bg.stored();

    assert_eq!(
      rig.handles.lease(&handle, WEBAPP, "banner").err(),
      Some(HandleError::Unknown(handle.clone())),
      "a pick another put already stored is used up for every key but the retry"
    );
    assert!(rig.handles.lease(&handle, WEBAPP, "thumb").is_ok());
  }

  #[test]
  fn a_handle_every_put_stored_goes_when_the_last_put_ends() {
    let rig = memory();
    let handle = filled(&rig, b"shared pick");
    let (_, _, bg) = rig.handles.lease(&handle, WEBAPP, "bg").unwrap();
    let (_, _, thumb) = rig.handles.lease(&handle, WEBAPP, "thumb").unwrap();

    bg.stored();
    assert!(holds(&rig, &handle), "the put still running keeps the bytes");
    thumb.stored();

    assert!(!holds(&rig, &handle));
  }

  #[test]
  fn a_failure_for_one_webapp_is_not_settled_by_a_store_for_another() {
    let rig = memory();
    let handle = filled(&rig, b"shared pick");
    let other = Uuid::from_u128(2);
    let (_, _, mine) = rig.handles.lease(&handle, WEBAPP, "bg").unwrap();
    let (_, _, theirs) = rig.handles.lease(&handle, other, "bg").unwrap();

    drop(mine);
    theirs.stored();

    assert!(
      live(&rig, &handle),
      "the put that failed keeps its handle for the retry"
    );
  }

  #[test]
  fn a_handle_owed_a_retry_still_idles_out() {
    let rig = memory();
    let handle = filled(&rig, b"shared pick");
    let (_, _, bg) = rig.handles.lease(&handle, WEBAPP, "bg").unwrap();
    let (_, _, thumb) = rig.handles.lease(&handle, WEBAPP, "thumb").unwrap();
    bg.stored();
    drop(thumb);

    rig.clock.advance(HANDLE_TTL);

    assert!(!live(&rig, &handle));
    assert!(!holds(&rig, &handle));
  }

  #[tokio::test(start_paused = true)]
  async fn an_idle_handle_expires_on_the_sweep_timer_without_another_handle_call() {
    let rig = memory();
    let handle = filled(&rig, b"forgotten");
    tokio::spawn(rig.handles.sweeper());

    rig.clock.advance(HANDLE_TTL);
    tokio::time::sleep(HANDLE_SWEEP_PERIOD * 2).await;

    assert!(rig.handles.store().held.lock().unwrap().is_empty());
    assert!(rig.handles.registry.held.lock().unwrap().get(&handle).is_none());
  }

  #[tokio::test(start_paused = true)]
  async fn the_sweep_timer_stops_with_its_handles() {
    let rig = memory();
    let sweeping = tokio::spawn(rig.handles.sweeper());

    drop(rig);
    tokio::time::sleep(HANDLE_SWEEP_PERIOD * 2).await;

    assert!(sweeping.is_finished());
  }

  #[test]
  fn a_handle_built_from_many_chunks_allocates_only_its_bytes() {
    let rig = memory();
    let handle = rig.handles.create("application/octet-stream".into()).unwrap();
    for n in 0..64u8 {
      rig.handles.append(&handle, &[n; 1000]).unwrap();
    }
    let expected: Vec<u8> = (0..64u8).flat_map(|n| [n; 1000]).collect();
    assert_eq!(read(&rig.handles.resolve(&handle).unwrap().1), expected);
    assert_eq!(
      rig.handles.store().artifact(&handle).unwrap().sha256().unwrap(),
      crate::bundle::fetch::sha256_hex(&expected)
    );

    let segments = rig.handles.store().held.lock().unwrap().remove(&handle).unwrap();
    let segments = Arc::try_unwrap(segments).ok().expect("nothing else holds the bytes");
    let allocated: usize = segments
      .chunks
      .into_iter()
      .map(|chunk| chunk.try_into_mut().expect("each chunk owns its allocation").capacity())
      .sum();
    assert_eq!(allocated, expected.len());
  }

  #[test]
  fn a_budget_full_of_uploads_refuses_more_bytes_and_drops_nothing() {
    let rig = memory_within(200);
    let first = filled(&rig, &[1u8; 100]);
    let second = filled(&rig, &[2u8; 100]);
    let (_, first_upload) = rig.handles.resolve(&first).unwrap();
    let (_, second_upload) = rig.handles.resolve(&second).unwrap();

    let incoming = rig.handles.create("image/png".into()).unwrap();
    let refused = rig.handles.append(&incoming, &[3u8; 50]).unwrap_err();

    assert_eq!(refused, HandleError::Busy { budget: 200 });
    assert!(refused.to_string().starts_with("busy:"), "got {refused}");
    assert_eq!(read(&first_upload), [1u8; 100]);
    assert_eq!(read(&second_upload), [2u8; 100]);
    assert_eq!(rig.handles.resolve(&incoming).unwrap().0.size, 0);
  }

  #[test]
  fn a_put_shares_the_handle_buffer_instead_of_copying_it() {
    let rig = memory();
    let handle = filled(&rig, &[9u8; 4096]);
    let holders = || Arc::strong_count(&rig.handles.store().held.lock().unwrap()[&handle]);
    assert_eq!(holders(), 1);

    let (_, first) = rig.handles.resolve(&handle).unwrap();
    let (_, second) = rig.handles.resolve(&handle).unwrap();

    assert_eq!(holders(), 3, "every put reads the one buffer the handle holds");
    assert_eq!(read(&first), read(&second));
  }

  #[test]
  fn an_append_during_a_put_leaves_the_put_reading_what_it_started_with() {
    let rig = memory();
    let handle = filled(&rig, b"snapshot");
    let (_, uploading) = rig.handles.resolve(&handle).unwrap();

    assert_eq!(rig.handles.append(&handle, b" and more").unwrap(), 17);

    assert_eq!(read(&uploading), b"snapshot");
    assert_eq!(read(&rig.handles.resolve(&handle).unwrap().1), b"snapshot and more");
  }

  #[test]
  fn appending_to_an_imported_handle_never_changes_the_picked_file() {
    let dir = tempfile::tempdir().unwrap();
    let rig = on_disk(dir.path(), HANDLE_BUDGET_BYTES);
    let source = dir.path().join("picked.gif");
    std::fs::write(&source, b"GIF89a").unwrap();

    let (handle, blob) = rig.handles.import(&source, "image/gif".into()).unwrap();
    assert_eq!(blob.size, 6);
    assert_eq!(rig.handles.append(&handle, b"tail").unwrap(), 10);

    assert_eq!(read(&rig.handles.resolve(&handle).unwrap().1), b"GIF89atail");
    assert_eq!(std::fs::read(&source).unwrap(), b"GIF89a");
  }

  #[cfg(unix)]
  #[test]
  fn a_handle_imported_from_a_read_only_file_still_takes_appends() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let rig = on_disk(dir.path(), HANDLE_BUDGET_BYTES);
    let source = dir.path().join("locked.bin");
    std::fs::write(&source, b"locked").unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o444)).unwrap();

    let (handle, _) = rig.handles.import(&source, "application/octet-stream".into()).unwrap();

    assert_eq!(rig.handles.append(&handle, b"+").unwrap(), 7);
    assert_eq!(std::fs::read(&source).unwrap(), b"locked");
  }

  #[test]
  fn an_expired_import_takes_its_copy_with_it_and_leaves_the_picked_file() {
    let dir = tempfile::tempdir().unwrap();
    let rig = on_disk(dir.path(), HANDLE_BUDGET_BYTES);
    let source = dir.path().join("picked.gif");
    std::fs::write(&source, b"GIF89a").unwrap();
    let (handle, _) = rig.handles.import(&source, "image/gif".into()).unwrap();
    let copy = rig.handles.store().dir().join(&handle);
    assert!(copy.exists());

    rig.clock.advance(HANDLE_TTL);

    assert!(!live(&rig, &handle));
    assert!(!copy.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"GIF89a");
  }

  #[test]
  fn an_import_past_the_budget_is_refused_and_leaves_the_held_copy() {
    let dir = tempfile::tempdir().unwrap();
    let rig = on_disk(dir.path(), 10);
    let source = dir.path().join("picked.gif");
    std::fs::write(&source, b"GIF89a").unwrap();
    let (first, _) = rig.handles.import(&source, "image/gif".into()).unwrap();
    rig.clock.advance(MINUTE);

    let refused = rig.handles.import(&source, "image/gif".into()).unwrap_err();

    assert_eq!(refused, HandleError::Busy { budget: 10 });
    assert_eq!(read(&rig.handles.resolve(&first).unwrap().1), b"GIF89a");
    assert_eq!(
      std::fs::read_dir(rig.handles.store().dir()).unwrap().count(),
      1,
      "the refused import left no copy behind"
    );
  }

  #[test]
  fn an_adopted_scratch_file_moves_under_its_handle() {
    let dir = tempfile::tempdir().unwrap();
    let rig = on_disk(dir.path(), HANDLE_BUDGET_BYTES);
    std::fs::create_dir_all(rig.handles.store().dir()).unwrap();
    let scratch = rig.handles.store().dir().join("normalized.jpg");
    std::fs::write(&scratch, b"jpeg").unwrap();

    let (handle, _) = rig.handles.adopt(&scratch, "image/jpeg".into()).unwrap();

    assert!(!scratch.exists());
    assert_eq!(read(&rig.handles.resolve(&handle).unwrap().1), b"jpeg");
  }

  #[test]
  fn a_scratch_file_that_cannot_be_adopted_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let rig = on_disk(dir.path(), 8);
    let held = filled(&rig, &[1u8; 8]);
    let (_, uploading) = rig.handles.resolve(&held).unwrap();
    let scratch = rig.handles.store().dir().join("normalized.jpg");
    std::fs::write(&scratch, b"jpeg").unwrap();

    let refused = rig.handles.adopt(&scratch, "image/jpeg".into()).unwrap_err();

    assert_eq!(refused, HandleError::Busy { budget: 8 });
    assert!(
      !scratch.exists(),
      "a refused scratch file must not linger in the handle dir"
    );
    assert_eq!(read(&uploading), [1u8; 8]);
  }

  #[test]
  fn handles_from_a_previous_run_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    let first = BlobHandles::new(FsHandleStore::new(dir.path().join("handles")));
    let handle = first.create("text/plain".into()).unwrap();
    let path = first.store().dir().join(&handle);
    assert!(path.exists());

    let _second = BlobHandles::new(FsHandleStore::new(dir.path().join("handles")));
    assert!(!path.exists());
  }
}
