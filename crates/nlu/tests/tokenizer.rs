use std::path::Path;

use nlu::tokenize::TranscriptTokenizer;

fn tokenizer() -> TranscriptTokenizer {
  let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bundle/tokenizer.json");
  assert!(
    path.exists(),
    "no {}; run scripts/bridgething-fetch-nlu-bundle",
    path.display()
  );
  TranscriptTokenizer::load(&path, 64).expect("tokenizer loads")
}

fn slice_chars(text: &str, start: u32, end: u32) -> String {
  text.chars().skip(start as usize).take((end - start) as usize).collect()
}

#[test]
fn offsets_are_char_positions_into_the_transcript() {
  let tok = tokenizer();
  let transcript = "play héroes by beyoncé now";
  let encoded = tok.encode(transcript).unwrap();

  let mut rebuilt = String::new();
  for i in 0..encoded.input_ids.len() {
    if encoded.attention_mask[i] == 0 || encoded.offset_ends[i] <= encoded.offset_starts[i] {
      continue;
    }
    rebuilt.push_str(&slice_chars(
      transcript,
      encoded.offset_starts[i],
      encoded.offset_ends[i],
    ));
  }
  let squashed: String = rebuilt.chars().filter(|c| !c.is_whitespace()).collect();
  let expected: String = transcript.chars().filter(|c| !c.is_whitespace()).collect();
  assert_eq!(squashed, expected, "token offsets must reassemble the transcript");
}

#[test]
fn encoding_is_fixed_length_with_a_padded_tail() {
  let tok = tokenizer();
  let encoded = tok.encode("pause").unwrap();
  assert_eq!(encoded.input_ids.len(), 64);
  assert_eq!(encoded.attention_mask.len(), 64);
  assert_eq!(encoded.offset_starts.len(), 64);
  assert_eq!(*encoded.attention_mask.last().unwrap(), 0);
  assert!(
    encoded.attention_mask.iter().sum::<i32>() >= 3,
    "cls + token + sep at minimum"
  );
}
