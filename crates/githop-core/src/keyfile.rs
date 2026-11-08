//! In-app SSH key creation and unlock (ADR 0007).
//!
//! The passphrase comes from GitHop's own form, is used in memory for the few
//! milliseconds it takes to encrypt or unlock the key, and is never written,
//! logged or kept (`Zeroizing`). The unlocked key goes straight to the Windows
//! ssh-agent ([`agent_add_request`]); nothing else ever sees it.
//!
//! Only ed25519 is created or unlocked here. Other key types the user brings
//! ("Use existing key file") keep the console `ssh-add` flow.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use zeroize::Zeroizing;

use crate::error::{CoreError, Result};

/// Shortest passphrase the form accepts (when one is used at all).
pub const MIN_PASSPHRASE_CHARS: usize = 8;

/// `<key>.pub`.
pub fn pub_path(key: &Path) -> PathBuf {
    let mut s = key.as_os_str().to_owned();
    s.push(".pub");
    PathBuf::from(s)
}

/// Create an ed25519 key pair at `key` / `key.pub`, encrypted with `passphrase`
/// when given. Never overwrites: fails if either file exists. Returns the
/// public key line (`ssh-ed25519 AAAA… comment`).
pub fn create_ed25519(key: &Path, comment: &str, passphrase: Option<&str>) -> Result<String> {
    let crypto = |e: ssh_key::Error| CoreError::Key(e.to_string());
    let mut private = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).map_err(crypto)?;
    private.set_comment(comment);
    let public = private.public_key().to_openssh().map_err(crypto)?;
    let stored = match passphrase.filter(|p| !p.is_empty()) {
        Some(p) => private.encrypt(&mut OsRng, p).map_err(crypto)?,
        None => private,
    };
    let text = stored.to_openssh(LineEnding::LF).map_err(crypto)?;

    if let Some(dir) = key.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
    }
    let pub_file = pub_path(key);
    write_new(key, text.as_bytes())?;
    if let Err(e) = write_new(&pub_file, format!("{public}\n").as_bytes()) {
        let _ = std::fs::remove_file(key);
        return Err(e);
    }
    Ok(public)
}

/// `create_new` write: refuses an existing file (never overwrites a key).
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| CoreError::io(path, e))?;
    f.write_all(bytes).map_err(|e| CoreError::io(path, e))?;
    f.sync_all().map_err(|e| CoreError::io(path, e))
}

/// An unlocked ed25519 key, wiped from memory when dropped.
pub struct Unlocked {
    seed: Zeroizing<[u8; 32]>,
    public: [u8; 32],
    comment: String,
}

/// Read `key` and unlock it with `passphrase` (ignored for an unencrypted key).
/// [`CoreError::WrongPassphrase`] when it doesn't match; [`CoreError::Key`] for a
/// key GitHop can't unlock in the app (not ed25519, unknown format).
pub fn unlock_ed25519(key: &Path, passphrase: Option<&str>) -> Result<Unlocked> {
    let text = Zeroizing::new(std::fs::read_to_string(key).map_err(|e| CoreError::io(key, e))?);
    let parsed = PrivateKey::from_openssh(text.as_bytes())
        .map_err(|_| CoreError::Key("this key isn't in a format GitHop can unlock".into()))?;
    let private = if parsed.is_encrypted() {
        let p = passphrase
            .filter(|p| !p.is_empty())
            .ok_or(CoreError::WrongPassphrase)?;
        parsed.decrypt(p).map_err(|_| CoreError::WrongPassphrase)?
    } else {
        parsed
    };
    let pair = private
        .key_data()
        .ed25519()
        .ok_or_else(|| CoreError::Key("only ed25519 keys can be remembered in the app".into()))?;
    Ok(Unlocked {
        seed: Zeroizing::new(pair.private.to_bytes()),
        public: pair.public.0,
        comment: private.comment().to_string(),
    })
}

/// SSH agent protocol `SSH2_AGENTC_ADD_IDENTITY` (17) for an ed25519 key, with
/// its 4-byte length prefix. Wiped from memory when dropped.
pub fn agent_add_request(key: &Unlocked) -> Zeroizing<Vec<u8>> {
    const ADD_IDENTITY: u8 = 17;
    let mut body = Zeroizing::new(Vec::with_capacity(160));
    body.push(ADD_IDENTITY);
    put_string(&mut body, b"ssh-ed25519");
    put_string(&mut body, &key.public);
    let mut secret = Zeroizing::new([0u8; 64]);
    secret[..32].copy_from_slice(&key.seed[..]);
    secret[32..].copy_from_slice(&key.public);
    put_string(&mut body, &secret[..]);
    put_string(&mut body, key.comment.as_bytes());
    let mut msg = Zeroizing::new(Vec::with_capacity(body.len() + 4));
    msg.extend_from_slice(&(body.len() as u32).to_be_bytes());
    msg.extend_from_slice(&body);
    msg
}

