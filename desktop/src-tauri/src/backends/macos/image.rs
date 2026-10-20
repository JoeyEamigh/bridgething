use bridgething_companion::{
  backend::{ImageScaler, ImageTarget, ScaledImage},
  media::{CoverPlan, MediaKind, MediaSniff, StillFormat, plan_cover},
};
use objc2_core_foundation::{
  CFData, CFDictionary, CFMutableData, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize, kCFBooleanTrue,
};
use objc2_core_graphics::{
  CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
  CGInterpolationQuality, kCGColorSpaceSRGB,
};
use objc2_image_io::{
  CGImageDestination, CGImageSource, kCGImageDestinationLossyCompressionQuality, kCGImagePropertyAPNGCanvasPixelHeight,
  kCGImagePropertyAPNGCanvasPixelWidth, kCGImagePropertyGIFCanvasPixelHeight, kCGImagePropertyGIFCanvasPixelWidth,
  kCGImagePropertyGIFDictionary, kCGImagePropertyHEICSCanvasPixelHeight, kCGImagePropertyHEICSCanvasPixelWidth,
  kCGImagePropertyHEICSDictionary, kCGImagePropertyPNGDictionary, kCGImagePropertyPixelHeight,
  kCGImagePropertyPixelWidth, kCGImagePropertyWebPCanvasPixelHeight, kCGImagePropertyWebPCanvasPixelWidth,
  kCGImagePropertyWebPDictionary, kCGImageSourceCreateThumbnailFromImageAlways,
  kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceThumbnailMaxPixelSize,
};
use objc2_uniform_type_identifiers::{UTTypeGIF, UTTypeJPEG, UTTypePNG};

use super::contain::contained;

const ENCODABLE: [StillFormat; 3] = [StillFormat::Jpeg, StillFormat::Png, StillFormat::Gif];

type Properties = CFDictionary<CFString, CFType>;

pub struct ImageIoScaler;

impl ImageScaler for ImageIoScaler {
  fn scale(&self, bytes: Vec<u8>, sniff: MediaSniff, target: ImageTarget) -> Option<ScaledImage> {
    contained(|| match target {
      ImageTarget::Thumbnail { max_edge, quality } => thumbnail(&bytes, max_edge, quality),
      ImageTarget::Cover { width, height } => cover(bytes, sniff, width, height),
    })
    .inspect_err(|thrown| tracing::warn!(%thrown, ?target, "imageio threw while scaling a still"))
    .ok()
    .flatten()
  }
}

fn thumbnail(bytes: &[u8], max_edge: u32, quality: f32) -> Option<ScaledImage> {
  let data = CFData::from_bytes(bytes);
  let source = unsafe { CGImageSource::with_data(&data, None) }?;
  let primary = unsafe { source.primary_image_index() };
  let image = upright(&source, primary, Some(max_edge))?;
  let (width, height) = dimensions(&image);
  Some(ScaledImage {
    bytes: encode(&image, StillFormat::Jpeg, quality)?,
    mime: StillFormat::Jpeg.mime().to_owned(),
    width,
    height,
  })
}

fn cover(bytes: Vec<u8>, sniffed: MediaSniff, cover_width: u32, cover_height: u32) -> Option<ScaledImage> {
  if sniffed.kind != MediaKind::Image {
    return None;
  }
  let data = CFData::from_bytes(&bytes);
  let source = unsafe { CGImageSource::with_data(&data, None) }?;

  if sniffed.animated {
    let (width, height) = canvas(&source)?;
    return Some(ScaledImage {
      bytes,
      mime: sniffed.mime?,
      width,
      height,
    });
  }

  let primary = unsafe { source.primary_image_index() };
  let decoded = unsafe { source.image_at_index(primary, None) }?;
  let alpha = has_alpha(&decoded);
  let image = upright(&source, primary, None)?;
  let (width, height) = dimensions(&image);
  let plan = plan_cover(
    sniffed.format,
    width,
    height,
    alpha,
    cover_width,
    cover_height,
    ENCODABLE.to_vec(),
  );
  match plan {
    CoverPlan::Keep => Some(ScaledImage {
      bytes,
      mime: sniffed.mime?,
      width,
      height,
    }),
    CoverPlan::Encode {
      width,
      height,
      format,
      mime,
      quality,
    } => {
      let resized = resize(&image, width, height, alpha)?;
      Some(ScaledImage {
        bytes: encode(&resized, format, quality)?,
        mime,
        width,
        height,
      })
    }
  }
}

