use std::sync::Arc;

use tauri::{
  AppHandle, Emitter, Manager, Runtime, Window, WindowEvent,
  image::Image,
  menu::{Menu, MenuItem},
  tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::{
  hints::{RESYNC, Visibility},
  shell::Shell,
};

pub const MAIN_WINDOW: &str = "main";

#[derive(Debug, thiserror::Error)]
pub enum TrayError {
  #[error("the tray icon could not be created: {0}")]
  Build(#[from] tauri::Error),
}

#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");
#[cfg(target_os = "linux")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-linux.png");
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/32x32.png");

pub fn install<R: Runtime>(app: &AppHandle<R>) -> Result<(), TrayError> {
  let show = MenuItem::with_id(app, "show", "open bridgething", true, None::<&str>)?;
  let quit = MenuItem::with_id(app, "quit", "quit", true, None::<&str>)?;
  let menu = Menu::with_items(app, &[&show, &quit])?;

  TrayIconBuilder::with_id("main")
    .icon(Image::from_bytes(TRAY_ICON)?)
    .icon_as_template(true)
    .menu(&menu)
    .show_menu_on_left_click(false)
    .on_menu_event(|app, event| match event.id.as_ref() {
      "show" => present(app),
      "quit" => crate::process::leave(app),
      _ => {}
    })
    .on_tray_icon_event(|tray, event| {
      if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
      } = event
      {
        present(tray.app_handle());
      }
    })
    .build(app)?;

  Ok(())
}

pub fn present<R: Runtime>(app: &AppHandle<R>) {
  let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
    return;
  };

  tracing::debug!("the shell window is coming to the front");
  dock(app, true);
  let _ = window.unminimize();
  let _ = window.show();
  let _ = window.set_focus();
  watch(app, true);
  let _ = app.emit(RESYNC, ());

  if let Some(shell) = app.try_state::<Arc<Shell>>() {
    let shell = Arc::clone(&shell);
    tauri::async_runtime::spawn(async move {
      shell.session().resumed().await;
      shell.session().time_changed().await;
    });
  }
}

pub fn dismiss<R: Runtime>(app: &AppHandle<R>) {
  tracing::debug!("the shell window is going tray-only");
  if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
    let _ = window.hide();
  }
  watch(app, false);
  dock(app, false);
}

pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
  if window.label() != MAIN_WINDOW {
    return;
  }
  match event {
    WindowEvent::CloseRequested { api, .. } => {
      api.prevent_close();
      dismiss(window.app_handle());
    }
    WindowEvent::ThemeChanged(theme) => crate::theme::paint(window.app_handle(), *theme),
    WindowEvent::Resized(_) => {
      let onscreen = window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false);
      watch(window.app_handle(), onscreen);
    }
    _ => {}
  }
}

fn watch<R: Runtime>(app: &AppHandle<R>, visible: bool) {
  if let Some(state) = app.try_state::<Visibility>() {
    state.set(visible);
  }
}

#[cfg(target_os = "macos")]
fn dock<R: Runtime>(app: &AppHandle<R>, visible: bool) {
  let policy = if visible {
    tauri::ActivationPolicy::Regular
  } else {
    tauri::ActivationPolicy::Accessory
  };
  let _ = app.set_activation_policy(policy);
}

#[cfg(not(target_os = "macos"))]
fn dock<R: Runtime>(_app: &AppHandle<R>, _visible: bool) {}
