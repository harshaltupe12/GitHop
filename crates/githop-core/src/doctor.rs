//! `doctor`: diagnose the setup and suggest one-line fixes (Phase 2 §4.6,
//! Phase 3 §4.4, Plan §12). Shared by `githop doctor` and the tray app.
//!
//! Every check runs on its own: a broken store or a missing git never stops
//! the others, and [`diagnose`] never fails. Results are data
//! ([`DoctorReport`], serialised exactly as `app/IPC.md` describes); callers
//! render them.
//!
//! Side effect (by design): when `refresh_aliases` is set, SSH host aliases
//! that resolve to `github.com` are cached in `settings.github_host_aliases`
//! (Plan §7.1 — hooks are too fast-path to run `ssh -G`, so doctor/scan do it).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backup;
use crate::error::Result;
use crate::git::{find_on_path, Git};
use crate::gitconfig::{ConfigScope, ConfigSnapshot};
use crate::hooks::{self, HookHealth};
use crate::paths::{to_slash, Paths};
use crate::profile::{ProfileSet, Severity};
use crate::settings::Settings;
use crate::ssh::{self, AgentStatus};
use crate::store::Store;

/// Minimum supported git version (`--show-scope` needs 2.26).
pub const MIN_GIT: (u32, u32) = (2, 26);

/// Most SSH aliases checked with `ssh -G` per run (each is a process spawn).
const MAX_ALIAS_LOOKUPS: usize = 16;

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    /// Fine. An `ok` check with a `fix` is a note (a suggestion, nothing wrong).
    Ok,
    /// Works, but something may go wrong.
    Warning,
    /// Broken: GitHop can't protect you until it's fixed.
    Problem,
}

/// One diagnosis line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    /// Stable id, e.g. `git-version`, `hooks-path` (several checks may share one).
    pub id: String,
    pub status: CheckStatus,
    /// Short name of what was checked, e.g. `Git`, `Hooks`.
    pub title: String,
    /// One plain-language sentence.
    pub detail: String,
    /// One next step, e.g. `Run: githop hooks install`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// Everything doctor found, in a stable order (setup → profiles → hooks → ssh → env).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// Number of checks with `status`.
    pub fn count(&self, status: CheckStatus) -> usize {
        self.checks.iter().filter(|c| c.status == status).count()
    }

    /// At least one problem.
    pub fn has_problems(&self) -> bool {
        self.count(CheckStatus::Problem) > 0
    }

    /// Checks with this id.
    pub fn by_id<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a DoctorCheck> + 'a {
        self.checks.iter().filter(move |c| c.id == id)
    }

    fn add(
        &mut self,
        id: &str,
        status: CheckStatus,
        title: &str,
        detail: impl Into<String>,
        fix: Option<&str>,
    ) {
        self.checks.push(DoctorCheck {
            id: id.to_string(),
            status,
            title: title.to_string(),
            detail: detail.into(),
            fix: fix.map(str::to_string),
        });
    }

    fn ok(&mut self, id: &str, title: &str, detail: impl Into<String>) {
        self.add(id, CheckStatus::Ok, title, detail, None);
    }
}

/// What to check besides the machine-wide setup.
#[derive(Debug, Clone, Default)]
pub struct DoctorOptions {
    /// Folder to check for repo-level overrides (local `core.hooksPath`, C3).
    /// Also the working folder for git config reads. `None` = skip repo checks.
    pub cwd: Option<PathBuf>,
    /// The running `githop.exe`, to spot hooks that call a different one.
    pub current_exe: Option<PathBuf>,
    /// Look up SSH host aliases for github.com (`ssh -G`) and cache them in settings.
    pub refresh_aliases: bool,
}

/// Run every check. Never fails; problems are reported as checks.
pub fn diagnose(paths: &Paths, git: Option<&Git>, opts: &DoctorOptions) -> DoctorReport {
    let mut r = DoctorReport::default();
    let cwd = opts.cwd.clone().unwrap_or_else(std::env::temp_dir);

    if let Some(git) = git {
        check_git_version(&mut r, git, &cwd);
    } else {
        r.add(
            "git-found",
            CheckStatus::Problem,
            "Git",
            "git isn't installed or isn't on PATH.",
            Some("Install Git for Windows: https://git-scm.com/download/win"),
        );
    }
    check_ssh_tools(&mut r);

    let mut store = check_store(&mut r, paths);
    if let Some(store) = &store {
        check_profiles(&mut r, store.profiles());
        check_backups(&mut r, store);
    }

    if let Some(git) = git {
        let settings = store
            .as_ref()
            .map(|s| s.settings().clone())
            .unwrap_or_else(|| crate::check::load_settings_lenient(paths));
        let has_profiles = store
            .as_ref()
            .is_some_and(|s| !s.profiles().profiles.is_empty());
        let health = hooks::verify_hooks(paths, git, &settings);
        check_hooks(&mut r, &health, has_profiles, opts.current_exe.as_deref());
        if settings.hooks_installed {
            check_optional_stubs(&mut r, &settings);
        }
        let snapshot = ConfigSnapshot::read(git, &cwd);
        if opts.cwd.is_some() {
            check_repo_hooks_path(&mut r, snapshot.as_ref().ok(), &settings, opts, paths, git);
            if let (Some(store), Ok(snap)) = (&store, &snapshot) {
                check_pin_disagreement(&mut r, git, &cwd, snap, store);
            }
        }
        check_ssh_config(&mut r, git);
        check_known_hosts(&mut r, git);
        if opts.refresh_aliases {
            check_aliases(&mut r, git, store.as_mut());
        }
        check_env(&mut r, |k| git.env_var(k));
        check_system_config(&mut r, snapshot);
    } else {
        check_env(&mut r, |k| std::env::var_os(k));
    }
    check_agent(&mut r);
    r
}

