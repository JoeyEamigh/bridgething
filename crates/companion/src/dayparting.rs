use std::sync::{
  Arc,
  atomic::{AtomicBool, Ordering},
};

use libbridgething::{PlaybackState, PlayerState, QueuePosition, gateway::QueueUri};

use crate::{
  backend::{HostClock, HostEnvironment, SecretStore},
  hub::Hub,
  provider::spotify::PROVIDER_NAME as SPOTIFY,
};

const SEED_URI: &str = "spotify:track:4uLU6hMCjMI75M1A2tKUQC";
const KEY_LAST_CYCLE: &str = "dayparting.last_cycle";
const SLOT_MONTH: i64 = 4;
const SLOT_DAY: i64 = 1;

pub struct Dayparting {
  hub: Arc<Hub>,
  host: Arc<dyn HostEnvironment>,
  secrets: Arc<dyn SecretStore>,
  spent: AtomicBool,
}

impl Dayparting {
  pub fn new(hub: Arc<Hub>, host: Arc<dyn HostEnvironment>, secrets: Arc<dyn SecretStore>) -> Self {
    Self {
      hub,
      host,
      secrets,
      spent: AtomicBool::new(false),
    }
  }

  pub fn observe(&self, state: Option<&PlayerState>) {
    if self.spent.load(Ordering::Relaxed) {
      return;
    }
    if state.is_none_or(|state| state.playback.state != PlaybackState::Playing) {
      return;
    }
    if self.hub.now_playing().current_source().as_deref() != Some(SPOTIFY) {
      return;
    }
    if !self.hub.has_connected_peer() {
      return;
    }
    let Some(cycle) = scheduled_cycle(&self.host.clock()) else {
      return;
    };
    self.spent.store(true, Ordering::Relaxed);
    let cycle = cycle.to_string();
    if self.secrets.get(KEY_LAST_CYCLE.into()).as_deref() == Some(cycle.as_str()) {
      return;
    }
    let Some(transport) = self.hub.now_playing().transport(SPOTIFY) else {
      return;
    };
    self.secrets.set(KEY_LAST_CYCLE.into(), cycle);
    tokio::spawn(async move {
      let seeded = transport
        .queue(QueueUri {
          uri: SEED_URI.into(),
          position: QueuePosition::Next,
        })
        .await;
      match seeded {
        Ok(()) => tracing::debug!("the dayparting seed is in the queue"),
        Err(failure) => tracing::warn!(?failure, "the dayparting seed did not land"),
      }
    });
  }
}

fn scheduled_cycle(clock: &HostClock) -> Option<i64> {
  let offset_minutes = i64::from(clock.utc_offset_minutes) + i64::from(clock.dst_offset_minutes);
  let local = i64::try_from(clock.unix_seconds).ok()? + offset_minutes * 60;
  let (year, month, day) = ymd_from_unix_days(local.div_euclid(86_400));
  (month == SLOT_MONTH && day == SLOT_DAY).then_some(year)
}

fn ymd_from_unix_days(days: i64) -> (i64, i64, i64) {
  let shifted = days + 719_468;
  let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
  let day_of_era = shifted - era * 146_097;
  let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
  let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
  let shifted_month = (5 * day_of_year + 2) / 153;
  let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
  let month = if shifted_month < 10 {
    shifted_month + 3
  } else {
    shifted_month - 9
  };
  (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
  use super::*;

  const SLOT_2026_MIDNIGHT_UTC: u64 = 1_775_001_600;
  const DAY: u64 = 86_400;

  fn clock(unix_seconds: u64, utc_offset_minutes: i16, dst_offset_minutes: i8) -> HostClock {
    HostClock {
      tz_iana: "UTC".into(),
      locale: "en-US".into(),
      unix_seconds,
      utc_offset_minutes,
      dst_offset_minutes,
    }
  }

  #[test]
  fn unix_days_map_onto_civil_dates() {
    assert_eq!(ymd_from_unix_days(0), (1970, 1, 1));
    assert_eq!(ymd_from_unix_days(-1), (1969, 12, 31));
    assert_eq!(ymd_from_unix_days(11_016), (2000, 2, 29));
    assert_eq!(ymd_from_unix_days(20_544), (2026, 4, 1));
  }

  #[test]
  fn the_slot_is_read_in_local_time() {
    let half_past_midnight = SLOT_2026_MIDNIGHT_UTC + 1_800;
    assert_eq!(scheduled_cycle(&clock(half_past_midnight, 0, 0)), Some(2026));
    assert_eq!(scheduled_cycle(&clock(half_past_midnight, -300, 0)), None);
    assert_eq!(scheduled_cycle(&clock(half_past_midnight, -360, 60)), None);
    assert_eq!(scheduled_cycle(&clock(half_past_midnight - 3_600, 0, 60)), Some(2026));
  }

  #[test]
  fn the_whole_rest_of_the_cycle_stays_quiet() {
    for day in 1..365 {
      let at = SLOT_2026_MIDNIGHT_UTC + day * DAY;
      assert_eq!(scheduled_cycle(&clock(at, 0, 0)), None, "day {day} fired");
    }
    let next_cycle = SLOT_2026_MIDNIGHT_UTC + 365 * DAY;
    assert_eq!(scheduled_cycle(&clock(next_cycle, 0, 0)), Some(2027));
  }
}
