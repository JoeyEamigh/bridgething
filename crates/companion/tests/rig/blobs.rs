use std::{
  path::{Path, PathBuf},
  sync::{Arc, Condvar, Mutex},
  time::{Duration, Instant},
};

use base64::Engine;
use bridgething_companion::{
  api::{BlobProgressSink, CompanionError, CompanionSession, MediaCover, MediaPickKind, PickedMedia, WebappBlob},
  backend::{
    ImageScaler, ImageTarget, MediaTranscoder, ScaledImage, TranscodeJob, TranscodeRequest, TranscodeSink,
    TranscodedVideo,
  },
  media::{MediaKind, MediaSniff},
};
use bridgething_delivery::webapp::handles::HANDLE_MAX_BYTES;
use uuid::Uuid;

use crate::{
  fixture::{self, Track, boxed},
  support::{Rig, Setup, TO_DEVICE},
};

const CHUNK: usize = 256 * 1024;
const PUT_DEADLINE: Duration = Duration::from_secs(60);
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F'];

fn b64(bytes: &[u8]) -> String {
  base64::engine::general_purpose::STANDARD.encode(bytes)
}

async fn plant(rig: &Rig) -> Uuid {
  let id = Uuid::now_v7();
  let dir = rig.harness.state_dir().join("webapps").join(id.simple().to_string());
  std::fs::create_dir_all(&dir).expect("bundle dir");
  std::fs::write(dir.join("index.html"), b"<h1>blobs</h1>").expect("index");
  std::fs::write(
    dir.join("manifest.json"),
    format!(r#"{{"id":"{id}","name":"blobs","version":"0.1.0"}}"#),
  )
  .expect("manifest");
  rig.harness.state().webapps.rescan().await;
  id
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
  (0..len)
    .map(|at| (at as u8).wrapping_mul(31).wrapping_add(seed))
    .collect()
}

fn on_device(rig: &Rig, id: Uuid, sha: &str) -> std::path::PathBuf {
  rig
    .harness
    .state_dir()
    .join("webapp-blobs")
    .join(id.simple().to_string())
    .join(sha)
}

async fn handle_of(rig: &Rig, mime: &str, bytes: &[u8]) -> String {
  let handle = rig.companion.create_blob_handle(mime.into()).expect("a handle");
  for chunk in bytes.chunks(CHUNK) {
    rig
      .companion
      .append_blob_handle(handle.clone(), b64(chunk))
      .await
      .expect("the chunk lands");
  }
  handle
}

async fn put(rig: &Rig, id: Uuid, key: &str, handle: &str) -> Result<WebappBlob, String> {
  tokio::time::timeout(
    PUT_DEADLINE,
    rig
      .companion
      .put_webapp_blob(rig.device_id().into(), id.to_string(), key.into(), handle.into(), None),
  )
  .await
  .expect("the put settles")
  .map_err(|error| error.to_string())
}

async fn listed(rig: &Rig, id: Uuid) -> Vec<WebappBlob> {
  rig
    .companion
    .list_webapp_blobs(rig.device_id().into(), id.to_string())
    .await
    .expect("the device lists its blobs")
}

fn fragments(rig: &Rig) -> usize {
  rig
    .transcript()
    .iter()
    .filter(|entry| entry.dir == TO_DEVICE && entry.msg == "transfer.fragment")
    .count()
}

fn sha_of(bytes: &[u8]) -> String {
  bridgething_delivery::bundle::fetch::sha256_hex(bytes)
}

async fn acked_past(rig: &Rig, bytes: u32) {
  let deadline = tokio::time::Instant::now() + PUT_DEADLINE;
  while rig.said().acked.iter().copied().max().unwrap_or(0) < bytes {
    assert!(
      tokio::time::Instant::now() < deadline,
      "the device never acked {bytes} bytes"
    );
    tokio::time::sleep(Duration::from_millis(2)).await;
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_built_handle_streams_to_the_device_and_lands_byte_for_byte() {
  let rig = Rig::start().await;
  let id = plant(&rig).await;
  let body = pattern(700 * 1024, 3);
  let handle = handle_of(&rig, "video/mp4", &body).await;

  let stored = put(&rig, id, "background", &handle)
    .await
    .expect("the device stores it");

  assert_eq!(
    stored,
    WebappBlob {
      key: "background".into(),
      sha256: sha_of(&body),
      mime: "video/mp4".into(),
      size: body.len() as u64,
    }
  );
  assert_eq!(
    std::fs::read(on_device(&rig, id, &stored.sha256)).expect("the blob is on the device"),
    body
  );
  assert_eq!(listed(&rig, id).await, vec![stored]);
}

#[derive(Default)]
struct Ticks(Mutex<Vec<(u64, u64)>>);

impl BlobProgressSink for Ticks {
  fn progress(&self, sent: u64, total: u64) {
    self.0.lock().unwrap().push((sent, total));
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_put_reports_throttled_progress_that_ends_on_the_whole_file() {
  let rig = Rig::start().await;
  let id = plant(&rig).await;
  let body = pattern(4 * 1024 * 1024, 11);
  let total = body.len() as u64;
  let handle = handle_of(&rig, "video/mp4", &body).await;
  let ticks = Arc::new(Ticks::default());

  tokio::time::timeout(
    PUT_DEADLINE,
    rig.companion.put_webapp_blob(
      rig.device_id().into(),
      id.to_string(),
      "clip".into(),
      handle.clone(),
      Some(ticks.clone() as Arc<dyn BlobProgressSink>),
    ),
  )
  .await
  .expect("the put settles")
  .expect("the device stores it");

  let seen = ticks.0.lock().unwrap().clone();
  assert_eq!(
    seen.first(),
    Some(&(0, total)),
    "the first tick says nothing has moved yet"
  );
  assert_eq!(seen.last(), Some(&(total, total)), "the last tick is the whole file");
  assert!(seen.iter().all(|(_, of)| *of == total));
  assert!(
    seen.windows(2).all(|pair| pair[0].0 <= pair[1].0),
    "progress never goes backwards: {seen:?}"
  );
  assert!(
    seen.len() < 64,
    "progress is throttled, not one tick per fragment: {} ticks",
    seen.len()
  );

  let handle = handle_of(&rig, "video/mp4", &body).await;
  let again = Arc::new(Ticks::default());
  rig
    .companion
    .put_webapp_blob(
      rig.device_id().into(),
      id.to_string(),
      "copy".into(),
      handle,
      Some(again.clone() as Arc<dyn BlobProgressSink>),
    )
    .await
    .expect("the device already holds it");
  assert_eq!(
    again.0.lock().unwrap().last(),
    Some(&(total, total)),
    "a put the device already holds still reports done"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn putting_bytes_the_device_already_holds_streams_nothing() {
  let rig = Rig::recording().await;
  let id = plant(&rig).await;
  let body = pattern(300 * 1024, 9);
  let handle = handle_of(&rig, "image/gif", &body).await;
  put(&rig, id, "a", &handle).await.expect("the first put lands");
  let after_first = fragments(&rig);
  let copy = handle_of(&rig, "image/gif", &body).await;

  let again = put(&rig, id, "b", &copy).await.expect("the second key lands");

  assert_eq!(again.sha256, sha_of(&body));
  assert_eq!(
    fragments(&rig),
    after_first,
    "the device acks content it already holds at full size, so not one fragment crosses the link"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delete_removes_the_key_from_the_listing() {
  let rig = Rig::start().await;
  let id = plant(&rig).await;
  let handle = handle_of(&rig, "image/png", &pattern(64 * 1024, 1)).await;
  put(&rig, id, "logo", &handle).await.expect("the put lands");

  rig
    .companion
    .delete_webapp_blob(rig.device_id().into(), id.to_string(), "logo".into())
    .await
    .expect("the delete lands");

  assert!(listed(&rig, id).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_devices_refusals_reach_the_caller_with_a_kind_it_can_branch_on() {
  let rig = Rig::start().await;
  let id = plant(&rig).await;
  let handle = handle_of(&rig, "image/png", &pattern(1024, 2)).await;

  let bad_key = put(&rig, id, "line\nbreak", &handle)
    .await
    .expect_err("a control character is refused");
  assert!(bad_key.starts_with("invalid_key:"), "got {bad_key}");

  let nobody = put(&rig, Uuid::now_v7(), "logo", &handle)
    .await
    .expect_err("no such webapp");
  assert!(nobody.starts_with("not_found:"), "got {nobody}");

  let gone = put(&rig, id, "logo", "no-such-handle")
    .await
    .expect_err("a dead handle");
  assert!(gone.starts_with("unknown_handle:"), "got {gone}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handle_cannot_grow_past_what_the_device_would_ever_take() {
  let rig = Rig::start().await;
  let handle = rig.companion.create_blob_handle("video/mp4".into()).expect("a handle");
  let chunk = b64(&vec![0u8; 8 * 1024 * 1024]);
  for _ in 0..4 {
    rig
      .companion
      .append_blob_handle(handle.clone(), chunk.clone())
      .await
      .expect("up to the cap fits");
  }

  let over = rig
    .companion
    .append_blob_handle(handle, b64(&[0u8]))
    .await
    .expect_err("one byte past the cap");
  assert!(over.to_string().starts_with("too_large:"), "got {over}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_put_cancelled_midway_leaves_a_partial_the_next_put_resumes() {
  let rig = Rig::recording().await;
  let id = plant(&rig).await;
  let body = pattern(6 * 1024 * 1024, 5);
  let handle = handle_of(&rig, "video/mp4", &body).await;

  let companion = rig.companion.clone();
  let (device, webapp, held) = (rig.device_id().to_owned(), id.to_string(), handle.clone());
  let first = tokio::spawn(async move {
    companion
      .put_webapp_blob(device, webapp, "clip".into(), held, None)
      .await
  });
  acked_past(&rig, 512 * 1024).await;
  first.abort();
  let _ = first.await;
  let kept = rig
    .said()
    .acked
    .iter()
    .copied()
    .max()
    .expect("the device acked a partial");
  assert!((kept as usize) < body.len(), "the first put was cut short");

  let stored = put(&rig, id, "clip", &handle).await.expect("the retry lands");

  assert_eq!(stored.sha256, sha_of(&body));
  let mut sent = std::collections::HashSet::new();
  for offset in rig.fragment_offsets() {
    assert!(
      sent.insert(offset) || offset >= kept,
      "the fragment at {offset} crossed the link twice though the device already held {kept} bytes"
    );
  }
  assert_eq!(
    std::fs::read(on_device(&rig, id, &stored.sha256)).expect("the blob is on the device"),
    body
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_put_of_the_same_key_supersedes_the_one_in_flight() {
  let rig = Rig::start().await;
  let id = plant(&rig).await;
  let big = handle_of(&rig, "video/mp4", &pattern(6 * 1024 * 1024, 7)).await;
  let small_body = pattern(32 * 1024, 8);
  let small = handle_of(&rig, "image/png", &small_body).await;

  let companion = rig.companion.clone();
  let (device, webapp) = (rig.device_id().to_owned(), id.to_string());
  let first = tokio::spawn(async move { companion.put_webapp_blob(device, webapp, "bg".into(), big, None).await });
  acked_past(&rig, 1).await;

  let stored = put(&rig, id, "bg", &small).await.expect("the newer put lands");
  let superseded = first
    .await
    .expect("the first task")
    .expect_err("the older put gives way");

  assert!(superseded.to_string().starts_with("superseded:"), "got {superseded}");
  assert_eq!(stored.sha256, sha_of(&small_body));
  assert_eq!(listed(&rig, id).await, vec![stored]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_put_whose_link_drops_fails_instead_of_hanging() {
  let rig = Rig::start().await;
  let id = plant(&rig).await;
  let handle = handle_of(&rig, "video/mp4", &pattern(6 * 1024 * 1024, 4)).await;

  let companion = rig.companion.clone();
  let (device, webapp) = (rig.device_id().to_owned(), id.to_string());
  let putting = tokio::spawn(async move {
    companion
      .put_webapp_blob(device, webapp, "clip".into(), handle, None)
      .await
  });
  acked_past(&rig, 1).await;
  rig.session.direct_disconnected(rig.device_id()).await;

  let failed = tokio::time::timeout(PUT_DEADLINE, putting)
    .await
    .expect("the put settles once the link is gone")
    .expect("the put task")
    .expect_err("a dead link cannot store anything");
  assert!(failed.to_string().starts_with("link_lost:"), "got {failed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_blob_verb_without_a_link_says_not_connected() {
  let rig = Rig::start().await;
  let id = plant(&rig).await.to_string();
  let handle = handle_of(&rig, "image/png", &pattern(1024, 6)).await;
  let nobody = "no-such-device".to_owned();

  let put = rig
    .companion
    .put_webapp_blob(nobody.clone(), id.clone(), "logo".into(), handle, None)
    .await
    .expect_err("nothing to put to");
  let deleted = rig
    .companion
    .delete_webapp_blob(nobody.clone(), id.clone(), "logo".into())
    .await
    .expect_err("nothing to delete from");
  let listed = rig
    .companion
    .list_webapp_blobs(nobody, id)
    .await
    .expect_err("nothing to list");

  for failed in [put, deleted, listed] {
    assert_eq!(failed.to_string(), "not_connected: no link to the device");
  }
}

fn leads_with_a_token(message: &str) -> bool {
  message.split_once(": ").is_some_and(|(token, _)| {
    token.starts_with(|first: char| first.is_ascii_lowercase())
      && token
        .chars()
        .all(|part| part.is_ascii_lowercase() || part.is_ascii_digit() || part == '_')
  })
}

#[tokio::test(flavor = "multi_thread")]
async fn every_refusal_of_a_delete_or_a_list_leads_with_a_token() {
  let rig = Rig::start().await;
  let device = rig.device_id().to_owned();

  let bad_id = rig
    .companion
    .list_webapp_blobs(device.clone(), "seven".into())
    .await
    .expect_err("seven is not a webapp id");
  assert!(bad_id.to_string().starts_with("invalid:"), "got {bad_id}");

  let nobody = rig
    .companion
    .list_webapp_blobs(device.clone(), Uuid::now_v7().to_string())
    .await
    .expect_err("no such webapp");
  assert!(nobody.to_string().starts_with("not_found:"), "got {nobody}");

  let unowned = rig
    .companion
    .delete_webapp_blob(device, Uuid::now_v7().to_string(), "logo".into())
    .await
    .expect_err("no such webapp");
  assert!(unowned.to_string().starts_with("not_found:"), "got {unowned}");

  for failed in [bad_id, nobody, unowned] {
    assert!(leads_with_a_token(&failed.to_string()), "got {failed}");
  }
}

#[derive(Default)]
struct BoxScaler {
  seen: Mutex<Vec<ImageTarget>>,
  sniffed: Mutex<Vec<MediaSniff>>,
  thumbnailed: Mutex<Vec<Vec<u8>>>,
  declines: bool,
}

impl BoxScaler {
  fn declining() -> Arc<Self> {
    Arc::new(Self {
      declines: true,
      ..Self::default()
    })
  }

  fn covered(&self) -> usize {
    let seen = self.seen.lock().unwrap();
    seen
      .iter()
      .filter(|target| matches!(target, ImageTarget::Cover { .. }))
      .count()
  }
}

impl ImageScaler for BoxScaler {
  fn scale(&self, bytes: Vec<u8>, sniff: MediaSniff, target: ImageTarget) -> Option<ScaledImage> {
    self.seen.lock().unwrap().push(target);
    self.sniffed.lock().unwrap().push(sniff);
    if matches!(target, ImageTarget::Thumbnail { .. }) {
      self.thumbnailed.lock().unwrap().push(bytes.clone());
    }
    match target {
      ImageTarget::Cover { .. } if self.declines => None,
      ImageTarget::Cover { width, height } => Some(ScaledImage {
        bytes: [bytes.as_slice(), b"-covered"].concat(),
        mime: "image/jpeg".into(),
        width,
        height,
      }),
      ImageTarget::Thumbnail { max_edge, .. } => Some(ScaledImage {
        bytes: b"thumb".to_vec(),
        mime: "image/jpeg".into(),
        width: max_edge,
        height: max_edge,
      }),
    }
  }
}

const TRANSCODED: &[u8] = b"\0\0\0\x10ftypisom\0\0\0\0";
const CLIP: &[u8] = b"\0\0\0\x14ftypqt  \0\0\0\0qt  ";

#[derive(Default)]
struct Gate {
  reached: Mutex<bool>,
  open: Mutex<bool>,
  wake: Condvar,
}

impl Gate {
  fn pass(&self) {
    *self.reached.lock().unwrap() = true;
    self.wake.notify_all();
    let open = self.open.lock().unwrap();
    drop(self.wake.wait_while(open, |open| !*open).unwrap());
  }

  fn opened(&self) {
    *self.open.lock().unwrap() = true;
    self.wake.notify_all();
  }

  async fn reached(&self) {
    let deadline = Instant::now() + PUT_DEADLINE;
    while !*self.reached.lock().unwrap() {
      assert!(Instant::now() < deadline, "the import never reached the gate");
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  }
}

#[derive(Default)]
struct FixedTranscoder {
  transcoded: Mutex<Vec<TranscodeRequest>>,
  postered: Mutex<Vec<(PathBuf, Vec<u8>)>>,
  output_bytes: Option<u64>,
  poster_gate: Option<Arc<Gate>>,
}

struct Finished;

impl TranscodeJob for Finished {
  fn cancel(&self) {}
}

impl MediaTranscoder for FixedTranscoder {
  fn transcode(&self, request: TranscodeRequest, sink: Arc<TranscodeSink>) -> Arc<dyn TranscodeJob> {
    std::fs::write(&request.destination, TRANSCODED).unwrap();
    if let Some(size) = self.output_bytes {
      std::fs::File::options()
        .write(true)
        .open(&request.destination)
        .unwrap()
        .set_len(size)
        .unwrap();
    }
    self.transcoded.lock().unwrap().push(request);
    sink.complete(TranscodedVideo {
      width: 854,
      height: 480,
      duration_ms: 4000,
    });
    Arc::new(Finished)
  }

  fn poster(&self, source: String) -> Option<Vec<u8>> {
    if let Some(gate) = &self.poster_gate {
      gate.pass();
    }
    let source = PathBuf::from(source);
    source.extension()?;
    let frame = std::fs::read(&source).ok()?;
    self.postered.lock().unwrap().push((source, frame));
    Some(b"poster-frame".to_vec())
  }
}

fn mp4(seconds: u32, tracks: &[Track]) -> Vec<u8> {
  fixture::movie_at_the_end(seconds * 1000, tracks, 64)
}

fn picked(dir: &Path, name: &str, bytes: &[u8]) -> String {
  let path = dir.join(name);
  std::fs::write(&path, bytes).unwrap();
  path.to_string_lossy().into_owned()
}

async fn import(
  companion: &CompanionSession,
  path: String,
  kind: MediaPickKind,
  cover: Option<MediaCover>,
  hint: Option<String>,
) -> Result<PickedMedia, CompanionError> {
  companion
    .import_picked_media(companion.media_import_ticket(), path, kind, cover, hint)
    .await
}

async fn media_rig(scaler: Option<Arc<BoxScaler>>, transcoder: bool) -> Rig {
  transcoding_rig(scaler, transcoder.then(|| Arc::new(FixedTranscoder::default()))).await
}

async fn transcoding_rig(scaler: Option<Arc<BoxScaler>>, transcoder: Option<Arc<FixedTranscoder>>) -> Rig {
  Rig::launch(Setup {
    image: scaler.map(|scaler| scaler as Arc<dyn ImageScaler>),
    transcoder: transcoder.map(|transcoder| transcoder as Arc<dyn MediaTranscoder>),
    ..Setup::default()
  })
  .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_picked_image_is_normalized_into_a_handle_with_a_preview_and_the_source_is_left_alone() {
  let scaler = Arc::new(BoxScaler::default());
  let rig = media_rig(Some(scaler.clone()), false).await;
  let dir = tempfile::tempdir().unwrap();
  let source = picked(dir.path(), "photo.jpg", JPEG);

  let media = import(
    &rig.companion,
    source.clone(),
    MediaPickKind::Image,
    Some(MediaCover {
      width: 640,
      height: 360,
    }),
    None,
  )
  .await
  .expect("the pick imports");

  assert_eq!(
    (media.mime.as_str(), media.width, media.height, media.duration_ms),
    ("image/jpeg", Some(640), Some(360), None)
  );
  assert_eq!(media.size, (JPEG.len() + b"-covered".len()) as u64);
  assert_eq!(media.preview, Some(format!("data:image/jpeg;base64,{}", b64(b"thumb"))));
  assert_eq!(
    scaler.seen.lock().unwrap()[0],
    ImageTarget::Cover {
      width: 640,
      height: 360
    }
  );
  assert_eq!(
    std::fs::read(&source).unwrap(),
    JPEG,
    "the picked file is the host's to clean up"
  );

  let id = plant(&rig).await;
  let stored = put(&rig, id, "photo", &media.handle)
    .await
    .expect("the normalized file uploads");
  assert_eq!((stored.mime.as_str(), stored.size), ("image/jpeg", media.size));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_picked_video_transcodes_and_carries_its_geometry() {
  let rig = media_rig(None, true).await;
  let dir = tempfile::tempdir().unwrap();
  let source = picked(dir.path(), "clip.mov", CLIP);

  let media = import(&rig.companion, source, MediaPickKind::Video, None, None)
    .await
    .expect("the pick imports");

  assert_eq!(
    (
      media.mime.as_str(),
      media.width,
      media.height,
      media.duration_ms,
      media.preview
    ),
    ("video/mp4", Some(854), Some(480), Some(4000), None)
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_picked_video_previews_its_poster_frame_through_the_thumbnail_target() {
  let scaler = Arc::new(BoxScaler::default());
  let transcoder = Arc::new(FixedTranscoder::default());
  let rig = transcoding_rig(Some(scaler.clone()), Some(transcoder.clone())).await;
  let dir = tempfile::tempdir().unwrap();
  let source = picked(dir.path(), "clip.mov", CLIP);

  let media = import(&rig.companion, source, MediaPickKind::Video, None, None)
    .await
    .expect("the pick imports");

  assert_eq!(media.preview, Some(format!("data:image/jpeg;base64,{}", b64(b"thumb"))));
  let postered = transcoder.postered.lock().unwrap().clone();
  assert_eq!(postered.len(), 1);
  assert_eq!(
    (
      postered[0].0.extension().and_then(|ext| ext.to_str()),
      &postered[0].1[..]
    ),
    (Some("mp4"), TRANSCODED),
    "the poster comes from the transcoded file the device will get, under a name that says what it is"
  );
  assert!(
    !postered[0].0.exists(),
    "the transcoder's output moved into the handle once the poster was taken"
  );
  assert_eq!(*scaler.thumbnailed.lock().unwrap(), vec![b"poster-frame".to_vec()]);
  assert_eq!(
    *scaler.seen.lock().unwrap(),
    vec![ImageTarget::Thumbnail {
      max_edge: 320,
      quality: 0.7
    }]
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pick_that_does_not_match_its_kind_is_refused() {
  let rig = media_rig(Some(Arc::new(BoxScaler::default())), true).await;
  let dir = tempfile::tempdir().unwrap();
  let document = picked(dir.path(), "notes.pdf", b"%PDF-1.7");

  let refused = import(&rig.companion, document, MediaPickKind::Image, None, None)
    .await
    .expect_err("a pdf is not an image");
  assert!(refused.to_string().starts_with("not_media:"), "got {refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_video_on_a_host_without_a_transcoder_is_unsupported() {
  let rig = media_rig(None, false).await;
  let dir = tempfile::tempdir().unwrap();
  let source = picked(dir.path(), "clip.mp4", b"\0\0\0\x10ftypisom\0\0\0\0");

  let refused = import(&rig.companion, source, MediaPickKind::Video, None, None)
    .await
    .expect_err("nothing can transcode it");
  assert!(
    matches!(&refused, CompanionError::Device(message) if message.starts_with("unsupported:")),
    "got {refused}"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_any_pick_is_untouched_and_falls_back_to_the_hosts_mime() {
  let scaler = Arc::new(BoxScaler::default());
  let rig = media_rig(Some(scaler.clone()), true).await;
  let dir = tempfile::tempdir().unwrap();
  let looped = fixture::gif(1, 0);
  let gif = picked(dir.path(), "loop.gif", &looped);
  let document = picked(dir.path(), "notes", b"plain words");

  let animated = import(&rig.companion, gif, MediaPickKind::Any, None, None)
    .await
    .expect("the gif imports");
  let text = import(
    &rig.companion,
    document,
    MediaPickKind::Any,
    None,
    Some("text/plain".into()),
  )
  .await
  .expect("the text imports");

  assert_eq!(
    (animated.mime.as_str(), animated.size, animated.width),
    ("image/gif", looped.len() as u64, None)
  );
  assert!(
    animated.preview.is_some(),
    "an image picked as any still gets a preview"
  );
  assert_eq!(scaler.covered(), 0, "an any pick is never normalized");
  assert_eq!((text.mime.as_str(), text.size, text.preview), ("text/plain", 11, None));
}

async fn any_mime(rig: &Rig, dir: &Path, name: &str, bytes: &[u8], hint: Option<&str>) -> String {
  import(
    &rig.companion,
    picked(dir, name, bytes),
    MediaPickKind::Any,
    None,
    hint.map(str::to_owned),
  )
  .await
  .expect("an any pick imports")
  .mime
}

#[tokio::test(flavor = "multi_thread")]
async fn an_any_pick_the_host_did_not_type_is_typed_by_its_file_name() {
  let rig = media_rig(None, false).await;
  let dir = tempfile::tempdir().unwrap();

  for (name, mime) in [
    ("logo.svg", "image/svg+xml"),
    ("theme.CSS", "text/css"),
    ("strings.json", "application/json"),
    ("face.woff2", "font/woff2"),
  ] {
    assert_eq!(any_mime(&rig, dir.path(), name, b"<svg/>", None).await, mime);
    assert_eq!(
      any_mime(&rig, dir.path(), name, b"<svg/>", Some("application/octet-stream")).await,
      mime,
      "an untyped hint says nothing"
    );
  }
  assert_eq!(
    any_mime(
      &rig,
      dir.path(),
      "logo.svg",
      b"<svg/>",
      Some("image/svg+xml; charset=utf-8")
    )
    .await,
    "image/svg+xml; charset=utf-8",
    "what the host says outranks the file name"
  );
  assert_eq!(
    any_mime(&rig, dir.path(), "mystery", b"<svg/>", None).await,
    "application/octet-stream"
  );
  assert_eq!(
    any_mime(&rig, dir.path(), "mystery.zzzzz", b"<svg/>", Some("nonsense")).await,
    "application/octet-stream"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn magic_that_only_names_a_container_yields_to_a_more_specific_type() {
  let rig = media_rig(None, false).await;
  let dir = tempfile::tempdir().unwrap();
  let voice = boxed(b"ftyp", b"M4A \0\0\0\0M4A mp42isom");
  let narrated = mp4(60, &[Track::Audio]);
  let raw = b"II*\0\x08\0\0\0";
  let matroska = [0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x82, 0x88, b'm', b'a', b't', b'r'];

  assert_eq!(
    any_mime(&rig, dir.path(), "voice.m4a", &voice, Some("audio/x-m4a")).await,
    "audio/x-m4a"
  );
  assert_eq!(any_mime(&rig, dir.path(), "voice", &voice, None).await, "audio/mp4");
  assert_eq!(
    any_mime(&rig, dir.path(), "narrated", &narrated, None).await,
    "audio/mp4"
  );
  assert_eq!(
    any_mime(&rig, dir.path(), "narrated.mp4", &narrated, Some("audio/mp4")).await,
    "audio/mp4"
  );
  assert_eq!(
    any_mime(&rig, dir.path(), "raw.dng", raw, None).await,
    "image/x-adobe-dng"
  );
  assert_eq!(any_mime(&rig, dir.path(), "scan", raw, None).await, "image/tiff");
  assert_eq!(
    any_mime(&rig, dir.path(), "album.mka", &matroska, Some("audio/x-matroska")).await,
    "audio/x-matroska"
  );
  assert_eq!(
    any_mime(&rig, dir.path(), "clip", &mp4(60, &[Track::Video]), None).await,
    "video/mp4"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn magic_that_names_one_type_outranks_the_host_and_the_file_name() {
  let rig = media_rig(None, false).await;
  let dir = tempfile::tempdir().unwrap();

  assert_eq!(
    any_mime(&rig, dir.path(), "photo.png", JPEG, Some("image/png")).await,
    "image/jpeg"
  );
  assert_eq!(
    any_mime(&rig, dir.path(), "loop.txt", &fixture::gif(1, 0), None).await,
    "image/gif"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_is_not_a_video() {
  let transcoder = Arc::new(FixedTranscoder::default());
  let rig = transcoding_rig(None, Some(transcoder.clone())).await;
  let dir = tempfile::tempdir().unwrap();

  for (name, bytes) in [
    ("voice.m4a", boxed(b"ftyp", b"M4A \0\0\0\0M4A mp42isom")),
    ("narrated.mp4", mp4(60, &[Track::Audio])),
    ("voice.webm", fixture::matroska("webm", 60_000, &[Track::Audio])),
    ("album.mka", fixture::recorded_matroska("matroska", &[Track::Audio])),
  ] {
    let refused = import(
      &rig.companion,
      picked(dir.path(), name, &bytes),
      MediaPickKind::Video,
      None,
      None,
    )
    .await
    .expect_err("audio has no picture");
    assert!(refused.to_string().starts_with("not_media:"), "{name}: got {refused}");
  }
  assert!(transcoder.transcoded.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_any_pick_of_a_video_previews_the_picked_file() {
  let scaler = Arc::new(BoxScaler::default());
  let transcoder = Arc::new(FixedTranscoder::default());
  let rig = transcoding_rig(Some(scaler.clone()), Some(transcoder.clone())).await;
  let dir = tempfile::tempdir().unwrap();
  let source = picked(dir.path(), "clip.mov", CLIP);

  let media = import(&rig.companion, source.clone(), MediaPickKind::Any, None, None)
    .await
    .expect("the pick imports");

  assert_eq!(media.preview, Some(format!("data:image/jpeg;base64,{}", b64(b"thumb"))));
  assert_eq!(
    *transcoder.postered.lock().unwrap(),
    vec![(PathBuf::from(&source), CLIP.to_vec())],
    "the poster is asked of the file that still carries its extension"
  );
  assert!(transcoder.transcoded.lock().unwrap().is_empty());
  assert_eq!(
    (media.mime.as_str(), media.size),
    ("video/quicktime", CLIP.len() as u64)
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_still_this_host_cannot_scale_fails_instead_of_uploading_as_it_came() {
  let dir = tempfile::tempdir().unwrap();
  let heic = boxed(b"ftyp", b"heic\0\0\0\0mif1heic");

  let declining = media_rig(Some(BoxScaler::declining()), false).await;
  let bare = media_rig(None, false).await;
  for (name, bytes) in [("photo.heic", heic.as_slice()), ("photo.jpg", JPEG)] {
    let undecoded = import(
      &declining.companion,
      picked(dir.path(), name, bytes),
      MediaPickKind::Image,
      None,
      None,
    )
    .await
    .expect_err("the scaler could not decode it");
    assert!(
      undecoded.to_string().starts_with("scale_failed:"),
      "{name}: got {undecoded}"
    );

    let unscaled = import(
      &bare.companion,
      picked(dir.path(), name, bytes),
      MediaPickKind::Image,
      None,
      None,
    )
    .await
    .expect_err("nothing can scale it");
    assert!(
      unscaled.to_string().starts_with("unsupported:"),
      "{name}: got {unscaled}"
    );
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_animated_image_passes_through_byte_for_byte_when_the_scaler_declines() {
  let rig = media_rig(Some(BoxScaler::declining()), false).await;
  let dir = tempfile::tempdir().unwrap();
  let animation = fixture::gif(2, 0);
  let source = picked(dir.path(), "loop.gif", &animation);

  let media = import(&rig.companion, source.clone(), MediaPickKind::Image, None, None)
    .await
    .expect("an animation the device can render needs no scaler");

  assert_eq!(
    (media.mime.as_str(), media.size, media.width),
    ("image/gif", animation.len() as u64, None)
  );
  assert_eq!(std::fs::read(&source).unwrap(), animation);
  let id = plant(&rig).await;
  let stored = put(&rig, id, "loop", &media.handle).await.expect("it uploads");
  assert_eq!(stored.sha256, sha_of(&animation));
}

fn padded(dir: &Path, name: &str, head: &[u8], size: u64) -> String {
  let path = picked(dir, name, head);
  std::fs::File::options()
    .write(true)
    .open(&path)
    .unwrap()
    .set_len(size)
    .unwrap();
  path
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pick_that_can_never_fit_is_refused_before_any_work_is_spent_on_it() {
  const HANDLE_MAX: u64 = bridgething_delivery::webapp::handles::HANDLE_MAX_BYTES;
  let scaler = Arc::new(BoxScaler::default());
  let transcoder = Arc::new(FixedTranscoder::default());
  let rig = transcoding_rig(Some(scaler.clone()), Some(transcoder.clone())).await;
  let dir = tempfile::tempdir().unwrap();
  let animation = fixture::gif(2, 0);

  let picks = [
    (
      padded(dir.path(), "huge.jpg", JPEG, 4 * HANDLE_MAX + 1),
      MediaPickKind::Image,
    ),
    (
      padded(dir.path(), "huge.gif", &animation, HANDLE_MAX + 1),
      MediaPickKind::Image,
    ),
    (
      padded(dir.path(), "archive.bin", b"PK", HANDLE_MAX + 1),
      MediaPickKind::Any,
    ),
  ];
  for (source, kind) in picks {
    let refused = import(&rig.companion, source.clone(), kind, None, None)
      .await
      .expect_err("it can never fit a handle");
    assert!(refused.to_string().starts_with("too_large:"), "{source}: got {refused}");
  }

  assert!(scaler.seen.lock().unwrap().is_empty(), "no still was read or scaled");
  assert!(
    transcoder.transcoded.lock().unwrap().is_empty(),
    "no video was transcoded"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_portrait_video_is_transcoded_at_a_bitrate_that_fits_a_handle() {
  let transcoder = Arc::new(FixedTranscoder::default());
  let rig = transcoding_rig(None, Some(transcoder.clone())).await;
  let dir = tempfile::tempdir().unwrap();

  for seconds in [120u32, 300] {
    let source = picked(dir.path(), "portrait.mov", &mp4(seconds, &[Track::Video, Track::Audio]));
    let media = import(&rig.companion, source, MediaPickKind::Video, None, None)
      .await
      .expect("a long clip imports instead of being refused up front");
    assert_eq!(media.mime, "video/mp4");

    let request = transcoder.transcoded.lock().unwrap().pop().expect("it was transcoded");
    let plan = bridgething_companion::media::plan_video(1080, 1920, 800, 480, request.max_bitrate);
    assert!(
      u64::from(plan.bitrate) * u64::from(seconds) / 8 <= HANDLE_MAX_BYTES,
      "{seconds} s at {} bps cannot fit a handle",
      plan.bitrate
    );
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transcode_that_still_comes_out_too_big_is_refused_and_leaves_nothing() {
  let transcoder = Arc::new(FixedTranscoder {
    output_bytes: Some(HANDLE_MAX_BYTES + 1),
    ..FixedTranscoder::default()
  });
  let rig = transcoding_rig(None, Some(transcoder.clone())).await;
  let dir = tempfile::tempdir().unwrap();
  let source = picked(dir.path(), "busy.mov", &mp4(240, &[Track::Video]));

  let refused = import(&rig.companion, source, MediaPickKind::Video, None, None)
    .await
    .expect_err("the encoder overshot the cap");
  assert!(refused.to_string().starts_with("too_large:"), "got {refused}");

  let requests = transcoder.transcoded.lock().unwrap().clone();
  assert_eq!(requests.len(), 2, "one refit at a lower ceiling before giving up");
  assert!(requests[1].max_bitrate < requests[0].max_bitrate);
  for request in requests {
    assert!(
      !PathBuf::from(request.destination).exists(),
      "no oversized output is left in scratch"
    );
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_any_pick_of_audio_in_a_video_container_is_not_stored_as_video() {
  let rig = media_rig(None, false).await;
  let dir = tempfile::tempdir().unwrap();

  for (name, bytes, mime) in [
    (
      "voice.webm",
      fixture::matroska("webm", 60_000, &[Track::Audio]),
      "audio/webm",
    ),
    (
      "memo.webm",
      fixture::recorded_matroska("webm", &[Track::Audio]),
      "audio/webm",
    ),
    (
      "album.mkv",
      fixture::matroska("matroska", 60_000, &[Track::Audio]),
      "audio/x-matroska",
    ),
    ("narrated.mp4", mp4(60, &[Track::Audio]), "audio/mp4"),
  ] {
    assert_eq!(any_mime(&rig, dir.path(), name, &bytes, None).await, mime, "{name}");
    assert_eq!(
      any_mime(&rig, dir.path(), name, &bytes, Some("video/webm")).await,
      mime,
      "{name}: a host that guessed video from the name is overruled by the track census"
    );
  }
  assert_eq!(
    any_mime(
      &rig,
      dir.path(),
      "voice.webm",
      &fixture::matroska("webm", 60_000, &[Track::Audio]),
      Some("audio/ogg")
    )
    .await,
    "audio/ogg",
    "a host hint that agrees there is no picture still outranks the container's guess"
  );
  assert_eq!(
    any_mime(
      &rig,
      dir.path(),
      "clip.webm",
      &fixture::matroska("webm", 60_000, &[Track::Video, Track::Audio]),
      None
    )
    .await,
    "video/webm"
  );
  assert_eq!(
    any_mime(
      &rig,
      dir.path(),
      "captions.mkv",
      &fixture::matroska("matroska", 60_000, &[Track::Text]),
      None
    )
    .await,
    "application/x-matroska"
  );
  assert_eq!(
    any_mime(&rig, dir.path(), "silent.mp4", &fixture::movie(60_000, &[]), None).await,
    "application/mp4"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_picked_still_and_its_preview_reach_the_scaler_already_sniffed() {
  let scaler = Arc::new(BoxScaler::default());
  let rig = media_rig(Some(scaler.clone()), false).await;
  let dir = tempfile::tempdir().unwrap();

  import(
    &rig.companion,
    picked(dir.path(), "photo.jpg", JPEG),
    MediaPickKind::Image,
    None,
    None,
  )
  .await
  .expect("the pick imports");

  let sniffed = scaler.sniffed.lock().unwrap().clone();
  assert_eq!(sniffed.len(), 2, "one cover and one thumbnail");
  for sniff in sniffed {
    assert_eq!(
      (sniff.kind, sniff.mime.as_deref()),
      (MediaKind::Image, Some("image/jpeg"))
    );
  }
}

fn handles_dir(rig: &Rig) -> PathBuf {
  rig.session.cache_dir().join("blob-handles")
}

fn held_files(rig: &Rig) -> usize {
  std::fs::read_dir(handles_dir(rig)).map_or(0, Iterator::count)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_abandoned_while_it_lands_leaves_no_handle_behind() {
  let gate = Arc::new(Gate::default());
  let transcoder = Arc::new(FixedTranscoder {
    poster_gate: Some(gate.clone()),
    ..FixedTranscoder::default()
  });
  let rig = transcoding_rig(Some(Arc::new(BoxScaler::default())), Some(transcoder.clone())).await;
  let dir = tempfile::tempdir().unwrap();
  let source = picked(dir.path(), "clip.mov", CLIP);
  let ticket = rig.companion.media_import_ticket();

  let importing = tokio::spawn({
    let companion = rig.companion.clone();
    async move {
      companion
        .import_picked_media(ticket, source, MediaPickKind::Video, None, None)
        .await
    }
  });
  gate.reached().await;
  assert_eq!(
    held_files(&rig),
    1,
    "the transcoded output waits in scratch for its handle"
  );

  rig.companion.cancel_media_import(ticket);
  let abandoned = importing.await.unwrap();
  assert!(matches!(abandoned, Err(CompanionError::Cancelled)), "{abandoned:?}");
  gate.opened();

  let deadline = Instant::now() + PUT_DEADLINE;
  while held_files(&rig) > 0 {
    assert!(
      Instant::now() < deadline,
      "the handle the abandoned import landed is still held: {:?}",
      std::fs::read_dir(handles_dir(&rig)).unwrap().collect::<Vec<_>>()
    );
    tokio::time::sleep(Duration::from_millis(5)).await;
  }
  assert_eq!(
    transcoder.postered.lock().unwrap().len(),
    1,
    "the poster was taken before the abandon landed"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_dropped_by_its_caller_is_cancelled_like_one_cancelled_by_ticket() {
  let gate = Arc::new(Gate::default());
  let transcoder = Arc::new(FixedTranscoder {
    poster_gate: Some(gate.clone()),
    ..FixedTranscoder::default()
  });
  let rig = transcoding_rig(Some(Arc::new(BoxScaler::default())), Some(transcoder.clone())).await;
  let dir = tempfile::tempdir().unwrap();
  let source = picked(dir.path(), "clip.mov", CLIP);

  let importing = tokio::spawn({
    let companion = rig.companion.clone();
    async move { import(&companion, source, MediaPickKind::Video, None, None).await }
  });
  gate.reached().await;

  importing.abort();
  assert!(importing.await.unwrap_err().is_cancelled());
  gate.opened();

  let deadline = Instant::now() + PUT_DEADLINE;
  while held_files(&rig) > 0 {
    assert!(Instant::now() < deadline, "the dropped import left a handle behind");
    tokio::time::sleep(Duration::from_millis(5)).await;
  }
}
