//! `check --stage commit` (pre-commit) — Phase 3 §3.3, Plan §7.3, C1/C8/C9.
//!
//! 1. Fast path (1 spawn, the config snapshot): pinned profile's email equals
//!    the effective `user.email`, which is set at local scope, no
//!    `author.email`/`committer.email` config and no `GIT_AUTHOR_EMAIL` /
//!    `GIT_COMMITTER_EMAIL` env → pass without `git var`.
//! 2. Otherwise resolve; unresolved → pass (hint once per repo).
//! 3. Effective identity (`git var -l`, 2nd spawn). Match → pass.
//! 4. Mismatch: repo not pinned → pin now, block with "run the commit again"
//!    (git loaded the identity before the hook ran). Pinned → block with the
//!    Design §9.3 message. `enforce = warn` → same facts as a warning, exit 0.

use crate::error::Result;
use crate::gitconfig::{ConfigScope, ConfigSnapshot};
use crate::identity::{self, EffectiveIdentity};
use crate::pin::PIN_KEY;
use crate::profile::ProfileSet;
use crate::resolver::{self, Resolution, ResolutionSource};
use crate::settings::EnforceMode;
use crate::Git;

use super::{CheckContext, CheckResult, Message, Outcome, Stage};

/// Identity env vars that override config (C9).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvOverrides {
    pub author_email: Option<String>,
    pub committer_email: Option<String>,
}

impl EnvOverrides {
    /// Read `GIT_AUTHOR_EMAIL` / `GIT_COMMITTER_EMAIL` as git would see them (empty = unset).
    pub fn from_git(git: &Git) -> Self {
        let get = |k: &str| {
            git.env_var(k)
                .map(|v| v.to_string_lossy().trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Self {
            author_email: get("GIT_AUTHOR_EMAIL"),
            committer_email: get("GIT_COMMITTER_EMAIL"),
        }
    }

    pub fn any(&self) -> bool {
        self.author_email.is_some() || self.committer_email.is_some()
    }

    /// Drop env values equal to what config already gives that role. `git commit`
    /// exports `GIT_AUTHOR_NAME/EMAIL/DATE` to its hooks with the configured
    /// identity, so only a value that *differs* from config is a real override.
    #[must_use]
    pub fn without_configured(self, snapshot: &ConfigSnapshot) -> Self {
        let user = snapshot.get("user.email");
        let author_cfg = snapshot.get("author.email").or(user);
        let committer_cfg = snapshot.get("committer.email").or(user);
        let keep = |env: Option<String>, cfg: Option<&str>| {
            env.filter(|v| !cfg.is_some_and(|c| email_eq(v, c)))
        };
        Self {
            author_email: keep(self.author_email, author_cfg),
            committer_email: keep(self.committer_email, committer_cfg),
        }
    }
}

fn email_eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Fast path (no `git var`). True only when the snapshot alone proves the
/// commit will use the pinned profile's email for author and committer.
pub fn fast_path_ok(snapshot: &ConfigSnapshot, set: &ProfileSet, env: &EnvOverrides) -> bool {
    if env.any() {
        return false;
    }
    let Some(pin) = snapshot.get_scoped(PIN_KEY, ConfigScope::Local) else {
        return false;
    };
    let Some(profile) = set.get(pin.trim()) else {
        return false;
    };
    // author.email / committer.email (any scope) override user.email.
    if snapshot.get("author.email").is_some() || snapshot.get("committer.email").is_some() {
        return false;
    }
    // The *effective* user.email must be the local one (a `git -c` or worktree
    // value would win over it).
    match snapshot.get_entry("user.email") {
        Some(e) if e.scope == ConfigScope::Local => e
            .value
            .as_deref()
            .is_some_and(|v| email_eq(v, &profile.email)),
        _ => false,
    }
}

/// What to do with a mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMismatch {
    pub profile_id: String,
    pub label: String,
    pub expected_email: String,
    /// The wrong email the commit would use (`None` = no email at all).
    pub would_use: Option<String>,
    /// The repo is already pinned (pin decided the profile).
    pub pinned: bool,
    /// Env var responsible for the wrong email, if any (C9).
    pub env_var: Option<&'static str>,
}

/// Commit decision (pure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitDecision {
    Pass,
    Unresolved,
    Mismatch(CommitMismatch),
}

