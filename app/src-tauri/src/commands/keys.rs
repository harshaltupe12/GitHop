//! SSH key commands: suggest a path, generate (passphrase typed in a console
//! window, never over IPC), read the public key, test the connection.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use githop_core::paths::to_slash;
use githop_core::ssh::{self, ConnectionTest, PassphraseState, RememberOutcome, WindowsAgent};
use githop_core::{CoreError, Profile};
use tauri::{AppHandle, Runtime};

use crate::dto::{GeneratedKey, IpcError, IpcResult};
use crate::events;
use crate::ops::{self, blocking};
use crate::validate;

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
/// The passphrase is typed by the user into a separate console window that
/// `ssh-keygen` opens itself — it never passes through GitHop or the WebView.
/// With `no_passphrase` the key is created silently with `-N ""`.
#[tauri::command]
pub async fn generate_key<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    email: Option<String>,
    no_passphrase: bool,
) -> IpcResult<GeneratedKey> {
    validate::profile_id(&id)?;
    let generated =
        blocking(move || generate_key_blocking(&id, email.as_deref(), no_passphrase)).await?;
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

/// "Remember my passphrase": `ssh-add <key>` in a visible console window (the
/// passphrase is typed there; it never passes through GitHop). Needs the
/// ssh-agent service running; otherwise returns `agent-off`.
#[tauri::command]
pub async fn remember_passphrase(id: String) -> IpcResult<RememberOutcome> {
    validate::profile_id(&id)?;
    blocking(move || remember_blocking(&id, false)).await
}

/// "Turn on": one UAC prompt sets the ssh-agent service to start
/// automatically and starts it, then `ssh-add` as above.
#[tauri::command]
pub async fn turn_on_agent(id: String) -> IpcResult<RememberOutcome> {
    validate::profile_id(&id)?;
    blocking(move || remember_blocking(&id, true)).await
}

fn remember_blocking(id: &str, turn_on: bool) -> IpcResult<RememberOutcome> {
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
    let outcome = if turn_on {
        ssh::turn_on_agent_and_remember(&WindowsAgent, &profile.ssh_key)?
    } else {
        ssh::remember_passphrase(&WindowsAgent, &profile.ssh_key)?
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
    no_passphrase: bool,
) -> IpcResult<GeneratedKey> {
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
    if !reused {
        // INTEGRATE: replace with a core call that can open a console, e.g.
        // `ssh::keygen_with(&key, &comment, no_passphrase, ssh::Console::New)`.
        // Core's `ssh::keygen` inherits stdio, which has no console in the tray app.
        keygen_in_console(&key, &comment, no_passphrase)?;
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
    Ok(GeneratedKey {
        public_key: ssh::read_public_key(&key)?.trim().to_string(),
        path: to_slash(&key),
        reused,
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

/// Windows process creation flags.
#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `ssh-keygen -t ed25519 -f <key> -C <comment> [-N ""]` (argument array).
/// With a passphrase, ssh-keygen gets its own visible console window and asks
/// there; GitHop waits for it to finish. Never overwrites an existing key.
fn keygen_in_console(key: &Path, comment: &str, no_passphrase: bool) -> IpcResult<()> {
    for existing in [key.to_path_buf(), pub_path(key)] {
        if std::fs::symlink_metadata(&existing).is_ok() {
            return Err(IpcError::conflict(format!(
                "A key already exists at {}. GitHop won't overwrite it.",
                to_slash(&existing)
            )));
        }
    }
    if let Some(parent) = key.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    let exe = locate_ssh_keygen().ok_or_else(|| {
        IpcError::not_found("ssh-keygen was not found. Install Git for Windows or OpenSSH.")
    })?;
    let mut cmd = Command::new(&exe);
    cmd.arg("-t")
        .arg("ed25519")
        .arg("-f")
        .arg(key)
        .arg("-C")
        .arg(comment);
    if no_passphrase {
        // An empty argument, not a shell-quoted `""`.
        cmd.arg("-N").arg("");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(if no_passphrase {
            CREATE_NO_WINDOW
        } else {
            CREATE_NEW_CONSOLE
        });
    }
    let status = cmd.status().map_err(|e| CoreError::io(&exe, e))?;
    if !status.success() {
        return Err(IpcError::new(
            crate::dto::ErrorCode::Io,
            "The key wasn't created. ssh-keygen was closed or failed. Try again.",
        ));
    }
    if !key.is_file() {
        return Err(IpcError::new(
            crate::dto::ErrorCode::Io,
            "ssh-keygen finished but didn't create the key. Try again.",
        ));
    }
    Ok(())
}

/// `<key>.pub` (appended — key names may contain dots).
fn pub_path(key: &Path) -> PathBuf {
    let mut s = key.as_os_str().to_owned();
    s.push(".pub");
    PathBuf::from(s)
}

/// PATH, then Windows' OpenSSH, then Git for Windows' bundled copy.
fn locate_ssh_keygen() -> Option<PathBuf> {
    if let Some(p) = githop_core::git::find_on_path("ssh-keygen") {
        return Some(p);
    }
    let mut candidates = Vec::new();
    if let Some(root) = std::env::var_os("SystemRoot") {
        candidates.push(
            PathBuf::from(root)
                .join("System32")
                .join("OpenSSH")
                .join("ssh-keygen.exe"),
        );
    }
    if let Some(git) = githop_core::git::find_on_path("git") {
        // <Git>/cmd/git.exe → <Git>/usr/bin/ssh-keygen.exe
        for dir in git.ancestors().skip(1).take(3) {
            candidates.push(dir.join("usr").join("bin").join("ssh-keygen.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
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

    #[test]
    fn keygen_refuses_existing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let key = tmp.path().join("githop_x");
        std::fs::write(&key, "secret").unwrap();
        let e = keygen_in_console(&key, "a@b.c", true).unwrap_err();
        assert_eq!(e.code, crate::dto::ErrorCode::Conflict);
        assert_eq!(std::fs::read_to_string(&key).unwrap(), "secret");

        let key2 = tmp.path().join("githop_y");
        std::fs::write(pub_path(&key2), "ssh-ed25519 AAAA").unwrap();
        let e = keygen_in_console(&key2, "a@b.c", true).unwrap_err();
        assert_eq!(e.code, crate::dto::ErrorCode::Conflict);
    }
}
