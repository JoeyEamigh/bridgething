#![cfg(any(target_os = "linux", target_os = "windows"))]

use std::{
  path::{Path, PathBuf},
  process::Command,
};

use bridgething_companion::backend::{
  ImageTarget, MediaTranscoder, TranscodeRequest, TranscodeSink, TranscodedVideo, sniff_and_scale,
};
use bridgething_desktop::backends::{
  ffmpeg::{FfmpegTranscoder, Sidecar},
  portable::PortableScaler,
};
use image::{GenericImageView, ImageFormat};
use serde_json::Value;
use tempfile::TempDir;

const VIDEO_COVER: (u32, u32) = (800, 480);

fn sidecar() -> Sidecar {
  let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries").join(format!(
    "bridgething-ffmpeg-{}{}",
    env!("TAURI_ENV_TARGET_TRIPLE"),
    std::env::consts::EXE_SUFFIX
  ));
  Sidecar::at(path).expect("tauri-build refuses to build without the sidecar, so it is on disk")
}

fn tool(name: &str, args: &[&str]) -> Vec<u8> {
  let output = Command::new(name)
    .args(args)
    .output()
    .unwrap_or_else(|error| panic!("fixtures are synthesized with {name} from PATH: {error}"));
  assert!(
    output.status.success(),
    "{name} {args:?} failed: {}",
    String::from_utf8_lossy(&output.stderr)
  );
  output.stdout
}

fn synthesize(dir: &TempDir, name: &str, args: &[&str]) -> PathBuf {
  let path = dir.path().join(name);
  let mut full = vec!["-hide_banner", "-loglevel", "error", "-y"];
  full.extend_from_slice(args);
  full.push(path.to_str().unwrap());
  tool("ffmpeg", &full);
  path
}

fn transcode(source: &Path, destination: &Path) -> Result<TranscodedVideo, String> {
  transcode_under(source, destination, None)
}

fn transcode_under(source: &Path, destination: &Path, max_bitrate: Option<u32>) -> Result<TranscodedVideo, String> {
  let (sink, settled) = TranscodeSink::channel();
  FfmpegTranscoder::new(sidecar()).transcode(
    TranscodeRequest {
      source: source.to_string_lossy().into_owned(),
      destination: destination.to_string_lossy().into_owned(),
      cover_width: VIDEO_COVER.0,
      cover_height: VIDEO_COVER.1,
      max_bitrate,
    },
    sink,
  );
  settled.blocking_recv().expect("the transcoder settles the sink")
}

fn streams(path: &Path) -> Vec<Value> {
  let probed: Value = serde_json::from_slice(&tool(
    "ffprobe",
    &["-v", "error", "-show_streams", "-of", "json", path.to_str().unwrap()],
  ))
  .unwrap();
  probed["streams"].as_array().cloned().unwrap_or_default()
}

fn first_frame_rgb(path: &Path) -> Vec<u8> {
  tool(
    "ffmpeg",
    &[
      "-hide_banner",
      "-loglevel",
      "error",
      "-i",
      path.to_str().unwrap(),
      "-frames:v",
      "1",
      "-f",
      "rawvideo",
      "-pix_fmt",
      "rgb24",
      "pipe:1",
    ],
  )
}

fn fps(stream: &Value) -> f64 {
  let rate = stream["avg_frame_rate"].as_str().unwrap();
  let (num, den) = rate.split_once('/').unwrap();
  num.parse::<f64>().unwrap() / den.parse::<f64>().unwrap()
}

fn reddish(pixel: &[u8]) -> bool {
  pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60
}

