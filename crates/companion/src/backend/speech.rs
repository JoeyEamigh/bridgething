use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SpeechSegment {
  pub text: String,
  pub start_ms: u64,
  pub end_ms: u64,
  pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Transcription {
  pub text: String,
  pub alternatives: Vec<String>,
  pub segments: Vec<SpeechSegment>,
  pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareEvent {
  Progress { received: u64, total: u64 },
  Ready,
  Failed { reason: String },
}

#[uniffi::export(with_foreign)]
pub trait SpeechRecognizer: Send + Sync {
  fn prepare(&self, sink: Arc<PrepareSink>);
  fn transcribe(&self, pcm: Vec<f32>, sample_rate_hz: u32, sink: Arc<TranscriptionSink>);
}

#[derive(uniffi::Object)]
pub struct PrepareSink {
  tx: mpsc::UnboundedSender<PrepareEvent>,
}

impl PrepareSink {
  pub fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<PrepareEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(Self { tx }), rx)
  }
}

#[uniffi::export]
impl PrepareSink {
  pub fn on_progress(&self, received: u64, total: u64) {
    let _ = self.tx.send(PrepareEvent::Progress { received, total });
  }

  pub fn on_ready(&self) {
    let _ = self.tx.send(PrepareEvent::Ready);
  }

  pub fn on_failed(&self, reason: String) {
    let _ = self.tx.send(PrepareEvent::Failed { reason });
  }
}

#[derive(uniffi::Object)]
pub struct TranscriptionSink {
  tx: std::sync::Mutex<Option<oneshot::Sender<Result<Transcription, String>>>>,
}

impl TranscriptionSink {
  pub fn channel() -> (Arc<Self>, oneshot::Receiver<Result<Transcription, String>>) {
    let (tx, rx) = oneshot::channel();
    (
      Arc::new(Self {
        tx: std::sync::Mutex::new(Some(tx)),
      }),
      rx,
    )
  }

  fn settle(&self, result: Result<Transcription, String>) {
    if let Some(tx) = self.tx.lock().unwrap().take() {
      let _ = tx.send(result);
    }
  }
}

#[uniffi::export]
impl TranscriptionSink {
  pub fn complete(&self, transcription: Transcription) {
    self.settle(Ok(speech_only(transcription)));
  }

  pub fn fail(&self, reason: String) {
    self.settle(Err(reason));
  }
}

const WAKE_PHRASE: &str = "bridgething";
const GREETINGS: &[&str] = &["hey", "hay", "hi", "ok", "okay"];

fn speech_only(transcription: Transcription) -> Transcription {
  Transcription {
    text: without_wake_word(&spoken_words(&transcription.text)),
    alternatives: transcription
      .alternatives
      .iter()
      .map(|alternative| without_wake_word(&spoken_words(alternative)))
      .filter(|alternative| !alternative.is_empty())
      .collect(),
    segments: segments_without_wake_word(
      transcription
        .segments
        .into_iter()
        .filter_map(|segment| {
          let text = spoken_words(&segment.text);
          (!text.is_empty()).then_some(SpeechSegment { text, ..segment })
        })
        .collect(),
    ),
    confidence: transcription.confidence,
  }
}

fn without_wake_word(text: &str) -> String {
  let words: Vec<&str> = text.split(' ').filter(|word| !word.is_empty()).collect();
  words[wake_prefix(&words)..].join(" ")
}

fn segments_without_wake_word(segments: Vec<SpeechSegment>) -> Vec<SpeechSegment> {
  let words: Vec<&str> = segments.iter().flat_map(|segment| segment.text.split(' ')).collect();
  let mut dropping = wake_prefix(&words);
  segments
    .into_iter()
    .filter_map(|segment| {
      let count = segment.text.split(' ').count();
      let skip = dropping.min(count);
      dropping -= skip;
      let text = segment.text.split(' ').skip(skip).collect::<Vec<&str>>().join(" ");
      (!text.is_empty()).then_some(SpeechSegment { text, ..segment })
    })
    .collect()
}