/// In a repo: the pin disagrees with the repo's owner or folder rule
/// (regression D7). One extra spawn (`rev-parse`) to find the repo root.
fn check_pin_disagreement(
    r: &mut DoctorReport,
    git: &Git,
    cwd: &Path,
    snapshot: &ConfigSnapshot,
    store: &Store,
) {
    let Ok(repo) = crate::repo::Repo::discover(git, cwd) else {
        return;
    };
    let aliases = crate::aliases::current(store.paths(), git, store.settings());
    let res = crate::resolver::resolve(snapshot, repo.root(), store.profiles(), &aliases);
    // ADR 0006: a GitHub HTTPS remote doesn't use the profile's key.
    if let Some(p) = res
        .profile_id
        .as_deref()
        .and_then(|id| store.profiles().get(id))
    {
        let https = crate::remote_ssh::https_remotes(snapshot);
        if !https.is_empty() {
            r.add(
                "repo-https",
                CheckStatus::Warning,
                "This repo",
                format!(
                    "{} ({})",
                    crate::remote_ssh::https_problem(&p.label),
                    crate::remote_ssh::names_and(&https)
                ),
                Some("Run: githop remote ssh"),
            );
        }
    }
    for w in &res.warnings {
        if let crate::resolver::ResolutionWarning::PinOwnerDisagree {
            pinned_profile,
            rule_profile,
            rule,
            owner,
        } = w
        {
            let label = |id: &str| {
                store
                    .profiles()
                    .get(id)
                    .map(|p| p.label.clone())
                    .unwrap_or_else(|| id.to_string())
            };
            let why = match (rule, owner) {
                (crate::resolver::ResolutionSource::Owner, Some(o)) => {
                    format!("owner {o} belongs to")
                }
                _ => "its folder belongs to".to_string(),
            };
            r.add(
                "pin-owner",
                CheckStatus::Warning,
                "This repo",
                format!(
                    "This repo is pinned to {}, but {why} {}. The pin wins.",
                    label(pinned_profile),
                    label(rule_profile)
                ),
                Some(&format!("Run: githop pin {rule_profile}")),
            );
        }
    }
}

/// Pinned repos with their own `.git/hooks/<optional hook>` while GitHop skips
/// those stubs: that repo hook silently stops running (C2).
fn check_optional_stubs(r: &mut DoctorReport, settings: &crate::Settings) {
    if settings.all_hook_stubs {
        return;
    }
    let mut hits = Vec::new();
    for repo in &settings.pinned_repos {
        for hook in hooks::OPTIONAL_HOOKS {
            if Path::new(repo)
                .join(".git")
                .join("hooks")
                .join(hook)
                .is_file()
            {
                hits.push(format!("{repo} ({hook})"));
            }
        }
    }
    if !hits.is_empty() {
        r.add(
            "hooks-optional",
            CheckStatus::Warning,
            "Hooks",
            format!(
                "These repos have hooks GitHop doesn't pass through by default: {}",
                hits.join(", ")
            ),
            Some("Run: githop hooks install --all-stubs"),
        );
    }
}

