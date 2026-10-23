use std::{ffi::OsString, process::Command, sync::Arc};

use tauri::{AppHandle, Manager as _, Runtime};

use crate::{AUTOSTART_ARG, extensions::Extensions, hints::Visibility};

pub fn release<R: Runtime>(app: &AppHandle<R>) {
  if let Some(extensions) = app.try_state::<Arc<Extensions>>() {
    extensions.halt();
  }
  #[cfg(any(target_os = "linux", target_os = "windows"))]
  if let Some(sidecar) = crate::backends::ffmpeg::Sidecar::bundled() {
    sidecar.halt();
  }
  app.cleanup_before_exit();
}

pub fn leave<R: Runtime>(app: &AppHandle<R>) -> ! {
  release(app);
  unsafe { libc::_exit(0) }
}

pub fn restart<R: Runtime>(app: &AppHandle<R>) -> ! {
  let resident = app.try_state::<Visibility>().is_some_and(|visible| !visible.get());
  let args = std::env::args_os()
    .skip(1)
    .filter(|arg| arg != AUTOSTART_ARG)
    .chain(resident.then(|| OsString::from(AUTOSTART_ARG)));
  match tauri::process::current_binary(&app.env()) {
    Ok(binary) => {
      if let Err(err) = Command::new(&binary).args(args).spawn() {
        tracing::error!(binary = %binary.display(), %err, "the replacement process did not spawn");
      }
    }
    Err(err) => tracing::error!(%err, "the running binary did not resolve; the app cannot come back"),
  }
  leave(app)
}
