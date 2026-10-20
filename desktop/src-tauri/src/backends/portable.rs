use std::io::Cursor;

use bridgething_companion::{
  backend::{ImageScaler, ImageTarget, ScaledImage},
  media::{CoverPlan, MediaFormat, MediaKind, MediaSniff, StillFormat, plan_cover},
};
use image::{
  DynamicImage, ImageDecoder, ImageFormat, ImageReader,
  codecs::{gif::GifEncoder, jpeg::JpegEncoder, png::PngEncoder, webp::WebPEncoder},
  imageops::FilterType,
};

use crate::backends::ffmpeg::Sidecar;

const QUALITY_FLOOR: u8 = 1;
const QUALITY_CEILING: u8 = 100;
const GIF_SPEED: i32 = 10;
const JPEG: &str = "image/jpeg";
const ENCODABLE: [StillFormat; 4] = [StillFormat::Jpeg, StillFormat::Png, StillFormat::Gif, StillFormat::Webp];

#[derive(Default)]
pub struct PortableScaler {
  sidecar: Option<Sidecar>,
}

impl PortableScaler {
  pub fn new(sidecar: Option<Sidecar>) -> Self {
    Self { sidecar }
  }

  fn thumbnail(&self, bytes: &[u8], format: MediaFormat, max_edge: u32, quality: f32) -> Option<ScaledImage> {
    let decoded = self.decode(bytes, format)?;
    let edge = decoded.width().max(decoded.height());
    let scaled = if max_edge > 0 && edge > max_edge {
      decoded.resize(max_edge, max_edge, FilterType::Lanczos3)
    } else {
      decoded
    };
    let (width, height) = (scaled.width(), scaled.height());
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, jpeg_quality(quality))
      .encode_image(&scaled.into_rgb8())
      .ok()?;
    Some(ScaledImage {
      bytes: out,
      mime: JPEG.to_owned(),
      width,
      height,
    })
  }

  fn cover(&self, bytes: Vec<u8>, sniffed: MediaSniff, cover_width: u32, cover_height: u32) -> Option<ScaledImage> {
    if sniffed.kind != MediaKind::Image {
      return None;
    }
    let mime = sniffed.mime?;
    if sniffed.animated {
      let (width, height) = self.canvas(&bytes, sniffed.format)?;
      return Some(ScaledImage {
        bytes,
        mime,
        width,
        height,
      });
    }
    let decoded = self.decode(&bytes, sniffed.format)?;
    let (width, height) = (decoded.width(), decoded.height());
    let plan = plan_cover(
      sniffed.format,
      width,
      height,
      decoded.color().has_alpha(),
      cover_width,
      cover_height,
      ENCODABLE.to_vec(),
    );
    match plan {
      CoverPlan::Keep => Some(ScaledImage {
        bytes,
        mime,
        width,
        height,
      }),
      CoverPlan::Encode {
        width,
        height,
        format,
        mime,
        quality,
      } => Some(ScaledImage {
        bytes: encode(
          &decoded.resize_exact(width, height, FilterType::Lanczos3),
          format,
          quality,
        )?,
        mime,
        width,
        height,
      }),
    }
  }

  fn decode(&self, bytes: &[u8], format: MediaFormat) -> Option<DynamicImage> {
    match format {
      MediaFormat::Heif | MediaFormat::Avif => {
        let frame = self
          .sidecar
          .as_ref()?
          .frame(bytes)
          .inspect_err(|error| tracing::warn!(?error, ?format, "the sidecar could not decode a still"))
          .ok()?;
        image::load_from_memory_with_format(&frame, ImageFormat::Pnm).ok()
      }
      _ => oriented(bytes),
    }
  }

  fn canvas(&self, bytes: &[u8], format: MediaFormat) -> Option<(u32, u32)> {
    match format {
      MediaFormat::Avif => self
        .sidecar
        .as_ref()?
        .size(bytes)
        .inspect_err(|error| tracing::warn!(?error, "the sidecar could not size an animated avif"))
        .ok(),
      _ => ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok(),
    }
  }
}

