use bridgething_companion::voice::fast_path;
use libbridgething::{NluRepeatMode, NluSlots, NluView};

fn intent(transcript: &str) -> Option<&'static str> {
  fast_path::match_transcript(transcript).map(|hit| hit.intent)
}

fn slots(transcript: &str) -> Option<NluSlots> {
  fast_path::match_transcript(transcript).map(|hit| hit.slots)
}

#[test]
fn bare_transport_commands_match() {
  assert_eq!(intent("play"), Some("PLAY"));
  assert_eq!(intent("pause"), Some("PAUSE"));
  assert_eq!(intent("next song"), Some("NEXT"));
  assert_eq!(intent("what's playing"), Some("SHOW_VIEW"));
}

#[test]
fn never_fires_on_a_command_carrying_content() {
  for utterance in [
    "play some jazz",
    "play bohemian rhapsody",
    "play the new album by black country new road",
    "play my liked songs",
    "add this to my dance playlist",
    "what album is this from",
  ] {
    assert_eq!(intent(utterance), None, "fast path must not claim: {utterance}");
  }
}

#[test]
fn preset_selection_captures_the_number() {
  let hit = fast_path::match_transcript("play preset 3").expect("preset 3 matches");
  assert_eq!(hit.intent, "PRESET_PLAY");
  assert_eq!(hit.slots.preset.as_deref(), Some("3"));
}

#[test]
fn preset_rejects_out_of_range_and_routes_save_phrasings() {
  assert_eq!(intent("play preset 7"), None);
  assert_eq!(intent("save preset 2"), Some("PRESET_SAVE"));
}

#[test]
fn rule_order_keeps_overlapping_repeat_phrasings_distinct() {
  assert_eq!(intent("repeat this"), Some("SET_REPEAT"));
  assert_eq!(
    slots("repeat this").and_then(|s| s.repeat_mode),
    Some(NluRepeatMode::One)
  );
  assert_eq!(slots("repeat on").and_then(|s| s.repeat_mode), Some(NluRepeatMode::All));
  assert_eq!(
    slots("repeat off").and_then(|s| s.repeat_mode),
    Some(NluRepeatMode::Off)
  );
}

#[test]
fn unhandled_phrasings_fall_through_instead_of_guessing() {
  assert_eq!(intent("repeat one"), None);
}

#[test]
fn matches_raw_recogniser_output_without_pre_normalisation() {
  assert_eq!(intent("Pause."), Some("PAUSE"));
  assert_eq!(intent("Next song."), Some("NEXT"));
  assert_eq!(intent("What's playing?"), Some("SHOW_VIEW"));
}

#[test]
fn politeness_determiners_and_generic_nouns_do_not_hide_the_command() {
  for (utterance, expected) in [
    ("Pause music now.", "PAUSE"),
    ("Could you pause the song?", "PAUSE"),
    ("Stop the song from playing.", "PAUSE"),
    ("Turn off music.", "PAUSE"),
    ("End this track.", "PAUSE"),
    ("Skip the track.", "NEXT"),
    ("Can you skip this song?", "NEXT"),
    ("Skip to the next track.", "NEXT"),
    ("Would you go to the next song please?", "NEXT"),
    ("Go back to previous song.", "PREVIOUS"),
    ("Please play the previous song.", "PREVIOUS"),
    ("Replay the last song.", "PREVIOUS"),
    ("Repeat the last song.", "PREVIOUS"),
    ("shuffle the tracks", "SET_SHUFFLE"),
    ("Put this playlist on shuffle.", "SET_SHUFFLE"),
  ] {
    assert_eq!(
      intent(utterance),
      Some(expected),
      "expected {expected} for: {utterance}"
    );
  }
}

#[test]
fn repeat_scope_survives_the_generic_noun_strip() {
  assert_eq!(
    slots("Put song on repeat for me.").and_then(|s| s.repeat_mode),
    Some(NluRepeatMode::One)
  );
  assert_eq!(
    slots("Could you play this song in loop?").and_then(|s| s.repeat_mode),
    Some(NluRepeatMode::One)
  );
  assert_eq!(
    slots("Repeat this playlist indefinitely.").and_then(|s| s.repeat_mode),
    Some(NluRepeatMode::All)
  );
}

#[test]
fn declines_anything_carrying_content_a_scope_or_a_second_setting() {
  for utterance in [
    "Play Pandora on Shuffle for us.",
    "Play more music like this.",
    "Skip the next two songs.",
    "Skip to track 20.",
    "Shuffle for the next five songs.",
    "Play a list of my favorite songs.",
    "Make a playlist with my most listened to tracks.",
    "Exit Spotify",
  ] {
    assert_eq!(intent(utterance), None, "fast path must not claim: {utterance}");
  }
}

#[test]
fn a_collection_word_blocks_the_transport_rules_the_generic_strip_would_blind() {
  for utterance in [
    "Go to the next playlist.",
    "Play my last playlist.",
    "shuffle play this playlist.",
    "next album",
    "start the next station",
  ] {
    assert_eq!(intent(utterance), None, "fast path must not claim: {utterance}");
  }
  assert_eq!(intent("skip this song"), Some("NEXT"));
}

#[test]
fn mute_folds_into_set_volume_and_whats_playing_into_show_view() {
  let mute = fast_path::match_transcript("mute").expect("mute matches");
  assert_eq!(mute.intent, "SET_VOLUME");
  assert_eq!(mute.slots.mute, Some(true));

  let unmute = fast_path::match_transcript("unmute").expect("unmute matches");
  assert_eq!(unmute.intent, "SET_VOLUME");
  assert_eq!(unmute.slots.mute, Some(false));

  let whats = fast_path::match_transcript("what's playing").expect("whats playing matches");
  assert_eq!(whats.intent, "SHOW_VIEW");
  assert_eq!(whats.slots.view, Some(NluView::NowPlaying));
}

#[test]
fn an_empty_or_filler_only_transcript_matches_nothing() {
  for utterance in ["", "   ", "uh um hmm", "..."] {
    assert_eq!(intent(utterance), None, "fast path must not claim: {utterance:?}");
  }
}

#[test]
fn an_underscore_splits_words_rather_than_welding_them() {
  assert_eq!(intent("next_song"), Some("NEXT"));
  assert_eq!(intent("what's_playing"), Some("SHOW_VIEW"));
}
