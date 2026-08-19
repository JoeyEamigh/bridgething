#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum WakeReason {
  UserPlay,
  ConnectResume,
}

#[uniffi::export(with_foreign)]
pub trait DeviceWaker: Send + Sync {
  fn wake_device(&self, reason: WakeReason, allow_play_tap: bool);
}
