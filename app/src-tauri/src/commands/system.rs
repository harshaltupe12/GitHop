//! App-level commands: state snapshot, open a fixed web page, about box, and
//! the window actions used by the tray menu window (show main, quit).

use githop_core::hooks::{self, HookHealth};
use githop_core::paths::to_slash;
use githop_core::{backup, gitconfig, Identity};
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt as _;

use crate::commands::settings::autostart_enabled;
use crate::dto::{AppInfo, AppState, IpcError, IpcResult, OnboardingState};
use crate::ops::{self, blocking};

/// GitHub's SSH keys settings page (opened in the user's browser).
pub const SSH_KEYS_URL: &str = "https://github.com/settings/keys";
/// Release downloads page. Placeholder owner until the repo is published.
pub const RELEASES_URL: &str = "https://github.com/githop-placeholder/githop/releases";
/// User guide. Placeholder owner until the repo is published.
pub const DOCS_URL: &str =
    "https://github.com/githop-placeholder/githop/blob/main/docs/user-guide.md";

/// Third-party licences shown in Settings → About (generated, repo root).
const LICENSES: &str = include_str!("../../../../THIRD-PARTY-LICENSES.md");

/// Everything the main window shows on start / focus.
#[tauri::command]
pub async fn get_state<R: Runtime>(app: AppHandle<R>) -> IpcResult<AppState> {
    let autostart = autostart_enabled(&app);
    blocking(move || {
        let store = ops::store()?;
        // Without git the app still shows profiles; the hooks card explains.
        let (hooks, global_identity) = match ops::git() {
            Ok(git) => (
                hooks::verify_hooks(store.paths(), &git, store.settings()),
                global_identity(&git),
            ),
            Err(e) => (
                HookHealth {
                    installed: store.settings().hooks_installed,
                    hooks_dir: hooks::hooks_dir_value(store.paths()),
                    githop_exe: store.settings().githop_exe.clone(),
                    previous_hooks_path: store.settings().previous_hooks_path.clone(),
                    config_error: Some(e.message),
                    ..HookHealth::default()
                },
                None,
            ),
        };
        let has_backup = backup::original_gitconfig(store.paths()).is_some();
        let has_profiles = !store.profiles().profiles.is_empty();
        let mut onboarding = onboarding(has_profiles, has_backup, &hooks, global_identity);
        onboarding.backup_dir = to_slash(&store.paths().backups_dir());
        Ok(AppState {
            profiles: store.profiles().clone(),
            settings: store.settings().clone(),
            hooks,
            onboarding,
            autostart,
        })
    })
    .await
}

/// Open one of three fixed pages in the user's browser. GitHop itself makes no request.
#[tauri::command]
pub async fn open_external<R: Runtime>(app: AppHandle<R>, target: String) -> IpcResult<()> {
    let url = external_url(&target)?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| IpcError::internal(format!("Couldn't open your browser: {e}")))
}

#[tauri::command]
pub async fn app_info<R: Runtime>(app: AppHandle<R>) -> IpcResult<AppInfo> {
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        licenses: LICENSES.to_string(),
    })
}

/// Show and focus the main window; hide the tray menu window.
#[tauri::command]
pub async fn show_main<R: Runtime>(app: AppHandle<R>) -> IpcResult<()> {
    crate::tray::hide_menu_window(&app);
    crate::show_main_window(&app);
    Ok(())
}

/// Tray "Quit": exit GitHop (hooks keep working without the app).
#[tauri::command]
pub async fn quit_app<R: Runtime>(app: AppHandle<R>) -> IpcResult<()> {
    app.exit(0);
    Ok(())
}

/// `"ssh-keys"` | `"releases"` | `"docs"` → fixed URL.
pub fn external_url(target: &str) -> IpcResult<&'static str> {
    match target {
        "ssh-keys" => Ok(SSH_KEYS_URL),
        "releases" => Ok(RELEASES_URL),
        "docs" => Ok(DOCS_URL),
        _ => Err(IpcError::bad_input(
            "GitHop can only open the GitHub SSH keys page, the releases page or the user guide.",
        )),
    }
}

fn global_identity(git: &githop_core::Git) -> Option<Identity> {
    let name = gitconfig::get_global(git, "user.name").ok().flatten()?;
    let email = gitconfig::get_global(git, "user.email").ok().flatten()?;
    Some(Identity { name, email })
}

/// Onboarding resume point (see `OnboardingState::step_hint`).
pub fn onboarding(
    has_profiles: bool,
    has_backup: bool,
    hooks: &HookHealth,
    global_identity: Option<Identity>,
) -> OnboardingState {
    let hooks_installed = hooks.installed && hooks.points_to_us;
    let step_hint = match (has_profiles, has_backup, hooks_installed) {
        (false, false, _) => "welcome",
        (false, true, _) => "first-profile",
        (true, _, false) => "protection",
        (true, _, true) => "done",
    };
    OnboardingState {
        has_profiles,
        hooks_installed,
        global_identity,
        step_hint: step_hint.to_string(),
        backup_dir: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::ErrorCode;

    #[test]
    fn only_the_fixed_pages_open() {
        assert_eq!(external_url("ssh-keys").unwrap(), SSH_KEYS_URL);
        assert_eq!(external_url("releases").unwrap(), RELEASES_URL);
        assert_eq!(external_url("docs").unwrap(), DOCS_URL);
        for bad in [
            "",
            "https://evil.example",
            "file:///C:/Windows/System32/calc.exe",
            "SSH-KEYS",
            "releases ",
        ] {
            assert_eq!(external_url(bad).unwrap_err().code, ErrorCode::BadInput);
        }
        assert!(SSH_KEYS_URL.starts_with("https://github.com/"));
        assert!(RELEASES_URL.starts_with("https://github.com/"));
        assert!(DOCS_URL.starts_with("https://github.com/"));
        assert!(LICENSES.contains("Third-party"));
    }

    #[test]
    fn onboarding_steps() {
        let mut h = HookHealth::default();
        assert_eq!(onboarding(false, false, &h, None).step_hint, "welcome");
        assert_eq!(onboarding(false, true, &h, None).step_hint, "first-profile");
        assert_eq!(onboarding(true, true, &h, None).step_hint, "protection");
        h.installed = true;
        assert_eq!(onboarding(true, true, &h, None).step_hint, "protection");
        h.points_to_us = true;
        let s = onboarding(true, true, &h, None);
        assert_eq!(s.step_hint, "done");
        assert!(s.hooks_installed);
    }
}
