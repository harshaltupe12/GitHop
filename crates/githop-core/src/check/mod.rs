//! `githop check --stage clone|commit|push` (Phase 3 §3.3–3.4; Plan §7.2–7.4, §12).
//!
//! Each stage is split into **pure decision functions** (no spawns, unit-tested)
//! and a thin orchestrator that reads git state through the contract API.
//!
//! Entry points: [`run_check`] (explicit context, used by tests) and
//! [`run_check_from_env`] (what `githop check` calls). Both **never panic or
//! return an error**: any GitHop failure becomes [`Outcome::InternalError`]
//! (exit 3, or 1 under `block-strict`). Messages are returned as data; the CLI
//! renders them (Design §9.4) on stderr.
//!
//! Spawn budget (Plan §11): commit fast path = 1 (`git config --list`), slow
//! path = 2 (+ `git var -l`); push = 2 (+ `git log`); clone = 1 (+ pin writes
//! when it pins). A non-clone checkout and a delete-only push spawn nothing.
//! The repo comes from git's hook environment without a spawn
//! ([`Repo::from_hook_env`]: hook cwd = work tree top, `GIT_DIR` for linked
//! worktrees); only an unrecognised layout costs one extra `rev-parse`.

pub mod clone;
pub mod commit;
pub mod identity_rules;
pub mod push;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::git::Git;
use crate::paths::{self, Paths};
use crate::profile::{Profile, ProfileSet};
use crate::repo::Repo;
use crate::settings::{EnforceMode, Settings};
use crate::store::Store;

pub use push::OffendingCommit;

/// Env var that turns GitHop checks off (escape hatch, like `--no-verify` for GitHop only).
pub const DISABLE_ENV: &str = "GITHOP_DISABLE";

/// Which hook is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    /// `post-checkout` after `git clone`.
    Clone,
    /// `pre-commit`, `pre-merge-commit` (merge commits) and
    /// `prepare-commit-msg` (revert / cherry-pick).
    Commit,
    /// `pre-push`.
    Push,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Clone => "clone",
            Stage::Commit => "commit",
            Stage::Push => "push",
        }
    }

    /// Parse `clone` / `commit` / `push` (also accepts the hook names).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "clone" | "post-checkout" => Some(Stage::Clone),
            "commit" | "pre-commit" => Some(Stage::Commit),
            "push" | "pre-push" => Some(Stage::Push),
            _ => None,
        }
    }
}

/// How a message line is shown (Design §9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// `✓` — something good happened (e.g. pinned).
    Ok,
    /// `✗` — blocked.
    Blocked,
    /// `!` — warning; the operation went through.
    Warning,
    /// Plain detail line under a headline.
    Info,
}

/// One line of output. `hint` (e.g. `Run: githop fix`) is shown dimmed on its own line after `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub level: Level,
    pub text: String,
    pub hint: Option<String>,
}

impl Message {
    pub fn new(level: Level, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
            hint: None,
        }
    }

    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn ok(text: impl Into<String>) -> Self {
        Self::new(Level::Ok, text)
    }
    pub fn blocked(text: impl Into<String>) -> Self {
        Self::new(Level::Blocked, text)
    }
    pub fn warning(text: impl Into<String>) -> Self {
        Self::new(Level::Warning, text)
    }
    pub fn info(text: impl Into<String>) -> Self {
        Self::new(Level::Info, text)
    }
}

/// What a check concluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Outcome {
    /// Nothing wrong (or nothing to check).
    Pass,
    /// Clone was auto-pinned to this profile.
    Pinned { profile_id: String },
    /// No profile owns this repo; never blocked.
    Unresolved,
    /// Confirmed identity mismatch. Blocks unless `enforce = warn`.
    /// `pinned_now` = profile the repo was pinned to during this check.
    Mismatch { pinned_now: Option<String> },
    /// GitHop itself failed. Fails open unless `block-strict`.
    InternalError { detail: String },
    /// `GITHOP_DISABLE=1`.
    Disabled,
}

impl Outcome {
    /// Process exit code for `githop check` (CLAUDE.md: 0 ok, 1 blocked, 3 internal error).
    pub fn exit_code(&self, stage: Stage, mode: EnforceMode) -> i32 {
        match self {
            Outcome::Pass | Outcome::Pinned { .. } | Outcome::Unresolved | Outcome::Disabled => 0,
            Outcome::Mismatch { .. } => {
                if mode == EnforceMode::Warn || stage == Stage::Clone {
                    0
                } else {
                    1
                }
            }
            Outcome::InternalError { .. } => {
                if mode == EnforceMode::BlockStrict {
                    1
                } else {
                    3
                }
            }
        }
    }

