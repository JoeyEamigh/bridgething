use std::{process::Command, sync::Arc};

use tauri::{AppHandle, Manager as _, Runtime};

use crate::{bluetooth::BluetoothRelay, extensions::Extensions};

pub fn leave<R: Runtime>(app: &AppHandle<R>) -> ! {
  if let Some(extensions) = app.try_state::<Arc<Extensions>>() {
    extensions.halt();
  }
  if let Some(bluetooth) = app.try_state::<Arc<BluetoothRelay>>() {
    bluetooth.stop();
  }
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