fn upright(source: &CGImageSource, index: usize, max_edge: Option<u32>) -> Option<CFRetained<CGImage>> {
  let always = unsafe { kCFBooleanTrue }?;
  let edge = max_edge.map(|edge| CFNumber::new_i64(i64::from(edge)));
  let mut entries: Vec<(&CFString, &CFType)> = vec![
    (unsafe { kCGImageSourceCreateThumbnailFromImageAlways }, always),
    (unsafe { kCGImageSourceCreateThumbnailWithTransform }, always),
  ];
  if let Some(edge) = &edge {
    entries.push((unsafe { kCGImageSourceThumbnailMaxPixelSize }, edge));
  }
  let options = dictionary(&entries);
  unsafe { source.thumbnail_at_index(index, Some(options.as_opaque())) }
}

fn resize(image: &CGImage, width: u32, height: u32, alpha: bool) -> Option<CFRetained<CGImage>> {
  let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
  let layout = if alpha {
    CGImageAlphaInfo::PremultipliedLast
  } else {
    CGImageAlphaInfo::NoneSkipLast
  };
  let context = unsafe {
    CGBitmapContextCreate(
      std::ptr::null_mut(),
      width as usize,
      height as usize,
      8,
      0,
      Some(&space),
      layout.0,
    )
  }?;
  let frame = CGRect::new(CGPoint::ZERO, CGSize::new(f64::from(width), f64::from(height)));
  CGContext::clear_rect(Some(&context), frame);
  CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
  CGContext::draw_image(Some(&context), frame, Some(image));
  CGBitmapContextCreateImage(Some(&context))
}

pub(super) fn encode(image: &CGImage, format: StillFormat, quality: f32) -> Option<Vec<u8>> {
  let kind = match format {
    StillFormat::Jpeg => unsafe { UTTypeJPEG },
    StillFormat::Png => unsafe { UTTypePNG },
    StillFormat::Gif => unsafe { UTTypeGIF },
    StillFormat::Webp => return None,
  };
  let out = CFMutableData::new(None, 0)?;
  let kind = CFString::from_str(&kind.identifier().to_string());
  let destination = unsafe { CGImageDestination::with_data(&out, &kind, 1, None) }?;

  let compression = CFNumber::new_f64(f64::from(quality.clamp(0.0, 1.0)));
  let options = (format == StillFormat::Jpeg)
    .then(|| dictionary(&[(unsafe { kCGImageDestinationLossyCompressionQuality }, &compression)]));
  unsafe { destination.add_image(image, options.as_deref().map(CFDictionary::as_opaque)) };
  unsafe { destination.finalize() }.then(|| out.to_vec())
}

fn canvas(source: &CGImageSource) -> Option<(u32, u32)> {
  let canvases = unsafe {
    [
      (
        kCGImagePropertyGIFDictionary,
        kCGImagePropertyGIFCanvasPixelWidth,
        kCGImagePropertyGIFCanvasPixelHeight,
      ),
      (
        kCGImagePropertyPNGDictionary,
        kCGImagePropertyAPNGCanvasPixelWidth,
        kCGImagePropertyAPNGCanvasPixelHeight,
      ),
      (
        kCGImagePropertyWebPDictionary,
        kCGImagePropertyWebPCanvasPixelWidth,
        kCGImagePropertyWebPCanvasPixelHeight,
      ),
      (
        kCGImagePropertyHEICSDictionary,
        kCGImagePropertyHEICSCanvasPixelWidth,
        kCGImagePropertyHEICSCanvasPixelHeight,
      ),
    ]
  };
  let container = unsafe { source.properties(None) }.map(typed);
  let declared = container.and_then(|container| {
    canvases.iter().find_map(|(family, width, height)| {
      let nested = typed(container.get(family)?.downcast::<CFDictionary>().ok()?);
      pair(&nested, width, height)
    })
  });
  declared.or_else(|| {
    let first = typed(unsafe { source.properties_at_index(0, None) }?);
    pair(&first, unsafe { kCGImagePropertyPixelWidth }, unsafe {
      kCGImagePropertyPixelHeight
    })
  })
}

fn pair(properties: &Properties, width: &CFString, height: &CFString) -> Option<(u32, u32)> {
  let read = |key: &CFString| {
    let value = properties.get(key)?.downcast::<CFNumber>().ok()?.as_i64()?;
    u32::try_from(value).ok().filter(|edge| *edge > 0)
  };
  Some((read(width)?, read(height)?))
}