/// Parse `git version 2.45.1.windows.1` → `(2, 45, 1)`.
pub fn parse_git_version(s: &str) -> Option<(u32, u32, u32)> {
    let rest = s.trim().strip_prefix("git version ")?;
    let mut parts = rest.split('.').map(|p| {
        let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u32>().ok()
    });
    let major = parts.next()??;
    let minor = parts.next()??;
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

fn check_git_version(r: &mut DoctorReport, git: &Git, cwd: &Path) {
    match git
        .run_text(cwd, &["--version"])
        .ok()
        .as_deref()
        .and_then(parse_git_version)
    {
        Some((maj, min, patch)) if (maj, min) >= MIN_GIT => r.ok(
            "git-version",
            "Git",
            format!("git {maj}.{min}.{patch} at {}", to_slash(git.exe())),
        ),
        Some((maj, min, patch)) => r.add(
            "git-version",
            CheckStatus::Problem,
            "Git",
            format!(
                "git {maj}.{min}.{patch} is too old. GitHop needs {}.{} or newer.",
                MIN_GIT.0, MIN_GIT.1
            ),
            Some("Update Git for Windows."),
        ),
        None => r.add(
            "git-version",
            CheckStatus::Warning,
            "Git",
            "Couldn't read the git version.",
            Some("Run git --version to check your install."),
        ),
    }
}

fn check_ssh_tools(r: &mut DoctorReport) {
    for (tool, id) in [("ssh", "ssh-found"), ("ssh-keygen", "ssh-keygen-found")] {
        match find_on_path(tool) {
            Some(p) => r.ok(id, "SSH", format!("{tool} found at {}", to_slash(&p))),
            None => r.add(
                id,
                CheckStatus::Problem,
                "SSH",
                format!("{tool} isn't on PATH."),
                Some("Install OpenSSH Client: Settings → System → Optional features."),
            ),
        }
    }
}

fn check_store(r: &mut DoctorReport, paths: &Paths) -> Option<Store> {
    match Store::open_at(paths.clone()) {
        Ok(s) => {
            r.ok(
                "store",
                "Profiles",
                format!("GitHop data at {}", to_slash(paths.root())),
            );
            Some(s)
        }
        Err(e) => {
            r.add(
                "store",
                CheckStatus::Problem,
                "Profiles",
                // One sentence (D16): the full parser report is in `githop doctor -v`.
                format!("GitHop data can't be read: {}", sentence(&e.to_string())),
                Some("Restore the last good copy from the backups folder in GitHop's data folder."),
            );
            None
        }
    }
}

fn check_profiles(r: &mut DoctorReport, set: &ProfileSet) {
    if set.profiles.is_empty() {
        r.add(
            "profiles",
            CheckStatus::Warning,
            "Profiles",
            "No profiles yet.",
            Some("Run: githop init"),
        );
        return;
    }
    let issues = set.issues();
    if !issues.iter().any(|i| i.severity == Severity::Error) {
        r.ok(
            "profiles",
            "Profiles",
            format!(
                "{}, all valid",
                crate::text::plural(set.profiles.len(), "profile", "profiles")
            ),
        );
    }
    for i in &issues {
        let who = i
            .profile_id
            .as_deref()
            .map(|p| format!("{p}: "))
            .unwrap_or_default();
        let status = match i.severity {
            Severity::Error => CheckStatus::Problem,
            Severity::Warning => CheckStatus::Warning,
            Severity::Info => CheckStatus::Ok,
        };
        let fix = i
            .profile_id
            .as_deref()
            .map(|p| format!("Run: githop edit {p}"));
        r.add(
            "profile-issue",
            status,
            "Profiles",
            format!("{who}{}", i.message),
            fix.as_deref(),
        );
    }
    if set.default_profile().is_none() {
        r.add(
            "default",
            CheckStatus::Ok,
            "Profiles",
            "No default profile. Repos with no owner or folder match aren't checked.",
            Some("Run: githop use <id>"),
        );
    }
    for p in &set.profiles {
        let key = &p.ssh_key;
        let shown = to_slash(key);
        if !key.is_file() {
            let fix = format!("Run: githop keygen {}", p.id);
            r.add(
                "key-file",
                CheckStatus::Warning,
                "Keys",
                format!("{}: no key file at {shown}", p.label),
                Some(&fix),
            );
            continue;
        }
        let mut pub_path = key.clone().into_os_string();
        pub_path.push(".pub");
        if Path::new(&pub_path).is_file() {
            r.ok("key-file", "Keys", format!("{}: key {shown}", p.label));
        } else {
            r.add(
                "key-pub",
                CheckStatus::Warning,
                "Keys",
                format!("{}: {shown}.pub is missing", p.label),
                Some("Recreate it: ssh-keygen -y -f <key> > <key>.pub"),
            );
        }
        // Windows ACLs: core has no locale-independent ACL *read* yet (`icacls`
        // output is localised); keygen sets user-only ACLs. Unix: mode bits.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(key) {
                if meta.permissions().mode() & 0o077 != 0 {
                    r.add(
                        "key-acl",
                        CheckStatus::Problem,
                        "Keys",
                        format!("{}: {shown} can be read by other users.", p.label),
                        Some("Run: chmod 600 <key>"),
                    );
                }
            }
        }
    }
}

fn check_backups(r: &mut DoctorReport, store: &Store) {
    let changed =
        !store.settings().managed_global_keys.is_empty() || store.settings().hooks_installed;
    match backup::original_gitconfig(store.paths()) {
        Some(p) => r.ok(
            "original",
            "Backups",
            format!("Original git config saved at {}", to_slash(&p)),
        ),
        None if changed => r.add(
            "original",
            CheckStatus::Problem,
            "Backups",
            "GitHop changed your global git config, but the original backup is missing.",
            Some("Keep a copy of ~/.gitconfig before running githop wipe."),
        ),
        None => r.ok(
            "original",
            "Backups",
            "No backup yet. GitHop hasn't changed your global git config.",
        ),
    }
}

const REINSTALL: &str = "Run: githop hooks install";

/// Hook checks from [`hooks::verify_hooks`] (Phase 3 §4.4). Pure.
pub fn hook_checks(
    health: &HookHealth,
    has_profiles: bool,
    current_exe: Option<&Path>,
) -> Vec<DoctorCheck> {
    let mut r = DoctorReport::default();
    check_hooks(&mut r, health, has_profiles, current_exe);
    r.checks
}

