use std::{collections::VecDeque, time::Duration};

use earshot::Detector;

use crate::scene::TargetEvidence;

pub const FRAME_SAMPLES: usize = 256;
pub const SAMPLE_RATE_HZ: f64 = 16_000.0;

const FRAME: Duration = Duration::from_nanos((FRAME_SAMPLES as f64 / SAMPLE_RATE_HZ * 1e9) as u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
  SilenceAfterSpeech,
  NoOnset,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
  pub threshold: f32,
  pub onset: Duration,
  pub min_speech: Duration,
  pub hangover: Duration,
  pub spatial_floor: f32,
  pub onset_floor: f32,
  pub min_band_share: f32,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      threshold: 0.6,
      onset: Duration::from_millis(3000),
      min_speech: Duration::from_millis(160),
      hangover: Duration::from_millis(1200),
      spatial_floor: 0.10,
      onset_floor: 0.275,
      min_band_share: 0.03,
    }
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OnsetTrace {
  pub peak_acoustic_run: usize,
  pub peak_gated_run: usize,
  pub vetoed_frames: usize,
  pub abstained_frames: usize,
  pub max_score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
  Armed,
  Speaking,
  Closed,
}

pub struct VoiceEndpointer {
  config: Config,
  detector: Box<Detector>,
  pending: Vec<f32>,
  scratch: [f32; FRAME_SAMPLES],
  phase: Phase,
  speech_seen: bool,
  armed_frames: usize,
  voiced_run: usize,
  gated_run: usize,
  run_evidence: VecDeque<Option<f32>>,
  trace: OnsetTrace,
  quiet_run: usize,
  onset_frames: usize,
  min_speech_frames: usize,
  hangover_frames: usize,
}

impl VoiceEndpointer {
  pub fn new(config: Config) -> Self {
    Self {
      detector: Detector::default_boxed(),
      pending: Vec::with_capacity(FRAME_SAMPLES * 2),
      scratch: [0.0; FRAME_SAMPLES],
      phase: Phase::Armed,
      speech_seen: false,
      armed_frames: 0,
      voiced_run: 0,
      gated_run: 0,
      run_evidence: VecDeque::with_capacity(frames_in(config.min_speech).max(1)),
      trace: OnsetTrace::default(),
      quiet_run: 0,
      onset_frames: frames_in(config.onset),
      min_speech_frames: frames_in(config.min_speech).max(1),
      hangover_frames: frames_in(config.hangover).max(1),
      config,
    }
  }

  pub fn speech_seen(&self) -> bool {
    self.speech_seen
  }

  pub fn onset_trace(&self) -> OnsetTrace {
    self.trace
  }

  pub fn observe(&mut self, samples: &[f32], evidence: Option<TargetEvidence>) -> Option<TurnEnd> {
    if self.phase == Phase::Closed {
      return None;
    }
    let mut verdict = None;
    self.pending.extend_from_slice(samples);
    let mut consumed = 0;
    while verdict.is_none() && consumed + FRAME_SAMPLES <= self.pending.len() {
      self
        .scratch
        .copy_from_slice(&self.pending[consumed..consumed + FRAME_SAMPLES]);
      consumed += FRAME_SAMPLES;
      let score = self.score_frame();
      verdict = self.advance(score, evidence);
    }
    self.pending.drain(..consumed);
    verdict
  }

  fn score_frame(&mut self) -> f32 {
    for sample in &mut self.scratch {
      *sample = sample.clamp(-1.0, 1.0);
    }
    self.detector.predict_f32(&self.scratch)
  }

  fn run_on_target(&self) -> bool {
    let (sum, count) = self
      .run_evidence
      .iter()
      .flatten()
      .fold((0.0f32, 0usize), |(sum, count), agreement| (sum + agreement, count + 1));
    count == 0 || sum / count as f32 >= self.config.onset_floor
  }

