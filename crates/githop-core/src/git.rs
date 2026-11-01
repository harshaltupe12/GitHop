//! Git process runner.
//!
//! Every git invocation goes through [`Git`]: argument arrays only (never shell
//! strings), optional env overrides (tests inject an isolated environment), and
//! debug timing for the hook performance budget (Plan §11).

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::error::{CoreError, Result};

/// Env var that turns on spawn timing output on stderr.
pub const DEBUG_TIMING_ENV: &str = "GITHOP_DEBUG_TIMING";

/// Handle for spawning git. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Git {
    exe: PathBuf,
    /// Env overrides applied to every spawn. `None` value = remove the variable.
    env: Vec<(OsString, Option<OsString>)>,
    /// Spawns made through [`Git::output`] by this handle and every clone of it
    /// (for the hook spawn budget, Plan §11).
    spawns: Arc<AtomicUsize>,
}

impl Git {
    /// Locate `git` on PATH (once). Errors with [`CoreError::GitNotFound`].
    pub fn locate() -> Result<Self> {
        let exe = find_on_path("git").ok_or(CoreError::GitNotFound)?;
        Ok(Self::with_exe(exe))
    }

    /// Use an explicit git executable.
    pub fn with_exe(exe: impl Into<PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            env: Vec::new(),
            spawns: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// Number of git processes spawned through [`Git::output`] (and so `run*`)
    /// by this handle and every handle cloned from it. Commands built with
    /// [`Git::command`] and spawned by the caller are not counted.
    pub fn spawn_count(&self) -> usize {
        self.spawns.load(Ordering::Relaxed)
    }

    /// Set an env var for every spawned git process.
    #[must_use]
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.set_override(key.as_ref(), Some(value.as_ref().to_os_string()));
        self
    }

    /// Remove an env var for every spawned git process.
    #[must_use]
    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Self {
        self.set_override(key.as_ref(), None);
        self
    }

    fn set_override(&mut self, key: &OsStr, value: Option<OsString>) {
        self.env.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
        self.env.push((key.to_os_string(), value));
    }

    /// Effective value of an env var as git would see it: override first, then
    /// this process's environment.
    pub fn env_var(&self, key: &str) -> Option<OsString> {
        for (k, v) in &self.env {
            if k.eq_ignore_ascii_case(key) {
                return v.clone();
            }
        }
        std::env::var_os(key)
    }

