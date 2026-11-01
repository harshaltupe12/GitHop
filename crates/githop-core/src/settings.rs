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

/// What GitHop does with a GitHub HTTPS remote when it pins a repo (ADR 0006).
///
/// Over HTTPS, git signs in with the account Windows saved (Git Credential
/// Manager), not the profile's SSH key, so a pinned repo can still push as the
/// wrong account.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteHttpsPolicy {
    /// Ask before switching (app: confirm dialog; CLI: prompt on a terminal).
    #[default]
    Ask,
    /// Switch GitHub HTTPS remotes to SSH without asking, and say so.
    Auto,
    /// Never switch; only warn.
    Never,
}

impl RemoteHttpsPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            RemoteHttpsPolicy::Ask => "ask",
            RemoteHttpsPolicy::Auto => "auto",
            RemoteHttpsPolicy::Never => "never",
        }
    }

    /// Parse `ask` / `auto` / `never` (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(Self::Ask),
            "auto" => Some(Self::Auto),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
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
    /// SSH host aliases known to resolve to `github.com`. Filled by `init`,
    /// `add`, `edit`, `use`, `hooks install`, `scan` and `doctor` (with
    /// `ssh -G`), and re-read statically by hooks and `status` whenever
    /// `~/.ssh/config` changed since (see [`crate::aliases`]).
    pub github_host_aliases: Vec<String>,
    /// Size and modification time of `~/.ssh/config` when
    /// `github_host_aliases` was last filled (`""` = never; `"none"` = no file).
    pub ssh_config_stamp: String,
    /// Repos already shown the "no owner match" hint (normalised root paths).
    pub hinted_repos: Vec<String>,
    /// Global git keys GitHop has changed (e.g. `user.email`), restored on wipe.
    pub managed_global_keys: Vec<String>,
    /// Also install the rarely used, frequently fired pass-through stubs
    /// (`hooks::OPTIONAL_HOOKS`). Off by default for commit speed.
    pub all_hook_stubs: bool,
    /// SHA-256 of GitHop's `hooks.gitconfig` include file as last written
    /// (include mode only; `""` = none), for tamper detection by doctor.
    pub hooks_include_hash: String,
    /// GitHub HTTPS remotes on pin: ask (default), switch to SSH, or leave them.
    pub remote_https_policy: RemoteHttpsPolicy,
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
            ssh_config_stamp: String::new(),
            hinted_repos: Vec::new(),
            managed_global_keys: Vec::new(),
            all_hook_stubs: false,
            hooks_include_hash: String::new(),
            remote_https_policy: RemoteHttpsPolicy::Ask,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_policy_defaults_to_ask_and_round_trips() {
        let s: Settings = toml::from_str("version = 1\n").unwrap();
        assert_eq!(s.remote_https_policy, RemoteHttpsPolicy::Ask);
        let s: Settings = toml::from_str("remote_https_policy = \"auto\"\n").unwrap();
        assert_eq!(s.remote_https_policy, RemoteHttpsPolicy::Auto);
        let text = toml::to_string(&s).unwrap();
        assert!(text.contains("remote_https_policy = \"auto\""), "{text}");
        for p in [
            RemoteHttpsPolicy::Ask,
            RemoteHttpsPolicy::Auto,
            RemoteHttpsPolicy::Never,
        ] {
            assert_eq!(RemoteHttpsPolicy::parse(p.as_str()), Some(p));
        }
        assert_eq!(RemoteHttpsPolicy::parse("Always"), None);
    }
}
