use num_complex::{Complex32, Complex64};
use realfft::RealFftPlanner;

use crate::{
  beamformer::{Design, Field, weights},
  calibration::Calibration,
  geometry::{CHANNELS, POSITION_TO_CHANNEL},
  scene::{self, SceneEstimator, TargetEvidence},
  stft::{Analyzer, BINS, FFT_SIZE, HOP, Synthesizer, bin_frequencies},
};

const SAMPLE_SCALE: f32 = 1.0 / 2_147_483_648.0;

#[derive(Debug, Clone, Copy)]
pub struct Config {
  pub sample_rate_hz: f64,
  pub steering_deg: f64,
  pub design: Design,
  pub calibration: Calibration,
  pub adaptation: Option<scene::Config>,
  pub redesign_hz: f64,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      sample_rate_hz: 16_000.0,
      steering_deg: 35.0,
      design: Design::Superdirective { wng_floor_db: -6.0 },
      calibration: Calibration::IDENTITY,
      adaptation: None,
      redesign_hz: 2.0,
    }
  }
}

pub struct Beamformer {
  config: Config,
  analyzer: Analyzer,
  synthesizer: Synthesizer,
  weights_conj: Box<[[Complex32; CHANNELS]; BINS]>,
  gains: [f32; CHANNELS],
  frames: [[f32; FFT_SIZE]; CHANNELS],
  spectra: [Box<[Complex32; BINS]>; CHANNELS],
  combined: Box<[Complex32; BINS]>,
  filled: usize,
  scene: Option<SceneEstimator>,
  redesign_interval: usize,
  since_redesign: usize,
}

impl Beamformer {
  pub fn new(config: Config) -> Self {
    let mut planner = RealFftPlanner::<f32>::new();
    let mut beamformer = Self {
      config,
      analyzer: Analyzer::new(&mut planner),
      synthesizer: Synthesizer::new(&mut planner),
      weights_conj: Box::new([[Complex32::new(0.0, 0.0); CHANNELS]; BINS]),
      gains: config.calibration.gains(),
      frames: [[0.0; FFT_SIZE]; CHANNELS],
      spectra: std::array::from_fn(|_| Box::new([Complex32::new(0.0, 0.0); BINS])),
      combined: Box::new([Complex32::new(0.0, 0.0); BINS]),
      filled: 0,
      scene: config
        .adaptation
        .map(|scene| SceneEstimator::new(scene, config.sample_rate_hz, HOP)),
      redesign_interval: ((config.sample_rate_hz / HOP as f64) / config.redesign_hz.max(0.01))
        .ceil()
        .max(1.0) as usize,
      since_redesign: 0,
    };
    beamformer.redesign();
    beamformer
  }

  pub fn config(&self) -> Config {
    self.config
  }

  pub fn set_steering(&mut self, steering_deg: f64) {
    if (self.config.steering_deg - steering_deg).abs() < f64::EPSILON {
      return;
    }
    self.config.steering_deg = steering_deg;
    self.redesign();
  }

  pub fn set_calibration(&mut self, calibration: Calibration) {
    self.config.calibration = calibration;
    self.gains = calibration.gains();
    self.redesign();
  }

  pub fn mark_target(&mut self) {
    let Some(scene) = self.scene.as_mut() else { return };
    scene.mark_target();
    self.since_redesign = 0;
    self.redesign();
  }

  pub fn point_like(&self) -> bool {
    self.scene.as_ref().is_some_and(SceneEstimator::point_like)
  }

  pub fn bearing_deg(&self) -> Option<f64> {
    self.scene.as_ref().and_then(SceneEstimator::bearing_deg)
  }

  pub fn target_evidence(&self) -> Option<TargetEvidence> {
    self.scene.as_ref().and_then(SceneEstimator::target_evidence)
  }

  pub fn hold_noise(&mut self, holding: bool) {
    if let Some(scene) = self.scene.as_mut() {
      scene.hold_noise(holding);
    }
  }

  pub fn measured_field(&self, bin: usize) -> Option<Field> {
    let scene = self.scene.as_ref()?;
    let freq = bin_frequencies(self.config.sample_rate_hz)[bin];
    scene
      .measures_look_direction(bin, freq)
      .then(|| scene.field(bin, freq, self.config.steering_deg))
  }

  pub fn adoption(&self) -> (usize, bool) {
    let Some(scene) = self.scene.as_ref() else {
      return (0, false);
    };
    let measured = bin_frequencies(self.config.sample_rate_hz)
      .into_iter()
      .enumerate()
      .filter(|(_, freq)| (750.0..4000.0).contains(freq))
      .filter(|(bin, freq)| scene.measures_look_direction(*bin, *freq))
      .count();
    (measured, scene.measures_noise())
  }

