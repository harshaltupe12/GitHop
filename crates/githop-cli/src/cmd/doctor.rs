//! `githop doctor [--json]` — check the setup and print one-line fixes (Phase 2 §4.6).
//!
//! Every check runs independently: a failing store or missing git never stops the
//! others. Phase 3 adds hook checks.

use std::path::Path;

use anyhow::Result;
use githop_core::git::find_on_path;
use githop_core::paths::to_slash;
use githop_core::ssh::{self, AgentStatus};
use githop_core::{backup, ConfigScope, ConfigSnapshot, Git, Severity, Store};
use serde::Serialize;
use serde_json::json;

use super::{key_display, Ctx};
use crate::exit::{sentence, Exit};

/// Minimum supported git version (`--show-scope` needs 2.26).
pub const MIN_GIT: (u32, u32) = (2, 26);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Info,
    Warn,
    Problem,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub group: &'static str,
    pub id: &'static str,
    pub status: CheckStatus,
    pub message: String,
    pub fix: Option<String>,
}

#[derive(Default)]
struct Report {
    checks: Vec<Check>,
}

impl Report {
    fn add(
        &mut self,
        group: &'static str,
        id: &'static str,
        status: CheckStatus,
        message: impl Into<String>,
        fix: Option<&str>,
    ) {
        self.checks.push(Check {
            group,
            id,
            status,
            message: message.into(),
            fix: fix.map(str::to_string),
        });
    }

    fn count(&self, s: CheckStatus) -> usize {
        self.checks.iter().filter(|c| c.status == s).count()
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

pub fn run(ctx: &Ctx) -> Result<Exit> {
    let mut r = Report::default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());

    let git = check_git(ctx, &mut r, &cwd);
    check_ssh_tools(&mut r);
    let store = check_store(&mut r);
    if let Some(store) = &store {
        check_profiles(&mut r, store);
        check_backups(&mut r, store);
    }
    if let Some(git) = git {
        check_ssh_config(&mut r, git);
        check_env(&mut r, git);
        check_system_config(&mut r, git, &cwd);
    }
    check_agent(&mut r);

    let problems = r.count(CheckStatus::Problem);
    let code = if problems > 0 {
        Exit::Blocked
    } else {
        Exit::Ok
    };
    if ctx.ui.json {
        ctx.ui.json_out(&json!({
            "checks": r.checks,
            "summary": {
                "ok": r.count(CheckStatus::Ok),
                "info": r.count(CheckStatus::Info),
                "warn": r.count(CheckStatus::Warn),
                "problem": problems,
            },
        }))?;
        return Ok(code);
    }
    print_report(ctx, &r);
    Ok(code)
}

fn print_report(ctx: &Ctx, r: &Report) {
    let ui = &ctx.ui;
    let groups: [(CheckStatus, &str); 4] = [
        (CheckStatus::Ok, "Passing"),
        (CheckStatus::Info, "Notes"),
        (CheckStatus::Warn, "Warnings"),
        (CheckStatus::Problem, "Problems"),
    ];
    let mut first = true;
    for (status, title) in groups {
        let items: Vec<&Check> = r.checks.iter().filter(|c| c.status == status).collect();
        if items.is_empty() || (status == CheckStatus::Ok && ui.quiet) {
            continue;
        }
        if !first {
            ui.line("");
        }
        first = false;
        if ui.quiet {
            println!("{title}");
        } else {
            ui.line(title);
        }
        for c in items {
            let msg = format!("{}: {}", c.group, c.message);
            match status {
                CheckStatus::Ok => ui.ok(&msg),
                CheckStatus::Info => {
                    ui.line(&format!("{} {msg}", if ui.ascii { "-" } else { "·" }))
                }
                CheckStatus::Warn => ui.warn(&msg),
                CheckStatus::Problem => ui.blocked(&msg),
            }
            if let Some(fix) = &c.fix {
                if status != CheckStatus::Ok {
                    ui.hint(&format!("  Fix: {fix}"));
                }
            }
        }
    }
    ui.line("");
    let problems = r.count(CheckStatus::Problem);
    let warnings = r.count(CheckStatus::Warn);
    if problems == 0 && warnings == 0 {
        ui.ok("Everything looks good.");
    } else {
        let text = format!("{problems} problem(s), {warnings} warning(s).");
        if problems > 0 {
            ui.blocked(&text);
        } else {
            ui.warn(&text);
        }
    }
}

fn check_git<'a>(ctx: &'a Ctx, r: &mut Report, cwd: &Path) -> Option<&'a Git> {
    let git = match ctx.git() {
        Ok(g) => g,
        Err(_) => {
            r.add(
                "Git",
                "git-found",
                CheckStatus::Problem,
                "git isn't installed or isn't on PATH.",
                Some("Install Git for Windows: https://git-scm.com/download/win"),
            );
            return None;
        }
    };
    match git
        .run_text(cwd, &["--version"])
        .ok()
        .as_deref()
        .and_then(parse_git_version)
    {
        Some((maj, min, patch)) if (maj, min) >= MIN_GIT => {
            r.add(
                "Git",
                "git-version",
                CheckStatus::Ok,
                format!("git {maj}.{min}.{patch} at {}", to_slash(git.exe())),
                None,
            );
        }
        Some((maj, min, patch)) => {
            r.add(
                "Git",
                "git-version",
                CheckStatus::Problem,
                format!(
                    "git {maj}.{min}.{patch} is too old. GitHop needs {}.{} or newer.",
                    MIN_GIT.0, MIN_GIT.1
                ),
                Some("Update Git for Windows."),
            );
        }
        None => {
            r.add(
                "Git",
                "git-version",
                CheckStatus::Warn,
                "Couldn't read the git version.",
                Some("Run git --version to check your install."),
            );
        }
    }
    Some(git)
}

