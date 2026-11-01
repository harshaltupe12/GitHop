//! App-level commands: state snapshot, open a fixed web page, about box, and
//! the three tray-menu-window actions.

use githop_core::hooks::{self, HookHealth};
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

/// Bundled third-party licence summary (About). Phase 6 replaces this with the
/// generated THIRD-PARTY-LICENSES file.
const LICENSES: &str = "GitHop is free software (MIT OR Apache-2.0).\n\
\n\
Includes:\n\
- IBM Plex Sans and IBM Plex Mono — SIL Open Font License 1.1\n\
- Fluent UI System Icons — MIT\n\
- Tauri and its plugins — MIT OR Apache-2.0\n\
- React — MIT\n\
- Rust crates — MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0, BSL-1.0, CC0-1.0, Unicode-3.0";

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
        let onboarding = onboarding(has_profiles, has_backup, &hooks, global_identity);
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

/// Open one of two fixed pages in the user's browser. GitHop itself makes no request.
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

/// Tray window → show the main window.
#[tauri::command]
pub async fn tray_open_main<R: Runtime>(app: AppHandle<R>) -> IpcResult<()> {
    crate::tray::hide_menu_window(&app);
    crate::show_main_window(&app);
    Ok(())
}

/// Tray window → close itself (Esc).
#[tauri::command]
pub async fn tray_hide<R: Runtime>(app: AppHandle<R>) -> IpcResult<()> {
    crate::tray::hide_menu_window(&app);
    Ok(())
}

/// Tray window → quit GitHop.
#[tauri::command]
pub async fn tray_quit<R: Runtime>(app: AppHandle<R>) -> IpcResult<()> {
    app.exit(0);
    Ok(())
}

/// `"ssh-keys"` | `"releases"` → fixed URL.
pub fn external_url(target: &str) -> IpcResult<&'static str> {
    match target {
        "ssh-keys" => Ok(SSH_KEYS_URL),
        "releases" => Ok(RELEASES_URL),
        _ => Err(IpcError::bad_input(
            "GitHop can only open the GitHub SSH keys page or the releases page.",
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::ErrorCode;

    #[test]
    fn only_the_two_fixed_pages_open() {
        assert_eq!(external_url("ssh-keys").unwrap(), SSH_KEYS_URL);
        assert_eq!(external_url("releases").unwrap(), RELEASES_URL);
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
