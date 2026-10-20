use num_complex::{Complex32, Complex64};

use crate::{
  beamformer::{Covariance, Field},
  geometry::{CHANNELS, ELEMENT_SPACING_M, SPEED_OF_SOUND_M_S, diffuse_covariance, steering_vector},
  stft::{BINS, bin_frequencies},
};

const WIDEST: (usize, usize) = (0, CHANNELS - 1);
const GATE_BAND_HZ: (f64, f64) = (1800.0, 2600.0);
const BROADBAND_FLOOR_HZ: f64 = 100.0;
const POWER_ITERATIONS: usize = 12;
const MIN_POOLED_BINS: usize = 4;
const RIDGE: f64 = 1e-3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetEvidence {
  pub agreement: f32,
  pub band_share: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
  pub noise_tau_s: f64,
  pub recent_tau_s: f64,
  pub gate_tau_s: f64,
  pub gate_threshold: f64,
  pub min_noise_frames: usize,
  pub min_eigenvalue_ratio: f64,
  pub max_freeze_s: f64,
  pub degrade_when_unsteered: bool,
  pub reference: usize,
  pub target_memory: f64,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      noise_tau_s: 3.0,
      recent_tau_s: 1.3,
      gate_tau_s: 0.15,
      gate_threshold: 0.4,
      min_noise_frames: 60,
      min_eigenvalue_ratio: 4.0,
      max_freeze_s: 2.5,
      degrade_when_unsteered: true,
      reference: 0,
      target_memory: 0.0,
    }
  }
}

fn decay(tau_s: f64, hop_s: f64) -> f64 {
  (-hop_s / tau_s).exp()
}

#[derive(Clone, Copy)]
struct Coherence {
  cross: Complex64,
  auto: [f64; 2],
}

impl Coherence {
  const ZERO: Self = Self {
    cross: Complex64::new(0.0, 0.0),
    auto: [0.0; 2],
  };

  fn observe(&mut self, a: Complex64, b: Complex64, alpha: f64) {
    let beta = 1.0 - alpha;
    self.cross = self.cross * alpha + a * b.conj() * beta;
    self.auto[0] = self.auto[0] * alpha + a.norm_sqr() * beta;
    self.auto[1] = self.auto[1] * alpha + b.norm_sqr() * beta;
  }

  fn magnitude_squared(&self) -> f64 {
    let denominator = self.auto[0] * self.auto[1];
    if denominator < 1e-30 {
      0.0
    } else {
      (self.cross.norm_sqr() / denominator).min(1.0)
    }
  }
}

pub struct SceneEstimator {
  config: Config,
  sample_rate_hz: f64,
  noise: Box<[Covariance; BINS]>,
  recent: Box<[Covariance; BINS]>,
  target: Box<[Covariance; BINS]>,
  gate: Box<[Coherence; BINS]>,
  gate_bins: (usize, usize),
  broadband: f64,
  broadband_from: usize,
  noise_alpha: f64,
  recent_alpha: f64,
  gate_alpha: f64,
  noise_frames: usize,
  pooled_bearing: Option<f64>,
  gate_reference: Vec<Complex64>,
  freeze_limit: usize,
  frozen_for: usize,
  holding: bool,
  has_target: bool,
  point_like: bool,
}

impl SceneEstimator {
  pub fn new(config: Config, sample_rate_hz: f64, hop_samples: usize) -> Self {
    let hop_s = hop_samples as f64 / sample_rate_hz;
    let frequencies = bin_frequencies(sample_rate_hz);
    let lower = frequencies.iter().position(|f| *f >= GATE_BAND_HZ.0).unwrap_or(0);
    let upper = frequencies
      .iter()
      .rposition(|f| *f <= GATE_BAND_HZ.1)
      .unwrap_or(BINS - 1)
      .max(lower);

    Self {
      config,
      sample_rate_hz,
      noise: Box::new(std::array::from_fn(|bin| diffuse_covariance(frequencies[bin]))),
      recent: Box::new([[[Complex64::new(0.0, 0.0); CHANNELS]; CHANNELS]; BINS]),
      target: Box::new([[[Complex64::new(0.0, 0.0); CHANNELS]; CHANNELS]; BINS]),
      gate: Box::new([Coherence::ZERO; BINS]),
      gate_bins: (lower, upper),
      broadband: 0.0,
      broadband_from: frequencies.iter().position(|f| *f >= BROADBAND_FLOOR_HZ).unwrap_or(0),
      noise_alpha: decay(config.noise_tau_s, hop_s),
      recent_alpha: decay(config.recent_tau_s, hop_s),
      gate_alpha: decay(config.gate_tau_s, hop_s),
      noise_frames: 0,
      pooled_bearing: None,
      gate_reference: Vec::new(),
      freeze_limit: (config.max_freeze_s / hop_s).ceil() as usize,
      frozen_for: 0,
      holding: false,
      has_target: false,
      point_like: false,
    }
  }