fn check_hooks(
    r: &mut DoctorReport,
    h: &HookHealth,
    has_profiles: bool,
    current_exe: Option<&Path>,
) {
    const T: &str = "Hooks";
    if !h.installed {
        if h.points_to_us {
            r.add(
                "hooks-installed",
                CheckStatus::Warning,
                T,
                "Global core.hooksPath points to GitHop's hooks, but GitHop's settings say they aren't installed.",
                Some(REINSTALL),
            );
        } else {
            r.add(
                "hooks-installed",
                if has_profiles {
                    CheckStatus::Problem
                } else {
                    CheckStatus::Warning
                },
                T,
                "Hooks aren't installed, so commits and pushes aren't checked.",
                Some(REINSTALL),
            );
        }
    } else {
        match (&h.config_error, h.points_to_us, &h.global_hooks_path) {
            (Some(e), _, _) => r.add(
                "hooks-path",
                CheckStatus::Warning,
                T,
                format!("Couldn't read global core.hooksPath: {e}"),
                None,
            ),
            (None, true, _) if h.overridden_by.is_empty() => r.ok(
                "hooks-path",
                T,
                format!("Global core.hooksPath points to GitHop's hooks ({})", h.hooks_dir),
            ),
            (None, true, _) => {}
            (None, false, Some(other)) => r.add(
                "hooks-path",
                CheckStatus::Problem,
                T,
                format!(
                    "Global core.hooksPath is {other}, not GitHop's hooks folder, so GitHop's hooks don't run."
                ),
                Some(REINSTALL),
            ),
            (None, false, None) => r.add(
                "hooks-path",
                CheckStatus::Problem,
                T,
                "Global core.hooksPath isn't set, so GitHop's hooks don't run.",
                Some(REINSTALL),
            ),
        }
        // Review 2026-11-01 #1: git uses the last core.hooksPath it reads, so
        // an included file read after GitHop's value turns GitHop off.
        for o in &h.overridden_by {
            let detail = match &o.condition {
                Some(cond) => format!(
                    "{} sets core.hooksPath = {} for repos matching {cond}, and git reads it after GitHop's setting, so GitHop's hooks don't run in those repos.",
                    o.file, o.value
                ),
                None => format!(
                    "{} sets core.hooksPath = {}, and git reads it after GitHop's setting, so GitHop's hooks don't run.",
                    o.file, o.value
                ),
            };
            r.add(
                "hooks-path",
                CheckStatus::Problem,
                T,
                detail,
                Some("Run: githop hooks install (GitHop's hooks then run first and still run those hooks)"),
            );
        }
        if h.overridden_by.is_empty() && !h.included_hooks_paths.is_empty() {
            let list: Vec<String> = h
                .included_hooks_paths
                .iter()
                .map(|i| format!("{} from {}", i.value, i.describe()))
                .collect();
            r.ok(
                "hooks-included",
                T,
                format!(
                    "Also runs the hooks your included git config sets: {}",
                    list.join(", ")
                ),
            );
        }
        let mut files_ok = true;
        if !h.missing.is_empty() {
            files_ok = false;
            r.add(
                "hooks-files",
                CheckStatus::Problem,
                T,
                format!(
                    "{} missing: {}",
                    files_are(h.missing.len()),
                    list(&h.missing)
                ),
                Some(REINSTALL),
            );
        }
        if !h.tampered.is_empty() {
            files_ok = false;
            r.add(
                "hooks-files",
                CheckStatus::Problem,
                T,
                format!(
                    "{} changed by hand: {}",
                    if h.tampered.len() == 1 {
                        "A hook file was"
                    } else {
                        "Hook files were"
                    },
                    list(&h.tampered)
                ),
                Some("Run: githop hooks install (it writes them again)"),
            );
        }
        if !h.stale.is_empty() {
            files_ok = false;
            r.add(
                "hooks-stale",
                CheckStatus::Warning,
                T,
                format!(
                    "{} out of date (written by an older GitHop): {}",
                    files_are(h.stale.len()),
                    list(&h.stale)
                ),
                Some(REINSTALL),
            );
        }
        if !h.unexpected.is_empty() {
            files_ok = false;
            r.add(
                "hooks-unexpected",
                CheckStatus::Warning,
                T,
                format!(
                    "GitHop didn't write {} in its hooks folder, and git may run {}: {}",
                    if h.unexpected.len() == 1 {
                        "this file"
                    } else {
                        "these files"
                    },
                    if h.unexpected.len() == 1 {
                        "it"
                    } else {
                        "them"
                    },
                    list(&h.unexpected)
                ),
                Some(&format!(
                    "Delete {} from {} unless you put {} there on purpose.",
                    if h.unexpected.len() == 1 {
                        "it"
                    } else {
                        "them"
                    },
                    h.hooks_dir,
                    if h.unexpected.len() == 1 {
                        "it"
                    } else {
                        "them"
                    }
                )),
            );
        }
        if files_ok {
            r.ok("hooks-files", T, "All hook files are present and unchanged");
        }
        let embedded = h.githop_exe.as_str();
        if h.exe_missing {
            let shown = if embedded.is_empty() {
                "(none)"
            } else {
                embedded
            };
            r.add(
                "hooks-exe",
                CheckStatus::Problem,
                T,
                format!(
                    "Hooks call githop at {shown}, which doesn't exist any more. Commits go through unchecked."
                ),
                Some(REINSTALL),
            );
        } else {
            match current_exe.map(to_slash) {
                Some(cur) if !same_file(&cur, embedded) => r.add(
                    "hooks-exe",
                    CheckStatus::Warning,
                    T,
                    format!("Hooks call githop at {embedded}, but you're running {cur}."),
                    Some("Run: githop hooks install (to use the one you're running)"),
                ),
                _ => r.ok("hooks-exe", T, format!("Hooks call githop at {embedded}")),
            }
        }
        if !h.previous_hooks_path.is_empty() {
            if h.previous_missing {
                r.add(
                    "hooks-previous",
                    CheckStatus::Warning,
                    T,
                    format!(
                        "Your previous hooks folder {} doesn't exist any more, so nothing runs from there.",
                        h.previous_hooks_path
                    ),
                    Some("Recreate it, or ignore this if you removed it on purpose."),
                );
            } else {
                r.ok(
                    "hooks-previous",
                    T,
                    format!("Also runs your previous hooks in {}", h.previous_hooks_path),
                );
            }
        }
    }
    match &h.sh_path {
        Some(sh) => r.ok("hooks-sh", T, format!("Hooks run with {}", to_slash(sh))),
        None => r.add(
            "hooks-sh",
            CheckStatus::Problem,
            T,
            "Git for Windows' sh wasn't found next to git, so hooks can't run.",
            Some("Reinstall Git for Windows."),
        ),
    }
}