  fn advance(&mut self, score: f32, evidence: Option<TargetEvidence>) -> Option<TurnEnd> {
    let acoustic = score >= self.config.threshold;
    let on_target = evidence.is_none_or(|evidence| evidence.agreement >= self.config.spatial_floor);
    match self.phase {
      Phase::Armed => {
        self.armed_frames += 1;
        let audible = evidence.filter(|evidence| evidence.band_share >= self.config.min_band_share);
        let agreement = audible.map(|evidence| evidence.agreement);
        let onset_on_target = agreement.is_none_or(|agreement| agreement >= self.config.onset_floor);
        if acoustic {
          self.voiced_run += 1;
          if self.run_evidence.len() == self.min_speech_frames {
            self.run_evidence.pop_front();
          }
          self.run_evidence.push_back(agreement);
        } else {
          self.voiced_run = 0;
          self.run_evidence.clear();
        }
        self.gated_run = if acoustic && onset_on_target {
          self.gated_run + 1
        } else {
          0
        };
        self.trace = OnsetTrace {
          peak_acoustic_run: self.trace.peak_acoustic_run.max(self.voiced_run),
          peak_gated_run: self.trace.peak_gated_run.max(self.gated_run),
          vetoed_frames: self.trace.vetoed_frames + usize::from(acoustic && !onset_on_target),
          abstained_frames: self.trace.abstained_frames
            + usize::from(acoustic && evidence.is_some() && audible.is_none()),
          max_score: self.trace.max_score.max(score),
        };
        let candidate = self.voiced_run > 0 && self.run_on_target();
        if candidate && self.voiced_run >= self.min_speech_frames {
          self.phase = Phase::Speaking;
          self.speech_seen = true;
          self.quiet_run = 0;
          return None;
        }
        if self.armed_frames >= self.onset_frames && !candidate {
          self.phase = Phase::Closed;
          return Some(TurnEnd::NoOnset);
        }
        None
      }
      Phase::Speaking => {
        let voiced = acoustic && on_target;
        self.quiet_run = if voiced { 0 } else { self.quiet_run + 1 };
        if self.quiet_run >= self.hangover_frames {
          self.phase = Phase::Closed;
          return Some(TurnEnd::SilenceAfterSpeech);
        }
        None
      }
      Phase::Closed => None,
    }
  }
}

fn frames_in(duration: Duration) -> usize {
  duration.div_duration_f64(FRAME).ceil() as usize
}

#[cfg(test)]
mod tests {
  use super::*;

  const TEST_THRESHOLD: f32 = 0.45;

  fn config() -> Config {
    Config {
      threshold: TEST_THRESHOLD,
      onset: Duration::from_millis(500),
      min_speech: Duration::from_millis(112),
      hangover: Duration::from_millis(160),
      spatial_floor: 0.0,
      onset_floor: 0.0,
      min_band_share: 0.0,
    }
  }

  fn judged(agreement: f32) -> Option<TargetEvidence> {
    Some(TargetEvidence {
      agreement,
      band_share: 0.2,
    })
  }

  fn dark(agreement: f32) -> Option<TargetEvidence> {
    Some(TargetEvidence {
      agreement,
      band_share: 0.005,
    })
  }

  fn voiced_frames(count: usize) -> Vec<f32> {
    const F0_HZ: f64 = 120.0;
    const FORMANTS: [(f64, f64); 3] = [(500.0, 1.0), (1500.0, 0.6), (2600.0, 0.35)];
    let mut out = Vec::with_capacity(count * FRAME_SAMPLES);
    for n in 0..count * FRAME_SAMPLES {
      let t = n as f64 / SAMPLE_RATE_HZ;
      let mut value = 0.0;
      for (centre, gain) in FORMANTS {
        for harmonic in 1..=40 {
          let hz = F0_HZ * harmonic as f64;
          value += gain * (-((hz - centre) / 260.0).powi(2)).exp() * (std::f64::consts::TAU * hz * t).sin();
        }
      }
      let syllables = (std::f64::consts::TAU * 4.0 * t).sin().mul_add(0.4, 0.6);
      out.push((value * syllables * 0.2) as f32);
    }
    out
  }

  fn quiet_frames(count: usize) -> Vec<f32> {
    vec![0.0; count * FRAME_SAMPLES]
  }

  fn scores(samples: &[f32]) -> (f32, f32) {
    let mut endpointer = VoiceEndpointer::new(config());
    let (mut low, mut high) = (f32::MAX, f32::MIN);
    for frame in samples.as_chunks::<FRAME_SAMPLES>().0 {
      endpointer.scratch.copy_from_slice(frame);
      let score = endpointer.score_frame();
      low = low.min(score);
      high = high.max(score);
    }
    (low, high)
  }

