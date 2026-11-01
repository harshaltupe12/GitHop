//! Encrypted export / import of profiles (Phase 6 §3, Plan §9.2, §10 "Multi-machine").
//!
//! A `.githop` file is an [`age`] file encrypted with a password (scrypt mode)
//! that holds one TOML bundle:
//!
//! ```toml
//! format = "githop-export"
//! version = 1
//! created = "2026-10-31T10:00:00Z"
//! [profiles]               # ProfileSet; `ssh_key` is a key file NAME, not a path
//! [settings]               # enforce, logging, log_retention_days only
//! [[keys]]                 # only with "Include SSH keys"
//! name = "githop_personal"
//! private = "<base64>"     # still protected by its own SSH passphrase if it had one
//! public = "ssh-ed25519 …"
//! ```
//!
//! Never exported: backups, logs, pinned repo paths, hooks state, the previous
//! hooks path, any machine-specific absolute key path.
//!
//! Safety rules for import (the file may come from anywhere):
//! - size-limited before and after decryption, strict schema (`deny_unknown_fields`),
//!   no control characters in any text field;
//! - key names must match `githop_[a-z0-9-]{1,32}` — no separators, `..`, drive
//!   letters, reserved Windows names or non-ASCII can get through;
//! - key paths are rebuilt under this machine's `~/.ssh`;
//! - existing key files are never overwritten unless the resolution is `replace`
//!   (the old files are then renamed aside, not deleted); new key files are
//!   created with `create_new` and restricted to the current user.
//!
//! The password lives only in memory for the duration of a call. GitHop never
//! stores it anywhere.

use std::collections::BTreeSet;
use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use age::secrecy::SecretString;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::CoreError;
use crate::git::Git;
use crate::paths::{folder_key, normalize_folder, to_slash, Paths};
use crate::profile::{Profile, ProfileColor, ProfileSet, ValidationIssue, PROFILES_VERSION};
use crate::settings::{EnforceMode, Settings};
use crate::ssh;
use crate::store::{atomic_write, write_new, Store};

/// `format` value of every bundle.
pub const EXPORT_FORMAT: &str = "githop-export";
/// Bundle schema version this GitHop writes and reads.
pub const EXPORT_VERSION: u32 = 1;
/// Suggested file extension (without the dot).
pub const EXPORT_EXTENSION: &str = "githop";
/// Minimum password length, in characters.
pub const MIN_PASSWORD_CHARS: usize = 12;
/// Maximum password length, in characters (keeps scrypt input sane).
pub const MAX_PASSWORD_CHARS: usize = 1024;
/// Largest accepted file, both encrypted and decrypted.
pub const MAX_BUNDLE_BYTES: u64 = 1024 * 1024;
/// Largest accepted private (or public) key file.
pub const MAX_KEY_BYTES: usize = 64 * 1024;
/// Most profiles one bundle may hold.
pub const MAX_PROFILES: usize = 64;

/// scrypt work factor for new exports: `N = 2^18` (≈ 256 MiB, about a second on
/// a recent PC). Fixed rather than calibrated, so a file made on a fast desktop
/// still opens on a slow laptop.
pub const EXPORT_WORK_FACTOR: u8 = 18;
/// Highest scrypt work factor accepted on import (`2^20` ≈ 1 GiB). Caps the
/// CPU/memory a hostile file can make GitHop spend.
pub const IMPORT_MAX_WORK_FACTOR: u8 = 20;

/// Prefix every importable key file name must start with.
const KEY_PREFIX: &str = "githop_";
/// Max length of the part after [`KEY_PREFIX`] (same as a profile id).
const KEY_SUFFIX_MAX: usize = 32;
/// Longest accepted `created` value.
const CREATED_MAX: usize = 64;
/// Retention days accepted from a bundle.
const RETENTION_RANGE: std::ops::RangeInclusive<u32> = 1..=365;

/// Result alias for this module.
pub type Result<T> = std::result::Result<T, ExportError>;

/// Everything that can go wrong exporting or importing.
///
/// Messages start lower-case like [`CoreError`]; front-ends capitalise them.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// Export password shorter than [`MIN_PASSWORD_CHARS`].
    #[error("the password must be at least {min} characters long")]
    WeakPassword { min: usize },

    /// Password longer than [`MAX_PASSWORD_CHARS`].
    #[error("the password is too long (more than {max} characters)")]
    PasswordTooLong { max: usize },

    /// No profiles to export.
    #[error("there are no profiles to export")]
    NothingToExport,

    /// Decryption failed: wrong password, or the encrypted key was changed.
    #[error("the password is wrong, or the file was changed after it was exported")]
    WrongPassword,

    /// Not an age file, not password-encrypted, truncated, or tampered with.
    #[error("this isn't a GitHop export file, or it was damaged or changed after export")]
    Corrupt,

    /// The file asks for more scrypt work than [`IMPORT_MAX_WORK_FACTOR`].
    #[error("this file needs more work to open than GitHop allows, so it wasn't opened")]
    ExcessiveWork,

    /// Encrypted or decrypted data is over [`MAX_BUNDLE_BYTES`].
    #[error("the file is too large to be a GitHop export (over {max} bytes)")]
    TooLarge { max: u64 },

    /// Bundle written by a newer GitHop.
    #[error("this export has format version {found}; this GitHop reads version {supported}")]
    UnsupportedVersion { found: u32, supported: u32 },

    /// The decrypted bundle doesn't match the schema.
    #[error("the export file isn't valid: {0}")]
    Invalid(String),

    /// A key name that could escape `~/.ssh` or isn't a GitHop key name.
    #[error("the export has an unsafe SSH key name {0:?}; only githop_<id> names are allowed")]
    UnsafeKeyName(String),

    /// The profiles in the bundle (or the merged result) don't validate.
    #[error("the profiles in the export aren't valid ({} issue(s))", .0.len())]
    InvalidProfiles(Vec<ValidationIssue>),

    /// Profiles that clash with existing ones but got no resolution.
    #[error("choose skip, replace or rename for: {}", .0.join(", "))]
    Unresolved(Vec<String>),

    /// A resolution can't be carried out (e.g. rename target taken).
    #[error("{0}")]
    Conflict(String),

    /// A key file couldn't be read for export or written on import.
    #[error("SSH key {}: {reason}", .path.display())]
    Key { path: PathBuf, reason: String },

    /// No home folder, so no `~/.ssh`.
    #[error("couldn't find your home folder for SSH keys")]
    NoHome,

    /// Store / file system error.
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl ExportError {
    /// True for problems with the user's input or file (wrong password,
    /// bad/hostile file, unresolved conflicts) — as opposed to internal/I/O errors.
    pub fn is_user_error(&self) -> bool {
        !matches!(self, ExportError::Core(_) | ExportError::NoHome)
    }
}