/// "A hook file is" / "2 hook files are".
fn files_are(n: usize) -> String {
    if n == 1 {
        "A hook file is".to_string()
    } else {
        format!("{n} hook files are")
    }
}

fn list(items: &[String]) -> String {
    const MAX: usize = 4;
    let mut s = items
        .iter()
        .take(MAX)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > MAX {
        s.push_str(&format!(" and {} more", items.len() - MAX));
    }
    s
}

/// Same file path, the way Windows compares them (slashes, case, canonical form).
fn same_file(a: &str, b: &str) -> bool {
    let key = |s: &str| {
        let canon = dunce::canonicalize(s)
            .map(|p| to_slash(&p))
            .unwrap_or_else(|_| s.replace('\\', "/"));
        canon.to_lowercase()
    };
    key(a) == key(b)
}

/// The `core.hooksPath` git uses in the cwd repo, from any scope or include
/// (review 2026-11-01 #1). Repo-level values (husky, lefthook…, C3) get the
/// snippet; anything else that isn't GitHop's (an `includeIf` for this repo's
/// folder, system config, …) is a problem when the hooks are installed.
fn check_repo_hooks_path(
    r: &mut DoctorReport,
    snapshot: Option<&ConfigSnapshot>,
    settings: &Settings,
    opts: &DoctorOptions,
    paths: &Paths,
    git: &Git,
) {
    let Some(snap) = snapshot else { return };
    let local = snap
        .get_scoped(hooks::HOOKS_PATH_KEY, ConfigScope::Worktree)
        .or_else(|| snap.get_scoped(hooks::HOOKS_PATH_KEY, ConfigScope::Local));
    let effective = hooks::repo_hooks_override(snap, paths, git.home_dir().as_deref());
    let Some(value) = local else {
        if !settings.hooks_installed {
            return;
        }
        if let Some(o) = effective {
            let detail = match &o.value {
                Some(v) => format!(
                    "In this repo, core.hooksPath is {v} (set in {}), so GitHop's hooks don't run here.",
                    if o.origin.is_empty() { "your git config" } else { &o.origin }
                ),
                None => "In this repo, core.hooksPath isn't set, so GitHop's hooks don't run here."
                    .to_string(),
            };
            // A global include: re-installing moves GitHop's value after it.
            // Elsewhere (system config, `-c`): GitHop can't change it.
            let fix = if matches!(o.scope, ConfigScope::Global) || o.value.is_none() {
                "Run: githop hooks install (GitHop's hooks then run first and still run those hooks)"
                    .to_string()
            } else {
                snippet_fix(settings, opts)
            };
            r.add(
                "repo-hooks-path",
                CheckStatus::Problem,
                "This repo",
                detail,
                Some(&fix),
            );
        }
        return;
    };
    let fix = snippet_fix(settings, opts);
    r.add(
        "local-hooks-path",
        CheckStatus::Warning,
        "This repo",
        format!(
            "This repo sets its own core.hooksPath ({value}), so GitHop's hooks don't run here."
        ),
        Some(&fix),
    );
}

/// "Add this line at the top of that folder's pre-commit hook: …" (C3).
fn snippet_fix(settings: &Settings, opts: &DoctorOptions) -> String {
    let exe = if settings.githop_exe.is_empty() {
        opts.current_exe
            .as_deref()
            .map(to_slash)
            .unwrap_or_else(|| "githop".to_string())
    } else {
        settings.githop_exe.clone()
    };
    hooks::snippet("commit", &exe)
        .map(|s| format!("Add this line at the top of that folder's pre-commit hook: {s}"))
        .unwrap_or_else(|_| "Run: githop hooks snippet commit".to_string())
}

fn check_ssh_config(r: &mut DoctorReport, git: &Git) {
    match ssh::ssh_config_conflicts(git) {
        Ok(conflicts) if conflicts.is_empty() => r.ok(
            "ssh-config",
            "SSH",
            "~/.ssh/config doesn't force a key for GitHub",
        ),
        Ok(conflicts) => {
            for c in conflicts {
                r.add(
                    "ssh-config",
                    CheckStatus::Warning,
                    "SSH",
                    format!(
                        "~/.ssh/config line {}: Host {} sets IdentityFile {}. SSH may offer that key first and sign you in as the wrong account.",
                        c.line, c.host_pattern, c.identity_file
                    ),
                    Some("Scope it under a different Host alias, e.g. Host github-work."),
                );
            }
        }
        Err(e) => r.add(
            "ssh-config",
            CheckStatus::Warning,
            "SSH",
            format!("Couldn't read ~/.ssh/config: {e}"),
            None,
        ),
    }
}

fn check_aliases(r: &mut DoctorReport, git: &Git, store: Option<&mut Store>) {
    let found = find_github_host_aliases(git);
    if let Some(store) = store {
        if let Err(e) = save_aliases(store, git, &found) {
            r.add(
                "ssh-aliases",
                CheckStatus::Warning,
                "SSH",
                format!(
                    "Couldn't save SSH host aliases: {}",
                    sentence(&e.to_string())
                ),
                None,
            );
            return;
        }
    }
    if !found.is_empty() {
        r.ok(
            "ssh-aliases",
            "SSH",
            format!(
                "SSH host aliases for github.com: {} (owner rules work with them)",
                found.join(", ")
            ),
        );
    }
}

