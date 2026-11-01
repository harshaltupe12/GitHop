//! Locked, atomic, backed-up storage of `profiles.toml` and `settings.toml` (Plan §8 C17).
//!
//! CONTRACT FILE: signatures fixed by the architect; bodies filled by the
//! Phase 1 "model & storage" task.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::paths::Paths;
use crate::profile::{ProfileSet, PROFILES_VERSION};
use crate::settings::{Settings, SETTINGS_VERSION};

/// How long `save` waits for the data lock before returning `CoreError::Locked`.
pub const LOCK_TIMEOUT_MS: u64 = 2000;

/// Number of `profiles.<timestamp>.toml` versions kept in `backups/`.
pub const PROFILE_BACKUPS_KEPT: usize = 5;

/// SHA-256 of a data file's bytes; `None` = the file doesn't exist.
type Digest = Option<[u8; 32]>;

/// What this `Store` last read or wrote for each file, to detect a write by
/// another process in between (review 2026-10-31 #2, lost update).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Fingerprints {
    profiles: Digest,
    settings: Digest,
}

/// Loaded GitHop data plus where it lives.
///
/// Concurrency (Plan §8 C17): the tray app, the CLI and hooks may all hold a
/// `Store` at once. [`Store::save`] / [`Store::save_settings`] refuse with
/// [`CoreError::Conflict`] when the file on disk changed since this `Store`
/// loaded (or last wrote) it, instead of silently overwriting the other
/// writer's change. [`Store::update`] is the conflict-free way to change data:
/// it re-reads both files under the lock and applies a closure to the fresh copy.
#[derive(Debug, Clone)]
pub struct Store {
    paths: Paths,
    profiles: ProfileSet,
    settings: Settings,
    seen: Fingerprints,
}

impl Store {
    /// Open using [`Paths::resolve`] (`GITHOP_HOME` or `%APPDATA%\githop`).
    pub fn open() -> Result<Self> {
        Self::open_at(Paths::resolve()?)
    }

    /// Open at an explicit data dir. Missing files → defaults. Never creates dirs.
    /// Unknown `version` → `CoreError::UnsupportedVersion`.
    ///
    /// Reads take no lock: writes are atomic renames, so a reader always sees a
    /// whole file (keeps hook checks fast).
    pub fn open_at(paths: Paths) -> Result<Self> {
        let (profiles, profiles_fp) =
            load_versioned::<ProfileSet>(&paths.profiles_file(), PROFILES_VERSION)?;
        let (settings, settings_fp) =
            load_versioned::<Settings>(&paths.settings_file(), SETTINGS_VERSION)?;
        Ok(Self {
            paths,
            profiles: profiles.unwrap_or_default(),
            settings: settings.unwrap_or_default(),
            seen: Fingerprints {
                profiles: profiles_fp,
                settings: settings_fp,
            },
        })
    }

    /// Re-read both files from disk (e.g. after [`CoreError::Conflict`]).
    pub fn reload(&mut self) -> Result<()> {
        *self = Self::open_at(self.paths.clone())?;
        Ok(())
    }

