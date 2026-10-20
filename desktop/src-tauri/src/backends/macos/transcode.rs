use std::{
  cell::Cell,
  path::Path,
  ptr::NonNull,
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
  },
  time::Duration,
};

use block2::RcBlock;
use bridgething_companion::{
  backend::{MediaTranscoder, TranscodeJob, TranscodeRequest, TranscodeSink, TranscodedVideo},
  media::{StillFormat, VideoPlan, plan_video},
};
use dispatch2::DispatchQueue;
use objc2::{
  Message,
  rc::{Retained, Weak, autoreleasepool},
  runtime::{AnyObject, ProtocolObject},
};
use objc2_av_foundation::{
  AVAssetImageGenerator, AVAssetImageGeneratorResult, AVAssetReader, AVAssetReaderStatus,
  AVAssetReaderVideoCompositionOutput, AVAssetTrack, AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus,
  AVFileTypeMPEG4, AVMediaTypeVideo, AVMutableVideoComposition, AVMutableVideoCompositionInstruction,
  AVMutableVideoCompositionLayerInstruction, AVURLAsset, AVVideoAverageBitRateKey, AVVideoCodecKey,
  AVVideoCodecTypeH264, AVVideoColorPrimaries_ITU_R_709_2, AVVideoColorPrimariesKey, AVVideoColorPropertiesKey,
  AVVideoCompressionPropertiesKey, AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey,
  AVVideoMaxKeyFrameIntervalDurationKey, AVVideoProfileLevelH264HighAutoLevel, AVVideoProfileLevelKey,
  AVVideoTransferFunction_ITU_R_709_2, AVVideoTransferFunctionKey, AVVideoWidthKey, AVVideoYCbCrMatrix_ITU_R_709_2,
  AVVideoYCbCrMatrixKey, NSValueAVFoundationExtensions,
};
use objc2_core_foundation::{CFRetained, CGAffineTransform, CGSize};
use objc2_core_graphics::CGImage;
use objc2_core_media::{CMTime, CMTimeRange, kCMTimeZero};
use objc2_core_video::{kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString, NSURL, NSValue};

use super::contain::contained;

type Settings = NSDictionary<NSString, AnyObject>;

const STALL_CHECK: Duration = Duration::from_millis(250);
const POSTER_DEADLINE: Duration = Duration::from_secs(30);
const POSTER_QUALITY: f32 = 0.9;
const FRAME_TICKS: i32 = 1000;
const CANCELLED: &str = "the transcode was cancelled";

pub struct AvMediaTranscoder;

struct AvJob(Arc<AtomicBool>);

impl TranscodeJob for AvJob {
  fn cancel(&self) {
    self.0.store(true, Ordering::SeqCst);
  }
}

impl MediaTranscoder for AvMediaTranscoder {
  fn transcode(&self, request: TranscodeRequest, sink: Arc<TranscodeSink>) -> Arc<dyn TranscodeJob> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let job = Arc::new(AvJob(Arc::clone(&cancelled)));
    let worker = std::thread::Builder::new().name("media-transcode".into());
    let failed = Arc::clone(&sink);
    let spawned = worker.spawn(move || settle(&sink, &request.destination, || transcode(&request, &cancelled)));
    if let Err(error) = spawned {
      failed.fail(format!("could not start the transcode worker: {error}"));
    }
    job
  }

  fn poster(&self, source: String) -> Option<Vec<u8>> {
    contained(|| poster(&source))
      .inspect_err(|thrown| tracing::warn!(%thrown, %source, "avfoundation threw while reading a poster frame"))
      .ok()
      .flatten()
  }
}

fn settle(sink: &TranscodeSink, destination: &str, work: impl FnOnce() -> Result<TranscodedVideo, String>) {
  match contained(work).and_then(|outcome| outcome) {
    Ok(video) => sink.complete(video),
    Err(reason) => {
      let _ = std::fs::remove_file(destination);
      sink.fail(reason);
    }
  }
}

fn poster(source: &str) -> Option<Vec<u8>> {
  let url = NSURL::fileURLWithPath(&NSString::from_str(source));
  let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };
  let generator = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset) };
  unsafe { generator.setAppliesPreferredTrackTransform(true) };
  let (frames, frame) = mpsc::channel::<Option<CFRetained<CGImage>>>();
  let handler = RcBlock::new(
    move |_: CMTime, image: *mut CGImage, _: CMTime, _: AVAssetImageGeneratorResult, _: *mut NSError| {
      let _ = frames.send(NonNull::new(image).map(|image| unsafe { CFRetained::retain(image) }));
    },
  );
  let first = NSArray::from_retained_slice(&[unsafe { NSValue::valueWithCMTime(kCMTimeZero) }]);
  unsafe { generator.generateCGImagesAsynchronouslyForTimes_completionHandler(&first, RcBlock::as_ptr(&handler)) };
  let image = frame.recv_timeout(POSTER_DEADLINE).ok()??;
  super::image::encode(&image, StillFormat::Jpeg, POSTER_QUALITY)
}

struct Geometry {
  display: (u32, u32),
  orient: CGAffineTransform,
}

