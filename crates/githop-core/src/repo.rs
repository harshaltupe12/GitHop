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

    /// Build the repo for a git hook **without spawning** (Plan §11 budget),
    /// falling back to [`Repo::discover`] (one spawn) when the layout isn't
    /// recognised.
    ///
    /// git runs `pre-commit` / `pre-push` / `post-checkout` with the working
    /// directory set to the top of the work tree, and exports `GIT_DIR` for
    /// linked worktrees and `--git-dir` invocations (verified with Git for
    /// Windows 2.51). So:
    /// - `GIT_DIR` set → git dir = `GIT_DIR` (relative to `cwd`); work tree =
    ///   `GIT_WORK_TREE` if set, else `cwd`.
    /// - else `cwd/.git` is a directory → that's the git dir, `cwd` the top.
    /// - else `cwd/.git` is a `gitdir: <path>` file (linked worktree / submodule).
    ///
    /// The common dir is `GIT_COMMON_DIR` if set, else the git dir's `commondir`
    /// file, else the git dir. The git dir must contain `HEAD`, otherwise this
    /// falls back to `discover`. Paths are canonicalised like `discover`.
    pub fn from_hook_env(git: &Git, cwd: &Path) -> Result<Self> {
        match Self::hook_layout(git, cwd) {
            Some((root, git_dir)) => {
                let common = git
                    .env_var("GIT_COMMON_DIR")
                    .filter(|v| !v.is_empty())
                    .map(|v| absolute(cwd, Path::new(&v)))
                    .or_else(|| read_commondir(&git_dir))
                    .unwrap_or_else(|| git_dir.clone());
                Ok(Self {
                    git: git.clone(),
                    root: canonical(&root),
                    common_dir: canonical(&common),
                })
            }
            None => Self::discover(git, cwd),
        }
    }

    /// `(work tree top, git dir)` from the hook environment, if recognisable.
    fn hook_layout(git: &Git, cwd: &Path) -> Option<(PathBuf, PathBuf)> {
        if let Some(dir) = git.env_var("GIT_DIR").filter(|v| !v.is_empty()) {
            let git_dir = absolute(cwd, Path::new(&dir));
            let root = git
                .env_var("GIT_WORK_TREE")
                .filter(|v| !v.is_empty())
                .map_or_else(|| cwd.to_path_buf(), |w| absolute(cwd, Path::new(&w)));
            return (git_dir.join("HEAD").is_file() && root.is_dir()).then_some((root, git_dir));
        }
        let dot_git = cwd.join(".git");
        let meta = std::fs::metadata(&dot_git).ok()?;
        let git_dir = if meta.is_dir() {
            dot_git
        } else {
            // `gitdir: <path>` (relative paths are relative to the .git file).
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
            if target.is_empty() {
                return None;
            }
            absolute(cwd, Path::new(target))
        };
        git_dir
            .join("HEAD")
            .is_file()
            .then(|| (cwd.to_path_buf(), git_dir))
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

    /// Up to two spawns (strict `git var`) — see [`crate::identity::effective_identity`].
    pub fn effective_identity(&self) -> Result<Option<EffectiveIdentity>> {
        crate::identity::effective_identity(&self.git, &self.root)
    }

    /// One spawn, given a snapshot read from this repo — see
    /// [`crate::identity::effective_identity_with`].
    pub fn effective_identity_with(
        &self,
        snapshot: &ConfigSnapshot,
    ) -> Result<Option<EffectiveIdentity>> {
        crate::identity::effective_identity_with(&self.git, &self.root, snapshot)
    }
}

/// `p` if absolute, else `base/p`.
fn absolute(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// The `commondir` file of a linked worktree's git dir (relative to the git dir).
fn read_commondir(git_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(git_dir.join("commondir")).ok()?;
    let rel = text.lines().next()?.trim();
    (!rel.is_empty()).then(|| absolute(git_dir, Path::new(rel)))
}
