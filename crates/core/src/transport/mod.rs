use bridgething_iap2::{HidCommand, NowPlayingCommand};
use libbridgething::{
  CompanionAuthorityScope, RepeatMode,
  gateway::{
    BridgeToGatewayAudioMsgCommand, BridgeToGatewayPlayerMsgCommand, SeekTo as GatewaySeekTo,
    SetRepeat as GatewaySetRepeat, SetShuffle as GatewaySetShuffle, SkipToIndex as GatewaySkipToIndex,
  },
};

use crate::{
  authority::AuthorityRegistry,
  bluetooth::{BluetoothMan, iap2::Iap2TransportHandle},
  player::Player,
};

const PREV_RESTART_THRESHOLD_MS: u32 = 3000;

pub(crate) mod hid_bit {
  pub const PLAY_PAUSE: u8 = 0x01;
  pub const NEXT: u8 = 0x02;
  pub const PREV: u8 = 0x04;
  pub const VOLUME_UP: u8 = 0x08;
  pub const VOLUME_DOWN: u8 = 0x10;
  pub const MUTE: u8 = 0x20;
  pub const SHUFFLE: u8 = 0x40;
  pub const REPEAT: u8 = 0x80;
}

#[derive(Debug, Clone)]
pub struct TransportController {
  authority: AuthorityRegistry,
  player: Player,
  bluetooth: BluetoothMan,
  iap2: Iap2TransportHandle,
}

impl TransportController {
  pub fn new(authority: AuthorityRegistry, player: Player, bluetooth: BluetoothMan, iap2: Iap2TransportHandle) -> Self {
    Self {
      authority,
      player,
      bluetooth,
      iap2,
    }
  }

  pub async fn play(&self) {
    if let Err(err) = self.player.apply_transport_intent(true).await {
      tracing::warn!(?err, "transport play: failed to broadcast optimistic intent");
    }
    self
      .dispatch_player("play", BridgeToGatewayPlayerMsgCommand::Resume, hid_bit::PLAY_PAUSE)
      .await;
  }

  pub async fn pause(&self) {
    if let Err(err) = self.player.apply_transport_intent(false).await {
      tracing::warn!(?err, "transport pause: failed to broadcast optimistic intent");
    }
    self
      .dispatch_player("pause", BridgeToGatewayPlayerMsgCommand::Pause, hid_bit::PLAY_PAUSE)
      .await;
  }

  pub async fn next(&self) {
    self
      .dispatch_player("next", BridgeToGatewayPlayerMsgCommand::SkipNext, hid_bit::NEXT)
      .await;
  }

  pub async fn prev(&self, allow_seeking: bool) {
    if allow_seeking && self.companion_owns_playback() && self.player.position_ms() > PREV_RESTART_THRESHOLD_MS {
      self.seek_to(0).await;
      return;
    }
    self
      .dispatch_player("prev", BridgeToGatewayPlayerMsgCommand::SkipPrev, hid_bit::PREV)
      .await;
  }

  pub async fn volume_up(&self) {
    self
      .dispatch_audio(
        "volume_up",
        BridgeToGatewayAudioMsgCommand::VolumeUp,
        hid_bit::VOLUME_UP,
      )
      .await;
  }

  pub async fn volume_down(&self) {
    self
      .dispatch_audio(
        "volume_down",
        BridgeToGatewayAudioMsgCommand::VolumeDown,
        hid_bit::VOLUME_DOWN,
      )
      .await;
  }

  pub async fn mute_toggle(&self) {
    self
      .dispatch_audio("mute_toggle", BridgeToGatewayAudioMsgCommand::MuteToggle, hid_bit::MUTE)
      .await;
  }

  pub async fn set_shuffle(&self, on: bool) {
    if self.companion_owns_playback() {
      self
        .send_player(BridgeToGatewayPlayerMsgCommand::SetShuffle(GatewaySetShuffle { on }))
        .await;
      return;
    }
    match self.player.iap2_shuffle() {
      None => {
        tracing::warn!("transport set_shuffle({on}): iap2 shuffle state unknown; refusing toggle");
      }
      Some(current) if current == on => {}
      Some(_) => self.send_iap2(HidCommand::Pulse(hid_bit::SHUFFLE)).await,
    }
  }

  pub async fn set_repeat(&self, mode: RepeatMode) {
    if self.companion_owns_playback() {
      self
        .send_player(BridgeToGatewayPlayerMsgCommand::SetRepeat(GatewaySetRepeat { mode }))
        .await;
      return;
    }
    match self.player.iap2_repeat_mode() {
      None => {
        tracing::warn!("transport set_repeat({mode:?}): iap2 repeat state unknown; refusing toggle");
      }
      Some(current) if current == mode => {}
      Some(current) => {
        let count = repeat_toggle_count(current, mode);
        self
          .send_iap2(HidCommand::Sequence {
            mask: hid_bit::REPEAT,
            count,
          })
          .await;
      }
    }
  }

