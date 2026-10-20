pub const MAGIC: [u8; 4] = *b"BTWW";
pub const VERSION: u32 = 1;

pub const KIND_FEATURES: u32 = 0;
pub const KIND_CLASSIFIER: u32 = 1;

pub mod tag {
  pub const CONV: u32 = 1;
  pub const ACTIVATION: u32 = 2;
  pub const MAX_POOL: u32 = 3;
  pub const CACHE: u32 = 4;
  pub const GEMM: u32 = 5;
  pub const LAYER_NORM: u32 = 6;
  pub const RELU: u32 = 7;
  pub const SIGMOID: u32 = 8;
}

pub const PARAMS: usize = 4;

pub struct Reader<'a> {
  bytes: &'a [u8],
  at: usize,
}

impl<'a> Reader<'a> {
  pub fn open(bytes: &'a [u8], kind: u32) -> Result<(Self, [usize; PARAMS], usize), String> {
    let mut reader = Self { bytes, at: 0 };
    let magic = reader.bytes.get(..4).ok_or_else(|| "not a model file".to_string())?;
    if magic != MAGIC {
      return Err("not a model file".into());
    }
    reader.at = 4;

    let version = reader.u32()?;
    if version != VERSION {
      return Err(format!("format version {version}, this build reads {VERSION}"));
    }
    let found = reader.u32()?;
    if found != kind {
      return Err(format!("holds kind {found}, not kind {kind}"));
    }

    let mut params = [0; PARAMS];
    for param in &mut params {
      *param = reader.usize()?;
    }
    let ops = reader.usize()?;
    Ok((reader, params, ops))
  }

  pub fn u32(&mut self) -> Result<u32, String> {
    let end = self.at + 4;
    let slot = self
      .bytes
      .get(self.at..end)
      .ok_or_else(|| "file ends mid-value".to_string())?;
    self.at = end;
    Ok(u32::from_le_bytes(slot.try_into().expect("four bytes")))
  }

  pub fn usize(&mut self) -> Result<usize, String> {
    self.u32().map(|value| value as usize)
  }

  pub fn f32(&mut self) -> Result<f32, String> {
    self.u32().map(f32::from_bits)
  }

  pub fn floats(&mut self, count: usize) -> Result<Vec<f32>, String> {
    let end = self.at + count * 4;
    let slot = self
      .bytes
      .get(self.at..end)
      .ok_or_else(|| "file ends mid-tensor".to_string())?;
    self.at = end;
    Ok(
      slot
        .as_chunks::<4>()
        .0
        .iter()
        .map(|value| f32::from_le_bytes(*value))
        .collect(),
    )
  }

  pub fn at_end(&self) -> bool {
    self.at == self.bytes.len()
  }
}
