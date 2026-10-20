use std::{env, path::PathBuf, time::Instant};

use bridgething_wakeword::{
  WakeWord,
  features::{CHUNK_SAMPLES, SAMPLE_RATE},
};

fn main() {
  let mut args = env::args().skip(1);
  let (Some(audio), Some(phrase)) = (args.next(), args.next()) else {
    eprintln!("usage: runtime_bench <audio.raw> <phrase-model> [threshold]");
    std::process::exit(2);
  };
  let threshold: f32 = args.next().map(|t| t.parse().expect("bad threshold")).unwrap_or(0.5);

  let bytes = std::fs::read(&audio).expect("audio should be readable");
  let samples: Vec<f32> = bytes
    .as_chunks::<2>()
    .0
    .iter()
    .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
    .collect();
  let duration = samples.len() as f64 / SAMPLE_RATE as f64;

  let mut detector = WakeWord::new(&PathBuf::from(phrase), threshold).expect("models should load");

  let mut peak = 0.0f32;
  let mut elapsed = 0.0f64;
  for chunk in samples.chunks(CHUNK_SAMPLES) {
    let start = Instant::now();
    let hit = detector.push(chunk).expect("inference should succeed");
    elapsed += start.elapsed().as_secs_f64();

    peak = peak.max(detector.score().expect("scoring should succeed"));
    if let Some(hit) = hit {
      println!(
        "hit at {:.3}s score {:.4} sample {}",
        hit.at_sample as f64 / SAMPLE_RATE as f64,
        hit.score,
        hit.at_sample
      );
    }
  }

  println!(
    "audio {duration:.1}s, compute {elapsed:.1}s, real-time factor {:.3}, peak score {peak:.4}",
    elapsed / duration
  );
}
