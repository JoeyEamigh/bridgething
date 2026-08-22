#[path = "../../../../../../../crates/companion/src/backend/audio.rs"]
pub mod audio;
#[path = "../../../../../../../crates/companion/src/backend/connectivity.rs"]
pub mod connectivity;
#[path = "../../../../../../../crates/companion/src/backend/geo.rs"]
pub mod geo;
#[path = "../../../../../../../crates/companion/src/backend/image.rs"]
pub mod image;
#[path = "../../../../../../../crates/companion/src/backend/media.rs"]
pub mod media;

pub use audio::{AudioBackend, EarconSink, SpeakEvent, SpeakSink};
pub use connectivity::{ConnectivityInbox, ConnectivityMonitor};
pub use geo::{GeoAccuracy, GeoError, GeoEvent, GeoInbox, GeoProvider, Position};
pub use image::ImageScaler;
pub use media::{
  MediaArt, MediaArtSink, MediaControl, MediaQueueEntry, MediaRepeatMode, MediaSessionBackend, MediaSessionInbox,
  MediaSessionSnapshot, MediaSnapshotSink,
};
