use std::{process::Output, time::Duration};

use tokio::process::Command;

const PRINT: &str = "fw_printenv";
const SET: &str = "fw_setenv";
const TOOL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub enum EnvError {
  #[error("io: {0}")]
  Io(#[from] std::io::Error),
  #[error("{command} exited {status}: {stderr}")]
  Failed {
    command: String,
    status: i32,
    stderr: String,
  },
  #[error("{command} did not finish within {seconds}s")]
  TimedOut { command: String, seconds: u64 },
  #[error("{key} is still set after clearing it")]
  StillSet { key: String },
}

pub async fn read(key: &str) -> Result<String, EnvError> {
  let out = run(PRINT, &["-n", key], TOOL_TIMEOUT).await?;
  Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub async fn set(key: &str, value: &str) -> Result<(), EnvError> {
  run(SET, &[key, value], TOOL_TIMEOUT).await?;
  Ok(())
}

pub async fn unset(key: &str) -> Result<(), EnvError> {
  run(SET, &[key], TOOL_TIMEOUT).await?;
  if !read(key).await?.trim().is_empty() {
    return Err(EnvError::StillSet { key: key.into() });
  }
  Ok(())
}

async fn run(tool: &str, args: &[&str], limit: Duration) -> Result<Output, EnvError> {
  let command = || {
    std::iter::once(tool)
      .chain(args.iter().copied())
      .collect::<Vec<_>>()
      .join(" ")
  };
  let finished = Command::new(tool).args(args).kill_on_drop(true).output();
  let out = tokio::time::timeout(limit, finished)
    .await
    .map_err(|_| EnvError::TimedOut {
      command: command(),
      seconds: limit.as_secs(),
    })??;
  if !out.status.success() {
    return Err(EnvError::Failed {
      command: command(),
      status: out.status.code().unwrap_or(-1),
      stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
    });
  }
  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn a_tool_that_never_finishes_times_out_instead_of_hanging_the_caller() {
    let stuck = run("sleep", &["600"], Duration::from_millis(100)).await;
    assert!(
      matches!(&stuck, Err(EnvError::TimedOut { command, .. }) if command == "sleep 600"),
      "{stuck:?}"
    );
  }

  #[tokio::test]
  async fn a_tool_that_exits_nonzero_reports_its_status_and_stderr() {
    let failed = run("sh", &["-c", "echo locked >&2; exit 3"], TOOL_TIMEOUT).await;
    assert!(
      matches!(&failed, Err(EnvError::Failed { status: 3, stderr, .. }) if stderr == "locked"),
      "{failed:?}"
    );
  }

  #[tokio::test]
  async fn a_tool_that_succeeds_returns_what_it_printed() {
    let out = run("sh", &["-c", "printf 1"], TOOL_TIMEOUT).await.expect("ran");
    assert_eq!(out.stdout, b"1");
  }
}
