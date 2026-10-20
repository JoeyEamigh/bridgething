pub mod disk;
pub mod net;
pub mod response;

use std::{
  path::Path,
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
};

use bridgething_io::HttpTransport;

use crate::{backend::ImageScaler, provider::art::ArtCache};

pub const ART_BYTES: u64 = 256 << 20;
pub const RESPONSE_BYTES: u64 = 32 << 20;
pub const NET_BYTES: u64 = 32 << 20;

pub struct PhoneCache {
  art: Arc<ArtCache>,
  responses: response::ResponseCache,
  net: disk::DiskCache,
  online: Arc<AtomicBool>,
}

impl PhoneCache {
  pub fn open(root: &Path, scaler: Option<Arc<dyn ImageScaler>>) -> Arc<Self> {
    let art = disk::DiskCache::new(root.join("art"), ART_BYTES);
    let responses = disk::DiskCache::new(root.join("responses"), RESPONSE_BYTES);
    let net = disk::DiskCache::new(root.join("net"), NET_BYTES);
    let stores = [art.clone(), responses.clone(), net.clone()];
    let scanning = std::thread::Builder::new()
      .name("phone-cache-scan".into())
      .spawn(move || stores.iter().for_each(disk::DiskCache::scan));
    if let Err(error) = scanning {
      tracing::warn!(%error, "the phone cache scan did not start; sizes are counted from this run only");
    }
    let online = Arc::new(AtomicBool::new(true));
    Arc::new(Self {
      art: Arc::new(ArtCache::new(art, scaler)),
      responses: response::ResponseCache::new(responses, online.clone(), response::REVALIDATE_BUDGET),
      net,
      online,
    })
  }

  pub fn art(&self) -> &Arc<ArtCache> {
    &self.art
  }

  pub fn responses(&self) -> &response::ResponseCache {
    &self.responses
  }

  pub fn net(&self, inner: Arc<dyn HttpTransport>) -> Arc<dyn HttpTransport> {
    Arc::new(net::CachingHttp::new(inner, self.net.clone()))
  }

  pub fn set_online(&self, online: bool) {
    self.online.store(online, Ordering::SeqCst);
  }
}
