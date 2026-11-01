//! GitHop tray app (Tauri 2). Thin shell over `githop-core`: the UI calls the
//! typed IPC commands in [`commands`]; all business logic lives in core.
//!
//! Local only: no http/updater/websocket/shell plugins, strict CSP
//! (`tauri.conf.json`), per-window capabilities (`capabilities/`).

pub mod commands;
pub mod dto;
pub mod events;
pub mod ops;
pub mod tray;
pub mod validate;
pub mod watcher;

use tauri::{AppHandle, Manager, RunEvent, Runtime, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

/// Label of the main window (`tauri.conf.json`).
pub const MAIN_WINDOW: &str = "main";

/// Argument the autostart entry passes: start in the tray, window hidden.
pub const HIDDEN_ARG: &str = "--hidden";

/// Show, un-minimise and focus the main window.
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Build and run the app. Returns the process exit code.
pub fn run() -> i32 {
    // Started by Windows at login → stay in the tray. Started by the user
    // (Start menu, installer "Run GitHop") → show the window.
    let hidden = std::env::args().any(|a| a == HIDDEN_ARG);

    let builder = tauri::Builder::default()
        // Must be first: a second launch focuses the running instance instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_ARG]),
        ))
        .invoke_handler(tauri::generate_handler![
            commands::system::get_state,
            commands::profiles::list_profiles,
            commands::profiles::validate_profile,
            commands::profiles::save_profile,
            commands::profiles::delete_profile,
            commands::profiles::set_default,
            commands::keys::suggest_key_path,
            commands::keys::generate_key,
            commands::keys::read_public_key,
            commands::keys::test_connection,
            commands::repos::scan,
            commands::repos::pin,
            commands::repos::unpin,
            commands::repos::pin_all,
            commands::repos::hooks_snippet,
            commands::fix::fix_plan,
            commands::fix::fix_apply,
            commands::fix::fix_undo,
            commands::settings::doctor,
            commands::settings::update_settings,
            commands::settings::install_hooks,
            commands::settings::wipe,
            commands::settings::export_profiles,
            commands::settings::import_preview,
            commands::settings::import_apply,
            commands::settings::restore_profiles_backup,
            commands::settings::save_diagnostics,
            commands::repos::cancel_scan,
            commands::system::open_external,
            commands::system::app_info,
            commands::system::show_main,
            commands::system::quit_app,
        ])
        .on_window_event(|window, event| {
            // Close hides to the tray; "Quit" lives in the tray menu (Design §4.4).
            if window.label() == MAIN_WINDOW {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            let handle = app.handle();
            tray::init(handle)?;
            watcher::start(handle.clone());
            if !hidden {
                show_main_window(handle);
            }
            Ok(())
        });

    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("GitHop couldn't start: {e}");
            return 1;
        }
    };
    app.run(|_app, event| {
        // Hidden windows must not end the app; only an explicit exit (Quit, wipe) does.
        if let RunEvent::ExitRequested { api, code, .. } = event {
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
    0
}
