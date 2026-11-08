//! Shared helpers for command bodies: store/git access, the app write lock,
//! default switching, pinned-repo bookkeeping and locating `githop.exe`.
//!
//! Everything here is synchronous and runs on a blocking worker thread
//! (see [`blocking`]); nothing here touches the WebView.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use githop_core::paths::{folder_key, normalize_repo_root};
use githop_core::{
    backup, gitconfig, pin as core_pin, ConfigScope, Git, Profile, ProfileSet, Repo, Store,
};

use crate::dto::{IpcError, IpcResult};

/// Global git keys GitHop writes when switching the default profile (and
/// `wipe` restores). The same list the CLI uses.
pub use githop_core::wipe::MANAGED_GLOBAL_KEYS;

/// Serialises the app's own read-modify-write commands so two UI actions can't
/// interleave. Cross-process safety (CLI vs app) is the core store lock.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Hold while changing GitHop data or git config.
pub fn write_lock() -> MutexGuard<'static, ()> {
    WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Run `f` on Tauri's blocking pool (git spawns, file I/O) so the UI thread
/// and the async runtime never stall.
pub async fn blocking<T, F>(f: F) -> IpcResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> IpcResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| IpcError::internal(format!("GitHop hit an internal error: {e}")))?
}

/// Open the store (`GITHOP_HOME` or `%APPDATA%\githop`).
pub fn store() -> IpcResult<Store> {
    Ok(Store::open()?)
}

/// Locate git on PATH.
pub fn git() -> IpcResult<Git> {
    Ok(Git::locate()?)
}

/// Profile by id or a `not_found` error.
pub fn profile<'a>(set: &'a ProfileSet, id: &str) -> IpcResult<&'a Profile> {
    set.get(id)
        .ok_or_else(|| IpcError::not_found(format!("There's no profile called \"{id}\".")))
}

/// Make `id` the default profile (Plan §7.6, mirrors the CLI's
/// `switch_default`): back up the global git config, record the keys GitHop
/// manages (saved *before* writing so wipe can always restore them), set
/// global `user.name`/`user.email`/`core.sshCommand`, then — in one
/// conflict-free [`Store::update`] — apply `change` (e.g. remove a deleted
/// profile) and set `default = id`.
///
/// `change` runs twice: once on a fresh copy to find the target profile, once
/// inside the update.
pub fn switch_default(
    git: &Git,
    store: &mut Store,
    id: &str,
    change: impl Fn(&mut ProfileSet),
) -> IpcResult<()> {
    store.reload()?;
    let mut preview = store.profiles().clone();
    change(&mut preview);
    let target = profile(&preview, id)?.clone();

    let paths = store.paths().clone();
    paths.ensure_dirs()?;
    backup::backup_global_gitconfig(&paths, git)?;
    store.update_settings(|s| {
        for key in MANAGED_GLOBAL_KEYS {
            if !s.managed_global_keys.iter().any(|k| k == key) {
                s.managed_global_keys.push(key.to_string());
            }
        }
        Ok(())
    })?;

    core_pin::set_default_profile_global(git, &target)?;

    // A profile removed meanwhile makes `default` invalid → Validation, nothing written.
    store.update(|p, _| {
        change(p);
        p.default = Some(id.to_string());
        Ok(())
    })?;
    Ok(())
}

/// Add a repo root to `settings.pinned_repos` (conflict-free).
pub fn remember_pinned(store: &mut Store, root: &Path) -> IpcResult<()> {
    store.update_settings(|s| {
        remember_repo(&mut s.pinned_repos, root);
        Ok(())
    })?;
    Ok(())
}

/// Remove a repo root from `settings.pinned_repos` (conflict-free).
pub fn forget_pinned(store: &mut Store, root: &Path) -> IpcResult<()> {
    store.update_settings(|s| {
        forget_repo(&mut s.pinned_repos, root);
        Ok(())
    })?;
    Ok(())
}