fn agent_check(r: &mut DoctorReport, status: AgentStatus) {
    match status {
        AgentStatus::Running => r.ok("agent", "SSH", "ssh-agent is running"),
        AgentStatus::Stopped => r.add(
            "agent",
            CheckStatus::Ok,
            "SSH",
            "ssh-agent is stopped, so keys with a passphrase ask for it on every push.",
            Some("In the app: edit the profile → Remember my passphrase → Turn on. Or in an admin PowerShell: Set-Service ssh-agent -StartupType Automatic; Start-Service ssh-agent"),
        ),
        AgentStatus::Unknown => {
            if cfg!(windows) {
                r.ok("agent", "SSH", "Couldn't check the ssh-agent service.");
            }
        }
    }
}

/// GitHub's host keys in `~/.ssh/known_hosts` (ADR 0006). Without them the
/// first SSH connect asks yes/no, which VS Code's Git can't answer. Read only.
fn check_known_hosts(r: &mut DoctorReport, git: &Git) {
    let text = ssh::known_hosts_path(git)
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    if ssh::known_hosts_trusts_github(&text) {
        r.ok("known-hosts", "SSH", "GitHub's host keys are trusted");
    } else {
        r.add(
            "known-hosts",
            CheckStatus::Warning,
            "SSH",
            "~/.ssh/known_hosts doesn't have GitHub's current host keys, so the first SSH push asks \"Are you sure you want to continue connecting?\" (VS Code can't answer it).",
            Some("Run: githop remote ssh in a repo, or githop test <profile> (both add GitHub's published keys)"),
        );
    }
}

fn check_agent(r: &mut DoctorReport) {
    agent_check(r, ssh::agent_status());
}

/// Env vars that override what GitHop sets (Plan §8 C9).
const OVERRIDE_ENV: &[(&str, &str)] = &[
    (
        "GIT_SSH_COMMAND",
        "overrides the SSH key GitHop picks for each repo.",
    ),
    ("GIT_SSH", "overrides the SSH program GitHop sets up."),
    (
        "GIT_AUTHOR_EMAIL",
        "overrides the commit email for every repo.",
    ),
    (
        "GIT_COMMITTER_EMAIL",
        "overrides the committer email for every repo.",
    ),
    (
        "GIT_AUTHOR_NAME",
        "overrides the commit name for every repo.",
    ),
    (
        "GIT_COMMITTER_NAME",
        "overrides the committer name for every repo.",
    ),
    (
        crate::check::DISABLE_ENV,
        "turns GitHop's checks off for every commit and push.",
    ),
];

fn check_env(r: &mut DoctorReport, var: impl Fn(&str) -> Option<std::ffi::OsString>) {
    let mut any = false;
    for (name, why) in OVERRIDE_ENV {
        let set = var(name).is_some_and(|v| {
            !v.is_empty()
                && (*name != crate::check::DISABLE_ENV || v.to_string_lossy().trim() == "1")
        });
        if set {
            any = true;
            r.add(
                "env",
                CheckStatus::Warning,
                "Environment",
                format!("{name} is set. It {why}"),
                Some("Remove it from your environment variables."),
            );
        }
    }
    if !any {
        r.ok(
            "env",
            "Environment",
            "No git identity environment variables set",
        );
    }
}

/// System-level keys that override or bypass GitHop (Plan §8 C18).
const SYSTEM_KEYS: &[&str] = &[
    "user.name",
    "user.email",
    "core.sshCommand",
    "core.hooksPath",
];

fn check_system_config(r: &mut DoctorReport, snapshot: Result<ConfigSnapshot>) {
    let snap = match snapshot {
        Ok(s) => s,
        Err(e) => {
            r.add(
                "system-config",
                CheckStatus::Warning,
                "Git",
                format!("Couldn't read git config: {e}"),
                None,
            );
            return;
        }
    };
    let mut any = false;
    for key in SYSTEM_KEYS {
        if let Some(value) = snap.get_scoped(key, ConfigScope::System) {
            any = true;
            let origin = snap
                .entries()
                .iter()
                .rev()
                .find(|e| e.scope == ConfigScope::System && e.key.eq_ignore_ascii_case(key))
                .map(|e| e.origin.trim_start_matches("file:").to_string())
                .unwrap_or_default();
            r.add(
                "system-config",
                CheckStatus::Warning,
                "Git",
                format!("System git config sets {key} = {value} ({origin}). It may be managed by your IT team; GitHop never changes it."),
                Some("Profiles and pins still override it. Ask IT if commits look wrong."),
            );
        }
    }
    if !any {
        r.ok(
            "system-config",
            "Git",
            "System git config doesn't set identity or hooks",
        );
    }
}

// ---------------------------------------------------------------------------
// SSH host aliases for github.com (cached in settings for the resolver).

