//! GitHop data directory layout and path normalisation helpers.
//!
//! Data dir = `GITHOP_HOME` if set (tests), else `%APPDATA%\githop`.
//! Directories are created lazily on first write, never on read.

use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};

/// Env var that overrides the data directory (used by tests).
pub const GITHOP_HOME_ENV: &str = "GITHOP_HOME";

/// Locations of every file GitHop owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Resolve the data dir from `GITHOP_HOME` or the platform config dir.
    pub fn resolve() -> Result<Self> {
        if let Some(home) = std::env::var_os(GITHOP_HOME_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::at(PathBuf::from(home)));
        }
        let base = dirs::config_dir().ok_or(CoreError::NoDataDir)?;
        Ok(Self::at(base.join("githop")))
    }

    /// Use an explicit data dir.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn profiles_file(&self) -> PathBuf {
        self.root.join("profiles.toml")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.toml")
    }

    pub fn hooks_dir(&self) -> PathBuf {
        self.root.join("hooks")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.root.join(".lock")
    }

    /// Create the data dir and its sub-directories. Call before any write.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.hooks_dir(),
            self.backups_dir(),
            self.logs_dir(),
        ] {
            std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
        }
        Ok(())
    }
}

/// Convert a path to a forward-slash string (`C:\a\b` → `C:/a/b`).
pub fn to_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// True when `s` is an absolute path on Windows or Unix
/// (`C:/…`, `C:\…`, UNC `//server/…` / `\\server\…`, or `/…`).
pub fn is_absolute_str(s: &str) -> bool {
    let b = s.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\');
    drive || s.starts_with('/') || s.starts_with('\\')
}

/// Normalise a folder rule: `\` → `/`, collapse duplicate separators (except a
/// leading UNC `//`), ensure a trailing `/`. Returns `None` for relative or empty paths.
pub fn normalize_folder(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() || !is_absolute_str(s) {
        return None;
    }
    let slashed = s.replace('\\', "/");
    let (prefix, rest) = if let Some(r) = slashed.strip_prefix("//") {
        ("//", r)
    } else {
        ("", slashed.as_str())
    };
    let mut out = String::with_capacity(slashed.len() + 1);
    out.push_str(prefix);
    let mut prev_slash = false;
    for c in rest.chars() {
        if c == '/' {
            if prev_slash {
                continue;
            }
            prev_slash = true;
        } else {
            prev_slash = false;
        }
        out.push(c);
    }
    if !out.ends_with('/') {
        out.push('/');
    }
    Some(out)
}

/// Case-insensitive comparison key for a normalised folder or repo path.
pub fn folder_key(normalized: &str) -> String {
    normalized.to_lowercase()
}

/// Normalise a repo root for folder matching: canonicalise (via `dunce`, so no
/// `\\?\` prefix), forward slashes, trailing `/`. Falls back to the raw path if
/// canonicalisation fails (e.g. path no longer exists).
pub fn normalize_repo_root(path: &Path) -> String {
    let canon = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = to_slash(&canon);
    normalize_folder(&s).unwrap_or_else(|| if s.ends_with('/') { s } else { format!("{s}/") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_layout() {
        let p = Paths::at("X:/data");
        assert_eq!(p.profiles_file(), Path::new("X:/data").join("profiles.toml"));
        assert_eq!(p.lock_file(), Path::new("X:/data").join(".lock"));
    }

    #[test]
    fn normalize_folder_cases() {
        assert_eq!(normalize_folder(r"C:\Office"), Some("C:/Office/".into()));
        assert_eq!(normalize_folder("C:/Office/"), Some("C:/Office/".into()));
        assert_eq!(normalize_folder(r"C:\My Work\\Side"), Some("C:/My Work/Side/".into()));
        assert_eq!(normalize_folder(r"\\server\share\x"), Some("//server/share/x/".into()));
        assert_eq!(normalize_folder("/home/me"), Some("/home/me/".into()));
        assert_eq!(normalize_folder("relative/dir"), None);
        assert_eq!(normalize_folder(""), None);
    }

    #[test]
    fn folder_key_is_case_insensitive() {
        assert_eq!(folder_key("C:/Office/"), folder_key("c:/office/"));
    }
}
