//! Profile commands: list, validate, save (add/edit/rename), delete, set default.

use githop_core::{pin as core_pin, CoreError, Profile, ProfileSet, Severity, ValidationIssue};
use tauri::{AppHandle, Runtime};

use crate::dto::{IpcError, IpcResult};
use crate::events;
use crate::ops::{self, blocking};
use crate::validate;

#[tauri::command]
pub async fn list_profiles() -> IpcResult<ProfileSet> {
    blocking(|| Ok(ops::store()?.profiles().clone())).await
}

/// Live form validation: every issue that belongs to `profile` (no write).
#[tauri::command]
pub async fn validate_profile(
    profile: Profile,
    original_id: Option<String>,
) -> IpcResult<Vec<ValidationIssue>> {
    if let Some(orig) = &original_id {
        validate::profile_id(orig)?;
    }
    blocking(move || {
        let store = ops::store()?;
        let shape = validate::profile_shape(&profile);
        if !shape.is_empty() {
            return Ok(shape);
        }
        let mut candidate = profile;
        candidate.normalize();
        let orig = original_id.as_deref();
        let set = candidate_set(store.profiles(), &candidate, orig)
            .ok_or_else(|| no_such_profile(orig.unwrap_or_default()))?;
        Ok(issues_for(&set, &candidate.id))
    })
    .await
}

/// Add (`original_id` = None) or edit/rename a profile.
#[tauri::command]
pub async fn save_profile<R: Runtime>(
    app: AppHandle<R>,
    profile: Profile,
    original_id: Option<String>,
) -> IpcResult<ProfileSet> {
    if let Some(orig) = &original_id {
        validate::profile_id(orig)?;
    }
    let set = blocking(move || save_profile_blocking(profile, original_id)).await?;
    events::data_changed(&app);
    Ok(set)
}

/// Delete a profile. Deleting the default needs `new_default` unless it's the last one.
#[tauri::command]
pub async fn delete_profile<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    new_default: Option<String>,
) -> IpcResult<ProfileSet> {
    validate::profile_id(&id)?;
    if let Some(nd) = &new_default {
        validate::profile_id(nd)?;
    }
    let set = blocking(move || delete_profile_blocking(&id, new_default.as_deref())).await?;
    events::data_changed(&app);
    Ok(set)
}

/// Make `id` the default profile (backs up global config first; updates the tray).
#[tauri::command]
pub async fn set_default<R: Runtime>(app: AppHandle<R>, id: String) -> IpcResult<ProfileSet> {
    validate::profile_id(&id)?;
    let set = blocking(move || set_default_blocking(&id)).await?;
    events::default_changed(&app, &set);
    Ok(set)
}

/// Shared with the native tray menu.
pub fn set_default_blocking(id: &str) -> IpcResult<ProfileSet> {
    let _guard = ops::write_lock();
    let git = ops::git()?;
    let mut store = ops::store()?;
    ops::profile(store.profiles(), id)?;
    ops::switch_default(&git, &mut store, id, |_| {})?;
    Ok(store.profiles().clone())
}

/// The set `candidate` (already normalised) would produce, with the candidate
/// moved to the end so clash issues ("email already used by …") land on it.
/// `None` when `original_id` doesn't exist.
fn candidate_set(
    current: &ProfileSet,
    candidate: &Profile,
    original_id: Option<&str>,
) -> Option<ProfileSet> {
    let mut set = current.clone();
    if let Some(orig) = original_id {
        let idx = set.profiles.iter().position(|p| p.id == orig)?;
        set.profiles.remove(idx);
        if set.default.as_deref() == Some(orig) {
            set.default = Some(candidate.id.clone());
        }
    }
    set.profiles.push(candidate.clone());
    Some(set)
}

fn no_such_profile(id: &str) -> IpcError {
    IpcError::not_found(format!("There's no profile called \"{id}\"."))
}

