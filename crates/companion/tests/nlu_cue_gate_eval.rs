use std::{collections::BTreeMap, env, fs, path::Path, sync::Arc};

use bridgething_companion::voice::{
  controller::{VoiceController, VoiceControllerConfig},
  fast_path,
  inference::{InferError, InferenceOutput, NluInference},
  intent_catalog,
  rejection::{self, RejectionOutcome, RejectionPolicy},
  slot_mapping,
};
use libbridgething::NluStage;
use nlu::SlotValue;
use serde::Deserialize;
use serde_json::Value;

const GATED: &[&str] = &["PLAY", "PHONE_ACTION"];
const PLAY_ANCHOR_SLOTS: &[&str] = &["target_type"];

#[derive(Deserialize)]
struct Gold {
  intent: String,
  #[serde(default)]
  slots: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct GoldRow {
  id: String,
  utterance: String,
  gold: Gold,
}

#[derive(Deserialize)]
struct Prediction {
  intent: String,
  #[serde(default)]
  slots: BTreeMap<String, Value>,
  margin: f64,
  ood_prob: f64,
}

#[derive(Deserialize)]
struct PredRow {
  id: String,
  pred: Prediction,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestRejection {
  in_domain_threshold: f64,
  clarify_margin: f64,
}

#[derive(Deserialize)]
struct Manifest {
  rejection: ManifestRejection,
}

struct Replay(InferenceOutput);

#[async_trait::async_trait]
impl NluInference for Replay {
  async fn prewarm(&self) {}

  async fn infer(&self, _transcript: &str) -> Result<InferenceOutput, InferError> {
    Ok(self.0.clone())
  }
}

fn logit(probability: f64) -> f64 {
  let clamped = probability.clamp(1e-9, 1.0 - 1e-9);
  (clamped / (1.0 - clamped)).ln()
}

fn replayed(prediction: &Prediction) -> InferenceOutput {
  let names = intent_catalog::SURFACE_NAMES;
  let top = names
    .iter()
    .position(|name| *name == prediction.intent)
    .unwrap_or_else(|| panic!("prediction names an intent outside the catalog: {}", prediction.intent));
  let runner_up = (top + 1) % names.len();
  let margin = prediction.margin.clamp(0.0, 1.0 - 1e-9);
  let mut intent_logits = vec![-60.0; names.len()];
  intent_logits[top] = 0.0;
  intent_logits[runner_up] = ((1.0 - margin) / (1.0 + margin)).ln();
  let slots: Vec<SlotValue> = prediction
    .slots
    .iter()
    .map(|(name, value)| SlotValue {
      name: name.clone(),
      value: match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_owned(),
        other => other.to_string(),
      },
    })
    .collect();
  InferenceOutput {
    intent_logits,
    in_domain_logit: logit(1.0 - prediction.ood_prob),
    slots: slot_mapping::apply(&slots),
  }
}

fn agrees(intent: &str, gold: &Gold) -> bool {
  match intent {
    "PLAY" => gold.intent == "PLAY" && gold.slots.keys().all(|key| PLAY_ANCHOR_SLOTS.contains(&key.as_str())),
    other => gold.intent == other,
  }
}

fn read_lines<T: for<'de> Deserialize<'de>>(path: &str) -> Vec<T> {
  fs::read_to_string(path)
    .unwrap_or_else(|error| panic!("{path}: {error}"))
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{path}: {error}")))
    .collect()
}

#[derive(Default)]
struct Tally {
  accepted: usize,
  wrong_before: usize,
  refused_wrong: usize,
  refused_right: usize,
}

#[tokio::test]
async fn the_cue_gate_refuses_model_guesses_without_refusing_real_commands() {
  let (Ok(gold_path), Ok(preds_path), Ok(bundle)) = (
    env::var("BRIDGETHING_ASR_EVAL"),
    env::var("BRIDGETHING_NLU_PREDS"),
    env::var("BRIDGETHING_NLU_BUNDLE"),
  ) else {
    eprintln!(
      "BRIDGETHING_ASR_EVAL, BRIDGETHING_NLU_PREDS or BRIDGETHING_NLU_BUNDLE unset; skipping the cue gate eval"
    );
    return;
  };
  let manifest: Manifest =
    serde_json::from_str(&fs::read_to_string(Path::new(&bundle).join("manifest.json")).expect("bundle manifest.json"))
      .expect("bundle manifest parses");
  let policy = RejectionPolicy {
    in_domain_threshold: manifest.rejection.in_domain_threshold,
    clarify_margin: manifest.rejection.clarify_margin,
    ..RejectionPolicy::default()
  };
  let predictions: BTreeMap<String, Prediction> = read_lines::<PredRow>(&preds_path)
    .into_iter()
    .map(|row| (row.id, row.pred))
    .collect();
  let rows: Vec<GoldRow> = read_lines(&gold_path);

  let mut tallies: BTreeMap<&str, Tally> = BTreeMap::new();
  let mut refusals = Vec::new();
  for row in &rows {
    let Some(prediction) = predictions.get(&row.id) else {
      continue;
    };
    if fast_path::match_transcript(&row.utterance).is_some() {
      continue;
    }
    let output = replayed(prediction);
    let RejectionOutcome::Accept { intent } = rejection::evaluate(&output, policy).expect("catalog-shaped head") else {
      continue;
    };
    let Some(&gated) = GATED.iter().find(|name| **name == intent) else {
      continue;
    };
    if gated == "PLAY" && output.slots.has_catalog_slots() {
      continue;
    }
    let controller = VoiceController::new(
      Some(Arc::new(Replay(output))),
      VoiceControllerConfig {
        rejection: policy,
        ..VoiceControllerConfig::default()
      },
    );
    let resolution = controller.resolve(&row.utterance).await.expect("replayed inference");
    let right = agrees(gated, &row.gold);
    let tally = tallies.entry(gated).or_default();
    tally.accepted += 1;
    tally.wrong_before += usize::from(!right);
    if resolution.stage == NluStage::RejectedNoIntent {
      if right {
        tally.refused_right += 1;
      } else {
        tally.refused_wrong += 1;
      }
      refusals.push((gated, right, row.gold.intent.clone(), row.utterance.clone()));
    }
  }

  println!("model acceptances the cue gate can touch (fast path misses only)");
  println!("  intent         accepted  wrong before  wrong after  refused wrong  refused right");
  for (intent, tally) in &tallies {
    println!(
      "  {intent:<14} {:>8}  {:>12}  {:>11}  {:>13}  {:>13}",
      tally.accepted,
      tally.wrong_before,
      tally.wrong_before - tally.refused_wrong,
      tally.refused_wrong,
      tally.refused_right
    );
  }
  println!("\nrefusals");
  for (intent, right, gold, utterance) in &refusals {
    let verdict = if *right { "FALSE REFUSAL" } else { "wrong fire removed" };
    println!("  {intent:<12} gold={gold:<14} {verdict:<18} {utterance:?}");
  }

  let false_refusals: usize = tallies.values().map(|tally| tally.refused_right).sum();
  assert_eq!(
    false_refusals, 0,
    "the cue gate refused {false_refusals} model readings gold agrees with"
  );
}