// ------------------------------------------------------------------ public DTOs

/// What clashes between an imported profile and this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Same profile id.
    Id,
    /// Same email (case-insensitive).
    Email,
    /// Same colour (resolved automatically by picking a free colour).
    Color,
    /// Shares a GitHub owner.
    Owner,
    /// A different key file already exists at the target path.
    KeyFile,
}

impl ConflictKind {
    /// Conflicts that need a skip / replace / rename choice. Colour clashes are
    /// fixed automatically.
    pub fn needs_choice(self) -> bool {
        !matches!(self, ConflictKind::Color)
    }
}

/// One clash, as shown in the import dialog (IPC `ImportConflict`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportConflict {
    /// Id of the profile in the file.
    pub profile_id: String,
    pub kind: ConflictKind,
    /// Id of the existing profile it clashes with (`""` for a key file no profile uses).
    pub existing_id: String,
}

/// What an import would do (IPC `ImportPreview`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPreview {
    /// Profiles in the file, with key paths already rewritten to this machine's `~/.ssh`.
    pub profiles: Vec<Profile>,
    pub conflicts: Vec<ImportConflict>,
    /// Key file names included in the file (`githop_<id>`).
    pub keys: Vec<String>,
    /// True when applying will also copy the file's settings (enforce mode,
    /// logging, retention). Only happens on a machine with no profiles yet, so an
    /// import never weakens an existing setup.
    pub include_settings: bool,
}

/// Choice for one imported profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportAction {
    /// Don't import this profile (or its key).
    Skip,
    /// Remove the existing profiles it clashes with (id, email, owner) and
    /// overwrite a clashing key file (the old one is renamed aside).
    Replace,
    /// Import under a new id; clashing owners/folders are dropped and a clashing
    /// key is written as `githop_<new id>`.
    Rename,
}

/// Resolution for one imported profile (IPC `ImportResolution`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportResolution {
    /// Id of the profile in the file.
    pub profile_id: String,
    pub action: ImportAction,
    /// New id for `rename`; picked automatically (`<id>-2`, …) when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_id: Option<String>,
}

/// What [`export`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    /// Profile ids, in file order.
    pub profiles: Vec<String>,
    /// Key file names included.
    pub keys: Vec<String>,
    /// Key files that were asked for but don't exist (profile exported without key).
    pub missing_keys: Vec<PathBuf>,
}

/// What [`apply_bundle`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// The saved profile set.
    pub profiles: ProfileSet,
    /// Final ids of the imported profiles.
    pub imported: Vec<String>,
    /// File ids that were skipped.
    pub skipped: Vec<String>,
    /// Existing ids removed by `replace`.
    pub replaced: Vec<String>,
    /// `(file id, new id)` for renamed profiles.
    pub renamed: Vec<(String, String)>,
    /// Key files written.
    pub keys_written: Vec<PathBuf>,
    /// Old key files renamed aside by `replace` (new name).
    pub keys_moved_aside: Vec<PathBuf>,
    /// Imported profiles whose key file isn't on this machine yet.
    pub missing_keys: Vec<(String, PathBuf)>,
    /// True when the file's settings were applied.
    pub settings_applied: bool,
    /// True when the default profile changed (run `githop use` to apply it globally).
    pub default_changed: bool,
    /// Plain-language notes (colour changed, owner dropped, …).
    pub notes: Vec<String>,
}

