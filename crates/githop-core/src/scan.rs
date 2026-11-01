//! Find git repos under a folder and resolve each (Phase 2 §4.8).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! "ssh & scan" task.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::git::Git;
use crate::identity::EffectiveIdentity;
use crate::paths::{folder_key, normalize_repo_root, to_slash};
use crate::profile::ProfileSet;
use crate::repo::Repo;
use crate::resolver::Resolution;

/// Default walk depth below the scan root.
pub const DEFAULT_MAX_DEPTH: usize = 4;

/// Directory names never descended into.
pub const SKIP_DIRS: &[&str] = &["node_modules", "target", ".git", "vendor", "dist", "build"];

/// Upper bound on scan worker threads.
const MAX_THREADS: usize = 8;

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
///
/// `root` itself is depth 0 (so `max_depth = 0` only checks `root`). Symlinks and
/// junctions are never followed, so the walk can't loop. Bare repos are not reported.
pub fn find_repos(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if is_repo_dir(&dir) {
            found.push(dir);
            continue;
        }
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // `file_type` does not follow links; junctions report as symlinks on Windows.
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                continue;
            }
            stack.push((entry.path(), depth + 1));
        }
    }
    found.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)).then_with(|| a.cmp(b)));
    found
}

fn sort_key(p: &Path) -> String {
    folder_key(&to_slash(p))
}

/// `dir/.git` is a directory, or a file starting with `gitdir:` (worktree/submodule).
fn is_repo_dir(dir: &Path) -> bool {
    let dot_git = dir.join(".git");
    let Ok(meta) = std::fs::symlink_metadata(&dot_git) else {
        return false;
    };
    if meta.is_dir() {
        return true;
    }
    if meta.is_file() {
        return std::fs::read(&dot_git)
            .map(|b| b.trim_ascii_start().starts_with(b"gitdir:"))
            .unwrap_or(false);
    }
    false
}

/// Find repos and resolve each in parallel (small std-thread pool, ≤ 8 threads).
/// Per repo: config snapshot + effective identity + `resolver::resolve`. Output sorted by root.
///
/// A repo whose config can't be read gets `error` set and an empty resolution. If only
/// the effective identity can't be read, the resolution is kept, `identity` is `None`
/// and `error` explains why.
pub fn scan(
    git: &Git,
    root: &Path,
    set: &ProfileSet,
    github_aliases: &[String],
    max_depth: usize,
) -> Vec<ScannedRepo> {
    let dirs = find_repos(root, max_depth);
    if dirs.is_empty() {
        return Vec::new();
    }
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, MAX_THREADS)
        .min(dirs.len());

    let slots: Vec<Mutex<Option<ScannedRepo>>> = dirs.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);

    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(dir) = dirs.get(i) else {
                        break;
                    };
                    let result = scan_one(git, dir, set, github_aliases);
                    if let Ok(mut slot) = slots[i].lock() {
                        *slot = Some(result);
                    }
                })
            })
            .collect();
        for h in handles {
            // A panicking worker leaves its slot empty; handled below.
            let _ = h.join();
        }
    });

    let mut out: Vec<ScannedRepo> = slots
        .into_iter()
        .zip(&dirs)
        .map(|(slot, dir)| {
            slot.into_inner().ok().flatten().unwrap_or_else(|| {
                failed(
                    dir,
                    "GitHop hit an internal error while reading this repo".to_string(),
                )
            })
        })
        .collect();
    out.sort_by(|a, b| {
        folder_key(&a.root)
            .cmp(&folder_key(&b.root))
            .then_with(|| a.root.cmp(&b.root))
    });
    out.dedup_by(|a, b| folder_key(&a.root) == folder_key(&b.root));
    out
}

fn empty_resolution() -> Resolution {
    Resolution {
        profile_id: None,
        source: None,
        remote_name: None,
        remote: None,
        pinned: None,
        warnings: Vec::new(),
    }
}

fn failed(dir: &Path, error: String) -> ScannedRepo {
    ScannedRepo {
        root: to_slash(dir),
        resolution: empty_resolution(),
        identity: None,
        error: Some(error),
    }
}

