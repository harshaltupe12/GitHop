//! `check --stage clone` (post-checkout) — Phase 3 §3.3, Plan §7.2, C3.
//!
//! Runs when `$1` (previous HEAD) is the null id, i.e. a fresh clone, or when
//! the hook says the checkout is in a submodule (`GITHOP_SUBMODULE=1`: git
//! checks a new submodule out with a non-null previous HEAD, regression D10).
//! In a submodule that isn't fresh, only an owner/folder pin happens, quietly.
//! Owner/folder match and not pinned → pin (`✓ GitHop: pinned to Office`).
//! Default profile only → not pinned, hint (same rule as `scan --apply`, ADR
//! 0003 #6). Unresolved → hint. Local `core.hooksPath` (husky etc.) → warning.
//! Never blocks.

use crate::error::Result;
use crate::gitconfig::ConfigSnapshot;
use crate::resolver::{self, Resolution, ResolutionSource, ResolutionWarning};
use crate::settings::EnforceMode;

use super::push::is_null_sha;
use super::{CheckContext, CheckResult, Message, Outcome, Stage};

/// Set by the post-checkout hook when the repo is a submodule.
pub const SUBMODULE_ENV: &str = "GITHOP_SUBMODULE";

/// True when post-checkout args say this is the checkout at the end of a clone.
pub fn is_fresh_clone(args: &[String]) -> bool {
    args.first().is_some_and(|a| is_null_sha(a.trim()))
}

/// What the clone check should do. Pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneDecision {
    /// Already pinned to an existing profile.
    AlreadyPinned { profile_id: String },
    /// Pin to this profile (an owner or folder rule matched).
    Pin { profile_id: String },
    /// Only the default profile applies: don't pin (a pin would later beat an
    /// owner rule added for this repo, review 2026-11-01 #4 / ADR 0003 #6),
    /// show the "no owner match" hint instead.
    DefaultOnly { profile_id: String },
    /// No profile; show the hint.
    Unresolved,
}

pub fn decide(resolution: &Resolution, profile_exists: impl Fn(&str) -> bool) -> CloneDecision {
    match (&resolution.profile_id, resolution.source) {
        (Some(id), _) if !profile_exists(id) => CloneDecision::Unresolved,
        (Some(id), Some(ResolutionSource::Pin)) => CloneDecision::AlreadyPinned {
            profile_id: id.clone(),
        },
        (Some(id), Some(ResolutionSource::Default)) => CloneDecision::DefaultOnly {
            profile_id: id.clone(),
        },
        (Some(id), _) => CloneDecision::Pin {
            profile_id: id.clone(),
        },
        (None, _) => CloneDecision::Unresolved,
    }
}

/// The "no owner match" hint for a clone that only the default profile
/// covers. Not pinned, so an owner rule added later still wins.
pub fn default_only_message(label: &str, profile_id: &str) -> Message {
    Message::warning(format!(
        "GitHop: this repo has no owner match, so it uses your default profile ({label})."
    ))
    .hint(format!("To keep it that way: githop pin {profile_id}"))
}

/// Warning for a repo-local `core.hooksPath` (C3), if the resolution reported one.
pub fn local_hooks_path_warning(resolution: &Resolution) -> Option<Message> {
    resolution.warnings.iter().find_map(|w| match w {
        ResolutionWarning::LocalHooksPath { path } => Some(
            Message::warning(format!(
                "GitHop: this repo uses its own hooks folder ({path}), so GitHop's checks won't run here."
            ))
            .hint("Run: githop hooks snippet commit (and add the line to the repo's hook)"),
        ),
        _ => None,
    })
}