/// The edited profile vanished between loading and saving (another window or
/// the CLI removed it). Reported as a validation issue on its id.
fn removed_meanwhile(id: &str) -> CoreError {
    CoreError::Validation(vec![ValidationIssue {
        profile_id: Some(id.to_string()),
        field: "id".to_string(),
        severity: Severity::Error,
        message: format!(
            "Profile \"{id}\" was removed by another GitHop window or command. Nothing was saved."
        ),
    }])
}

/// Issues addressed to profile `id`.
fn issues_for(set: &ProfileSet, id: &str) -> Vec<ValidationIssue> {
    set.issues()
        .into_iter()
        .filter(|i| i.profile_id.as_deref() == Some(id))
        .collect()
}

fn has_errors(issues: &[ValidationIssue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error)
}

fn save_profile_blocking(profile: Profile, original_id: Option<String>) -> IpcResult<ProfileSet> {
    let shape = validate::profile_shape(&profile);
    if !shape.is_empty() {
        return Err(IpcError::validation(shape));
    }
    let _guard = ops::write_lock();
    let git = ops::git()?;
    let mut store = ops::store()?;
    let orig = original_id.as_deref();
    let mut candidate = profile;
    candidate.normalize();

    // "Safety first": before the very first profile is written, back up the
    // global git config. Like `githop init`, the global identity itself is not
    // rewritten for the first profile (its SSH key may not exist yet).
    if store.profiles().profiles.is_empty() {
        let paths = store.paths().clone();
        githop_core::backup::backup_global_gitconfig(&paths, &git)?;
    }

    // Conflict-free: applied to the freshly re-read files under the lock.
    let (before, is_default) = store.update(|set, _| {
        let before = match orig {
            Some(o) => Some(set.get(o).cloned().ok_or_else(|| removed_meanwhile(o))?),
            None => None,
        };
        let check = candidate_set(set, &candidate, orig)
            .ok_or_else(|| removed_meanwhile(orig.unwrap_or(candidate.id.as_str())))?;
        let issues = issues_for(&check, &candidate.id);
        if has_errors(&issues) {
            return Err(CoreError::Validation(issues));
        }
        // Keep the original order (an edited profile stays in place).
        match orig {
            Some(o) => {
                if let Some(slot) = set.profiles.iter_mut().find(|p| p.id == o) {
                    *slot = candidate.clone();
                }
                if set.default.as_deref() == Some(o) {
                    set.default = Some(candidate.id.clone());
                }
            }
            None => set.profiles.push(candidate.clone()),
        }
        // The first profile becomes the default profile.
        if set.default.is_none() && set.profiles.len() == 1 {
            set.default = Some(candidate.id.clone());
        }
        Ok((
            before,
            set.default.as_deref() == Some(candidate.id.as_str()),
        ))
    })?;

    // Keep the global identity in sync when GitHop manages it (it matched the
    // default profile before this edit) and the identity changed.
    let identity_changed = before.as_ref().is_some_and(|b| {
        b.name != candidate.name
            || !b.email.eq_ignore_ascii_case(&candidate.email)
            || b.ssh_key != candidate.ssh_key
            || b.sign_commits != candidate.sign_commits
    });
    // A profile with a key: trust GitHub's published host keys now, so its
    // first push never asks yes/no (ADR 0006). Best effort.
    if candidate.ssh_key.is_file() {
        githop_core::ssh::ensure_github_known_hosts_best_effort(&git, store.paths());
    }
    if is_default
        && identity_changed
        && before
            .as_ref()
            .is_some_and(|b| ops::global_matches(&git, b))
    {
        ops::switch_default(&git, &mut store, &candidate.id, |_| {})?;
    }

    // Re-pin repos pinned to this profile so their local identity (and pin id
    // after a rename) stays right. Failures don't undo the save.
    if let Some(b) = &before {
        if identity_changed || b.id != candidate.id {
            let saved = store
                .profiles()
                .get(&candidate.id)
                .cloned()
                .unwrap_or(candidate);
            let pinned = store.settings().pinned_repos.clone();
            for repo in ops::repos_pinned_to(&git, &pinned, &b.id) {
                if let Err(e) = core_pin::pin_repo(&repo, &saved) {
                    eprintln!(
                        "GitHop: couldn't re-pin {}: {e}",
                        githop_core::paths::to_slash(repo.root())
                    );
                }
            }
        }
    }
    Ok(store.profiles().clone())
}