  pub fn hold_noise(&mut self, holding: bool) {
    self.holding = holding;
  }

  pub fn point_like(&self) -> bool {
    self.point_like
  }

  pub fn has_target(&self) -> bool {
    self.has_target
  }

  pub fn observe(&mut self, spectra: &[Box<[Complex32; BINS]>; CHANNELS]) {
    let mut gate_total = 0.0;
    let (lower, upper) = self.gate_bins;
    let a_bins = &spectra[WIDEST.0][lower..=upper];
    let b_bins = &spectra[WIDEST.1][lower..=upper];
    for ((gate, a), b) in self.gate[lower..=upper]
      .iter_mut()
      .zip(a_bins.iter())
      .zip(b_bins.iter())
    {
      let (a, b) = (
        Complex64::new(a.re as f64, a.im as f64),
        Complex64::new(b.re as f64, b.im as f64),
      );
      gate.observe(a, b, self.gate_alpha);
      gate_total += gate.magnitude_squared();
    }
    let frame_broadband: f64 = spectra[WIDEST.0][self.broadband_from..]
      .iter()
      .zip(spectra[WIDEST.1][self.broadband_from..].iter())
      .map(|(a, b)| 0.5 * (a.norm_sqr() as f64 + b.norm_sqr() as f64))
      .sum();
    self.broadband = self.broadband * self.gate_alpha + frame_broadband * (1.0 - self.gate_alpha);
    let bins = (self.gate_bins.1 - self.gate_bins.0 + 1) as f64;
    self.point_like = gate_total / bins >= self.config.gate_threshold;
    self.frozen_for = if self.point_like { self.frozen_for + 1 } else { 0 };
    let frozen = self.point_like && (self.holding || self.frozen_for <= self.freeze_limit);

    for (bin, (recent, noise)) in self.recent.iter_mut().zip(self.noise.iter_mut()).enumerate() {
      let snapshot: [Complex64; CHANNELS] =
        std::array::from_fn(|p| Complex64::new(spectra[p][bin].re as f64, spectra[p][bin].im as f64));
      accumulate(recent, &snapshot, self.recent_alpha);
      if !frozen {
        accumulate(noise, &snapshot, self.noise_alpha);
      }
    }
    if !frozen {
      self.noise_frames = self.noise_frames.saturating_add(1);
    }
  }

  pub fn mark_target(&mut self) {
    let keep = if self.has_target {
      self.config.target_memory.clamp(0.0, 1.0)
    } else {
      0.0
    };
    for (target, recent) in self.target.iter_mut().zip(self.recent.iter()) {
      let fresh = normalized(recent);
      for i in 0..CHANNELS {
        for j in 0..CHANNELS {
          target[i][j] = target[i][j] * keep + fresh[i][j] * (1.0 - keep);
        }
      }
    }
    self.has_target = true;
    self.pooled_bearing = self.pool_bearing();
    self.gate_reference = self.gate_reference_for(self.pooled_bearing);
  }

  fn gate_reference_for(&self, bearing: Option<f64>) -> Vec<Complex64> {
    let Some(bearing) = bearing else { return Vec::new() };
    let frequencies = bin_frequencies(self.sample_rate_hz);
    let (lower, upper) = self.gate_bins;
    (lower..=upper)
      .map(|bin| {
        let steering = steering_vector(frequencies[bin], bearing);
        steering[WIDEST.0] * steering[WIDEST.1].conj()
      })
      .collect()
  }

  pub fn target_evidence(&self) -> Option<TargetEvidence> {
    if self.gate_reference.is_empty() {
      return None;
    }
    let (lower, _) = self.gate_bins;
    let mut aligned = 0.0;
    let mut power = 0.0;
    let mut band = 0.0;
    for (offset, expected) in self.gate_reference.iter().enumerate() {
      let gate = &self.gate[lower + offset];
      aligned += (gate.cross * expected.conj()).re;
      power += (gate.auto[0] * gate.auto[1]).sqrt();
      band += 0.5 * (gate.auto[0] + gate.auto[1]);
    }
    (power > 1e-30).then(|| TargetEvidence {
      agreement: (aligned / power).clamp(0.0, 1.0) as f32,
      band_share: (band / self.broadband.max(band)) as f32,
    })
  }

