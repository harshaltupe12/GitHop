//! SSH helpers: key generation, ACLs, ssh config conflicts, connection test (Plan §8 C10/C11, §9.2).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! "ssh & scan" task. All processes are spawned with argument arrays.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::git::Git;

/// A `Host` block in `~/.ssh/config` that can make SSH offer the wrong key for GitHub (C10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConflict {
    /// The `Host` patterns line, e.g. `github.com` or `*`.
    pub host_pattern: String,
    /// The `IdentityFile` value inside that block.
    pub identity_file: String,
    /// 1-based line number of the `IdentityFile` directive.
    pub line: usize,
}

/// Result of `ssh -T git@github.com`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ConnectionTest {
    /// GitHub answered "Hi <user>!".
    Connected { user: String },
    /// Key was rejected (`Permission denied (publickey)`).
    Denied,
    /// Anything else (timeout, DNS, ssh missing…), with a short reason.
    Failed { reason: String },
}

/// Windows OpenSSH agent service state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Running,
    Stopped,
    Unknown,
}

/// `~/.ssh` under the home dir git/ssh would use.
pub fn ssh_dir(git: &Git) -> Option<PathBuf> {
    git.home_dir().map(|h| h.join(".ssh"))
}

/// Default key path for a profile: `~/.ssh/githop_<id>`.
pub fn default_key_path(git: &Git, profile_id: &str) -> Option<PathBuf> {
    ssh_dir(git).map(|d| d.join(format!("githop_{profile_id}")))
}

/// Find `Host` blocks matching `github.com` (incl. `*`, `github.com*` patterns) that set
/// `IdentityFile`. Pure; never panics. Ignores comments, handles `Host a b c` lists,
/// `key=value` and `key value` forms, case-insensitive keywords, `Match` blocks end a Host block.
pub fn find_github_conflicts(config: &str) -> Vec<SshConflict> {
    todo!("ssh & scan")
}

/// Read `~/.ssh/config` (missing file → empty list) and run [`find_github_conflicts`].
pub fn ssh_config_conflicts(git: &Git) -> Result<Vec<SshConflict>> {
    todo!("ssh & scan")
}

/// Spawn `ssh-keygen -t ed25519 -f <key> -C <comment>` with inherited stdio so the
/// passphrase prompt goes straight to the user (GitHop never sees it).
/// `no_passphrase` adds `-N ""`. Refuses with `CoreError::Io` (`AlreadyExists`) if the key exists.
/// Creates the parent dir. Calls [`restrict_key_acl`] afterwards.
pub fn keygen(key: &Path, comment: &str, no_passphrase: bool) -> Result<()> {
    todo!("ssh & scan")
}

/// Restrict the private key to the current user (Windows: `icacls` with arg array —
/// `/inheritance:r /grant:r <USERNAME>:F`; checks exit code only, output is locale-dependent.
/// Unix: chmod 600).
pub fn restrict_key_acl(key: &Path) -> Result<()> {
    todo!("ssh & scan")
}

/// Contents of `<key>.pub`, trimmed.
pub fn read_public_key(key: &Path) -> Result<String> {
    todo!("ssh & scan")
}

/// Parse ssh's stderr from `ssh -T git@github.com` into a [`ConnectionTest`].
pub fn parse_connection_output(stderr: &str, exit_code: Option<i32>) -> ConnectionTest {
    todo!("ssh & scan")
}

/// **Network (user-initiated only).** `ssh -T -i <key> -o IdentitiesOnly=yes
/// -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new git@github.com`.
pub fn test_connection(key: &Path) -> Result<ConnectionTest> {
    todo!("ssh & scan")
}

/// `ssh -G <alias>` → the `hostname` value (lower-cased). Too slow for hooks; scan/doctor only.
pub fn resolve_alias_hostname(alias: &str) -> Option<String> {
    todo!("ssh & scan")
}

/// Windows: query the `ssh-agent` service (`sc query ssh-agent`, exit code / STATE parse);
/// other OSes: `Unknown`.
pub fn agent_status() -> AgentStatus {
    todo!("ssh & scan")
}
