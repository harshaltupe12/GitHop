//! Timestamped backups of the global git config (Plan §7.7).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "model & storage" task.
//!
//! Layout in `backups/`:
//! - `gitconfig.<timestamp>`: one per [`backup_global_gitconfig`] call.
//! - `gitconfig.original`: copy of the first backup, never overwritten.
//! - `<backup>.absent`: empty marker next to a backup taken when there was no
//!   global config file. The backup itself is then empty, and restoring it
//!   *deletes* the global config instead of writing an empty one.
//! - `prerestore.gitconfig.<timestamp>`: the global config as it was just before
//!   a restore (so even a restore is backed up first, CLAUDE.md rule 4). These
//!   are never picked as "newest backup" by [`restore_global_gitconfig`].

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

use time::macros::format_description;
use time::OffsetDateTime;

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::paths::Paths;
use crate::store::{atomic_write, list_names, write_new, write_timestamped};

/// File name of the first-ever backup; never overwritten.
pub const ORIGINAL_GITCONFIG: &str = "gitconfig.original";

/// Prefix of timestamped global config backups.
const GITCONFIG_PREFIX: &str = "gitconfig.";

/// Prefix of the safety copy taken just before a restore.
const PRERESTORE_PREFIX: &str = "prerestore.gitconfig.";

/// Suffix of the "no file existed" marker.
const ABSENT_SUFFIX: &str = ".absent";

/// Last millisecond handed out by [`timestamp`], so rapid calls never repeat.
static LAST_MS: AtomicI64 = AtomicI64::new(0);

/// Filename-safe UTC timestamp, e.g. `20261031T142233.123Z` (add millis/counter if needed for uniqueness).
///
/// Fixed width with milliseconds, so name order = time order. Strictly
/// increasing within one process: if two calls land in the same millisecond the
/// second is bumped forward by 1 ms.
pub fn timestamp() -> String {
    let now = OffsetDateTime::now_utc();
    let now_ms = i64::try_from(now.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX);
    // Compare-and-swap loop (works on the MSRV; `fetch_update` is deprecated).
    let mut last = LAST_MS.load(Ordering::SeqCst);
    let ms = loop {
        let next = now_ms.max(last.saturating_add(1));
        match LAST_MS.compare_exchange_weak(last, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break next,
            Err(actual) => last = actual,
        }
    };
    let at = OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).unwrap_or(now);
    at.format(format_description!(
        "[year][month][day]T[hour][minute][second].[subsecond digits:3]Z"
    ))
    .unwrap_or_else(|_| ms.to_string())
}

