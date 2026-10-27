use std::{
  collections::VecDeque,
  time::{Duration, Instant},
};

const KEY_DOWN: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Key {
  Launcher,
  Other,
}

#[derive(Debug)]
pub(super) struct PressWindow {
  needed: usize,
  span: Duration,
  presses: VecDeque<Instant>,
}

impl PressWindow {
  pub(super) fn new(needed: usize, span: Duration) -> Self {
    Self {
      needed,
      span,
      presses: VecDeque::with_capacity(needed),
    }
  }

  pub(super) fn key(&mut self, key: Key, value: i32, at: Instant) -> bool {
    if value != KEY_DOWN {
      return false;
    }
    if key == Key::Other {
      self.presses.clear();
      return false;
    }
    while self
      .presses
      .front()
      .is_some_and(|first| at.duration_since(*first) > self.span)
    {
      self.presses.pop_front();
    }
    self.presses.push_back(at);
    if self.presses.len() < self.needed {
      return false;
    }
    self.presses.clear();
    true
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const KEY_UP: i32 = 0;
  const KEY_REPEAT: i32 = 2;
  const SPAN: Duration = Duration::from_millis(400);

  struct Clock(Instant);

  impl Clock {
    fn at(&self, ms: u64) -> Instant {
      self.0 + Duration::from_millis(ms)
    }
  }

  fn tap(window: &mut PressWindow, clock: &Clock, ms: u64) -> bool {
    let fired = window.key(Key::Launcher, KEY_DOWN, clock.at(ms));
    assert!(!window.key(Key::Launcher, KEY_UP, clock.at(ms + 60)));
    fired
  }

  #[test]
  fn two_presses_inside_the_window_fire() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(2, SPAN);
    assert!(!tap(&mut window, &clock, 0));
    assert!(tap(&mut window, &clock, 250));
  }

  #[test]
  fn presses_slower_than_the_window_never_fire() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(2, SPAN);
    for ms in [0, 450, 900, 1350, 1800] {
      assert!(!tap(&mut window, &clock, ms), "press at {ms}ms");
    }
  }

  #[test]
  fn a_late_press_starts_a_fresh_window() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(2, SPAN);
    assert!(!tap(&mut window, &clock, 0));
    assert!(!tap(&mut window, &clock, 500));
    assert!(tap(&mut window, &clock, 700));
  }

  #[test]
  fn another_key_between_presses_breaks_the_sequence() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(2, SPAN);
    assert!(!tap(&mut window, &clock, 0));
    assert!(!window.key(Key::Other, KEY_DOWN, clock.at(100)));
    assert!(!window.key(Key::Other, KEY_UP, clock.at(150)));
    assert!(!tap(&mut window, &clock, 200));
    assert!(tap(&mut window, &clock, 300));
  }

  #[test]
  fn releasing_another_key_mid_sequence_does_not_break_it() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(2, SPAN);
    assert!(!tap(&mut window, &clock, 0));
    assert!(!window.key(Key::Other, KEY_UP, clock.at(100)));
    assert!(tap(&mut window, &clock, 200));
  }

  #[test]
  fn a_held_press_counts_once_however_long_it_repeats() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(2, SPAN);
    assert!(!window.key(Key::Launcher, KEY_DOWN, clock.at(0)));
    for ms in (250..1200).step_by(33) {
      assert!(!window.key(Key::Launcher, KEY_REPEAT, clock.at(ms)), "repeat at {ms}ms");
    }
    assert!(!window.key(Key::Launcher, KEY_UP, clock.at(1200)));
    assert!(!tap(&mut window, &clock, 1300));
  }

  #[test]
  fn back_to_back_long_presses_never_fire() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(2, SPAN);
    for start in [0, 1000, 2000] {
      assert!(!window.key(Key::Launcher, KEY_DOWN, clock.at(start)));
      assert!(!window.key(Key::Launcher, KEY_REPEAT, clock.at(start + 250)));
      assert!(!window.key(Key::Launcher, KEY_UP, clock.at(start + 900)));
    }
  }

  #[test]
  fn every_press_after_a_fire_counts_toward_the_next() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(2, SPAN);
    let fired: Vec<bool> = [0, 100, 200, 300, 400]
      .into_iter()
      .map(|ms| tap(&mut window, &clock, ms))
      .collect();
    assert_eq!(fired, [false, true, false, true, false]);
  }

  #[test]
  fn five_presses_fire_only_on_the_fifth() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(5, Duration::from_millis(1500));
    let fired: Vec<bool> = [0, 200, 400, 600, 800]
      .into_iter()
      .map(|ms| tap(&mut window, &clock, ms))
      .collect();
    assert_eq!(fired, [false, false, false, false, true]);
  }

  #[test]
  fn the_window_slides_so_old_presses_age_out() {
    let clock = Clock(Instant::now());
    let mut window = PressWindow::new(5, Duration::from_millis(1500));
    for ms in [0, 400, 800, 1200] {
      assert!(!tap(&mut window, &clock, ms));
    }
    assert!(!tap(&mut window, &clock, 1600));
    assert!(tap(&mut window, &clock, 1700));
  }
}
