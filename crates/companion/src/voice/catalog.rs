use std::sync::Arc;

use libbridgething::{NluPopularityFilter, NluResolvedIntent, NluSlots, NluTargetType};
use voice_resolver::{Catalog, VoicePopularity, VoiceResolveRequest, VoiceTargetKind, resolve};

use crate::voice::dispatcher::{CatalogError, VoiceCatalogResolver};

pub struct CatalogResolver {
  catalog: Arc<dyn Catalog>,
}

impl CatalogResolver {
  pub fn new(catalog: Arc<dyn Catalog>) -> Arc<Self> {
    Arc::new(Self { catalog })
  }
}

pub fn catalog_intent(intent: &str) -> bool {
  matches!(
    intent,
    "PLAY" | "ADD_TO_QUEUE" | "ADD_TO_PLAYLIST" | "SEARCH" | "THUMBS_UP"
  )
}

fn clean(value: &Option<String>) -> Option<String> {
  value
    .as_ref()
    .map(|text| text.trim().to_owned())
    .filter(|text| !text.is_empty())
}

fn voice_kind(kind: NluTargetType) -> VoiceTargetKind {
  match kind {
    NluTargetType::Artist => VoiceTargetKind::Artist,
    NluTargetType::Track => VoiceTargetKind::Track,
    NluTargetType::Album => VoiceTargetKind::Album,
    NluTargetType::Playlist => VoiceTargetKind::Playlist,
    NluTargetType::Podcast => VoiceTargetKind::Show,
    NluTargetType::Episode => VoiceTargetKind::Episode,
    NluTargetType::Station => VoiceTargetKind::Station,
  }
}

fn voice_popularity(filter: NluPopularityFilter) -> VoicePopularity {
  match filter {
    NluPopularityFilter::Top5 => VoicePopularity::Top5,
    NluPopularityFilter::Top10 => VoicePopularity::Top10,
    NluPopularityFilter::Popular => VoicePopularity::Popular,
    NluPopularityFilter::Recent => VoicePopularity::Recent,
    NluPopularityFilter::New => VoicePopularity::New,
    NluPopularityFilter::First => VoicePopularity::First,
    NluPopularityFilter::Random => VoicePopularity::Random,
  }
}

pub fn voice_request(slots: &NluSlots) -> VoiceResolveRequest {
  VoiceResolveRequest {
    target: clean(&slots.target),
    target_type: slots.target_type.map(voice_kind),
    mood: clean(&slots.mood),
    genre: clean(&slots.genre),
    era: clean(&slots.era),
    popularity_filter: slots.popularity_filter.map(voice_popularity),
    position: slots.position,
  }
}

pub fn names_nothing(req: &VoiceResolveRequest) -> bool {
  req.target.is_none()
    && req.target_type.is_none()
    && req.mood.is_none()
    && req.genre.is_none()
    && req.era.is_none()
    && req.position.is_none()
    && req.popularity_filter.is_none()
}

#[async_trait::async_trait]
impl VoiceCatalogResolver for CatalogResolver {
  async fn decorate(&self, resolved: NluResolvedIntent) -> Result<NluResolvedIntent, CatalogError> {
    if !catalog_intent(&resolved.intent) {
      return Ok(resolved);
    }
    let request = voice_request(&resolved.slots);
    if names_nothing(&request) {
      return Ok(resolved);
    }
    let answer = resolve(self.catalog.as_ref(), request)
      .await
      .map_err(|error| CatalogError::Failed(error.to_string()))?;
    let mut resolved = resolved;
    resolved.slots.uri = Some(answer.uri);
    resolved.slots.context_uri = answer.context_uri;
    Ok(resolved)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_request_carrying_only_a_popularity_filter_still_reaches_the_catalog() {
    for filter in [
      NluPopularityFilter::Top5,
      NluPopularityFilter::Top10,
      NluPopularityFilter::Popular,
      NluPopularityFilter::Recent,
      NluPopularityFilter::New,
      NluPopularityFilter::First,
      NluPopularityFilter::Random,
    ] {
      let request = VoiceResolveRequest {
        popularity_filter: Some(voice_popularity(filter)),
        ..Default::default()
      };
      assert!(
        !names_nothing(&request),
        "{filter:?} is the whole request; dropping it here would silently ignore the filter"
      );
    }
  }

  #[test]
  fn a_request_naming_nothing_at_all_never_reaches_the_catalog() {
    assert!(names_nothing(&VoiceResolveRequest::default()));
    assert!(
      !names_nothing(&VoiceResolveRequest {
        target_type: Some(VoiceTargetKind::Album),
        ..Default::default()
      }),
      "a bare kind is read against now playing, so the resolver has to see it"
    );
  }
}
