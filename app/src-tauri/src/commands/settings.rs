//! Settings commands: update settings, install hooks, doctor, wipe, export/import.

use std::time::Duration;

use std::path::Path;

use githop_core::export::{self, ImportPreview, ImportResolution};
use githop_core::hooks::{self, HookHealth};
use githop_core::profile::PROFILES_VERSION;
use githop_core::{backup, doctor, wipe, CoreError, Paths, ProfileSet, Settings, Store};
use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt as _;

use crate::dto::{DoctorReport, IpcError, IpcResult, SettingsPatch, WipeOptions, WipeReport};
use crate::events;
use crate::ops::{self, blocking};
use crate::validate;

/// How long the app waits after a successful wipe before quitting, so the UI
/// can show the result.
const QUIT_AFTER_WIPE: Duration = Duration::from_secs(2);

/// Change enforce mode, logging and/or start-with-Windows.
#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app: AppHandle<R>,
    patch: SettingsPatch,
) -> IpcResult<Settings> {
    if let Some(on) = patch.autostart {
        set_autostart(&app, on)?;
    }
    let settings = blocking(move || {
        let _guard = ops::write_lock();
        let mut store = ops::store()?;
        if patch.enforce.is_some() || patch.logging.is_some() {
            store.update_settings(|s| {
                if let Some(enforce) = patch.enforce {
                    s.enforce = enforce;
                }
                if let Some(logging) = patch.logging {
                    s.logging = logging;
                }
                Ok(())
            })?;
        }
        Ok(store.settings().clone())
    })
    .await?;
    events::data_changed(&app);
    Ok(settings)
}

/// Install (or repair) the global hooks. Backs up the global git config first.
/// On a first install, also turns on start-with-Windows (Phase 5 §3.1: default on).
#[tauri::command]
pub async fn install_hooks<R: Runtime>(app: AppHandle<R>) -> IpcResult<HookHealth> {
    let (health, first_install) = blocking(|| {
        let _guard = ops::write_lock();
        let git = ops::git()?;
        let mut store = ops::store()?;
        let paths = store.paths().clone();
        paths.ensure_dirs()?;
        backup::backup_global_gitconfig(&paths, &git)?;
        let exe = ops::githop_exe(&store.settings().githop_exe)?;
        // Inside the update so the recorded hook state lands on fresh settings.
        let report = store.update_settings(|s| hooks::install_hooks(&paths, &git, s, &exe))?;
        Ok((
            hooks::verify_hooks(&paths, &git, store.settings()),
            !report.reinstall,
        ))
    })
    .await?;
    if first_install {
        if let Err(e) = set_autostart(&app, true) {
            eprintln!("GitHop: couldn't turn on start with Windows: {}", e.message);
        }
    }
    events::data_changed(&app);
    Ok(health)
}

#[tauri::command]
pub async fn doctor() -> IpcResult<DoctorReport> {
    blocking(run_doctor).await
}

/// Run core doctor as the app: git may be missing (reported as a check), the
/// hooks are compared with the bundled `githop.exe`, aliases are refreshed.
fn run_doctor() -> IpcResult<DoctorReport> {
    let paths = Paths::resolve()?;
    let git = ops::git().ok();
    let current_exe = ops::githop_exe("").ok();
    let opts = doctor::DoctorOptions {
        cwd: None,
        current_exe,
        refresh_aliases: true,
    };
    Ok(doctor::diagnose(&paths, git.as_ref(), &opts))
}

/// Remove GitHop: restore git config, remove hooks, optionally unpin repos and
/// delete keys, delete data. The app quits shortly after a successful wipe.
#[tauri::command]
pub async fn wipe<R: Runtime>(app: AppHandle<R>, opts: WipeOptions) -> IpcResult<WipeReport> {
    let report = blocking(move || {
        let _guard = ops::write_lock();
        let git = ops::git()?;
        let paths = Paths::resolve()?;
        Ok(wipe::wipe(&paths, &git, &opts)?)
    })
    .await?;
    // Core removes its own Run entry; the autostart plugin's entry too (Phase 6 §4).
    if let Err(e) = set_autostart(&app, false) {
        eprintln!(
            "GitHop: couldn't turn off start with Windows: {}",
            e.message
        );
    }
    if report.is_complete() {
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(QUIT_AFTER_WIPE);
            handle.exit(0);
        });
    }
    Ok(report)
}

