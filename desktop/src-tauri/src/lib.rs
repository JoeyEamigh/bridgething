pub mod autoconnect;
pub mod backends;
pub mod clock;
pub mod commands;
pub mod extensions;
pub mod hints;
pub mod inherited;
pub mod known_device;
pub mod logs;
pub mod process;
pub mod route;
pub mod self_update;
pub mod settings;
pub mod shell;
pub mod theme;
pub mod tray;

use std::sync::Arc;

use bridgething_delivery::discovery::Discovery;
use tauri::Manager;

use crate::{
  extensions::Deps,
  hints::{ENDPOINTS, Hint, HintSink, Visibility, WindowHints},
  route::Route,
  shell::{DesktopPaths, Shell, ShellConfig},
};

const AUTOSTART_ARG: &str = "--autostart";

fn opener<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Arc<dyn Fn(String) -> Result<(), String> + Send + Sync> {
  use tauri_plugin_opener::OpenerExt as _;
  Arc::new(move |url| {
    app
      .opener()
      .open_url(url, None::<&str>)
      .map_err(|error| error.to_string())
  })
}

#[macro_export]
macro_rules! desktop_commands {
  () => {
    tauri::generate_handler![
      $crate::commands::session_snapshot,
      $crate::commands::host_info,
      $crate::commands::capabilities,
      $crate::commands::capability_support,
      $crate::commands::providers,
      $crate::commands::provider_priority,
      $crate::commands::library_provider,
      $crate::commands::peers,
      $crate::commands::now_playing,
      $crate::commands::device_meta,
      $crate::commands::device_auto_resume,
      $crate::commands::device_resume_target,
      $crate::commands::device_log_streaming,
      $crate::commands::device_log_verbosity,
      $crate::commands::voice_model,
      $crate::commands::ota_runs,
      $crate::commands::ota_run_progress,
      $crate::commands::ota_available,
      $crate::commands::ota_poll,
      $crate::commands::webapps,
      $crate::commands::webapp_active,
      $crate::commands::webapp_resource,
      $crate::commands::webapp_slots,
      $crate::commands::webapp_config,
      $crate::commands::webapp_doc,
      $crate::commands::webapp_doc_entry,
      $crate::settings::settings_fetch,
      $crate::settings::settings_authorize,
      $crate::commands::device_logs,
      $crate::commands::export_logs,
      $crate::commands::ota_manifest,
      $crate::commands::endpoints,
      $crate::commands::default_gateway,
      $crate::commands::route,
      $crate::commands::set_route,
      $crate::commands::catalog_sources,
      $crate::commands::add_catalog_source,
      $crate::commands::remove_catalog_source,
      $crate::commands::connect,
      $crate::commands::disconnect,
      $crate::commands::selected_device,
      $crate::commands::select_device,
      $crate::commands::set_provider_priority,
      $crate::commands::connect_provider,
      $crate::commands::disconnect_provider,
      $crate::commands::cancel_provider_auth,
      $crate::commands::complete_provider_auth,
      $crate::commands::set_capability_flags,
      $crate::commands::set_device_auto_resume,
      $crate::commands::set_device_resume_target,
      $crate::commands::set_device_log_streaming,
      $crate::commands::set_device_log_verbosity,
      $crate::commands::set_device_nickname,
      $crate::commands::set_launcher_gesture,
      $crate::commands::switch_webapp,
      $crate::commands::uninstall_webapp,
      $crate::commands::set_webapp_slot,
      $crate::commands::set_webapp_config_field,
      $crate::commands::delete_webapp_config_field,
      $crate::commands::set_webapp_doc,
      $crate::commands::delete_webapp_doc,
      $crate::commands::media_import_ticket,
      $crate::commands::import_picked_media,
      $crate::commands::cancel_media_import,
      $crate::commands::create_blob_handle,
      $crate::commands::append_blob_handle,
      $crate::commands::put_webapp_blob,
      $crate::commands::delete_webapp_blob,
      $crate::commands::webapp_blobs,
      $crate::commands::set_ota_poll_config,
      $crate::commands::apply_ota_update,
      $crate::commands::ota_push_daemon,
      $crate::commands::ota_install_webapp,
      $crate::commands::webapp_bundle_extension,
      $crate::commands::install_webapp_from_url,
      $crate::commands::ota_check_now,
      $crate::commands::ota_dismiss_run,
      $crate::commands::debug_logging,
      $crate::commands::set_debug_logging,
      $crate::commands::known_devices,
      $crate::commands::forget_known_device,
      $crate::commands::extensions,
      $crate::commands::set_extension_enabled,
      $crate::commands::remove_extension,
      $crate::commands::open_extension_data,
      $crate::commands::retry_extension_runtime,
      $crate::self_update::self_update,
      $crate::self_update::check_self_update,
      $crate::self_update::apply_self_update,
      $crate::commands::quit,
    ]
  };
}