/// Concrete `Host` aliases in an ssh config whose block sets `HostName`
/// to `github.com` (or to a `%` token that `ssh -G` must expand). Pure.
///
/// Returns `(alias, static_github)`: `static_github` is true when the block
/// literally says `HostName github.com`. Wildcard/negated patterns and
/// `github.com` itself are skipped; `Match` blocks end a `Host` block.
pub fn alias_candidates(config: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut seen_hostname: Vec<String> = Vec::new();
    for raw in config.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, rest) = match line.find(|c: char| c.is_whitespace() || c == '=') {
            Some(i) => (
                &line[..i],
                line[i..].trim_start_matches(|c: char| c.is_whitespace() || c == '='),
            ),
            None => (line, ""),
        };
        let rest = rest.trim();
        if key.eq_ignore_ascii_case("host") {
            current = rest
                .split_whitespace()
                .map(|p| p.trim_matches('"').to_ascii_lowercase())
                .filter(|p| !p.is_empty() && !p.contains(['*', '?', '!']) && p != "github.com")
                .collect();
        } else if key.eq_ignore_ascii_case("match") {
            current.clear();
        } else if key.eq_ignore_ascii_case("hostname") {
            let host = rest.trim_matches('"').to_ascii_lowercase();
            for alias in &current {
                // First HostName for an alias wins (ssh's own rule).
                if seen_hostname.contains(alias) {
                    continue;
                }
                seen_hostname.push(alias.clone());
                if host == "github.com" || host.contains('%') {
                    out.push((alias.clone(), host == "github.com"));
                }
            }
        }
    }
    out
}

