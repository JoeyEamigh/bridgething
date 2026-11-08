use std::collections::VecDeque;

use libbridgething::BrightnessMode;

#[derive(Debug, Clone, Copy)]
pub struct Curve {
  pub raw_at_max: f64,
  pub min_brightness: u32,
  pub dim_knee: u32,
  pub median_window: usize,
}

#[derive(Debug)]
pub struct Sensor {
  curve: Curve,
  samples: VecDeque<u32>,
}

impl Sensor {
  pub fn new(curve: Curve) -> Self {
    Self {
      curve,
      samples: VecDeque::with_capacity(curve.median_window.max(1)),
    }
  }

  pub fn push(&mut self, raw: u32) {
    if self.samples.len() == self.curve.median_window {
      self.samples.pop_front();
    }
    self.samples.push_back(raw);
  }

  pub fn median(&self) -> Option<u32> {
    if self.samples.len() < self.curve.median_window {
      return None;
    }
    let mut sorted: Vec<u32> = self.samples.iter().copied().collect();
    sorted.sort_unstable();
    Some(sorted[sorted.len() / 2])
  }

  pub fn target(&self, max: u32) -> Option<u32> {
    self.median().map(|raw| target_for_raw(&self.curve, raw, max))
  }

  pub fn ambient_level(&self) -> u8 {
    self.median().map_or(0, |raw| ambient_for_raw(&self.curve, raw))
  }
}

fn light(curve: &Curve, raw: u32) -> f64 {
  let log_max = (1.0 + curve.raw_at_max).log10();
  ((1.0 + raw as f64).log10() / log_max).clamp(0.0, 1.0)
}

pub fn target_for_raw(curve: &Curve, raw: u32, max: u32) -> u32 {
  if max == 0 {
    return 0;
  }
  let min_t = curve.min_brightness.min(max);
  if raw <= curve.dim_knee {
    return min_t;
  }
  let span = (max - min_t) as f64;
  min_t + (span * light(curve, raw)).round() as u32
}

pub fn ambient_for_raw(curve: &Curve, raw: u32) -> u8 {
  (light(curve, raw) * 100.0).round() as u8
}

pub fn level_to_ticks(level: f32, max: u32) -> u32 {
  (level.clamp(0.0, 1.0) * max as f32 + 0.5) as u32
}

pub fn dimmed_ticks(mode: BrightnessMode, normal: u32, level: f32, max: u32) -> u32 {
  let ticks = match mode {
    BrightnessMode::Auto => (normal as f32 * level.clamp(0.0, 1.0) + 0.5) as u32,
    BrightnessMode::Manual => normal.min(level_to_ticks(level, max)),
  };
  if level > 0.0 && normal > 0 { ticks.max(1) } else { ticks }
}

pub fn ease_step(current: u32, target: u32, ease_pct: f32) -> u32 {
  if current == target {
    return current;
  }
  let diff = target as i32 - current as i32;
  let mag = diff.unsigned_abs();
  let step = (((mag as f32) * ease_pct).round() as u32).clamp(1, mag);
  if diff > 0 { current + step } else { current - step }
}

#[cfg(test)]
mod tests {
  use super::*;

  const CURVE: Curve = Curve {
    raw_at_max: 1500.0,
    min_brightness: 16,
    dim_knee: 3,
    median_window: 3,
  };

  #[test]
  fn a_dimmed_manual_level_is_capped_at_the_dim_level() {
    assert_eq!(dimmed_ticks(BrightnessMode::Manual, 204, 0.25, 255), 64);
    assert_eq!(
      dimmed_ticks(BrightnessMode::Manual, 26, 0.25, 255),
      26,
      "a level already under the cap stays put"
    );
  }

  #[test]
  fn a_dimmed_auto_level_is_scaled_by_the_dim_level() {
    assert_eq!(dimmed_ticks(BrightnessMode::Auto, 255, 0.5, 255), 128);
    assert_eq!(dimmed_ticks(BrightnessMode::Auto, 16, 0.5, 255), 8);
  }