fn scan_one(git: &Git, dir: &Path, set: &ProfileSet, github_aliases: &[String]) -> ScannedRepo {
    let repo = match Repo::discover(git, dir) {
        Ok(r) => r,
        Err(e) => return failed(dir, e.to_string()),
    };
    // A broken `.git` can make git walk up to a parent repo; don't report that one here.
    if folder_key(&normalize_repo_root(repo.root())) != folder_key(&normalize_repo_root(dir)) {
        return failed(dir, "this folder's .git isn't a valid git repo".to_string());
    }
    let root = to_slash(repo.root());
    let snapshot = match repo.config_snapshot() {
        Ok(s) => s,
        Err(e) => {
            return ScannedRepo {
                root,
                resolution: empty_resolution(),
                identity: None,
                error: Some(e.to_string()),
            }
        }
    };
    let resolution = crate::resolver::resolve(&snapshot, repo.root(), set, github_aliases);
    let (identity, error) = match repo.effective_identity_with(&snapshot) {
        Ok(id) => (id, None),
        Err(e) => (None, Some(e.to_string())),
    };
    ScannedRepo {
        root,
        resolution,
        identity,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkdirs(root: &Path, rel: &str) -> PathBuf {
        let p = root.join(rel);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn rel(root: &Path, found: &[PathBuf]) -> Vec<String> {
        found
            .iter()
            .map(|p| to_slash(p.strip_prefix(root).unwrap()))
            .collect()
    }

    #[test]
    fn finds_repos_sorted_and_respects_skip_and_nesting() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        mkdirs(r, "b-repo/.git");
        mkdirs(r, "a-repo/.git");
        mkdirs(r, "Group/C-repo/.git");
        // Nested repo inside a repo: not descended.
        mkdirs(r, "a-repo/sub/inner/.git");
        // Skip dirs.
        // (`.git` itself is covered by `root_itself_is_a_repo`.)
        for skip in SKIP_DIRS.iter().filter(|s| **s != ".git") {
            mkdirs(r, &format!("group2/{skip}/hidden/.git"));
        }
        mkdirs(r, "proj/node_modules/pkg/.git");
        mkdirs(r, "proj/Target/x/.git");
        // Plain folders without repos.
        mkdirs(r, "empty/deeper");
        let found = find_repos(r, DEFAULT_MAX_DEPTH);
        assert_eq!(rel(r, &found), vec!["a-repo", "b-repo", "Group/C-repo"]);
    }

    #[test]
    fn depth_limit() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        mkdirs(r, "d1/.git");
        mkdirs(r, "x/d2/.git");
        mkdirs(r, "x/y/d3/.git");
        mkdirs(r, "x/y/z/d4/.git");
        mkdirs(r, "x/y/z/w/d5/.git");
        assert_eq!(rel(r, &find_repos(r, 1)), vec!["d1"]);
        assert_eq!(rel(r, &find_repos(r, 2)), vec!["d1", "x/d2"]);
        assert_eq!(
            rel(r, &find_repos(r, DEFAULT_MAX_DEPTH)),
            vec!["d1", "x/d2", "x/y/d3", "x/y/z/d4"]
        );
        assert_eq!(find_repos(r, 0), Vec::<PathBuf>::new());
    }

    #[test]
    fn root_itself_is_a_repo() {
        let t = tempfile::tempdir().unwrap();
        mkdirs(t.path(), ".git");
        mkdirs(t.path(), "child/.git");
        assert_eq!(find_repos(t.path(), 0), vec![t.path().to_path_buf()]);
        assert_eq!(find_repos(t.path(), 4), vec![t.path().to_path_buf()]);
    }

    #[test]
    fn git_file_worktree_detected_but_random_file_not() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        let wt = mkdirs(r, "worktree");
        std::fs::write(wt.join(".git"), "gitdir: C:/somewhere/.git/worktrees/wt\n").unwrap();
        let bogus = mkdirs(r, "bogus");
        std::fs::write(bogus.join(".git"), "not a gitdir pointer").unwrap();
        mkdirs(r, "bogus/real/.git");
        assert_eq!(rel(r, &find_repos(r, 4)), vec!["bogus/real", "worktree"]);
    }

    #[test]
    fn missing_root_is_empty() {
        let t = tempfile::tempdir().unwrap();
        assert!(find_repos(&t.path().join("nope"), 4).is_empty());
    }

    #[test]
    fn scan_of_empty_tree_spawns_nothing() {
        let t = tempfile::tempdir().unwrap();
        // A git exe that doesn't exist: scan must not need it when there are no repos.
        let git = Git::with_exe(t.path().join("no-git.exe"));
        assert!(scan(&git, t.path(), &ProfileSet::default(), &[], 4).is_empty());
    }

    #[test]
    fn scan_records_discover_errors() {
        let t = tempfile::tempdir().unwrap();
        mkdirs(t.path(), "r1/.git");
        mkdirs(t.path(), "r2/.git");
        // Missing git: every repo reports an error instead of panicking or vanishing.
        let git = Git::with_exe(t.path().join("no-git.exe"));
        let out = scan(&git, t.path(), &ProfileSet::default(), &[], 4);
        assert_eq!(out.len(), 2);
        assert!(out[0].root.ends_with("/r1"));
        assert!(out[1].root.ends_with("/r2"));
        assert!(out
            .iter()
            .all(|r| r.error.is_some() && r.resolution.profile_id.is_none()));
    }

    #[cfg(windows)]
    #[test]
    fn junctions_are_not_followed() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        let target = mkdirs(r, "real");
        mkdirs(r, "real/repo/.git");
        let link = r.join("loop");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            return; // can't create junctions here; nothing to test
        }
        // Also a self-loop junction inside the tree.
        let _ = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(target.join("back"))
            .arg(r)
            .stdout(std::process::Stdio::null())
            .status();
        assert_eq!(rel(r, &find_repos(r, 8)), vec!["real/repo"]);
    }
}
