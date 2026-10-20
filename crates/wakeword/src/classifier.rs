use crate::{
  Error,
  blob::{KIND_CLASSIFIER, Reader, tag},
  lane::{self, Lane, PER_TILE, TILE},
};

const WINDOWS: usize = 2;
const _: () = assert!(
  WINDOWS == 2,
  "the kernel names both windows rather than looping over them"
);

struct Gemm {
  out_dim: usize,
  in_dim: usize,
  weights: Vec<Lane>,
  bias: Vec<Lane>,
}

struct Norm {
  size: usize,
  epsilon: f32,
  scale: Vec<f32>,
  bias: Vec<f32>,
}

enum Op {
  Gemm(Gemm),
  LayerNorm(Norm),
  Relu,
  Sigmoid,
}

pub struct Classifier {
  ops: Vec<Op>,
  window_frames: usize,
  embedding_dim: usize,
  x: Vec<f32>,
  y: Vec<f32>,
}

impl Classifier {
  pub fn load(path: &std::path::Path) -> Result<Self, Error> {
    let fail = |reason: String| Error::Model(path.display().to_string(), reason);
    let bytes = std::fs::read(path).map_err(|err| fail(err.to_string()))?;
    Self::parse(&bytes).map_err(fail)
  }

  fn parse(bytes: &[u8]) -> Result<Self, String> {
    let (mut reader, params, count) = Reader::open(bytes, KIND_CLASSIFIER)?;
    let [window_frames, embedding_dim, _, _] = params;

    let mut ops = Vec::with_capacity(count);
    for _ in 0..count {
      ops.push(match reader.u32()? {
        tag::GEMM => {
          let out_dim = reader.usize()?;
          let in_dim = reader.usize()?;
          let weights = reader.floats(in_dim * out_dim)?;
          let mut bias = reader.floats(out_dim)?;
          bias.resize(out_dim.div_ceil(TILE) * TILE, 0.0);
          Op::Gemm(Gemm {
            weights: lane::tile(&weights, out_dim, in_dim),
            bias: lane::lanes(&bias),
            out_dim,
            in_dim,
          })
        }
        tag::LAYER_NORM => {
          let size = reader.usize()?;
          let epsilon = reader.f32()?;
          Op::LayerNorm(Norm {
            size,
            epsilon,
            scale: reader.floats(size)?,
            bias: reader.floats(size)?,
          })
        }
        tag::RELU => Op::Relu,
        tag::SIGMOID => Op::Sigmoid,
        other => return Err(format!("op {other} is not one this classifier can hold")),
      });
    }
    if !reader.at_end() {
      return Err("trailing bytes past the last op".into());
    }

    Ok(Self {
      ops,
      window_frames,
      embedding_dim,
      x: Vec::new(),
      y: Vec::new(),
    })
  }

  pub fn window_frames(&self) -> usize {
    self.window_frames
  }

  pub fn embedding_dim(&self) -> usize {
    self.embedding_dim
  }

  pub fn score(&mut self, windows: &[f32], count: usize, scores: &mut Vec<f32>) -> Result<(), Error> {
    let width = self.window_frames * self.embedding_dim;
    if windows.len() != count * width {
      return Err(Error::Infer(format!(
        "expected {count} windows of {width} values, got {}",
        windows.len()
      )));
    }

    self.x.clear();
    self.x.extend_from_slice(windows);
    let mut row = width;
    for op in &self.ops {
      row = op.apply(&mut self.x, &mut self.y, count, row)?;
    }

    if row != 1 {
      return Err(Error::Infer(format!("the classifier ended {row} values wide, not 1")));
    }
    scores.extend_from_slice(&self.x);
    Ok(())
  }
}

impl Op {
  fn apply(&self, x: &mut Vec<f32>, y: &mut Vec<f32>, count: usize, row: usize) -> Result<usize, Error> {
    match self {
      Op::Relu => {
        for value in x.iter_mut() {
          *value = value.max(0.0);
        }
        Ok(row)
      }
      Op::Sigmoid => {
        for value in x.iter_mut() {
          *value = 1.0 / (1.0 + (-*value).exp());
        }
        Ok(row)
      }
      Op::LayerNorm(norm) => {
        if norm.size != row {
          return Err(Error::Infer(format!(
            "normalising {} values over rows of {row}",
            norm.size
          )));
        }
        for values in x.chunks_exact_mut(row) {
          let mean = values.iter().sum::<f32>() / row as f32;
          let variance = values.iter().map(|value| (value - mean) * (value - mean)).sum::<f32>() / row as f32;
          let scale = 1.0 / (variance + norm.epsilon).sqrt();
          for ((value, weight), bias) in values.iter_mut().zip(&norm.scale).zip(&norm.bias) {
            *value = (*value - mean) * scale * weight + bias;
          }
        }
        Ok(row)
      }
      Op::Gemm(gemm) => {
        if gemm.in_dim != row {
          return Err(Error::Infer(format!("a {}-wide matrix met rows of {row}", gemm.in_dim)));
        }
        y.clear();
        y.resize(count * gemm.out_dim, 0.0);

        for group in (0..count).step_by(WINDOWS) {
          let second = (group + 1).min(count - 1);
          let (left, right) = (&x[group * row..][..row], &x[second * row..][..row]);

          for tile in 0..gemm.out_dim.div_ceil(TILE) {
            let mut accumulator = [[Lane::ZERO; PER_TILE]; WINDOWS];
            for slot in &mut accumulator {
              slot.copy_from_slice(&gemm.bias[tile * PER_TILE..(tile + 1) * PER_TILE]);
            }

            let weights = &gemm.weights[tile * gemm.in_dim * PER_TILE..][..gemm.in_dim * PER_TILE];
            for ((left, right), taps) in left.iter().zip(right).zip(weights.as_chunks::<PER_TILE>().0) {
              lane::accumulate(&mut accumulator[0], taps, *left);
              lane::accumulate(&mut accumulator[1], taps, *right);
            }

            for (window, accumulator) in accumulator.iter().enumerate() {
              let window = group + window;
              if window == count {
                break;
              }
              let at = window * gemm.out_dim + tile * TILE;
              let width = TILE.min(gemm.out_dim - tile * TILE);
              lane::spill(accumulator, &mut y[at..at + width]);
            }
          }
        }
        std::mem::swap(x, y);
        Ok(gemm.out_dim)
      }
    }
  }
}