/// Decide from the resolution and the effective identity. Pure.
pub fn decide(
    resolution: &Resolution,
    set: &ProfileSet,
    identity: Option<&EffectiveIdentity>,
    env: &EnvOverrides,
) -> CommitDecision {
    let Some(profile) = resolution.profile_id.as_deref().and_then(|id| set.get(id)) else {
        return CommitDecision::Unresolved;
    };
    let pinned = resolution.source == Some(ResolutionSource::Pin);
    let mk = |would_use: Option<String>, env_var| {
        CommitDecision::Mismatch(CommitMismatch {
            profile_id: profile.id.clone(),
            label: profile.label.clone(),
            expected_email: profile.email.clone(),
            would_use,
            pinned,
            env_var,
        })
    };
    let Some(id) = identity else {
        return mk(None, None);
    };
    let author_ok = email_eq(&id.author.email, &profile.email);
    let committer_ok = email_eq(&id.committer.email, &profile.email);
    if author_ok && committer_ok {
        return CommitDecision::Pass;
    }
    let (email, env_name, env_val) = if !author_ok {
        (
            &id.author.email,
            "GIT_AUTHOR_EMAIL",
            env.author_email.as_deref(),
        )
    } else {
        (
            &id.committer.email,
            "GIT_COMMITTER_EMAIL",
            env.committer_email.as_deref(),
        )
    };
    let env_var = env_val.filter(|v| email_eq(v, email)).map(|_| env_name);
    let would_use = (!email.trim().is_empty()).then(|| email.trim().to_string());
    mk(would_use, env_var)
}

/// Result of trying to pin during the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinAttempt {
    /// Repo was already pinned; nothing tried.
    AlreadyPinned,
    /// Pinned now.
    Pinned,
    /// Pinning failed (reason).
    Failed(String),
}

/// The line explaining an env override (C9), and what to do (without the
/// final "commit again" part). `git commit --author=…` reaches hooks through
/// `GIT_AUTHOR_EMAIL` too, and a hook can't tell the two apart (review
/// 2026-11-01 #11), so the author line names both.
fn env_override_lines(var: &str, email: &str) -> (String, String) {
    if var == "GIT_AUTHOR_EMAIL" {
        (
            format!("This commit's author email is {email} (from --author or GIT_AUTHOR_EMAIL)."),
            "Drop --author or unset GIT_AUTHOR_EMAIL".to_string(),
        )
    } else {
        (
            format!("{var} is set, and it overrides the repo's settings."),
            format!("Unset {var}"),
        )
    }
}

