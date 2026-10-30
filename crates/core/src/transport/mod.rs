use bridgething_iap2::{HidCommand, NowPlayingCommand};
use libbridgething::{
  AudioError, CompanionAuthorityScope, PlayContext, PlayerError, QueuePosition, RepeatMode,
  gateway::{
    BridgeToGatewayAudioMsgCommand, BridgeToGatewayMsgData, BridgeToGatewayPlayerMsgCommand, PlayUri, QueueUri,
    SeekTo as GatewaySeekTo, SetMute, SetRepeat as GatewaySetRepeat, SetShuffle as GatewaySetShuffle, SetSpeed,
    SetVolume, SkipToIndex as GatewaySkipToIndex,
  },
  wire::WireCommand,
};

use crate::{
  authority::AuthorityRegistry,
  bluetooth::{Address, BluetoothMan, iap2::Iap2TransportHandle},
  capabilities::{CapabilitiesRegistry, is_valid_scheme},
  peer::PeerTracker,
  player::Player,
  state::State,
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
  #[error("no iPhone or companion is connected to carry {0}")]
  NoTarget(&'static str),
  #[error("no connected companion claims the {0:?} scheme")]
  SchemeUnclaimed(String),
  #[error("{0}")]
  Refused(String),
}

impl From<TransportError> for PlayerError {
  fn from(err: TransportError) -> Self {
    match err {
      TransportError::NoTarget(_) => PlayerError::NoGateway,
      TransportError::SchemeUnclaimed(scheme) => PlayerError::SchemeUnclaimed { scheme },
      TransportError::Refused(reason) => PlayerError::PlayFailed { reason },
    }
  }
}

impl From<TransportError> for AudioError {
  fn from(err: TransportError) -> Self {
    match err {
      TransportError::NoTarget(_) | TransportError::SchemeUnclaimed(_) => AudioError::NoTarget,
      TransportError::Refused(reason) => AudioError::ActionRejected { reason },
    }
  }
}

pub type TransportResult = Result<(), TransportError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
  Companion(Address),
  Iap2,
}

#[derive(Debug, Clone)]
pub struct TransportController {
  authority: AuthorityRegistry,
  capabilities: CapabilitiesRegistry,
  peers: PeerTracker,
  player: Player,
  bluetooth: BluetoothMan,
  iap2: Iap2TransportHandle,
}

impl TransportController {
  pub fn new(state: &State, bluetooth: BluetoothMan) -> Self {
    Self {
      authority: state.authority.clone(),
      capabilities: state.capabilities.clone(),
      peers: state.peers.clone(),
      player: state.player.clone(),
      iap2: bluetooth.iap2.transport.clone(),
      bluetooth,
    }
  }

  pub async fn play(&self) -> TransportResult {
    let route = self.player_route("play")?;
    if let Err(err) = self.player.apply_transport_intent(true).await {
      tracing::warn!(?err, "transport play: failed to broadcast optimistic intent");
    }
    self
      .deliver_player(route, BridgeToGatewayPlayerMsgCommand::Resume, hid_bit::PLAY_PAUSE)
      .await
  }

  pub async fn pause(&self) -> TransportResult {
    let route = self.player_route("pause")?;
    if let Err(err) = self.player.apply_transport_intent(false).await {
      tracing::warn!(?err, "transport pause: failed to broadcast optimistic intent");
    }
    self
      .deliver_player(route, BridgeToGatewayPlayerMsgCommand::Pause, hid_bit::PLAY_PAUSE)
      .await
  }

  pub async fn next(&self) -> TransportResult {
    self
      .dispatch_player("next", BridgeToGatewayPlayerMsgCommand::SkipNext, hid_bit::NEXT)
      .await
  }

  pub async fn prev(&self, allow_seeking: bool) -> TransportResult {
    if allow_seeking && self.companion_owns_playback() && self.player.position_ms() > PREV_RESTART_THRESHOLD_MS {
      return self.seek_to(0).await;
    }
    self
      .dispatch_player("prev", BridgeToGatewayPlayerMsgCommand::SkipPrev, hid_bit::PREV)
      .await
  }

