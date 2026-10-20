use crate::CatalogError;

pub type VoiceResult<T> = std::result::Result<T, VoiceResolveError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceTargetKind {
  Track,
  Album,
  Artist,
  Playlist,
  Show,
  Episode,
  Station,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicePopularity {
  Top5,
  Top10,
  Popular,
  Recent,
  New,
  First,
  Random,
}

impl VoicePopularity {
  pub(crate) fn depth(self) -> Option<usize> {
    match self {
      VoicePopularity::Top5 => Some(5),
      VoicePopularity::Top10 => Some(10),
      _ => None,
    }
  }
}

#[derive(Debug, Clone, Default)]
pub struct VoiceResolveRequest {
  pub target: Option<String>,
  pub target_type: Option<VoiceTargetKind>,
  pub mood: Option<String>,
  pub genre: Option<String>,
  pub era: Option<String>,
  pub popularity_filter: Option<VoicePopularity>,
  pub position: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceAlternative {
  pub uri: String,
  pub display: String,
  pub kind: VoiceTargetKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceResolved {
  pub uri: String,
  pub context_uri: Option<String>,
  pub display: String,
  pub kind: VoiceTargetKind,
  pub artist: Option<String>,
  pub year: Option<i32>,
  pub alternatives: Vec<VoiceAlternative>,
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceResolveError {
  #[error("nothing matched the request")]
  NoMatch,
  #[error("a position needs a context and nothing is playing")]
  NoAnchorContext,
  #[error(transparent)]
  Catalog(#[from] CatalogError),
}