    /// Short label for the opt-in log (no data, just the kind).
    pub fn log_label(&self, mode: EnforceMode) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Pinned { .. } => "pinned",
            Outcome::Unresolved => "unresolved",
            Outcome::Mismatch { .. } if mode == EnforceMode::Warn => "warned",
            Outcome::Mismatch { .. } => "blocked",
            Outcome::InternalError { .. } => "error",
            Outcome::Disabled => "disabled",
        }
    }
}

/// Full result of a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub stage: Stage,
    pub outcome: Outcome,
    /// Enforce mode in effect.
    pub enforce: EnforceMode,
    /// Lines to print on stderr, in order.
    pub messages: Vec<Message>,
    /// Profile that owns the repo, if resolved.
    pub profile_id: Option<String>,
    /// Push stage: every offending commit found (not just the 10 listed). Phase 4 `fix` consumes this.
    pub offending: Vec<OffendingCommit>,
    /// Push stage: the scan hit [`push::MAX_COMMITS`].
    pub capped: bool,
}

impl CheckResult {
    pub fn new(stage: Stage, enforce: EnforceMode, outcome: Outcome) -> Self {
        Self {
            stage,
            outcome,
            enforce,
            messages: Vec::new(),
            profile_id: None,
            offending: Vec::new(),
            capped: false,
        }
    }

    /// Exit code for `githop check`.
    pub fn exit_code(&self) -> i32 {
        self.outcome.exit_code(self.stage, self.enforce)
    }

    /// Result for a GitHop failure (fail-open message, or blocked under block-strict).
    pub fn internal_error(stage: Stage, enforce: EnforceMode, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let mut r = Self::new(
            stage,
            enforce,
            Outcome::InternalError {
                detail: detail.clone(),
            },
        );
        let what = stage_noun(stage);
        if enforce == EnforceMode::BlockStrict && stage != Stage::Clone {
            r.messages.push(
                Message::blocked(format!(
                    "GitHop couldn't check this {what}, so it was blocked (block-strict mode)."
                ))
                .hint("Run: githop doctor"),
            );
        } else {
            r.messages.push(
                Message::warning(format!(
                    "GitHop couldn't check this {what} (it went through)."
                ))
                .hint("Run: githop doctor"),
            );
        }
        r.messages.push(Message::info(format!("Details: {detail}")));
        r
    }
}

fn stage_noun(stage: Stage) -> &'static str {
    match stage {
        Stage::Clone => "clone",
        Stage::Commit => "commit",
        Stage::Push => "push",
    }
}

/// Where and with what a check runs.
#[derive(Debug, Clone)]
pub struct CheckContext {
    pub git: Git,
    /// Working directory of the hook (the repo's top level for client hooks).
    pub cwd: PathBuf,
    pub paths: Paths,
}

/// True when `GITHOP_DISABLE=1` (as git and its hooks would see it).
pub fn is_disabled(git: &Git) -> bool {
    git.env_var(DISABLE_ENV)
        .is_some_and(|v| v.to_string_lossy().trim() == "1")
}

/// Read `settings.toml` for the enforce mode even when `profiles.toml` is
/// broken. Any problem → defaults (`enforce = block`).
pub fn load_settings_lenient(paths: &Paths) -> Settings {
    std::fs::read_to_string(paths.settings_file())
        .ok()
        .and_then(|s| toml::from_str::<Settings>(&s).ok())
        .unwrap_or_default()
}

/// Build a context from the process environment and run the check.
/// Never fails: setup problems (no git, no data dir) become an internal error.
pub fn run_check_from_env(stage: Stage, args: &[String], stdin: &str) -> CheckResult {
    let paths = match Paths::resolve() {
        Ok(p) => p,
        Err(e) => return CheckResult::internal_error(stage, EnforceMode::Block, e.to_string()),
    };
    let enforce = load_settings_lenient(&paths).enforce;
    let git = match Git::locate() {
        Ok(g) => g,
        Err(e) => return CheckResult::internal_error(stage, enforce, e.to_string()),
    };
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => return CheckResult::internal_error(stage, enforce, e.to_string()),
    };
    run_check(stage, args, stdin, &CheckContext { git, cwd, paths })
}

