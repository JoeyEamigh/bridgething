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

fn speech_only(transcription: Transcription) -> Transcription {
  Transcription {
    text: spoken_words(&transcription.text),
    alternatives: transcription
      .alternatives
      .iter()
      .map(|alternative| spoken_words(alternative))
      .filter(|alternative| !alternative.is_empty())
      .collect(),
    segments: transcription
      .segments
      .into_iter()
      .filter_map(|segment| {
        let text = spoken_words(&segment.text);
        (!text.is_empty()).then_some(SpeechSegment { text, ..segment })
      })
      .collect(),
    confidence: transcription.confidence,
  }
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
}
