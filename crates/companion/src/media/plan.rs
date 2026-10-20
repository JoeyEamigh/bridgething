use super::sniff::{MediaFormat, MediaSniff};

pub const STILL_COVER: (u32, u32) = (800, 800);
pub const VIDEO_COVER: (u32, u32) = (800, 480);
const STILL_QUALITY: f32 = 0.85;
const MAX_FPS: u32 = 30;
const KEYFRAME_SECONDS: u32 = 2;
const BITS_PER_PIXEL: f64 = 0.1;
const FLOOR_BITS_PER_PIXEL: f64 = 0.01;
const FIT_SHARE: f64 = 0.85;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum StillFormat {
  Jpeg,
  Png,
  Gif,
  Webp,
}

impl StillFormat {
  pub fn mime(self) -> &'static str {
    match self {
      StillFormat::Jpeg => "image/jpeg",
      StillFormat::Png => "image/png",
      StillFormat::Gif => "image/gif",
      StillFormat::Webp => "image/webp",
    }
  }
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum CoverPlan {
  Keep,
  Encode {
    width: u32,
    height: u32,
    format: StillFormat,
    mime: String,
    quality: f32,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct VideoPlan {
  pub width: u32,
  pub height: u32,
  pub max_fps: u32,
  pub bitrate: u32,
  pub fitted: bool,
  pub keyframe_seconds: u32,
}

pub fn passes_untouched(sniffed: &MediaSniff) -> bool {
  sniffed.animated && renderable(sniffed.format)
}

fn renderable(format: MediaFormat) -> bool {
  preserved(format).is_some() || format == MediaFormat::Avif
}

fn preserved(format: MediaFormat) -> Option<StillFormat> {
  match format {
    MediaFormat::Jpeg => Some(StillFormat::Jpeg),
    MediaFormat::Png => Some(StillFormat::Png),
    MediaFormat::Gif => Some(StillFormat::Gif),
    MediaFormat::Webp => Some(StillFormat::Webp),
    _ => None,
  }
}

#[uniffi::export]
pub fn plan_cover(
  format: MediaFormat,
  width: u32,
  height: u32,
  has_alpha: bool,
  cover_width: u32,
  cover_height: u32,
  encodable: Vec<StillFormat>,
) -> CoverPlan {
  let scale = cover_scale(width, height, cover_width, cover_height);
  let fitted = (
    scaled(width, scale).clamp(1, width.max(1)),
    scaled(height, scale).clamp(1, height.max(1)),
  );
  if renderable(format) && fitted == (width, height) {
    return CoverPlan::Keep;
  }
  let fallback = if has_alpha { StillFormat::Png } else { StillFormat::Jpeg };
  let chosen = preserved(format)
    .filter(|wanted| encodable.contains(wanted))
    .unwrap_or(fallback);
  CoverPlan::Encode {
    width: fitted.0,
    height: fitted.1,
    format: chosen,
    mime: chosen.mime().to_owned(),
    quality: STILL_QUALITY,
  }
}

pub fn fitted_bitrate(duration_ms: u64, max_bytes: u64) -> Option<u32> {
  (duration_ms > 0).then(|| {
    let bits = max_bytes as f64 * 8.0 * FIT_SHARE;
    (bits * 1000.0 / duration_ms as f64)
      .floor()
      .clamp(1.0, f64::from(u32::MAX)) as u32
  })
}

pub fn refitted_bitrate(
  asked: Option<u32>,
  width: u32,
  height: u32,
  produced_bytes: u64,
  max_bytes: u64,
) -> Option<u32> {
  let used = video_bitrate(width, height, asked).0;
  let shrink = max_bytes as f64 * FIT_SHARE / produced_bytes.max(1) as f64;
  let ceiling = (f64::from(used) * shrink).floor().clamp(0.0, f64::from(u32::MAX)) as u32;
  let refit = video_bitrate(width, height, Some(ceiling)).0;
  (refit < used).then_some(refit)
}

#[uniffi::export]
pub fn plan_video(width: u32, height: u32, cover_width: u32, cover_height: u32, max_bitrate: Option<u32>) -> VideoPlan {
  let scale = cover_scale(width, height, cover_width, cover_height);
  let even = |edge: u32| {
    let rounded = ((f64::from(edge) * scale / 2.0).round() as u32) * 2;
    rounded.min(edge - edge % 2).max(2)
  };
  let (width, height) = (even(width), even(height));
  let (bitrate, fitted) = video_bitrate(width, height, max_bitrate);
  VideoPlan {
    width,
    height,
    max_fps: MAX_FPS,
    bitrate,
    fitted,
    keyframe_seconds: KEYFRAME_SECONDS,
  }
}

fn video_bitrate(width: u32, height: u32, ceiling: Option<u32>) -> (u32, bool) {
  let bits = |per_pixel: f64| (f64::from(width) * f64::from(height) * f64::from(MAX_FPS) * per_pixel).round() as u32;
  let quality = bits(BITS_PER_PIXEL);
  let floor = bits(FLOOR_BITS_PER_PIXEL).clamp(1, quality.max(1));
  let bitrate = ceiling.map_or(quality, |ceiling| quality.min(ceiling.max(floor)));
  (bitrate, bitrate < quality)
}

fn cover_scale(width: u32, height: u32, cover_width: u32, cover_height: u32) -> f64 {
  if width == 0 || height == 0 {
    return 1.0;
  }
  let scale = (f64::from(cover_width) / f64::from(width)).max(f64::from(cover_height) / f64::from(height));
  if scale > 0.0 { scale.min(1.0) } else { 1.0 }
}

fn scaled(edge: u32, scale: f64) -> u32 {
  (f64::from(edge) * scale).round() as u32
}

#[cfg(test)]
mod tests {
  use super::*;

  const EVERYTHING: [StillFormat; 4] = [StillFormat::Jpeg, StillFormat::Png, StillFormat::Gif, StillFormat::Webp];

  fn encode(plan: CoverPlan) -> (u32, u32, StillFormat) {
    match plan {
      CoverPlan::Encode {
        width, height, format, ..
      } => (width, height, format),
      CoverPlan::Keep => panic!("expected an encode, got a keep"),
    }
  }

  #[test]
  fn a_large_still_shrinks_until_its_short_edge_meets_the_box() {
    assert_eq!(
      encode(plan_cover(
        MediaFormat::Jpeg,
        4032,
        3024,
        false,
        800,
        800,
        EVERYTHING.to_vec()
      )),
      (1067, 800, StillFormat::Jpeg)
    );
    assert_eq!(
      encode(plan_cover(
        MediaFormat::Png,
        3024,
        4032,
        true,
        800,
        800,
        EVERYTHING.to_vec()
      )),
      (800, 1067, StillFormat::Png)
    );
  }

  #[test]
  fn the_cover_box_is_covered_on_both_axes() {
    let (width, height, _) = encode(plan_cover(
      MediaFormat::Jpeg,
      3000,
      1000,
      false,
      800,
      480,
      EVERYTHING.to_vec(),
    ));
    assert_eq!((width, height), (1440, 480));
    let (width, height, _) = encode(plan_cover(
      MediaFormat::Jpeg,
      1000,
      3000,
      false,
      800,
      480,
      EVERYTHING.to_vec(),
    ));
    assert_eq!((width, height), (800, 2400));
  }

  #[test]
  fn a_renderable_still_that_already_fits_is_kept_untouched() {
    for format in [
      MediaFormat::Jpeg,
      MediaFormat::Png,
      MediaFormat::Gif,
      MediaFormat::Webp,
      MediaFormat::Avif,
    ] {
      assert_eq!(
        plan_cover(format, 640, 480, false, 800, 800, EVERYTHING.to_vec()),
        CoverPlan::Keep
      );
    }
    assert_eq!(
      plan_cover(MediaFormat::Png, 1000, 400, true, 800, 800, EVERYTHING.to_vec()),
      CoverPlan::Keep
    );
  }

  #[test]
  fn heif_always_becomes_jpeg_even_when_small() {
    assert_eq!(
      encode(plan_cover(
        MediaFormat::Heif,
        640,
        480,
        false,
        800,
        800,
        EVERYTHING.to_vec()
      )),
      (640, 480, StillFormat::Jpeg)
    );
    assert_eq!(
      encode(plan_cover(
        MediaFormat::Heif,
        4032,
        3024,
        false,
        800,
        800,
        EVERYTHING.to_vec()
      )),
      (1067, 800, StillFormat::Jpeg)
    );
  }

  #[test]
  fn an_unrenderable_still_falls_back_on_alpha() {
    assert_eq!(
      encode(plan_cover(
        MediaFormat::Tiff,
        100,
        100,
        true,
        800,
        800,
        EVERYTHING.to_vec()
      ))
      .2,
      StillFormat::Png
    );
    assert_eq!(
      encode(plan_cover(
        MediaFormat::Bmp,
        100,
        100,
        false,
        800,
        800,
        EVERYTHING.to_vec()
      ))
      .2,
      StillFormat::Jpeg
    );
    assert_eq!(
      encode(plan_cover(
        MediaFormat::Avif,
        2000,
        2000,
        true,
        800,
        800,
        EVERYTHING.to_vec()
      ))
      .2,
      StillFormat::Png
    );
  }

  #[test]
  fn a_format_the_platform_cannot_encode_falls_back() {
    let apple = vec![StillFormat::Jpeg, StillFormat::Png, StillFormat::Gif];
    assert_eq!(
      encode(plan_cover(MediaFormat::Webp, 2000, 2000, true, 800, 800, apple.clone())).2,
      StillFormat::Png
    );
    assert_eq!(
      encode(plan_cover(MediaFormat::Webp, 2000, 2000, false, 800, 800, apple)).2,
      StillFormat::Jpeg
    );
  }

  #[test]
  fn the_encode_carries_its_mime_and_quality() {
    match plan_cover(MediaFormat::Gif, 2000, 1000, true, 800, 800, EVERYTHING.to_vec()) {
      CoverPlan::Encode { mime, quality, .. } => {
        assert_eq!(mime, "image/gif");
        assert_eq!(quality, STILL_QUALITY);
      }
      CoverPlan::Keep => panic!("a 2000px gif still must shrink"),
    }
  }

  #[test]
  fn video_matches_the_design_examples() {
    let landscape = plan_video(1920, 1080, 800, 480, None);
    assert_eq!((landscape.width, landscape.height), (854, 480));
    let portrait = plan_video(1080, 1920, 800, 480, None);
    assert_eq!((portrait.width, portrait.height), (800, 1422));
    assert_eq!((landscape.max_fps, landscape.keyframe_seconds), (30, 2));
    assert_eq!(landscape.bitrate, 1_229_760);
  }

  #[test]
  fn a_long_clip_gets_a_bitrate_that_fits_the_cap() {
    const CAP: u64 = 32 * 1024 * 1024;
    for seconds in [120u64, 300, 600] {
      let ceiling = fitted_bitrate(seconds * 1000, CAP);
      let portrait = plan_video(1080, 1920, 800, 480, ceiling);
      assert!(u64::from(portrait.bitrate) * seconds / 8 <= CAP, "{seconds} s");
      assert_eq!(
        Some(portrait.bitrate),
        ceiling,
        "{seconds} s is long enough to need the ceiling"
      );
      assert!(portrait.fitted);
    }
    let short = plan_video(1080, 1920, 800, 480, fitted_bitrate(20_000, CAP));
    assert_eq!(short.bitrate, plan_video(1080, 1920, 800, 480, None).bitrate);
    assert!(!short.fitted);
  }

  #[test]
  fn a_clip_too_long_to_fit_at_the_floor_is_planned_at_the_floor() {
    const CAP: u64 = 32 * 1024 * 1024;
    let hour = plan_video(1080, 1920, 800, 480, fitted_bitrate(3_600_000, CAP));
    assert_eq!(hour.bitrate, floor(PORTRAIT));
    assert!(
      u64::from(hour.bitrate) * 3600 / 8 > CAP,
      "the cap refuses it once encoded"
    );
  }

  const LANDSCAPE: (u32, u32) = (854, 480);
  const PORTRAIT: (u32, u32) = (800, 1422);

  fn quality((width, height): (u32, u32)) -> u32 {
    plan_video(width, height, 800, 480, None).bitrate
  }

  fn floor((width, height): (u32, u32)) -> u32 {
    plan_video(width, height, 800, 480, Some(0)).bitrate
  }

  #[test]
  fn an_overshoot_is_refitted_by_how_far_it_missed() {
    const CAP: u64 = 1_000_000;
    let (width, height) = LANDSCAPE;
    let ceiling = 600_000;
    assert_eq!(
      refitted_bitrate(Some(ceiling), width, height, 1_700_000, CAP),
      Some(300_000)
    );
    assert_eq!(
      refitted_bitrate(None, width, height, 2 * CAP, CAP),
      Some((f64::from(quality(LANDSCAPE)) * (CAP as f64 * 0.85 / (2 * CAP) as f64)).floor() as u32),
      "a clip of unknown length is refitted from how far its quality encode missed"
    );
    assert_eq!(
      refitted_bitrate(Some(ceiling), width, height, 800_000, CAP),
      None,
      "an encode that already fits is never asked for more"
    );
  }

  #[test]
  fn a_refit_scales_the_bitrate_the_encode_used_not_the_ceiling_it_was_offered() {
    const CAP: u64 = 32 * 1024 * 1024;
    const SECONDS: u64 = 150;
    let (width, height) = LANDSCAPE;
    let asked = fitted_bitrate(SECONDS * 1000, CAP).unwrap();
    let first = plan_video(width, height, 800, 480, Some(asked));
    assert!(
      asked > first.bitrate,
      "the ceiling sits above quality, so quality set the bitrate"
    );
    assert!(!first.fitted);
    let produced = u64::from(first.bitrate) * 3 / 2 * SECONDS / 8;
    assert!(produced > CAP);

    let refit = refitted_bitrate(Some(asked), width, height, produced, CAP).expect("an overshoot is refitted");
    let second = plan_video(width, height, 800, 480, Some(refit));
    assert!(
      second.bitrate < first.bitrate,
      "the second encode asks for less than the first used: {} vs {}",
      second.bitrate,
      first.bitrate
    );
    assert!(second.fitted);
  }

  #[test]
  fn a_second_encode_never_repeats_the_first() {
    const CAP: u64 = 32 * 1024 * 1024;
    for frame in [LANDSCAPE, PORTRAIT] {
      for asked in [
        None,
        Some(1),
        Some(floor(frame)),
        Some(quality(frame) / 2),
        Some(quality(frame)),
        Some(u32::MAX),
      ] {
        for produced in [CAP + 1, CAP * 2, CAP * 50, u64::MAX] {
          let used = plan_video(frame.0, frame.1, 800, 480, asked).bitrate;
          if let Some(refit) = refitted_bitrate(asked, frame.0, frame.1, produced, CAP) {
            let next = plan_video(frame.0, frame.1, 800, 480, Some(refit)).bitrate;
            assert!(next < used, "{frame:?} {asked:?} {produced}: {next} after {used}");
            assert!(
              next >= floor(frame),
              "{frame:?} {asked:?} {produced}: {next} under the floor"
            );
          }
        }
      }
    }
  }

  #[test]
  fn an_encode_already_at_the_floor_is_not_redone() {
    const CAP: u64 = 32 * 1024 * 1024;
    let (width, height) = PORTRAIT;
    assert_eq!(refitted_bitrate(Some(1), width, height, CAP * 4, CAP), None);
    assert_eq!(
      refitted_bitrate(Some(floor(PORTRAIT)), width, height, CAP * 4, CAP),
      None
    );
  }

  #[test]
  fn the_ceiling_never_drops_below_a_bitrate_the_encoders_can_honor() {
    assert_eq!(floor(LANDSCAPE), 122_976);
    assert_eq!(floor(PORTRAIT), 341_280);
    for ceiling in [0, 1, 1000, 100_000] {
      let plan = plan_video(1920, 1080, 800, 480, Some(ceiling));
      assert_eq!(plan.bitrate, floor(LANDSCAPE), "{ceiling}");
      assert!(plan.fitted, "{ceiling}");
    }
    let tiny = plan_video(1, 1, 800, 480, Some(0));
    assert_eq!(tiny.bitrate, 1);
    let lied = fitted_bitrate(1000 * 60 * 60 * 1000, 32 * 1024 * 1024);
    assert_eq!(plan_video(1080, 1920, 800, 480, lied).bitrate, floor(PORTRAIT));
  }

  #[test]
  fn the_fitted_bitrate_spends_most_of_the_cap_and_never_asks_for_nothing() {
    assert_eq!(fitted_bitrate(1000, 1000), Some(6800));
    assert_eq!(fitted_bitrate(0, 1000), None);
    assert_eq!(fitted_bitrate(u64::MAX, 1), Some(1));
    assert_eq!(fitted_bitrate(1, u64::MAX), Some(u32::MAX));
  }

  #[test]
  fn a_plan_says_whether_the_cap_or_quality_set_its_bitrate() {
    assert!(!plan_video(1920, 1080, 800, 480, None).fitted);
    assert!(!plan_video(1920, 1080, 800, 480, Some(u32::MAX)).fitted);
    assert!(!plan_video(1920, 1080, 800, 480, Some(quality(LANDSCAPE))).fitted);
    assert!(plan_video(1920, 1080, 800, 480, Some(quality(LANDSCAPE) - 1)).fitted);
  }

  #[test]
  fn only_an_animated_renderable_still_passes_untouched() {
    let sniffed = |format, animated| MediaSniff {
      format,
      kind: crate::media::sniff::MediaKind::Image,
      animated,
      mime: None,
    };
    for format in [MediaFormat::Gif, MediaFormat::Png, MediaFormat::Webp, MediaFormat::Avif] {
      assert!(passes_untouched(&sniffed(format, true)));
      assert!(!passes_untouched(&sniffed(format, false)));
    }
    for format in [MediaFormat::Heif, MediaFormat::Tiff, MediaFormat::Bmp] {
      assert!(!passes_untouched(&sniffed(format, true)));
    }
  }

  #[test]
  fn video_never_upscales_and_stays_even() {
    let small = plan_video(640, 360, 800, 480, None);
    assert_eq!((small.width, small.height), (640, 360));
    let odd = plan_video(641, 361, 800, 480, None);
    assert_eq!((odd.width, odd.height), (640, 360));
    let tiny = plan_video(1, 1, 800, 480, None);
    assert_eq!((tiny.width, tiny.height), (2, 2));
  }

  #[test]
  fn an_empty_cover_box_keeps_the_source_size() {
    assert_eq!(
      plan_cover(MediaFormat::Jpeg, 2000, 2000, false, 0, 0, EVERYTHING.to_vec()),
      CoverPlan::Keep
    );
  }
}
