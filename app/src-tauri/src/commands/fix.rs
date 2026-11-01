//! Fix commands (Phase 4): plan, apply, undo — thin wrappers over
//! `githop_core::fix`. Core refuses unsafe states (`CoreError::FixRefused` →
//! `bad_input` with core's sentence) and never touches pushed commits.

use githop_core::fix::{self, FixPlan, FixResult};
use githop_core::Paths;
use tauri::{AppHandle, Runtime};

use crate::dto::IpcResult;
use crate::events;
use crate::ops::{self, blocking};
use crate::validate;

/// What a fix in `repo` would do. Read-only.
#[tauri::command]
pub async fn fix_plan(repo: String) -> IpcResult<FixPlan> {
    let dir = validate::existing_dir(&repo)?;
    blocking(move || {
        let git = ops::git()?;
        let paths = Paths::resolve()?;
        Ok(fix::plan(&git, &dir, &paths)?)
    })
    .await
}

/// Re-stamp the unpushed wrong-identity commits (backup ref first). Long
/// operation: runs on a worker thread.
#[tauri::command]
pub async fn fix_apply<R: Runtime>(app: AppHandle<R>, repo: String) -> IpcResult<FixResult> {
    let dir = validate::existing_dir(&repo)?;
    let result = blocking(move || {
        let _guard = ops::write_lock();
        let git = ops::git()?;
        let paths = Paths::resolve()?;
        Ok(fix::apply(&git, &dir, &paths)?)
    })
    .await?;
    events::data_changed(&app);
    Ok(result)
}

/// Restore the branch from `backup_ref` (from `FixResult::backup_ref`).
#[tauri::command]
pub async fn fix_undo<R: Runtime>(
    app: AppHandle<R>,
    repo: String,
    backup_ref: String,
) -> IpcResult<()> {
    let dir = validate::existing_dir(&repo)?;
    validate::backup_ref(&backup_ref)?;
    blocking(move || {
        let _guard = ops::write_lock();
        let git = ops::git()?;
        Ok(fix::undo(&git, &dir, &backup_ref)?)
    })
    .await?;
    events::data_changed(&app);
    Ok(())
}