impl ImageScaler for PortableScaler {
  fn scale(&self, bytes: Vec<u8>, sniff: MediaSniff, target: ImageTarget) -> Option<ScaledImage> {
    match target {
      ImageTarget::Thumbnail { max_edge, quality } => self.thumbnail(&bytes, sniff.format, max_edge, quality),
      ImageTarget::Cover { width, height } => self.cover(bytes, sniff, width, height),
    }
  }
}

fn oriented(bytes: &[u8]) -> Option<DynamicImage> {
  let mut decoder = ImageReader::new(Cursor::new(bytes))
    .with_guessed_format()
    .ok()?
    .into_decoder()
    .ok()?;
  let orientation = decoder.orientation().ok()?;
  let mut decoded = DynamicImage::from_decoder(decoder).ok()?;
  decoded.apply_orientation(orientation);
  Some(decoded)
}

fn encode(image: &DynamicImage, format: StillFormat, quality: f32) -> Option<Vec<u8>> {
  let mut out = Vec::new();
  match format {
    StillFormat::Jpeg => JpegEncoder::new_with_quality(&mut out, jpeg_quality(quality))
      .encode_image(&image.to_rgb8())
      .ok()?,
    StillFormat::Png => image.write_with_encoder(PngEncoder::new(&mut out)).ok()?,
    StillFormat::Webp => eight_bit(image)
      .write_with_encoder(WebPEncoder::new_lossless(&mut out))
      .ok()?,
    StillFormat::Gif => eight_bit(image)
      .write_with_encoder(GifEncoder::new_with_speed(&mut out, GIF_SPEED))
      .ok()?,
  }
  Some(out)
}

fn eight_bit(image: &DynamicImage) -> DynamicImage {
  if image.color().has_alpha() {
    DynamicImage::ImageRgba8(image.to_rgba8())
  } else {
    DynamicImage::ImageRgb8(image.to_rgb8())
  }
}

fn jpeg_quality(quality: f32) -> u8 {
  ((quality.clamp(0.0, 1.0) * f32::from(QUALITY_CEILING)).round() as u8).clamp(QUALITY_FLOOR, QUALITY_CEILING)
}

#[cfg(test)]
mod tests {
  use image::{
    Delay, ExtendedColorType, Frame, GenericImageView, ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage,
    codecs::bmp::BmpEncoder,
  };

  use bridgething_companion::{backend::sniff_and_scale, media::sniff};

  use super::*;

  const COVER: ImageTarget = ImageTarget::Cover {
    width: 800,
    height: 800,
  };

  fn scaler() -> PortableScaler {
    PortableScaler::default()
  }