/// Messages for a mismatch (Design §9.3). Pure.
pub fn mismatch_messages(m: &CommitMismatch, mode: EnforceMode, pin: &PinAttempt) -> Vec<Message> {
    let warn = mode == EnforceMode::Warn;
    let would = m.would_use.as_deref().unwrap_or("no email");
    let mut out = vec![
        if warn {
            Message::warning("GitHop: this commit uses the wrong identity (it went through).")
        } else {
            Message::blocked("GitHop: commit blocked")
        },
        Message::info(format!("This repo belongs to {}.", m.label)),
    ];
    let would_line = Message::info(format!("Commit would use {would}."));
    let pin_hint = format!("Run: githop pin {}", m.profile_id);
    match (pin, m.env_var) {
        (PinAttempt::Pinned, _) if warn => {
            out.push(would_line);
            out.push(
                Message::ok(format!(
                    "Pinned this repo to {} for your next commits.",
                    m.label
                ))
                .hint("To re-stamp this commit: githop fix"),
            );
        }
        (PinAttempt::Pinned, None) => {
            out.push(would_line);
            out.push(
                Message::ok(format!(
                    "Identity fixed for this repo: pinned to {}.",
                    m.label
                ))
                .hint("Run the commit again."),
            );
        }
        (PinAttempt::Pinned, Some(var)) => {
            let (why, action) = env_override_lines(var, would);
            out.push(would_line);
            out.push(Message::ok(format!("Pinned this repo to {}.", m.label)));
            out.push(Message::info(why).hint(format!("{action}, then run the commit again.")));
        }
        (PinAttempt::Failed(reason), _) => {
            out.push(would_line.hint(pin_hint));
            out.push(Message::warning(format!(
                "GitHop couldn't pin this repo: {reason}"
            )));
        }
        (PinAttempt::AlreadyPinned, Some(var)) => {
            let (why, action) = env_override_lines(var, would);
            out.push(would_line);
            out.push(Message::info(why).hint(if warn {
                format!("{action}. To re-stamp this commit: githop fix")
            } else {
                format!("{action}, then commit again.")
            }));
        }
        (PinAttempt::AlreadyPinned, None) => {
            out.push(would_line.hint(if warn {
                format!("{pin_hint}. To re-stamp this commit: githop fix")
            } else {
                pin_hint
            }));
        }
    }
    out
}

