use libbridgething::{
  ItemKind, ItemRef, NluDirection, NluPhoneAction, NluPlaybackSpeed, NluRepeatMode, NluResolvedIntent, NluScope,
  NluSlots, NluTargetType, PlayContext, QueuePosition, RepeatMode, VoiceDispatchErrorCode, VoiceDispatchTarget,
  client::{
    BridgeToClientVoiceMsgEvent, VoiceActivity, VoiceActivityError, VoiceDisplayIntent, VoiceIntent, VoicePhase,
  },
  gateway::{
    self, BridgeToGatewayLibraryMsgCommand, BridgeToGatewayVoiceMsg, GatewayToBridgeVoiceMsgCommandDispatch,
    VoiceCloseReason, VoiceDispatch, VoiceDispatchFailed, VoiceDispatched, VoiceMicOpen,
  },
};
use uuid::Uuid;

use super::{HandlerResult, MsgHandle, webapp::navigate_url_for_active};
use crate::{chrome::ChromeCommand, mic::MicError, state::TelephonyManager, transport::TransportResult};

type Routed = Result<(VoiceDispatchTarget, Option<Uuid>), (VoiceDispatchErrorCode, String)>;

fn bad(msg: impl Into<String>) -> (VoiceDispatchErrorCode, String) {
  (VoiceDispatchErrorCode::BadSlots, msg.into())
}

pub struct VoiceHandler {
  handle: MsgHandle,
}

impl VoiceHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }
}

impl GatewayToBridgeVoiceMsgCommandDispatch for VoiceHandler {
  type Output = HandlerResult;

  async fn mic_open(&self, params: VoiceMicOpen) -> HandlerResult {
    match self.handle.state.mic.open(params.reason).await {
      Ok(stream_id) => tracing::debug!("({:?}) gateway opened mic -> stream {stream_id}", &self.handle.address),
      Err(err) => tracing::warn!("({:?}) gateway mic open failed: {err}", &self.handle.address),
    }
    Ok(())
  }

  async fn mic_close(&self) -> HandlerResult {
    if let Err(err) = self.handle.state.mic.stop_with(VoiceCloseReason::Cancelled).await {
      tracing::warn!("({:?}) gateway mic close failed: {err}", &self.handle.address);
    }
    Ok(())
  }

  async fn dispatch(&self, params: VoiceDispatch) -> HandlerResult {
    let VoiceDispatch {
      stream_id,
      resolved,
      stage,
      asr_confidence,
    } = params;
    tracing::info!(
      "({:?}) voice dispatch for {stream_id}: intent={} transcript={:?} asr_confidence={asr_confidence:?}",
      &self.handle.address,
      resolved.intent,
      resolved.transcript,
    );

    let claimed = match self.handle.state.mic.claim(stream_id).await {
      Ok(claimed) => claimed,
      Err(err) => {
        tracing::info!(
          "({:?}) voice dispatch for {stream_id} dropped: {err}",
          &self.handle.address
        );
        let code = match err {
          MicError::NotResolving(_) => VoiceDispatchErrorCode::StaleTurn,
          _ => VoiceDispatchErrorCode::Internal,
        };
        self.refuse(stream_id, &resolved.intent, code, err.to_string()).await;
        return Ok(());
      }
    };

    let turn = VoiceActivity {
      transcript: Some(resolved.transcript.clone()),
      asr_confidence,
      intent: Some(resolved.intent.clone()),
      slots: resolved.slots.clone(),
      stage,
      ..claimed
    };

    let outcome = match self.route(&resolved).await {
      Ok((target, webapp_id)) => {
        tracing::debug!("({:?}) voice dispatch -> {target:?}", &self.handle.address);
        self
          .handle
          .send_info(BridgeToGatewayVoiceMsg::Dispatched(VoiceDispatched {
            stream_id,
            target,
            intent: resolved.intent.clone(),
            webapp_id: webapp_id.map(|id| id.to_string()),
          }))
          .await;
        VoiceActivity {
          target: Some(target),
          ..turn
        }
      }
      Err((code, msg)) => {
        tracing::debug!("({:?}) voice dispatch refused: {code:?}: {msg}", &self.handle.address);
        self.refuse(stream_id, &resolved.intent, code, msg.clone()).await;
        VoiceActivity {
          phase: VoicePhase::Failed,
          error: Some(VoiceActivityError { code, msg }),
          ..turn
        }
      }
    };

    if let Err(err) = self.handle.state.mic.finish(outcome).await {
      tracing::debug!("({:?}) could not publish voice outcome: {err}", &self.handle.address);
    }
    Ok(())
  }
}

