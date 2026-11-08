//! `wipe`: remove GitHop and restore the original git setup (Plan §7.7,
//! Phase 6 §4). Shared by `githop wipe`, the tray app and the uninstaller.
//!
//! Order (each step is reported in [`WipeReport`]):
//! 1. `backup` — copy the global git config before the first write (CLAUDE.md
//!    rule 4). Only when a later step will write.
//! 2. `hooks` — [`hooks::uninstall_hooks`]: restore the previous global
//!    `core.hooksPath` (or unset it) and delete GitHop's hook files.
//! 3. `identity` — restore each global key GitHop changed
//!    (`settings.managed_global_keys`) to its value in `gitconfig.original`.
//!    Values, not the whole file: unrelated changes made since are kept.
//! 4. `unpin` — remove pins from repos GitHop pinned (`opts.unpin_repos`).
//! 5. `autostart` — delete `HKCU\…\Run\GitHop` (`reg delete`, ignored if absent).
//! 6. `data` — delete the data dir (unless `opts.keep_data`).
//! 7. `keys` — delete `~/.ssh/githop_*` (only with `opts.delete_keys`).
//!
//! Idempotent and resumable: progress is saved to `settings.toml` after each
//! step, every step checks the current state before writing, and the data dir
//! (which holds `gitconfig.original`) is only deleted when steps 1–3 worked.
//! Running it again after an interruption finishes the job.
//!
//! A broken `profiles.toml`/`settings.toml` must not stop the escape hatch:
//! settings are then read leniently, and the previous hooks path and managed
//! keys fall back to what `gitconfig.original` says.
//!
//! Registry: set `GITHOP_NO_REGISTRY=1` (test helpers do) to skip step 5.
//! `GITHOP_AUTOSTART_KEY` may point step 5 at a throwaway test key, but only
//! under `HKCU\Software\GitHopTest`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::backup;
use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::gitconfig;
use crate::hooks::{self, HooksPathChange};
use crate::paths::{to_slash, Paths};
use crate::pin;
use crate::repo::Repo;
use crate::settings::Settings;
use crate::ssh;
use crate::store::Store;

/// Global keys `githop use` writes; the fallback list when settings can't be read.
pub const MANAGED_GLOBAL_KEYS: [&str; 3] = ["user.name", "user.email", "core.sshCommand"];

/// Env var that skips the registry step (set by every test helper).
pub const NO_REGISTRY_ENV: &str = "GITHOP_NO_REGISTRY";

/// Env var that points the registry step at a test key (must start with [`TEST_KEY_PREFIX`]).
pub const AUTOSTART_KEY_ENV: &str = "GITHOP_AUTOSTART_KEY";

/// Only keys under this prefix are accepted from [`AUTOSTART_KEY_ENV`].
pub const TEST_KEY_PREFIX: &str = r"HKCU\Software\GitHopTest";

/// Where Windows keeps per-user startup programs.
pub const AUTOSTART_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// Value name of GitHop's startup entry.
pub const AUTOSTART_VALUE: &str = "GitHop";

/// Step ids, in order.
pub const STEP_BACKUP: &str = "backup";
pub const STEP_HOOKS: &str = "hooks";
pub const STEP_IDENTITY: &str = "identity";
pub const STEP_UNPIN: &str = "unpin";
pub const STEP_AUTOSTART: &str = "autostart";
pub const STEP_DATA: &str = "data";
pub const STEP_KEYS: &str = "keys";

/// What to wipe besides hooks and global settings (always restored).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WipeOptions {
    /// Remove pins from the repos GitHop pinned.
    pub unpin_repos: bool,
    /// Also delete `~/.ssh/githop_*` (separate explicit confirmation in UIs).
    pub delete_keys: bool,
    /// Keep the data dir (uninstaller "keep my settings").
    pub keep_data: bool,
}

/// One step's outcome. `detail` is a user-facing sentence (Design §9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WipeStep {
    /// Step id: `backup`, `hooks`, `identity`, `unpin`, `autostart`, `data`, `keys`.
    pub step: String,
    pub ok: bool,
    pub detail: String,
}

/// Every step, in the order run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WipeReport {
    pub steps: Vec<WipeStep>,
}

