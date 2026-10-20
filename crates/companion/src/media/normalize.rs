use std::{
  fs::File,
  io::BufReader,
  path::{Path, PathBuf},
  sync::Arc,
};

use bridgething_delivery::webapp::handles::HANDLE_MAX_BYTES;
use tokio::sync::oneshot;

use super::{
  plan::{STILL_COVER, VIDEO_COVER, fitted_bitrate, passes_untouched, refitted_bitrate},
  sniff::{MediaKind, MediaProbe, probe},
};
use crate::backend::{
  ImageScaler, ImageTarget, MediaTranscoder, TranscodeJob, TranscodeRequest, TranscodeSink, TranscodedVideo,
};

const STILL_SOURCE_MAX_BYTES: u64 = 4 * HANDLE_MAX_BYTES;

#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
  pub fn path(&self) -> &Path {
    &self.0
  }
}

impl Drop for Scratch {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}

#[derive(Debug)]
pub enum Staged {
  Source(PathBuf),
  Scratch(Scratch),
}

#[derive(Debug)]
pub struct NormalizedMedia {
  pub staged: Staged,
  pub mime: Option<String>,
  pub width: Option<u32>,
  pub height: Option<u32>,
  pub duration_ms: Option<u64>,
}

impl NormalizedMedia {
  pub fn untouched(source: &Path, mime: Option<String>) -> Self {
    Self {
      staged: Staged::Source(source.to_owned()),
      mime,
      width: None,
      height: None,
      duration_ms: None,
    }
  }
}

#[derive(Debug, thiserror::Error)]
pub enum NormalizeError {
  #[error("io: {0}")]
  Io(#[from] std::io::Error),
  #[error("unsupported: this companion cannot scale images")]
  NoScaler,
  #[error("unsupported: this companion cannot transcode video")]
  NoTranscoder,
  #[error("scale_failed: this companion could not decode the image")]
  Scale,
  #[error("transcode_failed: {0}")]
  Transcode(String),
  #[error("too_large: the image is over the {max_bytes} byte limit")]
  StillTooLarge { max_bytes: u64 },
}

pub struct MediaNormalizer {
  image: Option<Arc<dyn ImageScaler>>,
  transcoder: Option<Arc<dyn MediaTranscoder>>,
  scratch: PathBuf,
}

impl MediaNormalizer {
  pub fn new(
    image: Option<Arc<dyn ImageScaler>>,
    transcoder: Option<Arc<dyn MediaTranscoder>>,
    scratch: PathBuf,
  ) -> Self {
    Self {
      image,
      transcoder,
      scratch,
    }
  }

  pub async fn normalize(
    &self,
    source: &Path,
    probed: &MediaProbe,
    cover: Option<(u32, u32)>,
  ) -> Result<NormalizedMedia, NormalizeError> {
    match probed.sniff.kind {
      MediaKind::Image => self.still(source, probed, cover.unwrap_or(STILL_COVER)).await,
      MediaKind::Video => self.video(source, probed, cover.unwrap_or(VIDEO_COVER)).await,
      MediaKind::Other => Ok(NormalizedMedia::untouched(source, None)),
    }
  }

  async fn still(
    &self,
    source: &Path,
    probed: &MediaProbe,
    cover: (u32, u32),
  ) -> Result<NormalizedMedia, NormalizeError> {
    let untouched =
      passes_untouched(&probed.sniff).then(|| NormalizedMedia::untouched(source, probed.sniff.mime.clone()));
    let max_bytes = if untouched.is_some() {
      HANDLE_MAX_BYTES
    } else {
      STILL_SOURCE_MAX_BYTES
    };
    if probed.size > max_bytes {
      return Err(NormalizeError::StillTooLarge { max_bytes });
    }
    let Some(scaler) = self.image.clone() else {
      return untouched.ok_or(NormalizeError::NoScaler);
    };
    let owned = source.to_owned();
    let bytes = blocking(move || std::fs::read(owned)).await?;
    let sniffed = probed.sniff.clone();
    let target = ImageTarget::Cover {
      width: cover.0,
      height: cover.1,
    };
    let scaled = tokio::task::spawn_blocking(move || scaler.scale(bytes, sniffed, target))
      .await
      .ok()
      .flatten();
    let Some(scaled) = scaled else {
      return untouched.ok_or(NormalizeError::Scale);
    };
    let scratch = self.scratch_file(extension(&scaled.mime));
    let dir = self.scratch.clone();
    let scratch = blocking(move || {
      std::fs::create_dir_all(dir)?;
      std::fs::write(scratch.path(), scaled.bytes)?;
      Ok(scratch)
    })
    .await?;
    Ok(NormalizedMedia {
      staged: Staged::Scratch(scratch),
      mime: Some(scaled.mime),
      width: Some(scaled.width),
      height: Some(scaled.height),
      duration_ms: None,
    })
  }

