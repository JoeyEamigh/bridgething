use crate::media::{MediaSniff, sniff};

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum ImageTarget {
  Thumbnail { max_edge: u32, quality: f32 },
  Cover { width: u32, height: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ScaledImage {
  pub bytes: Vec<u8>,
  pub mime: String,
  pub width: u32,
  pub height: u32,
}

#[uniffi::export(with_foreign)]
pub trait ImageScaler: Send + Sync {
  fn scale(&self, bytes: Vec<u8>, sniff: MediaSniff, target: ImageTarget) -> Option<ScaledImage>;
}

pub fn sniff_and_scale(scaler: &dyn ImageScaler, bytes: Vec<u8>, target: ImageTarget) -> Option<ScaledImage> {
  let sniffed = sniff(&bytes);
  scaler.scale(bytes, sniffed, target)
}