  fn pool_bearing(&self) -> Option<f64> {
    let frequencies = bin_frequencies(self.sample_rate_hz);
    let mut bearings: Vec<f64> = (0..BINS)
      .filter(|bin| (750.0..4000.0).contains(&frequencies[*bin]))
      .filter_map(|bin| {
        let assumed = Field::assumed(frequencies[bin], 0.0);
        let noise = self.resolved_noise(bin, &assumed);
        let steering = self.measured_steering(bin, &noise)?;
        implied_bearing_deg(&steering, frequencies[bin])
      })
      .collect();
    if bearings.len() < MIN_POOLED_BINS {
      return None;
    }
    bearings.sort_by(f64::total_cmp);
    Some(bearings[bearings.len() / 2])
  }

  pub fn bearing_deg(&self) -> Option<f64> {
    self.pooled_bearing
  }

  pub fn unsteered(&self) -> bool {
    self.config.degrade_when_unsteered && self.pooled_bearing.is_none()
  }

  pub fn reset(&mut self) {
    let frequencies = bin_frequencies(self.sample_rate_hz);
    for (bin, &frequency) in frequencies.iter().enumerate().take(BINS) {
      self.noise[bin] = diffuse_covariance(frequency);
      self.recent[bin] = [[Complex64::new(0.0, 0.0); CHANNELS]; CHANNELS];
      self.target[bin] = [[Complex64::new(0.0, 0.0); CHANNELS]; CHANNELS];
      self.gate[bin] = Coherence::ZERO;
    }
    self.broadband = 0.0;
    self.noise_frames = 0;
    self.pooled_bearing = None;
    self.gate_reference.clear();
    self.frozen_for = 0;
    self.holding = false;
    self.has_target = false;
    self.point_like = false;
  }

  pub fn field(&self, bin: usize, freq_hz: f64, fallback_angle_deg: f64) -> Field {
    let assumed = Field::assumed(freq_hz, fallback_angle_deg);
    let noise = self.resolved_noise(bin, &assumed);
    let steering = self.measured_steering(bin, &noise).unwrap_or_else(|| {
      self
        .pooled_bearing
        .map_or(assumed.steering, |deg| steering_vector(freq_hz, deg))
    });
    Field {
      freq_hz,
      steering,
      noise,
    }
  }

  pub fn measures_look_direction(&self, bin: usize, freq_hz: f64) -> bool {
    let assumed = Field::assumed(freq_hz, 0.0);
    let noise = self.resolved_noise(bin, &assumed);
    self.measured_steering(bin, &noise).is_some()
  }

  pub fn measures_noise(&self) -> bool {
    self.noise_frames >= self.config.min_noise_frames
  }

  fn resolved_noise(&self, bin: usize, assumed: &Field) -> Covariance {
    if self.measures_noise() {
      normalized(&self.noise[bin])
    } else {
      assumed.noise
    }
  }

  fn measured_steering(&self, bin: usize, noise: &Covariance) -> Option<[Complex64; CHANNELS]> {
    if !self.has_target {
      return None;
    }
    relative_transfer_function(
      &self.target[bin],
      noise,
      self.config.reference,
      self.config.min_eigenvalue_ratio,
    )
  }
}

pub fn implied_bearing_deg(steering: &[Complex64; CHANNELS], freq_hz: f64) -> Option<f64> {
  let mut phases = [0.0f64; CHANNELS];
  let mut previous = 0.0;
  for position in 0..CHANNELS {
    let mut phase = steering[position].arg();
    while phase - previous > std::f64::consts::PI {
      phase -= std::f64::consts::TAU;
    }
    while previous - phase > std::f64::consts::PI {
      phase += std::f64::consts::TAU;
    }
    phases[position] = phase;
    previous = phase;
  }
  let centre = (CHANNELS - 1) as f64 / 2.0;
  let numerator: f64 = (0..CHANNELS).map(|p| (p as f64 - centre) * phases[p]).sum();
  let denominator: f64 = (0..CHANNELS).map(|p| (p as f64 - centre).powi(2)).sum();
  let slope = numerator / denominator;
  let sine = slope * SPEED_OF_SOUND_M_S / (std::f64::consts::TAU * freq_hz * ELEMENT_SPACING_M);
  (-1.0..=1.0).contains(&sine).then(|| sine.asin().to_degrees())
}

fn accumulate(covariance: &mut Covariance, snapshot: &[Complex64; CHANNELS], alpha: f64) {
  let beta = 1.0 - alpha;
  for i in 0..CHANNELS {
    for j in 0..CHANNELS {
      covariance[i][j] = covariance[i][j] * alpha + snapshot[i] * snapshot[j].conj() * beta;
    }
  }
}

fn normalized(covariance: &Covariance) -> Covariance {
  let trace: f64 = (0..CHANNELS).map(|i| covariance[i][i].re).sum();
  if trace < 1e-30 {
    return *covariance;
  }
  let scale = CHANNELS as f64 / trace;
  std::array::from_fn(|i| std::array::from_fn(|j| covariance[i][j] * scale))
}

