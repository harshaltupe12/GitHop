//! Effective author/committer identity via `git var` (Plan §8 C8, C9).
//!
//! CONTRACT FILE: types fixed by the architect; bodies filled by the
//! Phase 1 "git integration" task.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::git::Git;

/// A name + email pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    /// Case-insensitive email comparison.
    pub fn email_matches(&self, email: &str) -> bool {
        self.email.trim().eq_ignore_ascii_case(email.trim())
    }
}

/// The identities the next commit would use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveIdentity {
    pub author: Identity,
    pub committer: Identity,
}

/// Parse `Name <email> 1730000000 +0100` (timestamp/tz optional).
pub fn parse_ident(s: &str) -> Option<Identity> {
    todo!("Phase 1: git integration")
}

/// Parse `GIT_AUTHOR_IDENT=` / `GIT_COMMITTER_IDENT=` lines out of `git var -l` output.
pub fn parse_var_list(stdout: &str) -> Option<EffectiveIdentity> {
    todo!("Phase 1: git integration")
}

/// One spawn (`git var -l`). `Ok(None)` when identity is unset (e.g.
/// `user.useConfigOnly` with no email) — a valid state, not an error.
pub fn effective_identity(git: &Git, cwd: &Path) -> Result<Option<EffectiveIdentity>> {
    todo!("Phase 1: git integration")
}