impl WipeReport {
    /// Every step worked.
    pub fn is_complete(&self) -> bool {
        self.steps.iter().all(|s| s.ok)
    }

    /// The step with this id.
    pub fn step(&self, id: &str) -> Option<&WipeStep> {
        self.steps.iter().find(|s| s.step == id)
    }

    fn push(&mut self, step: &str, ok: bool, detail: impl Into<String>) {
        self.steps.push(WipeStep {
            step: step.to_string(),
            ok,
            detail: detail.into(),
        });
    }
}

/// Read one value from a backup file (`git config --file <backup> --get <key>`).
/// `Ok(None)` when the backup doesn't set it, or the backup recorded "no file".
pub fn read_backup_value(git: &Git, file: &Path, key: &str) -> Result<Option<String>> {
    let mut marker = file.as_os_str().to_os_string();
    marker.push(".absent");
    if Path::new(&marker).is_file() {
        return Ok(None);
    }
    let file_arg = file.to_string_lossy();
    let cwd = file.parent().unwrap_or_else(|| Path::new("."));
    let args = ["config", "--file", file_arg.as_ref(), "--get", key];
    let out = git.output(cwd, &args, None)?;
    match out.status.code() {
        Some(0) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let value = text.strip_suffix('\n').unwrap_or(&text);
            let value = value.strip_suffix('\r').unwrap_or(value);
            Ok(Some(value.to_string()))
        }
        Some(1) => Ok(None),
        code => Err(CoreError::GitFailed {
            args: args.join(" "),
            code,
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        }),
    }
}

/// Settings as wipe sees them, plus whether they could be trusted.
struct Loaded {
    store: Option<Store>,
    settings: Settings,
    /// `settings.toml` parsed (or is absent): its fields are the truth.
    settings_ok: bool,
    profile_keys: Vec<PathBuf>,
}

fn load(paths: &Paths) -> Loaded {
    match Store::open_at(paths.clone()) {
        Ok(store) => Loaded {
            settings: store.settings().clone(),
            profile_keys: store
                .profiles()
                .profiles
                .iter()
                .map(|p| p.ssh_key.clone())
                .collect(),
            store: Some(store),
            settings_ok: true,
        },
        Err(_) => {
            let (settings, settings_ok) = match std::fs::read_to_string(paths.settings_file()) {
                Ok(text) => match toml::from_str::<Settings>(&text) {
                    Ok(s) => (s, true),
                    Err(_) => (Settings::default(), false),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), true),
                Err(_) => (Settings::default(), false),
            };
            Loaded {
                store: None,
                settings,
                settings_ok,
                profile_keys: Vec::new(),
            }
        }
    }
}

/// Save progress (best effort: a broken store can't be written, and the steps
/// are idempotent anyway). Copies only the fields wipe changes onto the fresh
/// settings (`Store::update_settings`), so a concurrent edit isn't lost.
fn persist(store: &mut Option<Store>, settings: &Settings) {
    if let Some(store) = store.as_mut() {
        if store.paths().root().exists() {
            let _ = store.update_settings(|s| {
                s.hooks_installed = settings.hooks_installed;
                s.hook_hashes = settings.hook_hashes.clone();
                s.previous_hooks_path = settings.previous_hooks_path.clone();
                s.managed_global_keys = settings.managed_global_keys.clone();
                s.pinned_repos = settings.pinned_repos.clone();
                Ok(())
            });
        }
    }
}

