mod connectivity;
mod fnv;
mod geo;
mod image;
mod media;
pub mod nlu;
mod speech;

use std::{path::Path, sync::Arc};

use bridgething_companion::api::ModelPlatform;

use crate::backends::{ModelPaths, Platform, asr, geo::Locator, models};

pub fn platform(config_dir: &Path) -> Platform {
  let paths = ModelPaths::default();
  Platform {
    geo: Some(Arc::new(Locator::new(geo::run))),
    notifications: None,
    media_sessions: Some(Arc::new(media::MediaRemoteSessions::new(config_dir))),
    audio: Some(Arc::new(speech::AvAudio::new())),
    connectivity: Some(Arc::new(connectivity::NwPathConnectivity::default())),
    image: Some(Arc::new(image::ImageIoScaler)),
    speech: Some(Arc::new(asr::WhisperSpeech::new(paths.clone()))),
    nlu: Some(Arc::new(nlu::CoreMlNlu::new(paths.clone()))),
    model_validator: Some(Arc::new(models::DesktopArtifactValidator)),
    model_platform: Some(ModelPlatform::Macos),
    models: paths,
  }
}

#[cfg(test)]
mod tests {
  use bridgething_companion::{
    api::VoiceModelPaths,
    backend::{ModelArtifactKind, ModelArtifactValidator, PrepareSink, SpeechRecognizer, TranscriptionSink},
    voice::{
      inference::{BundleInference, NluInference},
      rejection::{RejectionOutcome, evaluate},
    },
  };

  use super::*;
  use crate::backends::{
    models::DesktopArtifactValidator,
    probe::{armed, fixtures},
  };

  fn spoken() -> Vec<f32> {
    let wav = std::fs::read(fixtures().join("speech.wav")).expect("a spoken wav fixture");
    let data = wav
      .windows(4)
      .position(|window| window == b"data")
      .expect("a data chunk in the wav")
      + 8;
    wav[data..]
      .chunks_exact(2)
      .map(|sample| f32::from(i16::from_le_bytes([sample[0], sample[1]])) / f32::from(i16::MAX))
      .collect()
  }

  #[test]
  #[ignore = "needs BRIDGETHING_VOICE_FIXTURES pointing at fetched model artifacts"]
  fn the_validator_takes_a_published_bundle_and_refuses_bytes_that_are_not_one() {
    let validator = DesktopArtifactValidator;

    validator
      .validate(
        ModelArtifactKind::NluModel,
        fixtures().join("bundle").display().to_string(),
      )
      .expect("the published bundle compiles");
    validator
      .validate(
        ModelArtifactKind::AsrModel,
        fixtures().join("model.bin").display().to_string(),
      )
      .expect("the published weights open with a ggml header");

    let shredded = tempfile::tempdir().expect("a scratch directory");
    std::fs::write(shredded.path().join("model.bin"), b"not a model").expect("wrote the decoy");
    validator
      .validate(
        ModelArtifactKind::AsrModel,
        shredded.path().join("model.bin").display().to_string(),
      )
      .expect_err("a file with no ggml header is not weights");
  }

  #[tokio::test]
  #[ignore = "needs BRIDGETHING_VOICE_FIXTURES pointing at fetched model artifacts"]
  async fn a_published_bundle_answers_a_typed_utterance_with_an_intent() {
    let bundle = fixtures().join("bundle");
    let runner = Arc::new(nlu::CoreMlNlu::new(armed(VoiceModelPaths {
      nlu_bundle_dir: Some(bundle.display().to_string()),
      asr_weights: None,
    })));

    let inference = BundleInference::load(&bundle, runner).expect("the bundle loads");
    let policy = inference.rejection().unwrap_or_default();
    let output = inference.infer("pause the music").await.expect("the model answers");

    assert_eq!(
      evaluate(&output, policy).expect("the intent head matches the catalog"),
      RejectionOutcome::Accept { intent: "PAUSE" }
    );
  }

  #[tokio::test]
  #[ignore = "needs BRIDGETHING_VOICE_FIXTURES pointing at fetched model artifacts"]
  async fn spoken_audio_reaches_an_intent_through_whisper_and_coreml() {
    let recognizer = asr::WhisperSpeech::new(armed(VoiceModelPaths {
      nlu_bundle_dir: None,
      asr_weights: Some(fixtures().join("model.bin").display().to_string()),
    }));

    let (sink, mut ready) = PrepareSink::channel();
    recognizer.prepare(sink);
    assert!(
      matches!(
        ready.recv().await,
        Some(bridgething_companion::backend::PrepareEvent::Ready)
      ),
      "the recognizer arms against the published weights"
    );

    let (sink, spoken_back) = TranscriptionSink::channel();
    recognizer.transcribe(spoken(), 16_000, sink);
    let transcription = spoken_back.await.expect("the sink settles").expect("a transcription");
    let heard = transcription.text.to_lowercase();
    assert!(heard.contains("pause"), "whisper heard {heard:?}");

    let bundle = fixtures().join("bundle");
    let runner = Arc::new(nlu::CoreMlNlu::new(armed(VoiceModelPaths {
      nlu_bundle_dir: Some(bundle.display().to_string()),
      asr_weights: None,
    })));
    let inference = BundleInference::load(&bundle, runner).expect("the bundle loads");
    let policy = inference.rejection().unwrap_or_default();
    let output = inference.infer(&transcription.text).await.expect("the model answers");

    assert_eq!(
      evaluate(&output, policy).expect("the intent head matches the catalog"),
      RejectionOutcome::Accept { intent: "PAUSE" }
    );
  }
}
