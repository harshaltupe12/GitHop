//! SSH key commands: suggest a path, generate (passphrase typed in a console
//! window, never over IPC), read the public key, test the connection.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use githop_core::paths::to_slash;
use githop_core::ssh::{self, ConnectionTest};
use githop_core::{CoreError, Profile};
use tauri::{AppHandle, Runtime};

use crate::dto::{IpcError, IpcResult};
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
/// The passphrase is typed by the user into a separate console window that
/// `ssh-keygen` opens itself — it never passes through GitHop or the WebView.
/// With `no_passphrase` the key is created silently with `-N ""`.
#[tauri::command]
pub async fn generate_key<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    no_passphrase: bool,
) -> IpcResult<String> {
    validate::profile_id(&id)?;
    let public = blocking(move || generate_key_blocking(&id, no_passphrase)).await?;
    events::data_changed(&app);
    Ok(public)
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
        Ok(ssh::test_connection(&p.ssh_key)?)
    })
    .await
}

fn generate_key_blocking(id: &str, no_passphrase: bool) -> IpcResult<String> {
    let git = ops::git()?;
    let profile: Profile = {
        let store = ops::store()?;
        ops::profile(store.profiles(), id)?.clone()
    };
    let key = if profile.ssh_key.as_os_str().is_empty() {
        ssh::default_key_path(&git, &profile.id)
            .ok_or_else(|| IpcError::not_found("GitHop couldn't find your home folder."))?
    } else {
        profile.ssh_key.clone()
    };

    // INTEGRATE: replace with a core call that can open a console, e.g.
    // `ssh::keygen_with(&key, &profile.email, no_passphrase, ssh::Console::New)`.
    // Core's `ssh::keygen` inherits stdio, which has no console in the tray app.
    keygen_in_console(&key, &profile.email, no_passphrase)?;
    ssh::restrict_key_acl(&key)?;

    if profile.ssh_key != key {
        let _guard = ops::write_lock();
        let mut store = ops::store()?;
        let mut set = store.profiles().clone();
        if let Some(p) = set.get_mut(&profile.id) {
            p.ssh_key = key.clone();
        }
        let settings = store.settings().clone();
        store.save(&set, &settings)?;
    }
    Ok(ssh::read_public_key(&key)?)
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