    /// Change profiles and/or settings without losing anyone else's update.
    ///
    /// Under the exclusive lock: re-read both files fresh, run `f` on the fresh
    /// copies, then write only what `f` changed — profiles are normalised and
    /// validated first (→ `CoreError::Validation`, nothing written) and backed up
    /// like [`Store::save`]. An error from `f` aborts without writing. On success
    /// the in-memory copy is the fresh, updated data. Returns `f`'s value.
    ///
    /// `f` runs while the lock is held: keep it short and never prompt in it.
    pub fn update<T>(
        &mut self,
        f: impl FnOnce(&mut ProfileSet, &mut Settings) -> Result<T>,
    ) -> Result<T> {
        self.paths.ensure_dirs()?;
        let paths = self.paths.clone();
        let (value, profiles, settings, seen) =
            with_lock(&paths, Duration::from_millis(LOCK_TIMEOUT_MS), || {
                let profiles_path = paths.profiles_file();
                let settings_path = paths.settings_file();
                let (p0, profiles_fp) =
                    load_versioned::<ProfileSet>(&profiles_path, PROFILES_VERSION)?;
                let (s0, settings_fp) =
                    load_versioned::<Settings>(&settings_path, SETTINGS_VERSION)?;
                let p0 = p0.unwrap_or_default();
                let s0 = s0.unwrap_or_default();
                let mut p = p0.clone();
                let mut s = s0.clone();
                let value = f(&mut p, &mut s)?;
                let mut seen = Fingerprints {
                    profiles: profiles_fp,
                    settings: settings_fp,
                };

                // Prepare everything before writing anything.
                let profiles_toml = if p != p0 {
                    p.normalize();
                    p.version = PROFILES_VERSION;
                    p.validate().map_err(CoreError::Validation)?;
                    Some(to_toml(&p, &profiles_path)?)
                } else {
                    None
                };
                let settings_toml = if s != s0 {
                    s.version = SETTINGS_VERSION;
                    Some(to_toml(&s, &settings_path)?)
                } else {
                    None
                };

                if let Some(text) = &profiles_toml {
                    backup_profiles_file(&paths)?;
                    atomic_write(&profiles_path, text.as_bytes())?;
                    seen.profiles = Some(digest(text.as_bytes()));
                    prune_profile_backups(&paths.backups_dir(), PROFILE_BACKUPS_KEPT)?;
                }
                if let Some(text) = &settings_toml {
                    atomic_write(&settings_path, text.as_bytes())?;
                    seen.settings = Some(digest(text.as_bytes()));
                }
                Ok((value, p, s, seen))
            })?;
        self.profiles = profiles;
        self.settings = settings;
        self.seen = seen;
        Ok(value)
    }

    /// [`Store::update`] for settings only (profiles untouched).
    pub fn update_settings<T>(&mut self, f: impl FnOnce(&mut Settings) -> Result<T>) -> Result<T> {
        self.update(|_, s| f(s))
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn profiles(&self) -> &ProfileSet {
        &self.profiles
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Validate, then under the exclusive file lock (timeout [`LOCK_TIMEOUT_MS`]):
    /// back up the current `profiles.toml`, atomically write both files (temp +
    /// fsync + rename, retry 3× on rename failure), prune old backups.
    /// On success the in-memory copy is replaced.
    ///
    /// A normalised copy of `profiles` is what gets validated and written
    /// (owners lower-cased, folders normalised), and both files are written with
    /// the current schema version. Validation failure → `CoreError::Validation`
    /// with every issue; nothing is written and no directory is created.
    ///
    /// If either file changed on disk since this `Store` loaded or last wrote it
    /// (and doesn't already hold exactly what would be written), nothing is
    /// written and `CoreError::Conflict` names the file. Prefer [`Store::update`].
    pub fn save(&mut self, profiles: &ProfileSet, settings: &Settings) -> Result<()> {
        let mut profiles = profiles.clone();
        profiles.normalize();
        profiles.version = PROFILES_VERSION;
        profiles.validate().map_err(CoreError::Validation)?;
        let mut settings = settings.clone();
        settings.version = SETTINGS_VERSION;

        let profiles_path = self.paths.profiles_file();
        let settings_path = self.paths.settings_file();
        let profiles_toml = to_toml(&profiles, &profiles_path)?;
        let settings_toml = to_toml(&settings, &settings_path)?;
        let new_seen = Fingerprints {
            profiles: Some(digest(profiles_toml.as_bytes())),
            settings: Some(digest(settings_toml.as_bytes())),
        };

        self.paths.ensure_dirs()?;
        let seen = self.seen;
        with_lock(&self.paths, Duration::from_millis(LOCK_TIMEOUT_MS), || {
            ensure_unchanged(&profiles_path, seen.profiles, new_seen.profiles)?;
            ensure_unchanged(&settings_path, seen.settings, new_seen.settings)?;
            backup_profiles_file(&self.paths)?;
            atomic_write(&profiles_path, profiles_toml.as_bytes())?;
            atomic_write(&settings_path, settings_toml.as_bytes())?;
            prune_profile_backups(&self.paths.backups_dir(), PROFILE_BACKUPS_KEPT)
        })?;

        self.profiles = profiles;
        self.settings = settings;
        self.seen = new_seen;
        Ok(())
    }

    /// Save only settings (profiles unchanged, no profile backup).
    /// `CoreError::Conflict` when `settings.toml` changed since it was loaded —
    /// see [`Store::save`]; prefer [`Store::update_settings`].
    pub fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        let mut settings = settings.clone();
        settings.version = SETTINGS_VERSION;
        let path = self.paths.settings_file();
        let text = to_toml(&settings, &path)?;
        let new_fp = Some(digest(text.as_bytes()));

        self.paths.ensure_dirs()?;
        let seen = self.seen.settings;
        with_lock(&self.paths, Duration::from_millis(LOCK_TIMEOUT_MS), || {
            ensure_unchanged(&path, seen, new_fp)?;
            atomic_write(&path, text.as_bytes())
        })?;

        self.settings = settings;
        self.seen.settings = new_fp;
        Ok(())
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).into()
}

/// Current fingerprint of `path` (`None` = no file).
fn file_digest(path: &Path) -> Result<Digest> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(digest(&b))),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CoreError::io(path, e)),
    }
}