fn put_string(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
}

/// `SSH_AGENT_SUCCESS` (6) after a length prefix.
pub fn agent_reply_ok(reply: &[u8]) -> bool {
    reply.len() >= 5 && reply[4] == 6
}

/// The Windows OpenSSH agent's named pipe. Local only: never a network socket.
#[cfg(windows)]
pub const AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

/// Send one agent request and read the reply. `Ok(false)` when the agent said no.
#[cfg(windows)]
pub fn agent_send(request: &[u8]) -> Result<bool> {
    use std::io::Read;
    let pipe = Path::new(AGENT_PIPE);
    let mut conn = OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)
        .map_err(|e| CoreError::io(pipe, e))?;
    conn.write_all(request)
        .map_err(|e| CoreError::io(pipe, e))?;
    let mut len = [0u8; 4];
    conn.read_exact(&mut len)
        .map_err(|e| CoreError::io(pipe, e))?;
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > 64 * 1024 {
        return Err(CoreError::io(
            pipe,
            io::Error::new(io::ErrorKind::InvalidData, "unexpected agent reply"),
        ));
    }
    let mut body = vec![0u8; n];
    conn.read_exact(&mut body)
        .map_err(|e| CoreError::io(pipe, e))?;
    let mut reply = len.to_vec();
    reply.extend_from_slice(&body);
    Ok(agent_reply_ok(&reply))
}

#[cfg(not(windows))]
pub fn agent_send(_request: &[u8]) -> Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_an_encrypted_key_that_unlocks_with_its_passphrase_only() {
        let tmp = tempfile::tempdir().unwrap();
        let key = tmp.path().join("githop_work");
        let public = create_ed25519(&key, "jane@acme.example", Some("correct horse")).unwrap();
        assert!(public.starts_with("ssh-ed25519 "));
        assert!(public.ends_with(" jane@acme.example"));
        assert_eq!(
            std::fs::read_to_string(pub_path(&key)).unwrap().trim(),
            public
        );
        let text = std::fs::read_to_string(&key).unwrap();
        assert_eq!(crate::ssh::private_key_is_encrypted(&text), Some(true));
        assert!(!text.contains("correct horse"));

        let u = unlock_ed25519(&key, Some("correct horse")).unwrap();
        assert_eq!(u.comment, "jane@acme.example");
        assert!(matches!(
            unlock_ed25519(&key, Some("wrong")),
            Err(CoreError::WrongPassphrase)
        ));
        assert!(matches!(
            unlock_ed25519(&key, None),
            Err(CoreError::WrongPassphrase)
        ));
    }

    #[test]
    fn never_overwrites_an_existing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let key = tmp.path().join("githop_work");
        std::fs::write(&key, "mine").unwrap();
        assert!(create_ed25519(&key, "c", None).is_err());
        assert_eq!(std::fs::read_to_string(&key).unwrap(), "mine");
        assert!(!pub_path(&key).exists());
    }

    #[test]
    fn unencrypted_key_needs_no_passphrase() {
        let tmp = tempfile::tempdir().unwrap();
        let key = tmp.path().join("k");
        create_ed25519(&key, "c", None).unwrap();
        let text = std::fs::read_to_string(&key).unwrap();
        assert_eq!(crate::ssh::private_key_is_encrypted(&text), Some(false));
        assert!(unlock_ed25519(&key, None).is_ok());
    }

    #[test]
    fn agent_request_matches_the_protocol() {
        let u = Unlocked {
            seed: Zeroizing::new([1u8; 32]),
            public: [2u8; 32],
            comment: "c".into(),
        };
        let m = agent_add_request(&u);
        let len = u32::from_be_bytes(m[..4].try_into().unwrap()) as usize;
        assert_eq!(len, m.len() - 4);
        assert_eq!(m[4], 17);
        // string "ssh-ed25519"
        assert_eq!(&m[5..9], &11u32.to_be_bytes());
        assert_eq!(&m[9..20], b"ssh-ed25519");
        // public (32), then secret (seed || public = 64), then comment
        assert_eq!(&m[20..24], &32u32.to_be_bytes());
        assert_eq!(&m[56..60], &64u32.to_be_bytes());
        assert_eq!(&m[60..92], &[1u8; 32]);
        assert_eq!(&m[92..124], &[2u8; 32]);
        assert_eq!(&m[124..], &[0, 0, 0, 1, b'c']);
        assert!(agent_reply_ok(&[0, 0, 0, 1, 6]));
        assert!(!agent_reply_ok(&[0, 0, 0, 1, 5]));
    }
}