/// Copy the global config file git uses (`git.global_config_path()`, honours
/// `GIT_CONFIG_GLOBAL`) to `backups/gitconfig.<timestamp>`. The first backup is
/// also written as `backups/gitconfig.original`. A missing global config is
/// recorded as an empty `gitconfig.original` (so restore can recreate "no file").
/// Returns the timestamped backup path.
///
/// "No file" is recorded as an empty backup plus an empty `<backup>.absent`
/// marker (for both the timestamped copy and `gitconfig.original`).
/// Creates the data dirs if needed.
pub fn backup_global_gitconfig(paths: &Paths, git: &Git) -> Result<PathBuf> {
    let src = git.global_config_path()?;
    let (bytes, existed) = match std::fs::read(&src) {
        Ok(b) => (b, true),
        Err(e) if e.kind() == ErrorKind::NotFound => (Vec::new(), false),
        Err(e) => return Err(CoreError::io(&src, e)),
    };

    paths.ensure_dirs()?;
    let dir = paths.backups_dir();

    let stamped = write_timestamped(&dir, GITCONFIG_PREFIX, "", &bytes)?;
    if !existed {
        write_marker(&marker_for(&stamped))?;
    }

    let original = dir.join(ORIGINAL_GITCONFIG);
    match write_new(&original, &bytes) {
        Ok(()) => {
            // Original first, marker second: a crash in between leaves an empty
            // original without marker, which restores as an empty file (harmless).
            let marker = marker_for(&original);
            if existed {
                remove_if_exists(&marker)?;
            } else {
                write_marker(&marker)?;
            }
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(CoreError::io(&original, e)),
    }

    Ok(stamped)
}

/// Path to `gitconfig.original` if it exists.
pub fn original_gitconfig(paths: &Paths) -> Option<PathBuf> {
    let p = paths.backups_dir().join(ORIGINAL_GITCONFIG);
    p.is_file().then_some(p)
}

/// Restore the global config: `original = true` → from `gitconfig.original`,
/// else from the newest timestamped backup. Writes atomically.
///
/// The current global config (if any) is first copied to
/// `backups/prerestore.gitconfig.<timestamp>`. If the chosen backup has an
/// `.absent` marker, the global config file is deleted instead of written.
/// A symlinked global config is written through to its target.
/// No backup to restore from → `CoreError::BackupFailed`.
pub fn restore_global_gitconfig(paths: &Paths, git: &Git, original: bool) -> Result<()> {
    let source = if original {
        original_gitconfig(paths).ok_or_else(|| {
            CoreError::BackupFailed("there's no original backup of your git config".into())
        })?
    } else {
        newest_backup(&paths.backups_dir())?.ok_or_else(|| {
            CoreError::BackupFailed("there's no backup of your git config yet".into())
        })?
    };
    let absent = marker_for(&source).is_file();
    let bytes = if absent {
        Vec::new()
    } else {
        std::fs::read(&source).map_err(|e| CoreError::io(&source, e))?
    };

    let dest = write_target(&git.global_config_path()?)?;

    // Back up what's there now before replacing it.
    match std::fs::read(&dest) {
        Ok(current) => {
            paths.ensure_dirs()?;
            write_timestamped(&paths.backups_dir(), PRERESTORE_PREFIX, "", &current)?;
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(CoreError::io(&dest, e)),
    }

    if absent {
        remove_if_exists(&dest)
    } else {
        atomic_write(&dest, &bytes)
    }
}

/// Newest `gitconfig.<timestamp>` in `dir` (not `.original`, not markers).
fn newest_backup(dir: &Path) -> Result<Option<PathBuf>> {
    let mut names = list_names(dir, is_timestamped_gitconfig)?;
    names.sort();
    Ok(names.pop().map(|n| dir.join(n)))
}

/// `gitconfig.<timestamp>` where the timestamp starts with a digit.
fn is_timestamped_gitconfig(name: &str) -> bool {
    !name.ends_with(ABSENT_SUFFIX)
        && name
            .strip_prefix(GITCONFIG_PREFIX)
            .is_some_and(|ts| ts.starts_with(|c: char| c.is_ascii_digit()))
}

fn marker_for(backup: &Path) -> PathBuf {
    let mut s = backup.as_os_str().to_os_string();
    s.push(ABSENT_SUFFIX);
    PathBuf::from(s)
}

fn write_marker(path: &Path) -> Result<()> {
    std::fs::write(path, b"").map_err(|e| CoreError::io(path, e))
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CoreError::io(path, e)),
    }
}