  pub async fn volume_up(&self) -> TransportResult {
    self
      .dispatch_audio(
        "volume_up",
        BridgeToGatewayAudioMsgCommand::VolumeUp,
        hid_bit::VOLUME_UP,
      )
      .await
  }

  pub async fn volume_down(&self) -> TransportResult {
    self
      .dispatch_audio(
        "volume_down",
        BridgeToGatewayAudioMsgCommand::VolumeDown,
        hid_bit::VOLUME_DOWN,
      )
      .await
  }

  pub async fn mute_toggle(&self) -> TransportResult {
    self
      .dispatch_audio("mute_toggle", BridgeToGatewayAudioMsgCommand::MuteToggle, hid_bit::MUTE)
      .await
  }

  pub async fn set_shuffle(&self, on: bool) -> TransportResult {
    match self.player_route("set_shuffle")? {
      Route::Companion(addr) => {
        self
          .send_companion(
            addr,
            BridgeToGatewayPlayerMsgCommand::SetShuffle(GatewaySetShuffle { on }),
          )
          .await;
        Ok(())
      }
      Route::Iap2 => match self.player.iap2_shuffle() {
        None => Err(refused(format!(
          "set_shuffle({on}): the phone's shuffle state is unknown, so a toggle could land either way"
        ))),
        Some(current) if current == on => Ok(()),
        Some(_) => self.send_iap2(HidCommand::Pulse(hid_bit::SHUFFLE)).await,
      },
    }
  }

  pub async fn set_repeat(&self, mode: RepeatMode) -> TransportResult {
    match self.player_route("set_repeat")? {
      Route::Companion(addr) => {
        self
          .send_companion(
            addr,
            BridgeToGatewayPlayerMsgCommand::SetRepeat(GatewaySetRepeat { mode }),
          )
          .await;
        Ok(())
      }
      Route::Iap2 => match self.player.iap2_repeat_mode() {
        None => Err(refused(format!(
          "set_repeat({mode:?}): the phone's repeat state is unknown, so a toggle could land anywhere"
        ))),
        Some(current) if current == mode => Ok(()),
        Some(current) => {
          self
            .send_iap2(HidCommand::Sequence {
              mask: hid_bit::REPEAT,
              count: repeat_toggle_count(current, mode),
            })
            .await
        }
      },
    }
  }

  pub async fn seek_to(&self, position_ms: u32) -> TransportResult {
    let route = self.player_route("seek_to")?;
    if route == Route::Iap2 && self.player.iap2_set_elapsed_time_available() == Some(false) {
      return Err(refused(format!(
        "seek_to({position_ms}): the foreground app refuses absolute seek"
      )));
    }
    if let Err(err) = self.player.apply_seek_intent(position_ms).await {
      tracing::warn!(?err, "transport seek_to: failed to broadcast optimistic intent");
    }
    match route {
      Route::Companion(addr) => {
        self
          .send_companion(
            addr,
            BridgeToGatewayPlayerMsgCommand::SeekTo(GatewaySeekTo { position_ms }),
          )
          .await;
        Ok(())
      }
      Route::Iap2 => {
        self
          .send_iap2_now_playing(NowPlayingCommand {
            elapsed_time_ms: Some(position_ms),
            queue_index: None,
          })
          .await
      }
    }
  }

  pub async fn skip_to_index(&self, index: u32) -> TransportResult {
    match self.player_route("skip_to_index")? {
      Route::Companion(addr) => {
        self
          .send_companion(
            addr,
            BridgeToGatewayPlayerMsgCommand::SkipToIndex(GatewaySkipToIndex { index }),
          )
          .await;
        Ok(())
      }
      Route::Iap2 => {
        self
          .send_iap2_now_playing(NowPlayingCommand {
            elapsed_time_ms: None,
            queue_index: Some(index),
          })
          .await
      }
    }
  }