/// Orchestrator. Errors become an internal error in [`super::run_check`].
pub(crate) fn run(ctx: &CheckContext, enforce: EnforceMode) -> Result<CheckResult> {
    let repo = super::hook_repo(ctx)?;
    let root = repo.root().to_path_buf();
    let snapshot = ConfigSnapshot::read(&ctx.git, &root)?;
    let mut store = super::open_store(ctx)?;
    let env = EnvOverrides::from_git(&ctx.git).without_configured(&snapshot);

    let mut result = CheckResult::new(Stage::Commit, enforce, Outcome::Pass);
    if fast_path_ok(&snapshot, store.profiles(), &env) {
        result.profile_id = snapshot
            .get_scoped(PIN_KEY, ConfigScope::Local)
            .map(|s| s.trim().to_string());
        return Ok(result);
    }

    let aliases = store.settings().github_host_aliases.clone();
    let resolution = resolver::resolve(&snapshot, &root, store.profiles(), &aliases);
    if super::resolved_profile(store.profiles(), resolution.profile_id.as_deref()).is_none() {
        result.outcome = Outcome::Unresolved;
        if super::first_hint(&mut store, &root) {
            result.messages.push(super::unresolved_message());
        }
        return Ok(result);
    }
    result.profile_id = resolution.profile_id.clone();

    let identity = identity::effective_identity_with(&ctx.git, &root, &snapshot)?;
    let decision = decide(&resolution, store.profiles(), identity.as_ref(), &env);
    let CommitDecision::Mismatch(m) = decision else {
        return Ok(result);
    };

    let pin = if m.pinned {
        PinAttempt::AlreadyPinned
    } else {
        let profile = store.profiles().get(&m.profile_id).cloned();
        match profile {
            Some(p) => match super::pin_and_record(&mut store, &repo, &p) {
                Ok(()) => PinAttempt::Pinned,
                Err(e) => PinAttempt::Failed(e.to_string()),
            },
            None => PinAttempt::Failed("profile not found".into()),
        }
    };
    result.outcome = Outcome::Mismatch {
        pinned_now: (pin == PinAttempt::Pinned).then(|| m.profile_id.clone()),
    };
    result.messages = mismatch_messages(&m, enforce, &pin);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::check::Level;
    use crate::gitconfig::ConfigEntry;
    use crate::identity::Identity;
    use crate::profile::{Profile, ProfileColor};

    fn profile(id: &str, label: &str, email: &str) -> Profile {
        Profile {
            id: id.into(),
            label: label.into(),
            name: label.into(),
            email: email.into(),
            color: ProfileColor::Blue,
            ssh_key: PathBuf::from(format!("C:/Users/me/.ssh/githop_{id}")),
            owners: vec![],
            folders: vec![],
        }
    }

    fn set() -> ProfileSet {
        ProfileSet {
            version: 1,
            default: Some("personal".into()),
            profiles: vec![
                profile("office", "Office", "harshal@office.example"),
                profile("personal", "Personal", "harshal@personal.example"),
            ],
        }
    }

    fn entry(scope: ConfigScope, key: &str, value: &str) -> ConfigEntry {
        ConfigEntry {
            scope,
            origin: String::new(),
            key: key.into(),
            value: Some(value.into()),
        }
    }

    fn pinned_snapshot(local_email: &str) -> Vec<ConfigEntry> {
        vec![
            entry(
                ConfigScope::Global,
                "user.email",
                "harshal@personal.example",
            ),
            entry(ConfigScope::Local, "user.email", local_email),
            entry(ConfigScope::Local, "githop.profile", "office"),
        ]
    }

    fn ident(author: &str, committer: &str) -> EffectiveIdentity {
        EffectiveIdentity {
            author: Identity {
                name: "H".into(),
                email: author.into(),
            },
            committer: Identity {
                name: "H".into(),
                email: committer.into(),
            },
        }
    }

    fn resolution(id: Option<&str>, source: Option<ResolutionSource>) -> Resolution {
        Resolution {
            profile_id: id.map(Into::into),
            source,
            remote_name: None,
            remote: None,
            pinned: (source == Some(ResolutionSource::Pin)).then(|| id.unwrap_or("").to_string()),
            warnings: vec![],
        }
    }

    #[test]
    fn fast_path_pinned_and_matching() {
        let s = ConfigSnapshot::from_entries(pinned_snapshot("Harshal@Office.example"));
        assert!(fast_path_ok(&s, &set(), &EnvOverrides::default()));
    }

    #[test]
    fn fast_path_rejects_each_unsafe_case() {
        let none = EnvOverrides::default();
        // wrong local email
        let s = ConfigSnapshot::from_entries(pinned_snapshot("harshal@personal.example"));
        assert!(!fast_path_ok(&s, &set(), &none));
        // env override
        let s = ConfigSnapshot::from_entries(pinned_snapshot("harshal@office.example"));
        let env = EnvOverrides {
            author_email: Some("x@y".into()),
            committer_email: None,
        };
        assert!(!fast_path_ok(&s, &set(), &env));
        // not pinned
        let s = ConfigSnapshot::from_entries(vec![entry(
            ConfigScope::Local,
            "user.email",
            "harshal@office.example",
        )]);
        assert!(!fast_path_ok(&s, &set(), &none));
        // pin to a profile that no longer exists
        let mut e = pinned_snapshot("harshal@office.example");
        e[2] = entry(ConfigScope::Local, "githop.profile", "gone");
        assert!(!fast_path_ok(
            &ConfigSnapshot::from_entries(e),
            &set(),
            &none
        ));
        // pin at global scope doesn't count
        let mut e = pinned_snapshot("harshal@office.example");
        e[2] = entry(ConfigScope::Global, "githop.profile", "office");
        assert!(!fast_path_ok(
            &ConfigSnapshot::from_entries(e),
            &set(),
            &none
        ));
        // `git -c user.email=…` (command scope) wins over local
        let mut e = pinned_snapshot("harshal@office.example");
        e.push(entry(
            ConfigScope::Command,
            "user.email",
            "harshal@office.example",
        ));
        assert!(!fast_path_ok(
            &ConfigSnapshot::from_entries(e),
            &set(),
            &none
        ));
        // author.email overrides user.email
        let mut e = pinned_snapshot("harshal@office.example");
        e.insert(0, entry(ConfigScope::Global, "author.email", "x@y"));
        assert!(!fast_path_ok(
            &ConfigSnapshot::from_entries(e),
            &set(),
            &none
        ));
    }

    #[test]
    fn decide_table() {
        let none = EnvOverrides::default();
        let s = set();
        let pinned = resolution(Some("office"), Some(ResolutionSource::Pin));
        let owner = resolution(Some("office"), Some(ResolutionSource::Owner));
        let office = ident("harshal@office.example", "HARSHAL@office.example");
        let personal = ident("harshal@personal.example", "harshal@personal.example");

        assert_eq!(
            decide(&pinned, &s, Some(&office), &none),
            CommitDecision::Pass
        );
        assert_eq!(
            decide(&resolution(None, None), &s, Some(&personal), &none),
            CommitDecision::Unresolved
        );
        assert_eq!(
            decide(
                &resolution(Some("gone"), Some(ResolutionSource::Owner)),
                &s,
                Some(&personal),
                &none
            ),
            CommitDecision::Unresolved
        );
        let CommitDecision::Mismatch(m) = decide(&owner, &s, Some(&personal), &none) else {
            panic!("expected mismatch")
        };
        assert!(!m.pinned);
        assert_eq!(m.would_use.as_deref(), Some("harshal@personal.example"));
        assert_eq!(m.env_var, None);
        let CommitDecision::Mismatch(m) = decide(&pinned, &s, Some(&personal), &none) else {
            panic!("expected mismatch")
        };
        assert!(m.pinned);
        // committer only
        let mixed = ident("harshal@office.example", "harshal@personal.example");
        assert!(matches!(
            decide(&pinned, &s, Some(&mixed), &none),
            CommitDecision::Mismatch(_)
        ));
        // no identity at all
        let CommitDecision::Mismatch(m) = decide(&owner, &s, None, &none) else {
            panic!("expected mismatch")
        };
        assert_eq!(m.would_use, None);
    }

    #[test]
    fn decide_detects_env_override() {
        let env = EnvOverrides {
            author_email: Some("harshal@personal.example".into()),
            committer_email: None,
        };
        let pinned = resolution(Some("office"), Some(ResolutionSource::Pin));
        let id = ident("harshal@personal.example", "harshal@office.example");
        let CommitDecision::Mismatch(m) = decide(&pinned, &set(), Some(&id), &env) else {
            panic!("expected mismatch")
        };
        assert_eq!(m.env_var, Some("GIT_AUTHOR_EMAIL"));
    }

    fn mismatch(pinned: bool, env_var: Option<&'static str>) -> CommitMismatch {
        CommitMismatch {
            profile_id: "office".into(),
            label: "Office".into(),
            expected_email: "harshal@office.example".into(),
            would_use: Some("harshal@personal".into()),
            pinned,
            env_var,
        }
    }

    #[test]
    fn pinned_mismatch_message_matches_design() {
        let msgs = mismatch_messages(
            &mismatch(true, None),
            EnforceMode::Block,
            &PinAttempt::AlreadyPinned,
        );
        let lines: Vec<_> = msgs
            .iter()
            .map(|m| (m.level, m.text.as_str(), m.hint.as_deref()))
            .collect();
        assert_eq!(
            lines,
            vec![
                (Level::Blocked, "GitHop: commit blocked", None),
                (Level::Info, "This repo belongs to Office.", None),
                (
                    Level::Info,
                    "Commit would use harshal@personal.",
                    Some("Run: githop pin office")
                ),
            ]
        );
    }

    #[test]
    fn unpinned_mismatch_pins_and_asks_rerun() {
        let msgs = mismatch_messages(
            &mismatch(false, None),
            EnforceMode::Block,
            &PinAttempt::Pinned,
        );
        assert_eq!(msgs[0].level, Level::Blocked);
        let last = msgs.last().unwrap();
        assert_eq!(last.level, Level::Ok);
        assert_eq!(last.text, "Identity fixed for this repo: pinned to Office.");
        assert_eq!(last.hint.as_deref(), Some("Run the commit again."));
    }

    #[test]
    fn warn_mode_never_uses_blocked_level() {
        for pin in [
            PinAttempt::AlreadyPinned,
            PinAttempt::Pinned,
            PinAttempt::Failed("x".into()),
        ] {
            for env in [None, Some("GIT_AUTHOR_EMAIL")] {
                let msgs = mismatch_messages(&mismatch(false, env), EnforceMode::Warn, &pin);
                assert!(msgs.iter().all(|m| m.level != Level::Blocked), "{msgs:?}");
                assert!(msgs[0].text.contains("it went through"));
            }
        }
    }

    /// Review 2026-11-01 #11: `git commit --author=…` reaches the hook as
    /// GIT_AUTHOR_EMAIL, so the message must not claim the variable was set.
    #[test]
    fn author_override_message_covers_author_flag_and_env() {
        let msgs = mismatch_messages(
            &mismatch(true, Some("GIT_AUTHOR_EMAIL")),
            EnforceMode::Block,
            &PinAttempt::AlreadyPinned,
        );
        assert!(
            msgs.iter().any(|m| m.text
                == "This commit's author email is harshal@personal (from --author or GIT_AUTHOR_EMAIL)."),
            "{msgs:?}"
        );
        assert!(msgs.iter().all(|m| !m.text.contains("is set")), "{msgs:?}");
        assert_eq!(
            msgs.last().unwrap().hint.as_deref(),
            Some("Drop --author or unset GIT_AUTHOR_EMAIL, then commit again.")
        );
        let msgs = mismatch_messages(
            &mismatch(false, Some("GIT_AUTHOR_EMAIL")),
            EnforceMode::Block,
            &PinAttempt::Pinned,
        );
        assert_eq!(
            msgs.last().unwrap().hint.as_deref(),
            Some("Drop --author or unset GIT_AUTHOR_EMAIL, then run the commit again.")
        );
        // The committer has no command-line flag: that one is the env var.
        let msgs = mismatch_messages(
            &mismatch(true, Some("GIT_COMMITTER_EMAIL")),
            EnforceMode::Block,
            &PinAttempt::AlreadyPinned,
        );
        assert!(msgs
            .iter()
            .any(|m| m.text.starts_with("GIT_COMMITTER_EMAIL is set")));
        assert_eq!(
            msgs.last().unwrap().hint.as_deref(),
            Some("Unset GIT_COMMITTER_EMAIL, then commit again.")
        );
    }

    #[test]
    fn env_overrides_from_git_handle() {
        let g = Git::with_exe("git")
            .env("GIT_AUTHOR_EMAIL", " a@b ")
            .env("GIT_COMMITTER_EMAIL", "");
        let e = EnvOverrides::from_git(&g);
        assert_eq!(e.author_email.as_deref(), Some("a@b"));
        assert_eq!(e.committer_email, None);
    }

    #[test]
    fn env_equal_to_config_is_not_an_override() {
        // `git commit` exports GIT_AUTHOR_EMAIL to hooks with the configured value.
        let s = ConfigSnapshot::from_entries(pinned_snapshot("harshal@office.example"));
        let exported = EnvOverrides {
            author_email: Some("Harshal@Office.example".into()),
            committer_email: None,
        }
        .without_configured(&s);
        assert_eq!(exported, EnvOverrides::default());
        assert!(fast_path_ok(&s, &set(), &exported));

        // A different value is a real override (C9).
        let real = EnvOverrides {
            author_email: Some("harshal@personal.example".into()),
            committer_email: None,
        }
        .without_configured(&s);
        assert_eq!(
            real.author_email.as_deref(),
            Some("harshal@personal.example")
        );
        assert!(!fast_path_ok(&s, &set(), &real));
    }
}
