use std::{
  sync::Arc,
  time::{Duration, Instant, SystemTime},
};

use bridgething_companion::backend::{HostClock, HostEnvironment as _};

use crate::{backends::DesktopHost, self_update::Updates, shell::Shell};

const TICK: Duration = Duration::from_secs(60);
const SLEPT: Duration = Duration::from_secs(2 * 60);
const SKEW: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
  Shifted,
  Woke,
}

#[derive(Debug, Clone)]
pub struct Reading {
  pub wall: SystemTime,
  pub monotonic: Instant,
  pub clock: HostClock,
}

pub trait Clock: Send + 'static {
  fn read(&self) -> Reading;
}

pub struct HostTime;

impl Clock for HostTime {
  fn read(&self) -> Reading {
    Reading {
      wall: SystemTime::now(),
      monotonic: Instant::now(),
      clock: DesktopHost.clock(),
    }
  }
}

pub struct Watch<C> {
  clock: C,
  last: Reading,
}

impl<C: Clock> Watch<C> {
  pub fn new(clock: C) -> Self {
    let last = clock.read();
    Self { clock, last }
  }

  pub fn step(&mut self) -> Option<Change> {
    let now = self.clock.read();
    let change = classify(&self.last, &now);
    self.last = now;
    change
  }
}

fn classify(before: &Reading, after: &Reading) -> Option<Change> {
  let ran = after.monotonic.saturating_duration_since(before.monotonic);
  let skew = match after.wall.duration_since(before.wall) {
    Ok(walked) if walked > SLEPT => return Some(Change::Woke),
    Ok(walked) => walked.abs_diff(ran),
    Err(behind) => behind.duration() + ran,
  };
  (skew > SKEW || zone(&before.clock) != zone(&after.clock)).then_some(Change::Shifted)
}

fn zone(clock: &HostClock) -> (&str, &str, i16, i8) {
  (
    &clock.tz_iana,
    &clock.locale,
    clock.utc_offset_minutes,
    clock.dst_offset_minutes,
  )
}

pub fn spawn(shell: Arc<Shell>, updates: Arc<Updates>) {
  tauri::async_runtime::spawn(async move {
    let mut watch = Watch::new(HostTime);
    loop {
      tokio::time::sleep(TICK).await;
      if let Some(change) = watch.step() {
        tauri::async_runtime::spawn(follow(Arc::clone(&shell), Arc::clone(&updates), change));
      }
    }
  });
}

async fn follow(shell: Arc<Shell>, updates: Arc<Updates>, change: Change) {
  match change {
    Change::Woke => {
      tracing::debug!("the host woke from sleep; resyncing providers, the device clock and the update feed");
      updates.check_now();
      shell.wake().notify_one();
      shell.session().resumed().await;
    }
    Change::Shifted => tracing::debug!("the host clock or time zone moved; pushing time to every device"),
  }
  shell.session().time_changed().await;
}

#[cfg(test)]
mod tests {
  use std::sync::Mutex;

  use super::*;

  #[derive(Clone)]
  struct Fake(Arc<Mutex<Reading>>);

  impl Clock for Fake {
    fn read(&self) -> Reading {
      self.0.lock().unwrap().clone()
    }
  }

  impl Fake {
    fn watching() -> (Self, Watch<Self>) {
      let fake = Self(Arc::new(Mutex::new(Reading {
        wall: SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000),
        monotonic: Instant::now(),
        clock: HostClock {
          tz_iana: "America/New_York".to_owned(),
          locale: "en-US".to_owned(),
          unix_seconds: 1_790_000_000,
          utc_offset_minutes: -240,
          dst_offset_minutes: 0,
        },
      })));
      let watch = Watch::new(fake.clone());
      (fake, watch)
    }

    fn pass(&self, ran: Duration, walked: Duration) {
      let mut held = self.0.lock().unwrap();
      held.monotonic += ran;
      held.wall += walked;
    }

    fn rewind(&self, ran: Duration, back: Duration) {
      let mut held = self.0.lock().unwrap();
      held.monotonic += ran;
      held.wall -= back;
    }

    fn move_to(&self, tz: &str, offset: i16) {
      let mut held = self.0.lock().unwrap();
      held.clock.tz_iana = tz.to_owned();
      held.clock.utc_offset_minutes = offset;
    }
  }

  #[test]
  fn an_ordinary_tick_changes_nothing() {
    let (clock, mut watch) = Fake::watching();

    clock.pass(TICK, TICK);
    assert_eq!(watch.step(), None);

    clock.pass(TICK, TICK + Duration::from_millis(30));
    assert_eq!(watch.step(), None, "ntp slewing a few ms is not a clock change");
  }

  #[test]
  fn a_clock_stepped_forward_is_pushed() {
    let (clock, mut watch) = Fake::watching();

    clock.pass(TICK, TICK + Duration::from_secs(45));

    assert_eq!(watch.step(), Some(Change::Shifted));
  }

  #[test]
  fn a_clock_stepped_backward_is_pushed() {
    let (clock, mut watch) = Fake::watching();

    clock.rewind(TICK, Duration::from_secs(10 * 60));

    assert_eq!(watch.step(), Some(Change::Shifted));
  }

  #[test]
  fn a_daylight_saving_change_is_pushed_though_the_clock_did_not_move() {
    let (clock, mut watch) = Fake::watching();

    clock.move_to("America/New_York", -300);
    clock.pass(TICK, TICK);

    assert_eq!(watch.step(), Some(Change::Shifted));
  }

  #[test]
  fn a_new_time_zone_is_pushed() {
    let (clock, mut watch) = Fake::watching();

    clock.move_to("America/Chicago", -300);
    clock.pass(TICK, TICK);

    assert_eq!(watch.step(), Some(Change::Shifted));
  }

  #[test]
  fn a_sleep_that_paused_the_monotonic_clock_is_a_wake() {
    let (clock, mut watch) = Fake::watching();

    clock.pass(TICK, Duration::from_secs(8 * 60 * 60));

    assert_eq!(watch.step(), Some(Change::Woke));
  }

  #[test]
  fn a_sleep_the_monotonic_clock_counted_through_is_a_wake() {
    let (clock, mut watch) = Fake::watching();

    clock.pass(Duration::from_secs(8 * 60 * 60), Duration::from_secs(8 * 60 * 60));

    assert_eq!(watch.step(), Some(Change::Woke));
  }

  #[test]
  fn a_change_is_reported_once() {
    let (clock, mut watch) = Fake::watching();
    clock.move_to("Europe/Berlin", 120);
    clock.pass(TICK, TICK);
    assert_eq!(watch.step(), Some(Change::Shifted));

    clock.pass(TICK, TICK);

    assert_eq!(watch.step(), None);
  }
}