impl VoiceHandler {
  async fn refuse(&self, stream_id: Uuid, intent: &str, code: VoiceDispatchErrorCode, msg: String) {
    self
      .handle
      .send_info(BridgeToGatewayVoiceMsg::DispatchFailed(VoiceDispatchFailed {
        stream_id,
        code,
        intent: intent.to_owned(),
        msg,
      }))
      .await;
  }

  async fn route(&self, resolved: &NluResolvedIntent) -> Routed {
    let slots = &resolved.slots;
    match resolved.intent.as_str() {
      "PLAY" if slots.has_catalog_slots() => self.play_catalog(slots).await,
      "PLAY" => played(self.handle.transport.play().await),
      "PAUSE" => played(self.handle.transport.pause().await),
      "NEXT" => {
        for _ in 0..skip_count(slots)? {
          played(self.handle.transport.next().await)?;
        }
        Ok((VoiceDispatchTarget::Playback, None))
      }
      "PREVIOUS" if slots.scope == Some(NluScope::Restart) => played(self.handle.transport.seek_to(0).await),
      "PREVIOUS" => played(self.handle.transport.prev(false).await),
      "SET_SHUFFLE" => {
        let enabled = slots.enabled.ok_or_else(|| bad("SET_SHUFFLE without `enabled`"))?;
        played(self.handle.transport.set_shuffle(enabled).await)
      }
      "SET_REPEAT" => {
        let mode = slots
          .repeat_mode
          .ok_or_else(|| bad("SET_REPEAT without `repeatMode`"))?;
        played(self.handle.transport.set_repeat(repeat_mode(mode)).await)
      }
      "SEEK_RELATIVE" => {
        let seconds = slots.seconds.ok_or_else(|| bad("SEEK_RELATIVE without `seconds`"))?;
        let current = self.handle.state.player.position_ms() as i64;
        let target = (current + seconds as i64 * 1000).max(0) as u32;
        played(self.handle.transport.seek_to(target).await)
      }
      "SET_PLAYBACK_SPEED" => {
        let speed = slots.speed.ok_or_else(|| bad("SET_PLAYBACK_SPEED without `speed`"))?;
        played(self.handle.transport.set_speed(playback_speed(speed)).await)
      }
      "SET_VOLUME" => self.set_volume(slots).await,
      "PRESET_PLAY" => self.play_preset(slots).await,
      "PRESET_SAVE" => self.save_to_preset(slots).await,
      "ADD_TO_QUEUE" => {
        let uri = slots.uri.clone().ok_or((
          VoiceDispatchErrorCode::PlaybackFailed,
          "no resolved uri to queue".into(),
        ))?;
        played(self.handle.transport.queue_uri(uri, QueuePosition::Append).await)
      }
      "THUMBS_UP" => self.thumbs_up(slots).await,
      "ADD_TO_PLAYLIST" => Err((
        VoiceDispatchErrorCode::Unsupported,
        "no playlist-mutation surface exists yet".into(),
      )),

      "OPEN_WEBAPP" => self.open_webapp(slots).await,
      "SET_DISCOVERABLE" => {
        let enabled = slots.enabled.ok_or_else(|| bad("SET_DISCOVERABLE without `enabled`"))?;
        self
          .handle
          .bluetooth
          .profile_man
          .set_discoverable(enabled)
          .await
          .map_err(|err| (VoiceDispatchErrorCode::Internal, format!("{err}")))?;
        Ok((VoiceDispatchTarget::Device, None))
      }
      "PHONE_ACTION" => {
        let action = slots
          .phone_action
          .ok_or_else(|| bad("PHONE_ACTION without `phoneAction`"))?;
        self.phone_action(action).await
      }
      "CANCEL_INTERACTION" => {
        if let Err(err) = self.handle.state.mic.cancel().await {
          tracing::debug!("voice cancel: {err}");
        }
        Ok((VoiceDispatchTarget::Device, None))
      }

      "SEARCH" => self.display(VoiceDisplayIntent::Search, resolved).await,
      "MORE_LIKE_THIS" => self.display(VoiceDisplayIntent::MoreLikeThis, resolved).await,
      "SHOW_VIEW" => {
        slots.view.ok_or_else(|| bad("SHOW_VIEW without `view`"))?;
        self.display(VoiceDisplayIntent::ShowView, resolved).await
      }

      "HELP" | "CLARIFY" | "NO_INTENT" => Err((
        VoiceDispatchErrorCode::NotDispatchable,
        format!("{} is resolved at the companion edge", resolved.intent),
      )),

      other => Err((
        VoiceDispatchErrorCode::Internal,
        format!("unknown intent {other}; schema drift between companion and daemon"),
      )),
    }
  }

