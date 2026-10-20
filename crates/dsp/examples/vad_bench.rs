use std::{
  collections::BTreeMap,
  env,
  path::Path,
  time::{Duration, Instant},
};

use bridgething_dsp::{
  bench::{Scene, read_scene},
  geometry::CHANNELS,
  pipeline::{Beamformer, Config, to_pcm16},
  scene::{self, TargetEvidence},
  vad::{Config as VadConfig, FRAME_SAMPLES, TurnEnd, VoiceEndpointer},
};

const RATE: f64 = 16_000.0;
const LEAD: usize = 8_000;
const PATIENCE: usize = 48_000;

const HANGOVERS_MS: [u64; 2] = [800, 1200];
const THRESHOLDS: [f32; 2] = [0.50, 0.60];
const FLOORS: [f32; 6] = [0.0, 0.05, 0.10, 0.20, 0.30, 0.45];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
  Turns,
  Pause,
  Silent,
  Command,
}

struct Turn {
  archetype: String,
  arm_at: usize,
  speech_end: usize,
  limit: usize,
  pause: Option<usize>,
}

#[derive(Default)]
struct Outcome {
  latencies_ms: Vec<f64>,
  cuts: usize,
  onset_missed: usize,
  never_closed: usize,
  onset_fired: usize,
  turns: usize,
}

impl Outcome {
  fn percentile(&self, fraction: f64) -> f64 {
    if self.latencies_ms.is_empty() {
      return f64::NAN;
    }
    let mut sorted = self.latencies_ms.clone();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
  }

  fn rate(&self, count: usize) -> f64 {
    if self.turns == 0 {
      f64::NAN
    } else {
      count as f64 / self.turns as f64
    }
  }

  fn absorb(&mut self, other: &Outcome) {
    self.latencies_ms.extend_from_slice(&other.latencies_ms);
    self.cuts += other.cuts;
    self.onset_missed += other.onset_missed;
    self.never_closed += other.never_closed;
    self.onset_fired += other.onset_fired;
    self.turns += other.turns;
  }
}

struct FrontEnd {
  mono: Vec<f32>,
  evidence: Vec<Option<TargetEvidence>>,
}

impl FrontEnd {
  fn at(&self, frame: usize) -> Option<TargetEvidence> {
    self.evidence.get(frame).copied().flatten()
  }
}

fn front_end(interleaved: &[i32], marks: &[usize], streaming: bool) -> FrontEnd {
  let mut beamformer = Beamformer::new(Config {
    adaptation: Some(scene::Config::default()),
    ..Config::default()
  });
  let (replayed, live) = if streaming { (&[][..], marks) } else { (marks, &[][..]) };

  let mut cursor = 0usize;
  for mark in replayed {
    let end = (mark * CHANNELS).min(interleaved.len());
    if end <= cursor {
      continue;
    }
    let mut discard = Vec::with_capacity((end - cursor) / CHANNELS);
    beamformer.process(&interleaved[cursor..end], &mut discard);
    beamformer.mark_target();
    beamformer.hold_noise(true);
    cursor = end;
  }
  if !replayed.is_empty() {
    beamformer.reset_frames();
  }

  let mut out = Vec::with_capacity(interleaved.len() / CHANNELS);
  let mut evidence = Vec::with_capacity(out.capacity() / FRAME_SAMPLES);
  let mut pending = live.iter().peekable();
  for chunk in interleaved.chunks(FRAME_SAMPLES * CHANNELS) {
    if pending.next_if(|mark| out.len() >= **mark).is_some() {
      beamformer.mark_target();
      beamformer.hold_noise(true);
    }
    beamformer.process(chunk, &mut out);
    let agreement = beamformer.target_evidence();
    while evidence.len() < out.len() / FRAME_SAMPLES {
      evidence.push(agreement);
    }
  }

  let mut encoded = Vec::with_capacity(out.len() * 2);
  to_pcm16(&out, &mut encoded);
  FrontEnd {
    mono: encoded
      .as_chunks::<2>()
      .0
      .iter()
      .map(|pair| i16::from_le_bytes(*pair) as f32 / 32768.0)
      .collect(),
    evidence,
  }
}