pub fn run() {
  let config_dir = DesktopPaths::xdg().ok().map(|paths| paths.config_dir);
  let verbosity = logs::install(config_dir.as_deref());

  let resident = std::env::args().any(|arg| arg == AUTOSTART_ARG);

  tauri::Builder::default()
    .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
      tray::present(app)
    }))
    .plugin(tray::window_state(config_dir.as_deref()))
    .plugin(tauri_plugin_autostart::init(
      tauri_plugin_autostart::MacosLauncher::LaunchAgent,
      Some(vec![AUTOSTART_ARG]),
    ))
    .plugin(tauri_plugin_deep_link::init())
    .plugin(tauri_plugin_opener::init())
    .plugin(tauri_plugin_clipboard_manager::init())
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_updater::Builder::new().build())
    .plugin(tauri_plugin_notification::init())
    .setup(move |app| {
      use tauri_plugin_deep_link::DeepLinkExt as _;

      app.manage(verbosity);
      let authorize = Arc::new(settings::Authorize::default());
      app.manage(Arc::clone(&authorize));
      let browser = Arc::clone(&authorize);
      #[cfg(any(target_os = "linux", target_os = "windows"))]
      if let Err(error) = app.deep_link().register_all() {
        tracing::warn!(%error, "the bridgething url scheme did not register with the desktop");
      }
      app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
          authorize.deliver(&url);
        }
      });
      let visible = if resident {
        Visibility::hidden()
      } else {
        Visibility::shown()
      };
      app.manage(visible.clone());
      let hints = Arc::new(WindowHints::new(app.handle().clone(), visible));
      let config = ShellConfig::from_env()?;
      let paths = DesktopPaths::xdg()?;
      let shell = Shell::create(config, hints.clone())?;
      let extensions = Arc::clone(shell.extensions());
      extensions.spawn(Deps {
        http: shell.http().clone(),
        authorize: Arc::clone(&browser),
        open_url: opener(app.handle().clone()),
        hints: hints.clone(),
      });
      logs::attach(shell.session().log_inbox());
      tauri::async_runtime::block_on(shell.start());
      let updates = self_update::Updates::new(self_update::TauriFeed::new(app.handle()), hints.clone(), {
        let app = app.handle().clone();
        let announced = self_update::Announced::open(&paths.state_dir);
        move |ready| self_update::offer(&app, &announced, ready)
      });
      let online = shell.online();
      let wake = shell.wake();
      let discovery = Discovery::spawn(move |_| {
        hints.emit(Hint::bare(ENDPOINTS));
        wake.notify_one();
      })?;
      autoconnect::spawn(shell.clone(), {
        let discovery = Arc::clone(&discovery);
        move || discovery.endpoints()
      });
      clock::spawn(shell.clone(), Arc::clone(&updates));
      app.manage(shell);
      app.manage(extensions);
      app.manage(discovery);
      app.manage(Route::open(&paths.config_dir));
      tray::install(app.handle())?;
      self_update::listen(app.handle());
      updates.spawn(online);
      app.manage(updates);
      theme::sync(app.handle());
      tray::fit(app.handle());
      if resident {
        tray::dismiss(app.handle());
      } else {
        tray::present(app.handle());
      }
      tracing::info!(resident, "the shell is up and resident in the tray");
      Ok(())
    })
    .on_page_load(|webview, payload| {
      if let Some(shell) = webview.try_state::<Arc<Shell>>() {
        shell.page_loading(payload.event());
      }
    })
    .on_window_event(tray::on_window_event)
    .invoke_handler(desktop_commands!())
    .build(tauri::generate_context!())
    .expect("the desktop shell runs")
    .run(|app, event| {
      if let tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit = event {
        process::leave(app);
      }
    });
}