// ------------------------------------------------------------------ bundle schema

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleFile {
    format: String,
    version: u32,
    created: String,
    profiles: BundleProfiles,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settings: Option<BundleSettings>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    keys: Vec<BundleKey>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleProfiles {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<String>,
    #[serde(rename = "profile", default)]
    profiles: Vec<BundleProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleProfile {
    id: String,
    label: String,
    name: String,
    email: String,
    color: ProfileColor,
    /// Key file name only (`githop_<id>`).
    ssh_key: String,
    #[serde(default)]
    owners: Vec<String>,
    #[serde(default)]
    folders: Vec<String>,
}

/// Settings subset carried by a bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSettings {
    pub enforce: EnforceMode,
    pub logging: bool,
    pub log_retention_days: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleKey {
    name: String,
    private: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public: Option<String>,
}

/// One decoded key from a bundle. Private bytes are wiped on drop.
#[derive(Clone)]
struct KeyData {
    name: String,
    private: Zeroizing<Vec<u8>>,
    public: Option<String>,
}

/// A decrypted, fully validated export. Hold it only as long as needed: it may
/// contain private keys (wiped from memory on drop).
#[derive(Clone)]
pub struct Bundle {
    created: String,
    default: Option<String>,
    profiles: Vec<BundleProfile>,
    settings: Option<BundleSettings>,
    keys: Vec<KeyData>,
}

impl fmt::Debug for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bundle")
            .field("created", &self.created)
            .field("default", &self.default)
            .field(
                "profiles",
                &self.profiles.iter().map(|p| &p.id).collect::<Vec<_>>(),
            )
            .field("settings", &self.settings)
            .field(
                "keys",
                &self.keys.iter().map(|k| &k.name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Bundle {
    /// When the file was exported (RFC 3339, as written by the exporter).
    pub fn created(&self) -> &str {
        &self.created
    }

    /// Profile ids in the file.
    pub fn profile_ids(&self) -> Vec<String> {
        self.profiles.iter().map(|p| p.id.clone()).collect()
    }

    /// Default profile id recorded in the file.
    pub fn default_id(&self) -> Option<&str> {
        self.default.as_deref()
    }

    /// Key file names included in the file.
    pub fn key_names(&self) -> Vec<String> {
        self.keys.iter().map(|k| k.name.clone()).collect()
    }

    /// Settings carried by the file.
    pub fn settings(&self) -> Option<BundleSettings> {
        self.settings
    }

    fn key(&self, name: &str) -> Option<&KeyData> {
        self.keys.iter().find(|k| k.name == name)
    }
}

// ------------------------------------------------------------------ validation helpers

/// `githop_[a-z0-9-]{1,32}` — the only key file names import accepts.
///
/// This alone rules out path separators, `..`, drive letters / absolute paths,
/// reserved Windows device names (`CON`, `NUL`, …), dots, spaces and non-ASCII.
pub fn is_safe_key_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(KEY_PREFIX) else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= KEY_SUFFIX_MAX
        && rest
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Check an export password: [`MIN_PASSWORD_CHARS`]..=[`MAX_PASSWORD_CHARS`] characters.
pub fn check_password(password: &str) -> Result<()> {
    let n = password.chars().count();
    if n < MIN_PASSWORD_CHARS {
        return Err(ExportError::WeakPassword {
            min: MIN_PASSWORD_CHARS,
        });
    }
    if n > MAX_PASSWORD_CHARS {
        return Err(ExportError::PasswordTooLong {
            max: MAX_PASSWORD_CHARS,
        });
    }
    Ok(())
}

/// A free id based on `id`: `id-2`, `id-3`, … (kept within 32 characters).
pub fn suggest_id(id: &str, taken: &[String]) -> String {
    let base: String = id.chars().take(32).collect();
    for n in 2..1000u32 {
        let suffix = format!("-{n}");
        let keep = 32usize.saturating_sub(suffix.len());
        let stem: String = base.chars().take(keep).collect();
        let stem = stem.trim_end_matches('-');
        let candidate = format!("{stem}{suffix}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    format!("{base}-new")
}

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// At most 40 characters of untrusted text, for messages (printed with `{:?}`).
fn shown(s: &str) -> String {
    s.chars().take(40).collect()
}

fn invalid(msg: impl Into<String>) -> ExportError {
    ExportError::Invalid(msg.into())
}

fn ssh_dir(git: &Git) -> Result<PathBuf> {
    ssh::ssh_dir(git).ok_or(ExportError::NoHome)
}

/// Case-insensitive path comparison on forward-slash form.
fn same_path(a: &Path, b: &Path) -> bool {
    to_slash(a).to_lowercase() == to_slash(b).to_lowercase()
}

fn pub_path(key: &Path) -> PathBuf {
    let mut s = key.as_os_str().to_owned();
    s.push(".pub");
    PathBuf::from(s)
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Looks like an OpenSSH / PEM private key file.
fn looks_like_private_key(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    text.trim_start().starts_with("-----BEGIN ")
}

/// One printable line starting with a known SSH key type.
fn valid_public_key(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_KEY_BYTES
        && !has_control(s)
        && (s.starts_with("ssh-") || s.starts_with("ecdsa-sha2-") || s.starts_with("sk-"))
}

/// Read at most `max` bytes; `None` when the file is bigger.
fn read_limited(path: &Path, max: u64) -> std::io::Result<Option<Vec<u8>>> {
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    file.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Ok(None);
    }
    Ok(Some(buf))
}

// ------------------------------------------------------------------ crypto

/// Encrypt `plain` with `password` (age scrypt mode, work factor `2^log_n`).
///
/// `log_n` is clamped to `1..=63`. Exports use [`EXPORT_WORK_FACTOR`]; tests may
/// pass a tiny factor to stay fast.
#[doc(hidden)]
pub fn encrypt_bytes(plain: &[u8], password: &str, log_n: u8) -> Result<Vec<u8>> {
    let mut recipient = age::scrypt::Recipient::new(SecretString::from(password.to_owned()));
    recipient.set_work_factor(log_n.clamp(1, 63));
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .map_err(|e| invalid(format!("couldn't encrypt: {e}")))?;
    let mut out = Vec::with_capacity(plain.len() + 512);
    let io_err = |e: std::io::Error| invalid(format!("couldn't encrypt: {e}"));
    let mut writer = encryptor.wrap_output(&mut out).map_err(io_err)?;
    writer.write_all(plain).map_err(io_err)?;
    writer.finish().map_err(io_err)?;
    Ok(out)
}

/// Decrypt an export file's bytes. Never panics on hostile input; the output is
/// capped at [`MAX_BUNDLE_BYTES`] and wiped from memory on drop.
pub fn decrypt_bytes(bytes: &[u8], password: &str) -> Result<Zeroizing<Vec<u8>>> {
    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(ExportError::TooLarge {
            max: MAX_BUNDLE_BYTES,
        });
    }
    if password.is_empty() {
        return Err(ExportError::WrongPassword);
    }
    if password.chars().count() > MAX_PASSWORD_CHARS {
        return Err(ExportError::PasswordTooLong {
            max: MAX_PASSWORD_CHARS,
        });
    }
    let decryptor = age::Decryptor::new_buffered(bytes).map_err(map_decrypt_error)?;
    if !decryptor.is_scrypt() {
        return Err(ExportError::Corrupt);
    }
    let mut identity = age::scrypt::Identity::new(SecretString::from(password.to_owned()));
    identity.set_max_work_factor(IMPORT_MAX_WORK_FACTOR);
    let reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(map_decrypt_error)?;
    let mut out = Zeroizing::new(Vec::new());
    reader
        .take(MAX_BUNDLE_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|_| ExportError::Corrupt)?;
    if out.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(ExportError::TooLarge {
            max: MAX_BUNDLE_BYTES,
        });
    }
    Ok(out)
}

fn map_decrypt_error(e: age::DecryptError) -> ExportError {
    match e {
        age::DecryptError::DecryptionFailed
        | age::DecryptError::KeyDecryptionFailed
        | age::DecryptError::NoMatchingKeys => ExportError::WrongPassword,
        age::DecryptError::ExcessiveWork { .. } => ExportError::ExcessiveWork,
        _ => ExportError::Corrupt,
    }
}

// ------------------------------------------------------------------ export

/// Export every profile (plus the settings subset, and the SSH keys when
/// `include_keys`) to `out`, encrypted with `password` (≥ [`MIN_PASSWORD_CHARS`]).
///
/// Written atomically (temp file + rename); the parent folder must exist.
/// A profile whose key file is missing is still exported (listed in
/// [`ExportSummary::missing_keys`]).
pub fn export(
    paths: &Paths,
    git: &Git,
    out: &Path,
    password: &str,
    include_keys: bool,
) -> Result<ExportSummary> {
    export_with_work_factor(paths, git, out, password, include_keys, EXPORT_WORK_FACTOR)
}

/// [`export`] with an explicit scrypt work factor (tests only).
#[doc(hidden)]
pub fn export_with_work_factor(
    paths: &Paths,
    _git: &Git,
    out: &Path,
    password: &str,
    include_keys: bool,
    log_n: u8,
) -> Result<ExportSummary> {
    check_password(password)?;
    let store = Store::open_at(paths.clone())?;
    let set = store.profiles();
    if set.profiles.is_empty() {
        return Err(ExportError::NothingToExport);
    }

    // Key file name per profile: its own name when it's already a GitHop name,
    // else `githop_<id>`. Two different files may not end up with one name.
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    let mut profiles = Vec::with_capacity(set.profiles.len());
    for p in &set.profiles {
        let own = p
            .ssh_key
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| is_safe_key_name(n))
            .map(str::to_string);
        let fallback = format!("{KEY_PREFIX}{}", p.id);
        let mut name = own.unwrap_or_else(|| fallback.clone());
        let taken_by_other = |n: &str, names: &[(String, PathBuf)]| {
            names
                .iter()
                .any(|(other, path)| other == n && !same_path(path, &p.ssh_key))
        };
        if taken_by_other(&name, &names) {
            name = fallback;
        }
        if taken_by_other(&name, &names) || !is_safe_key_name(&name) {
            return Err(ExportError::Conflict(format!(
                "two profiles' SSH keys would get the same name in the export ({name}); \
                 give profile \"{}\" its own key file",
                p.id
            )));
        }
        if !names.iter().any(|(n, _)| *n == name) {
            names.push((name.clone(), p.ssh_key.clone()));
        }
        profiles.push(BundleProfile {
            id: p.id.clone(),
            label: p.label.clone(),
            name: p.name.clone(),
            email: p.email.clone(),
            color: p.color,
            ssh_key: name,
            owners: p.owners.clone(),
            folders: p.folders.clone(),
        });
    }

    let mut keys = Vec::new();
    let mut missing_keys = Vec::new();
    if include_keys {
        for (name, path) in &names {
            let private = match read_limited(path, MAX_KEY_BYTES as u64) {
                Ok(Some(b)) => Zeroizing::new(b),
                Ok(None) => {
                    return Err(ExportError::Key {
                        path: path.clone(),
                        reason: "the file is too large to be an SSH key".into(),
                    })
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    missing_keys.push(path.clone());
                    continue;
                }
                Err(e) => {
                    return Err(ExportError::Key {
                        path: path.clone(),
                        reason: e.to_string(),
                    })
                }
            };
            if !looks_like_private_key(&private) {
                return Err(ExportError::Key {
                    path: path.clone(),
                    reason: "this isn't an OpenSSH private key file".into(),
                });
            }
            let public = std::fs::read_to_string(pub_path(path))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| valid_public_key(s));
            keys.push(BundleKey {
                name: name.clone(),
                private: base64::engine::general_purpose::STANDARD.encode(&*private),
                public,
            });
        }
    }

    let settings = store.settings();
    let file = BundleFile {
        format: EXPORT_FORMAT.to_string(),
        version: EXPORT_VERSION,
        created: now_rfc3339(),
        profiles: BundleProfiles {
            version: PROFILES_VERSION,
            default: set.default.clone(),
            profiles,
        },
        settings: Some(BundleSettings {
            enforce: settings.enforce,
            logging: settings.logging,
            log_retention_days: settings.log_retention_days,
        }),
        keys,
    };
    let summary = ExportSummary {
        profiles: file
            .profiles
            .profiles
            .iter()
            .map(|p| p.id.clone())
            .collect(),
        keys: file.keys.iter().map(|k| k.name.clone()).collect(),
        missing_keys,
    };

    let plain = Zeroizing::new(
        toml::to_string_pretty(&file).map_err(|e| invalid(format!("couldn't build: {e}")))?,
    );
    // The base64 copies of the keys are no longer needed.
    drop(file);
    let encrypted = encrypt_bytes(plain.as_bytes(), password, log_n)?;
    atomic_write(out, &encrypted)?;
    Ok(summary)
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    let now = now.replace_nanosecond(0).unwrap_or(now);
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

// ------------------------------------------------------------------ import: parse

/// Parse and validate a decrypted bundle. Pure (no I/O); never panics.
///
/// Checks: format/version, schema (no unknown fields), sizes and counts, no
/// control characters, safe key names, decodable keys, every profile's key
/// name is safe, and the profiles validate as a set.
pub fn parse_bundle(plain: &[u8]) -> Result<Bundle> {
    if plain.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(ExportError::TooLarge {
            max: MAX_BUNDLE_BYTES,
        });
    }
    let text = std::str::from_utf8(plain).map_err(|_| invalid("it isn't UTF-8 text"))?;

    // Probe the header first so a newer format reports a version, not a schema error.
    #[derive(Deserialize)]
    struct Probe {
        format: Option<toml::Value>,
        version: Option<toml::Value>,
    }
    let probe: Probe = toml::from_str(text).map_err(|_| invalid("it isn't a GitHop bundle"))?;
    match probe.format.as_ref().and_then(toml::Value::as_str) {
        Some(EXPORT_FORMAT) => {}
        _ => return Err(invalid("it isn't a GitHop bundle")),
    }
    match probe.version.as_ref().and_then(toml::Value::as_integer) {
        Some(v) if v == i64::from(EXPORT_VERSION) => {}
        Some(v) => {
            return Err(ExportError::UnsupportedVersion {
                found: u32::try_from(v).unwrap_or(u32::MAX),
                supported: EXPORT_VERSION,
            })
        }
        None => return Err(invalid("the version is missing")),
    }

    let file: BundleFile = toml::from_str(text).map_err(|e| invalid(short_toml_error(&e)))?;
    if file.profiles.version != PROFILES_VERSION {
        return Err(ExportError::UnsupportedVersion {
            found: file.profiles.version,
            supported: PROFILES_VERSION,
        });
    }
    if file.created.len() > CREATED_MAX || has_control(&file.created) {
        return Err(invalid("the created date isn't valid"));
    }
    if file.profiles.profiles.is_empty() {
        return Err(invalid("it has no profiles"));
    }
    if file.profiles.profiles.len() > MAX_PROFILES {
        return Err(invalid(format!("it has more than {MAX_PROFILES} profiles")));
    }
    if file.keys.len() > MAX_PROFILES {
        return Err(invalid(format!("it has more than {MAX_PROFILES} keys")));
    }
    if let Some(s) = &file.settings {
        if !RETENTION_RANGE.contains(&s.log_retention_days) {
            return Err(invalid("log_retention_days is out of range"));
        }
    }
    if let Some(d) = &file.profiles.default {
        if has_control(d) {
            return Err(invalid("the default profile id has control characters"));
        }
    }

    for p in &file.profiles.profiles {
        let texts = [&p.id, &p.label, &p.name, &p.email, &p.ssh_key];
        if texts.iter().any(|t| has_control(t))
            || p.owners.iter().chain(&p.folders).any(|t| has_control(t))
        {
            return Err(invalid(format!(
                "profile {:?} has control characters in its text",
                shown(&p.id)
            )));
        }
        if p.owners.len() > 256 || p.folders.len() > 256 {
            return Err(invalid(format!(
                "profile {:?} has too many rules",
                shown(&p.id)
            )));
        }
        if !is_safe_key_name(&p.ssh_key) {
            return Err(ExportError::UnsafeKeyName(shown(&p.ssh_key)));
        }
    }

    let mut keys: Vec<KeyData> = Vec::with_capacity(file.keys.len());
    for k in &file.keys {
        if !is_safe_key_name(&k.name) {
            return Err(ExportError::UnsafeKeyName(shown(&k.name)));
        }
        if keys.iter().any(|o| o.name == k.name) {
            return Err(invalid(format!("key {} appears twice", k.name)));
        }
        if !file.profiles.profiles.iter().any(|p| p.ssh_key == k.name) {
            return Err(invalid(format!("key {} isn't used by any profile", k.name)));
        }
        if k.private.len() > MAX_KEY_BYTES * 2 {
            return Err(invalid(format!("key {} is too large", k.name)));
        }
        let private = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(k.private.trim())
                .map_err(|_| invalid(format!("key {} isn't valid base64", k.name)))?,
        );
        if private.is_empty() || private.len() > MAX_KEY_BYTES || !looks_like_private_key(&private)
        {
            return Err(invalid(format!(
                "key {} isn't an OpenSSH private key",
                k.name
            )));
        }
        let public = match &k.public {
            Some(p) => {
                let p = p.trim();
                if !valid_public_key(p) {
                    return Err(invalid(format!("public key {} isn't valid", k.name)));
                }
                Some(p.to_string())
            }
            None => None,
        };
        keys.push(KeyData {
            name: k.name.clone(),
            private,
            public,
        });
    }

    // Validate the profiles as a set, with key paths under a placeholder home.
    let placeholder = PathBuf::from("C:/githop-import/.ssh");
    let set = ProfileSet {
        version: PROFILES_VERSION,
        default: file.profiles.default.clone(),
        profiles: file
            .profiles
            .profiles
            .iter()
            .map(|p| to_profile(p, &placeholder))
            .collect(),
    };
    set.validate().map_err(ExportError::InvalidProfiles)?;

    Ok(Bundle {
        created: file.created.clone(),
        default: file.profiles.default.clone(),
        profiles: file.profiles.profiles.clone(),
        settings: file.settings,
        keys,
    })
}

/// First line of a TOML error, without echoing file content.
fn short_toml_error(e: &toml::de::Error) -> String {
    let msg = e.message();
    let line: String = msg
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    if line.is_empty() {
        "the contents don't match the expected layout".to_string()
    } else {
        line
    }
}

fn to_profile(p: &BundleProfile, ssh_dir: &Path) -> Profile {
    let mut profile = Profile {
        id: p.id.clone(),
        label: p.label.clone(),
        name: p.name.clone(),
        email: p.email.clone(),
        color: p.color,
        ssh_key: PathBuf::from(to_slash(&ssh_dir.join(&p.ssh_key))),
        owners: p.owners.clone(),
        folders: p.folders.clone(),
    };
    profile.normalize();
    profile
}

/// Read, size-check, decrypt and validate an export file.
pub fn open_bundle(path: &Path, password: &str) -> Result<Bundle> {
    let bytes = match read_limited(path, MAX_BUNDLE_BYTES) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return Err(ExportError::TooLarge {
                max: MAX_BUNDLE_BYTES,
            })
        }
        Err(e) => return Err(CoreError::io(path, e).into()),
    };
    let plain = decrypt_bytes(&bytes, password)?;
    parse_bundle(&plain)
}