fn transcode(request: &TranscodeRequest, cancelled: &Arc<AtomicBool>) -> Result<TranscodedVideo, String> {
  let source = NSURL::fileURLWithPath(&NSString::from_str(&request.source));
  let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&source, None) };
  let media_type = constant(unsafe { AVMediaTypeVideo })?;
  let track = unsafe { asset.tracks() }
    .iter()
    .find(|track| unsafe { track.mediaType() }.isEqualToString(media_type))
    .ok_or("the source has no video track")?;

  let geometry = geometry(&track)?;
  let plan = plan_video(
    geometry.display.0,
    geometry.display.1,
    request.cover_width,
    request.cover_height,
    request.max_bitrate,
  );
  let fps = frame_rate(unsafe { track.nominalFrameRate() }, plan.max_fps);
  let duration = unsafe { asset.duration() };

  let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(&asset) }.map_err(describe)?;
  let output = composition_output(&track, &geometry, &plan, fps, duration)?;
  if !unsafe { reader.canAddOutput(&output) } {
    return Err("the reader refused the composition output".into());
  }
  unsafe { reader.addOutput(&output) };

  let destination = Path::new(&request.destination);
  match std::fs::remove_file(destination) {
    Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.to_string()),
    _ => {}
  }
  let url = NSURL::fileURLWithPath(&NSString::from_str(&request.destination));
  let container = constant(unsafe { AVFileTypeMPEG4 })?;
  let writer = unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&url, container) }.map_err(describe)?;
  unsafe { writer.setShouldOptimizeForNetworkUse(true) };
  let settings = encoder_settings(&plan, fps)?;
  if !unsafe { writer.canApplyOutputSettings_forMediaType(Some(&settings), media_type) } {
    return Err("the h.264 encoder refused the output settings".into());
  }
  let input = unsafe { AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(media_type, Some(&settings)) };
  unsafe { input.setExpectsMediaDataInRealTime(false) };
  if !unsafe { writer.canAddInput(&input) } {
    return Err("the writer refused the video input".into());
  }
  unsafe { writer.addInput(&input) };

  if cancelled.load(Ordering::SeqCst) {
    return Err(CANCELLED.into());
  }
  if !unsafe { reader.startReading() } {
    return Err(reader_error(&reader));
  }
  if !unsafe { writer.startWriting() } {
    unsafe { reader.cancelReading() };
    return Err(writer_error(&writer));
  }
  unsafe { writer.startSessionAtSourceTime(kCMTimeZero) };

  let fed = pump(&output, &input, &reader, &writer, cancelled);
  let read = if unsafe { reader.status() } == AVAssetReaderStatus::Failed {
    Err(reader_error(&reader))
  } else {
    Ok(())
  };
  if let Err(reason) = read.and(fed) {
    unsafe { reader.cancelReading() };
    unsafe { writer.cancelWriting() };
    return Err(reason);
  }

  finish(&writer)?;
  let written = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };
  let seconds = unsafe { written.duration().seconds() };
  Ok(TranscodedVideo {
    width: plan.width,
    height: plan.height,
    duration_ms: (seconds * 1000.0).round().max(0.0) as u64,
  })
}

fn geometry(track: &AVAssetTrack) -> Result<Geometry, String> {
  let natural = unsafe { track.naturalSize() };
  let transform = unsafe { track.preferredTransform() };
  let corners = [
    (0.0, 0.0),
    (natural.width, 0.0),
    (0.0, natural.height),
    (natural.width, natural.height),
  ]
  .map(|(x, y)| {
    (
      transform.a * x + transform.c * y + transform.tx,
      transform.b * x + transform.d * y + transform.ty,
    )
  });
  let (left, right) = span(corners.map(|(x, _)| x));
  let (top, bottom) = span(corners.map(|(_, y)| y));
  let display = ((right - left).round() as u32, (bottom - top).round() as u32);
  if display.0 == 0 || display.1 == 0 {
    return Err("the source video track has no picture".into());
  }
  Ok(Geometry {
    display,
    orient: CGAffineTransform {
      tx: transform.tx - left,
      ty: transform.ty - top,
      ..transform
    },
  })
}

fn span(values: [f64; 4]) -> (f64, f64) {
  values
    .into_iter()
    .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
      (low.min(value), high.max(value))
    })
}

fn frame_rate(nominal: f32, max_fps: u32) -> f64 {
  let ceiling = f64::from(max_fps);
  let slowest = 1.0 / f64::from(FRAME_TICKS);
  if nominal > 0.0 {
    f64::from(nominal).min(ceiling).max(slowest)
  } else {
    ceiling
  }
}

fn frame_timescale(fps: f64) -> i32 {
  (fps * f64::from(FRAME_TICKS)).round() as i32
}