  pub async fn seek_to(&self, position_ms: u32) {
    if self.companion_owns_playback() {
      if let Err(err) = self.player.apply_seek_intent(position_ms).await {
        tracing::warn!(?err, "transport seek_to: failed to broadcast optimistic intent");
      }
      self
        .send_player(BridgeToGatewayPlayerMsgCommand::SeekTo(GatewaySeekTo { position_ms }))
        .await;
      return;
    }
    if self.player.iap2_set_elapsed_time_available() == Some(false) {
      tracing::warn!("transport seek_to({position_ms}): foreground app refuses absolute seek; ignoring");
      return;
    }
    if let Err(err) = self.player.apply_seek_intent(position_ms).await {
      tracing::warn!(?err, "transport seek_to: failed to broadcast optimistic intent");
    }
    self
      .send_iap2_now_playing(NowPlayingCommand {
        elapsed_time_ms: Some(position_ms),
        queue_index: None,
      })
      .await;
  }

  pub async fn skip_to_index(&self, index: u32) {
    if self.companion_owns_playback() {
      self
        .send_player(BridgeToGatewayPlayerMsgCommand::SkipToIndex(GatewaySkipToIndex {
          index,
        }))
        .await;
      return;
    }
    self
      .send_iap2_now_playing(NowPlayingCommand {
        elapsed_time_ms: None,
        queue_index: Some(index),
      })
      .await;
  }

  fn companion_owns_playback(&self) -> bool {
    self.player.companion_playback_authoritative()
  }

  fn companion_owns_volume(&self) -> bool {
    self.authority.is_authoritative(CompanionAuthorityScope::Volume)
  }

  async fn dispatch_player(&self, verb: &str, companion_msg: BridgeToGatewayPlayerMsgCommand, hid_mask: u8) {
    if self.companion_owns_playback() {
      tracing::debug!("transport {verb}: routing to companion (player)");
      self.send_player(companion_msg).await;
      return;
    }
    tracing::debug!("transport {verb}: routing to iAP2 HID");
    self.send_iap2(HidCommand::Pulse(hid_mask)).await;
  }

  async fn dispatch_audio(&self, verb: &str, companion_msg: BridgeToGatewayAudioMsgCommand, hid_mask: u8) {
    if self.companion_owns_volume() {
      tracing::debug!("transport {verb}: routing to companion (audio)");
      self.send_audio(companion_msg).await;
      return;
    }
    tracing::debug!("transport {verb}: routing to iAP2 HID (best-effort)");
    self.send_iap2(HidCommand::Pulse(hid_mask)).await;
  }

  async fn send_player(&self, msg: BridgeToGatewayPlayerMsgCommand) {
    let Some(primary) = self.authority.primary() else {
      tracing::warn!("transport: the playback companion left before {msg:?} could be sent");
      return;
    };
    self.bluetooth.gateway_man.send_command(primary, msg).await;
  }

  async fn send_audio(&self, msg: BridgeToGatewayAudioMsgCommand) {
    let Some(primary) = self.authority.primary() else {
      tracing::warn!("transport: the volume companion left before {msg:?} could be sent");
      return;
    };
    self.bluetooth.gateway_man.send_command(primary, msg).await;
  }

  async fn send_iap2(&self, cmd: HidCommand) {
    self.iap2.send_hid(cmd).await;
  }

  async fn send_iap2_now_playing(&self, cmd: NowPlayingCommand) {
    self.iap2.send_now_playing(cmd).await;
  }
}

fn repeat_toggle_count(current: RepeatMode, target: RepeatMode) -> u8 {
  let cur = repeat_cycle_index(current);
  let tgt = repeat_cycle_index(target);
  ((tgt + 3 - cur) % 3) as u8
}

fn repeat_cycle_index(mode: RepeatMode) -> u32 {
  match mode {
    RepeatMode::Off => 0,
    RepeatMode::All => 1,
    RepeatMode::One => 2,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn repeat_toggle_count_off_to_all() {
    assert_eq!(repeat_toggle_count(RepeatMode::Off, RepeatMode::All), 1);
  }

  #[test]
  fn repeat_toggle_count_off_to_one() {
    assert_eq!(repeat_toggle_count(RepeatMode::Off, RepeatMode::One), 2);
  }

  #[test]
  fn repeat_toggle_count_all_to_one() {
    assert_eq!(repeat_toggle_count(RepeatMode::All, RepeatMode::One), 1);
  }

  #[test]
  fn repeat_toggle_count_one_to_off() {
    assert_eq!(repeat_toggle_count(RepeatMode::One, RepeatMode::Off), 1);
  }

  #[test]
  fn repeat_toggle_count_one_to_all() {
    assert_eq!(repeat_toggle_count(RepeatMode::One, RepeatMode::All), 2);
  }

  #[test]
  fn repeat_toggle_count_all_to_off() {
    assert_eq!(repeat_toggle_count(RepeatMode::All, RepeatMode::Off), 2);
  }

  #[test]
  fn repeat_toggle_count_same_state_is_zero() {
    assert_eq!(repeat_toggle_count(RepeatMode::Off, RepeatMode::Off), 0);
    assert_eq!(repeat_toggle_count(RepeatMode::All, RepeatMode::All), 0);
    assert_eq!(repeat_toggle_count(RepeatMode::One, RepeatMode::One), 0);
  }
}