#[test]
fn a_rotated_sixty_fps_clip_with_sound_lands_upright_silent_and_capped() {
  let dir = tempfile::tempdir().unwrap();
  let flat = synthesize(
    &dir,
    "flat.mp4",
    &[
      "-f",
      "lavfi",
      "-i",
      "color=c=blue:size=1920x1080:rate=60,drawbox=x=0:y=0:w=320:h=240:color=red:t=fill",
      "-f",
      "lavfi",
      "-i",
      "sine=frequency=440:sample_rate=48000",
      "-t",
      "2",
      "-c:v",
      "libx264",
      "-preset",
      "ultrafast",
      "-pix_fmt",
      "yuv420p",
      "-c:a",
      "aac",
      "-shortest",
    ],
  );
  let source = synthesize(
    &dir,
    "rotated.mp4",
    &["-display_rotation", "90", "-i", flat.to_str().unwrap(), "-c", "copy"],
  );
  let source_streams = streams(&source);
  assert!(source_streams.iter().any(|stream| stream["codec_type"] == "audio"));

  let destination = dir.path().join("normalized.mp4");
  let video = transcode(&source, &destination).unwrap();
  assert_eq!((video.width, video.height), (800, 1422));
  assert!((1900..=2100).contains(&video.duration_ms), "{} ms", video.duration_ms);

  let streams = streams(&destination);
  assert_eq!(streams.len(), 1, "{streams:?}");
  let stream = &streams[0];
  assert_eq!(stream["codec_type"], "video");
  assert_eq!(stream["codec_name"], "h264");
  assert_eq!(stream["profile"], "High");
  assert_eq!(stream["pix_fmt"], "yuv420p");
  assert_eq!(
    (stream["width"].as_u64(), stream["height"].as_u64()),
    (Some(800), Some(1422))
  );
  assert_eq!(stream["color_primaries"], "bt709");
  assert_eq!(stream["color_transfer"], "bt709");
  assert_eq!(stream["color_space"], "bt709");
  assert!(fps(stream) <= 30.0, "{}", fps(stream));
  assert!(
    stream["side_data_list"]
      .as_array()
      .is_none_or(|list| list.iter().all(|datum| datum.get("rotation").is_none())),
    "{stream:?}"
  );

  let frame = first_frame_rgb(&destination);
  let at = |x: usize, y: usize| &frame[(y * 800 + x) * 3..][..3];
  assert!(
    reddish(at(20, 1400)),
    "the red corner should sit bottom left once upright: {:?}",
    at(20, 1400)
  );
  assert!(!reddish(at(780, 20)));
}

#[test]
fn a_poster_is_the_upright_first_frame_the_scaler_can_thumbnail() {
  let dir = tempfile::tempdir().unwrap();
  let flat = synthesize(
    &dir,
    "flat.mp4",
    &[
      "-f",
      "lavfi",
      "-i",
      "color=c=blue:size=1280x720:rate=30,drawbox=x=0:y=0:w=320:h=240:color=red:t=fill",
      "-t",
      "1",
      "-c:v",
      "libx264",
      "-preset",
      "ultrafast",
      "-pix_fmt",
      "yuv420p",
    ],
  );
  let source = synthesize(
    &dir,
    "rotated.mp4",
    &["-display_rotation", "90", "-i", flat.to_str().unwrap(), "-c", "copy"],
  );

  let poster = FfmpegTranscoder::new(sidecar())
    .poster(source.to_string_lossy().into_owned())
    .expect("a poster");

  let decoded = image::load_from_memory(&poster).unwrap();
  assert_eq!(decoded.dimensions(), (720, 1280), "the poster is upright");
  let rgb = decoded.to_rgb8();
  assert!(reddish(&rgb.get_pixel(10, 1270).0), "{:?}", rgb.get_pixel(10, 1270));
  assert!(!reddish(&rgb.get_pixel(710, 10).0));
  let thumbnail = sniff_and_scale(
    &PortableScaler::new(Some(sidecar())),
    poster,
    ImageTarget::Thumbnail {
      max_edge: 320,
      quality: 0.7,
    },
  )
  .expect("the scaler thumbnails the poster");
  assert_eq!(thumbnail.mime, "image/jpeg");
  assert_eq!(thumbnail.width.max(thumbnail.height), 320);
}