/// Wipe GitHop. See the module docs for the order and guarantees. Never stops
/// half-way on an error: each step reports its own outcome, and the data dir is
/// kept whenever restoring git config didn't fully work.
pub fn wipe(paths: &Paths, git: &Git, opts: &WipeOptions) -> Result<WipeReport> {
    let mut report = WipeReport::default();
    let original = backup::original_gitconfig(paths);
    let Loaded {
        mut store,
        mut settings,
        settings_ok,
        profile_keys,
    } = load(paths);

    // --- plan the global writes (reads only) -------------------------------
    let ours = hooks::hooks_dir_value(paths);
    let current_hooks_path = gitconfig::get_global(git, hooks::HOOKS_PATH_KEY);
    let points_to_us = matches!(&current_hooks_path, Ok(Some(v)) if hooks::same_dir(v, &ours));
    let hook_files_left = hooks::all_hook_names()
        .iter()
        .any(|n| paths.hooks_dir().join(n).is_file());
    let hooks_needed = settings.hooks_installed || points_to_us || hook_files_left;

    let mut managed: Vec<String> = settings.managed_global_keys.clone();
    if managed.is_empty() && store.is_none() && original.is_some() {
        managed = MANAGED_GLOBAL_KEYS.iter().map(|k| k.to_string()).collect();
    }
    // Keys whose current global values differ from the original (all values,
    // so multi-valued keys compare exactly). Reads only.
    let mut identity_plan: Vec<(String, Vec<String>)> = Vec::new();
    let mut identity_error: Option<String> = None;
    if let Some(orig) = &original {
        let mut marker = orig.as_os_str().to_os_string();
        marker.push(".absent");
        let absent = Path::new(&marker).is_file();
        let global = git.global_config_path();
        for key in &managed {
            let want = if absent {
                Ok(Vec::new())
            } else {
                gitconfig::get_all_file(git, orig, key)
            };
            let now = global
                .as_ref()
                .map_err(|e| CoreError::BackupFailed(e.to_string()))
                .and_then(|g| gitconfig::get_all_file(git, g, key));
            match (want, now) {
                (Ok(w), Ok(n)) if w == n => {}
                (Ok(w), Ok(_)) => identity_plan.push((key.clone(), w)),
                (Err(e), _) | (_, Err(e)) => {
                    identity_error = Some(e.to_string());
                    break;
                }
            }
        }
    }

    // --- 1. backup ------------------------------------------------------------
    let will_write = hooks_needed || !identity_plan.is_empty();
    let backup_ok = if will_write {
        match backup::backup_global_gitconfig(paths, git) {
            Ok(_) => {
                let kept = if opts.keep_data {
                    " It's kept in GitHop's backups folder."
                } else {
                    ""
                };
                report.push(
                    STEP_BACKUP,
                    true,
                    format!("Backed up your git config before changing it.{kept}"),
                );
                true
            }
            Err(e) => {
                report.push(
                    STEP_BACKUP,
                    false,
                    format!("Couldn't back up your git config first: {e}"),
                );
                false
            }
        }
    } else {
        report.push(
            STEP_BACKUP,
            true,
            "Nothing to back up: no git settings need changing.",
        );
        true
    };

    // --- 2. hooks -------------------------------------------------------------
    let mut critical_failed = !backup_ok;
    if !hooks_needed {
        report.push(STEP_HOOKS, true, "GitHop's hooks weren't installed.");
    } else if !backup_ok {
        report.push(
            STEP_HOOKS,
            false,
            "Skipped removing hooks, because the backup failed.",
        );
    } else {
        if settings.previous_hooks_path.is_empty() && (!settings_ok || !settings.hooks_installed) {
            // Settings can't tell us; the original config can.
            if let Some(prev) = original
                .as_deref()
                .and_then(|o| {
                    read_backup_value(git, o, hooks::HOOKS_PATH_KEY)
                        .ok()
                        .flatten()
                })
                .filter(|p| !p.trim().is_empty() && !hooks::same_dir(p, &ours))
            {
                settings.previous_hooks_path = prev;
            }
        }
        match hooks::uninstall_hooks(paths, git, &mut settings) {
            Ok(rep) => {
                let detail = match rep.hooks_path {
                    HooksPathChange::Restored { value } => format!(
                        "Removed GitHop's hooks. core.hooksPath is back to {value}."
                    ),
                    HooksPathChange::Unset => {
                        "Removed GitHop's hooks and unset core.hooksPath.".to_string()
                    }
                    HooksPathChange::LeftAlone { value } => format!(
                        "Removed GitHop's hooks. core.hooksPath ({value}) was changed after GitHop set it, so it was left as is."
                    ),
                };
                report.push(STEP_HOOKS, true, detail);
                persist(&mut store, &settings);
            }
            Err(e) => {
                critical_failed = true;
                report.push(
                    STEP_HOOKS,
                    false,
                    format!("Couldn't remove GitHop's hooks: {e}"),
                );
            }
        }
    }

    // --- 3. identity ------------------------------------------------------------
    if managed.is_empty() {
        report.push(
            STEP_IDENTITY,
            true,
            "GitHop didn't change your global identity.",
        );
    } else if original.is_none() {
        report.push(
            STEP_IDENTITY,
            true,
            format!(
                "Your original git config backup is missing, so {} keep their current values.",
                managed.join(", ")
            ),
        );
        settings.managed_global_keys.clear();
        persist(&mut store, &settings);
    } else if let Some(e) = identity_error {
        critical_failed = true;
        report.push(
            STEP_IDENTITY,
            false,
            format!("Couldn't read your original git settings: {e}"),
        );
    } else if identity_plan.is_empty() {
        report.push(
            STEP_IDENTITY,
            true,
            "Your global git settings were already back to the original.",
        );
        settings.managed_global_keys.clear();
        persist(&mut store, &settings);
    } else if !backup_ok {
        report.push(
            STEP_IDENTITY,
            false,
            "Skipped restoring your git settings, because the backup failed.",
        );
    } else {
        // Only the keys that differ; core restores every original value (multi-valued too).
        let keys: Vec<String> = identity_plan.iter().map(|(k, _)| k.clone()).collect();
        match backup::restore_managed_keys(paths, git, &keys) {
            Ok(done) => {
                let restored: Vec<String> = done.into_iter().map(|r| r.key).collect();
                report.push(
                    STEP_IDENTITY,
                    true,
                    format!(
                        "Your original git settings are back ({}).",
                        restored.join(", ")
                    ),
                );
                settings.managed_global_keys.clear();
                persist(&mut store, &settings);
            }
            Err(e) => {
                critical_failed = true;
                report.push(
                    STEP_IDENTITY,
                    false,
                    format!("Couldn't restore your git settings: {e}"),
                );
            }
        }
    }

    // --- 4. unpin ----------------------------------------------------------------
    let pinned = settings.pinned_repos.clone();
    if pinned.is_empty() {
        report.push(STEP_UNPIN, true, "GitHop hadn't pinned any repos.");
    } else if !opts.unpin_repos {
        report.push(
            STEP_UNPIN,
            true,
            format!(
                "{} {} their pins and GitHop's Fix backups (refs/githop/…). \
                 Remove a pin with: githop unpin",
                crate::text::plural(pinned.len(), "pinned repo", "pinned repos"),
                if pinned.len() == 1 { "keeps" } else { "keep" }
            ),
        );
    } else {
        let mut done = 0usize;
        let mut failures: Vec<String> = Vec::new();
        let mut left: Vec<String> = Vec::new();
        for root in &pinned {
            let path = Path::new(root);
            if !path.exists() {
                continue;
            }
            match Repo::discover(git, path).and_then(|r| {
                let unpinned = pin::unpin_repo(&r)?;
                // Regression D20: GitHop's own data in the repo goes too.
                remove_repo_data(git, &r);
                Ok(unpinned)
            }) {
                Ok(true) => done += 1,
                Ok(false) => {}
                Err(e) => {
                    failures.push(format!("{root} ({e})"));
                    left.push(root.clone());
                }
            }
        }
        settings.pinned_repos = left;
        persist(&mut store, &settings);
        let unpinned = format!(
            "Unpinned {} and removed GitHop's Fix backups (refs/githop/…) and push records \
             there. Repos GitHop didn't pin keep theirs.",
            crate::text::plural(done, "repo", "repos")
        );
        if failures.is_empty() {
            report.push(STEP_UNPIN, true, unpinned);
        } else {
            report.push(
                STEP_UNPIN,
                false,
                format!("{unpinned} Couldn't unpin: {}", failures.join("; ")),
            );
        }
    }

    // --- 5. autostart -----------------------------------------------------------
    let (ok, detail) = remove_autostart(git);
    report.push(STEP_AUTOSTART, ok, detail);

    // --- 6. data ----------------------------------------------------------------
    if opts.keep_data {
        persist(&mut store, &settings);
        report.push(
            STEP_DATA,
            true,
            format!("Kept GitHop data in {}.", to_slash(paths.root())),
        );
    } else if critical_failed {
        report.push(
            STEP_DATA,
            false,
            "Kept GitHop data (it holds your original git config) because a step above didn't finish. Run wipe again.",
        );
    } else {
        drop(store.take());
        match remove_githop_data(paths) {
            Ok(true) => report.push(
                STEP_DATA,
                true,
                format!("Deleted GitHop data in {}.", to_slash(paths.root())),
            ),
            Ok(false) => report.push(STEP_DATA, true, "GitHop data was already deleted."),
            Err(e) => report.push(
                STEP_DATA,
                false,
                format!(
                    "Couldn't delete {}: {e}. Close the GitHop app and run wipe again.",
                    to_slash(paths.root())
                ),
            ),
        }
    }

    // --- 7. keys ----------------------------------------------------------------
    if !opts.delete_keys {
        report.push(STEP_KEYS, true, "Kept your SSH keys.");
    } else if critical_failed {
        report.push(
            STEP_KEYS,
            false,
            "Kept your SSH keys because a step above didn't finish.",
        );
    } else {
        let (ok, detail) = delete_githop_keys(git, &profile_keys);
        report.push(STEP_KEYS, ok, detail);
    }

    Ok(report)
}

