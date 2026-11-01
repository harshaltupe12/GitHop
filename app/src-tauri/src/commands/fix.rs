//! Fix commands (Phase 4): plan, apply, undo.
//!
//! Inputs are validated here already; the bodies switch on once core's `fix`
//! module lands (see the `// INTEGRATE:` lines — each is a one-line swap).

use tauri::{AppHandle, Runtime};

use crate::dto::{FixPlan, FixResult, IpcError, IpcResult};
use crate::events;
use crate::ops::{self, blocking};
use crate::validate;

/// Message while the core API is missing in this build.
const NOT_YET: &str = "Fixing commits isn't available in this build yet. Run: githop fix";

#[tauri::command]
pub async fn fix_plan(repo: String) -> IpcResult<FixPlan> {
    let dir = validate::existing_dir(&repo)?;
    blocking(move || {
        let _git = ops::git()?;
        let _store = ops::store()?;
        let _ = dir;
        // INTEGRATE: Ok(githop_core::fix::plan(&_git, &dir, &_store)?)
        Err(IpcError::unsupported(NOT_YET))
    })
    .await
}

/// Long operation: runs on a worker thread.
#[tauri::command]
pub async fn fix_apply<R: Runtime>(app: AppHandle<R>, repo: String) -> IpcResult<FixResult> {
    let dir = validate::existing_dir(&repo)?;
    let result = blocking(move || {
        let _guard = ops::write_lock();
        let _git = ops::git()?;
        let _store = ops::store()?;
        let _ = dir;
        // INTEGRATE: Ok(githop_core::fix::apply(&_git, &dir, &_store)?)
        Err::<FixResult, _>(IpcError::unsupported(NOT_YET))
    })
    .await?;
    events::data_changed(&app);
    Ok(result)
}

#[tauri::command]
pub async fn fix_undo<R: Runtime>(
    app: AppHandle<R>,
    repo: String,
    backup_ref: String,
) -> IpcResult<()> {
    let dir = validate::existing_dir(&repo)?;
    validate::git_ref(&backup_ref)?;
    blocking(move || {
        let _guard = ops::write_lock();
        let _git = ops::git()?;
        let _ = (dir, backup_ref);
        // INTEGRATE: Ok(githop_core::fix::undo(&_git, &dir, &backup_ref)?)
        Err::<(), _>(IpcError::unsupported(NOT_YET))
    })
    .await?;
    events::data_changed(&app);
    Ok(())
}
