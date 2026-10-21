use std::{collections::BTreeSet, path::Path};

use bridgething_companion::voice::intent_catalog;
use nlu::Manifest;

#[test]
fn catalog_matches_the_published_bundle_manifest() {
  let manifest = Manifest::load(Path::new(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../nlu/tests/bundle/manifest.json"
  )))
  .expect("the published bundle manifest loads");
  let intents: Vec<&str> = manifest.intents.iter().map(|intent| intent.name.as_str()).collect();

  assert_eq!(intents, intent_catalog::SURFACE_NAMES.to_vec());
}

#[test]
fn label_indices_are_unique_and_stable() {
  let names = intent_catalog::SURFACE_NAMES;
  let unique: BTreeSet<&str> = names.iter().copied().collect();
  assert_eq!(
    unique.len(),
    names.len(),
    "duplicate intent names would collide label indices"
  );

  let mut sorted = names.to_vec();
  sorted.sort_unstable();
  assert_eq!(
    sorted,
    names.to_vec(),
    "label order must stay alphabetical to match the exported head"
  );

  assert_eq!(intent_catalog::name_at(0), names.first().copied());
  assert_eq!(intent_catalog::name_at(names.len() - 1), names.last().copied());
  assert_eq!(intent_catalog::name_at(names.len()), None);
}

#[test]
fn rejection_wire_values_are_not_model_classes() {
  assert!(!intent_catalog::contains(intent_catalog::NO_INTENT));
  assert!(!intent_catalog::contains(intent_catalog::CLARIFY));
}

#[test]
fn every_catalog_name_round_trips_through_lookup() {
  for (index, name) in intent_catalog::SURFACE_NAMES.iter().enumerate() {
    assert_eq!(intent_catalog::name_at(index), Some(*name));
    assert!(intent_catalog::contains(name));
  }
  assert!(!intent_catalog::contains("play"));
  assert!(!intent_catalog::contains(""));
}