/// SSH host aliases that resolve to `github.com`, from `~/.ssh/config`
/// (lower-case, sorted). Each candidate is confirmed with `ssh -G` (follows
/// `Include`, `%h` tokens); when ssh can't answer, the literal `HostName` is
/// trusted. Spawns up to [`MAX_ALIAS_LOOKUPS`] processes — never call from hooks.
pub fn find_github_host_aliases(git: &Git) -> Vec<String> {
    let Some(dir) = ssh::ssh_dir(git) else {
        return Vec::new();
    };
    let config = dir.join("config");
    let Ok(bytes) = std::fs::read(&config) else {
        return Vec::new();
    };
    let mut out: Vec<String> = alias_candidates(&String::from_utf8_lossy(&bytes))
        .into_iter()
        .take(MAX_ALIAS_LOOKUPS)
        .filter_map(|(alias, literal)| match ssh_g_hostname(&config, &alias) {
            Some(host) => (host == "github.com").then_some(alias),
            None => literal.then_some(alias),
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `ssh -F <config> -G -- <alias>` → the `hostname` value (lower-cased).
///
/// Like `ssh::resolve_alias_hostname`, but pinned to the config file GitHop
/// parsed: Windows OpenSSH finds `~` from the user profile, not `HOME`, so
/// without `-F` it could read a different file than git's ssh does.
fn ssh_g_hostname(config: &Path, alias: &str) -> Option<String> {
    use std::process::{Command, Stdio};
    let alias = alias.trim();
    // Never let an alias be read as an option.
    if alias.is_empty() || alias.starts_with('-') || alias.chars().any(char::is_control) {
        return None;
    }
    let exe = find_on_path("ssh").or_else(|| {
        let root = std::env::var_os("SystemRoot")?;
        let p = PathBuf::from(root)
            .join("System32")
            .join("OpenSSH")
            .join("ssh.exe");
        p.is_file().then_some(p)
    })?;
    let mut cmd = Command::new(exe);
    cmd.arg("-F")
        .arg(config)
        .arg("-G")
        .arg("--")
        .arg(alias)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| {
            let (k, v) = line.trim().split_once(char::is_whitespace)?;
            let v = v.trim();
            (k.eq_ignore_ascii_case("hostname") && !v.is_empty()).then(|| v.to_ascii_lowercase())
        })
}

fn save_aliases(store: &mut Store, git: &Git, found: &[String]) -> Result<bool> {
    let mut current = store.settings().github_host_aliases.clone();
    current.sort();
    let stamp = crate::aliases::stamp(crate::aliases::ssh_config_path(git).as_deref());
    if current == found && store.settings().ssh_config_stamp == stamp {
        return Ok(false);
    }
    store.update_settings(|s| {
        s.github_host_aliases = found.to_vec();
        s.ssh_config_stamp = stamp;
        Ok(())
    })?;
    Ok(true)
}

/// Look up github.com aliases and cache them in `settings.github_host_aliases`
/// (saved only when they changed). Returns the aliases. For `scan` and doctor.
pub fn refresh_github_host_aliases(store: &mut Store, git: &Git) -> Result<Vec<String>> {
    let found = find_github_host_aliases(git);
    save_aliases(store, git, &found)?;
    Ok(found)
}

fn sentence(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_version_parsing() {
        assert_eq!(
            parse_git_version("git version 2.45.1.windows.1"),
            Some((2, 45, 1))
        );
        assert_eq!(parse_git_version("git version 2.26.0\n"), Some((2, 26, 0)));
        assert_eq!(parse_git_version("git version 2.39"), Some((2, 39, 0)));
        assert_eq!(
            parse_git_version("git version 2.50.0-rc1"),
            Some((2, 50, 0))
        );
        assert_eq!(parse_git_version("nonsense"), None);
        assert!((2, 25) < MIN_GIT);
        assert!((2, 26) >= MIN_GIT);
    }

    #[test]
    fn report_serialises_like_ipc_contract() {
        let mut r = DoctorReport::default();
        r.add("x", CheckStatus::Warning, "T", "d", Some("f"));
        r.ok("y", "T", "d");
        // Core has no serde_json; TOML shows the same serde field names/values.
        let text = toml::to_string(&r).unwrap();
        assert!(text.contains("status = \"warning\""), "{text}");
        assert!(text.contains("status = \"ok\""), "{text}");
        assert_eq!(text.matches("fix = ").count(), 1, "None fix is skipped");
        for key in ["id = ", "title = ", "detail = "] {
            assert_eq!(text.matches(key).count(), 2, "{key}");
        }
        let back: DoctorReport = toml::from_str(&text).unwrap();
        assert_eq!(back, r);
        assert_eq!(r.count(CheckStatus::Warning), 1);
        assert!(!r.has_problems());
    }

    #[test]
    fn alias_candidates_cases() {
        let cfg =
            "# c\nHost github-work gh-personal\n  HostName github.com\n  IdentityFile ~/.ssh/k\n\
                   Host *.corp\n  HostName github.com\n\
                   Host other\n  HostName gitlab.com\n\
                   Host=tok\n  HostName=%h.github.com\n\
                   Host github.com\n  HostName github.com\n\
                   Match host x\n  HostName github.com\n\
                   Host Dup\n  HostName example.com\n  HostName github.com\n";
        assert_eq!(
            alias_candidates(cfg),
            vec![
                ("github-work".to_string(), true),
                ("gh-personal".to_string(), true),
                ("tok".to_string(), false),
            ]
        );
        assert!(alias_candidates("").is_empty());
    }

    fn healthy() -> HookHealth {
        HookHealth {
            installed: true,
            hooks_dir: "C:/d/hooks".into(),
            global_hooks_path: Some("C:/d/hooks".into()),
            points_to_us: true,
            githop_exe: "C:/bin/githop.exe".into(),
            sh_path: Some(PathBuf::from("C:/Git/bin/sh.exe")),
            ..HookHealth::default()
        }
    }

    fn statuses(checks: &[DoctorCheck]) -> Vec<(String, CheckStatus)> {
        checks.iter().map(|c| (c.id.clone(), c.status)).collect()
    }

    #[test]
    fn healthy_hooks_are_all_ok() {
        let c = hook_checks(&healthy(), true, None);
        assert!(c.iter().all(|c| c.status == CheckStatus::Ok), "{c:?}");
        assert!(c.iter().any(|c| c.id == "hooks-path"));
        assert!(c.iter().any(|c| c.id == "hooks-sh"));
    }

    #[test]
    fn hook_problems_have_one_line_fixes() {
        let mut h = healthy();
        h.points_to_us = false;
        h.global_hooks_path = Some("C:/elsewhere".into());
        h.missing = vec!["pre-push".into()];
        h.tampered = vec!["pre-commit".into()];
        h.stale = vec!["commit-msg".into()];
        h.exe_missing = true;
        h.previous_hooks_path = "C:/old".into();
        h.previous_missing = true;
        h.sh_path = None;
        let c = hook_checks(&h, true, None);
        let s = statuses(&c);
        assert!(s.contains(&("hooks-path".into(), CheckStatus::Problem)));
        assert!(s.contains(&("hooks-files".into(), CheckStatus::Problem)));
        assert!(s.contains(&("hooks-stale".into(), CheckStatus::Warning)));
        assert!(s.contains(&("hooks-exe".into(), CheckStatus::Problem)));
        assert!(s.contains(&("hooks-previous".into(), CheckStatus::Warning)));
        assert!(s.contains(&("hooks-sh".into(), CheckStatus::Problem)));
        for check in c.iter().filter(|c| c.status != CheckStatus::Ok) {
            assert!(check.fix.is_some(), "{check:?} has no fix");
            assert!(!check.detail.contains('\n'));
        }
        assert!(c
            .iter()
            .any(|c| c.detail.contains("C:/elsewhere") && c.id == "hooks-path"));
    }

    #[test]
    fn not_installed_is_problem_only_with_profiles() {
        let h = HookHealth {
            sh_path: Some(PathBuf::from("sh")),
            ..HookHealth::default()
        };
        let with = hook_checks(&h, true, None);
        assert_eq!(with[0].status, CheckStatus::Problem);
        assert_eq!(with[0].fix.as_deref(), Some(REINSTALL));
        let without = hook_checks(&h, false, None);
        assert_eq!(without[0].status, CheckStatus::Warning);
    }

    #[test]
    fn different_running_exe_is_a_warning() {
        let c = hook_checks(&healthy(), true, Some(Path::new("D:/new/githop.exe")));
        let exe = c.iter().find(|c| c.id == "hooks-exe").unwrap();
        assert_eq!(exe.status, CheckStatus::Warning);
        let c = hook_checks(&healthy(), true, Some(Path::new(r"c:\BIN\githop.exe")));
        let exe = c.iter().find(|c| c.id == "hooks-exe").unwrap();
        assert_eq!(exe.status, CheckStatus::Ok);
    }

    #[test]
    fn env_check_flags_overrides_and_disable() {
        let mut r = DoctorReport::default();
        check_env(&mut r, |k| match k {
            "GIT_AUTHOR_EMAIL" => Some("x@y".into()),
            "GITHOP_DISABLE" => Some("1".into()),
            _ => None,
        });
        assert_eq!(r.count(CheckStatus::Warning), 2);
        let mut r = DoctorReport::default();
        check_env(&mut r, |k| (k == "GITHOP_DISABLE").then(|| "0".into()));
        assert_eq!(r.count(CheckStatus::Warning), 0);
    }

    #[test]
    fn agent_stopped_is_a_note() {
        let mut r = DoctorReport::default();
        agent_check(&mut r, AgentStatus::Stopped);
        assert_eq!(r.checks[0].status, CheckStatus::Ok);
        assert!(r.checks[0].fix.is_some());
    }
}
