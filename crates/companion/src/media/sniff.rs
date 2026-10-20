use std::{
  io::{Cursor, Read, Seek, SeekFrom},
  ops::Range,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MediaFormat {
  Jpeg,
  Png,
  Gif,
  Webp,
  Avif,
  Heif,
  Bmp,
  Tiff,
  IsoVideo,
  Matroska,
  Avi,
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MediaKind {
  Image,
  Video,
  Other,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaSniff {
  pub format: MediaFormat,
  pub kind: MediaKind,
  pub animated: bool,
  pub mime: Option<String>,
}

impl MediaSniff {
  pub fn names_its_type(&self) -> bool {
    !matches!(
      self.format,
      MediaFormat::IsoVideo | MediaFormat::Matroska | MediaFormat::Tiff | MediaFormat::Unknown
    )
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Movie {
  pub duration_ms: Option<u64>,
  pub has_video: bool,
  pub has_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaProbe {
  pub sniff: MediaSniff,
  pub size: u64,
  pub movie: Option<Movie>,
}

type Classified = (MediaFormat, bool, Option<&'static str>);

const HEAD_BYTES: u64 = 4096;
const BOX_LIMIT: usize = 256;
const HEVC_BRANDS: [&[u8; 4]; 6] = [b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx"];
const AUDIO_BRANDS: [&[u8; 4]; 5] = [b"M4A ", b"M4B ", b"M4P ", b"F4A ", b"F4B "];
const QUICKTIME_ATOMS: [&[u8; 4]; 6] = [b"moov", b"mdat", b"wide", b"free", b"skip", b"pnot"];
const EBML_SEGMENT: u32 = 0x1853_8067;
const EBML_INFO: u32 = 0x1549_A966;
const EBML_TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const EBML_DURATION: u32 = 0x4489;
const EBML_TRACKS: u32 = 0x1654_AE6B;
const EBML_TRACK_ENTRY: u32 = 0xAE;
const EBML_TRACK_TYPE: u32 = 0x83;
const EBML_CLUSTER: u32 = 0x1F43_B675;
const EBML_VIDEO_TRACK: u64 = 1;
const EBML_AUDIO_TRACK: u64 = 2;
const EBML_DEFAULT_SCALE_NS: u64 = 1_000_000;

pub fn sniff(bytes: &[u8]) -> MediaSniff {
  probe(&mut Cursor::new(bytes))
    .map(|probed| probed.sniff)
    .unwrap_or_else(|_| described((MediaFormat::Unknown, false, None)))
}

pub fn probe<R: Read + Seek>(source: &mut R) -> std::io::Result<MediaProbe> {
  let size = source.seek(SeekFrom::End(0))?;
  source.seek(SeekFrom::Start(0))?;
  let mut head = Vec::new();
  source.by_ref().take(HEAD_BYTES).read_to_end(&mut head)?;
  let classified = classify(&head, source);
  let movie = match classified.0 {
    MediaFormat::IsoVideo => movie(source),
    MediaFormat::Matroska => matroska(source),
    MediaFormat::Unknown if iso(&head) => movie(source),
    _ => None,
  };
  let classified = match (classified, movie) {
    ((format @ (MediaFormat::IsoVideo | MediaFormat::Matroska), _, mime), Some(found)) if !found.has_video => (
      MediaFormat::Unknown,
      false,
      Some(pictureless(format, mime == Some("video/webm"), found.has_audio)),
    ),
    (classified, _) => classified,
  };
  Ok(MediaProbe {
    sniff: described(classified),
    size,
    movie,
  })
}

fn pictureless(container: MediaFormat, webm: bool, has_audio: bool) -> &'static str {
  match (container, has_audio) {
    (MediaFormat::Matroska, true) if webm => "audio/webm",
    (MediaFormat::Matroska, true) => "audio/x-matroska",
    (MediaFormat::Matroska, false) => "application/x-matroska",
    (_, true) => "audio/mp4",
    (_, false) => "application/mp4",
  }
}

fn iso(head: &[u8]) -> bool {
  head.len() >= 12 && &head[4..8] == b"ftyp"
}

fn described((format, animated, mime): Classified) -> MediaSniff {
  let kind = match format {
    MediaFormat::Jpeg
    | MediaFormat::Png
    | MediaFormat::Gif
    | MediaFormat::Webp
    | MediaFormat::Avif
    | MediaFormat::Heif
    | MediaFormat::Bmp
    | MediaFormat::Tiff => MediaKind::Image,
    MediaFormat::IsoVideo | MediaFormat::Matroska | MediaFormat::Avi => MediaKind::Video,
    MediaFormat::Unknown => MediaKind::Other,
  };
  MediaSniff {
    format,
    kind,
    animated,
    mime: mime.map(str::to_owned),
  }
}

fn classify<R: Read + Seek>(head: &[u8], source: &mut R) -> Classified {
  if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
    return (MediaFormat::Jpeg, false, Some("image/jpeg"));
  }
  if head.starts_with(b"\x89PNG\r\n\x1a\n") {
    return (MediaFormat::Png, png_animated(source), Some("image/png"));
  }
  if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
    return (MediaFormat::Gif, gif_frames(source) > 1, Some("image/gif"));
  }
  if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
    return (MediaFormat::Webp, webp_animated(head), Some("image/webp"));
  }
  if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"AVI " {
    return (MediaFormat::Avi, false, Some("video/x-msvideo"));
  }
  if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
    let doctype = &head[..head.len().min(64)];
    let webm = doctype.windows(4).any(|window| window == b"webm");
    return (
      MediaFormat::Matroska,
      false,
      Some(if webm { "video/webm" } else { "video/x-matroska" }),
    );
  }
  if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
    return (MediaFormat::Tiff, false, Some("image/tiff"));
  }
  if bmp(head) {
    return (MediaFormat::Bmp, false, Some("image/bmp"));
  }
  if iso(head) {
    return iso_brand(head);
  }
  if head.len() >= 8 && QUICKTIME_ATOMS.iter().any(|atom| &head[4..8] == *atom) {
    return (MediaFormat::IsoVideo, false, Some("video/quicktime"));
  }
  (MediaFormat::Unknown, false, None)
}

fn iso_brand(bytes: &[u8]) -> Classified {
  let declared = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
  let end = declared.clamp(12, bytes.len());
  let major = &bytes[8..12];
  let (compatible, _) = bytes[16.min(end)..end].as_chunks::<4>();
  let has = |brand: &[u8; 4]| major == brand || compatible.contains(brand);

  let hevc = HEVC_BRANDS.iter().any(|brand| has(brand));
  if !hevc && (has(b"avif") || has(b"avis")) {
    return (MediaFormat::Avif, has(b"avis"), Some("image/avif"));
  }
  if hevc || has(b"mif1") || has(b"msf1") {
    let heic = has(b"heic") || has(b"heix");
    return (
      MediaFormat::Heif,
      false,
      Some(if heic { "image/heic" } else { "image/heif" }),
    );
  }
  if AUDIO_BRANDS.iter().any(|brand| major == *brand) {
    return (MediaFormat::Unknown, false, Some("audio/mp4"));
  }
  if major == b"crx " {
    return (MediaFormat::Unknown, false, Some("image/x-canon-cr3"));
  }
  if major == b"qt  " {
    return (MediaFormat::IsoVideo, false, Some("video/quicktime"));
  }
  if major.starts_with(b"3g2") {
    return (MediaFormat::IsoVideo, false, Some("video/3gpp2"));
  }
  if major.starts_with(b"3g") {
    return (MediaFormat::IsoVideo, false, Some("video/3gpp"));
  }
  (MediaFormat::IsoVideo, false, Some("video/mp4"))
}

fn bmp(bytes: &[u8]) -> bool {
  bytes.len() >= 18
    && bytes.starts_with(b"BM")
    && matches!(
      u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]),
      12 | 40 | 52 | 56 | 64 | 108 | 124
    )
}