// ------------------------------------------------------------------ import: preview

/// Decrypt `path` and describe what importing it would do.
pub fn preview(paths: &Paths, git: &Git, path: &Path, password: &str) -> Result<ImportPreview> {
    let bundle = open_bundle(path, password)?;
    preview_bundle(paths, git, &bundle)
}

/// [`preview`] for an already opened bundle.
pub fn preview_bundle(paths: &Paths, git: &Git, bundle: &Bundle) -> Result<ImportPreview> {
    let store = Store::open_at(paths.clone())?;
    let existing = store.profiles();
    let dir = ssh_dir(git)?;
    let profiles: Vec<Profile> = bundle
        .profiles
        .iter()
        .map(|p| to_profile(p, &dir))
        .collect();
    let mut conflicts = Vec::new();
    for (bp, p) in bundle.profiles.iter().zip(&profiles) {
        conflicts.extend(conflicts_for(bundle, bp, p, existing, &dir));
    }
    Ok(ImportPreview {
        profiles,
        conflicts,
        keys: bundle.key_names(),
        include_settings: bundle.settings.is_some() && existing.profiles.is_empty(),
    })
}

/// Every clash between incoming profile `p` and the existing set / key files.
fn conflicts_for(
    bundle: &Bundle,
    bp: &BundleProfile,
    p: &Profile,
    existing: &ProfileSet,
    ssh_dir: &Path,
) -> Vec<ImportConflict> {
    let mut out = Vec::new();
    let mut push = |kind, existing_id: &str| {
        out.push(ImportConflict {
            profile_id: p.id.clone(),
            kind,
            existing_id: existing_id.to_string(),
        });
    };
    for e in &existing.profiles {
        if e.id == p.id {
            push(ConflictKind::Id, &e.id);
        }
        if e.email.trim().eq_ignore_ascii_case(p.email.trim()) {
            push(ConflictKind::Email, &e.id);
        }
        if e.color == p.color {
            push(ConflictKind::Color, &e.id);
        }
        if p.owners.iter().any(|o| e.owners.contains(o)) {
            push(ConflictKind::Owner, &e.id);
        }
    }
    if key_clashes(bundle, &bp.ssh_key, ssh_dir) {
        let target = ssh_dir.join(&bp.ssh_key);
        let user = existing
            .profiles
            .iter()
            .find(|e| same_path(&e.ssh_key, &target))
            .map(|e| e.id.as_str())
            .unwrap_or("");
        push(ConflictKind::KeyFile, user);
    }
    out
}