fn cholesky(matrix: &Covariance) -> Option<Covariance> {
  let mut lower = [[Complex64::new(0.0, 0.0); CHANNELS]; CHANNELS];
  for i in 0..CHANNELS {
    for j in 0..=i {
      let dot: Complex64 = (0..j).map(|k| lower[i][k] * lower[j][k].conj()).sum();
      if i == j {
        let pivot = matrix[i][i].re - dot.re;
        if pivot <= 1e-18 {
          return None;
        }
        lower[i][j] = Complex64::new(pivot.sqrt(), 0.0);
      } else {
        lower[i][j] = (matrix[i][j] - dot) / lower[j][j];
      }
    }
  }
  Some(lower)
}

fn forward_substitute(lower: &Covariance, rhs: &[Complex64; CHANNELS]) -> [Complex64; CHANNELS] {
  let mut out = [Complex64::new(0.0, 0.0); CHANNELS];
  for i in 0..CHANNELS {
    let dot: Complex64 = (0..i).map(|k| lower[i][k] * out[k]).sum();
    out[i] = (rhs[i] - dot) / lower[i][i];
  }
  out
}

fn back_substitute(lower: &Covariance, rhs: &[Complex64; CHANNELS]) -> [Complex64; CHANNELS] {
  let mut out = [Complex64::new(0.0, 0.0); CHANNELS];
  for i in (0..CHANNELS).rev() {
    let dot: Complex64 = (i + 1..CHANNELS).map(|k| lower[k][i].conj() * out[k]).sum();
    out[i] = (rhs[i] - dot) / lower[i][i];
  }
  out
}

fn column(matrix: &Covariance, index: usize) -> [Complex64; CHANNELS] {
  std::array::from_fn(|row| matrix[row][index])
}

fn multiply(matrix: &Covariance, vector: &[Complex64; CHANNELS]) -> [Complex64; CHANNELS] {
  std::array::from_fn(|i| (0..CHANNELS).map(|j| matrix[i][j] * vector[j]).sum())
}

fn norm(vector: &[Complex64; CHANNELS]) -> f64 {
  vector.iter().map(Complex64::norm_sqr).sum::<f64>().sqrt()
}

fn dominant(matrix: &Covariance, seed: usize) -> Option<([Complex64; CHANNELS], f64)> {
  let mut vector = [Complex64::new(0.0, 0.0); CHANNELS];
  vector[seed] = Complex64::new(1.0, 0.0);
  for _ in 0..POWER_ITERATIONS {
    let next = multiply(matrix, &vector);
    let magnitude = norm(&next);
    if magnitude < 1e-30 {
      return None;
    }
    vector = next.map(|v| v / magnitude);
  }
  let applied = multiply(matrix, &vector);
  let value: f64 = (0..CHANNELS).map(|i| (vector[i].conj() * applied[i]).re).sum();
  Some((vector, value))
}

