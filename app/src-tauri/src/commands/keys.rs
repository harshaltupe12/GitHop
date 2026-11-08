//! SSH key commands: suggest a path, generate (passphrase typed in a console
//! window, never over IPC), read the public key, test the connection.

use std::io;
use std::path::{Path, PathBuf};

use githop_core::paths::to_slash;
use githop_core::ssh::{self, ConnectionTest, PassphraseState, RememberOutcome, WindowsAgent};
use githop_core::{CoreError, Profile};
use tauri::{AppHandle, Runtime};

use crate::dto::{GeneratedKey, IpcError, IpcResult};
use crate::events;
use crate::ops::{self, blocking};
use crate::validate;
use githop_core::keyfile;
use zeroize::Zeroizing;

/// `~/.ssh/githop_<id>` (forward slashes).
#[tauri::command]
pub async fn suggest_key_path(id: String) -> IpcResult<String> {
    validate::profile_id(&id)?;
    blocking(move || {
        let git = ops::git()?;
        ssh::default_key_path(&git, &id)
            .map(|p| to_slash(&p))
            .ok_or_else(|| IpcError::not_found("GitHop couldn't find your home folder."))
    })
    .await
}

/// Create an ed25519 key for profile `id` and return its public key.
///
/// Works for a profile that isn't saved yet (onboarding, Add profile): the key
/// goes to `~/.ssh/githop_<id>` with `email` as its comment, and the form
/// saves that path with the profile. When a complete key pair is already at
/// the target path (for example kept from an earlier install), it is used
/// instead of failing.
///
/// The passphrase is typed in GitHop's form (ADR 0007). It encrypts the new key
/// (or unlocks the existing one) and the key is handed to the Windows ssh-agent
/// in the same step, so pushes from VS Code and the terminal never ask. It
/// lives only in memory for this call: never saved, logged or sent anywhere.
/// A stopped agent is turned on first (one Windows admin prompt).
/// With `no_passphrase` the key is created unencrypted and nothing is remembered.
#[tauri::command]
pub async fn generate_key<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    email: Option<String>,
    passphrase: Option<String>,
    no_passphrase: bool,
) -> IpcResult<GeneratedKey> {
    validate::profile_id(&id)?;
    let passphrase = passphrase.map(Zeroizing::new);
    let generated = blocking(move || {
        generate_key_blocking(
            &id,
            email.as_deref(),
            passphrase.as_deref().map(|p| p.as_str()),
            no_passphrase,
        )
    })
    .await?;
    events::data_changed(&app);
    Ok(generated)
}

#[tauri::command]
pub async fn read_public_key(id: String) -> IpcResult<String> {
    validate::profile_id(&id)?;
    blocking(move || {
        let store = ops::store()?;
        let p = ops::profile(store.profiles(), &id)?;
        if p.ssh_key.as_os_str().is_empty() {
            return Err(IpcError::not_found(format!(
                "{} has no SSH key yet. Generate one or choose an existing key file.",
                p.label
            )));
        }
        ssh::read_public_key(&p.ssh_key).map_err(|e| match e {
            CoreError::Io { ref source, .. } if source.kind() == io::ErrorKind::NotFound => {
                IpcError::not_found(format!(
                    "There's no public key at {}.pub.",
                    to_slash(&p.ssh_key)
                ))
            }
            other => other.into(),
        })
    })
    .await
}

/// **Network, user click only** (Plan §10): `ssh -T git@github.com` with the
/// profile's key. ssh may open a console window to ask for the key passphrase.
#[tauri::command]
pub async fn test_connection(id: String) -> IpcResult<ConnectionTest> {
    validate::profile_id(&id)?;
    blocking(move || {
        let store = ops::store()?;
        let p = ops::profile(store.profiles(), &id)?;
        if !p.ssh_key.is_file() {
            return Err(IpcError::not_found(format!(
                "There's no SSH key at {}. Generate one first.",
                to_slash(&p.ssh_key)
            )));
        }
        // GitHub's published host keys first, so ssh never asks yes/no (ADR 0006).
        if let Ok(git) = ops::git() {
            ssh::ensure_github_known_hosts_best_effort(&git, store.paths());
        }
        Ok(ssh::test_connection(&p.ssh_key)?)
    })
    .await
}

/// Key panel status: is the passphrase remembered by the Windows ssh-agent?
/// Reads the key's header and asks the agent (`sc query`, `ssh-add -l`); no network.
#[tauri::command]
pub async fn passphrase_state(id: String) -> IpcResult<PassphraseState> {
    validate::profile_id(&id)?;
    blocking(move || {
        let store = ops::store()?;
        let p = ops::profile(store.profiles(), &id)?;
        Ok(ssh::passphrase_state(&WindowsAgent, &p.ssh_key))
    })
    .await
}

