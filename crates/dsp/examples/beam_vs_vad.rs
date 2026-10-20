use std::{env, f64::consts::TAU, path::Path};

use bridgething_dsp::{
  beamformer::{Design, Nulls},
  geometry::{CHANNELS, POSITION_TO_CHANNEL, SPEED_OF_SOUND_M_S, position_offsets_m},
  highpass::HighPass,
  pipeline::{Beamformer, Config},
};
use earshot::Detector;
use realfft::RealFftPlanner;

const RATE: f64 = 16_000.0;
const FRAME: usize = 256;
const SCENE_SECONDS: usize = 8;
const SPEECH_AT_SECONDS: f64 = 4.0;
const TALKER_DEG: f64 = 35.0;
const INTERFERER_DEG: f64 = -60.0;

fn read_wav(path: &Path) -> Option<Vec<f32>> {
  let mut reader = hound::WavReader::open(path).ok()?;
  let spec = reader.spec();
  let samples: Vec<f32> = match spec.sample_format {
    hound::SampleFormat::Int => reader
      .samples::<i16>()
      .filter_map(Result::ok)
      .map(|s| s as f32 / 32768.0)
      .collect(),
    hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
  };
  if spec.channels > 1 {
    return Some(samples.iter().step_by(spec.channels as usize).copied().collect());
  }
  Some(samples)
}

fn fractional_delay(signal: &[f32], delay_samples: f64, planner: &mut RealFftPlanner<f32>) -> Vec<f32> {
  let n = signal.len().next_power_of_two() * 2;
  let (forward, inverse) = (planner.plan_fft_forward(n), planner.plan_fft_inverse(n));
  let mut padded = vec![0.0f32; n];
  padded[..signal.len()].copy_from_slice(signal);
  let mut spectrum = forward.make_output_vec();
  forward.process(&mut padded, &mut spectrum).expect("fft");

  for (bin, value) in spectrum.iter_mut().enumerate() {
    let phase = -TAU * bin as f64 * delay_samples / n as f64;
    let rotation = num_complex::Complex32::from_polar(1.0, phase as f32);
    *value *= rotation;
  }
  spectrum[0].im = 0.0;
  let last = spectrum.len() - 1;
  spectrum[last].im = 0.0;

  let mut out = vec![0.0f32; n];
  inverse.process(&mut spectrum, &mut out).expect("ifft");
  let scale = 1.0 / n as f32;
  out.truncate(signal.len());
  out.iter().map(|v| v * scale).collect()
}

fn rms(samples: &[f32]) -> f64 {
  if samples.is_empty() {
    return 0.0;
  }
  (samples.iter().map(|s| (s * s) as f64).sum::<f64>() / samples.len() as f64).sqrt()
}

fn render_scene(
  speech: &[f32],
  music: &[f32],
  snr_db: f64,
  planner: &mut RealFftPlanner<f32>,
) -> (Vec<i32>, usize, usize) {
  let total = SCENE_SECONDS * RATE as usize;
  let start = (SPEECH_AT_SECONDS * RATE) as usize;
  let speech_len = speech.len().min(total - start);

  let mut bed = vec![0.0f32; total];
  for (i, sample) in bed.iter_mut().enumerate() {
    *sample = music[i % music.len()];
  }

  let music_level = rms(&bed[start..start + speech_len]);
  let speech_level = rms(&speech[..speech_len]);
  let gain = if speech_level > 0.0 {
    music_level * 10f64.powf(snr_db / 20.0) / speech_level
  } else {
    0.0
  };
  let mut talker = vec![0.0f32; total];
  for i in 0..speech_len {
    talker[start + i] = speech[i] * gain as f32;
  }

  let offsets = position_offsets_m();
  let mut interleaved = vec![0i32; total * CHANNELS];
  for position in 0..CHANNELS {
    let delay_of = |deg: f64| -offsets[position] * deg.to_radians().sin() / SPEED_OF_SOUND_M_S * RATE;
    let talker_at = fractional_delay(&talker, delay_of(TALKER_DEG), planner);
    let music_at = fractional_delay(&bed, delay_of(INTERFERER_DEG), planner);
    let wire = POSITION_TO_CHANNEL[position];
    for n in 0..total {
      let mixed = (talker_at[n] + music_at[n]).clamp(-1.0, 1.0);
      interleaved[n * CHANNELS + wire] = (mixed as f64 * i32::MAX as f64) as i32;
    }
  }
  (interleaved, start, speech_len)
}

fn channel_average(interleaved: &[i32]) -> Vec<f32> {
  interleaved
    .as_chunks::<CHANNELS>()
    .0
    .iter()
    .map(|mics| mics.iter().map(|s| *s as f64).sum::<f64>() / CHANNELS as f64 / i32::MAX as f64)
    .map(|v| v as f32)
    .collect()
}

