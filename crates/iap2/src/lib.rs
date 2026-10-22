pub mod csm;
#[cfg(feature = "emulator")]
pub mod emulator;
mod error;
mod frame;
mod link;
pub mod session;

#[cfg(feature = "emulator")]
pub use emulator::{DeviceEaStream, DeviceEmulator, DeviceEmulatorHandle, EmulatorEvent};
pub use error::{Error, Result};
pub use frame::{
  ControlBits, DETECT_MARKER, LINK_HEADER_LEN, LINK_MAGIC, LinkCodec, LinkHeader, LinkPacket, Lsp, SessionTriple,
  SessionType,
};
pub use link::{Iap2Command, Iap2Event, Link, LinkConfig};
pub use session::{
  AppLaunch, HidCommand, Iap2Session, MfiAccess, MfiHandle, NowPlayingAuthorityState, NowPlayingCommand, SessionEvent,
  WorkerMfiAccess,
};

pub const IAP2_ACCESSORY_UUID: uuid::Uuid = uuid::Uuid::from_u128(0x00000000_DECA_FADE_DECA_DEAFDECACAFF);
pub const IAP2_DEVICE_UUID: uuid::Uuid = uuid::Uuid::from_u128(0x00000000_DECA_FADE_DECA_DEAFDECACAFE);
pub const IAP2_RFCOMM_CHANNEL: u8 = 2;