/// Write a password-protected export file (no cloud, Phase 6 §3).
#[tauri::command]
pub async fn export_profiles(path: String, password: String, include_keys: bool) -> IpcResult<()> {
    let target = validate::new_file_in_existing_dir(&path)?;
    validate::password(&password)?;
    blocking(move || {
        let git = ops::git()?;
        let paths = Paths::resolve()?;
        export::export(&paths, &git, &target, &password, include_keys)?;
        Ok(())
    })
    .await
}

/// Open an export file and show what importing it would do (no write).
#[tauri::command]
pub async fn import_preview(path: String, password: String) -> IpcResult<ImportPreview> {
    let source = validate::existing_file(&path)?;
    validate::password(&password)?;
    blocking(move || {
        let git = ops::git()?;
        let paths = Paths::resolve()?;
        Ok(export::preview(&paths, &git, &source, &password)?)
    })
    .await
}

/// Import with the user's choice per clashing profile (core writes through
/// `Store::update`).
#[tauri::command]
pub async fn import_apply<R: Runtime>(
    app: AppHandle<R>,
    path: String,
    password: String,
    resolutions: Vec<ImportResolution>,
) -> IpcResult<ProfileSet> {
    let source = validate::existing_file(&path)?;
    validate::password(&password)?;
    validate::import_resolutions(&resolutions)?;
    let set = blocking(move || {
        let _guard = ops::write_lock();
        let git = ops::git()?;
        let paths = Paths::resolve()?;
        Ok(export::apply(
            &paths,
            &git,
            &source,
            &password,
            &resolutions,
        )?)
    })
    .await?;
    events::data_changed(&app);
    Ok(set)
}

/// Write the doctor report to `path` (Settings → "Save diagnostics file").
#[tauri::command]
pub async fn save_diagnostics(path: String) -> IpcResult<()> {
    let target = validate::new_file_in_existing_dir(&path)?;
    blocking(move || {
        let report = run_doctor()?;
        let text = diagnostics_text(&report);
        std::fs::write(&target, text).map_err(|e| CoreError::io(&target, e))?;
        Ok(())
    })
    .await
}

/// Plain-text diagnostics: one block per check (what doctor already shows — no
/// extra paths, emails or repo data).
fn diagnostics_text(report: &DoctorReport) -> String {
    let mut out = format!("GitHop {} diagnostics\n\n", env!("CARGO_PKG_VERSION"));
    for c in &report.checks {
        let status = match c.status {
            doctor::CheckStatus::Ok => "ok",
            doctor::CheckStatus::Warning => "warning",
            doctor::CheckStatus::Problem => "problem",
        };
        out.push_str(&format!("[{status}] {} — {}\n", c.title, c.detail));
        if let Some(fix) = &c.fix {
            out.push_str(&format!("    fix: {fix}\n"));
        }
    }
    out
}

/// Design §8.7 "Restore last backup": replace `profiles.toml` with the newest
/// `backups/profiles.<time>.toml` that reads and validates. A current file
/// that can't be read is moved aside to `backups/profiles.broken-<time>.toml`
/// first (never deleted).
#[tauri::command]
pub async fn restore_profiles_backup<R: Runtime>(app: AppHandle<R>) -> IpcResult<ProfileSet> {
    let set = blocking(|| {
        let _guard = ops::write_lock();
        let paths = Paths::resolve()?;
        let restored = newest_valid_profiles_backup(&paths.backups_dir())?;
        let mut store = match Store::open_at(paths.clone()) {
            Ok(s) => s,
            Err(CoreError::Toml { .. } | CoreError::UnsupportedVersion { .. }) => {
                set_aside_broken(&paths)?;
                Store::open_at(paths.clone())?
            }
            Err(e) => return Err(e.into()),
        };
        store.update(|p, _| {
            *p = restored;
            Ok(())
        })?;
        Ok(store.profiles().clone())
    })
    .await?;
    events::data_changed(&app);
    Ok(set)
}