fn typed(dictionary: CFRetained<CFDictionary>) -> CFRetained<Properties> {
  unsafe { CFRetained::cast_unchecked(dictionary) }
}

fn dimensions(image: &CGImage) -> (u32, u32) {
  (CGImage::width(Some(image)) as u32, CGImage::height(Some(image)) as u32)
}

fn has_alpha(image: &CGImage) -> bool {
  !matches!(
    CGImage::alpha_info(Some(image)),
    CGImageAlphaInfo::None | CGImageAlphaInfo::NoneSkipFirst | CGImageAlphaInfo::NoneSkipLast
  )
}

fn dictionary(entries: &[(&CFString, &CFType)]) -> CFRetained<Properties> {
  let keys: Vec<&CFString> = entries.iter().map(|(key, _)| *key).collect();
  let values: Vec<&CFType> = entries.iter().map(|(_, value)| *value).collect();
  CFDictionary::from_slices(&keys, &values)
}

#[cfg(test)]
mod tests {
  use bridgething_companion::{
    backend::sniff_and_scale,
    media::{MediaFormat, sniff},
  };
  use objc2_core_graphics::{CGBitmapContextGetBytesPerRow, CGBitmapContextGetData};
  use objc2_image_io::{
    kCGImagePropertyAPNGDelayTime, kCGImagePropertyGIFDelayTime, kCGImagePropertyHasAlpha, kCGImagePropertyOrientation,
  };
  use objc2_uniform_type_identifiers::{UTType, UTTypeHEIC};

  use super::*;
  use crate::backends::jpeg;

  const COVER: ImageTarget = ImageTarget::Cover {
    width: 800,
    height: 800,
  };