/// Run one check. `args` = the hook's arguments (after `--`), `stdin` = the
/// hook's stdin (push only). Never panics (panics inside are caught when the
/// build unwinds; with `panic = "abort"` the hook script fails open on the
/// crash exit code), never returns an error.
pub fn run_check(stage: Stage, args: &[String], stdin: &str, ctx: &CheckContext) -> CheckResult {
    let lenient = load_settings_lenient(&ctx.paths);
    let enforce = lenient.enforce;
    if is_disabled(&ctx.git) {
        if stage == Stage::Push {
            // Regression D4: a push made with GITHOP_DISABLE=1 still publishes
            // commits, so record them (no spawn) and Fix never rewrites them.
            push::record_only(ctx, args, stdin);
        }
        return CheckResult::new(stage, enforce, Outcome::Disabled);
    }
    // Check the history git actually pushes: no replace refs, no grafts
    // (security regression #1, #7). Env only, no extra spawn.
    let real = CheckContext {
        git: ctx.git.real_history(),
        cwd: ctx.cwd.clone(),
        paths: ctx.paths.clone(),
    };
    let ctx = &real;
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match stage {
        Stage::Commit => commit::run(ctx, enforce),
        Stage::Push => push::run(ctx, args, stdin, enforce),
        Stage::Clone => clone::run(ctx, args, enforce),
    }));
    let result = match run {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => CheckResult::internal_error(stage, enforce, e.to_string()),
        Err(panic) => {
            let detail = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unexpected error".to_string());
            CheckResult::internal_error(stage, enforce, detail)
        }
    };
    if lenient.logging {
        let _ = crate::log::append(
            &ctx.paths,
            &lenient,
            stage,
            result.profile_id.as_deref(),
            result.outcome.log_label(enforce),
        );
    }
    if matches!(result.outcome, Outcome::Mismatch { .. }) && result.exit_code() == 1 {
        record_block(&ctx.paths);
    }
    result
}

/// File the tray reads to show its "recently blocked" dot: unix seconds, ASCII.
pub const LAST_BLOCK_FILE: &str = "last-block";

/// Best effort: note the time of a block (no repo, profile or email data).
/// Only when the data dir already exists — a check never creates it.
fn record_block(paths: &Paths) {
    if !paths.root().is_dir() {
        return;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = std::fs::write(paths.root().join(LAST_BLOCK_FILE), secs.to_string());
}

// ---------------------------------------------------------------------------
// Shared helpers for the stage orchestrators.

/// The repo a hook runs in, without spawning when git's hook environment
/// identifies it (see [`Repo::from_hook_env`]); else one `git rev-parse`.
pub(crate) fn hook_repo(ctx: &CheckContext) -> Result<Repo> {
    Repo::from_hook_env(&ctx.git, &ctx.cwd)
}

/// Load profiles + settings (no spawn).
pub(crate) fn open_store(ctx: &CheckContext) -> Result<Store> {
    Store::open_at(ctx.paths.clone())
}

/// Look up the resolved profile.
pub(crate) fn resolved_profile<'a>(set: &'a ProfileSet, id: Option<&str>) -> Option<&'a Profile> {
    id.and_then(|id| set.get(id))
}

/// Pin `repo` to `profile` and remember it in `settings.pinned_repos`
/// (recorded best-effort with [`Store::update_settings`], so a concurrent
/// tray/CLI change is never overwritten). Spawns: only the pin itself — the
/// repo is not discovered again.
pub(crate) fn pin_and_record(store: &mut Store, repo: &Repo, profile: &Profile) -> Result<()> {
    crate::pin::pin_repo(repo, profile)?;
    let key = paths::normalize_repo_root(repo.root());
    let fk = paths::folder_key(&key);
    let _ = store.update_settings(|s| {
        if !s.pinned_repos.iter().any(|r| paths::folder_key(r) == fk) {
            s.pinned_repos.push(key);
        }
        Ok(())
    });
    Ok(())
}

/// Record that `root` was shown the "no owner match" hint. Returns `true` when
/// it had not been shown before (i.e. show it now). Saves best-effort.
pub(crate) fn first_hint(store: &mut Store, root: &Path) -> bool {
    let key = paths::normalize_repo_root(root);
    let fk = paths::folder_key(&key);
    if store
        .settings()
        .hinted_repos
        .iter()
        .any(|r| paths::folder_key(r) == fk)
    {
        return false;
    }
    let _ = store.update_settings(|s| {
        if !s.hinted_repos.iter().any(|r| paths::folder_key(r) == fk) {
            s.hinted_repos.push(key);
        }
        Ok(())
    });
    true
}

