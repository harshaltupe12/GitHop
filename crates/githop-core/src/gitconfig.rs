//! Git config snapshot (one spawn) and local/global writes (Plan §11, §8 C18).
//!
//! CONTRACT FILE: types and accessor bodies are written by the architect.
//! `ConfigSnapshot::parse`, `read` and the write functions are filled by the
//! Phase 1 "git integration" task.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::git::Git;

/// Where a config value came from (`--show-scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigScope {
    System,
    Global,
    Local,
    Worktree,
    Command,
    Unknown,
}

impl ConfigScope {
    pub fn parse(s: &str) -> Self {
        match s {
            "system" => ConfigScope::System,
            "global" => ConfigScope::Global,
            "local" => ConfigScope::Local,
            "worktree" => ConfigScope::Worktree,
            "command" => ConfigScope::Command,
            _ => ConfigScope::Unknown,
        }
    }
}

/// One `key=value` line from `git config --list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub scope: ConfigScope,
    /// Origin as printed by `--show-origin` (e.g. `file:C:/Users/me/.gitconfig`).
    pub origin: String,
    /// Full key as git prints it: section and variable lower-case, subsection as-is
    /// (e.g. `remote.Origin.url`, `user.email`).
    pub key: String,
    /// `None` for a key with no `=` (boolean true shorthand).
    pub value: Option<String>,
}

/// All config entries visible from a repo, in git's precedence order (later wins).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSnapshot {
    entries: Vec<ConfigEntry>,
}

/// Compare keys: section + variable case-insensitive, subsection case-sensitive.
fn key_eq(a: &str, b: &str) -> bool {
    let (Some((a_sec, a_rest)), Some((b_sec, b_rest))) = (a.split_once('.'), b.split_once('.')) else {
        return a.eq_ignore_ascii_case(b);
    };
    if !a_sec.eq_ignore_ascii_case(b_sec) {
        return false;
    }
    match (a_rest.rsplit_once('.'), b_rest.rsplit_once('.')) {
        (Some((a_sub, a_var)), Some((b_sub, b_var))) => a_sub == b_sub && a_var.eq_ignore_ascii_case(b_var),
        (None, None) => a_rest.eq_ignore_ascii_case(b_rest),
        _ => false,
    }
}

impl ConfigSnapshot {
    /// Build from entries (tests, resolver unit tests).
    pub fn from_entries(entries: Vec<ConfigEntry>) -> Self {
        Self { entries }
    }

    /// Parse the NUL-separated output of
    /// `git config --list --show-origin --show-scope -z`.
    pub fn parse(raw: &[u8]) -> Result<Self> {
        todo!("Phase 1: git integration")
    }

    /// Run `git config --list --show-origin --show-scope -z` in `cwd` (one spawn) and parse it.
    pub fn read(git: &Git, cwd: &Path) -> Result<Self> {
        todo!("Phase 1: git integration")
    }

    pub fn entries(&self) -> &[ConfigEntry] {
        &self.entries
    }

    /// Effective (last) value of `key`. A value-less entry yields `Some("true")`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|e| key_eq(&e.key, key))
            .map(|e| e.value.as_deref().unwrap_or("true"))
    }

    /// Last value of `key` set at exactly `scope`.
    pub fn get_scoped(&self, key: &str, scope: ConfigScope) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.scope == scope && key_eq(&e.key, key))
            .map(|e| e.value.as_deref().unwrap_or("true"))
    }

    /// Effective entry (with scope/origin) for `key`.
    pub fn get_entry(&self, key: &str) -> Option<&ConfigEntry> {
        self.entries.iter().rev().find(|e| key_eq(&e.key, key))
    }

    /// All values of a multi-valued key, in order.
    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| key_eq(&e.key, key))
            .map(|e| e.value.as_deref().unwrap_or("true"))
            .collect()
    }

    /// Remote names that have a `remote.<name>.url`, sorted, de-duplicated.
    pub fn remotes(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .entries
            .iter()
            .filter_map(|e| {
                let (section, rest) = e.key.split_once('.')?;
                let (name, var) = rest.rsplit_once('.')?;
                (section.eq_ignore_ascii_case("remote") && var.eq_ignore_ascii_case("url"))
                    .then(|| name.to_string())
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// URL of remote `name` (no spawn).
    pub fn remote_url(&self, name: &str) -> Option<&str> {
        self.get(&format!("remote.{name}.url"))
    }
}

/// `git config --local <key> <value>` in `repo_root`.
pub fn set_local(git: &Git, repo_root: &Path, key: &str, value: &str) -> Result<()> {
    todo!("Phase 1: git integration")
}

/// `git config --local --unset-all <key>`; a missing key (exit 5) is not an error.
pub fn unset_local(git: &Git, repo_root: &Path, key: &str) -> Result<()> {
    todo!("Phase 1: git integration")
}

/// `git config --global <key> <value>`. Caller must back up first (`backup` module).
pub fn set_global(git: &Git, key: &str, value: &str) -> Result<()> {
    todo!("Phase 1: git integration")
}

/// `git config --global --unset-all <key>`; a missing key is not an error.
pub fn unset_global(git: &Git, key: &str) -> Result<()> {
    todo!("Phase 1: git integration")
}

/// Read one global value (`git config --global --get <key>`); `None` when unset.
pub fn get_global(git: &Git, key: &str) -> Result<Option<String>> {
    todo!("Phase 1: git integration")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(scope: ConfigScope, key: &str, value: &str) -> ConfigEntry {
        ConfigEntry {
            scope,
            origin: String::new(),
            key: key.into(),
            value: Some(value.into()),
        }
    }

    #[test]
    fn accessors() {
        let s = ConfigSnapshot::from_entries(vec![
            e(ConfigScope::Global, "user.email", "g@x"),
            e(ConfigScope::Local, "user.email", "l@x"),
            e(ConfigScope::Local, "remote.upstream.url", "u"),
            e(ConfigScope::Local, "remote.origin.url", "o"),
        ]);
        assert_eq!(s.get("user.email"), Some("l@x"));
        assert_eq!(s.get("User.Email"), Some("l@x"));
        assert_eq!(s.get_scoped("user.email", ConfigScope::Global), Some("g@x"));
        assert_eq!(s.remotes(), vec!["origin".to_string(), "upstream".to_string()]);
        assert_eq!(s.remote_url("origin"), Some("o"));
        assert_eq!(s.remote_url("Origin"), None);
    }
}