fn relative_transfer_function(
  target: &Covariance,
  noise: &Covariance,
  reference: usize,
  min_ratio: f64,
) -> Option<[Complex64; CHANNELS]> {
  let trace: f64 = (0..CHANNELS).map(|i| noise[i][i].re).sum();
  let ridge = (trace / CHANNELS as f64) * RIDGE;
  let mut loaded = *noise;
  for (i, row) in loaded.iter_mut().enumerate() {
    row[i] += ridge;
  }
  let lower = cholesky(&loaded)?;
  let half: Covariance = std::array::from_fn(|_| [Complex64::new(0.0, 0.0); CHANNELS]);
  let mut whitened = half;
  for (index, solved) in (0..CHANNELS).map(|index| (index, forward_substitute(&lower, &column(target, index)))) {
    for (row, value) in solved.iter().enumerate() {
      whitened[row][index] = *value;
    }
  }
  let mut hermitian = half;
  for index in 0..CHANNELS {
    let conjugated: [Complex64; CHANNELS] = std::array::from_fn(|row| whitened[index][row].conj());
    let solved = forward_substitute(&lower, &conjugated);
    for row in 0..CHANNELS {
      hermitian[index][row] = solved[row].conj();
    }
  }

  let (vector, leading) = dominant(&hermitian, reference.min(CHANNELS - 1))?;
  let mut deflated = hermitian;
  for i in 0..CHANNELS {
    for j in 0..CHANNELS {
      deflated[i][j] -= vector[i] * vector[j].conj() * leading;
    }
  }
  let second = dominant(&deflated, 0).map(|(_, value)| value).unwrap_or(0.0);
  if leading <= 0.0 || second.max(0.0) * min_ratio > leading {
    return None;
  }

  let generalized = back_substitute(&lower, &vector);
  let steering = multiply(&loaded, &generalized);
  let pivot = steering[reference.min(CHANNELS - 1)];
  if pivot.norm() < 1e-12 {
    return None;
  }
  let normalized = steering.map(|s| s / pivot);
  if normalized.iter().any(|s| !s.re.is_finite() || !s.im.is_finite()) {
    return None;
  }
  Some(normalized)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    geometry::{SPEED_OF_SOUND_M_S, position_offsets_m, steering_vector},
    stft::HOP,
  };

  const RATE: f64 = 16_000.0;

  fn estimator() -> SceneEstimator {
    SceneEstimator::new(Config::default(), RATE, HOP)
  }

  fn frame(sources: &[(f64, f64, f64)], uncorrelated: f64, seed: &mut u64) -> [Box<[Complex32; BINS]>; CHANNELS] {
    let frequencies = bin_frequencies(RATE);
    let mut out: [Box<[Complex32; BINS]>; CHANNELS] =
      std::array::from_fn(|_| Box::new([Complex32::new(0.0, 0.0); BINS]));
    let mut random = || {
      *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
      2.0 * ((*seed >> 33) as f64 / (1u64 << 31) as f64) - 1.0
    };
    for bin in 0..BINS {
      let freq = frequencies[bin];
      for (angle, power, _) in sources {
        let (re, im) = (random(), random());
        let amplitude = Complex64::new(re, im) * power.sqrt();
        let steering = steering_vector(freq, *angle);
        for (position, channel) in out.iter_mut().enumerate() {
          let value = amplitude * steering[position];
          channel[bin] += Complex32::new(value.re as f32, value.im as f32);
        }
      }
      for (position, channel) in out.iter_mut().enumerate() {
        let (re, im) = (random(), random());
        let value = Complex64::new(re, im) * uncorrelated.sqrt();
        let _ = position;
        channel[bin] += Complex32::new(value.re as f32, value.im as f32);
      }
    }
    out
  }

  fn diffuse_angles(count: usize) -> Vec<(f64, f64, f64)> {
    (0..count)
      .map(|k| {
        let sine = -1.0 + 2.0 * k as f64 / (count - 1) as f64;
        (sine.clamp(-1.0, 1.0).asin().to_degrees(), 1.0, 0.0)
      })
      .collect()
  }

  fn bin_near(freq_hz: f64) -> usize {
    let frequencies = bin_frequencies(RATE);
    (0..BINS)
      .min_by(|a, b| {
        (frequencies[*a] - freq_hz)
          .abs()
          .total_cmp(&(frequencies[*b] - freq_hz).abs())
      })
      .unwrap()
  }

  #[test]
  fn a_diffuse_field_does_not_trip_the_point_source_gate() {
    let mut estimator = estimator();
    let mut seed = 1;
    let spread = diffuse_angles(64);
    for _ in 0..200 {
      estimator.observe(&frame(&spread, 0.2, &mut seed));
    }
    assert!(!estimator.point_like(), "a diffuse field read as point-like");
    assert!(estimator.noise_frames >= estimator.config.min_noise_frames);
  }

  #[test]
  fn a_point_source_trips_the_gate_and_freezes_the_noise_estimate() {
    let mut estimator = estimator();
    let mut seed = 7;
    let spread = diffuse_angles(64);
    for _ in 0..200 {
      estimator.observe(&frame(&spread, 0.2, &mut seed));
    }
    let settled = estimator.noise_frames;

    for _ in 0..100 {
      estimator.observe(&frame(&[(35.0, 60.0, 0.0)], 0.001, &mut seed));
    }
    assert!(estimator.point_like(), "a lone point source did not trip the gate");
    let leaked = estimator.noise_frames - settled;
    let gate_frames = (estimator.config.gate_tau_s * RATE / HOP as f64).ceil() as usize;
    assert!(
      leaked <= 2 * gate_frames,
      "{leaked} frames leaked into the noise estimate against a {gate_frames}-frame gate"
    );
    let phrase_frames = (estimator.config.recent_tau_s * RATE / HOP as f64) as usize;
    assert!(
      leaked * 4 < phrase_frames,
      "{leaked} frames is not short against a phrase"
    );
  }

  #[test]
  fn the_estimated_look_direction_recovers_the_source_it_watched() {
    let mut estimator = estimator();
    let mut seed = 11;
    let spread = diffuse_angles(64);
    for _ in 0..300 {
      estimator.observe(&frame(&spread, 0.05, &mut seed));
    }
    for _ in 0..200 {
      estimator.observe(&frame(&[(-42.0, 80.0, 0.0)], 0.001, &mut seed));
    }
    estimator.mark_target();
    assert!(estimator.has_target());

    let frequencies = bin_frequencies(RATE);
    for freq in [1000.0, 2000.0, 3000.0] {
      let bin = bin_near(freq);
      let field = estimator.field(bin, frequencies[bin], 0.0);
      let truth = steering_vector(frequencies[bin], -42.0);
      let expected = truth.map(|t| t / truth[0]);
      for (position, expected_value) in expected.iter().enumerate() {
        assert!(
          (field.steering[position] - expected_value).norm() < 0.2,
          "{freq} Hz position {position}: got {:?} want {:?}",
          field.steering[position],
          expected_value
        );
      }
    }
  }

  #[test]
  fn the_pooled_bearing_recovers_the_source_and_is_used_where_bins_cannot_resolve() {
    let mut estimator = estimator();
    let mut seed = 23;
    for _ in 0..300 {
      estimator.observe(&frame(&diffuse_angles(64), 0.05, &mut seed));
    }
    for _ in 0..200 {
      estimator.observe(&frame(&[(-42.0, 80.0, 0.0)], 0.001, &mut seed));
    }
    estimator.mark_target();

    let bearing = estimator.bearing_deg().expect("bins should agree on a bearing");
    assert!((bearing + 42.0).abs() < 8.0, "pooled bearing {bearing:.1} against -42");

    let frequencies = bin_frequencies(RATE);
    let bin = bin_near(200.0);
    let field = estimator.field(bin, frequencies[bin], 35.0);
    let pooled = steering_vector(frequencies[bin], bearing);
    let fallback = steering_vector(frequencies[bin], 35.0);
    let to_pooled: f64 = (0..CHANNELS).map(|p| (field.steering[p] - pooled[p]).norm()).sum();
    let to_fallback: f64 = (0..CHANNELS).map(|p| (field.steering[p] - fallback[p]).norm()).sum();
    assert!(
      to_pooled < to_fallback,
      "an unresolved bin used the configured angle instead of the pooled bearing"
    );
  }

  #[test]
  fn a_wake_word_label_alone_does_not_count_as_a_look_direction() {
    let mut estimator = estimator();
    let mut seed = 31;
    for _ in 0..300 {
      estimator.observe(&frame(&diffuse_angles(64), 0.05, &mut seed));
    }
    estimator.mark_target();
    assert!(estimator.has_target(), "the label should still be recorded");
    assert!(
      estimator.bearing_deg().is_none(),
      "diffuse noise should resolve no bearing"
    );
    assert!(
      estimator.unsteered(),
      "a label with no resolved bearing must still read as unsteered"
    );
  }

  #[test]
  fn a_caller_that_knows_its_angle_is_never_forced_to_degrade() {
    let mut estimator = SceneEstimator::new(
      Config {
        degrade_when_unsteered: false,
        ..Config::default()
      },
      RATE,
      HOP,
    );
    let mut seed = 37;
    for _ in 0..200 {
      estimator.observe(&frame(&diffuse_angles(64), 0.05, &mut seed));
    }
    assert!(estimator.bearing_deg().is_none());
    assert!(!estimator.unsteered(), "an opted-out caller was degraded anyway");
  }

  #[test]
  fn no_pooled_bearing_is_invented_without_a_target() {
    let mut estimator = estimator();
    let mut seed = 29;
    for _ in 0..300 {
      estimator.observe(&frame(&diffuse_angles(64), 0.05, &mut seed));
    }
    assert!(estimator.bearing_deg().is_none(), "a bearing appeared with no target");
  }

  #[test]
  fn a_plane_wave_look_direction_reads_back_its_own_bearing() {
    for angle in [-70.0, -35.0, 0.0, 20.0, 61.0] {
      for freq in [900.0, 2000.0, 3500.0] {
        let recovered = implied_bearing_deg(&steering_vector(freq, angle), freq).expect("fit");
        assert!(
          (recovered - angle).abs() < 0.5,
          "{freq} Hz: read {recovered:.2} back from {angle}"
        );
      }
    }
  }

  #[test]
  fn the_geometric_look_direction_survives_until_the_wake_word_labels_one() {
    let estimator = estimator();
    let frequencies = bin_frequencies(RATE);
    let bin = bin_near(2000.0);
    let field = estimator.field(bin, frequencies[bin], 35.0);
    let expected = steering_vector(frequencies[bin], 35.0);
    assert!(
      field
        .steering
        .iter()
        .zip(expected.iter())
        .all(|(actual, expected)| { (*actual - *expected).norm() < 1e-12 })
    );
  }

  #[test]
  fn an_unobserved_scene_falls_back_to_the_assumed_field_exactly() {
    let estimator = estimator();
    let frequencies = bin_frequencies(RATE);
    for bin in [bin_near(750.0), bin_near(2000.0)] {
      let field = estimator.field(bin, frequencies[bin], 35.0);
      let assumed = Field::assumed(frequencies[bin], 35.0);
      for i in 0..CHANNELS {
        for j in 0..CHANNELS {
          assert!((field.noise[i][j] - assumed.noise[i][j]).norm() < 1e-12);
        }
      }
    }
  }

  #[test]
  fn the_look_direction_is_rejected_when_the_target_is_not_rank_one() {
    let mut estimator = estimator();
    let mut seed = 3;
    let spread = diffuse_angles(64);
    for _ in 0..300 {
      estimator.observe(&frame(&spread, 0.05, &mut seed));
    }
    for _ in 0..200 {
      estimator.observe(&frame(&[(-50.0, 40.0, 0.0), (55.0, 40.0, 0.0)], 0.001, &mut seed));
    }
    estimator.mark_target();

    let frequencies = bin_frequencies(RATE);
    let bin = bin_near(2000.0);
    let field = estimator.field(bin, frequencies[bin], 35.0);
    let geometric = steering_vector(frequencies[bin], 35.0);
    let fell_back = (0..CHANNELS).all(|p| (field.steering[p] - geometric[p]).norm() < 1e-12);
    assert!(fell_back, "an ambiguous target was adopted as the look direction");
  }

  #[test]
  fn the_estimated_noise_field_suppresses_a_loudspeaker_the_geometry_cannot_see() {
    use crate::beamformer::{Design, response_db, weights};

    let mut estimator = estimator();
    let mut seed = 5;
    for _ in 0..400 {
      estimator.observe(&frame(&[(-65.0, 50.0, 0.0)], 0.02, &mut seed));
    }

    let frequencies = bin_frequencies(RATE);
    let bin = bin_near(2000.0);
    let design = Design::Superdirective { wng_floor_db: -6.0 };
    let measured = weights(design, &estimator.field(bin, frequencies[bin], 35.0));
    let assumed = weights(design, &Field::assumed(frequencies[bin], 35.0));

    let (got, baseline) = (
      response_db(&measured, frequencies[bin], -65.0),
      response_db(&assumed, frequencies[bin], -65.0),
    );
    assert!(
      got < baseline - 10.0,
      "measured {got:.1} dB against assumed {baseline:.1} dB at the interferer"
    );
  }

  #[test]
  fn resetting_returns_every_estimate_to_the_assumed_field() {
    let mut estimator = estimator();
    let mut seed = 13;
    for _ in 0..200 {
      estimator.observe(&frame(&[(20.0, 30.0, 0.0)], 0.1, &mut seed));
    }
    estimator.mark_target();
    estimator.reset();

    assert!(!estimator.has_target());
    assert!(!estimator.point_like());
    let frequencies = bin_frequencies(RATE);
    let bin = bin_near(2000.0);
    let field = estimator.field(bin, frequencies[bin], 35.0);
    let assumed = Field::assumed(frequencies[bin], 35.0);
    for i in 0..CHANNELS {
      for j in 0..CHANNELS {
        assert!((field.noise[i][j] - assumed.noise[i][j]).norm() < 1e-12);
      }
    }
  }

  fn steered_to(angle_deg: f64, seed: u64) -> SceneEstimator {
    let mut estimator = estimator();
    let mut seed = seed;
    for _ in 0..300 {
      estimator.observe(&frame(&diffuse_angles(64), 0.05, &mut seed));
    }
    for _ in 0..200 {
      estimator.observe(&frame(&[(angle_deg, 80.0, 0.0)], 0.001, &mut seed));
    }
    estimator.mark_target();
    estimator
  }

  #[test]
  fn spatial_evidence_needs_a_bearing_before_it_reports_anything() {
    let mut estimator = estimator();
    let mut seed = 41;
    assert!(estimator.target_evidence().is_none(), "evidence before any observation");
    for _ in 0..300 {
      estimator.observe(&frame(&diffuse_angles(64), 0.05, &mut seed));
    }
    estimator.mark_target();
    assert!(
      estimator.target_evidence().is_none(),
      "diffuse noise resolves no bearing, so there is nothing to gate on"
    );
  }

  #[test]
  fn spatial_evidence_separates_the_marked_bearing_from_every_other_one() {
    let mut seed = 43;
    let mut estimator = steered_to(-42.0, seed);
    assert!(estimator.bearing_deg().is_some(), "the fixture failed to resolve");

    for _ in 0..100 {
      estimator.observe(&frame(&[(-42.0, 80.0, 0.0)], 0.001, &mut seed));
    }
    let on_target = estimator
      .target_evidence()
      .expect("a resolved bearing reports evidence")
      .agreement;

    for _ in 0..100 {
      estimator.observe(&frame(&[(45.0, 80.0, 0.0)], 0.001, &mut seed));
    }
    let off_target = estimator
      .target_evidence()
      .expect("evidence survives the source moving")
      .agreement;

    assert!(on_target > 0.6, "the marked bearing only scored {on_target:.3}");
    assert!(
      off_target < on_target / 2.0,
      "a source at 45 deg scored {off_target:.3} against {on_target:.3} on target"
    );
  }

  #[test]
  fn a_diffuse_field_reads_as_no_evidence_even_with_a_bearing_marked() {
    let mut seed = 47;
    let mut estimator = steered_to(-42.0, seed);
    for _ in 0..200 {
      estimator.observe(&frame(&diffuse_angles(64), 0.2, &mut seed));
    }
    let evidence = estimator
      .target_evidence()
      .expect("a marked bearing still reports")
      .agreement;
    assert!(
      evidence < 0.4,
      "a diffuse wash scored {evidence:.3}, which a gate would read as the talker"
    );
  }

  fn below(cutoff_hz: f64, mut spectra: [Box<[Complex32; BINS]>; CHANNELS]) -> [Box<[Complex32; BINS]>; CHANNELS] {
    let frequencies = bin_frequencies(RATE);
    for channel in &mut spectra {
      for (bin, value) in channel.iter_mut().enumerate() {
        if frequencies[bin] > cutoff_hz {
          *value = Complex32::new(0.0, 0.0);
        }
      }
    }
    spectra
  }

  #[test]
  fn evidence_reports_how_much_of_the_field_reaches_the_gate_band() {
    let mut seed = 59;
    let mut estimator = steered_to(-30.0, seed);
    for _ in 0..100 {
      estimator.observe(&frame(&[(-30.0, 80.0, 0.0)], 0.001, &mut seed));
    }
    let broadband = estimator
      .target_evidence()
      .expect("a marked bearing reports")
      .band_share;

    for _ in 0..100 {
      estimator.observe(&below(1200.0, frame(&[(-30.0, 80.0, 0.0)], 0.001, &mut seed)));
    }
    let dark = estimator
      .target_evidence()
      .expect("a marked bearing reports")
      .band_share;

    let frequencies = bin_frequencies(RATE);
    let (lower, upper) = estimator.gate_bins;
    let flat = (upper - lower + 1) as f32 / frequencies.iter().filter(|f| **f >= BROADBAND_FLOOR_HZ).count() as f32;
    assert!(
      (broadband - flat).abs() < flat * 0.25,
      "a flat source put {broadband:.3} of its power in the band, expected about {flat:.3}"
    );
    assert!(
      dark < 0.01,
      "a source with nothing above 1200 Hz still showed {dark:.4} in the band"
    );
  }

  #[test]
  fn holding_the_noise_estimate_outlasts_the_freeze_cap() {
    let mut seed = 53;
    let point = |estimator: &mut SceneEstimator, seed: &mut u64, frames: usize| {
      for _ in 0..frames {
        estimator.observe(&frame(&[(35.0, 60.0, 0.0)], 0.001, seed));
      }
    };

    let mut capped = estimator();
    let mut held = estimator();
    held.hold_noise(true);
    let past_cap = capped.freeze_limit + 200;
    point(&mut capped, &mut seed.clone(), past_cap);
    point(&mut held, &mut seed, past_cap);

    assert!(
      capped.noise_frames > 0,
      "the cap should have released and let the talker into the noise estimate"
    );
    assert_eq!(
      held.noise_frames, 0,
      "a held estimate absorbed {} frames of the talker it was told to protect",
      held.noise_frames
    );
  }

  #[test]
  fn the_gate_band_lands_where_diffuse_and_point_fields_separate_most() {
    let estimator = estimator();
    let frequencies = bin_frequencies(RATE);
    let aperture = position_offsets_m()[WIDEST.1] - position_offsets_m()[WIDEST.0];
    let first_null = SPEED_OF_SOUND_M_S / (2.0 * aperture);
    assert!(frequencies[estimator.gate_bins.1] < first_null);
    for &frequency in &frequencies[estimator.gate_bins.0..=estimator.gate_bins.1] {
      let z = std::f64::consts::TAU * frequency * aperture / SPEED_OF_SOUND_M_S;
      let diffuse_msc = (z.sin() / z).powi(2);
      assert!(
        diffuse_msc < estimator.config.gate_threshold,
        "a diffuse field reads {diffuse_msc:.3} at {:.0} Hz, above the gate threshold",
        frequency
      );
    }
  }
}