/// Files GitHop writes directly in its data dir.
const DATA_FILES: &[&str] = &[
    "profiles.toml",
    "settings.toml",
    ".lock",
    crate::check::LAST_BLOCK_FILE,
    // Include file for GitHop's core.hooksPath (written when an [include] sets one).
    "hooks.gitconfig",
];

/// GitHop's data in one repo: `refs/githop/*` (Fix backups and results) and
/// `<common dir>/githop/pushed`. Best effort, two spawns.
fn remove_repo_data(git: &Git, repo: &Repo) {
    if let Ok(refs) = git.run_text(
        repo.root(),
        &["for-each-ref", "--format=%(refname)", "refs/githop/"],
    ) {
        let script: String = refs
            .lines()
            .map(str::trim)
            .filter(|r| r.starts_with("refs/githop/"))
            .map(|r| format!("delete {r}\n"))
            .collect();
        if !script.is_empty() {
            let _ = git.run_with_stdin(
                repo.root(),
                &["update-ref", "--stdin"],
                Some(script.as_bytes()),
            );
        }
    }
    // The HTTPS→SSH undo notes (`githop.saved-remote-url-<remote>`). The remote
    // itself stays on SSH; only GitHop's note goes.
    if let Ok(keys) = git.run_text(
        repo.root(),
        &[
            "config",
            "--local",
            "--name-only",
            "--get-regexp",
            r"^githop\.saved-remote-url-",
        ],
    ) {
        for key in keys
            .lines()
            .map(str::trim)
            .filter(|k| k.starts_with(crate::remote_ssh::SAVED_URL_PREFIX))
        {
            let _ = git.run_text(repo.root(), &["config", "--local", "--unset-all", key]);
        }
    }
    let dir = repo.common_dir().join("githop");
    let _ = std::fs::remove_file(dir.join("pushed"));
    let _ = std::fs::remove_dir(&dir);
}

