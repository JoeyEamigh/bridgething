use std::{path::PathBuf, sync::Arc, time::Duration};

use bridgething_gateway::Gateway;
use bridgething_sdk_runtime::RequestFailure;
use libbridgething::{
  WebappError,
  gateway::{TransferBody, WebappResource, WebappResourceKind},
  wire::WireError,
};
use uuid::Uuid;

pub use crate::seam::CachedResource;
use crate::{
  blob::{digest_of, is_digest},
  bundle::{
    ArtifactDigest,
    fetch::{ArtifactFetch, DownloadRequest},
  },
  seam::{BlobStore, SlotIndex},
  transfer::{TransferReceiveError, TransferReceiver},
};

pub const RESOURCE_TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);

pub const HUB_WEBAPP_ID: Uuid = uuid::uuid!("019693c0-5c6a-71f0-a89d-7e2a4d9c0a01");
pub const STOCK_WEBAPP_ID: Uuid = uuid::uuid!("b12be731-416c-4cf7-8a91-3d2f19a45e21");
pub const BROWSER_WEBAPP_ID: Uuid = uuid::uuid!("019e7f1a-bcea-7187-a438-53e486c4d950");
pub const BUILTIN_WEBAPPS: [(&str, Uuid); 3] = [
  ("hub", HUB_WEBAPP_ID),
  ("browser", BROWSER_WEBAPP_ID),
  ("stock", STOCK_WEBAPP_ID),
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebappResourceError {
  #[error("webapp {webapp_id} {kind:?}: the daemon reported the cache current but nothing is cached")]
  StaleCacheMissing { webapp_id: Uuid, kind: WebappResourceKind },
  #[error("resource sha256 mismatch: expected {expected}, got {got}")]
  ShaMismatch { expected: String, got: String },
  #[error("the daemon rejected the resource fetch: {0:?}")]
  Domain(WebappError),
  #[error("resource fetch protocol error: {0:?}")]
  Wire(WireError),
  #[error(transparent)]
  Transfer(#[from] TransferReceiveError),
  #[error("blob store: {0}")]
  Store(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceOrigin {
  pub url: String,
  pub sha256: String,
  pub size: u64,
  pub mime: Option<String>,
}

pub struct WebappResourceService {
  blobs: Arc<dyn BlobStore>,
  slots: Arc<dyn SlotIndex>,
  receiver: Arc<TransferReceiver>,
  timeout: Duration,
  fetch: Option<(Arc<dyn ArtifactFetch>, PathBuf)>,
}

impl WebappResourceService {
  pub fn new(blobs: Arc<dyn BlobStore>, slots: Arc<dyn SlotIndex>, receiver: Arc<TransferReceiver>) -> Self {
    Self::with_timeout(blobs, slots, receiver, RESOURCE_TRANSFER_TIMEOUT)
  }

  pub fn with_timeout(
    blobs: Arc<dyn BlobStore>,
    slots: Arc<dyn SlotIndex>,
    receiver: Arc<TransferReceiver>,
    timeout: Duration,
  ) -> Self {
    Self {
      blobs,
      slots,
      receiver,
      timeout,
      fetch: None,
    }
  }

  pub fn with_fetch(mut self, fetch: Arc<dyn ArtifactFetch>, scratch: PathBuf) -> Self {
    self.fetch = Some((fetch, scratch));
    self
  }

  pub fn cached(&self, webapp_id: Uuid, kind: WebappResourceKind) -> Option<CachedResource> {
    let slot = slot(webapp_id, kind);
    match self.slots.get(&slot) {
      Some(resource) if self.blobs.contains(&resource.digest) => Some(resource),
      Some(_) => {
        if let Err(reason) = self.slots.remove(&slot) {
          tracing::warn!(%slot, %reason, "the slot index kept a digest whose blob is gone");
        }
        None
      }
      None => None,
    }
  }

  pub async fn fetch(
    &self,
    gateway: &Gateway,
    webapp_id: Uuid,
    kind: WebappResourceKind,
    origin: Option<&ResourceOrigin>,
  ) -> Result<CachedResource, WebappResourceError> {
    let cached = self.cached(webapp_id, kind);

    let origin = origin.filter(|_| kind == WebappResourceKind::Settings);

    if let Some(origin) = origin {
      if let Some(hit) = cached
        .as_ref()
        .filter(|resource| resource.digest.eq_ignore_ascii_case(&origin.sha256))
      {
        return Ok(hit.clone());
      }
      if let Some(resource) = self.fetch_from_origin(webapp_id, kind, origin).await {
        return Ok(resource);
      }
    }

    let reply = gateway
      .webapp()
      .resource(WebappResource {
        id: webapp_id,
        kind,
        have: cached.as_ref().map(|resource| resource.digest.clone()),
      })
      .await
      .map_err(|failure| match failure {
        RequestFailure::Domain(error) => WebappResourceError::Domain(error),
        RequestFailure::Protocol(error) => WebappResourceError::Wire(error),
        RequestFailure::ResponseMismatch => WebappResourceError::Wire(WireError::Malformed {
          reason: "the resource reply did not match the request".into(),
        }),
        RequestFailure::Timeout => WebappResourceError::Wire(WireError::HandlerFailed {
          reason: "the resource request timed out".into(),
        }),
        RequestFailure::Disconnected => WebappResourceError::Wire(WireError::HandlerFailed {
          reason: "the link closed before the resource reply".into(),
        }),
      })?;

    let Some(body) = reply.body else {
      let cached = cached.ok_or(WebappResourceError::StaleCacheMissing { webapp_id, kind })?;
      let resource = CachedResource {
        digest: cached.digest,
        mime: reply.mime.or(cached.mime),
      };
      self.remember(webapp_id, kind, &resource);
      return Ok(resource);
    };

    let bytes = match body {
      TransferBody::Inline(bytes) => bytes,
      TransferBody::Stream(reference) => {
        self.receiver.register(&reference);
        self.receiver.collect(reference.id, self.timeout).await?
      }
    };

    let digest = digest_of(&bytes);
    if !digest.eq_ignore_ascii_case(&reply.sha256) {
      return Err(WebappResourceError::ShaMismatch {
        expected: reply.sha256,
        got: digest,
      });
    }

    self.blobs.put(&digest, &bytes).map_err(WebappResourceError::Store)?;
    let resource = CachedResource {
      digest,
      mime: reply.mime,
    };
    if let Some(stale) = self.remember(webapp_id, kind, &resource) {
      let _ = self.blobs.remove(&stale);
    }
    Ok(resource)
  }

  async fn fetch_from_origin(
    &self,
    webapp_id: Uuid,
    kind: WebappResourceKind,
    origin: &ResourceOrigin,
  ) -> Option<CachedResource> {
    let (fetch, scratch) = self.fetch.as_ref()?;
    let sha256 = origin.sha256.to_lowercase();
    if !is_digest(&sha256) {
      tracing::warn!(%webapp_id, "hosted resource digest is not a sha256, refusing to fetch it");
      return None;
    }
    let path = fetch
      .download(DownloadRequest {
        url: origin.url.clone(),
        dir: scratch.clone(),
        filename: format!("{webapp_id}-{sha256}"),
        asset: "webapp resource".into(),
        expected: Some(ArtifactDigest {
          size: origin.size,
          sha256: sha256.clone(),
        }),
        progress: None,
      })
      .await
      .inspect_err(|error| tracing::debug!(%webapp_id, url = %origin.url, %error, "hosted resource unusable"))
      .ok()?;

    let bytes = std::fs::read(&path)
      .inspect_err(|error| tracing::warn!(%error, "could not read the downloaded resource"))
      .ok();
    let _ = std::fs::remove_file(&path);
    let bytes = bytes?;

    let digest = digest_of(&bytes);
    if digest != sha256 {
      tracing::warn!(%webapp_id, "hosted resource hashed to {digest}, not the {sha256} the device reports");
      return None;
    }
    if let Err(reason) = self.blobs.put(&digest, &bytes) {
      tracing::warn!(%reason, "could not store the hosted resource");
      return None;
    }

    let resource = CachedResource {
      digest,
      mime: origin.mime.clone(),
    };
    if let Some(stale) = self.remember(webapp_id, kind, &resource) {
      let _ = self.blobs.remove(&stale);
    }
    Some(resource)
  }

  fn remember(&self, webapp_id: Uuid, kind: WebappResourceKind, resource: &CachedResource) -> Option<String> {
    let slot = slot(webapp_id, kind);
    let displaced = self.slots.get(&slot).filter(|prior| prior.digest != resource.digest);
    if let Err(reason) = self.slots.set(&slot, resource) {
      tracing::warn!(%slot, %reason, "the slot index did not take the new digest");
    }
    let displaced = displaced?.digest;
    self
      .slots
      .entries()
      .iter()
      .all(|(_, held)| held.digest != displaced)
      .then_some(displaced)
  }
}

fn slot(webapp_id: Uuid, kind: WebappResourceKind) -> String {
  let kind = match kind {
    WebappResourceKind::Icon => "icon",
    WebappResourceKind::Settings => "settings",
    WebappResourceKind::Overlay => "overlay",
  };
  format!("{webapp_id}__{kind}")
}