  async fn video(
    &self,
    source: &Path,
    probed: &MediaProbe,
    cover: (u32, u32),
  ) -> Result<NormalizedMedia, NormalizeError> {
    let transcoder = self.transcoder.clone().ok_or(NormalizeError::NoTranscoder)?;
    let asked = probed
      .movie
      .and_then(|movie| movie.duration_ms)
      .and_then(|duration_ms| fitted_bitrate(duration_ms, HANDLE_MAX_BYTES));
    let (mut scratch, mut video) = self.transcode(&transcoder, source, cover, asked).await?;
    let produced = blocking_size(scratch.path()).await?;
    if produced > HANDLE_MAX_BYTES
      && let Some(refit) = refitted_bitrate(asked, video.width, video.height, produced, HANDLE_MAX_BYTES)
    {
      drop(scratch);
      (scratch, video) = self.transcode(&transcoder, source, cover, Some(refit)).await?;
    }
    Ok(NormalizedMedia {
      staged: Staged::Scratch(scratch),
      mime: Some("video/mp4".to_owned()),
      width: Some(video.width),
      height: Some(video.height),
      duration_ms: Some(video.duration_ms),
    })
  }

  async fn transcode(
    &self,
    transcoder: &Arc<dyn MediaTranscoder>,
    source: &Path,
    cover: (u32, u32),
    max_bitrate: Option<u32>,
  ) -> Result<(Scratch, TranscodedVideo), NormalizeError> {
    let scratch = self.scratch_file("mp4");
    let dir = self.scratch.clone();
    blocking(move || std::fs::create_dir_all(dir)).await?;
    let request = TranscodeRequest {
      source: source.to_string_lossy().into_owned(),
      destination: scratch.path().to_string_lossy().into_owned(),
      cover_width: cover.0,
      cover_height: cover.1,
      max_bitrate,
    };
    let (sink, settled) = TranscodeSink::channel();
    let (_wanted, abandoned) = oneshot::channel::<()>();
    let transcoder = Arc::clone(transcoder);
    let transcoding = tokio::spawn(async move {
      let outcome = match tokio::task::spawn_blocking(move || transcoder.transcode(request, sink)).await {
        Ok(job) => transcoded(job, settled, abandoned).await,
        Err(error) => Err(error.to_string()),
      };
      (scratch, outcome)
    });
    let (scratch, outcome) = transcoding
      .await
      .map_err(|error| NormalizeError::Transcode(error.to_string()))?;
    Ok((scratch, outcome.map_err(NormalizeError::Transcode)?))
  }

  fn scratch_file(&self, extension: &str) -> Scratch {
    Scratch(self.scratch.join(format!("{}.{extension}", uuid::Uuid::now_v7())))
  }
}

async fn transcoded(
  job: Arc<dyn TranscodeJob>,
  mut settled: oneshot::Receiver<Result<TranscodedVideo, String>>,
  abandoned: oneshot::Receiver<()>,
) -> Result<TranscodedVideo, String> {
  let dropped = |_| Err("the transcoder dropped the request".to_owned());
  tokio::select! {
    outcome = &mut settled => return outcome.unwrap_or_else(dropped),
    _ = abandoned => {}
  }
  let _ = tokio::task::spawn_blocking(move || job.cancel()).await;
  settled.await.unwrap_or_else(dropped)
}

async fn blocking_size(path: &Path) -> Result<u64, NormalizeError> {
  let owned = path.to_owned();
  blocking(move || Ok(std::fs::metadata(owned)?.len())).await
}

pub async fn probe_file(source: &Path) -> Result<MediaProbe, NormalizeError> {
  let owned = source.to_owned();
  blocking(move || probe(&mut BufReader::new(File::open(owned)?))).await
}

fn extension(mime: &str) -> &str {
  match mime {
    "image/jpeg" => "jpg",
    "image/png" => "png",
    "image/gif" => "gif",
    "image/webp" => "webp",
    "image/avif" => "avif",
    _ => "bin",
  }
}

async fn blocking<T: Send + 'static>(
  work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, NormalizeError> {
  tokio::task::spawn_blocking(work)
    .await
    .map_err(std::io::Error::other)?
    .map_err(NormalizeError::from)
}

#[cfg(test)]
mod tests {
  use std::{
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
  };