/// Does the global identity already match `p` (name, email, ssh command)?
pub fn global_matches(git: &Git, p: &Profile) -> bool {
    let get = |k: &str| gitconfig::get_global(git, k).ok().flatten();
    get("user.name").as_deref() == Some(p.name.as_str())
        && get("user.email")
            .as_deref()
            .is_some_and(|e| e.eq_ignore_ascii_case(&p.email))
        && get("core.sshCommand").as_deref() == Some(core_pin::ssh_command_for(&p.ssh_key).as_str())
}

/// Add a normalised repo root to `list` unless present (case-insensitive).
pub fn remember_repo(list: &mut Vec<String>, root: &Path) -> bool {
    let norm = normalize_repo_root(root);
    let key = folder_key(&norm);
    if list.iter().any(|r| folder_key(r) == key) {
        return false;
    }
    list.push(norm);
    true
}

/// Remove a repo root from `list` (case-insensitive).
pub fn forget_repo(list: &mut Vec<String>, root: &Path) -> bool {
    let key = folder_key(&normalize_repo_root(root));
    let before = list.len();
    list.retain(|r| folder_key(r) != key);
    list.len() != before
}

/// Repos in `pinned_repos` currently pinned (local `githop.profile`) to `id`.
pub fn repos_pinned_to(git: &Git, pinned_repos: &[String], id: &str) -> Vec<Repo> {
    pinned_repos
        .iter()
        .filter_map(|root| {
            let path = Path::new(root);
            if !path.is_dir() {
                return None;
            }
            let repo = Repo::discover(git, path).ok()?;
            let snap = repo.config_snapshot().ok()?;
            (snap.get_scoped(core_pin::PIN_KEY, ConfigScope::Local) == Some(id)).then_some(repo)
        })
        .collect()
}

/// File name of the CLI next to the app (installed as a Tauri sidecar).
pub const CLI_EXE: &str = if cfg!(windows) {
    "githop.exe"
} else {
    "githop"
};

/// Where `githop.exe` is, for hook scripts and snippets. Preference: next to
/// this app (installed sidecar, or `target/<profile>/` in dev) → the path the
/// current hooks use → PATH.
pub fn githop_exe(recorded: &str) -> IpcResult<PathBuf> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(CLI_EXE)))
        .filter(|p| p.is_file());
    let recorded = Some(PathBuf::from(recorded)).filter(|p| !recorded.is_empty() && p.is_file());
    sibling
        .or(recorded)
        .or_else(|| githop_core::git::find_on_path("githop"))
        .map(|p| dunce_simplify(&p))
        .ok_or_else(|| {
            IpcError::not_found(
                "GitHop's command-line tool (githop.exe) wasn't found next to the app. \
                 Reinstall GitHop.",
            )
        })
}

/// Drop a `\\?\` prefix if present (hooks embed this path in `sh` scripts).
fn dunce_simplify(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_lists_are_case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let mut list = Vec::new();
        assert!(remember_repo(&mut list, tmp.path()));
        assert!(!remember_repo(&mut list, tmp.path()));
        let upper = PathBuf::from(tmp.path().to_string_lossy().to_uppercase());
        if cfg!(windows) {
            assert!(!remember_repo(&mut list, &upper));
        }
        assert!(forget_repo(&mut list, tmp.path()));
        assert!(list.is_empty());
        assert!(!forget_repo(&mut list, tmp.path()));
    }

    #[test]
    fn verbatim_prefix_is_dropped() {
        assert_eq!(
            dunce_simplify(Path::new(r"\\?\C:\Program Files\GitHop\githop.exe")),
            PathBuf::from(r"C:\Program Files\GitHop\githop.exe")
        );
        assert_eq!(
            dunce_simplify(Path::new(r"C:\x\githop.exe")),
            PathBuf::from(r"C:\x\githop.exe")
        );
    }
}