  fn redesign(&mut self) {
    let Config {
      sample_rate_hz,
      steering_deg,
      design,
      calibration,
      ..
    } = self.config;
    let design = match self.scene.as_ref() {
      Some(scene) if scene.unsteered() => Design::DelayAndSum,
      _ => design,
    };
    for (bin, freq) in bin_frequencies(sample_rate_hz).into_iter().enumerate() {
      let field = match self.scene.as_ref() {
        Some(scene) => scene.field(bin, freq, steering_deg),
        None => Field::assumed(freq, steering_deg),
      };
      let designed = weights(design, &field);
      let correction = calibration.bin_correction(freq, sample_rate_hz);
      for position in 0..CHANNELS {
        let corrected: Complex64 = designed[position] * correction[POSITION_TO_CHANNEL[position]];
        self.weights_conj[bin][position] = Complex32::new(corrected.re as f32, -corrected.im as f32);
      }
    }
  }

  pub fn process(&mut self, interleaved: &[i32], out: &mut Vec<f32>) {
    for chunk in interleaved.as_chunks::<CHANNELS>().0 {
      for (position, &wire) in POSITION_TO_CHANNEL.iter().enumerate() {
        self.frames[position][self.filled] = chunk[wire] as f32 * SAMPLE_SCALE * self.gains[wire];
      }
      self.filled += 1;
      if self.filled == FFT_SIZE {
        self.emit_frame(out);
      }
    }
  }

  fn emit_frame(&mut self, out: &mut Vec<f32>) {
    for position in 0..CHANNELS {
      self
        .analyzer
        .analyze(&self.frames[position], self.spectra[position].as_mut_slice());
    }
    if let Some(scene) = self.scene.as_mut() {
      scene.observe(&self.spectra);
      self.since_redesign += 1;
      if self.since_redesign >= self.redesign_interval {
        self.since_redesign = 0;
        self.redesign();
      }
    }
    for bin in 0..BINS {
      let mut sum = Complex32::new(0.0, 0.0);
      for position in 0..CHANNELS {
        sum += self.weights_conj[bin][position] * self.spectra[position][bin];
      }
      self.combined[bin] = sum;
    }

    let start = out.len();
    out.resize(start + HOP, 0.0);
    self
      .synthesizer
      .synthesize(self.combined.as_mut_slice(), &mut out[start..]);

    for frame in &mut self.frames {
      frame.copy_within(HOP.., 0);
    }
    self.filled -= HOP;
  }

  pub fn reset_frames(&mut self) {
    self.filled = 0;
    self.frames = [[0.0; FFT_SIZE]; CHANNELS];
  }

  pub fn reset(&mut self) {
    self.filled = 0;
    self.frames = [[0.0; FFT_SIZE]; CHANNELS];
    self.since_redesign = 0;
    if let Some(scene) = self.scene.as_mut() {
      scene.reset();
    }
    self.redesign();
  }
}

pub fn to_pcm16<E: Extend<u8>>(samples: &[f32], out: &mut E) {
  for sample in samples {
    let scaled = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
    out.extend(scaled.to_le_bytes());
  }
}