#[test]
fn a_poster_of_something_that_is_not_video_is_none() {
  let dir = tempfile::tempdir().unwrap();
  let decoy = dir.path().join("decoy.mp4");
  std::fs::write(&decoy, b"plainly not a movie").unwrap();
  let transcoder = FfmpegTranscoder::new(sidecar());
  assert_eq!(transcoder.poster(decoy.to_string_lossy().into_owned()), None);
  assert_eq!(
    transcoder.poster(dir.path().join("missing.mp4").to_string_lossy().into_owned()),
    None
  );
}

#[test]
fn a_slow_clip_keeps_its_rate_and_size() {
  let dir = tempfile::tempdir().unwrap();
  let source = synthesize(
    &dir,
    "small.mkv",
    &[
      "-f",
      "lavfi",
      "-i",
      "testsrc2=size=640x360:rate=24",
      "-t",
      "1",
      "-c:v",
      "libvpx-vp9",
      "-deadline",
      "realtime",
    ],
  );
  let destination = dir.path().join("normalized.mp4");
  let video = transcode(&source, &destination).unwrap();
  assert_eq!((video.width, video.height), (640, 360));
  let streams = streams(&destination);
  assert_eq!(streams.len(), 1);
  assert!((fps(&streams[0]) - 24.0).abs() < 0.01, "{}", fps(&streams[0]));
}

#[test]
fn an_hdr_clip_comes_out_tagged_bt709_sdr() {
  let dir = tempfile::tempdir().unwrap();
  let source = synthesize(
    &dir,
    "hlg.mov",
    &[
      "-f",
      "lavfi",
      "-i",
      "testsrc2=size=1280x720:rate=30",
      "-t",
      "1",
      "-c:v",
      "libx265",
      "-preset",
      "ultrafast",
      "-pix_fmt",
      "yuv420p10le",
      "-x265-params",
      "log-level=error:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc",
      "-color_primaries",
      "bt2020",
      "-color_trc",
      "arib-std-b67",
      "-colorspace",
      "bt2020nc",
      "-tag:v",
      "hvc1",
    ],
  );
  let destination = dir.path().join("normalized.mp4");
  let video = transcode(&source, &destination).unwrap();
  assert_eq!((video.width, video.height), (854, 480));
  let stream = &streams(&destination)[0];
  assert_eq!(stream["pix_fmt"], "yuv420p");
  assert_eq!(stream["color_primaries"], "bt709");
  assert_eq!(stream["color_transfer"], "bt709");
}

#[test]
fn a_source_the_sidecar_cannot_read_fails_and_leaves_nothing_behind() {
  let dir = tempfile::tempdir().unwrap();
  let source = dir.path().join("broken.mp4");
  std::fs::write(
    &source,
    b"\0\0\0\x18ftypisom\0\0\0\0isomavc1 then nothing a demuxer could use",
  )
  .unwrap();
  let destination = dir.path().join("normalized.mp4");
  let reason = transcode(&source, &destination).unwrap_err();
  assert!(!reason.is_empty());
  assert!(!destination.exists());
}

fn bitrate(path: &Path) -> f64 {
  let stream = streams(path).into_iter().next().expect("a video stream");
  stream["bit_rate"].as_str().unwrap().parse().unwrap()
}

#[test]
fn a_bitrate_ceiling_keeps_a_detailed_clip_under_it() {
  let dir = tempfile::tempdir().unwrap();
  let grainy = synthesize(
    &dir,
    "grainy.mp4",
    &[
      "-f",
      "lavfi",
      "-i",
      "testsrc2=size=960x540:rate=30:duration=4,noise=alls=10:allf=t",
      "-c:v",
      "libx264",
      "-crf",
      "12",
      "-pix_fmt",
      "yuv420p",
    ],
  );
  const CEILING: u32 = 300_000;
  let free = dir.path().join("free.mp4");
  let capped = dir.path().join("capped.mp4");
  transcode_under(&grainy, &free, None).expect("the free encode");
  transcode_under(&grainy, &capped, Some(CEILING)).expect("the capped encode");

  let (free, capped) = (bitrate(&free), bitrate(&capped));
  assert!(
    capped <= f64::from(CEILING) * 1.25,
    "the encoder overshot its ceiling: {capped} bps"
  );
  assert!(
    capped * 2.0 < free,
    "the grain is detailed enough that the ceiling bound the encode: {capped} vs {free}"
  );
}

