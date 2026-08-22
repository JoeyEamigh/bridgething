mod connectivity;
mod geo;
mod media;
mod notifications;
mod speech;

use std::{path::Path, sync::Arc};

use bridgething_companion::api::ModelPlatform;

use crate::backends::{ModelPaths, Platform, asr, geo::Locator, models, nlu, portable::PortableScaler};

pub fn platform(_config_dir: &Path) -> Platform {
  let paths = ModelPaths::default();
  Platform {
    geo: Some(Arc::new(Locator::new(geo::run))),
    notifications: Some(Arc::new(notifications::FreedesktopNotifications::default())),
    media_sessions: Some(Arc::new(media::MprisMedia::default())),
    audio: Some(Arc::new(speech::SpeechDispatcher::new())),
    connectivity: Some(Arc::new(connectivity::NetworkManagerConnectivity::default())),
    image: Some(Arc::new(PortableScaler)),
    speech: Some(Arc::new(asr::WhisperSpeech::new(paths.clone()))),
    nlu: Some(Arc::new(nlu::OrtNlu::new(paths.clone()))),
    model_validator: Some(Arc::new(models::DesktopArtifactValidator)),
    model_platform: Some(ModelPlatform::Linux),
    models: paths,
  }
}