/// "Remember my passphrase". With `passphrase` (typed in GitHop's form, ADR
/// 0007) the key is unlocked in memory and handed to the Windows ssh-agent; the
/// passphrase is never saved or logged. Without it (key types the app can't
/// unlock), `ssh-add <key>` runs in a visible console instead. Needs the
/// ssh-agent service running; otherwise returns `agent-off`.
#[tauri::command]
pub async fn remember_passphrase(
    id: String,
    passphrase: Option<String>,
) -> IpcResult<RememberOutcome> {
    validate::profile_id(&id)?;
    let passphrase = passphrase.map(Zeroizing::new);
    blocking(move || remember_blocking(&id, passphrase.as_deref().map(|p| p.as_str()), false)).await
}

/// "Turn on and remember": one UAC prompt sets the ssh-agent service to start
/// automatically and starts it, then remembers as above.
#[tauri::command]
pub async fn turn_on_agent(id: String, passphrase: Option<String>) -> IpcResult<RememberOutcome> {
    validate::profile_id(&id)?;
    let passphrase = passphrase.map(Zeroizing::new);
    blocking(move || remember_blocking(&id, passphrase.as_deref().map(|p| p.as_str()), true)).await
}

fn remember_blocking(
    id: &str,
    passphrase: Option<&str>,
    turn_on: bool,
) -> IpcResult<RememberOutcome> {
    let profile: Profile = {
        let store = ops::store()?;
        ops::profile(store.profiles(), id)?.clone()
    };
    if !profile.ssh_key.is_file() {
        return Err(IpcError::not_found(format!(
            "There's no SSH key at {}. Generate one first.",
            to_slash(&profile.ssh_key)
        )));
    }
    let outcome = match passphrase {
        Some(p) => {
            ssh::remember_with_passphrase(&WindowsAgent, &profile.ssh_key, Some(p), turn_on)?
        }
        None if turn_on => ssh::turn_on_agent_and_remember(&WindowsAgent, &profile.ssh_key)?,
        None => ssh::remember_passphrase(&WindowsAgent, &profile.ssh_key)?,
    };
    if outcome == RememberOutcome::Remembered {
        refresh_ssh_commands(&profile);
    }
    Ok(outcome)
}

/// After the agent holds the key: rewrite `core.sshCommand` in this profile's
/// pinned repos (and globally when it's the default profile) so it names
/// Windows OpenSSH, the ssh that uses the agent. Older pins said plain `ssh`,
/// which in Git for Windows is its bundled copy that can't. Best effort; the
/// global write is backed up first (CLAUDE.md rule 4).
fn refresh_ssh_commands(profile: &Profile) {
    let Ok(git) = ops::git() else {
        return;
    };
    let _guard = ops::write_lock();
    let Ok(store) = ops::store() else {
        return;
    };
    let want = githop_core::pin::ssh_command_for(&profile.ssh_key);
    for repo in ops::repos_pinned_to(&git, &store.settings().pinned_repos, &profile.id) {
        let current = repo
            .config_snapshot()
            .ok()
            .and_then(|s| s.get("core.sshCommand").map(str::to_string));
        if current.as_deref() != Some(want.as_str()) {
            let _ = githop_core::pin::pin_repo(&repo, profile);
        }
    }
    if store.profiles().default.as_deref() == Some(profile.id.as_str()) {
        let current = githop_core::gitconfig::get_global(&git, "core.sshCommand")
            .ok()
            .flatten();
        if current.as_deref() != Some(want.as_str())
            && githop_core::backup::backup_global_gitconfig(store.paths(), &git).is_ok()
        {
            let _ = githop_core::gitconfig::set_global(&git, "core.sshCommand", &want);
        }
    }
}

