use std::time::Duration;

use tokio::{sync::mpsc, time::Instant};

use super::{SessionEvent, emit, send_csm};
use crate::{
  csm::{
    CsmFrame,
    now_playing::{NowPlayingUpdate, SetNowPlayingInformation, StartNowPlayingUpdates},
  },
  error::Result,
  link::Iap2Command,
};

const ART_SETTLE: Duration = Duration::from_secs(4);
const SUBSCRIPTION_SETTLE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NowPlayingAuthorityState {
  pub companion_metadata: bool,
  pub companion_playback: bool,
  pub app_bundle: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NowPlayingCommand {
  pub elapsed_time_ms: Option<u32>,
  pub queue_index: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NowPlayingState {
  Idle,
  Subscribed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Subscription {
  artwork: bool,
  position: bool,
}

impl Subscription {
  const INITIAL: Self = Self {
    artwork: false,
    position: true,
  };

  fn csm(self) -> StartNowPlayingUpdates {
    StartNowPlayingUpdates::subscription(self.artwork, self.position)
  }
}

pub(super) struct NowPlayingFlow {
  state: NowPlayingState,
  rx: mpsc::Receiver<NowPlayingCommand>,
  sent: Subscription,
  pending: Option<(Subscription, Instant)>,
  subscribed_at: Option<Instant>,
  last_app_bundle: Option<String>,
}

impl NowPlayingFlow {
  pub(super) fn new(rx: mpsc::Receiver<NowPlayingCommand>) -> Self {
    Self {
      state: NowPlayingState::Idle,
      rx,
      sent: Subscription::INITIAL,
      pending: None,
      subscribed_at: None,
      last_app_bundle: None,
    }
  }

  pub(super) fn handles(msg_id: u16) -> bool {
    msg_id == NowPlayingUpdate::CSM_MSG_ID
  }

  pub(super) fn reset(&mut self) {
    self.state = NowPlayingState::Idle;
    self.sent = Subscription::INITIAL;
    self.pending = None;
    self.subscribed_at = None;
    self.last_app_bundle = None;
  }

  pub(super) async fn ensure_subscribed(&mut self, link_command_tx: &mpsc::Sender<Iap2Command>) -> Result<()> {
    if matches!(self.state, NowPlayingState::Idle) {
      tracing::info!(
        artwork = self.sent.artwork,
        position = self.sent.position,
        "iap2 now-playing: subscribing"
      );
      send_csm(self.sent.csm(), link_command_tx).await?;
      self.state = NowPlayingState::Subscribed;
      self.subscribed_at = Some(Instant::now());
    }
    Ok(())
  }

  fn desired(&self, authority: &NowPlayingAuthorityState, now: Instant) -> Option<Subscription> {
    let foreground = self.last_app_bundle.as_deref()?;
    let claimed = authority.app_bundle.as_deref();
    let served = claimed == Some(foreground);
    let settled = self.subscribed_at.is_some_and(|at| now >= at + ART_SETTLE);
    let artwork = match claimed {
      Some(_) if !served => true,
      Some(_) if authority.companion_metadata => false,
      _ => settled,
    };
    Some(Subscription {
      artwork,
      position: !(served && authority.companion_playback),
    })
  }

  async fn reconcile(
    &mut self,
    delta_bundle: Option<&str>,
    authority: &NowPlayingAuthorityState,
    link_command_tx: &mpsc::Sender<Iap2Command>,
  ) -> Result<()> {
    if let Some(bundle) = delta_bundle.filter(|b| !b.is_empty()) {
      self.last_app_bundle = Some(bundle.to_string());
    }
    let now = Instant::now();
    let Some(desired) = self.desired(authority, now) else {
      return Ok(());
    };
    if desired == self.sent {
      self.pending = None;
      return Ok(());
    }
    let due = match self.pending {
      Some((held, due)) if held == desired => due,
      _ => now + SUBSCRIPTION_SETTLE,
    };
    self.pending = Some((desired, due));
    if now < due {
      return Ok(());
    }
    self.pending = None;
    self.sent = desired;
    tracing::info!(
      bundle = self.last_app_bundle.as_deref().unwrap_or_default(),
      claimed = authority.app_bundle.as_deref().unwrap_or_default(),
      artwork = desired.artwork,
      position = desired.position,
      "iap2 now-playing: changing subscription"
    );
    send_csm(desired.csm(), link_command_tx).await
  }

  pub(super) fn next_wakeup(&self) -> Option<Instant> {
    if !matches!(self.state, NowPlayingState::Subscribed) {
      return None;
    }
    let art_settles = self
      .subscribed_at
      .map(|at| at + ART_SETTLE)
      .filter(|at| *at > Instant::now());
    match (self.pending.map(|(_, due)| due), art_settles) {
      (Some(a), Some(b)) => Some(a.min(b)),
      (a, b) => a.or(b),
    }
  }

  pub(super) async fn reconcile_companion(
    &mut self,
    authority: &NowPlayingAuthorityState,
    link_command_tx: &mpsc::Sender<Iap2Command>,
  ) -> Result<()> {
    if !matches!(self.state, NowPlayingState::Subscribed) {
      return Ok(());
    }
    self.reconcile(None, authority, link_command_tx).await
  }

  pub(super) async fn handle(
    &mut self,
    frame: CsmFrame,
    authority: &NowPlayingAuthorityState,
    link_command_tx: &mpsc::Sender<Iap2Command>,
    session_events_tx: &mpsc::Sender<SessionEvent>,
  ) -> Result<Option<SessionEvent>> {
    if frame.msg_id != NowPlayingUpdate::CSM_MSG_ID {
      return Ok(None);
    }
    let update: NowPlayingUpdate = frame.try_into()?;
    if matches!(self.state, NowPlayingState::Idle) {
      tracing::warn!("iap2 now-playing: received update before subscribing; surfacing anyway");
    }
    tracing::trace!(?update, "iap2 now-playing: delta received");
    let app_bundle = update.playback.as_ref().and_then(|p| p.app_bundle.clone());
    emit(session_events_tx, SessionEvent::NowPlayingUpdate(update)).await;
    self
      .reconcile(app_bundle.as_deref(), authority, link_command_tx)
      .await?;
    Ok(None)
  }

  pub(super) async fn recv(&mut self) -> Option<NowPlayingCommand> {
    self.rx.recv().await
  }

  pub(super) async fn handle_command(
    &mut self,
    cmd: NowPlayingCommand,
    link_command_tx: &mpsc::Sender<Iap2Command>,
  ) -> Result<()> {
    if !matches!(self.state, NowPlayingState::Subscribed) {
      tracing::warn!(
        ?cmd,
        "iap2 now-playing: command before StartNowPlayingUpdates; dropping"
      );
      return Ok(());
    }
    if cmd.elapsed_time_ms.is_none() && cmd.queue_index.is_none() {
      tracing::trace!("iap2 now-playing: empty NowPlayingCommand; ignoring");
      return Ok(());
    }
    let csm = SetNowPlayingInformation {
      elapsed_time_ms: cmd.elapsed_time_ms,
      queue_index: cmd.queue_index,
      queue_list_content_transfer_start_index: None,
    };
    tracing::debug!(?csm, "iap2 now-playing: sending SetNowPlayingInformation");
    send_csm(csm, link_command_tx).await
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const SPOTIFY: &str = "com.spotify.client";
  const MUSIC: &str = "com.apple.Music";
  const YOUTUBE: &str = "com.google.ios.youtube";

  fn serving(bundle: &str) -> NowPlayingAuthorityState {
    NowPlayingAuthorityState {
      companion_metadata: true,
      companion_playback: true,
      app_bundle: Some(bundle.to_string()),
    }
  }

  fn absent() -> NowPlayingAuthorityState {
    NowPlayingAuthorityState::default()
  }

  struct Rig {
    flow: NowPlayingFlow,
    link_tx: mpsc::Sender<Iap2Command>,
    link_rx: mpsc::Receiver<Iap2Command>,
    _np_tx: mpsc::Sender<NowPlayingCommand>,
  }

  impl Rig {
    async fn subscribed() -> Self {
      let (np_tx, np_rx) = mpsc::channel(4);
      let (link_tx, link_rx) = mpsc::channel(64);
      let mut rig = Self {
        flow: NowPlayingFlow::new(np_rx),
        link_tx,
        link_rx,
        _np_tx: np_tx,
      };
      rig.flow.ensure_subscribed(&rig.link_tx).await.unwrap();
      assert_eq!(rig.sends(), 1, "the generation opens with one subscribe");
      rig
    }

    fn sends(&mut self) -> usize {
      let mut n = 0;
      while self.link_rx.try_recv().is_ok() {
        n += 1;
      }
      n
    }

    async fn delta(&mut self, bundle: &str, authority: &NowPlayingAuthorityState) {
      self
        .flow
        .reconcile(Some(bundle), authority, &self.link_tx)
        .await
        .unwrap();
    }

    async fn authority(&mut self, authority: &NowPlayingAuthorityState) {
      self.flow.reconcile_companion(authority, &self.link_tx).await.unwrap();
    }

    async fn run_for(&mut self, span: Duration, authority: &NowPlayingAuthorityState) {
      let end = Instant::now() + span;
      while let Some(at) = self.flow.next_wakeup().filter(|at| *at <= end) {
        tokio::time::sleep_until(at).await;
        self.authority(authority).await;
      }
      tokio::time::sleep_until(end).await;
    }

    fn sent(&self) -> Subscription {
      self.flow.sent
    }
  }

  const ART_OFF_POSITION_OFF: Subscription = Subscription {
    artwork: false,
    position: false,
  };
  const ART_ON_POSITION_ON: Subscription = Subscription {
    artwork: true,
    position: true,
  };

  #[tokio::test(start_paused = true)]
  async fn a_bundle_the_companion_claims_never_turns_iap2_art_on() {
    let mut rig = Rig::subscribed().await;
    let claim = serving(MUSIC);

    rig.delta(MUSIC, &claim).await;
    rig.run_for(ART_SETTLE * 3, &claim).await;

    assert!(
      !rig.sent().artwork,
      "the companion already ships this app's art over EA"
    );
    assert!(!rig.sent().position, "the companion already drives this app's bar");
    assert_eq!(rig.sends(), 1, "one subscription change for the settled state");
  }

  #[tokio::test(start_paused = true)]
  async fn an_app_the_companion_does_not_serve_gets_iap2_art() {
    let mut rig = Rig::subscribed().await;
    let claim = serving(SPOTIFY);

    rig.delta(YOUTUBE, &claim).await;
    rig
      .run_for(SUBSCRIPTION_SETTLE - Duration::from_millis(1), &claim)
      .await;
    assert_eq!(rig.sends(), 0, "nothing goes on the wire before the state settles");

    rig.run_for(Duration::from_millis(1), &claim).await;
    assert_eq!(rig.sent(), ART_ON_POSITION_ON);
    assert_eq!(rig.sends(), 1);
  }

  #[tokio::test(start_paused = true)]
  async fn an_unclaimed_bundle_holds_art_off_until_art_settle() {
    let mut rig = Rig::subscribed().await;
    let none = absent();

    rig.delta(SPOTIFY, &none).await;
    rig.run_for(ART_SETTLE - Duration::from_millis(1), &none).await;
    assert!(!rig.sent().artwork, "a companion may still be signing in");
    assert_eq!(rig.sends(), 0);

    rig.run_for(SUBSCRIPTION_SETTLE * 2, &none).await;
    assert!(
      rig.sent().artwork,
      "with no claim by the deadline iAP2 is the art source, even with no delta to prompt it"
    );
    assert!(rig.sent().position);
    assert_eq!(rig.sends(), 1);
  }

  #[tokio::test(start_paused = true)]
  async fn a_claim_landing_inside_art_settle_keeps_art_off() {
    let mut rig = Rig::subscribed().await;
    let none = absent();
    let claim = serving(SPOTIFY);

    rig.delta(SPOTIFY, &none).await;
    rig.run_for(Duration::from_secs(2), &none).await;
    rig.authority(&claim).await;
    rig.run_for(ART_SETTLE * 3, &claim).await;

    assert_eq!(rig.sent(), ART_OFF_POSITION_OFF);
    assert_eq!(rig.sends(), 1, "art never flickered on before the claim");
  }

  #[tokio::test(start_paused = true)]
  async fn a_restart_returns_art_to_off() {
    let mut rig = Rig::subscribed().await;
    let claim = serving(SPOTIFY);
    rig.delta(YOUTUBE, &claim).await;
    rig.run_for(SUBSCRIPTION_SETTLE, &claim).await;
    assert!(rig.sent().artwork);
    rig.sends();

    rig.flow.reset();
    rig.flow.ensure_subscribed(&rig.link_tx).await.unwrap();

    assert_eq!(rig.sends(), 1);
    assert_eq!(
      rig.sent(),
      Subscription::INITIAL,
      "a new generation must not ask iOS to resend art at its most fragile moment"
    );
  }

  #[tokio::test(start_paused = true)]
  async fn a_foreground_flap_during_a_handover_sends_one_subscription() {
    let mut rig = Rig::subscribed().await;
    let claim = serving(MUSIC);

    for i in 0..10 {
      let bundle = if i % 2 == 0 { SPOTIFY } else { MUSIC };
      rig.delta(bundle, &claim).await;
      rig.run_for(Duration::from_millis(300), &claim).await;
    }
    assert_eq!(rig.sends(), 0, "a flapping foreground never reaches the wire");

    rig.delta(MUSIC, &claim).await;
    rig.run_for(ART_SETTLE * 2, &claim).await;
    assert_eq!(rig.sent(), ART_OFF_POSITION_OFF);
    assert_eq!(rig.sends(), 1, "one StartNowPlayingUpdates for the settled state");
  }

  #[tokio::test(start_paused = true)]
  async fn a_claim_split_across_publishes_coalesces() {
    let mut rig = Rig::subscribed().await;
    rig.delta(SPOTIFY, &absent()).await;

    let metadata = NowPlayingAuthorityState {
      companion_metadata: true,
      companion_playback: false,
      app_bundle: Some(SPOTIFY.to_string()),
    };
    rig.authority(&metadata).await;
    rig.run_for(Duration::from_millis(400), &metadata).await;
    let full = serving(SPOTIFY);
    rig.authority(&full).await;
    rig.run_for(ART_SETTLE * 2, &full).await;

    assert_eq!(rig.sent(), ART_OFF_POSITION_OFF);
    assert_eq!(rig.sends(), 1);
  }

  #[tokio::test(start_paused = true)]
  async fn a_state_that_returns_to_what_was_sent_sends_nothing() {
    let mut rig = Rig::subscribed().await;
    let claim = serving(SPOTIFY);

    rig.delta(SPOTIFY, &claim).await;
    rig.run_for(Duration::from_millis(500), &claim).await;
    rig.authority(&absent()).await;
    rig.run_for(Duration::from_secs(1), &absent()).await;

    assert_eq!(rig.sends(), 0, "a blip that undoes itself is not a subscription change");
    assert_eq!(rig.sent(), Subscription::INITIAL);
  }

  #[tokio::test(start_paused = true)]
  async fn authority_release_restores_the_iap2_bar() {
    let mut rig = Rig::subscribed().await;
    let claim = serving(SPOTIFY);
    rig.delta(SPOTIFY, &claim).await;
    rig.run_for(ART_SETTLE * 2, &claim).await;
    assert!(!rig.sent().position);
    rig.sends();

    rig.authority(&absent()).await;
    rig.run_for(SUBSCRIPTION_SETTLE, &absent()).await;

    assert_eq!(
      rig.sent(),
      ART_ON_POSITION_ON,
      "with the companion gone iAP2 owns art and the bar again, with no delta to prompt it"
    );
    assert_eq!(rig.sends(), 1);
  }

  #[tokio::test(start_paused = true)]
  async fn art_and_position_gate_on_independent_scopes() {
    let mut rig = Rig::subscribed().await;
    let metadata_only = NowPlayingAuthorityState {
      companion_metadata: true,
      companion_playback: false,
      app_bundle: Some(SPOTIFY.to_string()),
    };
    rig.delta(SPOTIFY, &metadata_only).await;
    rig.run_for(ART_SETTLE * 2, &metadata_only).await;
    assert_eq!(
      rig.sent(),
      Subscription::INITIAL,
      "metadata authority drops art, no playback authority keeps the bar"
    );

    let playback_only = NowPlayingAuthorityState {
      companion_metadata: false,
      companion_playback: true,
      app_bundle: Some(SPOTIFY.to_string()),
    };
    rig.authority(&playback_only).await;
    rig.run_for(SUBSCRIPTION_SETTLE, &playback_only).await;
    assert_eq!(
      rig.sent(),
      Subscription {
        artwork: true,
        position: false,
      }
    );
  }

  #[tokio::test(start_paused = true)]
  async fn an_idle_sentinel_bundle_is_not_a_foreground_change() {
    let mut rig = Rig::subscribed().await;
    let claim = serving(SPOTIFY);
    rig.delta(SPOTIFY, &claim).await;
    rig.run_for(ART_SETTLE * 2, &claim).await;
    rig.sends();

    rig.delta("", &claim).await;
    rig.run_for(ART_SETTLE * 2, &claim).await;

    assert_eq!(rig.sends(), 0);
    assert_eq!(rig.sent(), ART_OFF_POSITION_OFF);
  }
}