/// True when the bundle carries key `name` and a *different* key (or a lone
/// `.pub`) already exists at `~/.ssh/<name>`.
fn key_clashes(bundle: &Bundle, name: &str, ssh_dir: &Path) -> bool {
    let Some(key) = bundle.key(name) else {
        return false;
    };
    let target = ssh_dir.join(name);
    if !exists(&target) {
        return exists(&pub_path(&target));
    }
    match read_limited(&target, MAX_KEY_BYTES as u64) {
        Ok(Some(current)) => {
            let current = Zeroizing::new(current);
            current.as_slice() != key.private.as_slice()
        }
        _ => true,
    }
}

// ------------------------------------------------------------------ import: apply

/// Decrypt `path` and import it with `resolutions`; returns the saved profiles.
pub fn apply(
    paths: &Paths,
    git: &Git,
    path: &Path,
    password: &str,
    resolutions: &[ImportResolution],
) -> Result<ProfileSet> {
    let bundle = open_bundle(path, password)?;
    apply_bundle(paths, git, &bundle, resolutions).map(|r| r.profiles)
}

/// What happens to one key name from the bundle.
enum KeyPlan {
    /// Not written (profile skipped, key not in file, or already identical).
    None,
    /// Write to this path (doesn't exist yet).
    Write(PathBuf),
    /// Move the existing file aside, then write.
    Replace(PathBuf),
}