    /// Path of the global config file `git config --global` **writes** to,
    /// resolved exactly like git (`builtin/config.c`):
    ///
    /// 1. `GIT_CONFIG_GLOBAL` when set and non-empty;
    /// 2. else `~/.gitconfig` when it exists;
    /// 3. else the XDG file (`$XDG_CONFIG_HOME/git/config`, or
    ///    `~/.config/git/config` when `XDG_CONFIG_HOME` is unset/empty) when it exists;
    /// 4. else `~/.gitconfig` (git creates it on the first write).
    ///
    /// Backups, restores and GitHop's own global writes (`gitconfig::set_global`
    /// writes with `--file <this path>`) all use this one path, so the file that
    /// is backed up is always the file that is changed (CLAUDE.md rule 4).
    pub fn global_config_path(&self) -> Result<PathBuf> {
        if let Some(p) = self.env_var("GIT_CONFIG_GLOBAL").filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(p));
        }
        let home = self.home_dir().ok_or(CoreError::NoDataDir)?;
        let user = home.join(".gitconfig");
        if exists_no_follow(&user) {
            return Ok(user);
        }
        if let Some(xdg) = self.xdg_global_config_path() {
            if exists_no_follow(&xdg) {
                return Ok(xdg);
            }
        }
        Ok(user)
    }

    /// The XDG global config file git reads (`$XDG_CONFIG_HOME/git/config`, else
    /// `~/.config/git/config`), whether or not it exists. `None` when
    /// `GIT_CONFIG_GLOBAL` is set (git then ignores the XDG file) or there's no home.
    pub fn xdg_global_config_path(&self) -> Option<PathBuf> {
        if self
            .env_var("GIT_CONFIG_GLOBAL")
            .is_some_and(|v| !v.is_empty())
        {
            return None;
        }
        match self.env_var("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            Some(x) => Some(PathBuf::from(x).join("git").join("config")),
            None => self
                .home_dir()
                .map(|h| h.join(".config").join("git").join("config")),
        }
    }

    /// Home directory as git/ssh would see it (`HOME`, else `USERPROFILE`).
    pub fn home_dir(&self) -> Option<PathBuf> {
        self.env_var("HOME")
            .filter(|v| !v.is_empty())
            .or_else(|| self.env_var("USERPROFILE").filter(|v| !v.is_empty()))
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
    }

    /// A configured `Command` for `git <args>` in `cwd`, with env overrides applied.
    pub fn command<I, S>(&self, cwd: &Path, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(&self.exe);
        cmd.current_dir(cwd).args(args);
        for (k, v) in &self.env {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        cmd
    }

    /// Run `git <args>` in `cwd`; return stdout on exit 0, else [`CoreError::GitFailed`].
    pub fn run(&self, cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
        self.run_with_stdin(cwd, args, None)
    }

    /// Run `git <args>` in `cwd` and return stdout as (lossy) UTF-8.
    pub fn run_text(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        self.run(cwd, args)
            .map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    /// Run `git <args>` feeding `stdin`; return stdout on exit 0.
    pub fn run_with_stdin(
        &self,
        cwd: &Path,
        args: &[&str],
        stdin: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let out = self.output(cwd, args, stdin)?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(CoreError::GitFailed {
                args: args.join(" "),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            })
        }
    }

    /// Run and return the raw output regardless of exit status.
    pub fn output(
        &self,
        cwd: &Path,
        args: &[&str],
        stdin: Option<&[u8]>,
    ) -> Result<std::process::Output> {
        let started = Instant::now();
        // A missing cwd also makes spawn fail with NotFound — report it as the folder, not git.
        if !cwd.is_dir() {
            return Err(CoreError::io(
                cwd,
                std::io::Error::new(std::io::ErrorKind::NotFound, "folder not found"),
            ));
        }
        let mut cmd = self.command(cwd, args);
        self.spawns.fetch_add(1, Ordering::Relaxed);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CoreError::GitNotFound
            } else {
                CoreError::io(&self.exe, e)
            }
        })?;
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(data)
                .map_err(|e| CoreError::io(&self.exe, e))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| CoreError::io(&self.exe, e))?;
        if std::env::var_os(DEBUG_TIMING_ENV).is_some() {
            eprintln!(
                "[githop timing] git {} — {:?}",
                args.join(" "),
                started.elapsed()
            );
        }
        Ok(out)
    }
}

/// True when `path` exists (a dangling symlink counts: git would write through it).
fn exists_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Find an executable on PATH (adds `.exe` on Windows).
///
/// Only **absolute** PATH entries are searched: an empty entry (`;;`), `.` or
/// any other relative entry resolves against the current directory, which in a
/// hook is the repo — a cloned repo could then plant `git.exe` / `ssh.exe` /
/// `icacls.exe` and have GitHop run it (Plan §9).
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    find_in_path_list(&path, name)
}

