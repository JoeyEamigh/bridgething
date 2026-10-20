use std::{
  path::{Path, PathBuf},
  sync::Arc,
};

use base64::Engine;
use bridgething_delivery::webapp::{
  blob::{BlobError, BlobProgress, BlobUpload},
  handles::{BlobHandles, FsHandleStore, HandleError, HeldBlob},
};
use uuid::Uuid;

use super::Session;
use crate::{
  api::{CompanionError, MediaCover, MediaPickKind, PickedMedia, WebappBlob},
  backend::{ImageTarget, sniff_and_scale},
  media::{MediaKind, MediaProbe, NormalizeError, NormalizedMedia, Staged, probe_file},
};

const PREVIEW_TARGET: ImageTarget = ImageTarget::Thumbnail {
  max_edge: 320,
  quality: 0.7,
};
const UNTYPED: &str = "application/octet-stream";

struct Landed {
  claim: Unclaimed,
  held: HeldBlob,
  still: Option<Vec<u8>>,
}

struct Unclaimed {
  handles: Arc<BlobHandles<FsHandleStore>>,
  handle: Option<String>,
}

impl Unclaimed {
  fn claim(mut self) -> String {
    self.handle.take().expect("a handle is claimed once")
  }
}

impl Drop for Unclaimed {
  fn drop(&mut self) {
    if let Some(handle) = self.handle.take() {
      self.handles.forget(&handle);
    }
  }
}

impl From<NormalizeError> for CompanionError {
  fn from(error: NormalizeError) -> Self {
    Self::Device(error.to_string())
  }
}

impl From<HandleError> for CompanionError {
  fn from(error: HandleError) -> Self {
    Self::Device(error.to_string())
  }
}

impl From<BlobError> for CompanionError {
  fn from(error: BlobError) -> Self {
    Self::Device(error.to_string())
  }
}

impl Session {
  pub fn media_import_ticket(&self) -> u64 {
    self.imports.ticket()
  }

  pub fn cancel_media_import(&self, ticket: u64) {
    self.imports.cancel(ticket);
  }

  pub fn clear_media_imports(&self) {
    self.imports.clear();
  }

  pub async fn import_media(
    self: &Arc<Self>,
    ticket: u64,
    source: PathBuf,
    kind: MediaPickKind,
    cover: Option<MediaCover>,
    mime_hint: Option<String>,
  ) -> Result<PickedMedia, CompanionError> {
    let session = Arc::clone(self);
    self
      .imports
      .run(
        ticket,
        async move { session.picked(&source, kind, cover, mime_hint).await },
      )
      .await
  }

  async fn picked(
    &self,
    source: &Path,
    kind: MediaPickKind,
    cover: Option<MediaCover>,
    mime_hint: Option<String>,
  ) -> Result<PickedMedia, CompanionError> {
    let probed = probe_file(source).await?;
    let wanted = match kind {
      MediaPickKind::Image => Some(MediaKind::Image),
      MediaPickKind::Video => Some(MediaKind::Video),
      MediaPickKind::Any => None,
    };
    let normalized = match wanted {
      Some(wanted) if probed.sniff.kind != wanted => return Err(not_media(wanted)),
      Some(_) => {
        self
          .normalizer
          .normalize(source, &probed, cover.map(|cover| (cover.width, cover.height)))
          .await?
      }
      None => NormalizedMedia::untouched(source, Some(typed(&probed, mime_hint, source))),
    };
    let mime = normalized.mime.unwrap_or_else(|| UNTYPED.to_owned());
    let landed = self.land(normalized.staged, mime, probed.sniff.kind).await?;
    let preview = self.preview(landed.still).await;
    Ok(PickedMedia {
      handle: landed.claim.claim(),
      mime: landed.held.mime,
      size: landed.held.size,
      width: normalized.width,
      height: normalized.height,
      duration_ms: normalized.duration_ms,
      preview,
    })
  }

  pub fn create_blob_handle(&self, mime: String) -> Result<String, CompanionError> {
    Ok(self.handles.create(mime)?)
  }

  pub async fn append_blob_handle(&self, handle: String, base64: String) -> Result<u64, CompanionError> {
    let handles = self.handles.clone();
    Ok(
      tokio::task::spawn_blocking(move || handles.append_base64(&handle, &base64))
        .await
        .map_err(|error| CompanionError::Runtime(error.to_string()))??,
    )
  }