  async fn play_catalog(&self, slots: &NluSlots) -> Routed {
    let gateway::PlayUri { uri, context } = play_uri(slots)?;
    played(self.handle.transport.play_uri(uri, context).await)
  }

  async fn set_volume(&self, slots: &NluSlots) -> Routed {
    if let Some(muted) = slots.mute {
      return played(self.handle.transport.set_mute(muted).await);
    }
    if let Some(level) = slots.level {
      return played(self.handle.transport.set_volume((level.min(100) as f32) / 100.0).await);
    }
    played(
      match slots
        .direction
        .ok_or_else(|| bad("SET_VOLUME without direction, level, or mute"))?
      {
        NluDirection::Up => self.handle.transport.volume_up().await,
        NluDirection::Down => self.handle.transport.volume_down().await,
      },
    )
  }

  async fn play_preset(&self, slots: &NluSlots) -> Routed {
    let slot = preset_slot(slots)?;
    let presets = crate::stock::presets::list(&self.handle.state.kv)
      .await
      .map_err(|err| (VoiceDispatchErrorCode::Internal, format!("{err}")))?;
    let uri = presets
      .iter()
      .find(|p| p.slot_index == slot as usize)
      .map(|p| p.context_uri.clone())
      .ok_or((
        VoiceDispatchErrorCode::PlaybackFailed,
        format!("preset {slot} is empty"),
      ))?;
    played(self.handle.transport.play_uri(uri, None).await)
  }

  async fn save_to_preset(&self, slots: &NluSlots) -> Routed {
    let slot = preset_slot(slots)?;
    let state = self.handle.state.player.state_reply().state;
    let context = state.context.ok_or((
      VoiceDispatchErrorCode::PlaybackFailed,
      "nothing is playing to save".into(),
    ))?;
    crate::stock::presets::save(
      &self.handle.state,
      &self.handle.bluetooth,
      vec![libbridgething::stock::StockPreset {
        slot_index: slot as usize,
        context_uri: context.uri,
        name: context.name,
        description: None,
        image_url: None,
      }],
    )
    .await
    .map_err(|err| (VoiceDispatchErrorCode::Internal, format!("{err}")))?;
    Ok((VoiceDispatchTarget::Device, None))
  }

  async fn thumbs_up(&self, slots: &NluSlots) -> Routed {
    let saved = slots.enabled.unwrap_or(true);
    if let Some(uri) = slots.uri.clone() {
      return self.set_favorite(uri, favorite_kind(slots.target_type), saved).await;
    }
    if slots.scope == Some(NluScope::PreviousTrack) {
      let uri = self
        .handle
        .state
        .player
        .queue_reply()
        .previous
        .first()
        .map(|item| item.uri.clone())
        .ok_or((
          VoiceDispatchErrorCode::PlaybackFailed,
          "no recently played track to favorite".into(),
        ))?;
      return self.set_favorite(uri, ItemKind::Track, saved).await;
    }
    let uri = self
      .handle
      .state
      .player
      .state_reply()
      .state
      .track
      .and_then(|t| t.uri)
      .ok_or((
        VoiceDispatchErrorCode::PlaybackFailed,
        "nothing is playing to favorite".into(),
      ))?;
    self.set_favorite(uri, ItemKind::Track, saved).await
  }