  fn gradient(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| Rgb([(x % 251) as u8, (y % 241) as u8, 96]))
  }

  fn jpeg(width: u32, height: u32, exif: Option<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, 90);
    if let Some(exif) = exif {
      encoder.set_exif_metadata(exif).unwrap();
    }
    encoder
      .write_image(gradient(width, height).as_raw(), width, height, ExtendedColorType::Rgb8)
      .unwrap();
    out
  }

  fn turned_clockwise() -> Vec<u8> {
    let mut tiff = b"II*\0\x08\0\0\0\x01\0".to_vec();
    tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
    tiff.extend_from_slice(&[0, 0, 0, 0]);
    tiff
  }

  fn half_clear(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, _| {
      if x < width / 2 {
        Rgba([0, 0, 0, 0])
      } else {
        Rgba([20, 200, 90, 255])
      }
    })
  }

  fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
      .write_image(
        half_clear(width, height).as_raw(),
        width,
        height,
        ExtendedColorType::Rgba8,
      )
      .unwrap();
    out
  }

  fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(u32::MAX, |crc, &byte| {
      (0..8).fold(crc ^ u32::from(byte), |crc, _| {
        if crc & 1 == 1 {
          (crc >> 1) ^ 0xEDB8_8320
        } else {
          crc >> 1
        }
      })
    })
  }

  fn apng(width: u32, height: u32) -> Vec<u8> {
    let still = png(width, height);
    let body = [2u32.to_be_bytes(), 0u32.to_be_bytes()].concat();
    let typed = [b"acTL".as_slice(), &body].concat();
    let mut chunk = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    chunk.extend_from_slice(&typed);
    chunk.extend_from_slice(&crc32(&typed).to_be_bytes());
    let after_header = 8 + 25;
    [&still[..after_header], &chunk, &still[after_header..]].concat()
  }

  fn gif(frames: u8, width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    GifEncoder::new(&mut out)
      .encode_frames((0..frames).map(|shade| {
        Frame::from_parts(
          RgbaImage::from_pixel(width, height, Rgba([shade * 80, 40, 40, 255])),
          0,
          0,
          Delay::from_numer_denom_ms(100, 1),
        )
      }))
      .unwrap();
    out
  }

  fn riff_chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = kind.to_vec();
    out.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
      out.push(0);
    }
    out
  }

  fn le24(value: u32) -> [u8; 3] {
    let [a, b, c, _] = value.to_le_bytes();
    [a, b, c]
  }

  fn webp(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
      .write_image(
        half_clear(width, height).as_raw(),
        width,
        height,
        ExtendedColorType::Rgba8,
      )
      .unwrap();
    out
  }

  fn animated_webp(width: u32, height: u32) -> Vec<u8> {
    let frame = webp(width, height);
    assert_eq!(&frame[12..16], b"VP8L");
    let mut header = vec![0x12, 0, 0, 0];
    header.extend_from_slice(&le24(width - 1));
    header.extend_from_slice(&le24(height - 1));
    let mut chunks = riff_chunk(b"VP8X", &header);
    chunks.extend(riff_chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
    for _ in 0..2 {
      let mut anmf = [le24(0), le24(0), le24(width - 1), le24(height - 1), le24(100)].concat();
      anmf.push(0);
      anmf.extend_from_slice(&frame[12..]);
      chunks.extend(riff_chunk(b"ANMF", &anmf));
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&u32::try_from(chunks.len() + 4).unwrap().to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend(chunks);
    out
  }

  fn cover(bytes: Vec<u8>) -> ScaledImage {
    sniff_and_scale(&scaler(), bytes, COVER).expect("a still the portable scaler handles")
  }

  #[test]
  fn a_thumbnail_caps_the_long_edge_as_jpeg_and_never_upscales() {
    let thumbnail = |bytes: Vec<u8>, max_edge: u32| {
      sniff_and_scale(&scaler(), bytes, ImageTarget::Thumbnail { max_edge, quality: 0.6 }).unwrap()
    };
    let capped = thumbnail(png(1024, 640), 512);
    assert_eq!((capped.width, capped.height, capped.mime.as_str()), (512, 320, JPEG));
    assert_eq!(image::guess_format(&capped.bytes).unwrap(), ImageFormat::Jpeg);
    assert_eq!(decoded_size(&capped.bytes), (512, 320));

    let small = thumbnail(jpeg(300, 200, None), 512);
    assert_eq!((small.width, small.height), (300, 200));
    let uncapped = thumbnail(jpeg(900, 600, None), 0);
    assert_eq!((uncapped.width, uncapped.height), (900, 600));
  }

  #[test]
  fn a_thumbnail_comes_out_upright() {
    let turned = sniff_and_scale(
      &scaler(),
      jpeg(600, 400, Some(turned_clockwise())),
      ImageTarget::Thumbnail {
        max_edge: 512,
        quality: 0.6,
      },
    )
    .unwrap();
    assert_eq!((turned.width, turned.height), (341, 512));
  }

  fn decoded_size(bytes: &[u8]) -> (u32, u32) {
    image::load_from_memory(bytes).unwrap().dimensions()
  }

  #[test]
  fn a_large_jpeg_shrinks_until_it_just_covers_the_box() {
    let scaled = cover(jpeg(1600, 1200, None));
    assert_eq!((scaled.width, scaled.height, scaled.mime.as_str()), (1067, 800, JPEG));
    assert_eq!(decoded_size(&scaled.bytes), (1067, 800));
  }

  #[test]
  fn the_cover_box_is_measured_upright() {
    let scaled = cover(jpeg(1600, 1200, Some(turned_clockwise())));
    assert_eq!((scaled.width, scaled.height), (800, 1067));
    assert_eq!(decoded_size(&scaled.bytes), (800, 1067));
  }

  #[test]
  fn a_still_that_already_fits_is_kept_byte_for_byte() {
    let small = jpeg(640, 480, None);
    let kept = cover(small.clone());
    assert_eq!(kept.bytes, small);
    assert_eq!((kept.width, kept.height, kept.mime.as_str()), (640, 480, JPEG));

    let tiny = png(32, 24);
    assert_eq!(cover(tiny.clone()).bytes, tiny);
  }

  #[test]
  fn a_png_keeps_its_alpha_through_a_downscale() {
    let scaled = cover(png(1600, 1000));
    assert_eq!(
      (scaled.width, scaled.height, scaled.mime.as_str()),
      (1280, 800, "image/png")
    );
    let pixels = image::load_from_memory_with_format(&scaled.bytes, ImageFormat::Png)
      .unwrap()
      .to_rgba8();
    assert_eq!(pixels.dimensions(), (1280, 800));
    assert_eq!(pixels.get_pixel(10, 400)[3], 0);
    assert_eq!(pixels.get_pixel(1270, 400)[3], 255);
  }

  #[test]
  fn animation_passes_through_untouched_with_its_canvas() {
    for (animated, mime, size) in [
      (gif(2, 1200, 900), "image/gif", (1200, 900)),
      (apng(1000, 1000), "image/png", (1000, 1000)),
      (animated_webp(1100, 900), "image/webp", (1100, 900)),
    ] {
      let passed = cover(animated.clone());
      assert_eq!(passed.bytes, animated, "{mime} was re-encoded");
      assert_eq!(
        (passed.mime.as_str(), passed.width, passed.height),
        (mime, size.0, size.1)
      );
    }
  }

  #[test]
  fn a_still_gif_and_webp_keep_their_format() {
    let gif = cover(gif(1, 1600, 1000));
    assert_eq!((gif.width, gif.height, gif.mime.as_str()), (1280, 800, "image/gif"));
    assert_eq!(image::guess_format(&gif.bytes).unwrap(), ImageFormat::Gif);
    assert_eq!(decoded_size(&gif.bytes), (1280, 800));

    let webp = cover(webp(1600, 1600));
    assert_eq!((webp.width, webp.height, webp.mime.as_str()), (800, 800, "image/webp"));
    let pixels = image::load_from_memory_with_format(&webp.bytes, ImageFormat::WebP)
      .unwrap()
      .to_rgba8();
    assert_eq!(pixels.get_pixel(10, 400)[3], 0);
  }

  #[test]
  fn an_unrenderable_still_lands_as_jpeg() {
    let mut bmp = Vec::new();
    BmpEncoder::new(&mut bmp)
      .write_image(gradient(400, 300).as_raw(), 400, 300, ExtendedColorType::Rgb8)
      .unwrap();
    let scaled = cover(bmp);
    assert_eq!((scaled.width, scaled.height, scaled.mime.as_str()), (400, 300, JPEG));
    assert_eq!(image::guess_format(&scaled.bytes).unwrap(), ImageFormat::Jpeg);
  }

  #[cfg(unix)]
  #[test]
  fn a_thumbnail_decodes_whatever_a_cover_can() {
    use crate::backends::ffmpeg::fake;
    let (_dir, sidecar) = fake::sidecar(fake::CANNED);
    fake::canned(&sidecar, 640, 480);
    let scaler = PortableScaler::new(Some(sidecar));
    let avif = [&24u32.to_be_bytes()[..], b"ftypavif\0\0\0\0mif1miaf", &[0; 32]].concat();

    let kept = sniff_and_scale(&scaler, avif.clone(), COVER).expect("the sidecar decodes the avif");
    assert_eq!(kept.bytes, avif, "a small avif is kept as it is");
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
      (320, 240, JPEG)
    );
    assert_eq!(decoded_size(&thumbnail.bytes), (320, 240));
  }

  #[test]
  fn what_the_scaler_cannot_handle_is_declined() {
    assert!(sniff_and_scale(&scaler(), b"%PDF-1.7 not a picture".to_vec(), COVER).is_none());
    let heic = [&24u32.to_be_bytes()[..], b"ftypheic\0\0\0\0mif1heic", &[0; 32]].concat();
    assert!(sniff_and_scale(&scaler(), heic, COVER).is_none());
    let jpeg = jpeg(300, 200, None);
    assert!(
      scaler().scale(jpeg, sniff(b"%PDF-1.7"), COVER).is_none(),
      "the cover trusts the sniff it is handed"
    );
  }
}
