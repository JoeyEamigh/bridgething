use std::path::Path;

use super::{DisplayConfig, DisplayError};

pub async fn apply_chip_config(config: &DisplayConfig) {
  let calibscale = config.als_path.with_file_name("in_intensity0_calibscale");
  let integ = config.als_path.with_file_name("in_intensity0_integration_time");
  if let Err(err) = tokio::fs::write(&integ, format!("{:.6}\n", config.integration_time_s)).await {
    tracing::warn!(
      "display: failed to write integration_time={} to {}: {err}",
      config.integration_time_s,
      integ.display(),
    );
  }
  if let Err(err) = tokio::fs::write(&calibscale, format!("{}\n", config.gain)).await {
    tracing::warn!(
      "display: failed to write gain={} to {}: {err}",
      config.gain,
      calibscale.display(),
    );
  }
}

pub async fn read_raw(path: &Path) -> Result<u32, DisplayError> {
  let bytes = tokio::fs::read(path).await?;
  Ok(parse_uint(&bytes))
}

pub async fn read_max_brightness(dir: &Path) -> Result<u32, DisplayError> {
  let path = dir.join("max_brightness");
  if !tokio::fs::try_exists(&path).await? {
    return Err(DisplayError::BacklightAbsent(dir.to_path_buf()));
  }
  let bytes = tokio::fs::read(&path).await?;
  Ok(parse_uint(&bytes))
}

pub async fn read_actual_brightness(dir: &Path) -> Result<u32, DisplayError> {
  let bytes = tokio::fs::read(dir.join("actual_brightness")).await?;
  Ok(parse_uint(&bytes))
}

pub async fn write_brightness(dir: &Path, ticks: u32) -> Result<(), DisplayError> {
  tokio::fs::write(dir.join("brightness"), format!("{ticks}\n")).await?;
  Ok(())
}

fn parse_uint(bytes: &[u8]) -> u32 {
  std::str::from_utf8(bytes).unwrap_or("0").trim().parse().unwrap_or(0)
}