fn check_ssh_tools(r: &mut Report) {
    for (tool, id) in [("ssh", "ssh-found"), ("ssh-keygen", "ssh-keygen-found")] {
        if let Some(p) = find_on_path(tool) {
            r.add(
                "SSH",
                id,
                CheckStatus::Ok,
                format!("{tool} found at {}", to_slash(&p)),
                None,
            );
        } else {
            r.add(
                "SSH",
                id,
                CheckStatus::Problem,
                format!("{tool} isn't on PATH."),
                Some("Install OpenSSH Client: Settings → System → Optional features."),
            );
        }
    }
}

fn check_store(r: &mut Report) -> Option<Store> {
    match Store::open() {
        Ok(s) => {
            r.add(
                "Profiles",
                "store",
                CheckStatus::Ok,
                format!("GitHop data at {}", to_slash(s.paths().root())),
                None,
            );
            Some(s)
        }
        Err(e) => {
            r.add(
                "Profiles",
                "store",
                CheckStatus::Problem,
                format!("GitHop data can't be read: {}", sentence(&e.to_string())),
                Some("Restore the last good copy from the backups folder in GitHop's data folder."),
            );
            None
        }
    }
}

fn check_profiles(r: &mut Report, store: &Store) {
    let set = store.profiles();
    if set.profiles.is_empty() {
        r.add(
            "Profiles",
            "profiles",
            CheckStatus::Warn,
            "No profiles yet.",
            Some("Run: githop init"),
        );
        return;
    }
    let issues = set.issues();
    let errors = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .count();
    if errors == 0 {
        r.add(
            "Profiles",
            "profiles",
            CheckStatus::Ok,
            format!("{} profile(s), all valid", set.profiles.len()),
            None,
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
            Severity::Warning => CheckStatus::Warn,
            Severity::Info => CheckStatus::Info,
        };
        let fix = i
            .profile_id
            .as_deref()
            .map(|p| format!("Run: githop edit {p}"));
        r.add(
            "Profiles",
            "profile-issue",
            status,
            format!("{who}{}", i.message),
            fix.as_deref(),
        );
    }
    if set.default_profile().is_none() {
        r.add(
            "Profiles",
            "default",
            CheckStatus::Info,
            "No default profile.",
            Some("Run: githop use <id>"),
        );
    }
    for p in &set.profiles {
        let key = &p.ssh_key;
        let shown = key_display(key);
        if !key.is_file() {
            let fix = format!("Run: githop keygen {}", p.id);
            r.add(
                "Keys",
                "key-file",
                CheckStatus::Warn,
                format!("{}: no key file at {shown}", p.label),
                Some(&fix),
            );
            continue;
        }
        let mut pub_path = key.clone().into_os_string();
        pub_path.push(".pub");
        if Path::new(&pub_path).is_file() {
            r.add(
                "Keys",
                "key-file",
                CheckStatus::Ok,
                format!("{}: key {shown}", p.label),
                None,
            );
        } else {
            r.add(
                "Keys",
                "key-pub",
                CheckStatus::Warn,
                format!("{}: {shown}.pub is missing", p.label),
                Some("Recreate it: ssh-keygen -y -f <key> > <key>.pub"),
            );
        }
        // Permission check: core has no ACL *read* API yet (only `restrict_key_acl`), and
        // `icacls` output is locale-dependent. Unix mode bits are checked here.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(key) {
                if meta.permissions().mode() & 0o077 != 0 {
                    r.add(
                        "Keys",
                        "key-acl",
                        CheckStatus::Problem,
                        format!("{}: {shown} can be read by other users.", p.label),
                        Some("Run: chmod 600 <key>"),
                    );
                }
            }
        }
    }
}