#[test]
fn a_tiled_rotated_heic_lands_as_an_upright_jpeg() {
  let heic = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grid-rotated.heic")).unwrap();
  let scaled = sniff_and_scale(
    &PortableScaler::new(Some(sidecar())),
    heic,
    ImageTarget::Cover {
      width: 800,
      height: 800,
    },
  )
  .expect("the sidecar decodes heic");
  assert_eq!(
    (scaled.width, scaled.height, scaled.mime.as_str()),
    (770, 1030, "image/jpeg")
  );
  let decoded = image::load_from_memory_with_format(&scaled.bytes, ImageFormat::Jpeg).unwrap();
  assert_eq!(decoded.dimensions(), (770, 1030));
  let rgb = decoded.to_rgb8();
  assert!(reddish(&rgb.get_pixel(760, 10).0), "{:?}", rgb.get_pixel(760, 10));
  assert!(!reddish(&rgb.get_pixel(10, 10).0));
  assert!(!reddish(&rgb.get_pixel(760, 1020).0));
}

fn avif_still(dir: &TempDir, size: &str) -> Vec<u8> {
  let source = synthesize(
    dir,
    "still.avif",
    &[
      "-f",
      "lavfi",
      "-i",
      &format!("color=c=red:size={size}"),
      "-frames:v",
      "1",
      "-c:v",
      "libaom-av1",
      "-still-picture",
      "1",
      "-cpu-used",
      "8",
      "-f",
      "avif",
    ],
  );
  std::fs::read(source).unwrap()
}

#[test]
fn a_small_avif_still_is_kept_and_still_gets_a_thumbnail() {
  let dir = tempfile::tempdir().unwrap();
  let avif = avif_still(&dir, "640x480");
  let scaler = PortableScaler::new(Some(sidecar()));
  let kept = sniff_and_scale(
    &scaler,
    avif.clone(),
    ImageTarget::Cover {
      width: 800,
      height: 800,
    },
  )
  .expect("the sidecar decodes avif");
  assert_eq!(kept.bytes, avif);
  assert_eq!((kept.width, kept.height, kept.mime.as_str()), (640, 480, "image/avif"));

  let thumbnail = sniff_and_scale(
    &scaler,
    kept.bytes,
    ImageTarget::Thumbnail {
      max_edge: 320,
      quality: 0.7,
    },
  )
  .expect("the kept avif has a thumbnail");
  assert_eq!(
    (thumbnail.width, thumbnail.height, thumbnail.mime.as_str()),
    (320, 240, "image/jpeg")
  );
  let rgb = image::load_from_memory(&thumbnail.bytes).unwrap().to_rgb8();
  assert!(reddish(&rgb.get_pixel(160, 120).0));
}

#[test]
fn a_large_avif_still_is_decoded_through_the_sidecar() {
  let dir = tempfile::tempdir().unwrap();
  let scaled = sniff_and_scale(
    &PortableScaler::new(Some(sidecar())),
    avif_still(&dir, "1600x1200"),
    ImageTarget::Cover {
      width: 800,
      height: 800,
    },
  )
  .expect("the sidecar decodes avif");
  assert_eq!(
    (scaled.width, scaled.height, scaled.mime.as_str()),
    (1067, 800, "image/jpeg")
  );
  let rgb = image::load_from_memory(&scaled.bytes).unwrap().to_rgb8();
  assert!(reddish(&rgb.get_pixel(500, 400).0));
}