fn composition_output(
  track: &AVAssetTrack,
  geometry: &Geometry,
  plan: &VideoPlan,
  fps: f64,
  duration: CMTime,
) -> Result<Retained<AVAssetReaderVideoCompositionOutput>, String> {
  let sx = f64::from(plan.width) / f64::from(geometry.display.0);
  let sy = f64::from(plan.height) / f64::from(geometry.display.1);
  let oriented = geometry.orient;
  let fitted = CGAffineTransform {
    a: oriented.a * sx,
    b: oriented.b * sy,
    c: oriented.c * sx,
    d: oriented.d * sy,
    tx: oriented.tx * sx,
    ty: oriented.ty * sy,
  };

  let layer =
    unsafe { AVMutableVideoCompositionLayerInstruction::videoCompositionLayerInstructionWithAssetTrack(track) };
  unsafe { layer.setTransform_atTime(fitted, kCMTimeZero) };
  let instruction = unsafe { AVMutableVideoCompositionInstruction::videoCompositionInstruction() };
  unsafe {
    instruction.setTimeRange(CMTimeRange {
      start: kCMTimeZero,
      duration,
    })
  };
  unsafe { instruction.setLayerInstructions(&NSArray::from_retained_slice(&[Retained::into_super(layer)])) };

  let composition = unsafe { AVMutableVideoComposition::videoComposition() };
  unsafe {
    composition.setRenderSize(CGSize::new(f64::from(plan.width), f64::from(plan.height)));
    composition.setFrameDuration(CMTime::new(i64::from(FRAME_TICKS), frame_timescale(fps)));
    composition.setColorPrimaries(Some(constant(AVVideoColorPrimaries_ITU_R_709_2)?));
    composition.setColorTransferFunction(Some(constant(AVVideoTransferFunction_ITU_R_709_2)?));
    composition.setColorYCbCrMatrix(Some(constant(AVVideoYCbCrMatrix_ITU_R_709_2)?));
    composition.setInstructions(&NSArray::from_retained_slice(&[ProtocolObject::from_retained(
      instruction,
    )]));
  }

  let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
  let pixels = settings(&[(unsafe { kCVPixelBufferPixelFormatTypeKey }.as_ref(), format.as_ref())]);
  let tracks = NSArray::from_slice(&[track]);
  let output = unsafe {
    AVAssetReaderVideoCompositionOutput::assetReaderVideoCompositionOutputWithVideoTracks_videoSettings(
      &tracks,
      Some(&pixels),
    )
  };
  unsafe {
    output.setAlwaysCopiesSampleData(false);
    output.setVideoComposition(Some(&composition));
  }
  Ok(output)
}

fn encoder_settings(plan: &VideoPlan, fps: f64) -> Result<Retained<Settings>, String> {
  let bitrate = NSNumber::new_u32(plan.bitrate);
  let keyframes = NSNumber::new_u32(plan.keyframe_seconds);
  let expected = NSNumber::new_f64(fps);
  let compression = settings(&[
    (constant(unsafe { AVVideoAverageBitRateKey })?, bitrate.as_ref()),
    (
      constant(unsafe { AVVideoProfileLevelKey })?,
      constant(unsafe { AVVideoProfileLevelH264HighAutoLevel })?.as_ref(),
    ),
    (
      constant(unsafe { AVVideoMaxKeyFrameIntervalDurationKey })?,
      keyframes.as_ref(),
    ),
    (
      constant(unsafe { AVVideoExpectedSourceFrameRateKey })?,
      expected.as_ref(),
    ),
  ]);
  let color = settings(&[
    (
      constant(unsafe { AVVideoColorPrimariesKey })?,
      constant(unsafe { AVVideoColorPrimaries_ITU_R_709_2 })?.as_ref(),
    ),
    (
      constant(unsafe { AVVideoTransferFunctionKey })?,
      constant(unsafe { AVVideoTransferFunction_ITU_R_709_2 })?.as_ref(),
    ),
    (
      constant(unsafe { AVVideoYCbCrMatrixKey })?,
      constant(unsafe { AVVideoYCbCrMatrix_ITU_R_709_2 })?.as_ref(),
    ),
  ]);
  let width = NSNumber::new_u32(plan.width);
  let height = NSNumber::new_u32(plan.height);
  Ok(settings(&[
    (
      constant(unsafe { AVVideoCodecKey })?,
      constant(unsafe { AVVideoCodecTypeH264 })?.as_ref(),
    ),
    (constant(unsafe { AVVideoWidthKey })?, width.as_ref()),
    (constant(unsafe { AVVideoHeightKey })?, height.as_ref()),
    (
      constant(unsafe { AVVideoCompressionPropertiesKey })?,
      compression.as_ref(),
    ),
    (constant(unsafe { AVVideoColorPropertiesKey })?, color.as_ref()),
  ]))
}

enum Fed {
  Drained,
  Refused,
  Cancelled,
}

fn feed(source: &AVAssetReaderVideoCompositionOutput, target: &AVAssetWriterInput, stop: &AtomicBool) -> Option<Fed> {
  while unsafe { target.isReadyForMoreMediaData() } {
    if stop.load(Ordering::SeqCst) {
      return Some(Fed::Cancelled);
    }
    let fed = autoreleasepool(|_| {
      let Some(sample) = (unsafe { source.copyNextSampleBuffer() }) else {
        unsafe { target.markAsFinished() };
        return Some(Fed::Drained);
      };
      (!unsafe { target.appendSampleBuffer(&sample) }).then_some(Fed::Refused)
    });
    if fed.is_some() {
      return fed;
    }
  }
  None
}

fn pump(
  output: &AVAssetReaderVideoCompositionOutput,
  input: &AVAssetWriterInput,
  reader: &AVAssetReader,
  writer: &AVAssetWriter,
  cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
  let (done, settled) = mpsc::channel();
  let queue = DispatchQueue::new("com.bridgething.desktop.transcode", None);
  let source = output.retain();
  let target = Weak::new(input);
  let over = Cell::new(false);
  let stop = Arc::clone(cancelled);
  let block = RcBlock::new(move || {
    if over.get() {
      return;
    }
    let Some(target) = target.load() else {
      return;
    };
    let fed = match contained(|| feed(&source, &target, &stop)) {
      Ok(None) => return,
      Ok(Some(fed)) => Ok(fed),
      Err(thrown) => Err(thrown),
    };
    over.set(true);
    let _ = done.send(fed);
  });
  unsafe { input.requestMediaDataWhenReadyOnQueue_usingBlock(&queue, &block) };
  loop {
    match settled.recv_timeout(STALL_CHECK) {
      Ok(Ok(Fed::Drained)) => return Ok(()),
      Ok(Ok(Fed::Cancelled)) => return Err(CANCELLED.into()),
      Ok(Err(thrown)) => return Err(thrown),
      Ok(Ok(Fed::Refused)) | Err(mpsc::RecvTimeoutError::Disconnected) => return Err(writer_error(writer)),
      Err(mpsc::RecvTimeoutError::Timeout) if cancelled.load(Ordering::SeqCst) => return Err(CANCELLED.into()),
      Err(mpsc::RecvTimeoutError::Timeout) => {
        let stalled = unsafe { writer.status() } != AVAssetWriterStatus::Writing
          || unsafe { reader.status() } == AVAssetReaderStatus::Failed;
        if stalled {
          return Err(writer_error(writer));
        }
      }
    }
  }
}

