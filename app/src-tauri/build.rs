//! Tauri build script.
//!
//! Every app command is listed here so Tauri generates an `allow-<command>`
//! permission for it. Commands are then granted per window in `capabilities/`
//! (main window: all of them; tray window: only what the tray menu needs).
//! A command missing from this list is not callable from any window.

/// Keep in sync with `tauri::generate_handler![..]` in `src/lib.rs` and `app/IPC.md`.
const COMMANDS: &[&str] = &[
    // profiles
    "get_state",
    "list_profiles",
    "validate_profile",
    "save_profile",
    "delete_profile",
    "set_default",
    // keys
    "suggest_key_path",
    "generate_key",
    "read_public_key",
    "test_connection",
    "passphrase_state",
    "remember_passphrase",
    "turn_on_agent",
    // repos
    "scan",
    "pin",
    "unpin",
    "pin_all",
    "hooks_snippet",
    "remote_ssh",
    "remote_ssh_all",
    "remote_undo",
    // fix
    "fix_plan",
    "fix_apply",
    "fix_undo",
    // settings
    "doctor",
    "update_settings",
    "install_hooks",
    "wipe",
    "export_profiles",
    "import_preview",
    "import_apply",
    // system
    "restore_profiles_backup",
    "save_diagnostics",
    "cancel_scan",
    // system
    "open_external",
    "app_info",
    "show_main",
    "quit_app",
];

fn main() {
    let attrs = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(e) = tauri_build::try_build(attrs) {
        // Build scripts report failure through a non-zero exit.
        eprintln!("tauri-build failed: {e:#}");
        std::process::exit(1);
    }
}
