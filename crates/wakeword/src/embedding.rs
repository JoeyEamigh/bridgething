use crate::{
  Error,
  blob::{KIND_FEATURES, Reader, tag},
  lane::{self, Lane, PER_TILE, TILE},
};

const BINS: usize = 2;
const _: () = assert!(
  BINS == 2,
  "the kernel names both output bins rather than looping over them"
);

#[derive(Default)]
struct Volume {
  data: Vec<f32>,
  frames: usize,
  bins: usize,
  channels: usize,
}

impl Volume {
  fn zeroed(frames: usize, bins: usize, channels: usize) -> Self {
    Self {
      data: vec![0.0; frames * bins * channels],
      frames,
      bins,
      channels,
    }
  }

  fn shape(&mut self, frames: usize, bins: usize, channels: usize) {
    self.frames = frames;
    self.bins = bins;
    self.channels = channels;
    self.data.clear();
    self.data.resize(frames * bins * channels, 0.0);
  }

  fn at(&self, frame: usize, bin: usize) -> &[f32] {
    let start = (frame * self.bins + bin) * self.channels;
    &self.data[start..start + self.channels]
  }

  fn at_mut(&mut self, frame: usize, bin: usize) -> &mut [f32] {
    let start = (frame * self.bins + bin) * self.channels;
    &mut self.data[start..start + self.channels]
  }
}

struct Conv {
  out_channels: usize,
  in_channels: usize,
  kernel_frames: usize,
  kernel_bins: usize,
  pad: usize,
  weights: Vec<Lane>,
  bias: Vec<Lane>,
}

enum Op {
  Conv(Conv),
  Activation {
    slope: f32,
    floor: f32,
  },
  MaxPool {
    pool_frames: usize,
    pool_bins: usize,
    frame_stride: usize,
    bin_stride: usize,
  },
  Cache {
    index: usize,
  },
}

struct State {
  x: Volume,
  y: Volume,
  padded: Volume,
  caches: Vec<Volume>,
}

pub struct Embedding {
  ops: Vec<Op>,
  state: State,
  frames_per_call: usize,
  mel_bins: usize,
  embedding_dim: usize,
  embeddings_per_call: usize,
}

impl Embedding {
  pub fn load(path: &std::path::Path) -> Result<Self, Error> {
    let fail = |reason: String| Error::Model(path.display().to_string(), reason);
    let bytes = std::fs::read(path).map_err(|err| fail(err.to_string()))?;
    Self::parse(&bytes).map_err(fail)
  }

  pub fn from_bytes(bytes: &[u8], label: &str) -> Result<Self, Error> {
    Self::parse(bytes).map_err(|reason| Error::Model(label.to_string(), reason))
  }

  fn parse(bytes: &[u8]) -> Result<Self, String> {
    let (mut reader, params, count) = Reader::open(bytes, KIND_FEATURES)?;
    let [frames_per_call, mel_bins, embedding_dim, embeddings_per_call] = params;

    let mut ops = Vec::with_capacity(count);
    let mut caches = Vec::new();
    for _ in 0..count {
      ops.push(match reader.u32()? {
        tag::CONV => {
          let out_channels = reader.usize()?;
          let in_channels = reader.usize()?;
          let kernel_frames = reader.usize()?;
          let kernel_bins = reader.usize()?;
          let pad = reader.usize()?;
          let biased = reader.u32()? != 0;
          let weights = reader.floats(kernel_frames * kernel_bins * in_channels * out_channels)?;
          let mut bias = if biased {
            reader.floats(out_channels)?
          } else {
            vec![0.0; out_channels]
          };
          bias.resize(out_channels.div_ceil(TILE) * TILE, 0.0);
          Op::Conv(Conv {
            bias: lane::lanes(&bias),
            weights: lane::tile(&weights, out_channels, kernel_frames * kernel_bins * in_channels),
            out_channels,
            in_channels,
            kernel_frames,
            kernel_bins,
            pad,
          })
        }
        tag::ACTIVATION => Op::Activation {
          slope: reader.f32()?,
          floor: reader.f32()?,
        },
        tag::MAX_POOL => Op::MaxPool {
          pool_frames: reader.usize()?,
          pool_bins: reader.usize()?,
          frame_stride: reader.usize()?,
          bin_stride: reader.usize()?,
        },
        tag::CACHE => {
          let frames = reader.usize()?;
          let bins = reader.usize()?;
          let channels = reader.usize()?;
          caches.push(Volume::zeroed(frames, bins, channels));
          Op::Cache {
            index: caches.len() - 1,
          }
        }
        other => return Err(format!("op {other} is not one this stack can hold")),
      });
    }
    if !reader.at_end() {
      return Err("trailing bytes past the last op".into());
    }

    Ok(Self {
      ops,
      state: State {
        x: Volume::default(),
        y: Volume::default(),
        padded: Volume::default(),
        caches,
      },
      frames_per_call,
      mel_bins,
      embedding_dim,
      embeddings_per_call,
    })
  }

  pub fn frames_per_call(&self) -> usize {
    self.frames_per_call
  }

  pub fn embeddings_per_call(&self) -> usize {
    self.embeddings_per_call
  }

  pub fn embedding_dim(&self) -> usize {
    self.embedding_dim
  }

  pub fn reset(&mut self) {
    for cache in &mut self.state.caches {
      cache.data.fill(0.0);
    }
  }