  use super::*;
  use crate::{
    backend::ScaledImage,
    media::{
      fixture::{self, Track},
      plan::plan_video,
      sniff::MediaSniff,
    },
  };

  const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
  const SETTLES: Duration = Duration::from_secs(10);

  #[derive(Default)]
  struct Scaler {
    seen: Mutex<Vec<ImageTarget>>,
    sniffed: Mutex<Vec<MediaSniff>>,
    declines: bool,
  }

  impl ImageScaler for Scaler {
    fn scale(&self, bytes: Vec<u8>, sniff: MediaSniff, target: ImageTarget) -> Option<ScaledImage> {
      self.seen.lock().unwrap().push(target);
      self.sniffed.lock().unwrap().push(sniff);
      (!self.declines).then(|| ScaledImage {
        bytes: [bytes.as_slice(), b"scaled"].concat(),
        mime: "image/png".into(),
        width: 12,
        height: 34,
      })
    }
  }

  fn declining() -> Arc<Scaler> {
    Arc::new(Scaler {
      declines: true,
      ..Scaler::default()
    })
  }

  type Sizing = Box<dyn Fn(u32) -> u64 + Send + Sync>;

  enum Outcome {
    Complete,
    Fail,
    Drop,
    Panic,
    Held,
    Sized(u64, Sizing),
  }

  struct Transcoder {
    seen: Mutex<Vec<TranscodeRequest>>,
    outcome: Outcome,
    cancelled: Arc<Mutex<usize>>,
  }

  impl Transcoder {
    fn new(outcome: Outcome) -> Arc<Self> {
      Arc::new(Self {
        seen: Mutex::default(),
        outcome,
        cancelled: Arc::default(),
      })
    }

    fn cancels(&self) -> usize {
      *self.cancelled.lock().unwrap()
    }
  }

  struct Job {
    cancel: Mutex<Option<mpsc::Sender<()>>>,
    cancelled: Arc<Mutex<usize>>,
  }

  impl TranscodeJob for Job {
    fn cancel(&self) {
      *self.cancelled.lock().unwrap() += 1;
      if let Some(cancel) = self.cancel.lock().unwrap().take() {
        let _ = cancel.send(());
      }
    }
  }

  const LONG_CLIP_MS: u64 = 300_000;

  fn overshooting(ratio: f64, duration_ms: u64) -> Outcome {
    Outcome::Sized(
      duration_ms,
      Box::new(move |bitrate| (f64::from(bitrate) * ratio * duration_ms as f64 / 8000.0) as u64),
    )
  }

  fn used(request: &TranscodeRequest) -> u32 {
    plan_video(
      DONE.width,
      DONE.height,
      request.cover_width,
      request.cover_height,
      request.max_bitrate,
    )
    .bitrate
  }

  const DONE: TranscodedVideo = TranscodedVideo {
    width: 854,
    height: 480,
    duration_ms: 3000,
  };

  impl MediaTranscoder for Transcoder {
    fn transcode(&self, request: TranscodeRequest, sink: Arc<TranscodeSink>) -> Arc<dyn TranscodeJob> {
      std::fs::write(&request.destination, b"mp4").unwrap();
      self.seen.lock().unwrap().push(request);
      let (cancel, cancelled) = mpsc::channel();
      match &self.outcome {
        Outcome::Complete => sink.complete(DONE),
        Outcome::Fail => sink.fail("no decoder".into()),
        Outcome::Drop => drop(sink),
        Outcome::Panic => panic!("the encoder fell over"),
        Outcome::Held => {
          std::thread::spawn(move || {
            cancelled.recv().unwrap();
            sink.fail("cancelled".into());
          });
        }
        Outcome::Sized(duration_ms, sizing) => {
          let request = self.seen.lock().unwrap().last().unwrap().clone();
          File::options()
            .write(true)
            .open(&request.destination)
            .unwrap()
            .set_len(sizing(used(&request)))
            .unwrap();
          sink.complete(TranscodedVideo {
            duration_ms: *duration_ms,
            ..DONE
          });
        }
      }
      Arc::new(Job {
        cancel: Mutex::new(Some(cancel)),
        cancelled: Arc::clone(&self.cancelled),
      })
    }

    fn poster(&self, _source: String) -> Option<Vec<u8>> {
      None
    }
  }