/// `Ok` when `path` still matches what we last saw, or already holds exactly
/// what we're about to write; otherwise [`CoreError::Conflict`]. Call under the lock.
fn ensure_unchanged(path: &Path, seen: Digest, about_to_write: Digest) -> Result<()> {
    let now = file_digest(path)?;
    if now == seen || now == about_to_write {
        Ok(())
    } else {
        Err(CoreError::Conflict {
            path: path.to_path_buf(),
        })
    }
}

/// Just the `version` key, read before the full parse so a newer file reports
/// `UnsupportedVersion` instead of a confusing parse error.
#[derive(Deserialize)]
struct VersionProbe {
    version: Option<u32>,
}

/// Read and parse a TOML file, with the fingerprint of the bytes read.
/// `(None, None)` when it doesn't exist.
fn load_versioned<T: DeserializeOwned>(path: &Path, supported: u32) -> Result<(Option<T>, Digest)> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok((None, None)),
        Err(e) => return Err(CoreError::io(path, e)),
    };
    let fp = Some(digest(&bytes));
    let text = String::from_utf8(bytes).map_err(|e| {
        CoreError::io(
            path,
            std::io::Error::new(ErrorKind::InvalidData, e.utf8_error()),
        )
    })?;
    parse_versioned(path, &text, supported).map(|v| (Some(v), fp))
}