/// Import an opened bundle.
///
/// Every profile with a conflict that [`ConflictKind::needs_choice`] must have a
/// resolution, else [`ExportError::Unresolved`]. Profiles without conflicts are
/// imported unless resolved as `skip`. Nothing is written unless the merged set
/// validates.
pub fn apply_bundle(
    paths: &Paths,
    git: &Git,
    bundle: &Bundle,
    resolutions: &[ImportResolution],
) -> Result<ImportReport> {
    let mut store = Store::open_at(paths.clone())?;
    let existing = store.profiles().clone();
    let dir = ssh_dir(git)?;

    for r in resolutions {
        if !bundle.profiles.iter().any(|p| p.id == r.profile_id) {
            return Err(ExportError::Conflict(format!(
                "there's no profile \"{}\" in the export file",
                r.profile_id
            )));
        }
    }
    let resolution_for = |id: &str| resolutions.iter().find(|r| r.profile_id == id);

    // 1. Decide the action per incoming profile.
    let mut unresolved = Vec::new();
    let mut plan: Vec<(
        BundleProfile,
        Profile,
        Option<ImportAction>,
        Vec<ImportConflict>,
    )> = Vec::new();
    for bp in &bundle.profiles {
        let p = to_profile(bp, &dir);
        let conflicts = conflicts_for(bundle, bp, &p, &existing, &dir);
        let action = resolution_for(&bp.id).map(|r| r.action);
        if action.is_none() && conflicts.iter().any(|c| c.kind.needs_choice()) {
            unresolved.push(bp.id.clone());
        }
        plan.push((bp.clone(), p, action, conflicts));
    }
    if !unresolved.is_empty() {
        return Err(ExportError::Unresolved(unresolved));
    }

    let mut report = ImportReport {
        profiles: ProfileSet::default(),
        imported: Vec::new(),
        skipped: Vec::new(),
        replaced: Vec::new(),
        renamed: Vec::new(),
        keys_written: Vec::new(),
        keys_moved_aside: Vec::new(),
        missing_keys: Vec::new(),
        settings_applied: false,
        default_changed: false,
        notes: Vec::new(),
    };

    // 2. Existing profiles removed by `replace`.
    let mut removed: BTreeSet<String> = BTreeSet::new();
    for (_, _, action, conflicts) in &plan {
        if *action == Some(ImportAction::Replace) {
            for c in conflicts {
                if matches!(
                    c.kind,
                    ConflictKind::Id | ConflictKind::Email | ConflictKind::Owner
                ) && !c.existing_id.is_empty()
                {
                    removed.insert(c.existing_id.clone());
                }
            }
        }
    }
    report.replaced = removed.iter().cloned().collect();

    // 3. Final ids (rename) — unique across kept existing + incoming.
    let mut taken: Vec<String> = existing
        .profiles
        .iter()
        .filter(|e| !removed.contains(&e.id))
        .map(|e| e.id.clone())
        .collect();
    for (bp, _, action, _) in &plan {
        if *action != Some(ImportAction::Skip) && *action != Some(ImportAction::Rename) {
            taken.push(bp.id.clone());
        }
    }
    let mut final_ids: Vec<Option<String>> = Vec::with_capacity(plan.len());
    for (bp, _, action, _) in &plan {
        match action {
            Some(ImportAction::Skip) => final_ids.push(None),
            Some(ImportAction::Rename) => {
                let wanted = resolution_for(&bp.id)
                    .and_then(|r| r.new_id.clone())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let new_id = match wanted {
                    Some(id) => {
                        if taken.contains(&id) {
                            return Err(ExportError::Conflict(format!(
                                "the new ID \"{id}\" for \"{}\" is already used; pick another",
                                bp.id
                            )));
                        }
                        id
                    }
                    None => suggest_id(&bp.id, &taken),
                };
                taken.push(new_id.clone());
                final_ids.push(Some(new_id));
            }
            _ => final_ids.push(Some(bp.id.clone())),
        }
    }

    // 4. Key plans per key name (decided by the profiles that use it).
    let mut key_paths: Vec<(String, PathBuf)> = Vec::new(); // name -> final path
    let mut key_plans: Vec<(String, KeyPlan)> = Vec::new();
    for ((bp, _, action, _), final_id) in plan.iter().zip(&final_ids) {
        let Some(final_id) = final_id else { continue };
        if key_paths.iter().any(|(n, _)| *n == bp.ssh_key) {
            continue;
        }
        let target = dir.join(&bp.ssh_key);
        let key_plan = if bundle.key(&bp.ssh_key).is_none() {
            KeyPlan::None
        } else if !key_clashes(bundle, &bp.ssh_key, &dir) {
            if exists(&target) {
                KeyPlan::None // identical key already there
            } else {
                KeyPlan::Write(target.clone())
            }
        } else {
            match action {
                Some(ImportAction::Replace) => KeyPlan::Replace(target.clone()),
                Some(ImportAction::Rename) => {
                    let renamed = dir.join(format!("{KEY_PREFIX}{final_id}"));
                    if exists(&renamed) || exists(&pub_path(&renamed)) {
                        return Err(ExportError::Conflict(format!(
                            "a key file already exists at {}; GitHop won't overwrite it",
                            to_slash(&renamed)
                        )));
                    }
                    KeyPlan::Write(renamed)
                }
                // Unreachable after the unresolved check; be safe and don't write.
                _ => KeyPlan::None,
            }
        };
        let path = match &key_plan {
            KeyPlan::Write(p) | KeyPlan::Replace(p) => p.clone(),
            KeyPlan::None => target,
        };
        key_paths.push((bp.ssh_key.clone(), path));
        key_plans.push((bp.ssh_key.clone(), key_plan));
    }

    // 5. Build the merged set.
    let mut merged: Vec<Profile> = Vec::new();
    let mut slot_for_replacement: Vec<(String, usize)> = Vec::new(); // removed id -> index
    for e in &existing.profiles {
        if removed.contains(&e.id) {
            slot_for_replacement.push((e.id.clone(), merged.len()));
        } else {
            merged.push(e.clone());
        }
    }
    let mut insert_at: Vec<(usize, Profile)> = Vec::new();
    let mut appended: Vec<Profile> = Vec::new();
    for ((bp, p, action, conflicts), final_id) in plan.iter().zip(&final_ids) {
        let Some(final_id) = final_id else {
            report.skipped.push(bp.id.clone());
            continue;
        };
        let mut p = p.clone();
        p.id = final_id.clone();
        if let Some((_, path)) = key_paths.iter().find(|(n, _)| *n == bp.ssh_key) {
            p.ssh_key = PathBuf::from(to_slash(path));
        }
        if *action == Some(ImportAction::Rename) {
            report.renamed.push((bp.id.clone(), final_id.clone()));
        }

        // Rules that clash with profiles that stay: drop them (rename) — with
        // `replace` the clashing profiles are gone, so nothing is dropped.
        let others: Vec<&Profile> = merged
            .iter()
            .chain(insert_at.iter().map(|(_, q)| q))
            .chain(appended.iter())
            .collect();
        if let Some(o) = others
            .iter()
            .find(|o| o.email.trim().eq_ignore_ascii_case(p.email.trim()))
        {
            return Err(ExportError::Conflict(format!(
                "profile \"{}\" uses the same email as profile \"{}\"; choose skip or replace",
                bp.id, o.id
            )));
        }
        let before = p.owners.len();
        p.owners
            .retain(|own| !others.iter().any(|o| o.owners.contains(own)));
        if p.owners.len() != before {
            report.notes.push(format!(
                "Some owners of {} already belong to another profile, so they weren't imported.",
                p.label
            ));
        }
        let before = p.folders.len();
        p.folders.retain(|f| {
            let key = normalize_folder(f).map(|n| folder_key(&n));
            !others.iter().any(|o| {
                o.folders
                    .iter()
                    .any(|of| normalize_folder(of).map(|n| folder_key(&n)) == key)
            })
        });
        if p.folders.len() != before {
            report.notes.push(format!(
                "Some folders of {} already belong to another profile, so they weren't imported.",
                p.label
            ));
        }
        if others.iter().any(|o| o.color == p.color) {
            let used: Vec<ProfileColor> = others.iter().map(|o| o.color).collect();
            if let Some(free) = ProfileColor::ALL.into_iter().find(|c| !used.contains(c)) {
                report.notes.push(format!(
                    "{} now uses the {} colour (its colour was taken).",
                    p.label,
                    free.as_str()
                ));
                p.color = free;
            }
        }

        report.imported.push(p.id.clone());
        // A replacement takes the place of the first profile it replaced.
        let slot = conflicts
            .iter()
            .filter(|c| *action == Some(ImportAction::Replace) && removed.contains(&c.existing_id))
            .find_map(|c| {
                slot_for_replacement
                    .iter()
                    .position(|(id, _)| *id == c.existing_id)
                    .map(|i| slot_for_replacement.remove(i).1)
            });
        match slot {
            Some(i) => insert_at.push((i, p)),
            None => appended.push(p),
        }
    }
    // Insert replacements (highest index first keeps earlier indexes valid).
    insert_at.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (i, p) in insert_at {
        let i = i.min(merged.len());
        merged.insert(i, p);
    }
    merged.extend(appended);

    // 6. Default profile.
    let mut default = existing.default.clone().filter(|d| !removed.contains(d));
    if default.is_none() {
        // Take the replacement of the old default, else the file's default.
        let old_default = existing.default.as_deref();
        let replacement = plan
            .iter()
            .zip(&final_ids)
            .find_map(|((_, _, a, cs), fid)| {
                let replaced_default = *a == Some(ImportAction::Replace)
                    && cs
                        .iter()
                        .any(|c| Some(c.existing_id.as_str()) == old_default);
                replaced_default.then(|| fid.clone()).flatten()
            });
        default = replacement.or_else(|| {
            let file_default = bundle.default.as_deref()?;
            plan.iter()
                .zip(&final_ids)
                .find(|((bp, ..), _)| bp.id == file_default)
                .and_then(|(_, fid)| fid.clone())
        });
    }
    report.default_changed = default != existing.default && default.is_some();

    let set = ProfileSet {
        version: PROFILES_VERSION,
        default,
        profiles: merged,
    };
    let mut normalized = set.clone();
    normalized.normalize();
    normalized
        .validate()
        .map_err(ExportError::InvalidProfiles)?;

    // 7. Settings: only on a machine with no profiles yet.
    let settings_patch = bundle
        .settings
        .filter(|_| existing.profiles.is_empty() && !report.imported.is_empty());
    report.settings_applied = settings_patch.is_some();

    // 8. Keys, then profiles. Make sure nobody changed the profiles since we
    // planned, before touching any key file.
    if Store::open_at(paths.clone())?.profiles() != &existing {
        return Err(CoreError::Conflict {
            path: paths.profiles_file(),
        }
        .into());
    }
    for (name, key_plan) in &key_plans {
        let Some(key) = bundle.key(name) else {
            continue;
        };
        match key_plan {
            KeyPlan::None => {}
            KeyPlan::Write(path) => {
                write_key(path, key)?;
                report.keys_written.push(path.clone());
            }
            KeyPlan::Replace(path) => {
                report.keys_moved_aside.extend(move_aside(path)?);
                write_key(path, key)?;
                report.keys_written.push(path.clone());
            }
        }
    }
    for p in &normalized.profiles {
        if report.imported.contains(&p.id) && !exists(&p.ssh_key) {
            report.missing_keys.push((p.id.clone(), p.ssh_key.clone()));
        }
    }

    // Fresh read under the lock: the plan only applies to the profiles it was
    // made from; settings changed meanwhile by someone else are kept.
    let profiles_path = paths.profiles_file();
    store.update(|profiles, settings: &mut Settings| {
        if *profiles != existing {
            return Err(CoreError::Conflict {
                path: profiles_path.clone(),
            });
        }
        *profiles = normalized.clone();
        if let Some(s) = settings_patch {
            let enforce_changed = settings.enforce != s.enforce;
            settings.enforce = s.enforce;
            settings.logging = s.logging;
            settings.log_retention_days = s.log_retention_days;
            // The enforce mode is baked into the hook scripts (block-strict).
            if enforce_changed {
                crate::hooks::refresh_if_installed(paths, git, settings)?;
            }
        }
        Ok(())
    })?;
    report.profiles = store.profiles().clone();
    Ok(report)
}

