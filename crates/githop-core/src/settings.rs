//! `settings.toml` model (Plan §6).
//!
//! CONTRACT FILE: fields are fixed by the architect. Add fields only with
//! `#[serde(default)]` so older files keep loading.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Current `settings.toml` schema version.
pub const SETTINGS_VERSION: u32 = 1;

/// What a confirmed identity mismatch does (Plan §12, Phase 3 §3.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnforceMode {
    /// Mismatch blocks; GitHop internal errors fail open.
    #[default]
    Block,
    /// Mismatch only warns.
    Warn,
    /// Mismatch blocks; internal errors also block.
    BlockStrict,
}

/// Contents of `settings.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    /// Opt-in logging (off by default).
    pub logging: bool,
    pub log_retention_days: u32,
    pub enforce: EnforceMode,
    pub hooks_installed: bool,
    /// User's prior global `core.hooksPath` (empty = none), for chaining/restore.
    pub previous_hooks_path: String,
    /// Absolute path of `githop.exe` embedded in the installed hooks.
    pub githop_exe: String,
    /// Hook file name → SHA-256 hex, for tamper/staleness detection.
    pub hook_hashes: BTreeMap<String, String>,
    /// Repos GitHop pinned (normalised root paths), for re-pin / wipe.
    pub pinned_repos: Vec<String>,
    /// SSH host aliases known to resolve to `github.com` (cached by doctor/scan).
    pub github_host_aliases: Vec<String>,
    /// Repos already shown the "no owner match" hint (normalised root paths).
    pub hinted_repos: Vec<String>,
    /// Global git keys GitHop has changed (e.g. `user.email`), restored on wipe.
    pub managed_global_keys: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            logging: false,
            log_retention_days: 7,
            enforce: EnforceMode::Block,
            hooks_installed: false,
            previous_hooks_path: String::new(),
            githop_exe: String::new(),
            hook_hashes: BTreeMap::new(),
            pinned_repos: Vec::new(),
            github_host_aliases: Vec::new(),
            hinted_repos: Vec::new(),
            managed_global_keys: Vec::new(),
        }
    }
}
