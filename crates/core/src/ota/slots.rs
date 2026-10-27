use crate::{
  paths::{ON_DEVICE_SENTINEL, is_on_device},
  uboot_env::{self, EnvError},
};

const TRY_MAX: &str = "3";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
  A,
  B,
}

impl Slot {
  fn tries_var(self) -> &'static str {
    match self {
      Slot::A => "slot_a_tries",
      Slot::B => "slot_b_tries",
    }
  }
}

pub async fn inactive_slot() -> Result<Slot, EnvError> {
  if !is_on_device() {
    tracing::warn!("inactive_slot off-device ({ON_DEVICE_SENTINEL} missing); defaulting to Slot::B");
    return Ok(Slot::B);
  }
  let active = uboot_env::read("slot_active").await?;
  Ok(match active.trim() {
    "b" => Slot::A,
    _ => Slot::B,
  })
}

pub async fn confirm_target(slot: Slot) -> Result<(), EnvError> {
  if !is_on_device() {
    tracing::warn!(
      ?slot,
      "confirm_target off-device ({ON_DEVICE_SENTINEL} missing); skipping"
    );
    return Ok(());
  }
  uboot_env::set(slot.tries_var(), TRY_MAX).await?;
  Ok(())
}
