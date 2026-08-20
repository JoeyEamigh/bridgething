use std::process::Command;

use tauri::{AppHandle, Runtime};

pub fn leave<R: Runtime>(app: &AppHandle<R>) -> ! {
  app.cleanup_before_exit();
  unsafe { libc::_exit(0) }
}

pub fn restart<R: Runtime>(app: &AppHandle<R>) -> ! {
  match std::env::current_exe() {
    Ok(binary) => {
      if let Err(err) = Command::new(&binary).args(std::env::args_os().skip(1)).spawn() {
        tracing::error!(binary = %binary.display(), %err, "the replacement process did not spawn");
      }
    }
    Err(err) => tracing::error!(%err, "the running binary did not resolve; the app cannot come back"),
  }
  leave(app)
}