/// `profiles.<timestamp>.toml` names, newest first (timestamps sort by name).
fn profile_backup_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| {
                    n.strip_prefix("profiles.")
                        .and_then(|r| r.strip_suffix(".toml"))
                        .is_some_and(|stamp| stamp.starts_with(|c: char| c.is_ascii_digit()))
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
}

/// Newest backup that parses, has the current schema version and validates.
fn newest_valid_profiles_backup(dir: &Path) -> IpcResult<ProfileSet> {
    for name in profile_backup_names(dir) {
        let Ok(text) = std::fs::read_to_string(dir.join(&name)) else {
            continue;
        };
        let Ok(mut set) = toml::from_str::<ProfileSet>(&text) else {
            continue;
        };
        if set.version != PROFILES_VERSION {
            continue;
        }
        set.normalize();
        if set.validate().is_ok() {
            return Ok(set);
        }
    }
    Err(IpcError::not_found(
        "There's no usable backup of your profiles yet.",
    ))
}

/// Move an unreadable `profiles.toml` into `backups/` so it can be restored over.
fn set_aside_broken(paths: &Paths) -> IpcResult<()> {
    let src = paths.profiles_file();
    let dir = paths.backups_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let dest = dir.join(format!("profiles.broken-{}.toml", backup::timestamp()));
    std::fs::rename(&src, &dest).map_err(|e| CoreError::io(&src, e))?;
    Ok(())
}

/// Turn start-with-Windows on/off (autostart plugin; HKCU Run entry).
pub fn set_autostart<R: Runtime>(app: &AppHandle<R>, on: bool) -> IpcResult<()> {
    let manager = app.autolaunch();
    let result = if on {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| {
        IpcError::new(
            crate::dto::ErrorCode::Io,
            format!("Couldn't change start with Windows: {e}"),
        )
    })
}

/// Is start-with-Windows on?
pub fn autostart_enabled<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "version = 1\ndefault = \"office\"\n\n[[profile]]\nid = \"office\"\nlabel = \"Office\"\nname = \"Me\"\nemail = \"me@company.com\"\ncolor = \"blue\"\nssh_key = \"C:/k/githop_office\"\n";

    #[test]
    fn newest_valid_backup_wins_and_bad_ones_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("profiles.20261030T100000.000Z.toml"), GOOD).unwrap();
        let newer = GOOD.replace("Me", "Newer");
        std::fs::write(dir.join("profiles.20261031T100000.000Z.toml"), newer).unwrap();
        // Newest of all, but broken → skipped.
        std::fs::write(
            dir.join("profiles.20261101T100000.000Z.toml"),
            "version = 1\n[[profile]\n",
        )
        .unwrap();
        // Not backups.
        std::fs::write(dir.join("profiles.broken-20261102T100000.000Z.toml"), GOOD).unwrap();
        std::fs::write(dir.join("gitconfig.original"), "").unwrap();

        let names = profile_backup_names(dir);
        assert_eq!(names.len(), 3);
        assert!(names[0].starts_with("profiles.20261101"));
        let set = newest_valid_profiles_backup(dir).unwrap();
        assert_eq!(set.profiles[0].name, "Newer");
    }

    #[test]
    fn no_backup_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let e = newest_valid_profiles_backup(tmp.path()).unwrap_err();
        assert_eq!(e.code, crate::dto::ErrorCode::NotFound);
        let e = newest_valid_profiles_backup(&tmp.path().join("missing")).unwrap_err();
        assert_eq!(e.code, crate::dto::ErrorCode::NotFound);
    }

    #[test]
    fn broken_profiles_file_is_moved_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::at(tmp.path());
        std::fs::write(paths.profiles_file(), "not toml [").unwrap();
        set_aside_broken(&paths).unwrap();
        assert!(!paths.profiles_file().exists());
        let moved: Vec<_> = std::fs::read_dir(paths.backups_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(moved.len(), 1);
        assert!(moved[0].starts_with("profiles.broken-"));
        assert!(profile_backup_names(&paths.backups_dir()).is_empty());
    }
}
