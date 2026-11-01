//! Error type for `githop-core`.

use std::path::PathBuf;

use crate::profile::ValidationIssue;

/// Result alias used across the core library.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Every failure the core library can report.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// File system error, with the path that failed.
    #[error("couldn't access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A TOML file couldn't be parsed or written.
    #[error("couldn't read {path}: {message}")]
    Toml { path: PathBuf, message: String },

    /// A file was written by a newer GitHop.
    #[error("{path} has version {found}, this GitHop understands version {supported}")]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    /// Profiles failed validation. Holds **all** issues (errors and warnings).
    #[error("profiles are not valid ({} issue(s))", .0.len())]
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
    #[error("{} was changed by another GitHop window or command. Nothing was saved. Try again", .path.display())]
    Conflict { path: PathBuf },

    /// A backup couldn't be created or restored.
    #[error("backup failed: {0}")]
    BackupFailed(String),

    /// No data directory could be determined (no `GITHOP_HOME`, no `%APPDATA%`).
    #[error("couldn't find a folder for GitHop data. Set GITHOP_HOME")]
    NoDataDir,
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
