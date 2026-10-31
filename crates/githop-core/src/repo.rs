//! A discovered git repository.
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "git integration" task.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git::Git;
use crate::gitconfig::ConfigSnapshot;
use crate::identity::EffectiveIdentity;

/// A git working tree: its root and common git dir (shared by worktrees).
#[derive(Debug, Clone)]
pub struct Repo {
    git: Git,
    root: PathBuf,
    common_dir: PathBuf,
}

impl Repo {
    /// One spawn: `git rev-parse --show-toplevel --git-common-dir` in `dir`.
    /// Not a repo → `CoreError::NotARepo`. Paths are absolute (common dir is
    /// resolved relative to `dir` when git prints it relative).
    pub fn discover(git: &Git, dir: &Path) -> Result<Self> {
        todo!("Phase 1: git integration")
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// One spawn — see [`ConfigSnapshot::read`].
    pub fn config_snapshot(&self) -> Result<ConfigSnapshot> {
        ConfigSnapshot::read(&self.git, &self.root)
    }

    /// One spawn — see [`crate::identity::effective_identity`].
    pub fn effective_identity(&self) -> Result<Option<EffectiveIdentity>> {
        crate::identity::effective_identity(&self.git, &self.root)
    }
}
