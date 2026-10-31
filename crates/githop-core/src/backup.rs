//! Timestamped backups of the global git config (Plan §7.7).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "model & storage" task.

use std::path::PathBuf;

use crate::error::Result;
use crate::git::Git;
use crate::paths::Paths;

/// File name of the first-ever backup; never overwritten.
pub const ORIGINAL_GITCONFIG: &str = "gitconfig.original";

/// Filename-safe UTC timestamp, e.g. `20261031T142233Z` (add millis/counter if needed for uniqueness).
pub fn timestamp() -> String {
    todo!("Phase 1: model & storage")
}

/// Copy the global config file git uses (`git.global_config_path()`, honours
/// `GIT_CONFIG_GLOBAL`) to `backups/gitconfig.<timestamp>`. The first backup is
/// also written as `backups/gitconfig.original`. A missing global config is
/// recorded as an empty `gitconfig.original` (so restore can recreate "no file").
/// Returns the timestamped backup path.
pub fn backup_global_gitconfig(paths: &Paths, git: &Git) -> Result<PathBuf> {
    todo!("Phase 1: model & storage")
}

/// Path to `gitconfig.original` if it exists.
pub fn original_gitconfig(paths: &Paths) -> Option<PathBuf> {
    todo!("Phase 1: model & storage")
}

/// Restore the global config: `original = true` → from `gitconfig.original`,
/// else from the newest timestamped backup. Writes atomically.
pub fn restore_global_gitconfig(paths: &Paths, git: &Git, original: bool) -> Result<()> {
    todo!("Phase 1: model & storage")
}