  #[test]
  fn the_fixtures_straddle_the_test_threshold() {
    let (quietest_voice, _) = scores(&voiced_frames(300));
    let (_, loudest_quiet) = scores(&quiet_frames(300));
    assert!(
      quietest_voice > TEST_THRESHOLD,
      "the voice fixture dipped to {quietest_voice}"
    );
    assert!(
      loudest_quiet < TEST_THRESHOLD,
      "the silence fixture rose to {loudest_quiet}"
    );
  }

  fn feed(endpointer: &mut VoiceEndpointer, samples: &[f32]) -> Option<TurnEnd> {
    feed_with(endpointer, samples, None)
  }

  fn feed_with(endpointer: &mut VoiceEndpointer, samples: &[f32], evidence: Option<TargetEvidence>) -> Option<TurnEnd> {
    let mut end = None;
    for frame in samples.chunks(FRAME_SAMPLES) {
      end = end.or(endpointer.observe(frame, evidence));
    }
    end
  }

  fn gated() -> Config {
    Config {
      spatial_floor: 0.25,
      onset_floor: 0.25,
      min_band_share: 0.03,
      ..config()
    }
  }

  #[test]
  fn a_silent_turn_closes_when_the_onset_window_runs_out() {
    let mut endpointer = VoiceEndpointer::new(config());
    assert_eq!(feed(&mut endpointer, &quiet_frames(60)), Some(TurnEnd::NoOnset));
    assert!(!endpointer.speech_seen());
  }

  #[test]
  fn the_onset_window_is_honoured_to_the_frame() {
    let mut endpointer = VoiceEndpointer::new(config());
    let window = frames_in(config().onset);
    assert_eq!(feed(&mut endpointer, &quiet_frames(window - 1)), None);
    assert_eq!(feed(&mut endpointer, &quiet_frames(1)), Some(TurnEnd::NoOnset));
  }

  #[test]
  fn speech_then_silence_closes_on_the_hangover() {
    let mut endpointer = VoiceEndpointer::new(config());
    assert_eq!(feed(&mut endpointer, &voiced_frames(20)), None);
    assert!(endpointer.speech_seen());
    assert_eq!(
      feed(&mut endpointer, &quiet_frames(40)),
      Some(TurnEnd::SilenceAfterSpeech)
    );
  }

  #[test]
  fn silence_shorter_than_the_hangover_does_not_end_the_turn() {
    let mut endpointer = VoiceEndpointer::new(config());
    feed(&mut endpointer, &voiced_frames(20));
    let pause = frames_in(config().hangover) - 1;
    assert_eq!(feed(&mut endpointer, &quiet_frames(pause)), None);
    assert_eq!(feed(&mut endpointer, &voiced_frames(20)), None);
    assert_eq!(
      feed(&mut endpointer, &quiet_frames(40)),
      Some(TurnEnd::SilenceAfterSpeech)
    );
  }

  #[test]
  fn speech_arriving_late_still_beats_the_onset_window() {
    let mut endpointer = VoiceEndpointer::new(config());
    let window = frames_in(config().onset);
    assert_eq!(feed(&mut endpointer, &quiet_frames(window - 8)), None);
    assert_eq!(feed(&mut endpointer, &voiced_frames(20)), None);
    assert!(endpointer.speech_seen());
  }

  #[test]
  fn a_single_voiced_frame_is_too_short_to_open_a_turn() {
    let mut endpointer = VoiceEndpointer::new(config());
    let click = voiced_frames(1);
    feed(&mut endpointer, &click);
    assert!(!endpointer.speech_seen(), "one frame is below the debounce");
    assert_eq!(feed(&mut endpointer, &quiet_frames(60)), Some(TurnEnd::NoOnset));
  }

  #[test]
  fn a_closed_turn_never_reports_twice() {
    let mut endpointer = VoiceEndpointer::new(config());
    feed(&mut endpointer, &voiced_frames(20));
    assert_eq!(
      feed(&mut endpointer, &quiet_frames(40)),
      Some(TurnEnd::SilenceAfterSpeech)
    );
    assert_eq!(feed(&mut endpointer, &quiet_frames(200)), None);
    assert_eq!(feed(&mut endpointer, &voiced_frames(20)), None);
  }