  async fn set_favorite(&self, uri: String, kind: ItemKind, saved: bool) -> Routed {
    let favorite = BridgeToGatewayLibraryMsgCommand::FavoritesSet(gateway::FavoritesSet {
      item: ItemRef {
        uri,
        kind,
        persistent_id: None,
      },
      liked: saved,
    });
    played(self.handle.transport.forward("favorites_set", favorite).await)
  }

  async fn open_webapp(&self, slots: &NluSlots) -> Routed {
    let spoken = slots
      .webapp_name
      .as_deref()
      .ok_or_else(|| bad("OPEN_WEBAPP without `webappName`"))?;
    let id = match self.handle.state.webapps.resolve_by_name(spoken).await {
      Some(id) => id,
      None => {
        self.handle.state.webapps.rescan().await;
        self.handle.state.webapps.resolve_by_name(spoken).await.ok_or((
          VoiceDispatchErrorCode::WebappNotFound,
          format!("no installed webapp matches {spoken:?}"),
        ))?
      }
    };

    self
      .handle
      .state
      .set_active_webapp(id)
      .await
      .map_err(|err| (VoiceDispatchErrorCode::Internal, format!("{err}")))?;
    let url = navigate_url_for_active(&self.handle.state).await;
    if let Err(err) = self.handle.state.chrome.send(ChromeCommand::Navigate(url)).await {
      tracing::warn!("failed to reload kiosk after voice webapp switch: {err:?}");
    }
    self
      .handle
      .bluetooth
      .gateway_man
      .broadcast(gateway::BridgeToGatewayWebappMsgEvent::ActiveChanged(
        self.handle.state.active_webapp_changed_event().await,
      ))
      .await;
    Ok((VoiceDispatchTarget::WebappSwitch, Some(id)))
  }

  async fn display(&self, intent: VoiceDisplayIntent, resolved: &NluResolvedIntent) -> Routed {
    let active = self
      .handle
      .state
      .active_webapp()
      .await
      .map_err(|err| (VoiceDispatchErrorCode::Internal, format!("{err}")))?;
    let renders = match active {
      Some(id) => self
        .handle
        .state
        .webapps
        .manifest(id)
        .await
        .is_some_and(|m| m.renders_voice_display),
      None => false,
    };
    if !renders {
      return Err((
        VoiceDispatchErrorCode::NotDispatchable,
        "active webapp does not render voice display intents".into(),
      ));
    }

    let event = BridgeToClientVoiceMsgEvent::Intent(VoiceIntent {
      intent,
      slots: resolved.slots.clone(),
      transcript: resolved.transcript.clone(),
    });
    if let Err(errs) = self.handle.state.bus.broadcast_event(event).await {
      tracing::debug!("voice intent broadcast: {} non-fatal errors", errs.len());
    }
    Ok((VoiceDispatchTarget::Display, None))
  }

  async fn phone_action(&self, action: NluPhoneAction) -> Routed {
    let cmd = match action {
      NluPhoneAction::Answer => TelephonyManager::build_accept(0, None),
      NluPhoneAction::Decline | NluPhoneAction::End => TelephonyManager::build_end(0, None),
      NluPhoneAction::Hold => TelephonyManager::build_hold(true, None),
      NluPhoneAction::Unhold => TelephonyManager::build_hold(false, None),
      NluPhoneAction::Swap => TelephonyManager::build_hold(true, None),
      NluPhoneAction::Merge => TelephonyManager::build_accept(2, None),
      NluPhoneAction::Mute => TelephonyManager::build_mute(true),
      NluPhoneAction::Unmute => TelephonyManager::build_mute(false),
    };
    self
      .handle
      .state
      .telephony
      .dispatch(cmd)
      .await
      .map_err(|err| (VoiceDispatchErrorCode::Internal, format!("{err}")))?;
    Ok((VoiceDispatchTarget::Phone, None))
  }
}

fn played(outcome: TransportResult) -> Routed {
  outcome
    .map(|()| (VoiceDispatchTarget::Playback, None))
    .map_err(|err| (VoiceDispatchErrorCode::PlaybackFailed, err.to_string()))
}

fn play_uri(slots: &NluSlots) -> Result<gateway::PlayUri, (VoiceDispatchErrorCode, String)> {
  let uri = slots.uri.clone().ok_or((
    VoiceDispatchErrorCode::PlaybackFailed,
    "catalog play arrived without a resolved uri".into(),
  ))?;
  Ok(gateway::PlayUri {
    uri,
    context: slots.context_uri.clone().map(|context_uri| PlayContext { context_uri }),
  })
}