fn score(mono: &[f32], speech_start: usize, speech_len: usize, threshold: f32) -> (f64, f64) {
  let mut mono = mono.to_vec();
  HighPass::at_array_knee(RATE).process(&mut mono);
  let mono = &mono[..];
  let mut vad = Detector::default();
  let (mut hit, mut speech_frames, mut false_alarm, mut quiet_frames) = (0usize, 0usize, 0usize, 0usize);
  for (index, frame) in mono.as_chunks::<FRAME>().0.iter().enumerate() {
    let at = index * FRAME;
    let voiced = vad.predict_f32(frame) >= threshold;
    if at >= speech_start && at + FRAME <= speech_start + speech_len {
      speech_frames += 1;
      hit += usize::from(voiced);
    } else if at + FRAME <= speech_start || at >= speech_start + speech_len {
      quiet_frames += 1;
      false_alarm += usize::from(voiced);
    }
  }
  (
    hit as f64 / speech_frames.max(1) as f64,
    false_alarm as f64 / quiet_frames.max(1) as f64,
  )
}

fn collect_wavs(dir: &Path, limit: usize) -> Vec<Vec<f32>> {
  let mut paths: Vec<_> = std::fs::read_dir(dir)
    .expect("readable directory")
    .filter_map(Result::ok)
    .map(|e| e.path())
    .filter(|p| p.extension().is_some_and(|e| e == "wav"))
    .collect();
  paths.sort();
  paths.iter().filter_map(|p| read_wav(p)).take(limit).collect()
}

fn main() {
  let args: Vec<String> = env::args().collect();
  let (speech_dir, music_dir) = (Path::new(&args[1]), Path::new(&args[2]));
  let pairs: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(20);

  let speech = collect_wavs(speech_dir, pairs);
  let music = collect_wavs(music_dir, pairs);
  println!("{} speech clips, {} music clips\n", speech.len(), music.len());

  let mut planner = RealFftPlanner::<f32>::new();
  println!("talker at {TALKER_DEG} deg, interferer at {INTERFERER_DEG} deg, vad threshold 0.50");
  println!(
    "{:>7}  {:>22}  {:>22}",
    "snr dB", "beamformed + null", "channel average"
  );
  println!(
    "{:>7}  {:>10} {:>11}  {:>10} {:>11}",
    "", "detect", "false alarm", "detect", "false alarm"
  );

  for snr_db in [12.0, 6.0, 3.0, 0.0, -3.0, -6.0] {
    let (mut beam_hit, mut beam_fa, mut avg_hit, mut avg_fa, mut runs) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (utterance, bed) in speech.iter().zip(&music) {
      let (scene, start, len) = render_scene(utterance, bed, snr_db, &mut planner);

      let mut beamformer = Beamformer::new(Config {
        steering_deg: TALKER_DEG,
        design: Design::Lcmv {
          wng_floor_db: -6.0,
          nulls: Nulls::new(&[INTERFERER_DEG]),
        },
        ..Config::default()
      });
      let mut beamed = Vec::new();
      beamformer.process(&scene, &mut beamed);

      let (bh, bf) = score(&beamed, start, len, 0.5);
      let (ah, af) = score(&channel_average(&scene), start, len, 0.5);
      beam_hit += bh;
      beam_fa += bf;
      avg_hit += ah;
      avg_fa += af;
      runs += 1.0;
    }
    println!(
      "{snr_db:>7.0}  {:>10.3} {:>11.3}  {:>10.3} {:>11.3}",
      beam_hit / runs,
      beam_fa / runs,
      avg_hit / runs,
      avg_fa / runs
    );
  }

  println!("\nthreshold sweep on the full front end at 0 dB snr");
  println!("{:>10} {:>10} {:>12}", "threshold", "detect", "false alarm");
  for threshold in [0.5f32, 0.3, 0.2, 0.1, 0.05, 0.02] {
    let (mut hit, mut fa, mut runs) = (0.0, 0.0, 0.0);
    for (utterance, bed) in speech.iter().zip(&music) {
      let (scene, start, len) = render_scene(utterance, bed, 0.0, &mut planner);
      let mut beamformer = Beamformer::new(Config {
        steering_deg: TALKER_DEG,
        design: Design::Lcmv {
          wng_floor_db: -6.0,
          nulls: Nulls::new(&[INTERFERER_DEG]),
        },
        ..Config::default()
      });
      let mut beamed = Vec::new();
      beamformer.process(&scene, &mut beamed);
      let (h, f) = score(&beamed, start, len, threshold);
      hit += h;
      fa += f;
      runs += 1.0;
    }
    println!("{threshold:>10.2} {:>10.3} {:>12.4}", hit / runs, fa / runs);
  }
}