  pub fn run(&mut self, mel: &[f32]) -> Result<&[f32], Error> {
    if mel.len() != self.frames_per_call * self.mel_bins {
      return Err(Error::Infer(format!(
        "expected {} mel values, got {}",
        self.frames_per_call * self.mel_bins,
        mel.len()
      )));
    }

    self.state.x.shape(self.frames_per_call, self.mel_bins, 1);
    self.state.x.data.copy_from_slice(mel);
    for op in &self.ops {
      op.apply(&mut self.state)?;
    }

    let out = &self.state.x;
    if out.frames != self.embeddings_per_call || out.bins != 1 || out.channels != self.embedding_dim {
      return Err(Error::Infer(format!(
        "the stack ended at {}x{}x{}, not {}x1x{}",
        out.frames, out.bins, out.channels, self.embeddings_per_call, self.embedding_dim
      )));
    }
    Ok(&out.data)
  }
}

impl Op {
  fn apply(&self, state: &mut State) -> Result<(), Error> {
    let State { x, y, padded, caches } = state;
    match self {
      Op::Activation { slope, floor } => {
        for value in &mut x.data {
          let leaky = if *value < 0.0 { slope * *value } else { *value };
          *value = leaky.max(*floor);
        }
        return Ok(());
      }
      Op::Conv(conv) => conv.apply(x, y, padded)?,
      Op::MaxPool {
        pool_frames,
        pool_bins,
        frame_stride,
        bin_stride,
      } => {
        if x.frames < *pool_frames || x.bins < *pool_bins {
          return Err(Error::Infer(format!(
            "pooling {pool_frames}x{pool_bins} over a {}x{} volume",
            x.frames, x.bins
          )));
        }
        y.shape(
          (x.frames - pool_frames) / frame_stride + 1,
          (x.bins - pool_bins) / bin_stride + 1,
          x.channels,
        );
        for frame in 0..y.frames {
          for bin in 0..y.bins {
            let out = y.at_mut(frame, bin);
            out.copy_from_slice(x.at(frame * frame_stride, bin * bin_stride));
            for tap_frame in 0..*pool_frames {
              for tap_bin in 0..*pool_bins {
                let src = x.at(frame * frame_stride + tap_frame, bin * bin_stride + tap_bin);
                for (slot, value) in out.iter_mut().zip(src) {
                  *slot = slot.max(*value);
                }
              }
            }
          }
        }
      }
      Op::Cache { index } => {
        let cache = &mut caches[*index];
        if cache.bins != x.bins || cache.channels != x.channels {
          return Err(Error::Infer(format!(
            "cache {index} holds {}x{} channels, the stack reached it at {}x{}",
            cache.bins, cache.channels, x.bins, x.channels
          )));
        }
        y.shape(cache.frames + x.frames, x.bins, x.channels);
        y.data[..cache.data.len()].copy_from_slice(&cache.data);
        y.data[cache.data.len()..].copy_from_slice(&x.data);
        let kept = y.data.len() - cache.data.len();
        cache.data.copy_from_slice(&y.data[kept..]);
      }
    }

    std::mem::swap(x, y);
    Ok(())
  }
}

impl Conv {
  fn apply(&self, x: &Volume, y: &mut Volume, padded: &mut Volume) -> Result<(), Error> {
    if x.channels != self.in_channels || x.frames < self.kernel_frames || x.bins + 2 * self.pad < self.kernel_bins {
      return Err(Error::Infer(format!(
        "a {}x{} kernel over {} channels met a {}x{} volume of {}",
        self.kernel_frames, self.kernel_bins, self.in_channels, x.frames, x.bins, x.channels
      )));
    }

    let frames = x.frames + 1 - self.kernel_frames;
    let bins = x.bins + 2 * self.pad + 1 - self.kernel_bins;
    let blocks = bins.div_ceil(BINS);
    y.shape(frames, bins, self.out_channels);

    padded.shape(x.frames, blocks * BINS + self.kernel_bins - 1, x.channels);
    for frame in 0..x.frames {
      let row = frame * x.bins * x.channels;
      let into = (frame * padded.bins + self.pad) * x.channels;
      padded.data[into..into + x.bins * x.channels].copy_from_slice(&x.data[row..row + x.bins * x.channels]);
    }

    let taps = self.kernel_frames * self.kernel_bins;
    let run = self.in_channels * PER_TILE;
    let stride = padded.bins * padded.channels;

    for frame in 0..frames {
      for block in 0..blocks {
        for tile in 0..self.out_channels.div_ceil(TILE) {
          let mut accumulator = [[Lane::ZERO; PER_TILE]; BINS];
          for slot in &mut accumulator {
            slot.copy_from_slice(&self.bias[tile * PER_TILE..(tile + 1) * PER_TILE]);
          }

          for tap_frame in 0..self.kernel_frames {
            let row = &padded.data[(frame + tap_frame) * stride..(frame + tap_frame + 1) * stride];
            for tap_bin in 0..self.kernel_bins {
              let tap = tap_frame * self.kernel_bins + tap_bin;
              let weights = &self.weights[((tile * taps) + tap) * run..][..run];
              let at = (block * BINS + tap_bin) * padded.channels;
              let (left, right) = (
                &row[at..at + padded.channels],
                &row[at + padded.channels..][..padded.channels],
              );

              for ((left, right), taps) in left.iter().zip(right).zip(weights.as_chunks::<PER_TILE>().0) {
                lane::accumulate(&mut accumulator[0], taps, *left);
                lane::accumulate(&mut accumulator[1], taps, *right);
              }
            }
          }

          for (bin, accumulator) in accumulator.iter().enumerate() {
            let bin = block * BINS + bin;
            if bin == bins {
              break;
            }
            let at = (frame * bins + bin) * self.out_channels + tile * TILE;
            let width = TILE.min(self.out_channels - tile * TILE);
            lane::spill(accumulator, &mut y.data[at..at + width]);
          }
        }
      }
    }
    Ok(())
  }
}