/// Rename `<key>` and `<key>.pub` (when present) to `*.replaced-<timestamp>`.
fn move_aside(path: &Path) -> Result<Vec<PathBuf>> {
    let stamp = crate::backup::timestamp().replace('.', "-");
    let mut moved = Vec::new();
    for src in [path.to_path_buf(), pub_path(path)] {
        if !exists(&src) {
            continue;
        }
        let mut dst = src.as_os_str().to_owned();
        dst.push(format!(".replaced-{stamp}"));
        let dst = PathBuf::from(dst);
        std::fs::rename(&src, &dst).map_err(|e| CoreError::io(&src, e))?;
        moved.push(dst);
    }
    Ok(moved)
}

/// Create the key files (never overwriting) and restrict the private key to
/// the current user. If the ACL can't be set the new private key is removed.
fn write_key(path: &Path, key: &KeyData) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
    }
    write_new(path, &key.private).map_err(|e| key_write_error(path, e))?;
    if let Err(e) = ssh::restrict_key_acl(path) {
        let _ = std::fs::remove_file(path);
        return Err(e.into());
    }
    if let Some(public) = &key.public {
        let pub_file = pub_path(path);
        match write_new(&pub_file, format!("{public}\n").as_bytes()) {
            Ok(()) => {}
            // A matching `.pub` may already be there; the private key is what matters.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(CoreError::io(&pub_file, e).into()),
        }
    }
    Ok(())
}