  fn staged(media: &NormalizedMedia) -> &Path {
    match &media.staged {
      Staged::Source(path) => path,
      Staged::Scratch(scratch) => scratch.path(),
    }
  }

  struct Rig {
    dir: tempfile::TempDir,
  }

  impl Rig {
    fn new() -> Self {
      Self {
        dir: tempfile::tempdir().unwrap(),
      }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
      let path = self.dir.path().join(name);
      std::fs::write(&path, bytes).unwrap();
      path
    }

    fn padded(&self, name: &str, head: &[u8], size: u64) -> PathBuf {
      let path = self.file(name, head);
      File::options().write(true).open(&path).unwrap().set_len(size).unwrap();
      path
    }

    fn scratch(&self) -> PathBuf {
      self.dir.path().join("scratch")
    }

    fn scratched(&self) -> usize {
      std::fs::read_dir(self.scratch()).map_or(0, Iterator::count)
    }

    fn normalizer(&self, image: Option<Arc<Scaler>>, transcoder: Option<Arc<Transcoder>>) -> MediaNormalizer {
      MediaNormalizer::new(
        image.map(|scaler| scaler as Arc<dyn ImageScaler>),
        transcoder.map(|transcoder| transcoder as Arc<dyn MediaTranscoder>),
        self.scratch(),
      )
    }
  }

  async fn normalized(
    normalizer: &MediaNormalizer,
    source: &Path,
    cover: Option<(u32, u32)>,
  ) -> Result<NormalizedMedia, NormalizeError> {
    normalizer
      .normalize(source, &probe_file(source).await.unwrap(), cover)
      .await
  }

  fn is_source(media: &NormalizedMedia, source: &Path) -> bool {
    matches!(&media.staged, Staged::Source(path) if path == source)
  }

  fn mp4() -> Vec<u8> {
    let mut bytes = 16u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(b"ftypisom\0\0\0\0isom");
    bytes
  }

  #[tokio::test]
  async fn a_still_scales_to_the_default_box_into_scratch() {
    let rig = Rig::new();
    let scaler = Arc::new(Scaler::default());
    let source = rig.file("photo.jpg", JPEG);

    let out = normalized(&rig.normalizer(Some(scaler.clone()), None), &source, None)
      .await
      .unwrap();

    assert_eq!(
      *scaler.seen.lock().unwrap(),
      vec![ImageTarget::Cover {
        width: 800,
        height: 800
      }]
    );
    let path = staged(&out);
    assert!(matches!(out.staged, Staged::Scratch(_)));
    assert!(path.starts_with(rig.scratch()));
    assert_eq!(path.extension().unwrap(), "png");
    assert_eq!(std::fs::read(path).unwrap(), [JPEG, b"scaled"].concat());
    assert_eq!(
      (out.mime.as_deref(), out.width, out.height, out.duration_ms),
      (Some("image/png"), Some(12), Some(34), None)
    );
    assert_eq!(std::fs::read(&source).unwrap(), JPEG);
  }

  #[tokio::test]
  async fn a_cover_override_reaches_the_scaler() {
    let rig = Rig::new();
    let scaler = Arc::new(Scaler::default());
    let source = rig.file("photo.jpg", JPEG);

    normalized(&rig.normalizer(Some(scaler.clone()), None), &source, Some((320, 240)))
      .await
      .unwrap();

    assert_eq!(
      *scaler.seen.lock().unwrap(),
      vec![ImageTarget::Cover {
        width: 320,
        height: 240
      }]
    );
  }

  #[tokio::test]
  async fn a_still_nothing_could_scale_is_refused_instead_of_passing_through() {
    let rig = Rig::new();
    for (name, bytes) in [
      ("photo.jpg", JPEG.to_vec()),
      ("photo.heic", fixture::ftyp(b"heic", &[b"mif1", b"heic"])),
      ("still.gif", fixture::gif(1, 0)),
    ] {
      let source = rig.file(name, &bytes);

      let bare = normalized(&rig.normalizer(None, None), &source, None).await;
      assert!(matches!(bare, Err(NormalizeError::NoScaler)), "{name}: {bare:?}");

      let declined = normalized(&rig.normalizer(Some(declining()), None), &source, None).await;
      assert!(matches!(declined, Err(NormalizeError::Scale)), "{name}: {declined:?}");
      assert_eq!(rig.scratched(), 0);
    }
  }