  fn painted(width: u32, height: u32, alpha: bool) -> CFRetained<CGImage> {
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).expect("an srgb space");
    let layout = if alpha {
      CGImageAlphaInfo::PremultipliedLast
    } else {
      CGImageAlphaInfo::NoneSkipLast
    };
    let context = unsafe {
      CGBitmapContextCreate(
        std::ptr::null_mut(),
        width as usize,
        height as usize,
        8,
        0,
        Some(&space),
        layout.0,
      )
    }
    .expect("a bitmap context");
    let half = f64::from(width) / 2.0;
    let full = f64::from(height);
    CGContext::clear_rect(
      Some(&context),
      CGRect::new(CGPoint::ZERO, CGSize::new(f64::from(width), full)),
    );
    CGContext::set_rgb_fill_color(Some(&context), 1.0, 0.0, 0.0, 1.0);
    CGContext::fill_rect(Some(&context), CGRect::new(CGPoint::ZERO, CGSize::new(half, full)));
    if !alpha {
      CGContext::set_rgb_fill_color(Some(&context), 0.0, 0.0, 1.0, 1.0);
      CGContext::fill_rect(
        Some(&context),
        CGRect::new(CGPoint::new(half, 0.0), CGSize::new(half, full)),
      );
    }
    CGBitmapContextCreateImage(Some(&context)).expect("a painted image")
  }

  fn written(kind: &UTType, frames: &[(&CGImage, Option<&CFDictionary>)]) -> Vec<u8> {
    let out = CFMutableData::new(None, 0).expect("a data sink");
    let kind = CFString::from_str(&kind.identifier().to_string());
    let destination =
      unsafe { CGImageDestination::with_data(&out, &kind, frames.len(), None) }.expect("an image destination");
    for (image, properties) in frames {
      unsafe { destination.add_image(image, *properties) };
    }
    assert!(unsafe { destination.finalize() }, "the fixture encodes");
    out.to_vec()
  }

  fn still(kind: &UTType, image: &CGImage) -> Vec<u8> {
    written(kind, &[(image, None)])
  }

  fn oriented(image: &CGImage, orientation: i64) -> Vec<u8> {
    let value = CFNumber::new_i64(orientation);
    let properties = dictionary(&[(unsafe { kCGImagePropertyOrientation }, &value)]);
    written(unsafe { UTTypeJPEG }, &[(image, Some(properties.as_opaque()))])
  }

  fn animated(kind: &UTType, family: &CFString, delay: &CFString) -> Vec<u8> {
    let first = painted(320, 200, false);
    let second = painted(320, 200, true);
    let seconds = CFNumber::new_f64(0.1);
    let timing = dictionary(&[(delay, &seconds)]);
    let frame = dictionary(&[(family, timing.as_opaque())]);
    written(
      kind,
      &[(&first, Some(frame.as_opaque())), (&second, Some(frame.as_opaque()))],
    )
  }

  fn decoded(bytes: &[u8]) -> (CFRetained<CGImageSource>, CFRetained<CGImage>) {
    let data = CFData::from_bytes(bytes);
    let source = unsafe { CGImageSource::with_data(&data, None) }.expect("the output decodes");
    let image = unsafe { source.image_at_index(0, None) }.expect("a first frame");
    (source, image)
  }

  fn alpha_at(image: &CGImage, x: usize, y: usize) -> u8 {
    let (width, height) = dimensions(image);
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).expect("an srgb space");
    let context = unsafe {
      CGBitmapContextCreate(
        std::ptr::null_mut(),
        width as usize,
        height as usize,
        8,
        0,
        Some(&space),
        CGImageAlphaInfo::PremultipliedLast.0,
      )
    }
    .expect("a readback context");
    let frame = CGRect::new(CGPoint::ZERO, CGSize::new(f64::from(width), f64::from(height)));
    CGContext::clear_rect(Some(&context), frame);
    CGContext::draw_image(Some(&context), frame, Some(image));
    let stride = CGBitmapContextGetBytesPerRow(Some(&context));
    let pixels = CGBitmapContextGetData(Some(&context)).cast::<u8>();
    unsafe { *pixels.add(y * stride + x * 4 + 3) }
  }

  fn cover(bytes: Vec<u8>) -> ScaledImage {
    sniff_and_scale(&ImageIoScaler, bytes, COVER).expect("the cover scales")
  }

  #[test]
  fn an_animated_gif_passes_through_byte_for_byte() {
    let bytes = animated(unsafe { UTTypeGIF }, unsafe { kCGImagePropertyGIFDictionary }, unsafe {
      kCGImagePropertyGIFDelayTime
    });
    assert!(sniff(&bytes).animated, "the fixture is a multi-frame gif");

    let scaled = cover(bytes.clone());

    assert_eq!(scaled.bytes, bytes);
    assert_eq!(scaled.mime, "image/gif");
    assert_eq!((scaled.width, scaled.height), (320, 200));
  }

  #[test]
  fn an_apng_passes_through_byte_for_byte() {
    let bytes = animated(unsafe { UTTypePNG }, unsafe { kCGImagePropertyPNGDictionary }, unsafe {
      kCGImagePropertyAPNGDelayTime
    });
    assert!(sniff(&bytes).animated, "the fixture is an apng");

    let scaled = cover(bytes.clone());

    assert_eq!(scaled.bytes, bytes);
    assert_eq!(scaled.mime, "image/png");
    assert_eq!((scaled.width, scaled.height), (320, 200));
  }

  #[test]
  fn png_alpha_survives_a_downscale_to_the_planned_size() {
    let bytes = still(unsafe { UTTypePNG }, &painted(2000, 1000, true));

    let scaled = cover(bytes);

    assert_eq!(scaled.mime, "image/png");
    assert_eq!((scaled.width, scaled.height), (1600, 800));
    let (source, image) = decoded(&scaled.bytes);
    assert_eq!(sniff(&scaled.bytes).format, MediaFormat::Png);
    assert_eq!(dimensions(&image), (1600, 800));
    let properties = typed(unsafe { source.properties_at_index(0, None) }.expect("frame properties"));
    let declared = properties
      .get(unsafe { kCGImagePropertyHasAlpha })
      .and_then(|value| value.downcast::<objc2_core_foundation::CFBoolean>().ok())
      .map(|value| value.as_bool());
    assert_eq!(declared, Some(true));
    assert_eq!(alpha_at(&image, 200, 400), 255);
    assert_eq!(alpha_at(&image, 1400, 400), 0);
  }

  #[test]
  fn heic_always_lands_as_jpeg() {
    let bytes = still(unsafe { UTTypeHEIC }, &painted(640, 480, false));
    assert_eq!(sniff(&bytes).format, MediaFormat::Heif);

    let scaled = cover(bytes);

    assert_eq!(scaled.mime, "image/jpeg");
    assert_eq!((scaled.width, scaled.height), (640, 480));
    assert_eq!(sniff(&scaled.bytes).format, MediaFormat::Jpeg);
    assert_eq!(dimensions(&decoded(&scaled.bytes).1), (640, 480));
  }

  #[test]
  fn a_large_jpeg_shrinks_until_its_short_edge_meets_the_box() {
    let bytes = still(unsafe { UTTypeJPEG }, &painted(4032, 3024, false));

    let scaled = cover(bytes);

    assert_eq!(scaled.mime, "image/jpeg");
    assert_eq!((scaled.width, scaled.height), (1067, 800));
    assert_eq!(dimensions(&decoded(&scaled.bytes).1), (1067, 800));
  }

  #[test]
  fn a_jpeg_that_already_fits_is_never_upscaled_or_touched() {
    let bytes = still(unsafe { UTTypeJPEG }, &painted(320, 240, false));

    let scaled = cover(bytes.clone());

    assert_eq!(scaled.bytes, bytes);
    assert_eq!(scaled.mime, "image/jpeg");
    assert_eq!((scaled.width, scaled.height), (320, 240));
  }

  #[test]
  fn exif_rotation_decides_the_upright_dimensions() {
    let small = oriented(&painted(200, 100, false), 6);
    let kept = cover(small.clone());
    assert_eq!(kept.bytes, small);
    assert_eq!((kept.width, kept.height), (100, 200));

    let scaled = cover(oriented(&painted(2000, 1000, false), 6));
    assert_eq!((scaled.width, scaled.height), (800, 1600));
    let (source, image) = decoded(&scaled.bytes);
    assert_eq!(dimensions(&image), (800, 1600));
    let properties = typed(unsafe { source.properties_at_index(0, None) }.expect("frame properties"));
    let orientation = properties
      .get(unsafe { kCGImagePropertyOrientation })
      .and_then(|value| value.downcast::<CFNumber>().ok())
      .and_then(|value| value.as_i64());
    assert!(matches!(orientation, None | Some(1)), "orientation {orientation:?}");
  }

  #[test]
  fn a_thumbnail_caps_the_long_edge_and_is_always_jpeg() {
    let target = ImageTarget::Thumbnail {
      max_edge: 512,
      quality: 0.6,
    };

    let wide = sniff_and_scale(
      &ImageIoScaler,
      still(unsafe { UTTypePNG }, &painted(2000, 1000, true)),
      target,
    )
    .expect("the thumbnail scales");
    assert_eq!(wide.mime, "image/jpeg");
    assert_eq!((wide.width, wide.height), (512, 256));
    assert_eq!(jpeg::edge(&wide.bytes), Some(512));

    let turned =
      sniff_and_scale(&ImageIoScaler, oriented(&painted(2000, 1000, false), 6), target).expect("the thumbnail scales");
    assert_eq!((turned.width, turned.height), (256, 512));
  }

  #[test]
  fn a_thumbnail_is_cut_from_the_primary_image_of_a_collection() {
    let mut bytes = written(
      unsafe { UTTypeHEIC },
      &[(&painted(320, 200, false), None), (&painted(640, 480, false), None)],
    );
    let primary = bytes
      .windows(4)
      .position(|window| window == b"pitm")
      .expect("a primary item box");
    let item = primary + 8;
    assert_eq!(
      bytes[item..item + 2],
      [0, 1],
      "the encoder names its first item primary"
    );
    bytes[item + 1] = 2;
    let data = CFData::from_bytes(&bytes);
    let source = unsafe { CGImageSource::with_data(&data, None) }.expect("the fixture decodes");
    assert_eq!(
      unsafe { source.primary_image_index() },
      1,
      "the fixture leads with a secondary image"
    );

    let scaled = sniff_and_scale(
      &ImageIoScaler,
      bytes,
      ImageTarget::Thumbnail {
        max_edge: 160,
        quality: 0.6,
      },
    )
    .expect("the thumbnail scales");

    assert_eq!((scaled.width, scaled.height), (160, 120));
  }

  #[test]
  fn bytes_that_are_not_an_image_are_declined() {
    assert_eq!(
      sniff_and_scale(&ImageIoScaler, b"plainly not an image".to_vec(), COVER),
      None
    );
    let jpeg = still(unsafe { UTTypeJPEG }, &painted(1200, 900, false));
    assert_eq!(
      ImageIoScaler.scale(jpeg, sniff(b"plainly not an image"), COVER),
      None,
      "the cover trusts the sniff it is handed"
    );
  }
}