fn delete_profile_blocking(id: &str, new_default: Option<&str>) -> IpcResult<ProfileSet> {
    let _guard = ops::write_lock();
    let mut store = ops::store()?;
    let set = store.profiles().clone();
    let target = ops::profile(&set, id)?.clone();
    let is_default = set.default.as_deref() == Some(id);
    let others = set.profiles.len() - 1;

    let new_default = match (is_default, others, new_default) {
        (true, n, None) if n > 0 => {
            return Err(IpcError::bad_input(format!(
                "{} is the default profile. Choose another default profile first.",
                target.label
            )))
        }
        (true, _, Some(nd)) if nd == id => {
            return Err(IpcError::bad_input(
                "The new default profile must be a different profile.",
            ))
        }
        (true, _, Some(nd)) => {
            ops::profile(&set, nd)?;
            Some(nd)
        }
        (false, _, Some(_)) => {
            return Err(IpcError::bad_input(format!(
                "{} isn't the default profile, so no new default is needed.",
                target.label
            )))
        }
        (_, _, None) => None,
    };

    match new_default {
        Some(nd) => {
            let git = ops::git()?;
            ops::switch_default(&git, &mut store, nd, |p| p.profiles.retain(|x| x.id != id))?;
        }
        None => {
            store.update(|p, _| {
                p.profiles.retain(|x| x.id != id);
                if p.default.as_deref() == Some(id) {
                    p.default = None;
                }
                Ok(())
            })?;
        }
    }
    Ok(store.profiles().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use githop_core::ProfileColor;
    use std::path::PathBuf;

    fn p(id: &str, email: &str, color: ProfileColor) -> Profile {
        Profile {
            id: id.into(),
            label: id.into(),
            name: "Me".into(),
            email: email.into(),
            color,
            ssh_key: PathBuf::from(format!("C:/k/githop_{id}")),
            owners: vec![],
            folders: vec![],
            sign_commits: false,
        }
    }

    fn set() -> ProfileSet {
        ProfileSet {
            version: 1,
            default: Some("office".into()),
            profiles: vec![
                p("office", "me@company.com", ProfileColor::Blue),
                p("personal", "me@home.dev", ProfileColor::Violet),
            ],
        }
    }

    #[test]
    fn clash_issues_land_on_the_edited_profile() {
        // Editing the first profile to the second's email: the issue must be
        // addressed to the edited profile, not to "personal".
        let edited = p("office", "me@home.dev", ProfileColor::Blue);
        let s = candidate_set(&set(), &edited, Some("office")).unwrap();
        let issues = issues_for(&s, &edited.id);
        assert!(has_errors(&issues), "{issues:?}");
        assert!(issues
            .iter()
            .all(|i| i.profile_id.as_deref() == Some("office")));
        assert!(issues.iter().any(|i| i.field == "email"));
    }

    #[test]
    fn rename_moves_default_and_unknown_original_is_none() {
        let renamed = p("work", "me@company.com", ProfileColor::Blue);
        let s = candidate_set(&set(), &renamed, Some("office")).unwrap();
        assert_eq!(s.default.as_deref(), Some("work"));
        assert!(s.get("office").is_none());
        assert!(candidate_set(&set(), &renamed, Some("nope")).is_none());
    }

    #[test]
    fn duplicate_id_on_add_is_an_error() {
        let dup = p("office", "x@y.dev", ProfileColor::Pink);
        let s = candidate_set(&set(), &dup, None).unwrap();
        let issues = issues_for(&s, &dup.id);
        assert!(issues
            .iter()
            .any(|i| i.field == "id" && i.severity == Severity::Error));
    }

    #[test]
    fn invalid_id_is_reported_by_core_validation() {
        let bad = p("Bad Id", "x@y.dev", ProfileColor::Pink);
        let s = candidate_set(&set(), &bad, None).unwrap();
        let issues = issues_for(&s, &bad.id);
        assert!(issues.iter().any(|i| i.field == "id"));
    }
}
