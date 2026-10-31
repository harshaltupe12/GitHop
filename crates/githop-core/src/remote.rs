//! Git remote URL parsing (Plan §7.1).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "resolver" task.

use serde::{Deserialize, Serialize};

/// Parsed remote URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInfo {
    /// Host as written (lower-cased), e.g. `github.com` or an SSH alias `github-personal`.
    pub host: String,
    /// Owner (user/org), lower-cased.
    pub owner: String,
    /// Repo name without `.git`.
    pub repo: String,
}

impl RemoteInfo {
    /// True when the host is `github.com` (or `www.github.com`), or one of the
    /// cached SSH aliases known to resolve to github.com (case-insensitive).
    pub fn is_github(&self, github_aliases: &[String]) -> bool {
        todo!("Phase 1: resolver")
    }
}

/// Parse scp-like (`git@host:owner/repo.git`), `https://[user@]host/owner/repo[.git][/]`,
/// `ssh://[user@]host[:port]/owner/repo[.git]` and `git://` URLs.
/// `file://` and local paths → `None`. Never panics on garbage input.
pub fn parse_remote_url(url: &str) -> Option<RemoteInfo> {
    todo!("Phase 1: resolver")
}