/// Files GitHop writes in `hooks/`: hook scripts and temp files.
fn is_githop_hook_file(name: &str) -> bool {
    hooks::all_hook_names().contains(&name) || name.starts_with(".githop-hook-")
}

/// Files GitHop writes in `backups/`.
fn is_githop_backup_file(name: &str) -> bool {
    let base = name.strip_suffix(".absent").unwrap_or(name);
    base.starts_with("gitconfig.")
        || base.starts_with("prerestore.gitconfig.")
        || crate::store::is_profile_backup_name(base)
        || is_githop_temp_name(name)
}

/// Files GitHop writes in `logs/`.
fn is_githop_log_file(name: &str) -> bool {
    crate::log::parse_file_date(name).is_some()
}

/// Delete the files `owned` picks in `dir` (not links to folders, not
/// sub-folders), then `dir` itself when that leaves it empty (security
/// regression #8: a user's own files there are kept).
fn remove_owned_files(dir: &Path, owned: fn(&str) -> bool) -> std::io::Result<bool> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut removed = false;
    for e in entries.flatten() {
        let is_file = std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.file_type().is_file());
        if !is_file || !e.file_name().to_str().is_some_and(owned) {
            continue;
        }
        let p = e.path();
        removed |= retry(|| match std::fs::remove_file(&p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            r => r.map(|()| true),
        })?;
    }
    // A junction or symlink in place of the folder: remove the link only.
    let is_link = std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink());
    if is_link {
        if std::fs::remove_dir(dir).is_ok() || std::fs::remove_file(dir).is_ok() {
            removed = true;
        }
    } else if std::fs::remove_dir(dir).is_ok() {
        removed = true;
    }
    Ok(removed)
}