pub fn from_pcm16(bytes: &[u8], out: &mut Vec<f32>) {
  out.extend(
    bytes
      .as_chunks::<2>()
      .0
      .iter()
      .map(|pair| i16::from_le_bytes(*pair) as f32 / 32768.0),
  );
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::geometry::{ELEMENT_SPACING_M, SPEED_OF_SOUND_M_S};

  const RATE: f64 = 16_000.0;

  fn plane_wave(freq_hz: f64, angle_deg: f64, samples: usize, amplitude: f64) -> Vec<i32> {
    let sin_theta = angle_deg.to_radians().sin();
    let centre = (CHANNELS - 1) as f64 / 2.0;
    let mut out = vec![0i32; samples * CHANNELS];
    for n in 0..samples {
      for position in 0..CHANNELS {
        let offset = (position as f64 - centre) * ELEMENT_SPACING_M;
        let delay = -offset * sin_theta / SPEED_OF_SOUND_M_S;
        let phase = std::f64::consts::TAU * freq_hz * (n as f64 / RATE - delay);
        let value = amplitude * phase.sin() * i32::MAX as f64;
        out[n * CHANNELS + POSITION_TO_CHANNEL[position]] = value as i32;
      }
    }
    out
  }

  fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
      return 0.0;
    }
    (samples.iter().map(|s| (s * s) as f64).sum::<f64>() / samples.len() as f64).sqrt()
  }

  fn response_db(steering_deg: f64, source_deg: f64, freq_hz: f64) -> f64 {
    let mut beamformer = Beamformer::new(Config {
      steering_deg,
      ..Config::default()
    });
    let mut out = Vec::new();
    beamformer.process(&plane_wave(freq_hz, source_deg, 4096, 0.25), &mut out);
    20.0 * rms(&out[HOP * 2..]).log10()
  }

  #[test]
  fn a_source_on_the_look_direction_passes_through_undistorted() {
    for (steering, freq) in [(0.0, 1000.0), (35.0, 1000.0), (-35.0, 2000.0)] {
      let level = response_db(steering, steering, freq);
      let expected = 20.0 * (0.25 / 2f64.sqrt()).log10();
      assert!(
        (level - expected).abs() < 0.5,
        "steer {steering}: {level:.2} vs {expected:.2} dB"
      );
    }
  }

  #[test]
  fn steering_sign_is_not_mirrored() {
    let aimed = response_db(35.0, 35.0, 2000.0);
    let mirrored = response_db(35.0, -35.0, 2000.0);
    assert!(
      aimed > mirrored + 3.0,
      "aimed {aimed:.2} dB vs mirrored {mirrored:.2} dB"
    );
  }

  #[test]
  fn an_off_axis_source_is_attenuated_relative_to_the_look_direction() {
    let on_axis = response_db(35.0, 35.0, 3000.0);
    for interferer in [-60.0, -30.0, 90.0] {
      let off = response_db(35.0, interferer, 3000.0);
      assert!(
        off < on_axis - 1.0,
        "source at {interferer} deg was not attenuated: {off:.2} dB"
      );
    }
  }

  #[test]
  fn output_length_tracks_the_hop_schedule() {
    let mut beamformer = Beamformer::new(Config::default());
    let mut out = Vec::new();
    beamformer.process(&plane_wave(1000.0, 0.0, HOP - 1, 0.2), &mut out);
    assert!(out.is_empty(), "nothing should be emitted before the window fills");

    beamformer.process(&plane_wave(1000.0, 0.0, FFT_SIZE, 0.2), &mut out);
    assert_eq!(out.len() % HOP, 0);
    assert!(!out.is_empty());
  }

  #[test]
  fn splitting_the_input_does_not_change_the_output() {
    let input = plane_wave(1200.0, 20.0, 2048, 0.2);
    let mut whole = Beamformer::new(Config::default());
    let mut whole_out = Vec::new();
    whole.process(&input, &mut whole_out);

    let mut split = Beamformer::new(Config::default());
    let mut split_out = Vec::new();
    for chunk in input.chunks(37 * CHANNELS) {
      split.process(chunk, &mut split_out);
    }

    assert_eq!(whole_out.len(), split_out.len());
    for (a, b) in whole_out.iter().zip(&split_out) {
      assert!((a - b).abs() < 1e-6);
    }
  }

  #[test]
  fn calibration_undoes_a_channel_that_reads_hot() {
    let clean = plane_wave(1000.0, 0.0, 2048, 0.2);
    let mut hot = clean.clone();
    for frame in hot.as_chunks_mut::<CHANNELS>().0 {
      frame[3] = (frame[3] as f64 * 1.5) as i32;
    }
    let measured = crate::calibration::Measurement {
      gain_db: [0.0, 0.0, 0.0, 20.0 * 1.5f64.log10()],
      residual_complex_db_rms: 0.0,
    };

    let level = |input: &[i32], calibration| {
      let mut beamformer = Beamformer::new(Config {
        calibration,
        ..Config::default()
      });
      let mut out = Vec::new();
      beamformer.process(input, &mut out);
      rms(&out[HOP * 2..])
    };

    let reference = level(&clean, Calibration::IDENTITY);
    let uncorrected = level(&hot, Calibration::IDENTITY);
    let corrected = level(&hot, measured.into());

    assert!(
      (uncorrected - reference).abs() > reference * 0.1,
      "the mismatch was not disruptive enough to test"
    );
    assert!(
      (corrected - reference).abs() < reference * 1e-4,
      "calibration did not restore the clean level: {corrected:.5} vs {reference:.5}"
    );
  }

  #[test]
  fn re_steering_changes_where_the_beam_points() {
    let mut beamformer = Beamformer::new(Config::default());
    let before = beamformer.config().steering_deg;
    beamformer.set_steering(-40.0);
    assert!((beamformer.config().steering_deg + 40.0).abs() < 1e-12);
    assert!((before - beamformer.config().steering_deg).abs() > 1.0);
  }

  #[test]
  fn pcm16_conversion_clamps_instead_of_wrapping() {
    let mut out = Vec::new();
    to_pcm16(&[0.0, 1.0, -1.0, 4.0, -4.0], &mut out);
    let decoded: Vec<i16> = out.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
    assert_eq!(decoded, vec![0, 32767, -32767, 32767, -32768]);
  }
}
