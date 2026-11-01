//! SSH host aliases for `github.com` (`Host gh-office` + `HostName github.com`
//! in `~/.ssh/config`), so owner rules work for `gh-office:org/repo` remotes
//! (regression D1).
//!
//! Two ways to fill `settings.github_host_aliases`:
//!
//! - [`refresh`] — the full lookup (`ssh -G` per candidate, follows `Include`
//!   and `%h` tokens). Spawns, so only commands run it: `init`, `add`, `edit`,
//!   `use`, `hooks install`, `scan`, `doctor`.
//! - [`current`] — what hooks and `status` use. When `~/.ssh/config` is
//!   unchanged since the cache was filled (same size and modification time,
//!   one `stat`), the cache is used as is. Otherwise the file is parsed in
//!   process (`Host` → `HostName github.com`, no spawn), the cache is updated
//!   best effort, and the result is used right away. An alias that only `ssh
//!   -G` can confirm (a `%h` `HostName`) is kept from the cache while its
//!   `Host` block is still there.
//!
//! The commit check reaches this only after its 1-spawn fast path failed, so
//! the fast path stays at one `git` call.

use std::path::{Path, PathBuf};

use crate::doctor::alias_candidates;
use crate::error::Result;
use crate::git::Git;
use crate::paths::Paths;
use crate::settings::Settings;
use crate::store::Store;

/// Stamp written when there is no `~/.ssh/config`.
const NO_FILE: &str = "none";

/// `~/.ssh/config` as git's ssh would read it (from git's `HOME`).
pub fn ssh_config_path(git: &Git) -> Option<PathBuf> {
    crate::ssh::ssh_dir(git).map(|d| d.join("config"))
}

/// `"<size>:<mtime ns>"` of the file, or `"none"` when it doesn't exist. No spawn.
pub fn stamp(path: Option<&Path>) -> String {
    let Some(meta) = path.and_then(|p| std::fs::metadata(p).ok()) else {
        return NO_FILE.to_string();
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{}:{mtime}", meta.len())
}

/// Aliases whose block literally says `HostName github.com`, plus those of
/// `cached` that are still candidates (confirmed by `ssh -G` before). Pure.
pub fn static_aliases(config: &str, cached: &[String]) -> Vec<String> {
    let mut out: Vec<String> = alias_candidates(config)
        .into_iter()
        .filter(|(alias, literal)| *literal || cached.iter().any(|c| c == alias))
        .map(|(alias, _)| alias)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn read_config(path: Option<&Path>) -> String {
    path.and_then(|p| std::fs::read(p).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// Aliases to use now, for hooks and `status` (see the module docs). Never
/// spawns; saves the refreshed cache best effort (settings only, so a broken
/// `profiles.toml` doesn't matter).
pub fn current(paths: &Paths, git: &Git, settings: &Settings) -> Vec<String> {
    let path = ssh_config_path(git);
    let now = stamp(path.as_deref());
    if now == settings.ssh_config_stamp {
        return settings.github_host_aliases.clone();
    }
    let found = static_aliases(&read_config(path.as_deref()), &settings.github_host_aliases);
    if paths.root().is_dir() {
        let save = found.clone();
        let _ = Store::update_settings_at(paths, |s| {
            s.github_host_aliases = save;
            s.ssh_config_stamp = now;
            Ok(())
        });
    }
    found
}

/// Full lookup with `ssh -G` (spawns), saved with the file's stamp. For
/// commands only — never call from hooks. Returns the aliases.
pub fn refresh(paths: &Paths, git: &Git) -> Result<Vec<String>> {
    let path = ssh_config_path(git);
    let now = stamp(path.as_deref());
    let found = crate::doctor::find_github_host_aliases(git);
    let save = found.clone();
    Store::update_settings_at(paths, |s| {
        let mut cur = s.github_host_aliases.clone();
        cur.sort();
        if cur != save {
            s.github_host_aliases = save;
        }
        s.ssh_config_stamp = now;
        Ok(())
    })?;
    Ok(found)
}

/// [`refresh`], ignoring errors (a command's main work must not fail over it).
pub fn refresh_best_effort(paths: &Paths, git: &Git) {
    let _ = refresh(paths, git);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_parse_keeps_literal_and_confirmed_aliases() {
        let cfg = "Host gh-office\n  HostName github.com\n\nHost gh-tok\n  HostName %h.example\n\
                   Host other\n  HostName gitlab.com\n";
        assert_eq!(static_aliases(cfg, &[]), vec!["gh-office".to_string()]);
        assert_eq!(
            static_aliases(cfg, &["gh-tok".into(), "gone".into()]),
            vec!["gh-office".to_string(), "gh-tok".to_string()]
        );
    }

    #[test]
    fn stamp_changes_with_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("config");
        assert_eq!(stamp(Some(&p)), "none");
        std::fs::write(&p, "Host a\n").unwrap();
        let a = stamp(Some(&p));
        std::fs::write(&p, "Host a\n  HostName github.com\n").unwrap();
        assert_ne!(stamp(Some(&p)), a);
    }

    #[test]
    fn current_reparses_when_the_file_changed_and_caches() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let paths = Paths::at(&data);
        let git = Git::with_exe("git")
            .env("HOME", &home)
            .env("USERPROFILE", &home);
        std::fs::write(
            home.join(".ssh").join("config"),
            "Host gh-office\n  HostName github.com\n",
        )
        .unwrap();
        let s = Settings::default();
        assert_eq!(current(&paths, &git, &s), vec!["gh-office".to_string()]);
        let saved = Store::load_settings_at(&paths).unwrap();
        assert_eq!(saved.github_host_aliases, vec!["gh-office".to_string()]);
        assert_eq!(
            saved.ssh_config_stamp,
            stamp(Some(&home.join(".ssh").join("config")))
        );
        // Unchanged file: the cache is trusted as is.
        let mut cached = saved.clone();
        cached.github_host_aliases = vec!["from-cache".into()];
        assert_eq!(
            current(&paths, &git, &cached),
            vec!["from-cache".to_string()]
        );
    }
}
