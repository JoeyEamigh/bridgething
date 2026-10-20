use std::{
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  thread,
  time::Duration,
};

use alsa::{
  Direction, ValueOr,
  pcm::{Access, Format, HwParams, PCM},
};
use bridgething_dsp::{
  geometry::CHANNELS,
  pipeline::{Beamformer, to_pcm16},
};
use bytes::BytesMut;
use tokio::sync::mpsc;

use super::{CapturedFrame, MicConfig, MicError};

#[derive(Debug)]
pub(super) struct WorkerHandle {
  cancel: Arc<AtomicBool>,
  mark_target: Arc<AtomicBool>,
  hold_noise: Arc<AtomicBool>,
}

impl WorkerHandle {
  pub fn start(config: MicConfig, frames: mpsc::Sender<CapturedFrame>) -> Result<Self, MicError> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mark_target = Arc::new(AtomicBool::new(false));
    let hold_noise = Arc::new(AtomicBool::new(false));
    let pcm = open_pcm(&config).map_err(|e| MicError::Alsa(e.to_string()))?;
    pcm.start().map_err(|e| MicError::Alsa(e.to_string()))?;

    let handle = Self {
      cancel: cancel.clone(),
      mark_target: mark_target.clone(),
      hold_noise: hold_noise.clone(),
    };
    thread::Builder::new()
      .name("mic-capture".into())
      .spawn(move || {
        if let Err(err) = run(pcm, config, frames, cancel, mark_target, hold_noise) {
          tracing::warn!("mic capture worker exited with error: {err}");
        }
      })
      .map_err(|e| MicError::Alsa(format!("spawn worker thread: {e}")))?;

    Ok(handle)
  }

  pub fn mark_target(&self) {
    self.mark_target.store(true, Ordering::Release);
  }

  pub fn hold_noise(&self, holding: bool) {
    self.hold_noise.store(holding, Ordering::Release);
  }

  pub fn stop(self) {
    self.cancel.store(true, Ordering::Release);
  }
}

fn open_pcm(config: &MicConfig) -> alsa::Result<PCM> {
  let pcm = PCM::new(&config.device, Direction::Capture, false)?;
  {
    let hwp = HwParams::any(&pcm)?;
    hwp.set_channels(CHANNELS as u32)?;
    hwp.set_rate(config.format.sample_rate_hz, ValueOr::Nearest)?;
    hwp.set_format(Format::s32())?;
    hwp.set_access(Access::RWInterleaved)?;
    pcm.hw_params(&hwp)?;
  }
  Ok(pcm)
}

fn run(
  pcm: PCM,
  config: MicConfig,
  frames: mpsc::Sender<CapturedFrame>,
  cancel: Arc<AtomicBool>,
  mark_target: Arc<AtomicBool>,
  hold_noise: Arc<AtomicBool>,
) -> alsa::Result<()> {
  let io = pcm.io_i32()?;
  let frame_samples = config.format.frame_samples as usize;
  let frame_byte_len = frame_samples * 2;
  let mut samples = vec![0i32; frame_samples * CHANNELS];
  let mut beamformer = Beamformer::new(config.dsp);
  let mut mono: Vec<f32> = Vec::with_capacity(frame_samples * 2);
  let mut pending = BytesMut::with_capacity(frame_byte_len * 2);

  loop {
    if cancel.load(Ordering::Acquire) {
      let _ = pcm.drop();
      return Ok(());
    }
    if mark_target.swap(false, Ordering::AcqRel) {
      beamformer.mark_target();
      tracing::debug!(bearing = ?beamformer.bearing_deg(), "beamformer marked a target");
    }
    beamformer.hold_noise(hold_noise.load(Ordering::Acquire));
    match io.readi(&mut samples) {
      Ok(0) => continue,
      Ok(read) => {
        mono.clear();
        beamformer.process(&samples[..read * CHANNELS], &mut mono);
        to_pcm16(&mono, &mut pending);
        let on_target = beamformer.target_evidence();
        while pending.len() >= frame_byte_len {
          let frame = CapturedFrame {
            pcm: pending.split_to(frame_byte_len).freeze(),
            on_target,
          };
          if frames.blocking_send(frame).is_err() {
            let _ = pcm.drop();
            return Ok(());
          }
        }
      }
      Err(err) => {
        tracing::trace!("mic capture read error: {err}; recovering");
        if pcm.recover(err.errno(), true).is_err() {
          thread::sleep(Duration::from_millis(20));
        }
      }
    }
  }
}
