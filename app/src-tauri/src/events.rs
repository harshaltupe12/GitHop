//! Rust → UI events (`app/IPC.md` § Events).

use githop_core::ProfileSet;
use tauri::{AppHandle, Emitter, Runtime};

/// `profiles.toml` / `settings.toml` changed (on disk or by a command). No payload.
pub const PROFILES_CHANGED: &str = "profiles://changed";
/// Scan progress, payload `ScanProgress`.
pub const SCAN_PROGRESS: &str = "scan://progress";
/// Default switched from the tray, payload `ProfileSet`.
pub const TRAY_DEFAULT_CHANGED: &str = "tray://default-changed";

/// Tell every window GitHop data changed and refresh the tray icon/menu.
pub fn data_changed<R: Runtime>(app: &AppHandle<R>) {
    if let Err(e) = app.emit(PROFILES_CHANGED, ()) {
        eprintln!("GitHop: couldn't send {PROFILES_CHANGED}: {e}");
    }
    crate::tray::refresh(app);
}

/// The default profile changed via the tray (menu or tray window).
pub fn default_changed<R: Runtime>(app: &AppHandle<R>, set: &ProfileSet) {
    if let Err(e) = app.emit(TRAY_DEFAULT_CHANGED, set) {
        eprintln!("GitHop: couldn't send {TRAY_DEFAULT_CHANGED}: {e}");
    }
    data_changed(app);
}