  #[test]
  fn samples_shorter_than_a_frame_accumulate_across_calls() {
    let mut endpointer = VoiceEndpointer::new(config());
    let quiet = quiet_frames(60);
    let mut end = None;
    for chunk in quiet.chunks(100) {
      end = end.or(endpointer.observe(chunk, None));
    }
    assert_eq!(end, Some(TurnEnd::NoOnset));
  }

  #[test]
  fn spatial_evidence_below_the_floor_keeps_a_loud_interferer_from_opening_a_turn() {
    let mut endpointer = VoiceEndpointer::new(gated());
    assert_eq!(
      feed_with(&mut endpointer, &voiced_frames(60), judged(0.05)),
      Some(TurnEnd::NoOnset)
    );
    assert!(!endpointer.speech_seen(), "off-target energy opened a turn");
  }

  #[test]
  fn spatial_evidence_above_the_floor_leaves_the_acoustic_decision_alone() {
    let mut endpointer = VoiceEndpointer::new(gated());
    assert_eq!(feed_with(&mut endpointer, &voiced_frames(20), judged(0.8)), None);
    assert!(endpointer.speech_seen());
  }

  #[test]
  fn a_gate_that_shuts_mid_turn_runs_the_hangover_down_and_closes() {
    let mut endpointer = VoiceEndpointer::new(gated());
    feed_with(&mut endpointer, &voiced_frames(20), judged(0.8));
    assert!(endpointer.speech_seen());
    assert_eq!(
      feed_with(&mut endpointer, &voiced_frames(40), judged(0.05)),
      Some(TurnEnd::SilenceAfterSpeech),
      "voice-scoring energy off the target bearing must not hold the turn open"
    );
  }

  #[test]
  fn the_gate_is_inert_when_the_beamformer_resolved_no_bearing() {
    let mut endpointer = VoiceEndpointer::new(gated());
    assert_eq!(feed_with(&mut endpointer, &voiced_frames(20), None), None);
    assert!(
      endpointer.speech_seen(),
      "no spatial evidence must mean no spatial veto"
    );
  }

  #[test]
  fn a_zero_floor_passes_every_frame_the_detector_calls_voice() {
    let mut endpointer = VoiceEndpointer::new(config());
    assert_eq!(feed_with(&mut endpointer, &voiced_frames(20), judged(0.0)), None);
    assert!(endpointer.speech_seen());
  }

  #[test]
  fn the_detector_scores_gated_frames_so_its_state_does_not_diverge() {
    let mut open = VoiceEndpointer::new(gated());
    let mut vetoed = VoiceEndpointer::new(gated());
    let speech = voiced_frames(20);
    for frame in speech.chunks(FRAME_SAMPLES) {
      open.observe(frame, judged(0.8));
      vetoed.observe(frame, judged(0.05));
    }
    let (mut open_scores, mut vetoed_scores) = (Vec::new(), Vec::new());
    for frame in voiced_frames(10).as_chunks::<FRAME_SAMPLES>().0 {
      open.scratch.copy_from_slice(frame);
      vetoed.scratch.copy_from_slice(frame);
      open_scores.push(open.score_frame());
      vetoed_scores.push(vetoed.score_frame());
    }
    assert_eq!(
      open_scores, vetoed_scores,
      "a vetoed run fed the detector differently from an open one"
    );
  }

  fn feed_frames(
    endpointer: &mut VoiceEndpointer,
    samples: &[f32],
    evidence: &[Option<TargetEvidence>],
  ) -> Option<TurnEnd> {
    let mut end = None;
    for (frame, frame_evidence) in samples.chunks(FRAME_SAMPLES).zip(evidence) {
      end = end.or(endpointer.observe(frame, *frame_evidence));
    }
    end
  }

  fn shipping_floor() -> Config {
    Config {
      threshold: TEST_THRESHOLD,
      ..Config::default()
    }
  }

