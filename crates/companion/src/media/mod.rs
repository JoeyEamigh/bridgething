#[cfg(test)]
pub mod fixture;
pub mod normalize;
pub mod plan;
pub mod sniff;

pub use normalize::{MediaNormalizer, NormalizeError, NormalizedMedia, Scratch, Staged, probe_file};
pub use plan::{CoverPlan, STILL_COVER, StillFormat, VIDEO_COVER, VideoPlan, passes_untouched, plan_cover, plan_video};
pub use sniff::{MediaFormat, MediaKind, MediaProbe, MediaSniff, Movie, movie, probe, sniff};
