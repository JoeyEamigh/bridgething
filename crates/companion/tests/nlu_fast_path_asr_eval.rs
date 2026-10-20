use std::{collections::BTreeMap, env, fs};

use bridgething_companion::voice::fast_path;
use libbridgething::{NluAmount, NluDirection, NluPlaybackSpeed, NluRepeatMode, NluSlots, NluView};
use serde::Deserialize;
use serde_json::Value;

const EXPRESSIBLE_SLOTS: &[(&str, &[&str])] = &[
  ("PLAY", &["target_type"]),
  ("PAUSE", &[]),
  ("NEXT", &[]),
  ("PREVIOUS", &[]),
  ("SET_VOLUME", &["level", "direction", "amount", "mute"]),
  ("SET_SHUFFLE", &["enabled"]),
  ("SET_REPEAT", &["repeat_mode"]),
  ("SET_PLAYBACK_SPEED", &["speed"]),
  ("SEEK_RELATIVE", &["seconds"]),
  ("PRESET_PLAY", &["preset"]),
  ("PRESET_SAVE", &["preset"]),
  ("SHOW_VIEW", &["view"]),
];

const SCORED: &[&str] = &["PAUSE", "NEXT", "PREVIOUS", "SET_SHUFFLE", "SET_REPEAT"];

const RECALL_FLOOR: f64 = 0.65;
const PARTIAL_FIRE_CEILING: usize = 2;

#[derive(Deserialize)]
struct Gold {
  intent: String,
  #[serde(default)]
  slots: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct Row {
  utterance: String,
  reference: String,
  gold: Gold,
}

type Lane = (&'static str, fn(&Row) -> &str);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
  Correct,
  SlotsWrong,
  IntentWrong,
  Declined,
}

#[derive(Default)]
struct Tally {
  correct: usize,
  slots_wrong: usize,
  intent_wrong: usize,
  declined: usize,
}

impl Tally {
  fn total(&self) -> usize {
    self.correct + self.slots_wrong + self.intent_wrong + self.declined
  }

  fn add(&mut self, outcome: Outcome) {
    match outcome {
      Outcome::Correct => self.correct += 1,
      Outcome::SlotsWrong => self.slots_wrong += 1,
      Outcome::IntentWrong => self.intent_wrong += 1,
      Outcome::Declined => self.declined += 1,
    }
  }
}

fn repeat_name(mode: NluRepeatMode) -> &'static str {
  match mode {
    NluRepeatMode::Off => "off",
    NluRepeatMode::All => "all",
    NluRepeatMode::One => "one",
  }
}

fn speed_name(speed: NluPlaybackSpeed) -> &'static str {
  match speed {
    NluPlaybackSpeed::One => "1",
    NluPlaybackSpeed::OnePointTwo => "1.2",
    NluPlaybackSpeed::OnePointFive => "1.5",
    NluPlaybackSpeed::Two => "2",
  }
}

fn direction_name(direction: NluDirection) -> &'static str {
  match direction {
    NluDirection::Up => "up",
    NluDirection::Down => "down",
  }
}

fn amount_name(amount: NluAmount) -> &'static str {
  match amount {
    NluAmount::Small => "small",
    NluAmount::Medium => "medium",
    NluAmount::Large => "large",
  }
}

fn library_collection(value: &Value) -> bool {
  value
    .as_str()
    .is_some_and(|target| [fast_path::LIKED_SONGS, fast_path::SAVED_ALBUMS].contains(&target))
}

fn expressible(gold: &Gold) -> bool {
  let Some((_, allowed)) = EXPRESSIBLE_SLOTS.iter().find(|(intent, _)| *intent == gold.intent) else {
    return false;
  };
  if !gold.slots.iter().all(|(key, value)| {
    allowed.contains(&key.as_str()) || (gold.intent == "PLAY" && key == "target" && library_collection(value))
  }) {
    return false;
  }
  if gold.intent == "SHOW_VIEW" {
    return gold.slots.get("view").and_then(Value::as_str) == Some("now_playing");
  }
  true
}

fn slots_agree(intent: &str, slots: &NluSlots, gold: &Gold) -> bool {
  gold.slots.iter().all(|(key, value)| match (key.as_str(), value) {
    ("repeat_mode", Value::String(want)) => slots.repeat_mode.map(repeat_name) == Some(want.as_str()),
    ("enabled", Value::Bool(want)) => slots.enabled == Some(*want),
    ("mute", Value::Bool(want)) => slots.mute == Some(*want),
    ("preset", Value::String(want)) => slots.preset.as_deref() == Some(want.as_str()),
    ("level", Value::Number(want)) => want.as_u64().is_some_and(|n| slots.level == Some(n as u32)),
    ("seconds", Value::Number(want)) => want.as_i64().is_some_and(|n| slots.seconds == Some(n as i32)),
    ("speed", Value::String(want)) => slots.speed.map(speed_name) == Some(want.as_str()),
    ("direction", Value::String(want)) => slots.direction.map(direction_name) == Some(want.as_str()),
    ("amount", Value::String(want)) => slots.amount.map(amount_name) == Some(want.as_str()),
    ("view", Value::String(want)) if want == "now_playing" => slots.view == Some(NluView::NowPlaying),
    ("target_type", Value::String(_)) => intent == "PLAY",
    ("target", Value::String(want)) => slots.target.as_deref() == Some(want.as_str()),
    _ => false,
  })
}