  #[test]
  fn a_short_command_with_two_off_target_frames_still_opens_the_turn() {
    let mut endpointer = VoiceEndpointer::new(shipping_floor());
    let evidence: Vec<Option<TargetEvidence>> = (0..12)
      .map(|frame| judged(if frame == 3 || frame == 7 { 0.05 } else { 0.6 }))
      .collect();
    feed_frames(&mut endpointer, &voiced_frames(12), &evidence);
    assert!(
      endpointer.speech_seen(),
      "two off-target frames inside a short command lost the whole command"
    );
    assert_eq!(
      feed(&mut endpointer, &quiet_frames(120)),
      Some(TurnEnd::SilenceAfterSpeech)
    );
  }

  #[test]
  fn a_command_the_gate_band_cannot_hear_opens_on_its_voicing() {
    let mut endpointer = VoiceEndpointer::new(shipping_floor());
    feed_with(&mut endpointer, &voiced_frames(12), dark(0.0));
    assert!(
      endpointer.speech_seen(),
      "a back-vowel command with nothing in the gate band was read as off target"
    );
  }

  #[test]
  fn an_interferer_the_gate_band_places_elsewhere_still_cannot_open_a_turn() {
    let mut endpointer = VoiceEndpointer::new(shipping_floor());
    assert_eq!(
      feed_with(&mut endpointer, &voiced_frames(400), judged(0.05)),
      Some(TurnEnd::NoOnset)
    );
    assert!(!endpointer.speech_seen());
  }

  #[test]
  fn an_open_turn_keeps_the_per_frame_gate_where_the_band_abstains() {
    let mut endpointer = VoiceEndpointer::new(gated());
    feed_with(&mut endpointer, &voiced_frames(20), judged(0.8));
    assert!(endpointer.speech_seen());
    assert_eq!(
      feed_with(&mut endpointer, &voiced_frames(40), dark(0.05)),
      Some(TurnEnd::SilenceAfterSpeech),
      "an interferer the band abstains on must not hold the turn open"
    );
  }

  #[test]
  fn target_speech_beginning_under_a_voiced_interferer_still_opens_the_turn() {
    let mut endpointer = VoiceEndpointer::new(gated());
    let evidence: Vec<Option<TargetEvidence>> = (0..30)
      .map(|frame| judged(if frame < 15 { 0.05 } else { 0.8 }))
      .collect();
    assert_eq!(feed_frames(&mut endpointer, &voiced_frames(30), &evidence), None);
    assert!(endpointer.speech_seen());
  }

  #[test]
  fn the_onset_trace_explains_a_turn_that_never_opened() {
    let mut endpointer = VoiceEndpointer::new(gated());
    let window = frames_in(gated().onset);
    assert_eq!(
      feed_with(&mut endpointer, &voiced_frames(60), judged(0.05)),
      Some(TurnEnd::NoOnset)
    );
    let trace = endpointer.onset_trace();
    assert_eq!(trace.peak_acoustic_run, window);
    assert_eq!(trace.peak_gated_run, 0);
    assert_eq!(trace.vetoed_frames, window);
    assert_eq!(trace.abstained_frames, 0);
    assert!(trace.max_score >= TEST_THRESHOLD, "max score {}", trace.max_score);
  }

  #[test]
  fn the_onset_trace_counts_frames_the_band_abstained_on() {
    let mut endpointer = VoiceEndpointer::new(gated());
    feed_with(&mut endpointer, &voiced_frames(2), dark(0.0));
    assert_eq!(feed(&mut endpointer, &quiet_frames(60)), Some(TurnEnd::NoOnset));
    let trace = endpointer.onset_trace();
    assert_eq!(trace.abstained_frames, 2);
    assert_eq!(trace.vetoed_frames, 0);
    assert!(trace.peak_acoustic_run >= 2, "peak run {}", trace.peak_acoustic_run);
  }

  #[test]
  fn a_partial_trailing_frame_is_kept_for_the_next_call() {
    let mut endpointer = VoiceEndpointer::new(config());
    endpointer.observe(&quiet_frames(1)[..FRAME_SAMPLES / 2], None);
    assert_eq!(endpointer.armed_frames, 0, "half a frame is not a decision");
    endpointer.observe(&quiet_frames(1)[..FRAME_SAMPLES / 2], None);
    assert_eq!(endpointer.armed_frames, 1);
  }
}