  pub async fn put_webapp_blob(
    &self,
    device_id: &str,
    webapp: Uuid,
    key: String,
    handle: &str,
    progress: Option<BlobProgress>,
  ) -> Result<WebappBlob, CompanionError> {
    let upload = BlobUpload::from_handle(&self.handles, webapp, key, handle)?;
    Ok(WebappBlob::from(self.ota.put_blob(device_id, upload, progress).await?))
  }

  pub async fn delete_webapp_blob(&self, device_id: &str, webapp: Uuid, key: String) -> Result<(), CompanionError> {
    Ok(self.ota.delete_blob(device_id, webapp, key).await?)
  }

  pub async fn list_webapp_blobs(&self, device_id: &str, webapp: Uuid) -> Result<Vec<WebappBlob>, CompanionError> {
    let entries = self.ota.list_blobs(device_id, webapp).await?;
    Ok(entries.into_iter().map(WebappBlob::from).collect())
  }

  async fn land(&self, staged: Staged, mime: String, kind: MediaKind) -> Result<Landed, CompanionError> {
    let handles = self.handles.clone();
    let previews = self.backends.image.is_some();
    let transcoder = self
      .backends
      .transcoder
      .clone()
      .filter(|_| previews && kind == MediaKind::Video);
    let landed = tokio::task::spawn_blocking(move || {
      let poster = |path: &Path| transcoder.as_ref()?.poster(path.to_string_lossy().into_owned());
      let (frame, (handle, held)) = match &staged {
        Staged::Scratch(scratch) => {
          let frame = poster(scratch.path());
          (frame, handles.adopt(scratch.path(), mime)?)
        }
        Staged::Source(source) => {
          let landed = handles.import(source, mime)?;
          (poster(source), landed)
        }
      };
      let still = match kind {
        MediaKind::Image if previews => held_bytes(&handles, &handle),
        _ => frame,
      };
      let claim = Unclaimed {
        handles,
        handle: Some(handle),
      };
      Ok::<_, HandleError>(Landed { claim, held, still })
    })
    .await
    .map_err(|error| CompanionError::Runtime(error.to_string()))??;
    Ok(landed)
  }

  async fn preview(&self, still: Option<Vec<u8>>) -> Option<String> {
    let scaler = self.backends.image.clone()?;
    let still = still?;
    let scaled = tokio::task::spawn_blocking(move || sniff_and_scale(scaler.as_ref(), still, PREVIEW_TARGET))
      .await
      .ok()
      .flatten()?;
    Some(format!(
      "data:{};base64,{}",
      scaled.mime,
      base64::engine::general_purpose::STANDARD.encode(scaled.bytes)
    ))
  }
}

fn held_bytes(handles: &BlobHandles<FsHandleStore>, handle: &str) -> Option<Vec<u8>> {
  let (_, artifact) = handles.resolve(handle).ok()?;
  let mut bytes = vec![0u8; usize::try_from(artifact.size().ok()?).ok()?];
  artifact.read_at(0, &mut bytes).ok()?;
  Some(bytes)
}

fn typed(probed: &MediaProbe, hint: Option<String>, source: &Path) -> String {
  let sniffed = &probed.sniff;
  if let Some(mime) = sniffed.mime.as_ref().filter(|_| sniffed.names_its_type()) {
    return mime.clone();
  }
  let pictureless = probed.movie.is_some_and(|movie| !movie.has_video);
  let fits = |mime: &String| mime.contains('/') && mime != UNTYPED && !(pictureless && mime.starts_with("video/"));
  hint
    .filter(fits)
    .or_else(|| {
      mime_guess::from_path(source)
        .first_raw()
        .map(str::to_owned)
        .filter(fits)
    })
    .or_else(|| sniffed.mime.clone())
    .unwrap_or_else(|| UNTYPED.to_owned())
}

fn not_media(wanted: MediaKind) -> CompanionError {
  CompanionError::Device(format!(
    "not_media: the picked file is not {}",
    if wanted == MediaKind::Image {
      "an image"
    } else {
      "a video"
    }
  ))
}
