//! Git process runner.
//!
//! Every git invocation goes through [`Git`]: argument arrays only (never shell
//! strings), optional env overrides (tests inject an isolated environment), and
//! debug timing for the hook performance budget (Plan §11).

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
        }
    }

    pub fn exe(&self) -> &Path {
        &self.exe
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

    /// Path of the global git config file git would use:
    /// `GIT_CONFIG_GLOBAL`, else `$HOME/.gitconfig`, else `%USERPROFILE%/.gitconfig`.
    pub fn global_config_path(&self) -> Result<PathBuf> {
        if let Some(p) = self.env_var("GIT_CONFIG_GLOBAL").filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(p));
        }
        let home = self
            .env_var("HOME")
            .filter(|v| !v.is_empty())
            .or_else(|| self.env_var("USERPROFILE").filter(|v| !v.is_empty()))
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .ok_or(CoreError::NoDataDir)?;
        Ok(home.join(".gitconfig"))
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
    pub fn run_with_stdin(&self, cwd: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>> {
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
    pub fn output(&self, cwd: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<std::process::Output> {
        let started = Instant::now();
        // A missing cwd also makes spawn fail with NotFound — report it as the folder, not git.
        if !cwd.is_dir() {
            return Err(CoreError::io(
                cwd,
                std::io::Error::new(std::io::ErrorKind::NotFound, "folder not found"),
            ));
        }
        let mut cmd = self.command(cwd, args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CoreError::GitNotFound
            } else {
                CoreError::io(&self.exe, e)
            }
        })?;
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(data).map_err(|e| CoreError::io(&self.exe, e))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| CoreError::io(&self.exe, e))?;
        if std::env::var_os(DEBUG_TIMING_ENV).is_some() {
            eprintln!("[githop timing] git {} — {:?}", args.join(" "), started.elapsed());
        }
        Ok(out)
    }
}

/// Find an executable on PATH (adds `.exe` on Windows).
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let candidates: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        candidates
            .iter()
            .map(|c| dir.join(c))
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins_and_remove_hides() {
        let g = Git::with_exe("git")
            .env("GIT_CONFIG_GLOBAL", "X:/tmp/.gitconfig")
            .env_remove("GITHOP_SURELY_UNSET");
        assert_eq!(g.env_var("GIT_CONFIG_GLOBAL"), Some(OsString::from("X:/tmp/.gitconfig")));
        assert_eq!(g.env_var("GITHOP_SURELY_UNSET"), None);
        assert_eq!(g.global_config_path().unwrap(), PathBuf::from("X:/tmp/.gitconfig"));
    }

    #[test]
    fn global_config_falls_back_to_home() {
        let g = Git::with_exe("git")
            .env("GIT_CONFIG_GLOBAL", "")
            .env("HOME", "X:/home/me");
        assert_eq!(g.global_config_path().unwrap(), PathBuf::from("X:/home/me").join(".gitconfig"));
    }
}