fn webp_animated(bytes: &[u8]) -> bool {
  bytes.len() >= 21 && &bytes[12..16] == b"VP8X" && bytes[20] & 0x02 != 0
}

fn at<R: Seek>(source: &mut R, offset: u64) -> Option<()> {
  source.seek(SeekFrom::Start(offset)).ok().map(drop)
}

fn skip<R: Seek>(source: &mut R, bytes: u64) -> Option<()> {
  source.seek_relative(i64::try_from(bytes).ok()?).ok()
}

fn take<const N: usize, R: Read>(source: &mut R) -> Option<[u8; N]> {
  let mut taken = [0; N];
  source.read_exact(&mut taken).ok()?;
  Some(taken)
}

fn be32(bytes: &[u8]) -> u32 {
  u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn png_animated<R: Read + Seek>(source: &mut R) -> bool {
  if at(source, 8).is_none() {
    return false;
  }
  while let Some(chunk) = take::<8, _>(source) {
    match &chunk[4..8] {
      b"acTL" => return take::<4, _>(source).is_some_and(|frames| be32(&frames) > 1),
      b"IDAT" | b"IEND" => return false,
      _ => {
        if skip(source, u64::from(be32(&chunk)) + 4).is_none() {
          return false;
        }
      }
    }
  }
  false
}

fn gif_frames<R: Read + Seek>(source: &mut R) -> usize {
  let mut frames = 0;
  let _ = gif_walk(source, &mut frames);
  frames
}

fn gif_walk<R: Read + Seek>(source: &mut R, frames: &mut usize) -> Option<()> {
  at(source, 10)?;
  let [flags] = take(source)?;
  skip(source, 2 + color_table(flags))?;
  loop {
    match take(source)? {
      [0x2C] => {
        *frames += 1;
        if *frames > 1 {
          return None;
        }
        skip(source, 8)?;
        let [local] = take(source)?;
        skip(source, color_table(local) + 1)?;
        skip_sub_blocks(source)?;
      }
      [0x21] => {
        skip(source, 1)?;
        skip_sub_blocks(source)?;
      }
      _ => return None,
    }
  }
}

fn color_table(flags: u8) -> u64 {
  if flags & 0x80 == 0 {
    0
  } else {
    3 * (1 << ((flags & 0x07) + 1))
  }
}

fn skip_sub_blocks<R: Read + Seek>(source: &mut R) -> Option<()> {
  loop {
    let [size] = take(source)?;
    if size == 0 {
      return Some(());
    }
    skip(source, u64::from(size))?;
  }
}

pub fn movie<R: Read + Seek>(source: &mut R) -> Option<Movie> {
  let end = source.seek(SeekFrom::End(0)).ok()?;
  let top = boxes(source, 0..end);
  let inside = boxes(source, body(&top, b"moov")?);
  let mut found = Movie {
    duration_ms: body(&inside, b"mvhd").and_then(|mvhd| movie_duration(source, mvhd)),
    has_video: false,
    has_audio: false,
  };
  for (_, track) in inside.iter().filter(|(kind, _)| kind == b"trak") {
    let parts = boxes(source, track.clone());
    match handler(source, &parts) {
      Some([b'v', b'i', b'd', b'e']) => found.has_video = true,
      Some([b's', b'o', b'u', b'n']) => found.has_audio = true,
      _ => {}
    }
  }
  Some(found)
}

fn boxes<R: Read + Seek>(source: &mut R, within: Range<u64>) -> Vec<([u8; 4], Range<u64>)> {
  let mut found = Vec::new();
  let mut start = within.start;
  while found.len() < BOX_LIMIT && within.end.saturating_sub(start) >= 8 && at(source, start).is_some() {
    let Some(head) = take::<8, _>(source) else {
      break;
    };
    let (size, header) = match be32(&head) {
      0 => (within.end - start, 8),
      1 => match take::<8, _>(source) {
        Some(large) => (u64::from_be_bytes(large), 16),
        None => break,
      },
      size => (u64::from(size), 8),
    };
    let Some(end) = start
      .checked_add(size)
      .filter(|end| size >= header && *end <= within.end)
    else {
      break;
    };
    found.push(([head[4], head[5], head[6], head[7]], start + header..end));
    start = end;
  }
  found
}

fn body(boxes: &[([u8; 4], Range<u64>)], wanted: &[u8; 4]) -> Option<Range<u64>> {
  boxes
    .iter()
    .find(|(kind, _)| kind == wanted)
    .map(|(_, body)| body.clone())
}

fn handler<R: Read + Seek>(source: &mut R, track: &[([u8; 4], Range<u64>)]) -> Option<[u8; 4]> {
  let media = boxes(source, body(track, b"mdia")?);
  at(source, body(&media, b"hdlr")?.start + 8)?;
  take(source)
}

fn movie_duration<R: Read + Seek>(source: &mut R, mvhd: Range<u64>) -> Option<u64> {
  at(source, mvhd.start)?;
  let [version] = take(source)?;
  let (timescale, duration) = match version {
    0 => {
      skip(source, 11)?;
      let times = take::<8, _>(source)?;
      let duration = be32(&times[4..]);
      (be32(&times), (duration != u32::MAX).then_some(u64::from(duration))?)
    }
    1 => {
      skip(source, 19)?;
      let timescale = take::<4, _>(source)?;
      let duration = u64::from_be_bytes(take(source)?);
      (be32(&timescale), (duration != u64::MAX).then_some(duration)?)
    }
    _ => return None,
  };
  (timescale > 0).then(|| duration.saturating_mul(1000) / u64::from(timescale))
}

fn matroska<R: Read + Seek>(source: &mut R) -> Option<Movie> {
  let end = source.seek(SeekFrom::End(0)).ok()?;
  let segment = element(&elements(source, 0..end), EBML_SEGMENT)?;
  let inside = elements(source, segment);
  let tracks = element(&inside, EBML_TRACKS)?;
  let mut found = Movie {
    duration_ms: element(&inside, EBML_INFO).and_then(|info| matroska_duration(source, info)),
    has_video: false,
    has_audio: false,
  };
  for (_, entry) in elements(source, tracks)
    .iter()
    .filter(|(id, _)| *id == EBML_TRACK_ENTRY)
  {
    let parts = elements(source, entry.clone());
    match element(&parts, EBML_TRACK_TYPE).and_then(|kind| unsigned(source, kind)) {
      Some(EBML_VIDEO_TRACK) => found.has_video = true,
      Some(EBML_AUDIO_TRACK) => found.has_audio = true,
      _ => {}
    }
  }
  Some(found)
}

fn elements<R: Read + Seek>(source: &mut R, within: Range<u64>) -> Vec<(u32, Range<u64>)> {
  let mut found = Vec::new();
  let mut start = within.start;
  while found.len() < BOX_LIMIT && start < within.end && at(source, start).is_some() {
    let Some((id, id_length)) = vint(source).filter(|(_, length)| *length <= 4) else {
      break;
    };
    let Some((size, size_length)) = vint(source) else {
      break;
    };
    let body = start + id_length + size_length;
    let marker = 1u64 << (7 * size_length);
    let open_ended = size & !marker == marker - 1;
    let end = if open_ended {
      Some(within.end)
    } else {
      body.checked_add(size & !marker)
    };
    let Some(end) = end.filter(|end| body <= *end && *end <= within.end) else {
      break;
    };
    let id = id as u32;
    found.push((id, body..end));
    if open_ended || id == EBML_CLUSTER {
      break;
    }
    start = end;
  }
  found
}

fn vint<R: Read>(source: &mut R) -> Option<(u64, u64)> {
  let [first] = take(source)?;
  let length = u64::from(first.leading_zeros()) + 1;
  if length > 8 {
    return None;
  }
  let mut value = u64::from(first);
  for _ in 1..length {
    let [next] = take(source)?;
    value = value << 8 | u64::from(next);
  }
  Some((value, length))
}

fn element(elements: &[(u32, Range<u64>)], wanted: u32) -> Option<Range<u64>> {
  elements
    .iter()
    .find(|(id, _)| *id == wanted)
    .map(|(_, body)| body.clone())
}

fn unsigned<R: Read + Seek>(source: &mut R, body: Range<u64>) -> Option<u64> {
  let length = usize::try_from(body.end - body.start)
    .ok()
    .filter(|length| *length <= 8)?;
  at(source, body.start)?;
  let mut bytes = [0u8; 8];
  source.read_exact(&mut bytes[8 - length..]).ok()?;
  Some(u64::from_be_bytes(bytes))
}

fn matroska_duration<R: Read + Seek>(source: &mut R, info: Range<u64>) -> Option<u64> {
  let parts = elements(source, info);
  let scale = match element(&parts, EBML_TIMESTAMP_SCALE) {
    Some(scale) => unsigned(source, scale).filter(|scale| *scale > 0)?,
    None => EBML_DEFAULT_SCALE_NS,
  };
  let body = element(&parts, EBML_DURATION)?;
  at(source, body.start)?;
  let ticks = match body.end - body.start {
    4 => f64::from(f32::from_be_bytes(take(source)?)),
    8 => f64::from_be_bytes(take(source)?),
    _ => return None,
  };
  let milliseconds = ticks * scale as f64 / 1_000_000.0;
  (milliseconds.is_finite() && milliseconds >= 0.0).then(|| milliseconds.round() as u64)
}

#[cfg(test)]
mod tests {
  use proptest::prelude::*;

  use super::*;
  use crate::media::fixture::{
    self, Track, boxed, ebml, ebml_open, ftyp, gif, matroska, matroska_head, matroska_info, matroska_tracks, moov,
    movie_at_the_end, mvhd, recorded_matroska,
  };

  fn png(actl: Option<u32>, profile_bytes: usize) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
      out.extend_from_slice(&(data.len() as u32).to_be_bytes());
      out.extend_from_slice(kind);
      out.extend_from_slice(data);
      out.extend_from_slice(&[0, 0, 0, 0]);
    };
    chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    if profile_bytes > 0 {
      chunk(b"iCCP", &vec![7; profile_bytes]);
    }
    if let Some(frames) = actl {
      let mut data = frames.to_be_bytes().to_vec();
      data.extend_from_slice(&0u32.to_be_bytes());
      chunk(b"acTL", &data);
    }
    chunk(b"IDAT", &[0; 4]);
    chunk(b"IEND", &[]);
    out
  }

  fn webp(flags: u8) -> Vec<u8> {
    let mut out = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
    out.extend_from_slice(&10u32.to_le_bytes());
    out.extend_from_slice(&[flags, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out
  }

  fn probed(bytes: &[u8]) -> MediaProbe {
    probe(&mut Cursor::new(bytes)).unwrap()
  }

  fn movie_of(bytes: &[u8]) -> Option<Movie> {
    movie(&mut Cursor::new(bytes))
  }

  #[test]
  fn a_gif_is_animated_only_with_a_second_frame() {
    let still = sniff(&gif(1, 0));
    assert_eq!(
      (still.format, still.kind, still.animated),
      (MediaFormat::Gif, MediaKind::Image, false)
    );
    assert!(sniff(&gif(2, 0)).animated);
    assert_eq!(sniff(&gif(2, 0)).mime.as_deref(), Some("image/gif"));
  }

  #[test]
  fn a_second_frame_is_found_however_far_in_it_sits() {
    assert!(sniff(&gif(2, 3 * HEAD_BYTES as usize)).animated);
    assert!(!sniff(&gif(1, 3 * HEAD_BYTES as usize)).animated);
    assert!(sniff(&png(Some(4), 3 * HEAD_BYTES as usize)).animated);
  }

  #[test]
  fn a_truncated_gif_counts_what_it_holds() {
    let mut cut = gif(2, 0);
    cut.truncate(cut.len() - 8);
    assert!(sniff(&cut).animated);
    let mut head = gif(1, 0);
    head.truncate(20);
    assert!(!sniff(&head).animated);
    assert!(!sniff(b"GIF89a").animated);
  }

  #[test]
  fn a_png_is_animated_when_actl_declares_more_than_one_frame() {
    assert!(!sniff(&png(None, 0)).animated);
    assert!(!sniff(&png(Some(1), 0)).animated);
    let apng = sniff(&png(Some(4), 0));
    assert_eq!((apng.format, apng.animated), (MediaFormat::Png, true));
  }

  #[test]
  fn a_png_chunk_that_claims_the_whole_address_space_ends_the_walk() {
    let mut hostile = b"\x89PNG\r\n\x1a\n".to_vec();
    hostile.extend_from_slice(&u32::MAX.to_be_bytes());
    hostile.extend_from_slice(b"tEXt");
    hostile.extend_from_slice(&[0; 64]);
    assert!(!sniff(&hostile).animated);
    let mut cut = png(Some(4), 0);
    cut.truncate(8 + 25 + 8 + 2);
    assert!(!sniff(&cut).animated);
  }

  #[test]
  fn a_webp_is_animated_when_vp8x_carries_the_animation_flag() {
    assert!(sniff(&webp(0x12)).animated);
    assert!(!sniff(&webp(0x10)).animated);
    assert!(!sniff(b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0").animated);
    assert_eq!(sniff(&webp(0)).format, MediaFormat::Webp);
  }

  #[test]
  fn iso_brands_split_stills_from_video() {
    let heic = sniff(&ftyp(b"heic", &[b"mif1", b"heic"]));
    assert_eq!(
      (heic.format, heic.kind, heic.mime.as_deref()),
      (MediaFormat::Heif, MediaKind::Image, Some("image/heic"))
    );
    assert_eq!(sniff(&ftyp(b"mif1", &[b"heix"])).format, MediaFormat::Heif);
    assert_eq!(sniff(&ftyp(b"mif1", &[b"miaf"])).mime.as_deref(), Some("image/heif"));

    let avif = sniff(&ftyp(b"avif", &[b"mif1", b"miaf"]));
    assert_eq!((avif.format, avif.animated), (MediaFormat::Avif, false));
    assert!(sniff(&ftyp(b"avis", &[b"msf1"])).animated);
    assert_eq!(sniff(&ftyp(b"mif1", &[b"avif"])).format, MediaFormat::Avif);

    let mp4 = sniff(&ftyp(b"isom", &[b"iso2", b"avc1", b"mp41"]));
    assert_eq!(
      (mp4.format, mp4.kind, mp4.mime.as_deref()),
      (MediaFormat::IsoVideo, MediaKind::Video, Some("video/mp4"))
    );
    assert_eq!(
      sniff(&ftyp(b"qt  ", &[b"qt  "])).mime.as_deref(),
      Some("video/quicktime")
    );
    assert_eq!(sniff(&ftyp(b"3gp4", &[b"isom"])).mime.as_deref(), Some("video/3gpp"));
    assert_eq!(sniff(&ftyp(b"3g2a", &[b"isom"])).mime.as_deref(), Some("video/3gpp2"));
  }

  #[test]
  fn an_iso_brand_that_is_not_video_is_not_called_video() {
    for brand in [b"M4A ", b"M4B ", b"M4P ", b"F4A ", b"F4B "] {
      let audio = sniff(&ftyp(brand, &[b"isom", b"mp42"]));
      assert_eq!(
        (audio.format, audio.kind, audio.mime.as_deref()),
        (MediaFormat::Unknown, MediaKind::Other, Some("audio/mp4"))
      );
    }
    let raw = sniff(&ftyp(b"crx ", &[b"crx ", b"isom"]));
    assert_eq!(
      (raw.kind, raw.mime.as_deref()),
      (MediaKind::Other, Some("image/x-canon-cr3"))
    );
  }

  #[test]
  fn an_iso_file_with_no_video_track_is_not_a_video() {
    let narrated = probed(&fixture::movie(60_000, &[Track::Audio]));
    assert_eq!(
      (
        narrated.sniff.format,
        narrated.sniff.kind,
        narrated.sniff.mime.as_deref(),
        narrated.movie
      ),
      (
        MediaFormat::Unknown,
        MediaKind::Other,
        Some("audio/mp4"),
        Some(Movie {
          duration_ms: Some(60_000),
          has_video: false,
          has_audio: true,
        })
      )
    );
    assert_eq!(
      probed(&fixture::movie(60_000, &[])).sniff.mime.as_deref(),
      Some("application/mp4")
    );

    let filmed = probed(&fixture::movie(60_000, &[Track::Audio, Track::Video]));
    assert_eq!(
      (filmed.sniff.kind, filmed.sniff.mime.as_deref()),
      (MediaKind::Video, Some("video/mp4"))
    );
    assert_eq!(
      filmed.movie,
      Some(Movie {
        duration_ms: Some(60_000),
        has_video: true,
        has_audio: true,
      })
    );
    assert_eq!(
      filmed.size,
      fixture::movie(60_000, &[Track::Audio, Track::Video]).len() as u64
    );
  }

  #[test]
  fn an_audio_brand_still_takes_a_track_census() {
    let voice = [
      ftyp(b"M4A ", &[b"isom", b"mp42"]),
      moov(mvhd(0, 1000, 4000), &[Track::Audio]),
    ]
    .concat();
    let probed = probed(&voice);
    assert_eq!(
      (probed.sniff.kind, probed.sniff.mime.as_deref()),
      (MediaKind::Other, Some("audio/mp4"))
    );
    assert_eq!(
      probed.movie.map(|movie| (movie.has_video, movie.has_audio)),
      Some((false, true))
    );
    assert_eq!(
      super::probe(&mut Cursor::new(ftyp(b"heic", &[b"mif1"]))).unwrap().movie,
      None
    );
  }

  #[test]
  fn the_movie_box_is_found_wherever_it_sits() {
    let tracks = [Track::Video];
    let trailing = movie_of(&movie_at_the_end(7_500, &tracks, 3 * HEAD_BYTES as usize)).unwrap();
    assert_eq!((trailing.duration_ms, trailing.has_video), (Some(7_500), true));

    let mut wide = ftyp(b"isom", &[b"mp42"]);
    wide.extend_from_slice(&1u32.to_be_bytes());
    wide.extend_from_slice(b"mdat");
    wide.extend_from_slice(&(16u64 + 32).to_be_bytes());
    wide.extend_from_slice(&[0xCD; 32]);
    wide.extend_from_slice(&moov(mvhd(0, 600, 1200), &tracks));
    assert_eq!(movie_of(&wide).unwrap().duration_ms, Some(2000));

    let mut bare = boxed(b"wide", &[]);
    bare.extend_from_slice(&moov(mvhd(0, 600, 300), &tracks));
    assert_eq!(probed(&bare).movie.unwrap().duration_ms, Some(500));
  }

  #[test]
  fn a_movie_reports_its_duration_in_either_header_version() {
    let tracks = [Track::Video];
    assert_eq!(
      movie_of(&moov(mvhd(0, 1000, 3067), &tracks)).unwrap().duration_ms,
      Some(3067)
    );
    assert_eq!(
      movie_of(&moov(mvhd(1, 90_000, 180_000), &tracks)).unwrap().duration_ms,
      Some(2000)
    );
    assert_eq!(
      movie_of(&moov(mvhd(1, 1000, u64::from(u32::MAX) + 1000), &tracks))
        .unwrap()
        .duration_ms,
      Some(u64::from(u32::MAX) + 1000)
    );
    for unknown in [
      mvhd(0, 0, 3067),
      mvhd(0, 1000, u64::from(u32::MAX)),
      mvhd(1, 1000, u64::MAX),
      mvhd(2, 1000, 3067),
      boxed(b"udta", &[0; 8]),
    ] {
      let found = movie_of(&moov(unknown, &tracks)).unwrap();
      assert_eq!((found.duration_ms, found.has_video), (None, true));
    }
  }

  #[test]
  fn a_matroska_file_with_a_picture_is_a_video_and_reports_its_length() {
    for (doctype, mime) in [("webm", "video/webm"), ("matroska", "video/x-matroska")] {
      let filmed = probed(&matroska(doctype, 125_000, &[Track::Audio, Track::Video]));
      assert_eq!(
        (filmed.sniff.format, filmed.sniff.kind, filmed.sniff.mime.as_deref()),
        (MediaFormat::Matroska, MediaKind::Video, Some(mime))
      );
      assert_eq!(
        filmed.movie,
        Some(Movie {
          duration_ms: Some(125_000),
          has_video: true,
          has_audio: true,
        })
      );
    }
  }

  #[test]
  fn an_audio_only_matroska_file_is_not_a_video() {
    for (doctype, mime) in [("webm", "audio/webm"), ("matroska", "audio/x-matroska")] {
      for bytes in [
        matroska(doctype, 60_000, &[Track::Audio]),
        recorded_matroska(doctype, &[Track::Audio]),
      ] {
        let heard = probed(&bytes);
        assert_eq!(
          (heard.sniff.format, heard.sniff.kind, heard.sniff.mime.as_deref()),
          (MediaFormat::Unknown, MediaKind::Other, Some(mime)),
          "{doctype}"
        );
        assert_eq!(heard.movie.map(|movie| movie.has_audio), Some(true));
      }
    }
    let captions = probed(&matroska("matroska", 1000, &[Track::Text]));
    assert_eq!(
      (captions.sniff.kind, captions.sniff.mime.as_deref()),
      (MediaKind::Other, Some("application/x-matroska"))
    );
  }

  #[test]
  fn a_recorded_matroska_stream_of_unknown_length_is_still_counted() {
    let recorded = probed(&recorded_matroska("webm", &[Track::Video, Track::Audio]));
    assert_eq!(recorded.sniff.kind, MediaKind::Video);
    assert_eq!(
      recorded.movie,
      Some(Movie {
        duration_ms: None,
        has_video: true,
        has_audio: true,
      })
    );
  }

  #[test]
  fn a_child_header_straddling_its_parents_end_is_not_read_past_it() {
    let segment = [
      matroska_tracks(&[Track::Video]),
      ebml(EBML_INFO, &[0x44, 0x89]),
      vec![0xFF],
    ]
    .concat();
    let crafted = [matroska_head("webm"), ebml(EBML_SEGMENT, &segment)].concat();
    assert_eq!(
      probed(&crafted).movie,
      Some(Movie {
        duration_ms: None,
        has_video: true,
        has_audio: false,
      })
    );
  }

  fn container_heads() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
      Just(Vec::new()),
      Just(matroska_head("webm")),
      Just([matroska_head("webm"), vec![0x18, 0x53, 0x80, 0x67]].concat()),
      Just(
        [
          matroska_head("webm"),
          ebml_open(
            EBML_SEGMENT,
            &[matroska_tracks(&[Track::Video]), vec![0x15, 0x49, 0xA9, 0x66]].concat()
          ),
        ]
        .concat()
      ),
      Just(ftyp(b"isom", &[b"isom", b"mp42"])),
      Just([ftyp(b"isom", &[b"mp42"]), b"\0\0\0\0moov".to_vec()].concat()),
      Just(b"\x89PNG\r\n\x1a\n".to_vec()),
      Just(b"GIF89a".to_vec()),
      Just(b"RIFF\0\0\0\0WEBPVP8X".to_vec()),
    ]
  }

  proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn no_byte_sequence_makes_the_probe_panic(head in container_heads(), tail in proptest::collection::vec(any::<u8>(), 0..512)) {
      let bytes = [head, tail].concat();
      let probed = probe(&mut Cursor::new(&bytes)).unwrap();
      prop_assert_eq!(probed.size, bytes.len() as u64);
    }
  }

  #[test]
  fn a_matroska_file_the_census_cannot_read_stays_a_video_container() {
    let whole = matroska("webm", 5000, &[Track::Audio]);
    for cut in [matroska_head("webm").len() + 6, matroska_head("webm").len()] {
      let truncated = probed(&whole[..cut]);
      assert_eq!(
        (truncated.sniff.kind, truncated.sniff.mime.as_deref(), truncated.movie),
        (MediaKind::Video, Some("video/webm"), None)
      );
    }
    let trackless = probed(&[matroska_head("webm"), ebml(0x1853_8067, &matroska_info(5000.0))].concat());
    assert_eq!((trackless.sniff.kind, trackless.movie), (MediaKind::Video, None));
  }

  #[test]
  fn a_matroska_duration_follows_its_timestamp_scale_and_float_width() {
    let tracks = matroska_tracks(&[Track::Video]);
    let segment = |info: Vec<u8>| {
      let bytes = [
        matroska_head("webm"),
        ebml(0x1853_8067, &[info, tracks.clone()].concat()),
      ]
      .concat();
      probed(&bytes).movie.unwrap().duration_ms
    };
    let scaled =
      |scale: &[u8], duration: &[u8]| ebml(0x1549_A966, &[ebml(0x2A_D7B1, scale), ebml(0x4489, duration)].concat());
    assert_eq!(segment(scaled(&[0x0F, 0x42, 0x40], &2500f32.to_be_bytes())), Some(2500));
    assert_eq!(segment(scaled(&[0x01], &1_500_000f64.to_be_bytes())), Some(2));
    assert_eq!(
      segment(ebml(0x1549_A966, &ebml(0x4489, &3000f64.to_be_bytes()))),
      Some(3000)
    );
    assert_eq!(segment(scaled(&[0x00], &3000f64.to_be_bytes())), None);
    assert_eq!(segment(scaled(&[0x0F, 0x42, 0x40], &[0; 3])), None);
    assert_eq!(segment(scaled(&[0x0F, 0x42, 0x40], &f64::NAN.to_be_bytes())), None);
  }

  #[test]
  fn a_broken_box_tree_yields_no_movie_instead_of_a_wrong_one() {
    let whole = fixture::movie(5000, &[Track::Video]);
    for cut in [whole.len() - 1, whole.len() - 40, 30] {
      assert_eq!(movie_of(&whole[..cut]), None);
      assert_eq!(sniff(&whole[..cut]).kind, MediaKind::Video);
    }

    let mut undersized = ftyp(b"isom", &[]);
    undersized.extend_from_slice(&4u32.to_be_bytes());
    undersized.extend_from_slice(b"moov");
    assert_eq!(movie_of(&undersized), None);

    let mut open_ended = ftyp(b"isom", &[]);
    open_ended.extend_from_slice(&0u32.to_be_bytes());
    open_ended.extend_from_slice(b"mdat");
    open_ended.extend_from_slice(&moov(mvhd(0, 1000, 1000), &[Track::Video]));
    assert_eq!(movie_of(&open_ended), None);

    let crowded = [
      ftyp(b"isom", &[]),
      boxed(b"free", &[]).repeat(BOX_LIMIT),
      moov(mvhd(0, 1000, 1000), &[Track::Video]),
    ]
    .concat();
    assert_eq!(movie_of(&crowded), None);
    assert_eq!(movie_of(&[]), None);
  }

  #[test]
  fn a_quicktime_file_without_ftyp_is_still_video() {
    let mut bytes = 8u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(b"wide");
    bytes.extend_from_slice(&[0; 8]);
    assert_eq!(sniff(&bytes).format, MediaFormat::IsoVideo);
  }

  #[test]
  fn only_magic_that_names_one_type_outranks_what_the_host_says() {
    for exact in [
      sniff(&[0xFF, 0xD8, 0xFF, 0xE0]),
      sniff(&gif(1, 0)),
      sniff(&ftyp(b"heic", &[b"mif1"])),
      sniff(b"RIFF\0\0\0\0AVI LIST"),
    ] {
      assert!(exact.names_its_type(), "{exact:?}");
    }
    for container in [
      sniff(&ftyp(b"isom", &[b"mp42"])),
      sniff(&ftyp(b"M4A ", &[b"isom"])),
      sniff(&[0x1A, 0x45, 0xDF, 0xA3, 0x9F]),
      sniff(b"II*\0rest"),
      sniff(b"%PDF-1.7"),
    ] {
      assert!(!container.names_its_type(), "{container:?}");
    }
  }

  #[test]
  fn the_rest_of_the_magic_table() {
    assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]).format, MediaFormat::Jpeg);
    assert_eq!(sniff(b"II*\0rest").format, MediaFormat::Tiff);
    let mut bmp = b"BM".to_vec();
    bmp.extend_from_slice(&[0; 12]);
    bmp.extend_from_slice(&40u32.to_le_bytes());
    assert_eq!(sniff(&bmp).format, MediaFormat::Bmp);
    assert_eq!(sniff(b"RIFF\0\0\0\0AVI LIST").kind, MediaKind::Video);
    let mut webm = vec![0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x82, 0x84];
    webm.extend_from_slice(b"webm");
    assert_eq!(sniff(&webm).mime.as_deref(), Some("video/webm"));
    let unknown = sniff(b"%PDF-1.7");
    assert_eq!(
      (unknown.format, unknown.kind, unknown.mime),
      (MediaFormat::Unknown, MediaKind::Other, None)
    );
    assert_eq!(sniff(&[]).kind, MediaKind::Other);
  }
}