/// Delete only what GitHop owns in the data dir — its files, the files it
/// writes in `hooks/`, `backups/` and `logs/` (then those folders when empty)
/// and leftover temp files from atomic writes — then the data dir itself if
/// that leaves it empty. Anything else (e.g. user files when `GITHOP_HOME`
/// points at a shared folder) is kept. `Ok(false)` = there was nothing to delete.
fn remove_githop_data(paths: &Paths) -> std::io::Result<bool> {
    let root = paths.root();
    if !root.exists() {
        return Ok(false);
    }
    let mut removed = false;
    removed |= remove_owned_files(&paths.hooks_dir(), is_githop_hook_file)?;
    removed |= remove_owned_files(&paths.backups_dir(), is_githop_backup_file)?;
    removed |= remove_owned_files(&paths.logs_dir(), is_githop_log_file)?;
    let mut names: Vec<std::ffi::OsString> = DATA_FILES.iter().map(Into::into).collect();
    if let Ok(entries) = std::fs::read_dir(root) {
        names.extend(
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.file_name())
                .filter(|n| n.to_str().is_some_and(is_githop_temp_name)),
        );
    }
    for name in names {
        let file = root.join(name);
        removed |= retry(|| match std::fs::remove_file(&file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            r => r.map(|()| true),
        })?;
    }
    // Only an empty folder is removed; "not empty" means the user keeps their files.
    if std::fs::remove_dir(root).is_ok() {
        removed = true;
    }
    Ok(removed)
}

/// Temp files left by an interrupted atomic write (`tempfile` names start with
/// `.tmp`; hook writes use `.githop-hook-`).
fn is_githop_temp_name(name: &str) -> bool {
    name.starts_with(".tmp") || name.starts_with(".githop-")
}

/// Windows: an antivirus or indexer may hold a file for a moment.
fn retry(mut op: impl FnMut() -> std::io::Result<bool>) -> std::io::Result<bool> {
    let mut last = None;
    for attempt in 0..3 {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(100 * (attempt + 1)));
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("couldn't delete the file")))
}

/// Delete `githop_*` files (private keys and `.pub`) directly inside `~/.ssh`.
fn delete_githop_keys(git: &Git, profile_keys: &[PathBuf]) -> (bool, String) {
    let Some(dir) = ssh::ssh_dir(git) else {
        return (
            true,
            "No ~/.ssh folder, so there were no keys to delete.".into(),
        );
    };
    let mut deleted = 0usize;
    let mut failures = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let is_ours = name.to_str().is_some_and(is_githop_key_name);
            let is_file = entry.file_type().is_ok_and(|t| t.is_file());
            if !is_ours || !is_file {
                continue;
            }
            let path = entry.path();
            match std::fs::remove_file(&path) {
                Ok(()) => deleted += 1,
                Err(e) => failures.push(format!("{} ({e})", to_slash(&path))),
            }
        }
    }
    let kept: Vec<String> = profile_keys
        .iter()
        .filter(|k| {
            !k.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_githop_key_name)
        })
        .map(|k| to_slash(k))
        .collect();
    let mut detail = format!(
        "Deleted {} GitHop created.",
        crate::text::plural(deleted, "SSH key file", "SSH key files")
    );
    if !kept.is_empty() {
        detail.push_str(&format!(
            " Kept {} (GitHop didn't create them).",
            kept.join(", ")
        ));
    }
    if failures.is_empty() {
        (true, detail)
    } else {
        (
            false,
            format!("{detail} Couldn't delete: {}", failures.join("; ")),
        )
    }
}

