//! Per-repo pin and global default identity writers (Plan §6, §7.6, §8 C16).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "git integration" task.

use std::path::Path;

use crate::error::Result;
use crate::git::Git;
use crate::profile::Profile;
use crate::repo::Repo;

/// Local config key that marks a GitHop pin.
pub const PIN_KEY: &str = "githop.profile";

/// `ssh -i "<key with forward slashes>" -o IdentitiesOnly=yes` (always quoted, C16).
pub fn ssh_command_for(key: &Path) -> String {
    todo!("Phase 1: git integration")
}

/// Extract the `-i` key path from a `core.sshCommand` value (quoted or bare), for
/// comparing a repo's key with its profile's key. `None` when there is no `-i`.
pub fn key_from_ssh_command(cmd: &str) -> Option<String> {
    todo!("Phase 1: git integration")
}

/// Set local `user.name`, `user.email`, `core.sshCommand`, `githop.profile`.
pub fn pin_repo(repo: &Repo, profile: &Profile) -> Result<()> {
    todo!("Phase 1: git integration")
}

/// Remove the keys `pin_repo` set — only if `githop.profile` is present.
/// Returns `true` when a pin was removed.
pub fn unpin_repo(repo: &Repo) -> Result<bool> {
    todo!("Phase 1: git integration")
}

/// Set global `user.name`, `user.email`, `core.sshCommand` to `profile`.
/// Caller is responsible for `backup::backup_global_gitconfig` first.
pub fn set_default_profile_global(git: &Git, profile: &Profile) -> Result<()> {
    todo!("Phase 1: git integration")
}
