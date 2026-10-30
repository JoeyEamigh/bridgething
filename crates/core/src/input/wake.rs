use std::time::{Duration, Instant};

const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;
const KEY_DOWN: i32 = 1;
const MIN_GAP: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
  Buttons,
  Dial,
  Touch,
}

impl Source {
  pub(super) fn device_name(self) -> &'static str {
    match self {
      Self::Buttons => "gpio-keys",
      Self::Dial => "rotary",
      Self::Touch => "tlsc6x",
    }
  }

  pub(super) fn wakes(self, ev_type: u16, value: i32) -> bool {
    match self {
      Self::Buttons => ev_type == EV_KEY && value == KEY_DOWN,
      Self::Dial => ev_type == EV_REL,
      Self::Touch => ev_type == EV_ABS || (ev_type == EV_KEY && value == KEY_DOWN),
    }
  }
}

#[derive(Debug, Default)]
pub(super) struct Throttle {
  last: Option<Instant>,
}

impl Throttle {
  pub(super) fn due(&mut self, now: Instant) -> bool {
    if self
      .last
      .is_some_and(|last| now.saturating_duration_since(last) < MIN_GAP)
    {
      return false;
    }
    self.last = Some(now);
    true
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_button_wakes_on_press_not_release() {
    assert!(Source::Buttons.wakes(EV_KEY, 1));
    assert!(!Source::Buttons.wakes(EV_KEY, 0));
  }

  #[test]
  fn any_turn_of_the_dial_wakes() {
    assert!(Source::Dial.wakes(EV_REL, 1));
    assert!(Source::Dial.wakes(EV_REL, -1));
  }

  #[test]
  fn a_touch_wakes_on_contact_and_on_movement() {
    assert!(Source::Touch.wakes(EV_ABS, 240));
    assert!(Source::Touch.wakes(EV_KEY, 1));
    assert!(!Source::Touch.wakes(EV_KEY, 0));
  }

  #[test]
  fn sync_events_never_wake() {
    for source in [Source::Buttons, Source::Dial, Source::Touch] {
      assert!(!source.wakes(0x00, 0));
    }
  }

  #[test]
  fn a_burst_of_events_wakes_once_per_gap() {
    let t0 = Instant::now();
    let mut throttle = Throttle::default();
    assert!(throttle.due(t0));
    assert!(!throttle.due(t0 + Duration::from_millis(50)));
    assert!(!throttle.due(t0 + Duration::from_millis(199)));
    assert!(throttle.due(t0 + MIN_GAP));
  }
}