fn wake_prefix(words: &[&str]) -> usize {
  let mut at = 0;
  loop {
    let greeted = words
      .get(at)
      .is_some_and(|word| GREETINGS.contains(&letters(word).as_str()));
    let from = at + usize::from(greeted);
    let single = words.get(from).map(|word| letters(word));
    let joined = words
      .get(from + 1)
      .and_then(|next| single.as_ref().map(|first| format!("{first}{}", letters(next))));
    let spoken = if single.as_deref().is_some_and(|word| sounds_like_wake(word, greeted)) {
      1
    } else if joined.as_deref().is_some_and(|word| sounds_like_wake(word, greeted)) {
      2
    } else {
      return at;
    };
    at = from + spoken;
  }
}

fn letters(word: &str) -> String {
  word
    .chars()
    .filter(char::is_ascii_alphabetic)
    .map(|letter| letter.to_ascii_lowercase())
    .collect()
}

fn sounds_like_wake(word: &str, greeted: bool) -> bool {
  let heard = phonetic_key(word);
  let wake = phonetic_key(WAKE_PHRASE);
  let (onset, slack) = if greeted { (&wake[..2], 3) } else { (&wake[..4], 2) };
  heard.len() >= 6 && heard.starts_with(onset) && edit_distance(heard.as_bytes(), wake.as_bytes()) <= slack
}

fn phonetic_key(word: &str) -> String {
  let spelled = word.replace("dge", "j").replace("dg", "j");
  let mut sounds = String::with_capacity(spelled.len());
  let mut letters = spelled.chars().peekable();
  while let Some(letter) = letters.next() {
    let next = letters.peek().copied();
    let sound = match (letter, next) {
      ('g', Some('e' | 'i' | 'y')) => 'j',
      ('t' | 'p' | 'c', Some('h')) => {
        letters.next();
        match letter {
          't' => 't',
          'p' => 'f',
          _ => 'j',
        }
      }
      ('c' | 'q', _) => 'k',
      ('z', _) => 's',
      ('a' | 'e' | 'i' | 'o' | 'u' | 'y', _) => 'a',
      _ => letter,
    };
    if !sounds.ends_with(sound) {
      sounds.push(sound);
    }
  }
  if sounds.len() > 3 && sounds.ends_with('a') {
    sounds.pop();
  }
  sounds
}

fn edit_distance(left: &[u8], right: &[u8]) -> usize {
  let mut row: Vec<usize> = (0..=right.len()).collect();
  for (i, a) in left.iter().enumerate() {
    let mut diagonal = row[0];
    row[0] = i + 1;
    for (j, b) in right.iter().enumerate() {
      let above = row[j + 1];
      row[j + 1] = (diagonal + usize::from(a != b)).min(row[j] + 1).min(above + 1);
      diagonal = above;
    }
  }
  row[right.len()]
}

fn spoken_words(text: &str) -> String {
  let mut kept = String::with_capacity(text.len());
  let mut depth = 0usize;
  let mut starred = false;
  for character in text.chars() {
    match character {
      '[' | '(' | '<' => depth += 1,
      ']' | ')' | '>' => depth = depth.saturating_sub(1),
      '*' => starred = !starred,
      '\u{266a}' | '\u{266b}' | '\u{266c}' => {}
      _ if depth == 0 && !starred => kept.push(character),
      _ => {}
    }
  }
  kept.split_whitespace().collect::<Vec<&str>>().join(" ")
}

#[cfg(test)]
mod tests {
  use super::*;

  fn heard(text: &str) -> Transcription {
    Transcription {
      text: text.to_owned(),
      alternatives: Vec::new(),
      segments: vec![SpeechSegment {
        text: text.to_owned(),
        start_ms: 0,
        end_ms: 600,
        confidence: Some(0.4),
      }],
      confidence: Some(0.4),
    }
  }