fn check_backups(r: &mut Report, store: &Store) {
    let managed = !store.settings().managed_global_keys.is_empty();
    match backup::original_gitconfig(store.paths()) {
        Some(p) => r.add(
            "Backups",
            "original",
            CheckStatus::Ok,
            format!("Original git config saved at {}", to_slash(&p)),
            None,
        ),
        None if managed => r.add(
            "Backups",
            "original",
            CheckStatus::Problem,
            "GitHop changed your global git config, but the original backup is missing.",
            Some("Keep a copy of ~/.gitconfig before running githop wipe."),
        ),
        None => r.add(
            "Backups",
            "original",
            CheckStatus::Info,
            "No backup yet. GitHop hasn't changed your global git config.",
            None,
        ),
    }
}

fn check_ssh_config(r: &mut Report, git: &Git) {
    match ssh::ssh_config_conflicts(git) {
        Ok(conflicts) if conflicts.is_empty() => {
            r.add(
                "SSH",
                "ssh-config",
                CheckStatus::Ok,
                "~/.ssh/config doesn't force a key for GitHub",
                None,
            );
        }
        Ok(conflicts) => {
            for c in conflicts {
                r.add(
                    "SSH",
                    "ssh-config",
                    CheckStatus::Warn,
                    format!(
                        "~/.ssh/config line {}: Host {} sets IdentityFile {}. SSH may offer that key first and sign you in as the wrong account.",
                        c.line, c.host_pattern, c.identity_file
                    ),
                    Some("Scope it under a different Host alias, e.g. Host github-work."),
                );
            }
        }
        Err(e) => r.add(
            "SSH",
            "ssh-config",
            CheckStatus::Warn,
            format!("Couldn't read ~/.ssh/config: {e}"),
            None,
        ),
    }
}

fn check_agent(r: &mut Report) {
    match ssh::agent_status() {
        AgentStatus::Running => r.add("SSH", "agent", CheckStatus::Ok, "ssh-agent is running", None),
        AgentStatus::Stopped => r.add(
            "SSH",
            "agent",
            CheckStatus::Info,
            "ssh-agent is stopped, so keys with a passphrase ask for it on every push.",
            Some("In an admin PowerShell: Set-Service ssh-agent -StartupType Automatic; Start-Service ssh-agent"),
        ),
        AgentStatus::Unknown => {
            if cfg!(windows) {
                r.add("SSH", "agent", CheckStatus::Info, "Couldn't check the ssh-agent service.", None);
            }
        }
    }
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
];

fn check_env(r: &mut Report, git: &Git) {
    let mut any = false;
    for (var, why) in OVERRIDE_ENV {
        if git.env_var(var).is_some_and(|v| !v.is_empty()) {
            any = true;
            r.add(
                "Environment",
                "env",
                CheckStatus::Warn,
                format!("{var} is set. It {why}"),
                Some("Remove it from your environment variables."),
            );
        }
    }
    if !any {
        r.add(
            "Environment",
            "env",
            CheckStatus::Ok,
            "No git identity environment variables set",
            None,
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

fn check_system_config(r: &mut Report, git: &Git, cwd: &Path) {
    let snap = match ConfigSnapshot::read(git, cwd) {
        Ok(s) => s,
        Err(e) => {
            r.add(
                "Git",
                "system-config",
                CheckStatus::Warn,
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
                "Git",
                "system-config",
                CheckStatus::Warn,
                format!("System git config sets {key} = {value} ({origin}). It may be managed by your IT team; GitHop never changes it."),
                Some("Profiles and pins still override it. Ask IT if commits look wrong."),
            );
        }
    }
    if !any {
        r.add(
            "Git",
            "system-config",
            CheckStatus::Ok,
            "System git config doesn't set identity or hooks",
            None,
        );
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
        assert!((3, 0) >= MIN_GIT);
    }
}