fn key_write_error(path: &Path, e: std::io::Error) -> ExportError {
    if e.kind() == ErrorKind::AlreadyExists {
        ExportError::Key {
            path: path.to_path_buf(),
            reason: "a key file already exists here; GitHop never overwrites keys".into(),
        }
    } else {
        CoreError::io(path, e).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names() {
        for good in [
            "githop_office",
            "githop_a",
            "githop_0-1",
            &format!("githop_{}", "a".repeat(32)),
        ] {
            assert!(is_safe_key_name(good), "{good}");
        }
        for bad in [
            "",
            "githop_",
            "office",
            "../x",
            "C:\\x",
            "C:/x",
            "/etc/passwd",
            "githop_a/../../b",
            "githop_a\\b",
            "githop_..",
            "githop_a.pub",
            "CON",
            "githop_CON",
            "NUL",
            "githop_caf\u{e9}",
            "githop_a b",
            "githop_a\0",
            "id_ed25519",
            &format!("githop_{}", "a".repeat(33)),
        ] {
            assert!(!is_safe_key_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn password_rules() {
        assert!(matches!(
            check_password("short"),
            Err(ExportError::WeakPassword { min: 12 })
        ));
        assert!(check_password("twelve chars").is_ok());
        // Counted in characters, not bytes.
        assert!(check_password("ääääääääääää").is_ok());
        assert!(check_password(&"x".repeat(MAX_PASSWORD_CHARS + 1)).is_err());
    }

    #[test]
    fn suggest_id_skips_taken() {
        let taken = vec!["office".to_string(), "office-2".to_string()];
        assert_eq!(suggest_id("office", &taken), "office-3");
        let long = "a".repeat(32);
        let s = suggest_id(&long, &[]);
        assert!(s.len() <= 32 && s.ends_with("-2"), "{s}");
    }

    #[test]
    fn crypto_round_trip_and_wrong_password() {
        let ct = encrypt_bytes(b"hello", "correct horse battery", 2).unwrap();
        assert_eq!(
            decrypt_bytes(&ct, "correct horse battery")
                .unwrap()
                .as_slice(),
            b"hello"
        );
        assert!(matches!(
            decrypt_bytes(&ct, "wrong password!!"),
            Err(ExportError::WrongPassword)
        ));
        assert!(matches!(
            decrypt_bytes(b"not an age file", "whatever12345"),
            Err(ExportError::Corrupt)
        ));
    }

    #[test]
    fn conflict_kind_serialises_snake_case() {
        let c = ImportConflict {
            profile_id: "a".into(),
            kind: ConflictKind::KeyFile,
            existing_id: String::new(),
        };
        let v = toml::Value::try_from(&c).unwrap();
        assert_eq!(v["kind"].as_str(), Some("key_file"));
        let r: ImportResolution =
            toml::from_str("profile_id = \"a\"\naction = \"rename\"\n").unwrap();
        assert_eq!(r.action, ImportAction::Rename);
        assert_eq!(r.new_id, None);
    }
}