  #[tokio::test]
  async fn an_animated_still_passes_through_untouched_when_nothing_scales_it() {
    let rig = Rig::new();
    let source = rig.file("loop.gif", &fixture::gif(2, 6000));

    for scaler in [None, Some(declining())] {
      let out = normalized(&rig.normalizer(scaler, None), &source, None).await.unwrap();
      assert!(is_source(&out, &source));
      assert_eq!((out.mime.as_deref(), out.width), (Some("image/gif"), None));
    }
  }

  #[tokio::test]
  async fn an_oversized_still_is_refused_before_it_is_read() {
    let rig = Rig::new();
    let scaler = Arc::new(Scaler::default());
    let normalizer = rig.normalizer(Some(scaler.clone()), None);

    let photo = rig.padded("huge.jpg", JPEG, STILL_SOURCE_MAX_BYTES + 1);
    let refused = normalized(&normalizer, &photo, None).await;
    assert!(
      matches!(refused, Err(NormalizeError::StillTooLarge { max_bytes }) if max_bytes == STILL_SOURCE_MAX_BYTES),
      "{refused:?}"
    );

    let animation = rig.padded("huge.gif", &fixture::gif(2, 0), HANDLE_MAX_BYTES + 1);
    let refused = normalized(&normalizer, &animation, None).await;
    assert!(
      matches!(refused, Err(NormalizeError::StillTooLarge { max_bytes }) if max_bytes == HANDLE_MAX_BYTES),
      "{refused:?}"
    );

    assert!(scaler.seen.lock().unwrap().is_empty());
    assert_eq!(rig.scratched(), 0);
  }

  #[tokio::test]
  async fn a_large_still_that_can_shrink_is_still_scaled() {
    let rig = Rig::new();
    let scaler = Arc::new(Scaler::default());
    let source = rig.padded("large.jpg", JPEG, HANDLE_MAX_BYTES + 1);

    let out = normalized(&rig.normalizer(Some(scaler.clone()), None), &source, None)
      .await
      .unwrap();

    assert_eq!(scaler.seen.lock().unwrap().len(), 1);
    assert!(matches!(out.staged, Staged::Scratch(_)));
  }