/// Where to write `path`: the symlink target if `path` is a symlink (so a
/// dotfiles-managed `.gitconfig` link is kept), else `path` itself.
fn write_target(path: &Path) -> Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            dunce::canonicalize(path).map_err(|e| CoreError::io(path, e))
        }
        _ => Ok(path.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A git handle whose global config is a temp file. No git process is spawned.
    fn isolated() -> (tempfile::TempDir, Paths, Git, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("home dir").join(".gitconfig");
        std::fs::create_dir_all(global.parent().unwrap()).unwrap();
        let git = Git::with_exe("git").env("GIT_CONFIG_GLOBAL", &global);
        let paths = Paths::at(dir.path().join("data"));
        (dir, paths, git, global)
    }

    #[test]
    fn timestamp_format() {
        let ts = timestamp();
        assert_eq!(ts.len(), "20261031T142233.123Z".len(), "{ts}");
        assert_eq!(&ts[8..9], "T");
        assert!(ts.ends_with('Z'));
        assert!(ts
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'T' | 'Z' | '.')));
    }

    #[test]
    fn timestamps_are_unique_and_ordered() {
        let mut all: Vec<String> = (0..2000).map(|_| timestamp()).collect();
        let sorted = {
            let mut s = all.clone();
            s.sort();
            s
        };
        assert_eq!(all, sorted, "timestamps must increase");
        all.dedup();
        assert_eq!(all.len(), 2000);

        let handles: Vec<_> = (0..4)
            .map(|_| std::thread::spawn(|| (0..500).map(|_| timestamp()).collect::<Vec<_>>()))
            .collect();
        let mut threaded: Vec<String> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        threaded.sort();
        threaded.dedup();
        assert_eq!(threaded.len(), 2000);
    }

    #[test]
    fn first_backup_becomes_original_and_is_never_overwritten() {
        let (_d, paths, git, global) = isolated();
        std::fs::write(&global, "[user]\n\tname = First\n").unwrap();
        let b1 = backup_global_gitconfig(&paths, &git).unwrap();
        std::fs::write(&global, "[user]\n\tname = Second\n").unwrap();
        let b2 = backup_global_gitconfig(&paths, &git).unwrap();
        assert_ne!(b1, b2);
        assert!(std::fs::read_to_string(&b2).unwrap().contains("Second"));
        let original = original_gitconfig(&paths).unwrap();
        assert!(std::fs::read_to_string(original).unwrap().contains("First"));
    }

    #[test]
    fn restore_newest_and_original() {
        let (_d, paths, git, global) = isolated();
        std::fs::write(&global, "one\n").unwrap();
        backup_global_gitconfig(&paths, &git).unwrap();
        std::fs::write(&global, "two\n").unwrap();
        backup_global_gitconfig(&paths, &git).unwrap();
        std::fs::write(&global, "three (changed by GitHop)\n").unwrap();

        restore_global_gitconfig(&paths, &git, false).unwrap();
        assert_eq!(std::fs::read_to_string(&global).unwrap(), "two\n");
        // Restoring again still picks the same newest backup (pre-restore copies don't count).
        restore_global_gitconfig(&paths, &git, false).unwrap();
        assert_eq!(std::fs::read_to_string(&global).unwrap(), "two\n");

        restore_global_gitconfig(&paths, &git, true).unwrap();
        assert_eq!(std::fs::read_to_string(&global).unwrap(), "one\n");

        // The state before the first restore was saved.
        let pre = list_names(&paths.backups_dir(), |n| n.starts_with(PRERESTORE_PREFIX)).unwrap();
        assert_eq!(pre.len(), 3);
        let saved = pre
            .iter()
            .map(|n| std::fs::read_to_string(paths.backups_dir().join(n)).unwrap())
            .any(|t| t.contains("three"));
        assert!(saved);
    }

    #[test]
    fn missing_global_config_round_trips_as_no_file() {
        let (_d, paths, git, global) = isolated();
        let b = backup_global_gitconfig(&paths, &git).unwrap();
        assert_eq!(std::fs::read(&b).unwrap(), b"");
        let original = original_gitconfig(&paths).unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), b"");
        assert!(marker_for(&original).is_file());
        assert!(marker_for(&b).is_file());

        std::fs::write(&global, "[user]\n\temail = x@example.com\n").unwrap();
        restore_global_gitconfig(&paths, &git, true).unwrap();
        assert!(!global.exists());

        // Restoring "no file" when there is no file is fine too.
        restore_global_gitconfig(&paths, &git, false).unwrap();
        assert!(!global.exists());
    }

    #[test]
    fn restore_without_backups_fails_clearly() {
        let (_d, paths, git, global) = isolated();
        std::fs::write(&global, "keep\n").unwrap();
        assert!(matches!(
            restore_global_gitconfig(&paths, &git, true),
            Err(CoreError::BackupFailed(_))
        ));
        assert!(matches!(
            restore_global_gitconfig(&paths, &git, false),
            Err(CoreError::BackupFailed(_))
        ));
        assert_eq!(std::fs::read_to_string(&global).unwrap(), "keep\n");
        assert!(original_gitconfig(&paths).is_none());
    }

    #[test]
    fn name_filters() {
        assert!(is_timestamped_gitconfig("gitconfig.20261031T142233.123Z"));
        assert!(!is_timestamped_gitconfig(
            "gitconfig.20261031T142233.123Z.absent"
        ));
        assert!(!is_timestamped_gitconfig(ORIGINAL_GITCONFIG));
        assert!(!is_timestamped_gitconfig(
            "prerestore.gitconfig.20261031T142233.123Z"
        ));
        assert!(!is_timestamped_gitconfig(
            "profiles.20261031T142233.123Z.toml"
        ));
    }
}
