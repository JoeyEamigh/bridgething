use std::sync::Arc;

use tauri::{
  AppHandle, Emitter, Manager, Runtime, Window, WindowEvent,
  image::Image,
  menu::{Menu, MenuItem, PredefinedMenuItem},
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

const TRAY: &str = "main";

fn menu<R: Runtime>(app: &AppHandle<R>, ready: Option<&str>) -> tauri::Result<Menu<R>> {
  let show = MenuItem::with_id(app, "show", "open bridgething", true, None::<&str>)?;
  let quit = MenuItem::with_id(app, "quit", "quit", true, None::<&str>)?;
  match ready {
    Some(version) => {
      let update = MenuItem::with_id(
        app,
        "update",
        format!("restart to update to v{version}"),
        true,
        None::<&str>,
      )?;
      Menu::with_items(app, &[&update, &PredefinedMenuItem::separator(app)?, &show, &quit])
    }
    None => Menu::with_items(app, &[&show, &quit]),
  }
}

pub fn install<R: Runtime>(app: &AppHandle<R>) -> Result<(), TrayError> {
  TrayIconBuilder::with_id(TRAY)
    .icon(Image::from_bytes(TRAY_ICON)?)
    .icon_as_template(true)
    .tooltip("bridgething")
    .menu(&menu(app, None)?)
    .show_menu_on_left_click(false)
    .on_menu_event(|app, event| match event.id.as_ref() {
      "update" => crate::self_update::restart_into(app),
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

pub fn offer<R: Runtime>(app: &AppHandle<R>, ready: Option<&str>) {
  let Some(tray) = app.tray_by_id(TRAY) else {
    return;
  };
  let offered = menu(app, ready)
    .and_then(|menu| tray.set_menu(Some(menu)))
    .and_then(|()| {
      tray.set_tooltip(Some(match ready {
        Some(version) => format!("bridgething: v{version} is ready, restart to update"),
        None => "bridgething".to_owned(),
      }))
    });
  if let Err(error) = offered.and_then(|()| tray.set_title(ready.map(|_| "update"))) {
    tracing::warn!(%error, "the tray did not take the update offer");
  }
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
