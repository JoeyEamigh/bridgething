use std::{
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use bridgething_iap2::HidCommand;
use libbridgething::{
  PlayerState,
  client::{BridgeToClientPlayerMsgEvent, PlayerErrorReply as ClientPlayerErrorReply},
  gateway::{
    GatewayToBridgePlayerMsgCommandDispatch, GatewayToBridgePlayerMsgEventDispatch, PlaybackTargets, PlayerErrorReply,
    QueueSnapshot, SpotifyWakeRequest,
  },
};

use super::{HandlerResult, MsgHandle};
use crate::{bluetooth::iap2::SPOTIFY_BUNDLE_ID, transport::hid_bit};

const WAKE_CLAIM_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
pub struct SpotifyWakeGate(Arc<AtomicBool>);

impl SpotifyWakeGate {
  pub fn new() -> Self {
    Self::default()
  }

  fn try_acquire(&self) -> Option<SpotifyWakeGuard> {
    self
      .0
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
      .is_ok()
      .then(|| SpotifyWakeGuard(Arc::clone(&self.0)))
  }
}

struct SpotifyWakeGuard(Arc<AtomicBool>);

impl Drop for SpotifyWakeGuard {
  fn drop(&mut self) {
    self.0.store(false, Ordering::Release);
  }
}

pub struct PlayerHandler {
  handle: MsgHandle,
}

impl PlayerHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl GatewayToBridgePlayerMsgEventDispatch for PlayerHandler {
  type Output = HandlerResult;

  async fn snapshot(&self, params: PlayerState) -> HandlerResult {
    let Some(addr) = self.handle.address else {
      return Ok(());
    };
    self.handle.state.player.apply_companion_snapshot(addr, params).await?;
    Ok(())
  }

  async fn queue_changed(&self, params: QueueSnapshot) -> HandlerResult {
    let Some(addr) = self.handle.address else {
      return Ok(());
    };
    self.handle.state.player.apply_companion_queue(addr, params).await?;
    Ok(())
  }

  async fn targets_changed(&self, params: PlaybackTargets) -> HandlerResult {
    let Some(addr) = self.handle.address else {
      return Ok(());
    };
    self
      .handle
      .state
      .playback_targets
      .apply_companion(addr, params.targets)
      .await?;
    Ok(())
  }

  async fn error_event(&self, params: PlayerErrorReply) -> HandlerResult {
    tracing::warn!(error = ?params.error, "companion refused a player verb");
    self
      .handle
      .state
      .bus
      .broadcast_event(BridgeToClientPlayerMsgEvent::ErrorEvent(ClientPlayerErrorReply {
        error: params.error,
      }))
      .await?;
    Ok(())
  }
}

impl GatewayToBridgePlayerMsgCommandDispatch for PlayerHandler {
  type Output = HandlerResult;

  async fn request_spotify_wake(&self, payload: SpotifyWakeRequest) -> HandlerResult {
    let Some(_wake) = self.handle.state.spotify_wake_gate.try_acquire() else {
      tracing::debug!("spotify wake already in progress; dropping duplicate request");
      return Ok(());
    };
    let allow_play_tap = payload.allow_play_tap;
    let transport = &self.handle.bluetooth.iap2.transport;
    let bundle = self.handle.state.player.iap2_app_bundle();
    let playing = self.handle.state.player.iap2_playing().unwrap_or(false);
    match bundle.as_deref() {
      Some(SPOTIFY_BUNDLE_ID) if playing => {
        tracing::debug!("spotify wake requested but spotify is already playing; ignoring");
      }
      Some(SPOTIFY_BUNDLE_ID) if allow_play_tap => {
        tracing::info!("spotify wake: spotify owns now-playing but is paused; tapping play");
        transport.send_hid(HidCommand::Pulse(hid_bit::PLAY_PAUSE)).await;
      }
      Some(SPOTIFY_BUNDLE_ID) => {
        tracing::info!("spotify wake: launch-only wake while paused; a tap could resume a remotely parked session");
        transport.wake_spotify().await;
      }
      Some(other) => {
        tracing::info!(bundle = %other, "spotify wake requested while another app owns now-playing; ignoring");
      }
      None => {
        tracing::info!("spotify wake: nothing playing; launching spotify");
        transport.wake_spotify().await;
        let mut snapshots = self.handle.state.player.snapshot_watch();
        let claimed = tokio::time::timeout(WAKE_CLAIM_DEADLINE, async {
          loop {
            {
              let snap = snapshots.borrow_and_update();
              match snap.iap2_app_bundle.as_deref() {
                Some(SPOTIFY_BUNDLE_ID) => return Some(snap.iap2_playing.unwrap_or(false)),
                Some(other) => {
                  tracing::info!(bundle = %other, "spotify wake: another app claimed now-playing during launch; skipping play tap");
                  return None;
                }
                None => {}
              }
            }
            if snapshots.changed().await.is_err() {
              return None;
            }
          }
        })
        .await;
        match claimed {
          Ok(Some(false)) if allow_play_tap => {
            tracing::info!("spotify wake: spotify claimed now-playing; tapping play");
            transport.send_hid(HidCommand::Pulse(hid_bit::PLAY_PAUSE)).await;
          }
          Ok(Some(false)) => {
            tracing::info!("spotify wake: spotify claimed now-playing; launch-only wake skips the play tap");
          }
          Ok(Some(true)) => tracing::debug!("spotify wake: spotify resumed on its own; play tap not needed"),
          Ok(None) => {}
          Err(_) => {
            tracing::warn!("spotify wake: spotify never claimed now-playing within the deadline; skipping play tap");
          }
        }
      }
    }
    Ok(())
  }
}
