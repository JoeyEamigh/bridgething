use std::sync::{Arc, Mutex};

use libbridgething::{NluAlternate, NluResolvedIntent, NluSlots, NluStage};

use crate::voice::{
  fast_path,
  inference::NluInference,
  intent_catalog,
  rejection::{self, RejectionError, RejectionOutcome, RejectionPolicy},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceControllerConfig {
  pub use_fast_path: bool,
  pub rejection: RejectionPolicy,
}

impl Default for VoiceControllerConfig {
  fn default() -> Self {
    Self {
      use_fast_path: true,
      rejection: RejectionPolicy::default(),
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
  pub resolved: NluResolvedIntent,
  pub stage: NluStage,
}

#[derive(Debug, thiserror::Error)]
pub enum ControllerError {
  #[error("nlu inference failed: {0}")]
  InferenceFailed(String),
  #[error(transparent)]
  Rejection(#[from] RejectionError),
}

pub struct ArmedModel {
  pub client: Arc<dyn NluInference>,
  pub bundle: Option<String>,
  pub rejection: Option<RejectionPolicy>,
}

struct Model {
  client: Arc<dyn NluInference>,
  bundle: Option<String>,
  rejection: RejectionPolicy,
}

pub struct VoiceController {
  model: Mutex<Option<Model>>,
  config: VoiceControllerConfig,
}

impl VoiceController {
  pub fn new(client: Option<Arc<dyn NluInference>>, config: VoiceControllerConfig) -> Self {
    let controller = Self {
      model: Mutex::new(None),
      config,
    };
    controller.set_model(client.map(|client| ArmedModel {
      client,
      bundle: None,
      rejection: None,
    }));
    controller
  }

  pub fn set_model(&self, model: Option<ArmedModel>) {
    *self.model.lock().unwrap() = model.map(|model| Model {
      client: model.client,
      bundle: model.bundle,
      rejection: model.rejection.unwrap_or(self.config.rejection),
    });
  }

  pub fn has_model(&self) -> bool {
    self.model.lock().unwrap().is_some()
  }

  pub fn armed_bundle(&self) -> Option<String> {
    self
      .model
      .lock()
      .unwrap()
      .as_ref()
      .and_then(|model| model.bundle.clone())
  }

  fn armed(&self) -> Option<(Arc<dyn NluInference>, RejectionPolicy)> {
    let held = self.model.lock().unwrap();
    held.as_ref().map(|model| (model.client.clone(), model.rejection))
  }

  pub async fn prewarm(&self) {
    if let Some((client, _)) = self.armed() {
      client.prewarm().await;
    }
  }

  pub async fn resolve(&self, transcript: &str) -> Result<Resolution, ControllerError> {
    let trimmed = transcript.trim();
    if trimmed.is_empty() {
      return Ok(no_intent(transcript, NluStage::RejectedNoIntent));
    }

    if self.config.use_fast_path
      && let Some(hit) = fast_path::match_transcript(trimmed)
    {
      return Ok(Resolution {
        resolved: resolved(hit.intent, hit.slots, transcript, None),
        stage: NluStage::FastPath,
      });
    }

    let Some((client, policy)) = self.armed() else {
      return Ok(no_intent(transcript, NluStage::NoModel));
    };

    let output = client
      .infer(trimmed)
      .await
      .map_err(|error| ControllerError::InferenceFailed(error.to_string()))?;

    Ok(match rejection::evaluate(&output, policy)? {
      RejectionOutcome::NoIntent => no_intent(transcript, NluStage::RejectedNoIntent),
      RejectionOutcome::Clarify { alternates } => Resolution {
        resolved: resolved(
          intent_catalog::CLARIFY,
          NluSlots::default(),
          transcript,
          Some(
            alternates
              .into_iter()
              .map(|intent| NluAlternate {
                intent: intent.to_owned(),
                slots: None,
              })
              .collect(),
          ),
        ),
        stage: NluStage::RejectedClarify,
      },
      RejectionOutcome::Accept { intent } if !fast_path::cued(intent, &output.slots, trimmed) => {
        no_intent(transcript, NluStage::RejectedNoIntent)
      }
      RejectionOutcome::Accept { intent } => Resolution {
        resolved: resolved(intent, output.slots, transcript, None),
        stage: NluStage::Model,
      },
    })
  }
}

fn resolved(
  intent: &str,
  slots: NluSlots,
  transcript: &str,
  alternates: Option<Vec<NluAlternate>>,
) -> NluResolvedIntent {
  NluResolvedIntent {
    intent: intent.to_owned(),
    slots,
    transcript: transcript.to_owned(),
    alternates,
  }
}

pub fn no_intent(transcript: &str, stage: NluStage) -> Resolution {
  Resolution {
    resolved: resolved(intent_catalog::NO_INTENT, NluSlots::default(), transcript, None),
    stage,
  }
}
