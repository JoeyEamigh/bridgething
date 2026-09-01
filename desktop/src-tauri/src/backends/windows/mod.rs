mod connectivity;
mod geo;
mod media;
mod speech;
mod volume;

use std::{path::Path, sync::Arc};

use bridgething_companion::api::ModelPlatform;

use crate::backends::{ModelPaths, Platform, asr, geo::Locator, models, nlu, portable::PortableScaler};

pub fn platform(_config_dir: &Path) -> Platform {
  let paths = ModelPaths::default();
  Platform {
    geo: Some(Arc::new(Locator::new(geo::run))),
    notifications: None,
    media_sessions: Some(Arc::new(media::GlobalSystemMediaSessions::default())),
    audio: Some(Arc::new(speech::WinRtAudio::default())),
    volume: Some(Arc::new(volume::EndpointVolume::default())),
    connectivity: Some(Arc::new(connectivity::NetworkInformationConnectivity::default())),
    image: Some(Arc::new(PortableScaler)),
    speech: Some(Arc::new(asr::WhisperSpeech::new(paths.clone()))),
    nlu: Some(Arc::new(nlu::OrtNlu::new(paths.clone()))),
    model_validator: Some(Arc::new(models::DesktopArtifactValidator)),
    model_platform: Some(ModelPlatform::Windows),
    models: paths,
  }
}
