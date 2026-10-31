//! Repo → profile resolution: pin → owner → folder → default (Plan §7.1, C7, C12, C13).
//!
//! CONTRACT FILE: types and signature fixed by the architect; body filled by the
//! Phase 1 "resolver" task. `resolve` must never spawn a process.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::gitconfig::ConfigSnapshot;
use crate::profile::ProfileSet;
use crate::remote::RemoteInfo;

/// Which rule decided the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolutionSource {
    Pin,
    Owner,
    Folder,
    Default,
}

/// Non-fatal findings during resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ResolutionWarning {
    /// Owner rule and folder rule point at different profiles; owner won (C7).
    OwnerFolderDisagree {
        owner_profile: String,
        folder_profile: String,
    },
    /// `githop.profile` names a profile that no longer exists.
    StalePin { profile_id: String },
    /// The chosen remote's URL couldn't be parsed.
    UnknownRemoteFormat { remote: String },
    /// Remote host isn't GitHub; owner rules skipped (v1).
    NonGithubHost { host: String },
    /// Repo sets `core.hooksPath` locally (husky etc.) — GitHop hooks won't run (C3).
    LocalHooksPath { path: String },
}

/// Result of [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// `None` = unresolved.
    pub profile_id: Option<String>,
    pub source: Option<ResolutionSource>,
    /// Remote name used for owner matching, if any.
    pub remote_name: Option<String>,
    /// Parsed remote, if any.
    pub remote: Option<RemoteInfo>,
    /// Value of `githop.profile` (even if stale).
    pub pinned: Option<String>,
    pub warnings: Vec<ResolutionWarning>,
}

/// Resolve which profile owns the repo at `repo_root`.
///
/// 1. Pin (`githop.profile`, local scope) if it names an existing profile; stale → warning, continue.
/// 2. Owner: remote = `githop.remote` if set, else `origin`, else first remote alphabetically;
///    only when `RemoteInfo::is_github(github_aliases)`.
/// 3. Folder: longest matching normalised prefix, case-insensitive (`paths::folder_key`).
/// 4. `set.default` if it exists.
///
/// Owner and folder disagree → owner wins + `OwnerFolderDisagree`. Local
/// `core.hooksPath` → `LocalHooksPath` warning. No process spawns.
pub fn resolve(
    snapshot: &ConfigSnapshot,
    repo_root: &Path,
    set: &ProfileSet,
    github_aliases: &[String],
) -> Resolution {
    todo!("Phase 1: resolver")
}
