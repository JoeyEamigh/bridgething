mod connectivity;
mod geo;
mod media;
mod notifications;
mod speech;
mod stream;
mod volume;

use std::{path::Path, sync::Arc};

use bridgething_companion::{api::ModelPlatform, backend::MediaTranscoder};

use crate::backends::{
  ModelPaths, Platform, asr,
  ffmpeg::{FfmpegTranscoder, Sidecar},
  geo::Locator,
  models, nlu,
  portable::PortableScaler,
};

pub fn platform(_config_dir: &Path) -> Platform {
  let paths = ModelPaths::default();
  let sidecar = Sidecar::bundled();
  Platform {
    geo: Some(Arc::new(Locator::new(geo::run))),
    notifications: Some(Arc::new(notifications::FreedesktopNotifications::default())),
    media_sessions: Some(Arc::new(media::MprisMedia::default())),
    stream: Some(Arc::new(stream::PulseStream::default())),
    audio: Some(Arc::new(speech::SpeechDispatcher::new())),
    volume: Some(Arc::new(volume::PulseVolume::default())),
    connectivity: Some(Arc::new(connectivity::NetworkManagerConnectivity::default())),
    image: Some(Arc::new(PortableScaler::new(sidecar.clone()))),
    transcoder: sidecar.map(|sidecar| Arc::new(FfmpegTranscoder::new(sidecar)) as Arc<dyn MediaTranscoder>),
    speech: Some(Arc::new(asr::WhisperSpeech::new(paths.clone()))),
    nlu: Some(Arc::new(nlu::OrtNlu::new(paths.clone()))),
    model_validator: Some(Arc::new(models::DesktopArtifactValidator)),
    model_platform: Some(ModelPlatform::Linux),
    models: paths,
  }
}