fn finish(writer: &AVAssetWriter) -> Result<(), String> {
  let (done, finished) = mpsc::channel();
  let handler = RcBlock::new(move || {
    let _ = done.send(());
  });
  unsafe { writer.finishWritingWithCompletionHandler(&handler) };
  finished.recv().map_err(|_| "the writer never finished".to_owned())?;
  if unsafe { writer.status() } == AVAssetWriterStatus::Completed {
    Ok(())
  } else {
    Err(writer_error(writer))
  }
}

fn settings(entries: &[(&NSString, &AnyObject)]) -> Retained<Settings> {
  let keys: Vec<&NSString> = entries.iter().map(|(key, _)| *key).collect();
  let values: Vec<&AnyObject> = entries.iter().map(|(_, value)| *value).collect();
  NSDictionary::from_slices(&keys, &values)
}

fn constant<T: ?Sized>(value: Option<&'static T>) -> Result<&'static T, String> {
  value.ok_or_else(|| "avfoundation is missing a settings constant".to_owned())
}

fn reader_error(reader: &AVAssetReader) -> String {
  unsafe { reader.error() }.map_or_else(|| "the reader failed".to_owned(), describe)
}

fn writer_error(writer: &AVAssetWriter) -> String {
  unsafe { writer.error() }.map_or_else(|| "the writer failed".to_owned(), describe)
}

fn describe(error: Retained<NSError>) -> String {
  error.localizedDescription().to_string()
}

#[cfg(test)]
mod tests {
  use std::{
    path::{Path, PathBuf},
    ptr::NonNull,
    time::Duration,
  };

  use bridgething_companion::backend::{ImageTarget, sniff_and_scale};
  use objc2_av_foundation::{
    AVAssetReaderTrackOutput, AVAssetWriterInputPixelBufferAdaptor, AVFileTypeQuickTimeMovie, AVMediaTypeAudio,
    AVVideoCodecTypeHEVC, AVVideoColorPrimaries_ITU_R_2020, AVVideoTransferFunction_ITU_R_2100_HLG,
    AVVideoYCbCrMatrix_ITU_R_2020,
  };
  use objc2_avf_audio::{AVFormatIDKey, AVNumberOfChannelsKey, AVSampleRateKey};
  use objc2_core_audio_types::{
    AudioStreamBasicDescription, kAudioFormatFlagIsPacked, kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM,
    kAudioFormatMPEG4AAC,
  };
  use objc2_core_foundation::{CFData, CFDictionary, CFRetained, CFString};
  use objc2_core_media::{
    CMAudioFormatDescriptionCreate, CMAudioSampleBufferCreateReadyWithPacketDescriptions, CMBlockBuffer,
    CMFormatDescription, CMSampleBuffer, kCMBlockBufferAssureMemoryNowFlag,
    kCMFormatDescriptionExtension_ColorPrimaries, kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms,
    kCMFormatDescriptionExtension_TransferFunction, kCMFormatDescriptionExtension_YCbCrMatrix,
  };
  use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool,
    CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey, kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
  };

  use super::*;

  const SAMPLE_RATE: i32 = 44_100;
  const AVC1: u32 = u32::from_be_bytes(*b"avc1");
  const HIGH_PROFILE: u8 = 100;
  const BT709: [&str; 3] = ["ITU_R_709_2", "ITU_R_709_2", "ITU_R_709_2"];
  const ROTATED: CGAffineTransform = CGAffineTransform {
    a: 0.0,
    b: 1.0,
    c: -1.0,
    d: 0.0,
    tx: 1080.0,
    ty: 0.0,
  };
  const UPRIGHT: CGAffineTransform = CGAffineTransform {
    a: 1.0,
    b: 0.0,
    c: 0.0,
    d: 1.0,
    tx: 0.0,
    ty: 0.0,
  };

  struct Clip {
    width: usize,
    height: usize,
    fps: i32,
    stride: i64,
    frames: i32,
    transform: CGAffineTransform,
    audio: bool,
    hlg: bool,
    grain: u32,
  }

  fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(path.to_str().expect("a utf-8 path")))
  }

  fn ready(input: &AVAssetWriterInput) {
    while !unsafe { input.isReadyForMoreMediaData() } {
      std::thread::sleep(Duration::from_millis(1));
    }
  }

  fn bridged(key: &CFString) -> &NSString {
    key.as_ref()
  }

  fn frame(pool: &CVPixelBufferPool, clip: &Clip) -> CFRetained<CVPixelBuffer> {
    let mut out = std::ptr::null_mut();
    let status = unsafe { CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut out)) };
    assert_eq!(status, 0, "the pool hands out a pixel buffer");
    let buffer = unsafe { CFRetained::from_raw(NonNull::new(out).expect("a pixel buffer")) };
    unsafe { CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty()) };
    let stride = CVPixelBufferGetBytesPerRow(&buffer);
    let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
    let mut state = (stride as u32).wrapping_mul(2_654_435_761) ^ (base as usize as u32) | 1;
    for y in 0..clip.height {
      let row = unsafe { std::slice::from_raw_parts_mut(base.add(y * stride), clip.width * 4) };
      for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let marked = x < clip.width / 2 && y < clip.height / 2;
        *pixel = if marked { [0, 0, 255, 255] } else { [0, 0, 0, 255] };
        if clip.grain > 0 {
          for channel in &mut pixel[..3] {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *channel = channel.saturating_add((state % clip.grain) as u8);
          }
        }
      }
    }
    unsafe { CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty()) };
    buffer
  }

  fn tone_format() -> CFRetained<CMFormatDescription> {
    let description = AudioStreamBasicDescription {
      mSampleRate: f64::from(SAMPLE_RATE),
      mFormatID: kAudioFormatLinearPCM,
      mFormatFlags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
      mBytesPerPacket: 2,
      mFramesPerPacket: 1,
      mBytesPerFrame: 2,
      mChannelsPerFrame: 1,
      mBitsPerChannel: 16,
      mReserved: 0,
    };
    let mut out = std::ptr::null();
    let status = unsafe {
      CMAudioFormatDescriptionCreate(
        None,
        NonNull::from(&description),
        0,
        std::ptr::null(),
        0,
        std::ptr::null(),
        None,
        NonNull::from(&mut out),
      )
    };
    assert_eq!(status, 0, "the pcm format describes");
    unsafe { CFRetained::from_raw(NonNull::new(out.cast_mut()).expect("a format description")) }
  }

  fn tone(format: &CMFormatDescription, start: i64, count: usize) -> CFRetained<CMSampleBuffer> {
    let samples: Vec<i16> = (0..count)
      .map(|index| {
        let at = (start + index as i64) as f64 / f64::from(SAMPLE_RATE);
        ((at * 440.0 * std::f64::consts::TAU).sin() * 8000.0) as i16
      })
      .collect();
    let length = count * 2;
    let mut block = std::ptr::null_mut();
    let status = unsafe {
      CMBlockBuffer::create_with_memory_block(
        None,
        std::ptr::null_mut(),
        length,
        None,
        std::ptr::null(),
        0,
        length,
        kCMBlockBufferAssureMemoryNowFlag,
        NonNull::from(&mut block),
      )
    };
    assert_eq!(status, 0, "the block buffer allocates");
    let block = unsafe { CFRetained::from_raw(NonNull::new(block).expect("a block buffer")) };
    let status = unsafe { CMBlockBuffer::replace_data_bytes(NonNull::from(&samples[0]).cast(), &block, 0, length) };
    assert_eq!(status, 0, "the samples copy in");
    let mut sample = std::ptr::null_mut();
    let status = unsafe {
      CMAudioSampleBufferCreateReadyWithPacketDescriptions(
        None,
        &block,
        format,
        count as isize,
        CMTime::new(start, SAMPLE_RATE),
        std::ptr::null(),
        NonNull::from(&mut sample),
      )
    };
    assert_eq!(status, 0, "the audio sample buffer builds");
    unsafe { CFRetained::from_raw(NonNull::new(sample).expect("an audio sample buffer")) }
  }

  fn record(path: &Path, clip: &Clip) {
    let writer = unsafe {
      AVAssetWriter::assetWriterWithURL_fileType_error(&url(path), AVFileTypeQuickTimeMovie.expect("a mov type"))
    }
    .expect("a fixture writer");

    let width = NSNumber::new_usize(clip.width);
    let height = NSNumber::new_usize(clip.height);
    let codec = if clip.hlg {
      unsafe { AVVideoCodecTypeHEVC }.expect("an hevc codec type")
    } else {
      unsafe { AVVideoCodecTypeH264 }.expect("an h.264 codec type")
    };
    let hdr = settings(&[
      (
        unsafe { AVVideoColorPrimariesKey }.expect("a primaries key"),
        unsafe { AVVideoColorPrimaries_ITU_R_2020 }
          .expect("bt.2020 primaries")
          .as_ref(),
      ),
      (
        unsafe { AVVideoTransferFunctionKey }.expect("a transfer key"),
        unsafe { AVVideoTransferFunction_ITU_R_2100_HLG }
          .expect("an hlg transfer")
          .as_ref(),
      ),
      (
        unsafe { AVVideoYCbCrMatrixKey }.expect("a matrix key"),
        unsafe { AVVideoYCbCrMatrix_ITU_R_2020 }
          .expect("a bt.2020 matrix")
          .as_ref(),
      ),
    ]);
    let mut entries = vec![
      (unsafe { AVVideoCodecKey }.expect("a codec key"), codec.as_ref()),
      (unsafe { AVVideoWidthKey }.expect("a width key"), width.as_ref()),
      (unsafe { AVVideoHeightKey }.expect("a height key"), height.as_ref()),
    ];
    if clip.hlg {
      entries.push((unsafe { AVVideoColorPropertiesKey }.expect("a color key"), hdr.as_ref()));
    }
    let compression = settings(&entries);
    let video_type = unsafe { AVMediaTypeVideo }.expect("a video media type");
    let video =
      unsafe { AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(video_type, Some(&compression)) };
    unsafe {
      video.setExpectsMediaDataInRealTime(true);
      video.setTransform(clip.transform);
    }
    let bgra = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let attributes = settings(&[
      (bridged(unsafe { kCVPixelBufferPixelFormatTypeKey }), bgra.as_ref()),
      (bridged(unsafe { kCVPixelBufferWidthKey }), width.as_ref()),
      (bridged(unsafe { kCVPixelBufferHeightKey }), height.as_ref()),
    ]);
    let adaptor = unsafe {
      AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
        &video,
        Some(&attributes),
      )
    };
    unsafe { writer.addInput(&video) };

    let aac = NSNumber::new_u32(kAudioFormatMPEG4AAC);
    let rate = NSNumber::new_i32(SAMPLE_RATE);
    let mono = NSNumber::new_u32(1);
    let encoding = settings(&[
      (unsafe { AVFormatIDKey }.expect("a format key"), aac.as_ref()),
      (unsafe { AVSampleRateKey }.expect("a rate key"), rate.as_ref()),
      (unsafe { AVNumberOfChannelsKey }.expect("a channels key"), mono.as_ref()),
    ]);
    let audio_type = unsafe { AVMediaTypeAudio }.expect("an audio media type");
    let audio =
      unsafe { AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(audio_type, Some(&encoding)) };
    unsafe { audio.setExpectsMediaDataInRealTime(true) };
    if clip.audio {
      unsafe { writer.addInput(&audio) };
    }

    assert!(unsafe { writer.startWriting() }, "the fixture writer starts");
    unsafe { writer.startSessionAtSourceTime(kCMTimeZero) };
    let pool = unsafe { adaptor.pixelBufferPool() }.expect("a pixel buffer pool");
    let format = tone_format();
    let per_frame = (SAMPLE_RATE / clip.fps) as usize;
    for index in 0..clip.frames {
      let buffer = frame(&pool, clip);
      ready(&video);
      assert!(
        unsafe {
          adaptor.appendPixelBuffer_withPresentationTime(&buffer, CMTime::new(i64::from(index) * clip.stride, clip.fps))
        },
        "the fixture frame appends"
      );
      if clip.audio {
        ready(&audio);
        let start = i64::from(index) * per_frame as i64;
        assert!(
          unsafe { audio.appendSampleBuffer(&tone(&format, start, per_frame)) },
          "the fixture tone appends"
        );
      }
    }
    unsafe { video.markAsFinished() };
    if clip.audio {
      unsafe { audio.markAsFinished() };
    }
    finish(&writer).expect("the fixture finishes");
  }

  fn request(source: &Path, destination: &Path, cover: (u32, u32), max_bitrate: Option<u32>) -> TranscodeRequest {
    TranscodeRequest {
      source: source.display().to_string(),
      destination: destination.display().to_string(),
      cover_width: cover.0,
      cover_height: cover.1,
      max_bitrate,
    }
  }

  fn run_under(
    source: &Path,
    destination: &Path,
    cover: (u32, u32),
    max_bitrate: Option<u32>,
  ) -> Result<TranscodedVideo, String> {
    let (sink, settled) = TranscodeSink::channel();
    AvMediaTranscoder.transcode(request(source, destination, cover, max_bitrate), sink);
    settled.blocking_recv().expect("the sink settles")
  }

  fn run(source: &Path, destination: &Path, cover: (u32, u32)) -> Result<TranscodedVideo, String> {
    run_under(source, destination, cover, None)
  }

  struct Written {
    tracks: Vec<Retained<AVAssetTrack>>,
    frames: usize,
    marked: (bool, bool),
  }

  fn inspect(path: &Path) -> Written {
    let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url(path), None) };
    let tracks: Vec<_> = unsafe { asset.tracks() }.iter().collect();
    let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(&asset) }.expect("an output reader");
    let bgra = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let decoded = settings(&[(bridged(unsafe { kCVPixelBufferPixelFormatTypeKey }), bgra.as_ref())]);
    let output =
      unsafe { AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&tracks[0], Some(&decoded)) };
    unsafe { reader.addOutput(&output) };
    assert!(unsafe { reader.startReading() }, "the output reads back");
    let mut frames = 0;
    let mut marked = (false, false);
    while let Some(sample) = unsafe { output.copyNextSampleBuffer() } {
      if frames == 0 {
        let buffer = unsafe { sample.image_buffer() }.expect("a decoded frame");
        unsafe { CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly) };
        let stride = CVPixelBufferGetBytesPerRow(&buffer);
        let (width, height) = (CVPixelBufferGetWidth(&buffer), CVPixelBufferGetHeight(&buffer));
        let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
        let red = |x: usize, y: usize| unsafe { *base.add(y * stride + x * 4 + 2) } > 128;
        marked = (red(width / 4, height / 4), red(width * 3 / 4, height / 4));
        unsafe { CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly) };
      }
      frames += 1;
    }
    Written { tracks, frames, marked }
  }

  fn described<R>(track: &AVAssetTrack, read: impl FnOnce(&CMFormatDescription) -> R) -> R {
    let descriptions = unsafe { track.formatDescriptions() };
    let first = descriptions.firstObject().expect("a format description");
    read(unsafe { &*Retained::as_ptr(&first).cast() })
  }

  fn colors(track: &AVAssetTrack) -> [String; 3] {
    described(track, |description| {
      unsafe {
        [
          kCMFormatDescriptionExtension_ColorPrimaries,
          kCMFormatDescriptionExtension_TransferFunction,
          kCMFormatDescriptionExtension_YCbCrMatrix,
        ]
      }
      .map(|key| {
        unsafe { description.extension(key) }
          .and_then(|value| value.downcast::<CFString>().ok())
          .map(|value| value.to_string())
          .unwrap_or_default()
      })
    })
  }

  fn profile(track: &AVAssetTrack) -> (u32, u8) {
    described(track, avc_profile)
  }

  fn avc_profile(description: &CMFormatDescription) -> (u32, u8) {
    let atoms = unsafe { description.extension(kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms) }
      .and_then(|atoms| atoms.downcast::<CFDictionary>().ok())
      .map(|atoms| unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFData>>(atoms) })
      .expect("sample description atoms");
    let avcc = atoms
      .get(&CFString::from_static_str("avcC"))
      .expect("an avcC atom")
      .to_vec();
    (unsafe { description.media_sub_type() }, avcc[1])
  }

  fn scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let source = dir.path().join("source.mov");
    let destination = dir.path().join("out.mp4");
    (dir, source, destination)
  }

  #[test]
  fn a_rotated_sixty_fps_clip_with_audio_becomes_one_upright_thirty_fps_h264_track() {
    let (_dir, source, destination) = scratch();
    record(
      &source,
      &Clip {
        width: 1920,
        height: 1080,
        fps: 60,
        stride: 1,
        frames: 60,
        transform: ROTATED,
        audio: true,
        hlg: false,
        grain: 0,
      },
    );
    let fixture = inspect(&source);
    assert_eq!(fixture.tracks.len(), 2, "the fixture carries video and audio");
    assert_eq!(unsafe { fixture.tracks[0].preferredTransform() }, ROTATED);
    assert!(unsafe { fixture.tracks[0].nominalFrameRate() } > 59.0);

    let video = run(&source, &destination, (800, 480)).expect("the transcode completes");

    assert_eq!((video.width, video.height), (800, 1422));
    assert!(
      (900..=1100).contains(&video.duration_ms),
      "duration {}",
      video.duration_ms
    );
    let written = inspect(&destination);
    assert_eq!(written.tracks.len(), 1, "audio is stripped");
    let track = &written.tracks[0];
    assert!(unsafe { track.mediaType() }.isEqualToString(unsafe { AVMediaTypeVideo }.unwrap()));
    let natural = unsafe { track.naturalSize() };
    assert_eq!((natural.width, natural.height), (800.0, 1422.0));
    assert_eq!(unsafe { track.preferredTransform() }, UPRIGHT);
    assert!(unsafe { track.nominalFrameRate() } <= 30.5, "fps {}", unsafe {
      track.nominalFrameRate()
    });
    assert!((25..=31).contains(&written.frames), "frames {}", written.frames);
    assert_eq!(profile(track), (AVC1, HIGH_PROFILE));
    assert_eq!(
      written.marked,
      (false, true),
      "the marked corner rotates to the top right"
    );
  }

  #[test]
  fn a_slow_small_clip_keeps_its_size_and_frame_rate() {
    let (_dir, source, destination) = scratch();
    record(
      &source,
      &Clip {
        width: 640,
        height: 360,
        fps: 24,
        stride: 1,
        frames: 48,
        transform: UPRIGHT,
        audio: false,
        hlg: false,
        grain: 0,
      },
    );

    let video = run(&source, &destination, (800, 480)).expect("the transcode completes");

    assert_eq!((video.width, video.height), (640, 360));
    let written = inspect(&destination);
    let fps = unsafe { written.tracks[0].nominalFrameRate() };
    assert!((23.5..=24.5).contains(&fps), "fps {fps}");
    assert!((46..=49).contains(&written.frames), "frames {}", written.frames);
    assert_eq!(written.marked, (true, false));
  }

  #[test]
  fn an_hlg_tagged_hevc_source_lands_as_bt709_h264() {
    let (_dir, source, destination) = scratch();
    record(
      &source,
      &Clip {
        width: 1280,
        height: 720,
        fps: 30,
        stride: 1,
        frames: 30,
        transform: UPRIGHT,
        audio: false,
        hlg: true,
        grain: 0,
      },
    );
    let fixture = inspect(&source);
    assert_eq!(
      colors(&fixture.tracks[0]),
      ["ITU_R_2020", "ITU_R_2100_HLG", "ITU_R_2020"].map(str::to_owned)
    );

    let video = run(&source, &destination, (800, 480)).expect("the transcode completes");

    assert_eq!((video.width, video.height), (854, 480));
    let written = inspect(&destination);
    assert_eq!(profile(&written.tracks[0]), (AVC1, HIGH_PROFILE));
    assert_eq!(colors(&written.tracks[0]), BT709.map(str::to_owned));
    assert_eq!(written.marked, (true, false));
  }

  #[test]
  fn cancelling_an_in_flight_transcode_fails_it_and_leaves_no_output() {
    let (_dir, source, destination) = scratch();
    record(
      &source,
      &Clip {
        width: 1920,
        height: 1080,
        fps: 30,
        stride: 1,
        frames: 180,
        transform: UPRIGHT,
        audio: false,
        hlg: false,
        grain: 0,
      },
    );

    let (sink, settled) = TranscodeSink::channel();
    let job = AvMediaTranscoder.transcode(request(&source, &destination, (800, 480), None), sink);
    job.cancel();

    let reason = settled
      .blocking_recv()
      .expect("the sink settles")
      .expect_err("a cancelled transcode does not complete");
    assert_eq!(reason, CANCELLED);
    assert!(!destination.exists(), "the cancelled output is removed");
  }

  #[test]
  fn a_bitrate_ceiling_keeps_a_detailed_clip_under_it() {
    const CEILING: u32 = 300_000;
    let (dir, source, _) = scratch();
    record(
      &source,
      &Clip {
        width: 960,
        height: 540,
        fps: 30,
        stride: 1,
        frames: 120,
        transform: UPRIGHT,
        audio: false,
        hlg: false,
        grain: 48,
      },
    );
    assert!(plan_video(960, 540, 800, 480, None).bitrate > CEILING * 3);

    let rate = |max_bitrate: Option<u32>| {
      let destination = dir.path().join(format!("out-{}.mp4", max_bitrate.unwrap_or(0)));
      let video = run_under(&source, &destination, (800, 480), max_bitrate).expect("the transcode completes");
      let bytes = std::fs::metadata(&destination).expect("an output").len();
      bytes as f64 * 8000.0 / video.duration_ms as f64
    };
    let (free, capped) = (rate(None), rate(Some(CEILING)));
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
  fn a_frame_duration_is_always_positive_and_never_shorter_than_the_cap_allows() {
    for nominal in [0.0001, 0.0004, 0.2, 23.976, 60.0, 0.0, -1.0, f32::NAN] {
      let timescale = frame_timescale(frame_rate(nominal, 30));
      assert!(
        (1..=30 * FRAME_TICKS).contains(&timescale),
        "{nominal} fps gave {timescale}"
      );
    }
    assert_eq!(frame_timescale(frame_rate(23.976, 30)), 23_976);
    assert_eq!(frame_timescale(frame_rate(0.0, 30)), 30_000);
  }

  #[test]
  fn an_exception_inside_a_transcode_fails_the_pick_and_removes_its_output() {
    let (_dir, _, destination) = scratch();
    std::fs::write(&destination, b"half an encode").expect("wrote a partial destination");
    let (sink, settled) = TranscodeSink::channel();

    settle(&sink, destination.to_str().expect("a utf-8 path"), || {
      let empty = NSArray::<NSString>::new();
      Err(empty.objectAtIndex(1).to_string())
    });

    let reason = settled
      .blocking_recv()
      .expect("the sink settles")
      .expect_err("the pick fails");
    assert!(reason.contains("beyond bounds"), "{reason}");
    assert!(!destination.exists(), "the partial destination is removed");
  }

  #[test]
  fn a_source_slower_than_a_frame_every_half_hour_still_transcodes() {
    let (_dir, source, destination) = scratch();
    record(
      &source,
      &Clip {
        width: 640,
        height: 360,
        fps: 1,
        stride: 10_000,
        frames: 2,
        transform: UPRIGHT,
        audio: false,
        hlg: false,
        grain: 0,
      },
    );
    let nominal = unsafe { inspect(&source).tracks[0].nominalFrameRate() };
    assert!(
      nominal > 0.0 && nominal < 0.0005,
      "the fixture rate {nominal} rounds to no frames at all"
    );

    let video = run(&source, &destination, (800, 480)).expect("the transcode completes");

    assert_eq!((video.width, video.height), (640, 360));
    assert!(inspect(&destination).frames >= 1);
  }

  fn still_size(bytes: &[u8]) -> (u32, u32) {
    let full = sniff_and_scale(
      &super::super::image::ImageIoScaler,
      bytes.to_vec(),
      ImageTarget::Thumbnail {
        max_edge: 8192,
        quality: 1.0,
      },
    )
    .expect("the poster decodes");
    (full.width, full.height)
  }

  #[test]
  fn a_poster_is_the_upright_first_frame_as_a_jpeg_the_scaler_can_thumbnail() {
    let (_dir, source, _) = scratch();
    record(
      &source,
      &Clip {
        width: 1920,
        height: 1080,
        fps: 30,
        stride: 1,
        frames: 30,
        transform: ROTATED,
        audio: false,
        hlg: false,
        grain: 0,
      },
    );

    let poster = AvMediaTranscoder
      .poster(source.to_string_lossy().into_owned())
      .expect("a poster");

    assert_eq!(&poster[..3], &[0xFF, 0xD8, 0xFF], "the poster is a jpeg");
    assert_eq!(still_size(&poster), (1080, 1920), "the poster is upright");
    let thumbnail = sniff_and_scale(
      &super::super::image::ImageIoScaler,
      poster,
      ImageTarget::Thumbnail {
        max_edge: 320,
        quality: 0.7,
      },
    )
    .expect("the scaler thumbnails it");
    assert_eq!(thumbnail.mime, "image/jpeg");
    assert_eq!(thumbnail.width.max(thumbnail.height), 320);
  }

  #[test]
  fn a_poster_of_something_that_is_not_video_is_none() {
    let (_dir, source, _) = scratch();
    std::fs::write(&source, b"plainly not a movie").expect("wrote the decoy");
    assert_eq!(AvMediaTranscoder.poster(source.to_string_lossy().into_owned()), None);
    assert_eq!(AvMediaTranscoder.poster("/nonexistent/clip.mov".into()), None);
  }

  #[test]
  fn a_source_that_is_not_video_fails_and_leaves_no_output() {
    let (_dir, source, destination) = scratch();
    std::fs::write(&source, b"plainly not a movie").expect("wrote the decoy");
    std::fs::write(&destination, b"stale output").expect("wrote a stale destination");

    let outcome = run(&source, &destination, (800, 480));

    assert!(outcome.is_err(), "outcome {outcome:?}");
    assert!(!destination.exists(), "the partial destination is removed");
  }
}
