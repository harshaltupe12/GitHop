//! Find git repos under a folder and resolve each (Phase 2 §4.8).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! "ssh & scan" task.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::Git;
use crate::identity::EffectiveIdentity;
use crate::profile::ProfileSet;
use crate::resolver::Resolution;

/// Default walk depth below the scan root.
pub const DEFAULT_MAX_DEPTH: usize = 4;

/// Directory names never descended into.
pub const SKIP_DIRS: &[&str] = &["node_modules", "target", ".git", "vendor", "dist", "build"];

/// One repo found by [`scan`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedRepo {
    /// Repo root (forward slashes).
    pub root: String,
    pub resolution: Resolution,
    pub identity: Option<EffectiveIdentity>,
    /// Set when the repo couldn't be read; resolution is then empty.
    pub error: Option<String>,
}

/// Walk `root` up to `max_depth` levels, returning dirs that contain a `.git` dir or
/// `.git` file (worktrees/submodules). Does not descend into a found repo or [`SKIP_DIRS`].
/// Sorted. Unreadable dirs are skipped silently.
pub fn find_repos(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    todo!("ssh & scan")
}

/// Find repos and resolve each in parallel (small std-thread pool, ≤ 8 threads).
/// Per repo: config snapshot + effective identity + `resolver::resolve`. Output sorted by root.
pub fn scan(
    git: &Git,
    root: &Path,
    set: &ProfileSet,
    github_aliases: &[String],
    max_depth: usize,
) -> Vec<ScannedRepo> {
    todo!("ssh & scan")
}