/// The unresolved-repo hint (Phase 3 §3.3).
pub(crate) fn unresolved_message() -> Message {
    Message::warning("GitHop: this repo has no owner match.").hint("Run: githop pin <profile>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_parse_roundtrip() {
        for s in [Stage::Clone, Stage::Commit, Stage::Push] {
            assert_eq!(Stage::parse(s.as_str()), Some(s));
        }
        assert_eq!(Stage::parse("pre-push"), Some(Stage::Push));
        assert_eq!(Stage::parse("Commit"), Some(Stage::Commit));
        assert_eq!(Stage::parse("merge"), None);
    }

    #[test]
    fn exit_codes_follow_enforce_modes() {
        use EnforceMode::*;
        let m = Outcome::Mismatch { pinned_now: None };
        let e = Outcome::InternalError { detail: "x".into() };
        // Spec §3.4 table.
        assert_eq!(m.exit_code(Stage::Commit, Block), 1);
        assert_eq!(m.exit_code(Stage::Commit, Warn), 0);
        assert_eq!(m.exit_code(Stage::Commit, BlockStrict), 1);
        assert_eq!(e.exit_code(Stage::Commit, Block), 3);
        assert_eq!(e.exit_code(Stage::Commit, Warn), 3);
        assert_eq!(e.exit_code(Stage::Commit, BlockStrict), 1);
        assert_eq!(m.exit_code(Stage::Push, Block), 1);
        // Clone never blocks on a mismatch.
        assert_eq!(m.exit_code(Stage::Clone, Block), 0);
        for ok in [
            Outcome::Pass,
            Outcome::Unresolved,
            Outcome::Disabled,
            Outcome::Pinned {
                profile_id: "office".into(),
            },
        ] {
            for mode in [Block, Warn, BlockStrict] {
                assert_eq!(ok.exit_code(Stage::Commit, mode), 0);
            }
        }
    }

    #[test]
    fn internal_error_messages() {
        let r = CheckResult::internal_error(Stage::Commit, EnforceMode::Block, "boom");
        assert_eq!(r.exit_code(), 3);
        assert_eq!(r.messages[0].level, Level::Warning);
        assert_eq!(
            r.messages[0].text,
            "GitHop couldn't check this commit (it went through)."
        );
        assert_eq!(r.messages[0].hint.as_deref(), Some("Run: githop doctor"));
        let s = CheckResult::internal_error(Stage::Push, EnforceMode::BlockStrict, "boom");
        assert_eq!(s.exit_code(), 1);
        assert_eq!(s.messages[0].level, Level::Blocked);
    }

    #[test]
    fn log_labels() {
        let m = Outcome::Mismatch { pinned_now: None };
        assert_eq!(m.log_label(EnforceMode::Block), "blocked");
        assert_eq!(m.log_label(EnforceMode::Warn), "warned");
    }

    #[test]
    fn disabled_short_circuits_without_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = CheckContext {
            // A git exe that doesn't exist: any spawn would be an internal error.
            git: Git::with_exe(tmp.path().join("no-git.exe")).env(DISABLE_ENV, "1"),
            cwd: tmp.path().to_path_buf(),
            paths: Paths::at(tmp.path().join("data")),
        };
        for stage in [Stage::Clone, Stage::Commit, Stage::Push] {
            let r = run_check(stage, &[], "", &ctx);
            assert_eq!(r.outcome, Outcome::Disabled);
            assert_eq!(r.exit_code(), 0);
        }
    }

    #[test]
    fn errors_fail_open_and_strict_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let ctx = CheckContext {
            git: Git::with_exe(tmp.path().join("no-git.exe")),
            cwd: tmp.path().to_path_buf(),
            paths: Paths::at(&data),
        };
        // Not a repo + git missing → internal error, never a panic.
        let r = run_check(Stage::Commit, &[], "", &ctx);
        assert!(matches!(r.outcome, Outcome::InternalError { .. }), "{r:?}");
        assert_eq!(r.exit_code(), 3);
        std::fs::write(
            data.join("settings.toml"),
            "version = 1\nenforce = \"block-strict\"\n",
        )
        .unwrap();
        let r = run_check(Stage::Commit, &[], "", &ctx);
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn lenient_settings_survive_garbage() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.toml"), "this is = = not toml").unwrap();
        assert_eq!(
            load_settings_lenient(&Paths::at(tmp.path())).enforce,
            EnforceMode::Block
        );
        std::fs::write(tmp.path().join("settings.toml"), "enforce = \"warn\"\n").unwrap();
        assert_eq!(
            load_settings_lenient(&Paths::at(tmp.path())).enforce,
            EnforceMode::Warn
        );
    }
}
