//! Settings commands: update settings, install hooks, doctor, wipe, export/import.

use std::time::Duration;

use githop_core::hooks::{self, HookHealth};
use githop_core::{backup, Settings};
use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt as _;

use crate::dto::{
    DoctorReport, ImportPreview, ImportResolution, IpcError, IpcResult, SettingsPatch, WipeOptions,
    WipeReport,
};
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
    blocking(|| {
        let _git = ops::git()?;
        let _store = ops::store()?;
        // INTEGRATE: Ok(githop_core::doctor::run(&_git, &_store))
        Err(IpcError::unsupported(
            "Doctor isn't available in the app in this build yet. Run: githop doctor",
        ))
    })
    .await
}

/// Remove GitHop: restore git config, remove hooks, optionally unpin repos and
/// delete keys, delete data. The app quits shortly after a successful wipe.
#[tauri::command]
pub async fn wipe<R: Runtime>(app: AppHandle<R>, opts: WipeOptions) -> IpcResult<WipeReport> {
    let report = blocking(move || {
        let _guard = ops::write_lock();
        let _git = ops::git()?;
        let _ = opts;
        // INTEGRATE: once core `wipe` exists:
        //   let paths = githop_core::Paths::resolve()?;
        //   let report = githop_core::wipe::wipe(&paths, &_git, &opts)?;
        //   Ok(report)
        // and, outside `blocking`, before returning: `set_autostart(&app, false)`
        // (wipe removes the autostart entry, Phase 6 §4).
        Err::<WipeReport, _>(IpcError::unsupported(
            "Wipe isn't available in the app in this build yet. Run: githop wipe",
        ))
    })
    .await?;
    if report.steps.iter().all(|s| s.ok) {
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
        let _store = ops::store()?;
        let _ = (target, password, include_keys);
        // INTEGRATE: Ok(githop_core::export::export(&_store, &target, &password, include_keys)?)
        Err(IpcError::unsupported(
            "Export isn't available in this build yet.",
        ))
    })
    .await
}

#[tauri::command]
pub async fn import_preview(path: String, password: String) -> IpcResult<ImportPreview> {
    let source = validate::existing_file(&path)?;
    validate::password(&password)?;
    blocking(move || {
        let _store = ops::store()?;
        let _ = (source, password);
        // INTEGRATE: Ok(githop_core::export::preview(&_store, &source, &password)?)
        Err(IpcError::unsupported(
            "Import isn't available in this build yet.",
        ))
    })
    .await
}

#[tauri::command]
pub async fn import_apply<R: Runtime>(
    app: AppHandle<R>,
    path: String,
    password: String,
    resolutions: Vec<ImportResolution>,
) -> IpcResult<githop_core::ProfileSet> {
    let source = validate::existing_file(&path)?;
    validate::password(&password)?;
    validate::import_resolutions(&resolutions)?;
    let set = blocking(move || {
        let _guard = ops::write_lock();
        let _store = ops::store()?;
        let _ = (source, password, resolutions);
        // INTEGRATE:
        //   let mut store = _store;
        //   githop_core::export::import(&mut store, &source, &password, &resolutions)?;
        //   Ok(store.profiles().clone())
        Err::<githop_core::ProfileSet, _>(IpcError::unsupported(
            "Import isn't available in this build yet.",
        ))
    })
    .await?;
    events::data_changed(&app);
    Ok(set)
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