fn score(transcript: &str, gold: &Gold) -> Outcome {
  let Some(hit) = fast_path::match_transcript(transcript) else {
    return Outcome::Declined;
  };
  if hit.intent != gold.intent {
    return Outcome::IntentWrong;
  }
  if slots_agree(hit.intent, &hit.slots, gold) {
    Outcome::Correct
  } else {
    Outcome::SlotsWrong
  }
}

fn pct(n: usize, d: usize) -> String {
  if d == 0 {
    "  n/a".to_string()
  } else {
    format!("{:5.1}%", 100.0 * n as f64 / d as f64)
  }
}

fn load_rows() -> Option<Vec<Row>> {
  let path = env::var("BRIDGETHING_ASR_EVAL").ok()?;
  let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("asr eval corpus at {path}: {e}"));
  Some(
    text
      .lines()
      .map(str::trim)
      .filter(|line| !line.is_empty())
      .map(|line| serde_json::from_str(line).expect("corpus row parses"))
      .collect(),
  )
}

#[test]
fn fast_path_holds_its_measured_floors_on_the_real_asr_corpus() {
  let Some(rows) = load_rows() else {
    eprintln!("BRIDGETHING_ASR_EVAL unset; skipping the real-asr fast path eval");
    return;
  };

  println!("rows: {}\n", rows.len());

  let lanes: [Lane; 2] = [
    ("hypothesis (whisper)", |row| row.utterance.as_str()),
    ("reference (clean)", |row| row.reference.as_str()),
  ];

  for (label, pick) in lanes {
    let mut by_intent: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut recall_hit = 0usize;
    let mut recall_total = 0usize;
    let mut wrong_fire = 0usize;
    let mut must_not_fire_total = 0usize;
    let mut partial_fire = 0usize;
    let mut served = 0usize;
    let mut needs_model = 0usize;

    for row in &rows {
      let outcome = score(pick(row), &row.gold);
      by_intent.entry(row.gold.intent.as_str()).or_default().add(outcome);

      let reachable = expressible(&row.gold);
      if SCORED.contains(&row.gold.intent.as_str()) && reachable {
        recall_total += 1;
        if outcome == Outcome::Correct {
          recall_hit += 1;
        }
      }
      if !reachable {
        must_not_fire_total += 1;
        match outcome {
          Outcome::Declined => {}
          Outcome::SlotsWrong | Outcome::Correct => partial_fire += 1,
          Outcome::IntentWrong => wrong_fire += 1,
        }
      }
      if outcome == Outcome::Correct {
        served += 1;
      } else if row.gold.intent != "NO_INTENT" {
        needs_model += 1;
      }
    }

    println!("== {label} ==");
    println!("  intent            n   correct  slotWrong intentWrong  declined");
    for (intent, tally) in &by_intent {
      let total = tally.total();
      println!(
        "  {:<16} {:>4}    {}     {}      {}     {}",
        intent,
        total,
        pct(tally.correct, total),
        pct(tally.slots_wrong, total),
        pct(tally.intent_wrong, total),
        pct(tally.declined, total)
      );
    }

    println!();
    println!(
      "  END TO END served correctly: {served}/{} {}",
      rows.len(),
      pct(served, rows.len())
    );
    println!(
      "  real commands needing the model: {needs_model}/{} {}",
      rows.len(),
      pct(needs_model, rows.len())
    );
    println!(
      "  recall on scored classes: {recall_hit}/{recall_total} {}",
      pct(recall_hit, recall_total)
    );
    println!(
      "  wrong fire on must-decline: {wrong_fire}/{must_not_fire_total} {}",
      pct(wrong_fire, must_not_fire_total)
    );
    println!("  partial fire (same intent, slot underserved): {partial_fire}/{must_not_fire_total}");
    println!();

    println!("== fires on must-decline, {label} ==");
    let mut shown = 0usize;
    for row in &rows {
      if expressible(&row.gold) {
        continue;
      }
      let text = pick(row);
      let Some(hit) = fast_path::match_transcript(text) else {
        continue;
      };
      shown += 1;
      println!("  gold={} got={}  {text:?}", row.gold.intent, hit.intent);
    }
    println!("  total: {shown}\n");

    if label.starts_with("hypothesis") {
      assert_eq!(
        wrong_fire, 0,
        "{label}: fast path claimed {wrong_fire} utterances it cannot serve"
      );
      assert!(
        partial_fire <= PARTIAL_FIRE_CEILING,
        "{label}: same-intent partial fires grew past the measured count"
      );
    }
    assert!(
      recall_hit as f64 / recall_total as f64 >= RECALL_FLOOR,
      "{label}: recall regressed below the measured floor"
    );
  }

  println!("== misses on scored classes (recognizer hypotheses) ==");
  let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
  for row in &rows {
    if !SCORED.contains(&row.gold.intent.as_str()) || !expressible(&row.gold) {
      continue;
    }
    if score(&row.utterance, &row.gold) == Outcome::Correct {
      continue;
    }
    *counts.entry(row.gold.intent.as_str()).or_default() += 1;
    let blame = if score(&row.reference, &row.gold) == Outcome::Correct {
      "asr"
    } else {
      "rules"
    };
    println!("  [{blame}] gold={} {:?}", row.gold.intent, row.utterance);
  }
  println!("  by intent: {counts:?}");
}