  pub async fn play_uri(&self, uri: String, context: Option<PlayContext>) -> TransportResult {
    let claimant = self.claimant_of("play_uri", &uri)?;
    self.yield_output_to(claimant).await;
    self
      .send_companion(
        claimant,
        BridgeToGatewayPlayerMsgCommand::Play(PlayUri { uri, context }),
      )
      .await;
    Ok(())
  }

  pub async fn queue_uri(&self, uri: String, position: QueuePosition) -> TransportResult {
    let claimant = self.claimant_of("queue_uri", &uri)?;
    self
      .send_companion(
        claimant,
        BridgeToGatewayPlayerMsgCommand::Queue(QueueUri { uri, position }),
      )
      .await;
    Ok(())
  }

  pub async fn set_speed(&self, speed: f32) -> TransportResult {
    match self.player_route("set_speed")? {
      Route::Companion(addr) => {
        self
          .send_companion(addr, BridgeToGatewayPlayerMsgCommand::SetSpeed(SetSpeed { speed }))
          .await;
        Ok(())
      }
      Route::Iap2 => Err(refused(format!(
        "set_speed({speed}): iAP2 has no playback speed control"
      ))),
    }
  }

  pub async fn set_volume(&self, level: f32) -> TransportResult {
    match self.audio_route("set_volume")? {
      Route::Companion(addr) => {
        self
          .send_companion(addr, BridgeToGatewayAudioMsgCommand::SetVolume(SetVolume { level }))
          .await;
        Ok(())
      }
      Route::Iap2 => Err(refused(format!(
        "set_volume({level}): iAP2 only steps the phone's volume, it cannot set a level"
      ))),
    }
  }

  pub async fn set_mute(&self, muted: bool) -> TransportResult {
    match self.audio_route("set_mute")? {
      Route::Companion(addr) => {
        self
          .send_companion(addr, BridgeToGatewayAudioMsgCommand::SetMute(SetMute { muted }))
          .await;
        Ok(())
      }
      Route::Iap2 => Err(refused(format!(
        "set_mute({muted}): the phone's mute state is unknown, so a toggle could land either way"
      ))),
    }
  }

  pub async fn forward<C: WireCommand<BridgeToGatewayMsgData>>(&self, verb: &'static str, cmd: C) -> TransportResult {
    let addr = self.capabilities.primary_addr().ok_or(TransportError::NoTarget(verb))?;
    self.send_companion(addr, cmd).await;
    Ok(())
  }

  fn companion_owns_playback(&self) -> bool {
    self.player.companion_playback_authoritative()
  }

  fn companion_owns_volume(&self) -> bool {
    self.authority.is_authoritative(CompanionAuthorityScope::Volume)
  }

  fn player_route(&self, verb: &'static str) -> Result<Route, TransportError> {
    self.route(verb, self.companion_owns_playback())
  }

  fn audio_route(&self, verb: &'static str) -> Result<Route, TransportError> {
    self.route(verb, self.companion_owns_volume())
  }

  fn route(&self, verb: &'static str, companion_owns: bool) -> Result<Route, TransportError> {
    let route = companion_owns
      .then(|| self.authority.primary())
      .flatten()
      .map(Route::Companion)
      .or_else(|| self.peers.iap2_identified().then_some(Route::Iap2))
      .or_else(|| self.capabilities.primary_addr().map(Route::Companion))
      .ok_or(TransportError::NoTarget(verb))?;
    tracing::debug!(?route, companion_owns, "transport {verb}: routed");
    Ok(route)
  }

  fn claimant_of(&self, verb: &'static str, uri: &str) -> Result<Address, TransportError> {
    let scheme = scheme_of(uri).ok_or_else(|| TransportError::SchemeUnclaimed(uri.to_string()))?;
    if self.capabilities.primary_addr().is_none() {
      return Err(TransportError::NoTarget(verb));
    }
    let claimant = self
      .capabilities
      .companion_for_scheme(&scheme)
      .ok_or(TransportError::SchemeUnclaimed(scheme))?;
    tracing::debug!(%claimant, "transport {verb}: routed by scheme");
    Ok(claimant)
  }