/// [`find_on_path`] over an explicit PATH value.
pub fn find_in_path_list(path: &OsStr, name: &str) -> Option<PathBuf> {
    let candidates: Vec<String> = if cfg!(windows) {
        vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    };
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| candidates.iter().map(|c| dir.join(c)).find(|p| p.is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins_and_remove_hides() {
        let g = Git::with_exe("git")
            .env("GIT_CONFIG_GLOBAL", "X:/tmp/.gitconfig")
            .env_remove("GITHOP_SURELY_UNSET");
        assert_eq!(
            g.env_var("GIT_CONFIG_GLOBAL"),
            Some(OsString::from("X:/tmp/.gitconfig"))
        );
        assert_eq!(g.env_var("GITHOP_SURELY_UNSET"), None);
        assert_eq!(
            g.global_config_path().unwrap(),
            PathBuf::from("X:/tmp/.gitconfig")
        );
    }

    #[test]
    fn global_config_falls_back_to_home() {
        let g = Git::with_exe("git")
            .env("GIT_CONFIG_GLOBAL", "")
            .env("HOME", "X:/home/me");
        assert_eq!(
            g.global_config_path().unwrap(),
            PathBuf::from("X:/home/me").join(".gitconfig")
        );
    }

    /// Review 2026-10-31 #1: with no `~/.gitconfig` but an XDG config, git
    /// writes the XDG file — so must we (and our backup).
    #[test]
    fn global_config_follows_git_xdg_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let xdg_home = tmp.path().join("xdg");
        std::fs::create_dir_all(&home).unwrap();
        let g = Git::with_exe("git")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("XDG_CONFIG_HOME", &xdg_home);
        let user = home.join(".gitconfig");
        let xdg = xdg_home.join("git").join("config");
        assert_eq!(g.xdg_global_config_path(), Some(xdg.clone()));
        // Neither exists → ~/.gitconfig.
        assert_eq!(g.global_config_path().unwrap(), user);
        // Only XDG exists → XDG.
        std::fs::create_dir_all(xdg.parent().unwrap()).unwrap();
        std::fs::write(&xdg, "").unwrap();
        assert_eq!(g.global_config_path().unwrap(), xdg);
        // Both exist → ~/.gitconfig wins.
        std::fs::write(&user, "").unwrap();
        assert_eq!(g.global_config_path().unwrap(), user);
        // Empty XDG_CONFIG_HOME → ~/.config/git/config.
        let g2 = g.clone().env("XDG_CONFIG_HOME", "");
        assert_eq!(
            g2.xdg_global_config_path(),
            Some(home.join(".config").join("git").join("config"))
        );
        // GIT_CONFIG_GLOBAL beats everything and disables XDG.
        let g3 = g.env("GIT_CONFIG_GLOBAL", tmp.path().join("explicit"));
        assert_eq!(g3.xdg_global_config_path(), None);
        assert_eq!(
            g3.global_config_path().unwrap(),
            tmp.path().join("explicit")
        );
    }

    /// Review #9: relative PATH entries (`;;`, `.`, `sub`) are never searched.
    #[test]
    fn find_on_path_skips_relative_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { "tool.exe" } else { "tool" };
        let abs = dunce::canonicalize(tmp.path()).unwrap().join("bin");
        std::fs::create_dir_all(&abs).unwrap();
        std::fs::write(abs.join(exe), b"").unwrap();
        // The same "planted" dir written as a path relative to the process cwd:
        // without the filter it would be found.
        let cwd = dunce::canonicalize(std::env::current_dir().unwrap()).unwrap();
        let relative = relative_to(&cwd, &abs);
        if let Some(rel) = &relative {
            assert!(cwd.join(rel).join(exe).is_file(), "{rel:?}");
        }
        let mut entries = vec![PathBuf::new(), PathBuf::from(".")];
        entries.extend(relative.clone());
        let rel_only = std::env::join_paths(&entries).unwrap();
        assert_eq!(find_in_path_list(&rel_only, "tool"), None);
        entries.push(abs.clone());
        let mixed = std::env::join_paths(&entries).unwrap();
        assert_eq!(find_in_path_list(&mixed, "tool"), Some(abs.join(exe)));
    }

    /// `to` written relative to `from` (`..\..\x`), when both share a root.
    fn relative_to(from: &Path, to: &Path) -> Option<PathBuf> {
        let f: Vec<_> = from.components().collect();
        let t: Vec<_> = to.components().collect();
        if f.first() != t.first() {
            return None;
        }
        let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
        let mut out = PathBuf::new();
        for _ in common..f.len() {
            out.push("..");
        }
        for c in &t[common..] {
            out.push(c.as_os_str());
        }
        Some(out)
    }

    #[test]
    fn spawn_counter_is_shared_by_clones() {
        let tmp = tempfile::tempdir().unwrap();
        let g = Git::with_exe(tmp.path().join("missing-git.exe"));
        let c = g.clone().env("X", "1");
        assert_eq!(g.spawn_count(), 0);
        let _ = c.output(tmp.path(), &["--version"], None);
        assert_eq!(g.spawn_count(), 1);
        assert_eq!(c.spawn_count(), 1);
    }
}
