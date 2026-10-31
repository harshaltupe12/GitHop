//! A discovered git repository.
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "git integration" task.

use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::gitconfig::ConfigSnapshot;
use crate::identity::EffectiveIdentity;

/// git error text (C locale) for "no work tree here".
const NOT_A_REPO_ERRORS: &[&str] = &["not a git repository", "must be run in a work tree"];

/// `dunce::canonicalize`, falling back to the path as given if it can't be resolved.
fn canonical(p: &Path) -> PathBuf {
    dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

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
    ///
    /// A bare repo or a path inside `.git` (no work tree) is also `NotARepo`:
    /// GitHop only manages working trees. Paths are canonicalised with `dunce`
    /// (no `\\?\` prefix, long names instead of 8.3 short names).
    pub fn discover(git: &Git, dir: &Path) -> Result<Self> {
        if !dir.is_dir() {
            // Spawning with a missing cwd would surface as "git not found".
            return Err(CoreError::io(
                dir,
                std::io::Error::new(std::io::ErrorKind::NotFound, "folder does not exist"),
            ));
        }
        // C locale so the stderr messages we match are not translated.
        let git_c = git.clone().env("LC_ALL", "C");
        let args = ["rev-parse", "--show-toplevel", "--git-common-dir"];
        let out = git_c.output(dir, &args, None)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if NOT_A_REPO_ERRORS.iter().any(|m| stderr.contains(m)) {
                return Err(CoreError::NotARepo);
            }
            return Err(crate::gitconfig::git_failed(&args, &out));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut lines = stdout.lines().map(|l| l.strip_suffix('\r').unwrap_or(l));
        let (Some(top), Some(common)) = (lines.next(), lines.next()) else {
            return Err(CoreError::GitFailed {
                args: args.join(" "),
                code: out.status.code(),
                stderr: format!("unexpected output: {}", stdout.trim()),
            });
        };
        if top.is_empty() {
            return Err(CoreError::NotARepo);
        }
        let root = canonical(Path::new(top));
        let common = Path::new(common);
        let common_dir = if common.is_absolute() {
            canonical(common)
        } else {
            // git prints it relative to the cwd it ran in.
            canonical(&dir.join(common))
        };
        Ok(Self {
            git: git.clone(),
            root,
            common_dir,
        })
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
