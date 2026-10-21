use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
  utterance: String,
  input_ids: Vec<i32>,
  attention_mask: Vec<i32>,
  offset_starts: Vec<u32>,
  offset_ends: Vec<u32>,
  intent_logits: Vec<f32>,
  bio_logits: Vec<f32>,
  closed_logits: Vec<Vec<f32>>,
  expected: Expected,
}

#[derive(Deserialize)]
struct Expected {
  intent: String,
  slots: BTreeMap<String, String>,
}

fn bundle() -> PathBuf {
  let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bundle");
  assert!(
    dir.join("tokenizer.json").exists(),
    "no tokenizer.json in {}; run scripts/bridgething-fetch-nlu-bundle",
    dir.display()
  );
  dir
}

fn fixtures(dir: &Path) -> Vec<Fixture> {
  std::fs::read_to_string(dir.join("fixtures.jsonl"))
    .expect("fixtures.jsonl in bundle dir")
    .lines()
    .filter(|l| !l.trim().is_empty())
    .map(|l| serde_json::from_str(l).expect("fixture row parses"))
    .collect()
}

#[test]
fn tokenization_matches_the_training_side() {
  let dir = bundle();
  let decoder = nlu::NluDecoder::load(&dir).expect("bundle loads");
  for fixture in fixtures(&dir) {
    let tokens = decoder.tokenize(&fixture.utterance).expect("tokenizes");
    assert_eq!(tokens.input_ids, fixture.input_ids, "ids for {:?}", fixture.utterance);
    assert_eq!(
      tokens.attention_mask, fixture.attention_mask,
      "mask for {:?}",
      fixture.utterance
    );
    assert_eq!(
      tokens.offset_starts, fixture.offset_starts,
      "offset starts for {:?}",
      fixture.utterance
    );
    assert_eq!(
      tokens.offset_ends, fixture.offset_ends,
      "offset ends for {:?}",
      fixture.utterance
    );
  }
}

#[test]
fn decode_matches_the_trainers_own_frames() {
  let dir = bundle();
  let decoder = nlu::NluDecoder::load(&dir).expect("bundle loads");
  for fixture in fixtures(&dir) {
    let tokens = decoder.tokenize(&fixture.utterance).expect("tokenizes");
    let frame = decoder
      .decode(
        &fixture.utterance,
        &tokens,
        &fixture.intent_logits,
        &fixture.bio_logits,
        &fixture.closed_logits,
      )
      .expect("decodes");
    assert_eq!(
      frame.intent, fixture.expected.intent,
      "intent for {:?}",
      fixture.utterance
    );
    let got: BTreeMap<String, String> = frame.slots.into_iter().map(|s| (s.name, s.value)).collect();
    assert_eq!(got, fixture.expected.slots, "slots for {:?}", fixture.utterance);
  }
}