  #[tokio::test]
  async fn video_transcodes_to_an_mp4_in_scratch_at_the_video_box() {
    let rig = Rig::new();
    let transcoder = Transcoder::new(Outcome::Complete);
    let source = rig.file("clip.mov", &mp4());

    let out = normalized(&rig.normalizer(None, Some(transcoder.clone())), &source, None)
      .await
      .unwrap();

    let seen = transcoder.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].source, source.to_string_lossy());
    assert_eq!((seen[0].cover_width, seen[0].cover_height), (800, 480));
    let path = staged(&out);
    assert_eq!(PathBuf::from(&seen[0].destination), path);
    assert!(path.starts_with(rig.scratch()));
    assert_eq!(path.extension().unwrap(), "mp4");
    assert_eq!(
      (out.mime.as_deref(), out.width, out.height, out.duration_ms),
      (Some("video/mp4"), Some(854), Some(480), Some(3000))
    );
  }

  #[tokio::test]
  async fn video_without_a_transcoder_is_refused() {
    let rig = Rig::new();
    let source = rig.file("clip.mp4", &mp4());
    let refused = normalized(&rig.normalizer(Some(Arc::new(Scaler::default())), None), &source, None).await;
    assert!(matches!(refused, Err(NormalizeError::NoTranscoder)));
  }

  #[tokio::test]
  async fn a_long_video_is_transcoded_at_a_bitrate_that_fits_a_handle() {
    let rig = Rig::new();
    let transcoder = Transcoder::new(Outcome::Complete);
    let normalizer = rig.normalizer(None, Some(transcoder.clone()));

    for seconds in [120u64, 300, 600] {
      let source = rig.file(
        "portrait.mp4",
        &fixture::movie((seconds * 1000) as u32, &[Track::Video]),
      );
      normalized(&normalizer, &source, None).await.unwrap();
      let request = transcoder.seen.lock().unwrap().pop().unwrap();
      let ceiling = request.max_bitrate.expect("a clip of known length gets a ceiling");
      let plan = plan_video(
        1080,
        1920,
        request.cover_width,
        request.cover_height,
        request.max_bitrate,
      );
      assert_eq!(plan.bitrate, ceiling, "{seconds} s");
      assert!(u64::from(plan.bitrate) * seconds / 8 <= HANDLE_MAX_BYTES, "{seconds} s");
    }
  }

  fn asked(transcoder: &Transcoder) -> Vec<u32> {
    transcoder.seen.lock().unwrap().iter().map(used).collect()
  }

  #[tokio::test]
  async fn an_encode_that_overshoots_the_cap_is_redone_once_at_a_ceiling_scaled_by_the_miss() {
    let rig = Rig::new();
    let transcoder = Transcoder::new(overshooting(1.3, LONG_CLIP_MS));
    let normalizer = rig.normalizer(None, Some(transcoder.clone()));
    let source = rig.file("busy.mp4", &fixture::movie(LONG_CLIP_MS as u32, &[Track::Video]));

    let out = normalized(&normalizer, &source, None).await.unwrap();

    let used = asked(&transcoder);
    assert_eq!(used.len(), 2, "{used:?}");
    assert!(used[1] < used[0], "{used:?}");
    assert!(std::fs::metadata(staged(&out)).unwrap().len() <= HANDLE_MAX_BYTES);
    assert_eq!(rig.scratched(), 1, "the oversized first encode is gone");
  }

  #[tokio::test]
  async fn a_clip_whose_ceiling_sits_above_quality_is_refitted_below_what_it_used() {
    const SECONDS: u64 = 150;
    let rig = Rig::new();
    let transcoder = Transcoder::new(overshooting(1.5, SECONDS * 1000));
    let normalizer = rig.normalizer(None, Some(transcoder.clone()));
    let source = rig.file(
      "landscape.mp4",
      &fixture::movie((SECONDS * 1000) as u32, &[Track::Video]),
    );

    let out = normalized(&normalizer, &source, None).await.unwrap();

    let requests = transcoder.seen.lock().unwrap().clone();
    let ceiling = fitted_bitrate(SECONDS * 1000, HANDLE_MAX_BYTES).unwrap();
    assert_eq!(requests[0].max_bitrate, Some(ceiling));
    let quality = plan_video(DONE.width, DONE.height, 800, 480, None).bitrate;
    assert!(ceiling > quality, "quality, not the ceiling, set the first encode");
    let used = asked(&transcoder);
    assert_eq!(used.len(), 2, "{used:?}");
    assert_eq!(used[0], quality);
    assert!(
      used[1] < used[0],
      "the second encode is never the first again: {used:?}"
    );
    assert!(std::fs::metadata(staged(&out)).unwrap().len() <= HANDLE_MAX_BYTES);
  }

  #[tokio::test]
  async fn a_clip_of_unknown_length_that_comes_out_too_big_is_refitted_by_how_far_it_missed() {
    let rig = Rig::new();
    let transcoder = Transcoder::new(overshooting(1.0, LONG_CLIP_MS));
    let normalizer = rig.normalizer(None, Some(transcoder.clone()));
    let source = rig.file("recorded.webm", &fixture::recorded_matroska("webm", &[Track::Video]));

    let out = normalized(&normalizer, &source, None).await.unwrap();

    let requests = transcoder.seen.lock().unwrap().clone();
    assert_eq!(requests[0].max_bitrate, None);
    let used = asked(&transcoder);
    assert_eq!(used.len(), 2, "{used:?}");
    assert!(used[1] < used[0], "{used:?}");
    assert!(std::fs::metadata(staged(&out)).unwrap().len() <= HANDLE_MAX_BYTES);
  }

  #[tokio::test]
  async fn an_encoder_that_cannot_get_smaller_is_asked_twice_and_no_more() {
    let rig = Rig::new();
    let transcoder = Transcoder::new(Outcome::Sized(LONG_CLIP_MS, Box::new(|_| HANDLE_MAX_BYTES * 2)));
    let normalizer = rig.normalizer(None, Some(transcoder.clone()));
    let source = rig.file("noise.mp4", &fixture::movie(LONG_CLIP_MS as u32, &[Track::Video]));

    let out = normalized(&normalizer, &source, None).await.unwrap();

    assert_eq!(transcoder.seen.lock().unwrap().len(), 2);
    assert!(
      std::fs::metadata(staged(&out)).unwrap().len() > HANDLE_MAX_BYTES,
      "the oversized output goes on to the handle cap, which refuses it"
    );
    assert_eq!(rig.scratched(), 1);
  }

  fn lying_movie() -> Vec<u8> {
    [
      fixture::ftyp(b"isom", &[b"isom", b"mp42"]),
      fixture::moov(fixture::mvhd(1, 1, 10_000_000_000), &[Track::Video]),
    ]
    .concat()
  }

  #[tokio::test]
  async fn a_header_that_lies_about_its_length_cannot_starve_the_encode() {
    let rig = Rig::new();
    let floor = plan_video(DONE.width, DONE.height, 800, 480, Some(0)).bitrate;
    for (name, bytes) in [
      ("lying.mp4", lying_movie()),
      ("lying.webm", fixture::matroska("webm", u32::MAX - 1, &[Track::Video])),
    ] {
      let transcoder = Transcoder::new(overshooting(1.0, 20_000));
      let normalizer = rig.normalizer(None, Some(transcoder.clone()));

      let out = normalized(&normalizer, &rig.file(name, &bytes), None).await.unwrap();

      let request = transcoder.seen.lock().unwrap()[0].clone();
      assert!(
        request.max_bitrate.unwrap() < floor,
        "{name}: the header claimed a very long clip"
      );
      assert_eq!(asked(&transcoder), vec![floor], "{name}: encoded once, at the floor");
      assert!(
        std::fs::metadata(staged(&out)).unwrap().len() <= HANDLE_MAX_BYTES,
        "{name}"
      );
    }
  }

  #[tokio::test]
  async fn a_clip_too_long_to_fit_at_the_floor_is_encoded_once_and_left_for_the_cap_to_refuse() {
    const HOUR_MS: u64 = 3_600_000;
    let rig = Rig::new();
    let transcoder = Transcoder::new(overshooting(1.0, HOUR_MS));
    let normalizer = rig.normalizer(None, Some(transcoder.clone()));
    let source = rig.file("hour.mp4", &fixture::movie(HOUR_MS as u32, &[Track::Video]));

    let out = normalized(&normalizer, &source, None).await.unwrap();

    let floor = plan_video(DONE.width, DONE.height, 800, 480, Some(0)).bitrate;
    assert_eq!(asked(&transcoder), vec![floor]);
    assert!(std::fs::metadata(staged(&out)).unwrap().len() > HANDLE_MAX_BYTES);
  }

  #[tokio::test]
  async fn a_video_of_unknown_length_is_transcoded_without_a_ceiling() {
    let rig = Rig::new();
    let transcoder = Transcoder::new(Outcome::Complete);
    let normalizer = rig.normalizer(None, Some(transcoder.clone()));

    for (name, bytes) in [
      ("streamed.mp4", fixture::movie(u32::MAX, &[Track::Video])),
      ("bare.mp4", mp4()),
      ("recorded.webm", fixture::recorded_matroska("webm", &[Track::Video])),
    ] {
      normalized(&normalizer, &rig.file(name, &bytes), None).await.unwrap();
      assert_eq!(
        transcoder.seen.lock().unwrap().pop().unwrap().max_bitrate,
        None,
        "{name}"
      );
    }

    let webm = rig.file("clip.webm", &fixture::matroska("webm", 600_000, &[Track::Video]));
    normalized(&normalizer, &webm, None).await.unwrap();
    assert_eq!(
      transcoder.seen.lock().unwrap().pop().unwrap().max_bitrate,
      fitted_bitrate(600_000, HANDLE_MAX_BYTES)
    );
  }

  #[tokio::test]
  async fn a_failed_abandoned_or_panicked_transcode_errors_and_leaves_no_output() {
    for outcome in [Outcome::Fail, Outcome::Drop, Outcome::Panic] {
      let rig = Rig::new();
      let source = rig.file("clip.mp4", &mp4());
      let failed = normalized(&rig.normalizer(None, Some(Transcoder::new(outcome))), &source, None).await;
      assert!(matches!(failed, Err(NormalizeError::Transcode(_))));
      assert_eq!(rig.scratched(), 0);
    }
  }

  #[tokio::test]
  async fn output_nobody_adopts_is_removed() {
    let rig = Rig::new();
    let still = rig.file("photo.jpg", JPEG);
    let clip = rig.file("clip.mp4", &mp4());
    let normalizer = rig.normalizer(
      Some(Arc::new(Scaler::default())),
      Some(Transcoder::new(Outcome::Complete)),
    );

    let outputs = [
      normalized(&normalizer, &still, None).await.unwrap(),
      normalized(&normalizer, &clip, None).await.unwrap(),
    ];
    assert_eq!(rig.scratched(), 2);

    drop(outputs);
    assert_eq!(rig.scratched(), 0);
    assert_eq!(std::fs::read(&still).unwrap(), JPEG);
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn an_import_dropped_mid_transcode_cancels_it_and_leaves_no_output() {
    let rig = Rig::new();
    let source = rig.file("clip.mp4", &mp4());
    let transcoder = Transcoder::new(Outcome::Held);
    let normalizer = Arc::new(rig.normalizer(None, Some(transcoder.clone())));

    let importing = tokio::spawn({
      let normalizer = normalizer.clone();
      async move { normalized(&normalizer, &source, None).await.map(drop) }
    });
    let deadline = Instant::now() + SETTLES;
    while transcoder.seen.lock().unwrap().is_empty() {
      assert!(Instant::now() < deadline, "the transcoder never started");
      tokio::task::yield_now().await;
    }
    assert_eq!(rig.scratched(), 1, "the transcoder is writing its output");
    assert_eq!(transcoder.cancels(), 0);

    importing.abort();
    assert!(importing.await.unwrap_err().is_cancelled());

    while rig.scratched() > 0 {
      assert!(Instant::now() < deadline, "the abandoned output was never removed");
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(transcoder.cancels(), 1, "the abandoned transcode was told to stop");
  }

  #[tokio::test]
  async fn a_transcode_that_finishes_is_never_cancelled() {
    let rig = Rig::new();
    let transcoder = Transcoder::new(Outcome::Complete);
    let source = rig.file("clip.mp4", &mp4());
    let out = normalized(&rig.normalizer(None, Some(transcoder.clone())), &source, None)
      .await
      .unwrap();
    drop(out);
    assert_eq!(transcoder.cancels(), 0);
  }

  #[tokio::test]
  async fn a_still_reaches_the_scaler_with_the_sniff_already_taken() {
    let rig = Rig::new();
    let scaler = Arc::new(Scaler::default());
    let source = rig.file("photo.heic", &fixture::ftyp(b"heic", &[b"mif1", b"heic"]));

    normalized(&rig.normalizer(Some(scaler.clone()), None), &source, None)
      .await
      .unwrap();

    let sniffed = scaler.sniffed.lock().unwrap().clone();
    assert_eq!(sniffed.len(), 1);
    assert_eq!(
      (sniffed[0].kind, sniffed[0].mime.as_deref()),
      (MediaKind::Image, Some("image/heic"))
    );
  }

  #[tokio::test]
  async fn anything_else_passes_through_without_touching_a_backend() {
    let rig = Rig::new();
    let scaler = Arc::new(Scaler::default());
    let transcoder = Transcoder::new(Outcome::Complete);
    let source = rig.file("notes.pdf", b"%PDF-1.7");

    let out = normalized(
      &rig.normalizer(Some(scaler.clone()), Some(transcoder.clone())),
      &source,
      None,
    )
    .await
    .unwrap();

    assert!(is_source(&out, &source));
    assert_eq!((out.mime, out.width, out.duration_ms), (None, None, None));
    assert!(scaler.seen.lock().unwrap().is_empty());
    assert!(transcoder.seen.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn a_file_is_probed_whole_not_by_its_head() {
    let rig = Rig::new();

    let gif = rig.file("loop.gif", &fixture::gif(2, 6000));
    let probed = probe_file(&gif).await.unwrap();
    assert!(probed.sniff.animated, "the second frame sits past the first 4 KiB");
    assert_eq!(probed.size, std::fs::metadata(&gif).unwrap().len());

    let clip = rig.file("clip.mp4", &fixture::movie_at_the_end(90_000, &[Track::Video], 8192));
    let movie = probe_file(&clip).await.unwrap().movie.unwrap();
    assert_eq!((movie.duration_ms, movie.has_video), (Some(90_000), true));
  }

  #[tokio::test]
  async fn a_missing_source_is_an_io_error() {
    let rig = Rig::new();
    let missing = probe_file(&rig.dir.path().join("gone")).await;
    assert!(matches!(missing, Err(NormalizeError::Io(_))));
  }

  #[test]
  fn every_refusal_leads_with_a_token() {
    for error in [
      NormalizeError::Io(std::io::Error::other("gone")),
      NormalizeError::NoScaler,
      NormalizeError::NoTranscoder,
      NormalizeError::Scale,
      NormalizeError::Transcode("no decoder".into()),
      NormalizeError::StillTooLarge { max_bytes: 1 },
    ] {
      let message = error.to_string();
      let (token, _) = message.split_once(": ").expect("a token and a reason");
      assert!(
        token.chars().all(|part| part.is_ascii_lowercase() || part == '_'),
        "{message}"
      );
    }
  }
}