fn skip_count(slots: &NluSlots) -> Result<u32, (VoiceDispatchErrorCode, String)> {
  match slots.count {
    None => Ok(1),
    Some(n) if (2..=5).contains(&n) => Ok(n),
    Some(n) => Err(bad(format!("NEXT count {n} is outside 2-5"))),
  }
}

fn favorite_kind(target_type: Option<NluTargetType>) -> ItemKind {
  match target_type {
    Some(NluTargetType::Artist) => ItemKind::Artist,
    Some(NluTargetType::Album) => ItemKind::Album,
    Some(NluTargetType::Playlist) => ItemKind::Playlist,
    Some(NluTargetType::Podcast) => ItemKind::Show,
    Some(NluTargetType::Episode) => ItemKind::PodcastEpisode,
    Some(NluTargetType::Station) => ItemKind::Station,
    Some(NluTargetType::Track) | None => ItemKind::Track,
  }
}

fn preset_slot(slots: &NluSlots) -> Result<u8, (VoiceDispatchErrorCode, String)> {
  slots
    .preset
    .as_deref()
    .and_then(|p| p.parse::<u8>().ok())
    .filter(|n| (1..=4).contains(n))
    .ok_or_else(|| bad("preset must be 1-4"))
}

fn repeat_mode(mode: NluRepeatMode) -> RepeatMode {
  match mode {
    NluRepeatMode::Off => RepeatMode::Off,
    NluRepeatMode::All => RepeatMode::All,
    NluRepeatMode::One => RepeatMode::One,
  }
}

fn playback_speed(speed: NluPlaybackSpeed) -> f32 {
  match speed {
    NluPlaybackSpeed::One => 1.0,
    NluPlaybackSpeed::OnePointTwo => 1.2,
    NluPlaybackSpeed::OnePointFive => 1.5,
    NluPlaybackSpeed::Two => 2.0,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn slots() -> NluSlots {
    NluSlots::default()
  }

  #[test]
  fn bare_play_is_not_a_catalog_play() {
    assert!(!slots().has_catalog_slots());
  }

  #[test]
  fn any_catalog_slot_makes_it_a_catalog_play() {
    let mut s = slots();
    s.target = Some("mitski".into());
    assert!(s.has_catalog_slots());
    let mut s = slots();
    s.position = Some(3);
    assert!(s.has_catalog_slots());
    let mut s = slots();
    s.context_uri = Some("spotify:album:9".into());
    assert!(s.has_catalog_slots());
    let mut s = slots();
    s.target_type = Some(NluTargetType::Album);
    assert!(s.has_catalog_slots());
  }

  #[test]
  fn catalog_play_carries_the_resolved_context() {
    let mut s = slots();
    s.uri = Some("spotify:track:1".into());
    s.context_uri = Some("spotify:album:9".into());
    let play = play_uri(&s).unwrap();
    assert_eq!(play.uri, "spotify:track:1");
    assert_eq!(
      play.context,
      Some(PlayContext {
        context_uri: "spotify:album:9".into()
      })
    );
  }

  #[test]
  fn catalog_play_without_a_context_stays_contextless() {
    let mut s = slots();
    s.uri = Some("spotify:track:1".into());
    assert_eq!(play_uri(&s).unwrap().context, None);
  }

  #[test]
  fn catalog_play_without_a_uri_is_rejected() {
    let mut s = slots();
    s.context_uri = Some("spotify:album:9".into());
    assert!(play_uri(&s).is_err());
  }

  #[test]
  fn preset_rejects_out_of_range() {
    let mut s = slots();
    s.preset = Some("5".into());
    assert!(preset_slot(&s).is_err());
    s.preset = Some("3".into());
    assert_eq!(preset_slot(&s).unwrap(), 3);
  }

  #[test]
  fn next_count_defaults_to_one_and_rejects_out_of_range() {
    assert_eq!(skip_count(&slots()).unwrap(), 1);
    let mut s = slots();
    s.count = Some(4);
    assert_eq!(skip_count(&s).unwrap(), 4);
    s.count = Some(7);
    assert!(skip_count(&s).is_err());
  }
}