  #[test]
  fn a_dim_level_of_zero_goes_dark_in_either_mode() {
    assert_eq!(dimmed_ticks(BrightnessMode::Auto, 255, 0.0, 255), 0);
    assert_eq!(dimmed_ticks(BrightnessMode::Manual, 255, 0.0, 255), 0);
  }

  #[test]
  fn a_tiny_dim_level_keeps_a_lit_panel_at_its_lowest_tick() {
    assert_eq!(dimmed_ticks(BrightnessMode::Auto, 16, 0.01, 255), 1);
    assert_eq!(dimmed_ticks(BrightnessMode::Manual, 128, 0.001, 255), 1);
    assert_eq!(
      dimmed_ticks(BrightnessMode::Manual, 0, 0.5, 255),
      0,
      "a panel set to zero is not lifted by dimming"
    );
  }

  #[test]
  fn levels_map_onto_the_panel_range_and_clamp() {
    assert_eq!(level_to_ticks(0.5, 255), 128);
    assert_eq!(level_to_ticks(1.5, 255), 255);
    assert_eq!(level_to_ticks(-1.0, 255), 0);
    assert_eq!(level_to_ticks(0.42, 255), 107);
  }

  #[test]
  fn the_sensor_waits_for_a_full_window_and_takes_the_median() {
    let mut sensor = Sensor::new(CURVE);
    sensor.push(10);
    sensor.push(1000);
    assert_eq!(sensor.median(), None);
    sensor.push(20);
    assert_eq!(sensor.median(), Some(20));
    sensor.push(5000);
    sensor.push(5000);
    assert_eq!(sensor.median(), Some(5000), "the oldest samples roll out");
  }

  #[test]
  fn the_curve_floors_a_dark_room_and_tops_out_at_full_scale() {
    assert_eq!(target_for_raw(&CURVE, 0, 255), 16);
    assert_eq!(target_for_raw(&CURVE, 3, 255), 16);
    assert_eq!(target_for_raw(&CURVE, 1500, 255), 255);
    assert_eq!(target_for_raw(&CURVE, 100_000, 255), 255);
    assert_eq!(target_for_raw(&CURVE, 100, 0), 0, "no panel, no target");
    let mid = target_for_raw(&CURVE, 40, 255);
    assert!(16 < mid && mid < 255, "{mid}");
  }

  #[test]
  fn room_light_maps_dark_to_zero_and_full_scale_to_a_hundred() {
    assert_eq!(ambient_for_raw(&CURVE, 0), 0);
    assert_eq!(ambient_for_raw(&CURVE, 1500), 100);
    assert_eq!(ambient_for_raw(&CURVE, 100_000), 100);
    let levels: Vec<u8> = [1, 3, 4, 40, 400]
      .into_iter()
      .map(|raw| ambient_for_raw(&CURVE, raw))
      .collect();
    assert!(levels.windows(2).all(|w| w[0] < w[1]), "{levels:?}");
    assert!(levels.iter().all(|l| (1..100).contains(l)), "{levels:?}");
    assert!(
      ambient_for_raw(&CURVE, 4) - ambient_for_raw(&CURVE, 3) <= 4,
      "room light has no step at the backlight floor's knee"
    );
  }

  #[test]
  fn room_light_waits_for_a_full_sensor_window() {
    let mut sensor = Sensor::new(CURVE);
    sensor.push(1500);
    sensor.push(1500);
    assert_eq!(sensor.ambient_level(), 0);
    sensor.push(1500);
    assert_eq!(sensor.ambient_level(), 100);
  }

  #[test]
  fn easing_moves_a_share_of_the_gap_and_at_least_one_tick() {
    assert_eq!(ease_step(100, 100, 0.15), 100);
    assert_eq!(ease_step(0, 100, 0.15), 15);
    assert_eq!(ease_step(100, 0, 0.15), 85);
    assert_eq!(ease_step(10, 11, 0.15), 11);
    assert_eq!(ease_step(10, 9, 0.15), 9);
    assert_eq!(ease_step(0, 2, 1.0), 2, "never overshoots");
  }
}