/// Parse TOML text, checking `version` first.
fn parse_versioned<T: DeserializeOwned>(path: &Path, text: &str, supported: u32) -> Result<T> {
    let toml_err = |e: toml::de::Error| CoreError::Toml {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    let probe: VersionProbe = toml::from_str(text).map_err(toml_err)?;
    if let Some(found) = probe.version {
        if found != supported {
            return Err(CoreError::UnsupportedVersion {
                path: path.to_path_buf(),
                found,
                supported,
            });
        }
    }
    toml::from_str(text).map_err(toml_err)
}

fn to_toml<T: Serialize>(value: &T, path: &Path) -> Result<String> {
    toml::to_string_pretty(value).map_err(|e| CoreError::Toml {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// How long to sleep between lock attempts.
const LOCK_POLL: Duration = Duration::from_millis(20);

/// Run `f` while holding the exclusive lock on `paths.lock_file()`.
/// The data dir must exist. Gives up with `CoreError::Locked` after `timeout`.
pub(crate) fn with_lock<T>(
    paths: &Paths,
    timeout: Duration,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let lock_path = paths.lock_file();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| CoreError::io(&lock_path, e))?;
    let mut lock = fd_lock::RwLock::new(file);
    let deadline = Instant::now() + timeout;
    loop {
        match lock.try_write() {
            Ok(guard) => {
                let result = f();
                drop(guard);
                return result;
            }
            Err(e) if is_lock_busy(&e) => {
                if Instant::now() >= deadline {
                    return Err(CoreError::Locked);
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err(e) => return Err(CoreError::io(&lock_path, e)),
        }
    }
}

/// "Someone else holds the lock", as opposed to a real I/O failure.
fn is_lock_busy(e: &std::io::Error) -> bool {
    // Windows: ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33).
    e.kind() == ErrorKind::WouldBlock || matches!(e.raw_os_error(), Some(32) | Some(33))
}

/// How many times a failed rename is retried, and the first back-off delay.
const PERSIST_RETRIES: u32 = 3;
const PERSIST_BACKOFF: Duration = Duration::from_millis(50);

/// Write `bytes` to `path` atomically: temp file in the same directory, fsync,
/// rename over the target. The rename is retried [`PERSIST_RETRIES`] times with
/// doubling back-off (an editor or virus scanner may briefly hold the target).
/// The parent directory must exist.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut tmp = tempfile::NamedTempFile::new_in(&dir).map_err(|e| CoreError::io(&dir, e))?;
    tmp.write_all(bytes)
        .map_err(|e| CoreError::io(tmp.path(), e))?;
    tmp.as_file()
        .sync_all()
        .map_err(|e| CoreError::io(tmp.path(), e))?;

    let mut delay = PERSIST_BACKOFF;
    let mut attempt = 0;
    loop {
        match tmp.persist(path) {
            Ok(_) => return Ok(()),
            Err(e) if attempt < PERSIST_RETRIES => {
                attempt += 1;
                tmp = e.file;
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(e) => return Err(CoreError::io(path, e.error)),
        }
    }
}

/// Create `path` with `bytes`, failing if it already exists. Synced to disk.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Write `bytes` to `<dir>/<prefix><timestamp><suffix>` (never overwriting),
/// picking a fresh timestamp if the name is taken (e.g. another process in the
/// same millisecond).
pub(crate) fn write_timestamped(
    dir: &Path,
    prefix: &str,
    suffix: &str,
    bytes: &[u8],
) -> Result<PathBuf> {
    let mut last = None;
    for _ in 0..10 {
        let path = dir.join(format!("{prefix}{}{suffix}", crate::backup::timestamp()));
        match write_new(&path, bytes) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => last = Some((path, e)),
            Err(e) => return Err(CoreError::io(&path, e)),
        }
    }
    match last {
        Some((path, e)) => Err(CoreError::io(path, e)),
        None => Err(CoreError::BackupFailed(
            "couldn't pick a backup file name".into(),
        )),
    }
}

const PROFILE_BACKUP_PREFIX: &str = "profiles.";
const PROFILE_BACKUP_SUFFIX: &str = ".toml";

/// Copy the current `profiles.toml` (if any) to `backups/profiles.<timestamp>.toml`.
fn backup_profiles_file(paths: &Paths) -> Result<Option<PathBuf>> {
    let src = paths.profiles_file();
    let bytes = match std::fs::read(&src) {
        Ok(b) => b,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CoreError::io(&src, e)),
    };
    write_timestamped(
        &paths.backups_dir(),
        PROFILE_BACKUP_PREFIX,
        PROFILE_BACKUP_SUFFIX,
        &bytes,
    )
    .map(Some)
}

/// Keep only the newest `keep` `profiles.<timestamp>.toml` files in `dir`.
/// Timestamps are fixed-width, so name order = age order.
fn prune_profile_backups(dir: &Path, keep: usize) -> Result<()> {
    let mut names = list_names(dir, is_profile_backup_name)?;
    names.sort();
    let excess = names.len().saturating_sub(keep);
    for name in names.into_iter().take(excess) {
        let path = dir.join(&name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(CoreError::io(&path, e)),
        }
    }
    Ok(())
}

/// Names of regular files in `dir` accepted by `keep`. Missing dir → empty.
pub(crate) fn list_names(dir: &Path, keep: impl Fn(&str) -> bool) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CoreError::io(dir, e)),
    };
    Ok(entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| keep(n))
        .collect())
}