/// `githop_<id>` or `githop_<id>.pub` — the names `githop keygen` creates —
/// and the `….replaced-<time>` copies import `replace` moved aside.
fn is_githop_key_name(name: &str) -> bool {
    let name = match name.find(".replaced-") {
        Some(i) => &name[..i],
        None => name,
    };
    name.strip_prefix("githop_").is_some_and(|rest| {
        let id = rest.strip_suffix(".pub").unwrap_or(rest);
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}

/// The registry key to use, honouring the test override.
fn autostart_key(git: &Git) -> String {
    git.env_var(AUTOSTART_KEY_ENV)
        .map(|v| v.to_string_lossy().trim().to_string())
        .filter(|k| {
            k.len() > TEST_KEY_PREFIX.len()
                && k.get(..TEST_KEY_PREFIX.len())
                    .is_some_and(|p| p.eq_ignore_ascii_case(TEST_KEY_PREFIX))
                && !k.contains("..")
        })
        .unwrap_or_else(|| AUTOSTART_RUN_KEY.to_string())
}

/// Step 5. `(ok, detail)`.
fn remove_autostart(git: &Git) -> (bool, String) {
    if git
        .env_var(NO_REGISTRY_ENV)
        .is_some_and(|v| v.to_string_lossy().trim() == "1")
    {
        return (
            true,
            format!("Skipped the startup entry ({NO_REGISTRY_ENV}=1)."),
        );
    }
    if !cfg!(windows) {
        return (true, "No startup entry on this system.".into());
    }
    let key = autostart_key(git);
    let query = run_reg(&["query", &key, "/v", AUTOSTART_VALUE]);
    match query {
        Ok(0) => {}
        Ok(1) => return (true, "GitHop wasn't set to start with Windows.".into()),
        Ok(code) => {
            return (
                false,
                format!("Couldn't check the startup entry (reg exit {code})."),
            )
        }
        Err(e) => return (false, format!("Couldn't check the startup entry: {e}")),
    }
    match run_reg(&["delete", &key, "/v", AUTOSTART_VALUE, "/f"]) {
        Ok(0) => (true, "GitHop no longer starts with Windows.".into()),
        Ok(code) => (
            false,
            format!("Couldn't remove the startup entry (reg exit {code})."),
        ),
        Err(e) => (false, format!("Couldn't remove the startup entry: {e}")),
    }
}

/// Run `reg.exe` with an argument array, no window, no output. Exit code.
fn run_reg(args: &[&str]) -> std::io::Result<i32> {
    let exe = std::env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join("System32").join("reg.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("reg.exe"));
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let status = cmd.status()?;
    Ok(status.code().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_removal_keeps_files_githop_did_not_write() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::at(tmp.path().join("shared"));
        paths.ensure_dirs().unwrap();
        for f in [
            "profiles.toml",
            "settings.toml",
            ".lock",
            "last-block",
            ".tmpAb12",
        ] {
            std::fs::write(paths.root().join(f), "x").unwrap();
        }
        std::fs::write(paths.backups_dir().join("gitconfig.original"), "x").unwrap();
        std::fs::write(paths.hooks_dir().join("pre-commit"), "x").unwrap();
        std::fs::write(paths.root().join("my-notes.txt"), "keep me").unwrap();
        std::fs::create_dir(paths.root().join("my-folder")).unwrap();
        // Security regression #8: user files inside GitHop's folders stay.
        std::fs::write(paths.hooks_dir().join("my-own-hook.sh"), "x").unwrap();
        std::fs::write(paths.backups_dir().join("my-db-backup.sql"), "x").unwrap();
        std::fs::create_dir_all(paths.logs_dir()).unwrap();
        std::fs::write(paths.logs_dir().join("app.log"), "x").unwrap();
        std::fs::write(paths.logs_dir().join("githop-2026-10-31.log"), "x").unwrap();
        std::fs::write(
            paths
                .backups_dir()
                .join("profiles.20261031T142233.123Z.toml"),
            "x",
        )
        .unwrap();

        assert!(remove_githop_data(&paths).unwrap());
        let left: Vec<String> = std::fs::read_dir(paths.root())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let mut left = left;
        left.sort();
        assert_eq!(
            left,
            ["backups", "hooks", "logs", "my-folder", "my-notes.txt"]
        );
        let only = |d: &Path| -> Vec<String> {
            std::fs::read_dir(d)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(only(&paths.hooks_dir()), ["my-own-hook.sh"]);
        assert_eq!(only(&paths.backups_dir()), ["my-db-backup.sql"]);
        assert_eq!(only(&paths.logs_dir()), ["app.log"]);

        // A dir holding only GitHop's files is removed entirely.
        let own = Paths::at(tmp.path().join("own"));
        own.ensure_dirs().unwrap();
        std::fs::write(own.profiles_file(), "x").unwrap();
        assert!(remove_githop_data(&own).unwrap());
        assert!(!own.root().exists());
        assert!(!remove_githop_data(&own).unwrap());
    }

    #[test]
    fn key_names() {
        assert!(is_githop_key_name("githop_office"));
        assert!(is_githop_key_name("githop_office.pub"));
        assert!(is_githop_key_name("githop_side-2"));
        assert!(!is_githop_key_name("githop_"));
        assert!(!is_githop_key_name("githop_Office"));
        assert!(!is_githop_key_name("id_ed25519"));
        assert!(!is_githop_key_name("githop_x.bak"));
        // Security regression 9.6: keys import moved aside go too.
        assert!(is_githop_key_name(
            "githop_office.replaced-20261101T101500Z"
        ));
        assert!(is_githop_key_name(
            "githop_office.pub.replaced-20261101T101500Z"
        ));
        assert!(!is_githop_key_name("id_rsa.replaced-1"));
        assert!(!is_githop_key_name("known_hosts"));
    }

    #[test]
    fn autostart_override_only_for_test_keys() {
        let g = Git::with_exe("git");
        let real = g.clone().env(AUTOSTART_KEY_ENV, r"HKCU\Software\Evil");
        assert_eq!(autostart_key(&real), AUTOSTART_RUN_KEY);
        let prefix_only = g.clone().env(AUTOSTART_KEY_ENV, TEST_KEY_PREFIX);
        assert_eq!(autostart_key(&prefix_only), AUTOSTART_RUN_KEY);
        let test = g
            .clone()
            .env(AUTOSTART_KEY_ENV, r"HKCU\Software\GitHopTest-123");
        assert_eq!(autostart_key(&test), r"HKCU\Software\GitHopTest-123");
        let dots = g.env(AUTOSTART_KEY_ENV, r"HKCU\Software\GitHopTest\..\..\Run");
        assert_eq!(autostart_key(&dots), AUTOSTART_RUN_KEY);
    }

    #[test]
    fn no_registry_env_skips_autostart() {
        let g = Git::with_exe("git").env(NO_REGISTRY_ENV, "1");
        let (ok, detail) = remove_autostart(&g);
        assert!(ok);
        assert!(detail.contains("Skipped"));
    }

    #[test]
    fn report_helpers_and_serde_shape() {
        let mut r = WipeReport::default();
        r.push(STEP_HOOKS, true, "a");
        assert!(r.is_complete());
        r.push(STEP_DATA, false, "b");
        assert!(!r.is_complete());
        assert_eq!(r.step(STEP_DATA).map(|s| s.ok), Some(false));
        let text = toml::to_string(&r).unwrap();
        assert!(text.contains("step = \"hooks\""), "{text}");
        assert!(text.contains("ok = false"), "{text}");
        let opts: WipeOptions =
            toml::from_str("unpin_repos = true\ndelete_keys = false\nkeep_data = true\n").unwrap();
        assert!(opts.unpin_repos && opts.keep_data && !opts.delete_keys);
    }

    #[test]
    fn missing_data_dir_and_nothing_installed_is_a_clean_noop() {
        let tmp = tempfile::tempdir().unwrap();
        // No git needed: nothing installed → no global reads succeed, no writes.
        let git = Git::with_exe(tmp.path().join("no-git.exe")).env(NO_REGISTRY_ENV, "1");
        let paths = Paths::at(tmp.path().join("data"));
        let r = wipe(&paths, &git, &WipeOptions::default()).unwrap();
        assert!(r.is_complete(), "{r:?}");
        assert_eq!(
            r.steps.iter().map(|s| s.step.as_str()).collect::<Vec<_>>(),
            vec![
                STEP_BACKUP,
                STEP_HOOKS,
                STEP_IDENTITY,
                STEP_UNPIN,
                STEP_AUTOSTART,
                STEP_DATA,
                STEP_KEYS
            ]
        );
        assert!(!paths.root().exists());
    }
}