fn turns_of(meta: &Scene, kind: Kind, total: usize) -> Vec<Turn> {
  let spans = meta.spans();
  let turn = |arm_at: usize, speech_end: usize, limit: usize, pause: Option<usize>| Turn {
    archetype: meta.archetype.clone(),
    arm_at,
    speech_end,
    limit: limit.min(total),
    pause,
  };
  match kind {
    Kind::Silent => vec![turn(meta.speech_start.saturating_sub(LEAD), 0, total, None)],
    Kind::Turns => spans
      .iter()
      .enumerate()
      .map(|(index, (start, len))| {
        let floor = index.checked_sub(1).map_or(0, |prev| spans[prev].0 + spans[prev].1);
        let next = spans.get(index + 1).map_or(usize::MAX, |(start, _)| *start);
        turn(
          start.saturating_sub(LEAD).max(floor),
          start + len,
          (start + len + PATIENCE).min(next),
          None,
        )
      })
      .collect(),
    Kind::Command => match spans.as_slice() {
      [(wake_start, wake_len), (start, len)] => {
        vec![turn(wake_start + wake_len, start + len, start + len + PATIENCE, None)]
      }
      _ => Vec::new(),
    },
    Kind::Pause => {
      let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
        return Vec::new();
      };
      if spans.len() < 2 || last.0 <= first.0 + first.1 {
        return Vec::new();
      }
      vec![turn(
        first.0.saturating_sub(LEAD),
        last.0 + last.1,
        last.0 + last.1 + PATIENCE,
        Some(last.0 - (first.0 + first.1)),
      )]
    }
  }
}

struct Cost {
  elapsed: Duration,
  frames: usize,
}

fn run_turn(front: &FrontEnd, turn: &Turn, config: VadConfig, cost: &mut Cost) -> Outcome {
  let mut endpointer = VoiceEndpointer::new(config);
  let mut outcome = Outcome {
    turns: 1,
    ..Outcome::default()
  };

  let mono = &front.mono;
  let from = turn.arm_at.min(mono.len());
  let window = &mono[from..turn.limit.min(mono.len())];
  let started = Instant::now();
  let mut closed = None;
  for (index, frame) in window.as_chunks::<FRAME_SAMPLES>().0.iter().enumerate() {
    cost.frames += 1;
    if let Some(end) = endpointer.observe(frame, front.at((from + index * FRAME_SAMPLES) / FRAME_SAMPLES)) {
      closed = Some((turn.arm_at + (index + 1) * FRAME_SAMPLES, end));
      break;
    }
  }
  cost.elapsed += started.elapsed();

  outcome.onset_fired = usize::from(endpointer.speech_seen());
  match closed {
    None => outcome.never_closed = 1,
    Some((_, TurnEnd::NoOnset)) => outcome.onset_missed = 1,
    Some((at, TurnEnd::SilenceAfterSpeech)) => {
      if at < turn.speech_end {
        outcome.cuts = 1;
      } else {
        outcome.latencies_ms.push((at - turn.speech_end) as f64 / RATE * 1000.0);
      }
    }
  }
  outcome
}

fn pause_bucket(samples: usize) -> &'static str {
  match (samples as f64 / RATE * 1000.0) as u64 {
    0..=399 => "<0.4s",
    400..=699 => "0.4-0.7s",
    700..=999 => "0.7-1.0s",
    1000..=1399 => "1.0-1.4s",
    _ => ">1.4s",
  }
}

