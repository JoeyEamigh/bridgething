mod asset;
mod audio;
mod authority;
mod bluetooth;
mod capabilities;
mod diagnostics;
mod extension;
mod forward;
mod geo;
mod hardware;
mod library;
mod lyrics;
mod net;
mod notifications;
mod now_playing;
mod peer;
mod phone;
mod player;
mod priority;
mod system;
mod time;
mod tunnel;
mod voice;
mod webapp;

pub use asset::*;
pub use audio::*;
pub use authority::*;
pub use bluetooth::*;
pub use capabilities::*;
pub use diagnostics::*;
pub use extension::*;
pub use forward::*;
pub use geo::*;
pub use hardware::*;
pub use library::*;
pub use lyrics::*;
pub use net::*;
pub use notifications::*;
pub use now_playing::*;
pub use peer::*;
pub use phone::*;
pub use player::*;
pub use priority::*;
pub use system::*;
pub use time::*;
pub use tunnel::*;
pub use voice::*;
pub use webapp::*;

pub fn to_slug(value: &str) -> String {
  value
    .trim()
    .replace(' ', "_")
    .chars()
    .filter(|c| c.is_alphanumeric())
    .collect::<String>()
}
