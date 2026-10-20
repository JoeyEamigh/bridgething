pub use wide::f32x4 as Lane;

pub const LANES: usize = 4;
pub const PER_TILE: usize = 6;
pub const TILE: usize = LANES * PER_TILE;

pub fn lanes(values: &[f32]) -> Vec<Lane> {
  values
    .chunks(LANES)
    .map(|chunk| {
      let mut lane = [0.0; LANES];
      lane[..chunk.len()].copy_from_slice(chunk);
      Lane::from(lane)
    })
    .collect()
}

pub fn tile(values: &[f32], out_channels: usize, stride: usize) -> Vec<Lane> {
  let tiles = out_channels.div_ceil(TILE);
  let mut packed = vec![Lane::ZERO; tiles * stride * PER_TILE];
  for tile in 0..tiles {
    let width = TILE.min(out_channels - tile * TILE);
    for step in 0..stride {
      let from = step * out_channels + tile * TILE;
      let to = (tile * stride + step) * PER_TILE;
      for (slot, lane) in packed[to..to + PER_TILE]
        .iter_mut()
        .zip(lanes(&values[from..from + width]))
      {
        *slot = lane;
      }
    }
  }
  packed
}

#[inline(always)]
pub fn accumulate(accumulator: &mut [Lane; PER_TILE], taps: &[Lane], value: f32) {
  let value = Lane::splat(value);
  for (accumulator, tap) in accumulator.iter_mut().zip(taps) {
    *accumulator = tap.mul_add(value, *accumulator);
  }
}

#[inline(always)]
pub fn spill(accumulator: &[Lane; PER_TILE], out: &mut [f32]) {
  let mut tile = [0.0; TILE];
  for (slot, lane) in tile.as_chunks_mut::<LANES>().0.iter_mut().zip(accumulator) {
    slot.copy_from_slice(lane.as_array());
  }
  out.copy_from_slice(&tile[..out.len()]);
}