fn main() {
  let mut sets: Vec<(Kind, String)> = Vec::new();
  for arg in env::args().skip(1) {
    let (kind, dir) = arg.split_once('=').expect("arguments are kind=dir");
    let kind = match kind {
      "turns" => Kind::Turns,
      "pause" => Kind::Pause,
      "silent" => Kind::Silent,
      "command" => Kind::Command,
      other => panic!("unknown scene kind {other}"),
    };
    sets.push((kind, dir.to_string()));
  }
  assert!(!sets.is_empty(), "give at least one kind=dir");

  let grid: Vec<(u64, f32, f32)> = HANGOVERS_MS
    .iter()
    .flat_map(|hangover| {
      THRESHOLDS
        .iter()
        .flat_map(move |threshold| FLOORS.iter().map(move |floor| (*hangover, *threshold, *floor)))
    })
    .collect();

  let mut by_setting: BTreeMap<(Kind, u64, u32, u32), Outcome> = BTreeMap::new();
  let mut by_archetype: BTreeMap<(Kind, u64, u32, u32, String), Outcome> = BTreeMap::new();
  let mut by_pause: BTreeMap<(u64, u32, u32, &'static str), Outcome> = BTreeMap::new();
  let mut unmarked_silent: BTreeMap<(u64, u32, u32), Outcome> = BTreeMap::new();
  let mut by_command: BTreeMap<(String, u64, u32, u32), Outcome> = BTreeMap::new();
  let mut cost = Cost {
    elapsed: Duration::ZERO,
    frames: 0,
  };
  let mut scenes_read = 0usize;
  let mut spread: BTreeMap<&'static str, Vec<f32>> = BTreeMap::new();
  let mut resolved = (0usize, 0usize);

  for (kind, dir) in &sets {
    let dir = Path::new(dir);
    let manifest: Vec<Scene> =
      serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).expect("manifest.json"))
        .expect("manifest parses");
    eprintln!("{} scenes from {}", manifest.len(), dir.display());

    for meta in &manifest {
      let Some(interleaved) = read_scene(&dir.join(&meta.audio)) else {
        continue;
      };
      scenes_read += 1;
      let spans = meta.spans();
      let marks: Vec<usize> = match (*kind, spans.first()) {
        (Kind::Command, Some((start, len))) => vec![start + len],
        (_, None) => vec![meta.speech_start.saturating_sub(LEAD)],
        _ => spans.iter().map(|(start, len)| start + len).collect(),
      };
      let front = front_end(&interleaved, &marks, *kind == Kind::Command);
      let unmarked = (*kind == Kind::Silent).then(|| front_end(&interleaved, &[], false));
      let label = dir
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
      let turns = turns_of(meta, *kind, front.mono.len());

      resolved.1 += 1;
      resolved.0 += usize::from(front.evidence.iter().any(Option::is_some));
      for (index, evidence) in front.evidence.iter().enumerate() {
        let Some(TargetEvidence {
          agreement: evidence, ..
        }) = evidence
        else {
          continue;
        };
        let at = index * FRAME_SAMPLES;
        let talking = spans.iter().any(|(start, len)| at >= *start && at < start + len);
        let bucket = match (*kind, talking) {
          (Kind::Silent, _) => "interferers only",
          (_, true) => "talker speaking",
          (_, false) => "between utterances",
        };
        spread.entry(bucket).or_default().push(*evidence);
      }

      for turn in &turns {
        for (hangover, threshold, floor) in &grid {
          let config = VadConfig {
            threshold: *threshold,
            hangover: Duration::from_millis(*hangover),
            spatial_floor: *floor,
            ..VadConfig::default()
          };
          let outcome = run_turn(&front, turn, config, &mut cost);
          let key = (*kind, *hangover, threshold.to_bits(), floor.to_bits());
          by_setting.entry(key).or_default().absorb(&outcome);
          by_archetype
            .entry((key.0, key.1, key.2, key.3, turn.archetype.clone()))
            .or_default()
            .absorb(&outcome);
          if let Some(pause) = turn.pause {
            by_pause
              .entry((*hangover, threshold.to_bits(), floor.to_bits(), pause_bucket(pause)))
              .or_default()
              .absorb(&outcome);
          }
          if *kind == Kind::Command {
            by_command
              .entry((label.clone(), *hangover, threshold.to_bits(), floor.to_bits()))
              .or_default()
              .absorb(&outcome);
          }
          if let Some(unmarked) = unmarked.as_ref() {
            let outcome = run_turn(unmarked, turn, config, &mut cost);
            unmarked_silent
              .entry((*hangover, threshold.to_bits(), floor.to_bits()))
              .or_default()
              .absorb(&outcome);
          }
        }
      }
    }
  }

  println!(
    "\n{scenes_read} scenes, onset window {:?}, min speech {:?}",
    VadConfig::default().onset,
    VadConfig::default().min_speech
  );
  println!(
    "endpointer cost: {:.1} us per 16 ms frame over {} frames ({:.4}x realtime on this host)",
    cost.elapsed.as_secs_f64() * 1e6 / cost.frames.max(1) as f64,
    cost.frames,
    cost.elapsed.as_secs_f64() * 1e6 / cost.frames.max(1) as f64 / 16_000.0
  );

  println!(
    "\nspatial evidence: {} of {} scenes resolved a bearing to gate on",
    resolved.0, resolved.1
  );
  println!(
    "{:>20} {:>9} {:>8} {:>8} {:>8} {:>8} {:>10}",
    "frames", "p10", "p25", "p50", "p75", "p90", "count"
  );
  for (bucket, values) in &mut spread {
    values.sort_by(f32::total_cmp);
    let quantile = |f: f64| values[(((values.len() - 1) as f64 * f).round() as usize).min(values.len() - 1)];
    println!(
      "{bucket:>20} {:>9.3} {:>8.3} {:>8.3} {:>8.3} {:>8.3} {:>10}",
      quantile(0.10),
      quantile(0.25),
      quantile(0.50),
      quantile(0.75),
      quantile(0.90),
      values.len()
    );
  }

  if by_setting.keys().any(|k| k.0 == Kind::Turns) {
    println!("\nindependent turns: close latency past the end of the utterance");
    println!(
      "{:>9} {:>10} {:>6} {:>9} {:>9} {:>10} {:>11} {:>12} {:>7}",
      "hangover", "threshold", "floor", "p50 ms", "p90 ms", "false cut", "onset miss", "never closed", "turns"
    );
    for ((kind, hangover, threshold, floor), outcome) in &by_setting {
      if *kind != Kind::Turns {
        continue;
      }
      println!(
        "{hangover:>9} {:>10.2} {:>6.2} {:>9.0} {:>9.0} {:>10.4} {:>11.4} {:>12.4} {:>7}",
        f32::from_bits(*threshold),
        f32::from_bits(*floor),
        outcome.percentile(0.5),
        outcome.percentile(0.9),
        outcome.rate(outcome.cuts),
        outcome.rate(outcome.onset_missed),
        outcome.rate(outcome.never_closed),
        outcome.turns
      );
    }
  }

  if !by_command.is_empty() {
    let hangover = HANGOVERS_MS[HANGOVERS_MS.len() - 1];
    println!("\nwake word then a short command, armed where the wake word ends (hangover {hangover} ms)");
    println!(
      "{:>28} {:>10} {:>6} {:>9} {:>10} {:>11} {:>12} {:>7}",
      "set", "threshold", "floor", "p50 ms", "false cut", "onset miss", "never closed", "turns"
    );
    for ((label, at_hangover, threshold, floor), outcome) in &by_command {
      if *at_hangover != hangover {
        continue;
      }
      println!(
        "{label:>28} {:>10.2} {:>6.2} {:>9.0} {:>10.4} {:>11.4} {:>12.4} {:>7}",
        f32::from_bits(*threshold),
        f32::from_bits(*floor),
        outcome.percentile(0.5),
        outcome.rate(outcome.cuts),
        outcome.rate(outcome.onset_missed),
        outcome.rate(outcome.never_closed),
        outcome.turns
      );
    }
  }

  if !by_pause.is_empty() {
    println!("\nutterance with a pause in it: rate of closing before the talker is done");
    let buckets = ["<0.4s", "0.4-0.7s", "0.7-1.0s", "1.0-1.4s", ">1.4s"];
    print!("{:>9} {:>10} {:>6}", "hangover", "threshold", "floor");
    for bucket in buckets {
      print!(" {bucket:>10}");
    }
    println!("{:>8}", "p50 ms");
    for (hangover, threshold, floor) in &grid {
      print!("{hangover:>9} {:>10.2} {:>6.2}", threshold, floor);
      let mut whole = Outcome::default();
      for bucket in buckets {
        match by_pause.get(&(*hangover, threshold.to_bits(), floor.to_bits(), bucket)) {
          Some(outcome) => {
            print!(" {:>10.3}", outcome.rate(outcome.cuts));
            whole.absorb(outcome);
          }
          None => print!(" {:>10}", "-"),
        }
      }
      println!("{:>8.0}", whole.percentile(0.5));
    }
    print!("{:>27}", "turns per bucket");
    for bucket in buckets {
      let turns = by_pause
        .get(&(grid[0].0, grid[0].1.to_bits(), grid[0].2.to_bits(), bucket))
        .map_or(0, |o| o.turns);
      print!(" {turns:>10}");
    }
    println!();
  }

  if by_setting.keys().any(|k| k.0 == Kind::Silent) {
    println!("\ntalker muted: the interferers alone must not read as a talker");
    println!(
      "{:>10} {:>6} {:>10} {:>12} {:>13} {:>12} {:>7}",
      "threshold", "floor", "front end", "onset fired", "closed silent", "never closed", "turns"
    );
    let hangover = HANGOVERS_MS[HANGOVERS_MS.len() - 1];
    for threshold in THRESHOLDS {
      for floor in FLOORS {
        let key = (hangover, threshold.to_bits(), floor.to_bits());
        for (label, outcome) in [
          ("unmarked", unmarked_silent.get(&key)),
          ("false fire", by_setting.get(&(Kind::Silent, key.0, key.1, key.2))),
        ] {
          let Some(outcome) = outcome else { continue };
          println!(
            "{threshold:>10.2} {floor:>6.2} {label:>10} {:>12.4} {:>13.4} {:>12.4} {:>7}",
            outcome.rate(outcome.onset_fired),
            outcome.rate(outcome.onset_missed),
            outcome.rate(outcome.never_closed),
            outcome.turns
          );
        }
      }
    }
  }

  let archetypes: Vec<String> = {
    let mut seen: Vec<String> = by_archetype.keys().map(|(_, _, _, _, a)| a.clone()).collect();
    seen.sort();
    seen.dedup();
    seen
  };
  if !archetypes.is_empty() && by_archetype.keys().any(|k| k.0 == Kind::Turns) {
    println!("\nindependent turns by archetype: p50 / p90 close latency in ms");
    print!("{:>9} {:>10} {:>6}", "hangover", "threshold", "floor");
    for archetype in &archetypes {
      print!(" {archetype:>16}");
    }
    println!();
    for (hangover, threshold, floor) in &grid {
      print!("{hangover:>9} {:>10.2} {:>6.2}", threshold, floor);
      for archetype in &archetypes {
        match by_archetype.get(&(
          Kind::Turns,
          *hangover,
          threshold.to_bits(),
          floor.to_bits(),
          archetype.clone(),
        )) {
          Some(outcome) => print!(" {:>7.0} /{:>7.0}", outcome.percentile(0.5), outcome.percentile(0.9)),
          None => print!(" {:>16}", "-"),
        }
      }
      println!();
    }
  }
}