  #[test]
  fn silence_narrated_as_an_annotation_leaves_nothing_to_act_on() {
    for narration in [
      "[BLANK_AUDIO]",
      "[ Silence ]",
      "(upbeat music)",
      "\u{266a}\u{266a}\u{266a}",
      "*laughs*",
      "<|nospeech|>",
      "[MUSIC",
    ] {
      let scrubbed = speech_only(heard(narration));
      assert!(
        scrubbed.text.is_empty(),
        "{narration} survived the scrub as {:?}",
        scrubbed.text
      );
      assert!(scrubbed.segments.is_empty(), "{narration} left a segment behind");
    }
  }

  #[test]
  fn an_annotation_alongside_a_command_keeps_the_command() {
    let scrubbed = speech_only(heard("[BLANK_AUDIO] play blank space"));
    assert_eq!(scrubbed.text, "play blank space");
    assert_eq!(scrubbed.segments.len(), 1);
    assert_eq!(scrubbed.segments[0].confidence, Some(0.4));
  }

  #[test]
  fn a_plain_command_is_untouched() {
    assert_eq!(speech_only(heard("skip this song")).text, "skip this song");
  }

  #[test]
  fn the_wake_word_in_front_of_a_command_is_dropped() {
    for (heard_as, command) in [
      ("Hey, Bridgestane, play all my love.", "play all my love."),
      ("Hey Bridgething resume", "resume"),
      ("hey bridgething next song", "next song"),
      ("Hey bridge thing, pause.", "pause."),
      ("OK Bridgething stop", "stop"),
      ("okay bridging pause", "pause"),
      ("Bridgething, skip this song", "skip this song"),
      ("Hey Bridgething. Hey Bridgething, pause.", "pause."),
      ("hey brigething what's playing", "what's playing"),
    ] {
      assert_eq!(speech_only(heard(heard_as)).text, command, "{heard_as}");
    }
  }

  #[test]
  fn a_wake_word_alone_leaves_nothing_to_act_on() {
    for wake in [
      "Hey Bridgething.",
      "hey bridgestone",
      "Bridgething",
      "Hey, Bridge Thing!",
    ] {
      let scrubbed = speech_only(heard(wake));
      assert!(scrubbed.text.is_empty(), "{wake} survived as {:?}", scrubbed.text);
      assert!(scrubbed.segments.is_empty(), "{wake} left a segment behind");
    }
  }

  #[test]
  fn content_that_only_resembles_the_wake_word_is_untouched() {
    for content in [
      "play hey jude",
      "play the bridge",
      "hey jude",
      "Bridge over troubled water",
      "Breathing by Ariana Grande",
      "Bridget Jones soundtrack",
      "hey there delilah",
      "play bridgething",
    ] {
      assert_eq!(speech_only(heard(content)).text, content);
    }
  }

  #[test]
  fn the_wake_word_is_dropped_across_segment_boundaries() {
    let scrubbed = speech_only(Transcription {
      text: "Hey Bridgething pause".to_owned(),
      alternatives: vec!["hey bridge thing pause".to_owned(), "hey bridgething".to_owned()],
      segments: vec![
        SpeechSegment {
          text: "Hey".to_owned(),
          start_ms: 0,
          end_ms: 200,
          confidence: Some(0.9),
        },
        SpeechSegment {
          text: "Bridgething pause".to_owned(),
          start_ms: 200,
          end_ms: 900,
          confidence: Some(0.7),
        },
      ],
      confidence: Some(0.8),
    });
    assert_eq!(scrubbed.text, "pause");
    assert_eq!(scrubbed.alternatives, vec!["pause".to_owned()]);
    assert_eq!(scrubbed.segments.len(), 1);
    assert_eq!(scrubbed.segments[0].text, "pause");
    assert_eq!(scrubbed.segments[0].start_ms, 200);
  }
}