fn generate_key_blocking(
    id: &str,
    email: Option<&str>,
    passphrase: Option<&str>,
    no_passphrase: bool,
) -> IpcResult<GeneratedKey> {
    let passphrase = if no_passphrase {
        None
    } else {
        let p = passphrase.filter(|p| !p.is_empty()).ok_or_else(|| {
            IpcError::bad_input(
                "Type a passphrase for the key, or tick “Create the key without a passphrase”.",
            )
        })?;
        if p.chars().count() < keyfile::MIN_PASSPHRASE_CHARS {
            return Err(IpcError::bad_input(format!(
                "Use at least {} characters for the passphrase.",
                keyfile::MIN_PASSPHRASE_CHARS
            )));
        }
        Some(p)
    };
    let git = ops::git()?;
    // A profile still being filled in (not saved yet) is fine.
    let saved: Option<Profile> = {
        let store = ops::store()?;
        store
            .profiles()
            .profiles
            .iter()
            .find(|p| p.id == id)
            .cloned()
    };
    let key = match saved.as_ref().filter(|p| !p.ssh_key.as_os_str().is_empty()) {
        Some(p) => p.ssh_key.clone(),
        None => ssh::default_key_path(&git, id)
            .ok_or_else(|| IpcError::not_found("GitHop couldn't find your home folder."))?,
    };
    let comment = key_comment(saved.as_ref().map(|p| p.email.as_str()).or(email), id);

    let reused = key_pair_exists(&key);
    if reused {
        // An earlier install's key: the passphrase typed must unlock it. Checked
        // here, before anything changes, so a typo gets a clear message.
        if let Err(CoreError::WrongPassphrase) = keyfile::unlock_ed25519(&key, passphrase) {
            return Err(IpcError::bad_input(format!(
                "A GitHop key already exists at {}. Type its passphrase to use it, \
                 or change the profile ID to create a new key.",
                to_slash(&key)
            )));
        }
    } else {
        for existing in [key.clone(), pub_path(&key)] {
            if std::fs::symlink_metadata(&existing).is_ok() {
                return Err(IpcError::conflict(format!(
                    "A key already exists at {}. GitHop won't overwrite it.",
                    to_slash(&existing)
                )));
            }
        }
        keyfile::create_ed25519(&key, &comment, passphrase)?;
    }
    ssh::restrict_key_acl(&key)?;
    {
        let store = ops::store()?;
        ssh::ensure_github_known_hosts_best_effort(&git, store.paths());
    }

    if let Some(profile) = saved.filter(|p| p.ssh_key != key) {
        let _guard = ops::write_lock();
        let mut store = ops::store()?;
        store.update(|set, _| {
            if let Some(p) = set.get_mut(&profile.id) {
                p.ssh_key = key.clone();
            }
            Ok(())
        })?;
    }
    // Remember it now (one admin prompt if the agent is off), so the very first
    // push from VS Code or the terminal doesn't ask. A key that can't be unlocked
    // in the app (another type) is still usable; the card offers the console flow.
    let remembered = match ssh::remember_with_passphrase(&WindowsAgent, &key, passphrase, true) {
        Ok(outcome) => outcome,
        Err(CoreError::WrongPassphrase | CoreError::Key(_)) => RememberOutcome::NotAdded,
        Err(e) => return Err(e.into()),
    };
    Ok(GeneratedKey {
        public_key: ssh::read_public_key(&key)?.trim().to_string(),
        path: to_slash(&key),
        reused,
        remembered,
    })
}

/// Both halves of a key pair are already on disk (an earlier install kept them).
fn key_pair_exists(key: &Path) -> bool {
    key.is_file() && pub_path(key).is_file()
}

/// The key's comment: the profile's email, else `githop-<id>`. Never empty and
/// never control characters (it ends up in the public key line).
fn key_comment(email: Option<&str>, id: &str) -> String {
    match email.map(str::trim) {
        Some(e) if !e.is_empty() && !e.chars().any(char::is_control) => e.to_string(),
        _ => format!("githop-{id}"),
    }
}

/// `<key>.pub` (appended — key names may contain dots).
fn pub_path(key: &Path) -> PathBuf {
    let mut s = key.as_os_str().to_owned();
    s.push(".pub");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pub_path_appends() {
        assert_eq!(
            pub_path(Path::new("C:/k/githop_a.b")),
            PathBuf::from("C:/k/githop_a.b.pub")
        );
    }

    #[test]
    fn existing_pair_is_reused_but_half_a_pair_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let key = tmp.path().join("githop_work");
        assert!(!key_pair_exists(&key));
        std::fs::write(&key, "private").unwrap();
        assert!(!key_pair_exists(&key), "no .pub: still refused by keygen");
        std::fs::write(pub_path(&key), "ssh-ed25519 AAAA x").unwrap();
        assert!(key_pair_exists(&key));
    }

    #[test]
    fn key_comment_uses_the_email_or_a_safe_fallback() {
        assert_eq!(
            key_comment(Some(" jane@acme.example "), "work"),
            "jane@acme.example"
        );
        assert_eq!(key_comment(Some(""), "work"), "githop-work");
        assert_eq!(key_comment(None, "work"), "githop-work");
        assert_eq!(key_comment(Some("a\nb"), "work"), "githop-work");
    }
}