  async fn yield_output_to(&self, next: Address) {
    let Some(outgoing) = self.capabilities.primary_addr().filter(|holder| *holder != next) else {
      return;
    };
    tracing::debug!(%outgoing, %next, "handing the output over to another companion");
    self
      .send_companion(outgoing, BridgeToGatewayPlayerMsgCommand::Pause)
      .await;
  }

  async fn dispatch_player(
    &self,
    verb: &'static str,
    companion_msg: BridgeToGatewayPlayerMsgCommand,
    hid_mask: u8,
  ) -> TransportResult {
    let route = self.player_route(verb)?;
    self.deliver_player(route, companion_msg, hid_mask).await
  }

  async fn deliver_player(
    &self,
    route: Route,
    companion_msg: BridgeToGatewayPlayerMsgCommand,
    hid_mask: u8,
  ) -> TransportResult {
    match route {
      Route::Companion(addr) => {
        self.send_companion(addr, companion_msg).await;
        Ok(())
      }
      Route::Iap2 => self.send_iap2(HidCommand::Pulse(hid_mask)).await,
    }
  }

  async fn dispatch_audio(
    &self,
    verb: &'static str,
    companion_msg: BridgeToGatewayAudioMsgCommand,
    hid_mask: u8,
  ) -> TransportResult {
    match self.audio_route(verb)? {
      Route::Companion(addr) => {
        self.send_companion(addr, companion_msg).await;
        Ok(())
      }
      Route::Iap2 => self.send_iap2(HidCommand::Pulse(hid_mask)).await,
    }
  }

  async fn send_companion<C: WireCommand<BridgeToGatewayMsgData>>(&self, addr: Address, msg: C) {
    self.bluetooth.gateway_man.send_command(addr, msg).await;
  }

  async fn send_iap2(&self, cmd: HidCommand) -> TransportResult {
    self
      .iap2
      .send_hid(cmd)
      .await
      .map_err(|err| refused(format!("{err}; {cmd:?} reached nothing")))
  }

  async fn send_iap2_now_playing(&self, cmd: NowPlayingCommand) -> TransportResult {
    self
      .iap2
      .send_now_playing(cmd)
      .await
      .map_err(|err| refused(format!("{err}; {cmd:?} reached nothing")))
  }
}

fn scheme_of(uri: &str) -> Option<String> {
  let (head, _) = uri.split_once(':')?;
  let scheme = head.to_ascii_lowercase();
  is_valid_scheme(&scheme).then_some(scheme)
}

fn refused(reason: String) -> TransportError {
  tracing::warn!("transport: {reason}");
  TransportError::Refused(reason)
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
  fn announce_and_play_share_one_grammar() {
    for raw in [
      "spotify",
      "Spotify",
      "apple-music",
      "ok+v.2",
      "1bad",
      "",
      "bad space",
      "x",
    ] {
      assert_eq!(
        is_valid_scheme(raw),
        scheme_of(&format!("{raw}:track:abc")).is_some(),
        "announce-time and play-time scheme grammars disagree on {raw:?}"
      );
    }
  }

  #[test]
  fn parses_simple_scheme() {
    assert_eq!(scheme_of("spotify:track:abc"), Some("spotify".into()));
  }

  #[test]
  fn lowercases() {
    assert_eq!(scheme_of("Spotify:track:abc"), Some("spotify".into()));
  }

  #[test]
  fn rejects_no_colon() {
    assert_eq!(scheme_of("spotifytrackabc"), None);
  }

  #[test]
  fn rejects_leading_digit() {
    assert_eq!(scheme_of("4spotify:track:abc"), None);
  }

  #[test]
  fn allows_compound_chars() {
    assert_eq!(scheme_of("apple-music:track:abc"), Some("apple-music".into()));
  }

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
