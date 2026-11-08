//! Error type for `githop-core`.

use std::path::PathBuf;

use crate::profile::ValidationIssue;

/// Result alias used across the core library.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Every failure the core library can report.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// File system error, with the path that failed.
    #[error("couldn't access {}: {source}", crate::paths::to_slash(.path))]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A TOML file couldn't be parsed or written. The message shown is the
    /// first line of the parser's report; `message` holds all of it.
    #[error("couldn't read {}: {}", crate::paths::to_slash(.path), first_line(.message))]
    Toml { path: PathBuf, message: String },

    /// A file was written by a newer GitHop.
    #[error("{} has version {found}, this GitHop understands version {supported}", crate::paths::to_slash(.path))]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    /// Profiles failed validation. Holds **all** issues (errors and warnings).
    #[error("profiles are not valid ({})", crate::text::plural(.0.len(), "issue", "issues"))]
    Validation(Vec<ValidationIssue>),

    /// `git` is not on PATH.
    #[error("git was not found. Install Git for Windows and try again")]
    GitNotFound,

    /// A git command exited non-zero.
    #[error("git {args} failed (exit {code:?}): {stderr}")]
    GitFailed {
        args: String,
        code: Option<i32>,
        stderr: String,
    },

    /// The directory is not inside a git repository.
    #[error("this folder is not a git repo")]
    NotARepo,

    /// Another GitHop process holds the data lock.
    #[error("GitHop data is in use by another GitHop window or command. Try again")]
    Locked,

    /// A data file changed on disk since this `Store` loaded it (another GitHop
    /// window, command or hook saved in between). Nothing was written; reload
    /// (`Store::reload`) and apply the change again, or use `Store::update`.
    #[error("{} was changed by another GitHop window or command. Nothing was saved. Try again", crate::paths::to_slash(.path))]
    Conflict { path: PathBuf },

    /// A backup couldn't be created or restored.
    #[error("backup failed: {0}")]
    BackupFailed(String),

    /// No data directory could be determined (no `GITHOP_HOME`, no `%APPDATA%`).
    #[error("couldn't find a folder for GitHop data. Set GITHOP_HOME")]
    NoDataDir,

    /// `fix` (or its undo) won't run in the repo's current state. The text is a
    /// complete, user-facing sentence with the next step (Design §9).
    #[error("{0}")]
    FixRefused(String),

    /// The passphrase typed in GitHop doesn't unlock the SSH key.
    #[error("That passphrase doesn't unlock this key. Check it and try again")]
    WrongPassphrase,

    /// An SSH key couldn't be created or read in the app (format, key type).
    #[error("SSH key: {0}")]
    Key(String),
}

/// First non-empty line of a (possibly multi-line) parser message, trimmed.
fn first_line(message: &str) -> &str {
    message
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

impl CoreError {
    /// Convenience constructor for [`CoreError::Io`].
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        CoreError::Io {
            path: path.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression D16: forward slashes and one line.
    #[test]
    fn messages_use_slashes_and_one_line() {
        let e = CoreError::Toml {
            path: PathBuf::from(r"C:\Users\me\githop\profiles.toml"),
            message: "TOML parse error at line 1, column 9\n  |\n1 | x\n  |  ^\nexpected `=`\n"
                .into(),
        };
        let s = e.to_string();
        assert_eq!(
            s,
            "couldn't read C:/Users/me/githop/profiles.toml: TOML parse error at line 1, column 9"
        );
        let e = CoreError::io(r"C:\a\b", std::io::Error::other("x"));
        assert!(e.to_string().starts_with("couldn't access C:/a/b:"), "{e}");
        assert_eq!(
            CoreError::Validation(Vec::new()).to_string(),
            "profiles are not valid (0 issues)"
        );
    }
}