/// Orchestrator.
pub(crate) fn run(
    ctx: &CheckContext,
    args: &[String],
    enforce: EnforceMode,
) -> Result<CheckResult> {
    let mut result = CheckResult::new(Stage::Clone, enforce, Outcome::Pass);
    let fresh = is_fresh_clone(args);
    let submodule = ctx
        .git
        .env_var(SUBMODULE_ENV)
        .is_some_and(|v| v.to_string_lossy().trim() == "1");
    if !fresh && !submodule {
        return Ok(result);
    }
    let repo = super::hook_repo(ctx)?;
    let root = repo.root().to_path_buf();
    let snapshot = ConfigSnapshot::read(&ctx.git, &root)?;
    let mut store = super::open_store(ctx)?;
    let aliases = crate::aliases::current(&ctx.paths, &ctx.git, store.settings());
    let resolution = resolver::resolve(&snapshot, &root, store.profiles(), &aliases);

    let decision = decide(&resolution, |id| store.profiles().get(id).is_some());
    if !fresh && !matches!(decision, CloneDecision::Pin { .. }) {
        // Every later checkout in a submodule: nothing to say.
        return Ok(result);
    }
    match decision {
        CloneDecision::AlreadyPinned { profile_id } => result.profile_id = Some(profile_id),
        CloneDecision::Pin { profile_id } => {
            result.profile_id = Some(profile_id.clone());
            if let Some(p) = store.profiles().get(&profile_id).cloned() {
                match super::pin_and_record(&mut store, &repo, &p) {
                    Ok(()) => {
                        result
                            .messages
                            .push(Message::ok(format!("GitHop: pinned to {}", p.label)));
                        result.outcome = Outcome::Pinned { profile_id };
                        // ADR 0006: GitHub HTTPS remote → policy; host keys so the
                        // first SSH push never stops at a yes/no prompt.
                        let policy = store.settings().remote_https_policy;
                        match crate::remote_ssh::apply_policy_with(&repo, &snapshot, policy) {
                            Ok(action) => result
                                .messages
                                .extend(crate::remote_ssh::pin_message(&action, &p.label)),
                            Err(e) => result.messages.push(
                                Message::warning(format!(
                                    "GitHop couldn't switch this repo to SSH: {e}"
                                ))
                                .hint("Run: githop remote ssh"),
                            ),
                        }
                        crate::ssh::ensure_github_known_hosts_best_effort(&ctx.git, &ctx.paths);
                    }
                    Err(e) => {
                        result.messages.push(
                            Message::warning(format!("GitHop couldn't pin this repo: {e}"))
                                .hint(format!("Run: githop pin {}", p.id)),
                        );
                    }
                }
            }
        }
        CloneDecision::DefaultOnly { profile_id } => {
            result.profile_id = Some(profile_id.clone());
            let label = store
                .profiles()
                .get(&profile_id)
                .map(|p| p.label.clone())
                .unwrap_or_else(|| profile_id.clone());
            result
                .messages
                .push(default_only_message(&label, &profile_id));
        }
        CloneDecision::Unresolved => {
            result.outcome = Outcome::Unresolved;
            result.messages.push(super::unresolved_message());
        }
    }
    if let Some(w) = local_hooks_path_warning(&resolution) {
        result.messages.push(w);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(
        id: Option<&str>,
        source: Option<ResolutionSource>,
        warnings: Vec<ResolutionWarning>,
    ) -> Resolution {
        Resolution {
            profile_id: id.map(Into::into),
            source,
            remote_name: None,
            remote: None,
            pinned: None,
            warnings,
        }
    }

    #[test]
    fn fresh_clone_detection() {
        assert!(is_fresh_clone(&["0".repeat(40), "abc".into(), "1".into()]));
        assert!(is_fresh_clone(&["0".repeat(64)]));
        assert!(!is_fresh_clone(&["1".repeat(40), "abc".into(), "1".into()]));
        assert!(!is_fresh_clone(&[]));
    }

    #[test]
    fn decision_table() {
        let exists = |id: &str| id != "gone";
        assert_eq!(
            decide(
                &res(Some("office"), Some(ResolutionSource::Owner), vec![]),
                exists
            ),
            CloneDecision::Pin {
                profile_id: "office".into()
            }
        );
        assert_eq!(
            decide(
                &res(Some("personal"), Some(ResolutionSource::Default), vec![]),
                exists
            ),
            CloneDecision::DefaultOnly {
                profile_id: "personal".into()
            },
            "review 2026-11-01 #4: a default-only match is never pinned"
        );
        assert_eq!(
            decide(
                &res(Some("work"), Some(ResolutionSource::Folder), vec![]),
                exists
            ),
            CloneDecision::Pin {
                profile_id: "work".into()
            }
        );
        assert_eq!(
            decide(
                &res(Some("office"), Some(ResolutionSource::Pin), vec![]),
                exists
            ),
            CloneDecision::AlreadyPinned {
                profile_id: "office".into()
            }
        );
        assert_eq!(
            decide(&res(None, None, vec![]), exists),
            CloneDecision::Unresolved
        );
        assert_eq!(
            decide(
                &res(Some("gone"), Some(ResolutionSource::Owner), vec![]),
                exists
            ),
            CloneDecision::Unresolved
        );
    }

    #[test]
    fn husky_warning() {
        let r = res(
            None,
            None,
            vec![ResolutionWarning::LocalHooksPath {
                path: ".husky".into(),
            }],
        );
        let w = local_hooks_path_warning(&r).unwrap();
        assert!(w.text.contains(".husky"));
        assert!(local_hooks_path_warning(&res(None, None, vec![])).is_none());
    }

    #[test]
    fn non_clone_checkout_spawns_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = CheckContext {
            git: crate::Git::with_exe(tmp.path().join("no-git.exe")),
            cwd: tmp.path().to_path_buf(),
            paths: crate::Paths::at(tmp.path()),
        };
        let r = run(
            &ctx,
            &["1".repeat(40), "2".repeat(40), "1".into()],
            EnforceMode::Block,
        )
        .unwrap();
        assert_eq!(r.outcome, Outcome::Pass);
        assert!(r.messages.is_empty());
    }
}
