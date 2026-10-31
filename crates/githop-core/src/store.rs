//! Locked, atomic, backed-up storage of `profiles.toml` and `settings.toml` (Plan §8 C17).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "model & storage" task.

use crate::error::Result;
use crate::paths::Paths;
use crate::profile::ProfileSet;
use crate::settings::Settings;

/// How long `save` waits for the data lock before returning `CoreError::Locked`.
pub const LOCK_TIMEOUT_MS: u64 = 2000;

/// Number of `profiles.<timestamp>.toml` versions kept in `backups/`.
pub const PROFILE_BACKUPS_KEPT: usize = 5;

/// Loaded GitHop data plus where it lives.
#[derive(Debug, Clone)]
pub struct Store {
    paths: Paths,
    profiles: ProfileSet,
    settings: Settings,
}

impl Store {
    /// Open using [`Paths::resolve`] (`GITHOP_HOME` or `%APPDATA%\githop`).
    pub fn open() -> Result<Self> {
        Self::open_at(Paths::resolve()?)
    }

    /// Open at an explicit data dir. Missing files → defaults. Never creates dirs.
    /// Unknown `version` → `CoreError::UnsupportedVersion`.
    pub fn open_at(paths: Paths) -> Result<Self> {
        todo!("Phase 1: model & storage")
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn profiles(&self) -> &ProfileSet {
        &self.profiles
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Validate, then under the exclusive file lock (timeout [`LOCK_TIMEOUT_MS`]):
    /// back up the current `profiles.toml`, atomically write both files (temp +
    /// fsync + rename, retry 3× on rename failure), prune old backups.
    /// On success the in-memory copy is replaced.
    pub fn save(&mut self, profiles: &ProfileSet, settings: &Settings) -> Result<()> {
        todo!("Phase 1: model & storage")
    }

    /// Save only settings (profiles unchanged, no profile backup).
    pub fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        todo!("Phase 1: model & storage")
    }
}