/// `profiles.<timestamp>.toml` (timestamp starts with a digit).
fn is_profile_backup_name(name: &str) -> bool {
    name.strip_prefix(PROFILE_BACKUP_PREFIX)
        .and_then(|r| r.strip_suffix(PROFILE_BACKUP_SUFFIX))
        .is_some_and(|ts| ts.starts_with(|c: char| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Arc, Barrier};

    use super::*;
    use crate::profile::{Profile, ProfileColor};
    use crate::settings::EnforceMode;

    fn temp() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("githop data"));
        (dir, paths)
    }

    fn sample() -> ProfileSet {
        ProfileSet {
            version: PROFILES_VERSION,
            default: Some("office".into()),
            profiles: vec![
                Profile {
                    id: "office".into(),
                    label: "Office".into(),
                    name: "Harshal (Work)".into(),
                    email: "harshal@company.com".into(),
                    color: ProfileColor::Blue,
                    ssh_key: PathBuf::from("C:/Users/My Name/.ssh/githop_office"),
                    owners: vec!["office-org".into()],
                    folders: vec!["C:/My Work/".into()],
                },
                Profile {
                    id: "personal".into(),
                    label: "Persönlich \u{2014} \u{65e5}\u{672c}".into(),
                    name: "H\u{e1}rshal \u{1f600}".into(),
                    email: "12345+harshal@users.noreply.github.com".into(),
                    color: ProfileColor::Violet,
                    ssh_key: PathBuf::from("D:/Schl\u{fc}ssel mit Leerzeichen/id_ed25519"),
                    owners: vec!["harshal-personal".into()],
                    folders: vec!["D:/\u{c9}t\u{e9} projets/".into()],
                },
            ],
        }
    }

    fn rich_settings() -> Settings {
        let mut s = Settings {
            logging: true,
            enforce: EnforceMode::BlockStrict,
            hooks_installed: true,
            previous_hooks_path: "C:/Users/My Name/hooks dir".into(),
            githop_exe: "C:/Program Files/GitHop/githop.exe".into(),
            pinned_repos: vec!["C:/My Work/repo one/".into()],
            github_host_aliases: vec!["github-office".into()],
            hinted_repos: vec!["D:/x/".into()],
            managed_global_keys: vec!["user.email".into()],
            ..Settings::default()
        };
        s.hook_hashes.insert("pre-commit".into(), "ab".repeat(32));
        s.hook_hashes.insert("pre-push".into(), "cd".repeat(32));
        s
    }

    fn profile_backups(paths: &Paths) -> Vec<String> {
        let mut v = list_names(&paths.backups_dir(), is_profile_backup_name).unwrap();
        v.sort();
        v
    }

    #[test]
    fn open_missing_gives_defaults_and_creates_nothing() {
        let (_d, paths) = temp();
        let store = Store::open_at(paths.clone()).unwrap();
        assert_eq!(store.profiles(), &ProfileSet::default());
        assert_eq!(store.settings(), &Settings::default());
        assert!(!paths.root().exists());
    }

    #[test]
    fn round_trip_unicode_and_spaces() {
        let (_d, paths) = temp();
        let mut store = Store::open_at(paths.clone()).unwrap();
        store.save(&sample(), &rich_settings()).unwrap();
        assert_eq!(store.profiles(), &sample());

        let reopened = Store::open_at(paths.clone()).unwrap();
        assert_eq!(reopened.profiles(), &sample());
        assert_eq!(reopened.settings(), &rich_settings());

        let text = std::fs::read_to_string(paths.profiles_file()).unwrap();
        assert!(text.contains("[[profile]]"), "{text}");
        assert!(text.contains("color = \"violet\""), "{text}");
    }

    #[test]
    fn save_normalises_a_copy() {
        let (_d, paths) = temp();
        let mut store = Store::open_at(paths.clone()).unwrap();
        let mut input = sample();
        input.profiles[0].owners = vec![" Office-ORG ".into()];
        input.profiles[0].folders = vec![r"C:\My Work".into()];
        store.save(&input, &Settings::default()).unwrap();
        assert_eq!(input.profiles[0].owners, vec![" Office-ORG "]); // caller's copy untouched
        assert_eq!(store.profiles(), &sample());
        assert_eq!(Store::open_at(paths).unwrap().profiles(), &sample());
    }

    #[test]
    fn unsupported_version() {
        let (_d, paths) = temp();
        std::fs::create_dir_all(paths.root()).unwrap();
        std::fs::write(
            paths.profiles_file(),
            "version = 2\n[[profile]]\nsomething_new = true\n",
        )
        .unwrap();
        match Store::open_at(paths.clone()) {
            Err(CoreError::UnsupportedVersion {
                found: 2,
                supported: 1,
                ..
            }) => {}
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }

        std::fs::remove_file(paths.profiles_file()).unwrap();
        std::fs::write(paths.settings_file(), "version = 9\n").unwrap();
        assert!(matches!(
            Store::open_at(paths),
            Err(CoreError::UnsupportedVersion { found: 9, .. })
        ));
    }

    #[test]
    fn malformed_toml_is_toml_error() {
        let (_d, paths) = temp();
        std::fs::create_dir_all(paths.root()).unwrap();
        std::fs::write(paths.profiles_file(), "version = 1\n[[profile]\n").unwrap();
        assert!(matches!(
            Store::open_at(paths.clone()),
            Err(CoreError::Toml { .. })
        ));
        // Wrong field type.
        std::fs::write(paths.profiles_file(), "version = 1\ndefault = 3\n").unwrap();
        assert!(matches!(Store::open_at(paths), Err(CoreError::Toml { .. })));
    }

    #[test]
    fn settings_file_may_omit_fields() {
        let (_d, paths) = temp();
        std::fs::create_dir_all(paths.root()).unwrap();
        std::fs::write(paths.settings_file(), "logging = true\n").unwrap();
        let s = Store::open_at(paths).unwrap();
        assert!(s.settings().logging);
        assert_eq!(s.settings().log_retention_days, 7);
    }

    #[test]
    fn invalid_save_writes_nothing() {
        let (_d, paths) = temp();
        let mut store = Store::open_at(paths.clone()).unwrap();
        let mut bad = sample();
        bad.profiles[1].owners = vec!["OFFICE-org".into()];
        bad.profiles[1].ssh_key = PathBuf::from("relative/key");
        match store.save(&bad, &Settings::default()) {
            Err(CoreError::Validation(issues)) => assert_eq!(issues.len(), 2, "{issues:?}"),
            other => panic!("expected Validation, got {other:?}"),
        }
        assert!(!paths.root().exists());
        assert_eq!(store.profiles(), &ProfileSet::default());
    }

    #[test]
    fn backups_are_pruned_to_five() {
        let (_d, paths) = temp();
        let mut store = Store::open_at(paths.clone()).unwrap();
        let mut set = sample();
        for i in 0..8 {
            set.profiles[0].name = format!("Save {i}");
            store.save(&set, &Settings::default()).unwrap();
        }
        let names = profile_backups(&paths);
        assert_eq!(names.len(), PROFILE_BACKUPS_KEPT, "{names:?}");
        // The newest backup holds the second-to-last save (the file before the last write).
        let newest =
            std::fs::read_to_string(paths.backups_dir().join(names.last().unwrap())).unwrap();
        assert!(newest.contains("Save 6"), "{newest}");
        let oldest = std::fs::read_to_string(paths.backups_dir().join(&names[0])).unwrap();
        assert!(oldest.contains("Save 2"), "{oldest}");
        // No stray temp files next to the data files.
        let stray: Vec<_> = std::fs::read_dir(paths.root())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".tmp"))
            .collect();
        assert!(stray.is_empty(), "{stray:?}");
    }

    #[test]
    fn save_settings_leaves_profiles_alone() {
        let (_d, paths) = temp();
        let mut store = Store::open_at(paths.clone()).unwrap();
        store.save(&sample(), &Settings::default()).unwrap();
        store.save_settings(&rich_settings()).unwrap();
        assert!(profile_backups(&paths).is_empty());
        let reopened = Store::open_at(paths).unwrap();
        assert_eq!(reopened.settings(), &rich_settings());
        assert_eq!(reopened.profiles(), &sample());
    }

    #[test]
    fn held_lock_gives_locked() {
        let (_d, paths) = temp();
        paths.ensure_dirs().unwrap();
        let (tx, rx) = mpsc::channel();
        let holder_paths = paths.clone();
        let holder = std::thread::spawn(move || {
            with_lock(&holder_paths, Duration::from_secs(1), || {
                tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(LOCK_TIMEOUT_MS + 700));
                Ok(())
            })
        });
        rx.recv().unwrap();
        let mut store = Store::open_at(paths.clone()).unwrap();
        let started = Instant::now();
        let res = store.save(&sample(), &Settings::default());
        assert!(matches!(res, Err(CoreError::Locked)), "{res:?}");
        assert!(started.elapsed() >= Duration::from_millis(LOCK_TIMEOUT_MS));
        assert!(!paths.profiles_file().exists());
        holder.join().unwrap().unwrap();
        // Once released, saving works.
        store.save(&sample(), &Settings::default()).unwrap();
    }

    #[test]
    fn short_lock_makes_save_wait() {
        let (_d, paths) = temp();
        paths.ensure_dirs().unwrap();
        let (tx, rx) = mpsc::channel();
        let holder_paths = paths.clone();
        let holder = std::thread::spawn(move || {
            with_lock(&holder_paths, Duration::from_secs(1), || {
                tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(300));
                Ok(())
            })
        });
        rx.recv().unwrap();
        let mut store = Store::open_at(paths.clone()).unwrap();
        let started = Instant::now();
        store.save(&sample(), &Settings::default()).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(250));
        holder.join().unwrap().unwrap();
    }

    #[test]
    fn concurrent_saves_never_corrupt() {
        let (_d, paths) = temp();
        let threads = 4;
        let barrier = Arc::new(Barrier::new(threads));
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let paths = paths.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut store = Store::open_at(paths.clone()).unwrap();
                    barrier.wait();
                    for i in 0..5 {
                        let mut set = sample();
                        set.profiles[0].name = format!("Thread {t} save {i}");
                        match store.save(&set, &Settings::default()) {
                            Ok(()) | Err(CoreError::Locked) => {}
                            // Another thread saved since our load: reload, never clobber.
                            Err(CoreError::Conflict { .. }) => store.reload().unwrap(),
                            Err(e) => panic!("unexpected error: {e:?}"),
                        }
                        // A reader never sees a half-written file.
                        if paths.profiles_file().exists() {
                            let seen = Store::open_at(paths.clone()).unwrap();
                            assert_eq!(seen.profiles().profiles.len(), 2);
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let last = Store::open_at(paths.clone()).unwrap();
        assert!(last.profiles().profiles[0].name.starts_with("Thread "));
        assert!(profile_backups(&paths).len() <= PROFILE_BACKUPS_KEPT);
    }

    /// Review #2: tray + CLI each loaded the data; the second save must not
    /// silently drop the first one's change.
    #[test]
    fn stale_save_is_a_conflict_not_a_lost_update() {
        let (_d, paths) = temp();
        let mut first = Store::open_at(paths.clone()).unwrap();
        first.save(&sample(), &Settings::default()).unwrap();

        let mut tray = Store::open_at(paths.clone()).unwrap();
        let mut cli = Store::open_at(paths.clone()).unwrap();

        let mut s = tray.settings().clone();
        s.previous_hooks_path = "C:/hooks".into();
        s.pinned_repos.push("C:/a/".into());
        tray.save_settings(&s).unwrap();

        // The CLI still holds the old settings: saving them would reset
        // previous_hooks_path / pinned_repos.
        let mut stale = cli.settings().clone();
        stale.logging = true;
        match cli.save_settings(&stale) {
            Err(CoreError::Conflict { path }) => assert_eq!(path, paths.settings_file()),
            other => panic!("expected Conflict, got {other:?}"),
        }
        match cli.save(&sample(), &stale) {
            Err(CoreError::Conflict { .. }) => {}
            other => panic!("expected Conflict, got {other:?}"),
        }
        let on_disk = Store::open_at(paths.clone()).unwrap();
        assert_eq!(on_disk.settings().previous_hooks_path, "C:/hooks");
        assert!(!on_disk.settings().logging);

        // After a reload the save goes through and keeps the other change.
        cli.reload().unwrap();
        let mut fresh = cli.settings().clone();
        fresh.logging = true;
        cli.save_settings(&fresh).unwrap();
        let on_disk = Store::open_at(paths.clone()).unwrap();
        assert_eq!(on_disk.settings().previous_hooks_path, "C:/hooks");
        assert!(on_disk.settings().logging);
        // Our own writes refresh the fingerprint: saving twice is fine.
        cli.save(&sample(), &fresh).unwrap();
        cli.save_settings(&fresh).unwrap();

        // Profiles changed elsewhere → save conflicts, save_settings doesn't care.
        let mut other = Store::open_at(paths.clone()).unwrap();
        let mut set = sample();
        set.profiles[0].name = "Changed elsewhere".into();
        other.save(&set, &fresh).unwrap();
        cli.reload().unwrap();
        let mut tray2 = cli.clone();
        let mut set2 = sample();
        set2.profiles[0].name = "Tray".into();
        tray2.save(&set2, &fresh).unwrap();
        match cli.save(&sample(), &fresh) {
            Err(CoreError::Conflict { path }) => assert_eq!(path, paths.profiles_file()),
            other => panic!("expected Conflict, got {other:?}"),
        }
        cli.save_settings(&fresh).unwrap();
    }

    #[test]
    fn identical_concurrent_write_is_not_a_conflict() {
        let (_d, paths) = temp();
        let mut a = Store::open_at(paths.clone()).unwrap();
        let mut b = Store::open_at(paths.clone()).unwrap();
        a.save(&sample(), &rich_settings()).unwrap();
        b.save(&sample(), &rich_settings()).unwrap();
    }

    #[test]
    fn update_rereads_under_the_lock() {
        let (_d, paths) = temp();
        let mut tray = Store::open_at(paths.clone()).unwrap();
        tray.save(&sample(), &Settings::default()).unwrap();
        let mut cli = Store::open_at(paths.clone()).unwrap();

        tray.update_settings(|s| {
            s.previous_hooks_path = "C:/hooks".into();
            Ok(())
        })
        .unwrap();
        // `cli` loaded before that change; update still sees it.
        let n = cli
            .update(|p, s| {
                assert_eq!(s.previous_hooks_path, "C:/hooks");
                s.pinned_repos.push("C:/a/".into());
                p.profiles[0].name = "Renamed".into();
                Ok(s.pinned_repos.len())
            })
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(cli.settings().previous_hooks_path, "C:/hooks");
        let on_disk = Store::open_at(paths.clone()).unwrap();
        assert_eq!(on_disk.settings().previous_hooks_path, "C:/hooks");
        assert_eq!(on_disk.settings().pinned_repos, vec!["C:/a/".to_string()]);
        assert_eq!(on_disk.profiles().profiles[0].name, "Renamed");
        // A plain save right after update is not a conflict.
        cli.save_settings(&on_disk.settings().clone()).unwrap();
        // The stale tray store conflicts instead of clobbering.
        assert!(matches!(
            tray.save_settings(&Settings::default()),
            Err(CoreError::Conflict { .. })
        ));
    }

    #[test]
    fn update_validates_and_skips_unchanged_files() {
        let (_d, paths) = temp();
        let mut store = Store::open_at(paths.clone()).unwrap();
        store.save(&sample(), &Settings::default()).unwrap();
        let before = profile_backups(&paths);

        // Settings-only change: no profile backup, profiles file untouched.
        let profiles_bytes = std::fs::read(paths.profiles_file()).unwrap();
        store
            .update_settings(|s| {
                s.logging = true;
                Ok(())
            })
            .unwrap();
        assert_eq!(profile_backups(&paths), before);
        assert_eq!(
            std::fs::read(paths.profiles_file()).unwrap(),
            profiles_bytes
        );

        // Invalid profiles → Validation, nothing written.
        let settings_bytes = std::fs::read(paths.settings_file()).unwrap();
        let r = store.update(|p, s| {
            p.profiles[1].owners = vec!["OFFICE-org".into()];
            s.logging = false;
            Ok(())
        });
        assert!(matches!(r, Err(CoreError::Validation(_))), "{r:?}");
        assert_eq!(
            std::fs::read(paths.profiles_file()).unwrap(),
            profiles_bytes
        );
        assert_eq!(
            std::fs::read(paths.settings_file()).unwrap(),
            settings_bytes
        );

        // Closure error aborts too.
        let r: Result<()> = store.update_settings(|s| {
            s.logging = false;
            Err(CoreError::NotARepo)
        });
        assert!(matches!(r, Err(CoreError::NotARepo)));
        assert!(Store::open_at(paths.clone()).unwrap().settings().logging);

        // Profile change: normalised, backed up.
        store
            .update(|p, _| {
                p.profiles[0].folders = vec![r"C:\My Work".into()];
                p.profiles[0].name = "New".into();
                Ok(())
            })
            .unwrap();
        assert_eq!(store.profiles().profiles[0].folders, vec!["C:/My Work/"]);
        assert_eq!(profile_backups(&paths).len(), before.len() + 1);
    }

    #[test]
    fn concurrent_updates_lose_nothing() {
        let (_d, paths) = temp();
        let threads = 4;
        let per = 5;
        let barrier = Arc::new(Barrier::new(threads));
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let paths = paths.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut store = Store::open_at(paths).unwrap();
                    barrier.wait();
                    for i in 0..per {
                        loop {
                            match store.update_settings(|s| {
                                s.pinned_repos.push(format!("C:/t{t}/r{i}/"));
                                Ok(())
                            }) {
                                Ok(()) => break,
                                Err(CoreError::Locked) => continue,
                                Err(e) => panic!("unexpected error: {e:?}"),
                            }
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let s = Store::open_at(paths).unwrap();
        assert_eq!(s.settings().pinned_repos.len(), threads * per);
    }

    #[test]
    fn atomic_write_replaces_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("file with spaces.txt");
        atomic_write(&target, b"one").unwrap();
        atomic_write(&target, "two \u{2713}".as_bytes()).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "two \u{2713}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn backup_name_filter() {
        assert!(is_profile_backup_name("profiles.20261031T142233.123Z.toml"));
        assert!(!is_profile_backup_name("profiles.toml"));
        assert!(!is_profile_backup_name("profiles.old.toml"));
        assert!(!is_profile_backup_name("gitconfig.20261031T142233.123Z"));
    }
}
