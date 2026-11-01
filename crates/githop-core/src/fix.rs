//! `githop fix` — re-stamp unpushed commits that used the wrong identity
//! (Phase 4; Plan §1, §3 D6, §7.5, §12).
//!
//! Entry points: [`plan`] (read-only, never fails because of repo *state* —
//! problems go into [`FixPlan::blocked_reason`] so the UI can show them),
//! [`apply`], [`undo`], [`list_backups`], [`latest_backup`].
//!
//! # Which commits are touched
//!
//! Only commits on the current branch that are **not pushed**. "Pushed" is
//! conservative (Review 2026-11-01 #2, CLAUDE.md rule 7) — a commit is pushed
//! when it is reachable from any of:
//!
//! - a remote-tracking ref (`--remotes`);
//! - any tag, local tags included (`--tags`);
//! - a sha the pre-push check recorded in `<common dir>/githop/pushed`
//!   ([`crate::check::push::PUSHED_FILE`]);
//! - the tip of any *other* local branch whose configured upstream
//!   (`branch.<b>.remote` + `branch.<b>.merge`) has no remote-tracking ref and
//!   no recorded push (single-branch clones, URL remotes, pruned branches).
//!
//! When the **current** branch is in that last state, Fix refuses
//! ([`FixBlocker::PushedUnknown`]); in a shallow clone it refuses unless the
//! branch has a live remote-tracking upstream ([`FixBlocker::Shallow`]).
//!
//! Inside the unpushed set a commit is recreated when it is an offender **or**
//! one of its parents was recreated. Offenders follow
//! [`crate::check::identity_rules`]: an email is wrong only when it is one of
//! the user's own identities and not the profile's — someone else's commits
//! are never offenders. Only the wrong fields change: a wrong author gets the
//! profile's name/email, a wrong committer the profile's identity and a new
//! date; every other field (a colleague's authorship, a correct committer and
//! its date) is copied exactly. Unpushed commits before the first offender keep
//! their hashes; pushed commits are never recreated, even when an unpushed
//! merge has a pushed parent (that parent is kept as is). A pushed commit can't
//! descend from an unpushed one, so the rewrite set never contains one.
//! Before anything is written the minimal recreated commits are checked again
//! with `for-each-ref --contains … refs/remotes/ refs/tags/` (Plan D6).
//!
//! # How the rewrite works — plumbing, not `git rebase`
//!
//! Each commit is rebuilt with `git commit-tree <same tree> -p <mapped parents>`,
//! oldest first, the raw message bytes fed on stdin and the identity passed in
//! `GIT_AUTHOR_*` / `GIT_COMMITTER_*` env vars. Then the branch is moved with one
//! compare-and-swap `git update-ref`. Why this instead of
//! `rebase --rebase-merges --exec 'git commit --amend …'`:
//!
//! - **Nothing to inject into.** Rebase `--exec` runs a shell string; here the
//!   profile name/email only ever travel in env vars, and every git call is an
//!   argument array. No `sh`, no quoting rules (Plan §9.2, CLAUDE.md rule 9).
//! - **Exact content.** Trees are reused by id, so files, submodule pointers and
//!   LFS pointers can't change, and merges keep their recorded tree — including
//!   conflict resolutions that `--rebase-merges` would have to redo (and could
//!   fail on). Messages are byte-for-byte (no cleanup, no editor); author dates
//!   are copied with their time zone (`GIT_AUTHOR_DATE=@<secs> <tz>`).
//! - **Precise.** Only the selected commits are recreated; a rebase would also
//!   re-pick pushed side-branch commits reachable through an unpushed merge.
//! - **No half-done state.** No sequencer, no conflicts, no editor
//!   (`GIT_EDITOR`/`GIT_SEQUENCE_EDITOR` are irrelevant), no hooks
//!   (`commit-tree` runs none), no index or work-tree writes. The repo only
//!   changes at the final `update-ref`, which is atomic and checks the old
//!   value, so a commit made meanwhile makes the fix fail instead of losing it.
//! - **Verified before the branch moves:** the new head's tree must equal the
//!   old head's tree, the new unpushed range must have the same number of
//!   commits and zero offenders.
//!
//! A backup ref `refs/githop/backup/<branch>/<UTC stamp>` is written first. On
//! any failure the branch is put back (if it moved) and the backup removed, so
//! the repo is exactly as before. After success the result is recorded in
//! `refs/githop/fixed/<branch>/<stamp>`; [`undo`] only restores a backup when
//! the branch still points at that result (so no later work can be lost).
//! The newest [`BACKUPS_KEPT`] backups per branch are kept.
//!
//! Known limits: commit signatures (`gpgsig`) and `mergetag` headers are
//! dropped from recreated commits ([`FixPlan::signed`] counts them so callers
//! can warn); remote-tracking refs are only as fresh as the last fetch; a push
//! that skipped GitHop's hook (`--no-verify`, before install) to a ref with no
//! remote-tracking ref and no upstream config can't be seen.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use time::macros::format_description;
use time::OffsetDateTime;

use crate::check::identity_rules::{find_offenders, IdentityRules};
use crate::check::push::{read_pushed, LogCommit, OffendingCommit, PushedEntry};
use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::gitconfig::ConfigSnapshot;
use crate::paths::{to_slash, Paths};
use crate::profile::Profile;
use crate::repo::Repo;
use crate::resolver;
use crate::store::Store;

/// Namespace of backup refs: `refs/githop/backup/<branch>/<stamp>`.
pub const BACKUP_PREFIX: &str = "refs/githop/backup/";
/// Namespace of fix results: `refs/githop/fixed/<branch>/<stamp>` (same stamp as the backup).
pub const RESULT_PREFIX: &str = "refs/githop/fixed/";
/// Backups kept per branch; older ones are pruned after each fix.
pub const BACKUPS_KEPT: usize = 10;
/// Most commits one fix recreates (each is one `git commit-tree` spawn).
pub const MAX_REWRITE: usize = 1000;
/// Most recent pushed commits scanned for [`FixPlan::pushed_offenders`].
pub const PUSHED_SCAN: usize = 200;

/// Why a fix can't run right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixBlocker {
    /// A rebase, merge, cherry-pick, revert, am or bisect is in progress.
    OperationInProgress,
    /// HEAD isn't on a branch.
    DetachedHead,
    /// No profile owns the repo.
    Unresolved,
    /// Staged or unstaged changes to tracked files.
    DirtyTree,
    /// More than [`MAX_REWRITE`] commits would be recreated.
    TooManyCommits,
    /// The branch has an upstream GitHop can't see (no remote-tracking ref, no
    /// recorded push), so it can't tell which commits are pushed.
    PushedUnknown,
    /// Shallow clone without a remote-tracking upstream for the branch.
    Shallow,
}

/// What `fix` would do. Serde shape per `app/IPC.md` (`FixPlan`), plus the
/// extra fields marked below (all additive).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixPlan {
    /// Repo root, forward slashes.
    pub repo: String,
    /// Current branch (short name); `None` when HEAD is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_email: Option<String>,
    /// Unpushed commits that use one of the user's other emails — these get
    /// re-stamped. Someone else's commits are never listed. Newest first.
    pub offenders: Vec<OffendingCommit>,
    /// Recent pushed commits on this branch that use one of the user's other
    /// emails. Never touched; shown so the user knows. Newest first.
    pub pushed_offenders: Vec<OffendingCommit>,
    /// Set when the fix can't run now: a full sentence with the next step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// Extra: machine-readable kind of `blocked_reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<FixBlocker>,
    /// Extra: unpushed commits checked on this branch (not reachable from a
    /// remote-tracking ref, a tag, a recorded push or an untracked upstream).
    #[serde(default)]
    pub unpushed: usize,
    /// Extra: commits that will get new hashes (offenders + unpushed commits after them).
    #[serde(default)]
    pub rewrite_count: usize,
    /// Extra: signed commits among those — their signatures will be dropped.
    #[serde(default)]
    pub signed: usize,
    /// Extra: the repo has remote-tracking refs. `false` = only tags and
    /// recorded pushes mark commits as pushed.
    #[serde(default)]
    pub has_remote_refs: bool,
    /// Extra: unpushed commits on this branch authored by someone else. Never
    /// re-stamped (they only get a new id when they sit on top of an offender).
    #[serde(default)]
    pub foreign: usize,
}

impl FixPlan {
    /// There is something to fix and nothing blocks it.
    pub fn can_apply(&self) -> bool {
        self.blocked_reason.is_none() && !self.offenders.is_empty()
    }
}

/// Outcome of [`apply`]. Serde shape per `app/IPC.md` (`FixResult`), plus extras.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixResult {
    /// Backup of the branch before the fix.
    pub backup_ref: String,
    /// Offending commits re-stamped ("2 commits fixed").
    pub rewritten: usize,
    /// Command that undoes this fix.
    pub undo_hint: String,
    /// Extra: commits recreated in total (offenders + later unpushed commits).
    #[serde(default)]
    pub recreated: usize,
    /// Extra: branch head before the fix.
    #[serde(default)]
    pub old_head: String,
    /// Extra: branch head after the fix.
    #[serde(default)]
    pub new_head: String,
    /// Extra: the new head fast-forwards from every last known remote value of
    /// the branch (its remote-tracking upstream and its last recorded push), or
    /// none is known — so a plain `git push` will be accepted.
    #[serde(default)]
    pub fast_forward: bool,
}

/// Test-only failure injection for [`apply_with_fault`].
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Fail after the first commit was recreated (branch not moved yet).
    DuringRewrite,
    /// Fail right after the branch was moved.
    AfterBranchUpdate,
}

/// Work out what `fix` would do in the repo containing `cwd`. Read-only.
///
/// Errors only for "can't even look" problems (not a repo, git missing, broken
/// GitHop data); repo-state problems are reported in `blocked_reason`.
pub fn plan(git: &Git, cwd: &Path, paths: &Paths) -> Result<FixPlan> {
    Ok(analyze(git, cwd, paths)?.plan)
}

/// Re-plan, then re-stamp the unpushed offending commits with the owning
/// profile's identity. Refuses (`CoreError::FixRefused`) when the plan is
/// blocked or there is nothing to fix. On any failure the repo is left exactly
/// as it was.
pub fn apply(git: &Git, cwd: &Path, paths: &Paths) -> Result<FixResult> {
    apply_with_fault(git, cwd, paths, None)
}

/// [`apply`] with an injected failure (tests only).
#[doc(hidden)]
pub fn apply_with_fault(
    git: &Git,
    cwd: &Path,
    paths: &Paths,
    fault: Option<Fault>,
) -> Result<FixResult> {
    let analysis = analyze(git, cwd, paths)?;
    let git = fix_git(git);
    let plan = &analysis.plan;
    if let Some(reason) = &plan.blocked_reason {
        return Err(CoreError::FixRefused(reason.clone()));
    }
    if plan.offenders.is_empty() {
        return Err(CoreError::FixRefused(NOTHING_TO_FIX.to_string()));
    }
    let (Some(branch_ref), Some(head), Some(profile), Some(rules)) = (
        analysis.state.branch_ref.as_deref(),
        analysis.state.head.as_deref(),
        analysis.profile.as_ref(),
        analysis.rules.as_ref(),
    ) else {
        return Err(CoreError::FixRefused(NOTHING_TO_FIX.to_string()));
    };
    let root = analysis.state.root.as_path();
    let offenders: HashMap<&str, &OffendingCommit> =
        plan.offenders.iter().map(|o| (o.sha.as_str(), o)).collect();
    let rewrite = rewrite_set(&analysis.commits, &offenders);

    // Rule 7 double-check: nothing we recreate may be on a remote or a tag.
    let minimal: Vec<&str> = analysis
        .commits
        .iter()
        .filter(|c| rewrite.contains(&c.sha) && !c.parents.iter().any(|p| rewrite.contains(p)))
        .map(|c| c.sha.as_str())
        .collect();
    if let Some(found) = remote_containing(&git, root, &minimal)? {
        let place = match found.strip_prefix("refs/tags/") {
            Some(tag) => format!("tag {tag}"),
            None => found
                .strip_prefix("refs/remotes/")
                .unwrap_or(&found)
                .to_string(),
        };
        return Err(CoreError::FixRefused(format!(
            "Some of these commits are already on {place}. GitHop never changes pushed commits, \
             so nothing was changed."
        )));
    }

    let branch = short_branch(branch_ref).to_string();
    let backup_ref = create_backup(&git, root, &branch, head)?;
    let job = Rewrite {
        git: &git,
        root,
        commits: &analysis.commits,
        rewrite: &rewrite,
        offenders: &offenders,
        profile,
        rules,
        negatives: &analysis.boundary.negatives,
        branch_ref,
        head,
        backup_ref: &backup_ref,
        expected_unpushed: plan.unpushed,
        fault,
    };
    let new_head = match job.run() {
        Ok(h) => h,
        Err(e) => return Err(rollback(&git, root, branch_ref, head, &backup_ref, e)),
    };

    // Record the result so `undo` can tell whether the branch moved on since.
    let stamp = backup_ref
        .rsplit_once('/')
        .map(|(_, s)| s.to_string())
        .unwrap_or_default();
    let result_ref = format!("{RESULT_PREFIX}{branch}/{stamp}");
    let _ = git.run(
        root,
        &[
            "update-ref",
            "-m",
            "githop fix: result",
            &result_ref,
            &new_head,
        ],
    );
    // Housekeeping only: a failure here must not turn a good fix into an error.
    let _ = prune(&git, root);
    let fast_forward = fast_forwards(&git, root, &analysis.boundary.head_known, &new_head);

    Ok(FixResult {
        undo_hint: format!("githop fix --undo {backup_ref}"),
        backup_ref,
        rewritten: plan.offenders.len(),
        recreated: rewrite.len(),
        old_head: head.to_string(),
        new_head,
        fast_forward,
    })
}

/// Every known remote value of the branch is an ancestor of `new_head`.
/// Unknown (git error) counts as "no".
fn fast_forwards(git: &Git, root: &Path, known: &[String], new_head: &str) -> bool {
    known.iter().all(|k| {
        git.output(root, &["merge-base", "--is-ancestor", k, new_head], None)
            .is_ok_and(|o| o.status.success())
    })
}

/// Restore `backup_ref` (a ref under [`BACKUP_PREFIX`]) on its branch.
///
/// Refuses (`CoreError::FixRefused`) unless: the ref is a GitHop backup that
/// exists, its branch is checked out, no git operation is in progress, the
/// tree is clean, and the branch still points at the commit that fix produced
/// (new commits since would be lost). Then `git reset --keep` to the backup and
/// delete the backup and result refs.
pub fn undo(git: &Git, cwd: &Path, backup_ref: &str) -> Result<()> {
    let git = fix_git(git);
    let not_backup = || {
        CoreError::FixRefused(format!(
            "\"{backup_ref}\" isn't a GitHop backup. Backups are named \
             refs/githop/backup/<branch>/<time>."
        ))
    };
    if !is_valid_ref_name(backup_ref) {
        return Err(not_backup());
    }
    let (branch, stamp) = parse_backup_ref(backup_ref).ok_or_else(not_backup)?;
    let state = inspect(&git, cwd)?;
    let root = state.root.as_path();
    let result_ref = format!("{RESULT_PREFIX}{branch}/{stamp}");
    let values = ref_values(&git, root, &[backup_ref, &result_ref])?;
    let Some(backup_sha) = values.get(backup_ref) else {
        return Err(CoreError::FixRefused(format!(
            "There's no backup called {backup_ref} in this repo."
        )));
    };
    if let Some(reason) = operation_in_progress(&state.git_dir) {
        return Err(CoreError::FixRefused(reason));
    }
    let want_ref = format!("refs/heads/{branch}");
    match state.branch_ref.as_deref() {
        None => {
            return Err(CoreError::FixRefused(format!(
                "HEAD isn't on a branch. This backup is for {branch}. Run: git switch {branch}"
            )))
        }
        Some(current) if current != want_ref => {
            return Err(CoreError::FixRefused(format!(
                "This backup is for {branch}, but {} is checked out. Run: git switch {branch}",
                short_branch(current)
            )))
        }
        Some(_) => {}
    }
    let manual = format!("To restore it by hand: git reset --keep {backup_ref}");
    let Some(result_sha) = values.get(&result_ref) else {
        return Err(CoreError::FixRefused(format!(
            "GitHop has no record of the fix that made this backup, so it can't check that \
             undo is safe. {manual}"
        )));
    };
    if state.head.as_deref() != Some(result_sha.as_str()) {
        return Err(CoreError::FixRefused(format!(
            "{branch} has changed since the fix (new commits or a reset). Undo would lose that \
             work, so GitHop won't do it. {manual}"
        )));
    }
    if is_dirty(&git, root)? {
        return Err(CoreError::FixRefused(DIRTY.to_string()));
    }
    git.clone()
        .env("GIT_REFLOG_ACTION", "githop fix --undo")
        .run(root, &["reset", "-q", "--keep", backup_sha])?;
    let now = rev_parse_head(&git, root)?;
    if now.as_deref() != Some(backup_sha.as_str()) {
        return Err(CoreError::FixRefused(format!(
            "Undo didn't finish: {branch} isn't at the backup. {manual}"
        )));
    }
    let script = format!("delete {backup_ref} {backup_sha}\ndelete {result_ref} {result_sha}\n");
    // The branch is restored; leftover refs are harmless.
    let _ = run_piped(&git, root, &["update-ref", "--stdin"], script.into_bytes());
    Ok(())
}

/// All GitHop backup refs in the repo containing `cwd`, newest first.
pub fn list_backups(git: &Git, cwd: &Path) -> Result<Vec<String>> {
    let git = fix_git(git);
    let repo = Repo::discover(&git, cwd)?;
    let out = git.run_text(
        repo.root(),
        &["for-each-ref", "--format=%(refname)", BACKUP_PREFIX],
    )?;
    let mut refs: Vec<(String, String)> = out
        .lines()
        .map(str::trim)
        .filter_map(|r| parse_backup_ref(r).map(|(_, stamp)| (stamp, r.to_string())))
        .collect();
    refs.sort_by(|a, b| b.cmp(a));
    Ok(refs.into_iter().map(|(_, r)| r).collect())
}

/// Newest backup for the branch currently checked out, if any.
pub fn latest_backup(git: &Git, cwd: &Path) -> Result<Option<String>> {
    let state = inspect(&fix_git(git), cwd)?;
    let Some(branch_ref) = state.branch_ref.as_deref() else {
        return Ok(None);
    };
    let branch = short_branch(branch_ref);
    Ok(list_backups(git, cwd)?
        .into_iter()
        .find(|r| parse_backup_ref(r).is_some_and(|(b, _)| b == branch)))
}

/// Split `refs/githop/backup/<branch>/<stamp>` into `(branch, stamp)`.
/// The stamp is `YYYYMMDDTHHMMSSZ`, optionally followed by `-NN`.
pub fn parse_backup_ref(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix(BACKUP_PREFIX)?;
    let (branch, stamp) = rest.rsplit_once('/')?;
    (!branch.is_empty() && is_stamp(stamp)).then(|| (branch.to_string(), stamp.to_string()))
}

// ------------------------------------------------------------------ messages

const NOTHING_TO_FIX: &str = "Nothing to fix. All unpushed commits use the right email.";
const DIRTY: &str =
    "This repo has uncommitted changes. Commit or stash your changes first, then try again.";

// ------------------------------------------------------------------ analysis

/// What we learned about the repo; `plan` is the public part.
struct Analysis {
    plan: FixPlan,
    state: State,
    profile: Option<Profile>,
    /// Which emails are wrong (`None` when no profile owns the repo).
    rules: Option<IdentityRules>,
    /// Unpushed commits on HEAD, parents before children.
    commits: Vec<RawCommit>,
    /// What counts as pushed besides remote-tracking refs and tags.
    boundary: Boundary,
}

/// Where HEAD is.
struct State {
    root: PathBuf,
    /// Per-worktree git dir (where `rebase-merge/`, `MERGE_HEAD`… live).
    git_dir: PathBuf,
    /// Common git dir (shared by worktrees: `shallow`, `githop/pushed`).
    common_dir: PathBuf,
    /// `refs/heads/<branch>`; `None` when detached.
    branch_ref: Option<String>,
    /// HEAD commit; `None` on an unborn branch.
    head: Option<String>,
}

/// Pushed commits that `--remotes --tags` can't see.
#[derive(Debug, Default)]
struct Boundary {
    /// Extra "pushed" commits, fed to `rev-list` as `^<sha>` on stdin: recorded
    /// pushes that still exist locally, and tips of other branches whose
    /// upstream has no remote-tracking ref and no recorded push.
    negatives: Vec<String>,
    /// The current branch's upstream can't be seen: the reason to refuse.
    head_unknown: Option<String>,
    /// The current branch has an existing remote-tracking upstream.
    head_tracked: bool,
    /// Last known remote values of the current branch (tracking ref name,
    /// last recorded sha) for [`FixResult::fast_forward`].
    head_known: Vec<String>,
}

fn analyze(git: &Git, cwd: &Path, paths: &Paths) -> Result<Analysis> {
    let git = fix_git(git);
    let state = inspect(&git, cwd)?;
    let root = state.root.clone();
    let store = Store::open_at(paths.clone())?;
    let snapshot = ConfigSnapshot::read(&git, &root)?;
    let resolution = resolver::resolve(
        &snapshot,
        &root,
        store.profiles(),
        &store.settings().github_host_aliases,
    );
    let profile = resolution
        .profile_id
        .as_deref()
        .and_then(|id| store.profiles().get(id))
        .cloned();
    let rules = profile
        .as_ref()
        .map(|p| IdentityRules::for_repo(&git, paths, store.profiles(), p, &snapshot));

    let mut plan = FixPlan {
        repo: to_slash(&root),
        branch: state
            .branch_ref
            .as_deref()
            .map(|b| short_branch(b).to_string()),
        profile_id: profile.as_ref().map(|p| p.id.clone()),
        profile_email: profile.as_ref().map(|p| p.email.clone()),
        offenders: Vec::new(),
        pushed_offenders: Vec::new(),
        blocked_reason: None,
        blocker: None,
        unpushed: 0,
        rewrite_count: 0,
        signed: 0,
        has_remote_refs: false,
        foreign: 0,
    };

    let mut blocker: Option<(FixBlocker, String)> = None;
    if let Some(reason) = operation_in_progress(&state.git_dir) {
        blocker = Some((FixBlocker::OperationInProgress, reason));
    } else if state.branch_ref.is_none() {
        blocker = Some((
            FixBlocker::DetachedHead,
            "HEAD isn't on a branch (detached HEAD). Check out the branch to fix, then try again."
                .to_string(),
        ));
    } else if profile.is_none() {
        blocker = Some((
            FixBlocker::Unresolved,
            "GitHop doesn't know which profile owns this repo. Pin this repo first: \
             githop pin <profile>"
                .to_string(),
        ));
    }

    let mut commits = Vec::new();
    let mut boundary = Boundary::default();
    if let (Some(head), Some(rules)) = (state.head.as_deref(), rules.as_ref()) {
        plan.has_remote_refs = has_remote_refs(&git, &root)?;
        boundary = pushed_boundary(&git, &state, &snapshot)?;
        let shas = unpushed_list(&git, &root, head, &boundary.negatives)?;
        commits = read_commits(&git, &root, &shas)?;
        plan.unpushed = commits.len();
        let log: Vec<LogCommit> = commits.iter().map(RawCommit::log_commit).collect();
        plan.foreign = log
            .iter()
            .filter(|c| rules.is_foreign(&c.author_email))
            .count();
        let mut offenders = find_offenders(&log, rules);
        offenders.reverse(); // newest first, like `git log`
        let by_sha: HashMap<&str, &OffendingCommit> =
            offenders.iter().map(|o| (o.sha.as_str(), o)).collect();
        let rewrite = rewrite_set(&commits, &by_sha);
        plan.rewrite_count = rewrite.len();
        plan.signed = commits
            .iter()
            .filter(|c| c.signed && rewrite.contains(&c.sha))
            .count();
        plan.offenders = offenders;
        plan.pushed_offenders = pushed_offenders(&git, &root, head, &commits, rules)?;
    }

    if blocker.is_none() && !plan.offenders.is_empty() {
        if let Some(reason) = boundary.head_unknown.clone() {
            blocker = Some((FixBlocker::PushedUnknown, reason));
        } else if state.common_dir.join("shallow").exists() && !boundary.head_tracked {
            blocker = Some((
                FixBlocker::Shallow,
                "This repo is a shallow clone, so GitHop can't confirm these commits were never \
                 pushed. Nothing was changed. Run: git fetch --unshallow, then try again."
                    .to_string(),
            ));
        } else if is_dirty(&git, &root)? {
            blocker = Some((FixBlocker::DirtyTree, DIRTY.to_string()));
        } else if plan.rewrite_count > MAX_REWRITE {
            blocker = Some((
                FixBlocker::TooManyCommits,
                format!(
                    "{} commits would need new hashes. GitHop fixes up to {MAX_REWRITE} at a \
                     time, so nothing was changed.",
                    plan.rewrite_count
                ),
            ));
        }
    }
    if let Some((kind, reason)) = blocker {
        plan.blocker = Some(kind);
        plan.blocked_reason = Some(reason);
    }
    Ok(Analysis {
        plan,
        state,
        profile,
        rules,
        commits,
        boundary,
    })
}

/// Commits on `tip` that aren't pushed, parents first: `rev-list --topo-order
/// --reverse <tip> --not --remotes --tags`, plus `^<sha>` for each of
/// `negatives` on stdin (no command-line length limit). One spawn.
fn unpushed_list(git: &Git, root: &Path, tip: &str, negatives: &[String]) -> Result<Vec<String>> {
    let input: String = negatives.iter().map(|n| format!("^{n}\n")).collect();
    let out = run_piped(
        git,
        root,
        &[
            "rev-list",
            "--topo-order",
            "--reverse",
            "--stdin",
            tip,
            "--not",
            "--remotes",
            "--tags",
        ],
        input.into_bytes(),
    )?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Work out the pushed commits `--remotes --tags` doesn't cover (see the
/// module docs). Spawns: one `for-each-ref`, plus one `cat-file --batch-check`
/// when the pushed record isn't empty.
fn pushed_boundary(git: &Git, state: &State, snapshot: &ConfigSnapshot) -> Result<Boundary> {
    let root = state.root.as_path();
    let recorded = read_pushed(&state.common_dir)
        .map_err(|e| CoreError::io(crate::check::push::pushed_record_path(&state.common_dir), e))?;
    let mut b = Boundary {
        negatives: existing_objects(git, root, &recorded)?,
        ..Boundary::default()
    };
    let out = git.run_text(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream)%00%(upstream:track)",
            "refs/heads/",
        ],
    )?;
    let head_branch = state.branch_ref.as_deref();
    for line in out.lines() {
        let mut f = line.split('\0');
        let (Some(refname), Some(tip), Some(upstream), track) =
            (f.next(), f.next(), f.next(), f.next().unwrap_or_default())
        else {
            continue;
        };
        let branch = short_branch(refname);
        let remote = snapshot.get(&format!("branch.{branch}.remote"));
        let merge = snapshot.get(&format!("branch.{branch}.merge"));
        let (Some(remote), Some(merge)) = (remote, merge) else {
            continue; // no upstream: only the record can tell
        };
        if remote == "." || remote.is_empty() || merge.is_empty() {
            continue; // upstream is a local branch
        }
        let is_head = head_branch == Some(refname);
        let gone = track.contains("gone");
        let last_recorded = recorded
            .iter()
            .rev()
            .find(|e| e.remote == remote && e.remote_ref == merge);
        if is_head {
            if !upstream.is_empty() && !gone {
                b.head_tracked = true;
                b.head_known.push(upstream.to_string());
            }
            if let Some(e) = last_recorded {
                b.head_known.push(e.sha.clone());
            }
        }
        if (!upstream.is_empty() && !gone) || last_recorded.is_some() {
            continue; // covered by --remotes or by the recorded shas
        }
        if is_head {
            b.head_unknown = Some(unknown_upstream_reason(
                branch,
                remote,
                merge,
                gone.then_some(upstream),
                snapshot,
            ));
        } else if !b.negatives.iter().any(|n| n == tip) {
            b.negatives.push(tip.to_string());
        }
    }
    Ok(b)
}

/// Why Fix refuses when the current branch's upstream can't be seen.
fn unknown_upstream_reason(
    branch: &str,
    remote: &str,
    merge: &str,
    gone: Option<&str>,
    snapshot: &ConfigSnapshot,
) -> String {
    let short = merge.strip_prefix("refs/heads/").unwrap_or(merge);
    let named = snapshot.remotes().iter().any(|r| r == remote);
    match gone {
        Some(tracking) => format!(
            "{branch} was pushed to {remote}, but {} is gone, so GitHop can't tell which commits \
             are already pushed. Nothing was changed. Run: git fetch {remote}, then try again.",
            tracking.strip_prefix("refs/remotes/").unwrap_or(tracking)
        ),
        None if named => format!(
            "{branch} was pushed to {remote}, but this clone doesn't track {remote}/{short}, so \
             GitHop can't tell which commits are already pushed. Nothing was changed. \
             Run: git remote set-branches --add {remote} {short}, then git fetch {remote}, \
             then try again."
        ),
        None => format!(
            "{branch} was pushed to {remote}, and GitHop can't tell which of its commits are \
             already there. Nothing was changed."
        ),
    }
}

/// The recorded shas that exist in this repo (a pushed branch may have been
/// deleted and its commits pruned), de-duplicated. One spawn when non-empty.
fn existing_objects(git: &Git, root: &Path, recorded: &[PushedEntry]) -> Result<Vec<String>> {
    let mut shas: Vec<&str> = Vec::new();
    for e in recorded {
        if !shas.contains(&e.sha.as_str()) {
            shas.push(&e.sha);
        }
    }
    if shas.is_empty() {
        return Ok(Vec::new());
    }
    let input: String = shas.iter().map(|s| format!("{s}\n")).collect();
    let out = run_piped(
        git,
        root,
        &["cat-file", "--batch-check=%(objectname) %(objecttype)"],
        input.into_bytes(),
    )?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|l| {
            let (sha, kind) = l.trim().split_once(' ')?;
            (matches!(kind, "commit" | "tag") && is_hex(sha)).then(|| sha.to_string())
        })
        .collect())
}

/// Recent pushed commits on HEAD that use one of the user's other emails.
fn pushed_offenders(
    git: &Git,
    root: &Path,
    head: &str,
    unpushed: &[RawCommit],
    rules: &IdentityRules,
) -> Result<Vec<OffendingCommit>> {
    let count = format!("--max-count={}", unpushed.len() + PUSHED_SCAN);
    let recent = rev_list(git, root, &["rev-list", &count, head])?;
    let skip: HashSet<&str> = unpushed.iter().map(|c| c.sha.as_str()).collect();
    let pushed: Vec<String> = recent
        .into_iter()
        .filter(|s| !skip.contains(s.as_str()))
        .take(PUSHED_SCAN)
        .collect();
    let commits = read_commits(git, root, &pushed)?;
    let log: Vec<LogCommit> = commits.iter().map(RawCommit::log_commit).collect();
    Ok(find_offenders(&log, rules))
}

/// Commits to recreate: offenders and every unpushed descendant of one.
/// `commits` must list parents before children.
fn rewrite_set(
    commits: &[RawCommit],
    offenders: &HashMap<&str, &OffendingCommit>,
) -> HashSet<String> {
    let mut set = HashSet::new();
    for c in commits {
        if offenders.contains_key(c.sha.as_str()) || c.parents.iter().any(|p| set.contains(p)) {
            set.insert(c.sha.clone());
        }
    }
    set
}

// ------------------------------------------------------------------- rewrite

struct Rewrite<'a> {
    git: &'a Git,
    root: &'a Path,
    commits: &'a [RawCommit],
    rewrite: &'a HashSet<String>,
    offenders: &'a HashMap<&'a str, &'a OffendingCommit>,
    profile: &'a Profile,
    rules: &'a IdentityRules,
    /// Extra pushed commits (see [`Boundary::negatives`]).
    negatives: &'a [String],
    branch_ref: &'a str,
    head: &'a str,
    backup_ref: &'a str,
    expected_unpushed: usize,
    fault: Option<Fault>,
}

impl Rewrite<'_> {
    /// Recreate, verify, move the branch. Returns the new head.
    fn run(&self) -> Result<String> {
        let mut map: HashMap<String, String> = HashMap::new();
        for c in self
            .commits
            .iter()
            .filter(|c| self.rewrite.contains(&c.sha))
        {
            let new = self.write_commit(c, &map)?;
            map.insert(c.sha.clone(), new);
            if self.fault == Some(Fault::DuringRewrite) {
                return Err(injected());
            }
        }
        let new_head = map
            .get(self.head)
            .cloned()
            .ok_or_else(|| verify_failed("the branch head wasn't recreated"))?;
        self.verify(&new_head)?;

        let msg = format!(
            "githop fix: re-stamped {} commit(s), backup {}",
            map.len(),
            self.backup_ref
        );
        self.git.run(
            self.root,
            &[
                "update-ref",
                "-m",
                &msg,
                self.branch_ref,
                &new_head,
                self.head,
            ],
        )?;
        if self.fault == Some(Fault::AfterBranchUpdate) {
            return Err(injected());
        }
        if rev_parse_head(self.git, self.root)?.as_deref() != Some(new_head.as_str()) {
            return Err(verify_failed("HEAD isn't at the fixed commits"));
        }
        Ok(new_head)
    }

    /// `git commit-tree` with the same tree and message; identity via env only.
    /// Only the wrong fields change: a wrong author → the profile's name and
    /// email (author date kept); a wrong committer → the profile's identity
    /// and the current date. Anything else (someone else's authorship, a right
    /// committer and its date) is copied exactly.
    fn write_commit(&self, c: &RawCommit, map: &HashMap<String, String>) -> Result<String> {
        let p = self.profile;
        let off = self.offenders.get(c.sha.as_str());
        let author_wrong = off.is_some_and(|o| o.author_mismatch);
        let committer_wrong = off.is_some_and(|o| o.committer_mismatch);
        let (author_name, author_email) = if author_wrong {
            (p.name.as_str(), p.email.as_str())
        } else {
            (c.author.name.as_str(), c.author.email.as_str())
        };
        let mut g = self
            .git
            .clone()
            .env("GIT_AUTHOR_NAME", author_name)
            .env("GIT_AUTHOR_EMAIL", author_email);
        g = match &c.author.date {
            Some(d) => g.env("GIT_AUTHOR_DATE", d),
            None => g.env_remove("GIT_AUTHOR_DATE"),
        };
        g = if committer_wrong {
            g.env("GIT_COMMITTER_NAME", &p.name)
                .env("GIT_COMMITTER_EMAIL", &p.email)
                .env_remove("GIT_COMMITTER_DATE")
        } else {
            let g = g
                .env("GIT_COMMITTER_NAME", &c.committer.name)
                .env("GIT_COMMITTER_EMAIL", &c.committer.email);
            match &c.committer.date {
                Some(d) => g.env("GIT_COMMITTER_DATE", d),
                None => g.env_remove("GIT_COMMITTER_DATE"),
            }
        };
        let mut args: Vec<String> = Vec::new();
        if let Some(enc) = &c.encoding {
            args.push("-c".into());
            args.push(format!("i18n.commitEncoding={enc}"));
        }
        args.extend(["commit-tree".into(), "--no-gpg-sign".into(), c.tree.clone()]);
        for parent in &c.parents {
            args.push("-p".into());
            args.push(map.get(parent).unwrap_or(parent).clone());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run_piped(&g, self.root, &refs, c.message.clone())?;
        let sha = String::from_utf8_lossy(&out).trim().to_string();
        if !is_hex(&sha) {
            return Err(verify_failed("git commit-tree printed no commit id"));
        }
        Ok(sha)
    }

    /// Same tree, same number of unpushed commits, zero offenders.
    fn verify(&self, new_head: &str) -> Result<()> {
        let old_tree = format!("{}^{{tree}}", self.head);
        let new_tree = format!("{new_head}^{{tree}}");
        let trees = self
            .git
            .run_text(self.root, &["rev-parse", &old_tree, &new_tree])?;
        let trees: Vec<&str> = trees.lines().map(str::trim).collect();
        if trees.len() != 2 || trees[0] != trees[1] {
            return Err(verify_failed("the files would change"));
        }
        let shas = unpushed_list(self.git, self.root, new_head, self.negatives)?;
        if shas.len() != self.expected_unpushed {
            return Err(verify_failed("the number of commits would change"));
        }
        let commits = read_commits(self.git, self.root, &shas)?;
        let log: Vec<LogCommit> = commits.iter().map(RawCommit::log_commit).collect();
        if !find_offenders(&log, self.rules).is_empty() {
            return Err(verify_failed("some commits still use the wrong email"));
        }
        Ok(())
    }
}

fn injected() -> CoreError {
    CoreError::FixRefused("Injected failure (test).".to_string())
}

fn verify_failed(what: &str) -> CoreError {
    CoreError::FixRefused(format!(
        "GitHop stopped the fix because {what}. Nothing was changed."
    ))
}

/// Put the branch back at `head` if it moved, then drop the backup.
/// Returns the error to report.
fn rollback(
    git: &Git,
    root: &Path,
    branch_ref: &str,
    head: &str,
    backup_ref: &str,
    err: CoreError,
) -> CoreError {
    let restored = (|| -> Result<bool> {
        let current = git.run_text(root, &["rev-parse", "--verify", "-q", branch_ref])?;
        if current.trim() != head {
            git.run(
                root,
                &[
                    "update-ref",
                    "-m",
                    "githop fix: rolled back",
                    branch_ref,
                    head,
                ],
            )?;
        }
        let current = git.run_text(root, &["rev-parse", "--verify", "-q", branch_ref])?;
        Ok(current.trim() == head)
    })();
    match restored {
        Ok(true) => {
            let _ = git.run(root, &["update-ref", "-d", backup_ref, head]);
            err
        }
        _ => CoreError::FixRefused(format!(
            "{err} GitHop couldn't put {} back by itself. Your original commits are saved at \
             {backup_ref}. Restore them with: git reset --keep {backup_ref}",
            short_branch(branch_ref)
        )),
    }
}

/// `refs/githop/backup/<branch>/<stamp>` → HEAD, never overwriting an existing ref.
fn create_backup(git: &Git, root: &Path, branch: &str, head: &str) -> Result<String> {
    let stamp = timestamp();
    let mut last_err = None;
    for n in 0..100 {
        let name = if n == 0 {
            format!("{BACKUP_PREFIX}{branch}/{stamp}")
        } else {
            format!("{BACKUP_PREFIX}{branch}/{stamp}-{n:02}")
        };
        // Empty old value = the ref must not exist yet.
        let args = ["update-ref", "-m", "githop fix: backup", &name, head, ""];
        let out = git.output(root, &args, None)?;
        if out.status.success() {
            return Ok(name);
        }
        let exists = git
            .output(root, &["rev-parse", "--verify", "-q", &name], None)?
            .status
            .success();
        if !exists {
            return Err(crate::gitconfig::git_failed(&args, &out));
        }
        last_err = Some(name);
    }
    Err(CoreError::BackupFailed(format!(
        "couldn't pick a free backup name (last tried {})",
        last_err.unwrap_or_default()
    )))
}

/// UTC `YYYYMMDDTHHMMSSZ`.
fn timestamp() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(format_description!(
        "[year][month][day]T[hour][minute][second]Z"
    ))
    .unwrap_or_else(|_| format!("{}", now.unix_timestamp()))
}

fn is_stamp(s: &str) -> bool {
    let (base, suffix) = match s.split_once('-') {
        Some((b, n)) => (b, Some(n)),
        None => (s, None),
    };
    let b = base.as_bytes();
    let base_ok = b.len() == 16
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'T'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'Z';
    let suffix_ok = suffix.is_none_or(|n| n.len() == 2 && n.bytes().all(|c| c.is_ascii_digit()));
    base_ok && suffix_ok
}

/// Keep the newest [`BACKUPS_KEPT`] backups per branch; drop older ones and
/// result refs whose backup is gone.
fn prune(git: &Git, root: &Path) -> Result<()> {
    let out = git.run_text(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)",
            BACKUP_PREFIX,
            RESULT_PREFIX,
        ],
    )?;
    let mut by_branch: HashMap<String, Vec<String>> = HashMap::new();
    let mut results = Vec::new();
    for r in out.lines().map(str::trim).filter(|r| !r.is_empty()) {
        if let Some((branch, stamp)) = parse_backup_ref(r) {
            by_branch.entry(branch).or_default().push(stamp);
        } else if r.starts_with(RESULT_PREFIX) {
            results.push(r.to_string());
        }
    }
    let mut kept: HashSet<String> = HashSet::new();
    let mut delete = Vec::new();
    for (branch, mut stamps) in by_branch {
        stamps.sort_by(|a, b| b.cmp(a));
        for (i, stamp) in stamps.into_iter().enumerate() {
            let key = format!("{branch}/{stamp}");
            if i < BACKUPS_KEPT {
                kept.insert(key);
            } else {
                delete.push(format!("{BACKUP_PREFIX}{key}"));
            }
        }
    }
    for r in results {
        let key = r.strip_prefix(RESULT_PREFIX).unwrap_or_default();
        if !kept.contains(key) {
            delete.push(r);
        }
    }
    if delete.is_empty() {
        return Ok(());
    }
    let script: String = delete.iter().map(|r| format!("delete {r}\n")).collect();
    run_piped(git, root, &["update-ref", "--stdin"], script.into_bytes())?;
    Ok(())
}

// ----------------------------------------------------------------- git state

/// Git handle for fix: replace refs off so we read (and rewrite) real objects.
fn fix_git(git: &Git) -> Git {
    git.clone().env("GIT_NO_REPLACE_OBJECTS", "1")
}

fn inspect(git: &Git, cwd: &Path) -> Result<State> {
    let repo = Repo::discover(git, cwd)?;
    let root = repo.root().to_path_buf();
    let common_dir = repo.common_dir().to_path_buf();
    let git_dir = git.run_text(&root, &["rev-parse", "--absolute-git-dir"])?;
    let git_dir = PathBuf::from(git_dir.trim());
    let args = ["symbolic-ref", "-q", "HEAD"];
    let out = git.output(&root, &args, None)?;
    let branch_ref = match out.status.code() {
        Some(0) => Some(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        Some(1) => None,
        _ => return Err(crate::gitconfig::git_failed(&args, &out)),
    };
    // Only local branches can be fixed (HEAD → refs/heads/…).
    let branch_ref = branch_ref.filter(|b| b.starts_with("refs/heads/"));
    let head = rev_parse_head(git, &root)?;
    Ok(State {
        root,
        git_dir,
        common_dir,
        branch_ref,
        head,
    })
}

fn rev_parse_head(git: &Git, root: &Path) -> Result<Option<String>> {
    let args = ["rev-parse", "-q", "--verify", "HEAD^{commit}"];
    let out = git.output(root, &args, None)?;
    match out.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )),
        Some(1) => Ok(None),
        _ => Err(crate::gitconfig::git_failed(&args, &out)),
    }
}

/// A sentence naming the operation in progress and how to end it.
fn operation_in_progress(git_dir: &Path) -> Option<String> {
    const OPS: [(&str, &str, &str); 7] = [
        ("rebase-merge", "A rebase", "git rebase --abort"),
        (
            "rebase-apply",
            "A rebase or git am",
            "git rebase --abort or git am --abort",
        ),
        ("MERGE_HEAD", "A merge", "git merge --abort"),
        (
            "CHERRY_PICK_HEAD",
            "A cherry-pick",
            "git cherry-pick --abort",
        ),
        ("REVERT_HEAD", "A revert", "git revert --abort"),
        (
            "sequencer",
            "A cherry-pick or revert",
            "git cherry-pick --abort",
        ),
        ("BISECT_LOG", "A bisect", "git bisect reset"),
    ];
    OPS.iter()
        .find(|(file, _, _)| git_dir.join(file).exists())
        .map(|(_, what, how)| {
            format!("{what} is in progress. Finish it or stop it ({how}), then try again.")
        })
}

/// Staged or unstaged changes to tracked files (untracked files don't matter:
/// fix never touches the work tree).
fn is_dirty(git: &Git, root: &Path) -> Result<bool> {
    let out = git.run(
        root,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=no",
        ],
    )?;
    Ok(!out.is_empty())
}

fn has_remote_refs(git: &Git, root: &Path) -> Result<bool> {
    let out = git.run_text(
        root,
        &[
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            "refs/remotes/",
        ],
    )?;
    Ok(!out.trim().is_empty())
}

/// First remote-tracking ref or tag containing any of `commits`.
fn remote_containing(git: &Git, root: &Path, commits: &[&str]) -> Result<Option<String>> {
    if commits.is_empty() {
        return Ok(None);
    }
    let mut args = vec!["for-each-ref", "--format=%(refname)"];
    for c in commits {
        args.push("--contains");
        args.push(c);
    }
    args.push("refs/remotes/");
    args.push("refs/tags/");
    let out = git.run_text(root, &args)?;
    Ok(out
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string))
}

/// Exact-name lookup of refs → object ids (missing refs are absent).
fn ref_values(git: &Git, root: &Path, names: &[&str]) -> Result<HashMap<String, String>> {
    let mut args = vec!["for-each-ref", "--format=%(objectname) %(refname)"];
    args.extend_from_slice(names);
    let out = git.run_text(root, &args)?;
    Ok(out
        .lines()
        .filter_map(|l| l.trim().split_once(' '))
        .filter(|(_, r)| names.contains(r))
        .map(|(sha, r)| (r.to_string(), sha.to_string()))
        .collect())
}

fn rev_list(git: &Git, root: &Path, args: &[&str]) -> Result<Vec<String>> {
    Ok(git
        .run_text(root, args)?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

fn short_branch(branch_ref: &str) -> &str {
    branch_ref.strip_prefix("refs/heads/").unwrap_or(branch_ref)
}

/// A ref name we are willing to pass to git: the subset of
/// `git check-ref-format` rules that matter for safety.
fn is_valid_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name.contains("//")
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
        && name
            .split('/')
            .all(|seg| !seg.starts_with('.') && !seg.ends_with(".lock"))
}

fn is_hex(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Run git with `input` on stdin, written from a thread so large inputs and
/// outputs can't deadlock on full pipes. Returns stdout on exit 0.
fn run_piped(git: &Git, root: &Path, args: &[&str], input: Vec<u8>) -> Result<Vec<u8>> {
    let mut cmd = git.command(root, args);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CoreError::GitNotFound
        } else {
            CoreError::io(git.exe(), e)
        }
    })?;
    let stdin = child.stdin.take();
    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        if let Some(mut pipe) = stdin {
            pipe.write_all(&input)?;
        }
        Ok(())
    });
    let out = child
        .wait_with_output()
        .map_err(|e| CoreError::io(git.exe(), e))?;
    let written = writer.join();
    if !out.status.success() {
        return Err(CoreError::GitFailed {
            args: args.join(" "),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    match written {
        Ok(Ok(())) => Ok(out.stdout),
        Ok(Err(e)) => Err(CoreError::io(git.exe(), e)),
        Err(_) => Err(CoreError::GitFailed {
            args: args.join(" "),
            code: None,
            stderr: "couldn't write to git".to_string(),
        }),
    }
}

// --------------------------------------------------------------- raw commits

/// `Name <email> <secs> <tz>` from a commit header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Ident {
    name: String,
    email: String,
    /// `@<secs> <tz>` — the exact form `GIT_AUTHOR_DATE` accepts.
    date: Option<String>,
}

fn parse_ident(s: &str) -> Ident {
    let Some(lt) = s.find('<') else {
        return Ident {
            name: s.trim().to_string(),
            ..Ident::default()
        };
    };
    let after = &s[lt + 1..];
    let Some(gt) = after.find('>') else {
        return Ident {
            name: s[..lt].trim().to_string(),
            email: after.trim().to_string(),
            date: None,
        };
    };
    let date = after[gt + 1..].trim();
    Ident {
        name: s[..lt].trim().to_string(),
        email: after[..gt].to_string(),
        date: (!date.is_empty()).then(|| format!("@{date}")),
    }
}

/// One commit object, as stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RawCommit {
    sha: String,
    tree: String,
    parents: Vec<String>,
    author: Ident,
    committer: Ident,
    encoding: Option<String>,
    signed: bool,
    /// Everything after the blank line, byte-for-byte.
    message: Vec<u8>,
}

impl RawCommit {
    fn parse(sha: &str, raw: &[u8]) -> Self {
        let split = raw.windows(2).position(|w| w == b"\n\n");
        let (headers, message) = match split {
            Some(i) => (&raw[..i], raw[i + 2..].to_vec()),
            None => (raw, Vec::new()),
        };
        let mut c = RawCommit {
            sha: sha.to_string(),
            message,
            ..RawCommit::default()
        };
        for line in String::from_utf8_lossy(headers).split('\n') {
            if line.starts_with(' ') {
                continue; // continuation of a multi-line header (gpgsig, mergetag)
            }
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "tree" => c.tree = value.trim().to_string(),
                "parent" => c.parents.push(value.trim().to_string()),
                "author" => c.author = parse_ident(value),
                "committer" => c.committer = parse_ident(value),
                "encoding" => c.encoding = Some(value.trim().to_string()),
                "gpgsig" | "gpgsig-sha256" => c.signed = true,
                _ => {}
            }
        }
        c
    }

    /// git's `%s`: the first paragraph of the message on one line.
    fn subject(&self) -> String {
        String::from_utf8_lossy(&self.message)
            .lines()
            .map(str::trim)
            .skip_while(|l| l.is_empty())
            .take_while(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn log_commit(&self) -> LogCommit {
        LogCommit {
            sha: self.sha.clone(),
            author_email: self.author.email.clone(),
            committer_email: self.committer.email.clone(),
            subject: self.subject(),
        }
    }
}

/// `git cat-file --batch` for `shas`, in order. One spawn.
fn read_commits(git: &Git, root: &Path, shas: &[String]) -> Result<Vec<RawCommit>> {
    if shas.is_empty() {
        return Ok(Vec::new());
    }
    let input: String = shas.iter().map(|s| format!("{s}\n")).collect();
    let out = run_piped(git, root, &["cat-file", "--batch"], input.into_bytes())?;
    let objects = parse_batch(&out).ok_or_else(|| CoreError::GitFailed {
        args: "cat-file --batch".to_string(),
        code: None,
        stderr: "unexpected output".to_string(),
    })?;
    if objects.len() != shas.len() {
        return Err(CoreError::GitFailed {
            args: "cat-file --batch".to_string(),
            code: None,
            stderr: format!("expected {} commits, got {}", shas.len(), objects.len()),
        });
    }
    Ok(objects
        .iter()
        .map(|(sha, raw)| RawCommit::parse(sha, raw))
        .collect())
}

/// Parse `<oid> commit <size>\n<content>\n` records. Any other type, a
/// missing object or a short read → `None`.
fn parse_batch(out: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut pos = 0;
    let mut objects = Vec::new();
    while pos < out.len() {
        let nl = out[pos..].iter().position(|&b| b == b'\n')? + pos;
        let header = String::from_utf8_lossy(&out[pos..nl]).to_string();
        let mut parts = header.split(' ');
        let (Some(sha), Some("commit"), Some(size)) = (parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        let size: usize = size.trim().parse().ok()?;
        let start = nl + 1;
        let end = start.checked_add(size)?;
        if end > out.len() {
            return None;
        }
        objects.push((sha.to_string(), out[start..end].to_vec()));
        pos = end + 1; // trailing LF
    }
    Some(objects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ident_parsing() {
        let i = parse_ident("Harshal \"O'B\" <me@x.com> 1700000000 +0530");
        assert_eq!(i.name, "Harshal \"O'B\"");
        assert_eq!(i.email, "me@x.com");
        assert_eq!(i.date.as_deref(), Some("@1700000000 +0530"));
        let i = parse_ident(" <e@x> ");
        assert_eq!(i.name, "");
        assert_eq!(i.date, None);
        assert_eq!(parse_ident("no email").name, "no email");
    }

    #[test]
    fn raw_commit_parsing_keeps_message_bytes() {
        let raw = b"tree aaaa\nparent p1\nparent p2\nauthor A <a@x> 1 +0000\n\
committer C <c@x> 2 -0100\nencoding ISO-8859-1\ngpgsig -----BEGIN-----\n \n -----END-----\n\n\
Subject line\n  more\n\nbody \"q\" \xc3\xa9\n";
        let c = RawCommit::parse("abc", raw);
        assert_eq!(c.tree, "aaaa");
        assert_eq!(c.parents, ["p1", "p2"]);
        assert_eq!(c.author.email, "a@x");
        assert_eq!(c.committer.date.as_deref(), Some("@2 -0100"));
        assert_eq!(c.encoding.as_deref(), Some("ISO-8859-1"));
        assert!(c.signed);
        assert_eq!(
            c.message,
            b"Subject line\n  more\n\nbody \"q\" \xc3\xa9\n".to_vec()
        );
        assert_eq!(c.subject(), "Subject line more");
    }

    #[test]
    fn raw_commit_without_message() {
        let c = RawCommit::parse(
            "abc",
            b"tree t\nauthor A <a@x> 1 +0000\ncommitter A <a@x> 1 +0000\n\n",
        );
        assert!(c.message.is_empty());
        assert!(c.parents.is_empty());
        assert_eq!(c.subject(), "");
    }

    #[test]
    fn batch_parsing() {
        let out = b"aaa commit 3\nx\ny\nbbb commit 0\n\n";
        let v = parse_batch(out).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].1, b"x\ny".to_vec());
        assert!(v[1].1.is_empty());
        assert!(parse_batch(b"aaa missing\n").is_none());
        assert!(parse_batch(b"aaa commit 10\nshort\n").is_none());
        assert!(parse_batch(b"aaa blob 1\nx\n").is_none());
    }

    #[test]
    fn backup_ref_names() {
        assert_eq!(
            parse_backup_ref("refs/githop/backup/main/20261031T101500Z"),
            Some(("main".into(), "20261031T101500Z".into()))
        );
        assert_eq!(
            parse_backup_ref("refs/githop/backup/feature/x/20261031T101500Z-02"),
            Some(("feature/x".into(), "20261031T101500Z-02".into()))
        );
        assert_eq!(
            parse_backup_ref("refs/githop/backup/20261031T101500Z"),
            None
        );
        assert_eq!(parse_backup_ref("refs/heads/main"), None);
        assert_eq!(parse_backup_ref("refs/githop/backup/main/latest"), None);
        assert!(is_stamp(&timestamp()));
    }

    #[test]
    fn ref_name_safety() {
        assert!(is_valid_ref_name(
            "refs/githop/backup/main/20261031T101500Z"
        ));
        for bad in [
            "",
            "-x",
            "refs/githop/backup/../heads/main/20261031T101500Z",
            "refs/githop/backup/a b/20261031T101500Z",
            "refs/githop/backup/a@{1}/20261031T101500Z",
            "refs/githop/backup/a^/20261031T101500Z",
            "refs/githop/backup/.a/20261031T101500Z",
            "refs/githop/backup/a.lock/20261031T101500Z",
            "refs/githop/backup/a\\b/20261031T101500Z",
        ] {
            assert!(!is_valid_ref_name(bad), "{bad}");
        }
    }

    fn rc(sha: &str, parents: &[&str]) -> RawCommit {
        RawCommit {
            sha: sha.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            ..RawCommit::default()
        }
    }

    #[test]
    fn rewrite_set_covers_descendants_only() {
        // a → b(offender) → c ; side d off a ; merge m(c, d)
        let commits = vec![
            rc("a", &["base"]),
            rc("b", &["a"]),
            rc("d", &["a"]),
            rc("c", &["b"]),
            rc("m", &["c", "d"]),
        ];
        let off = OffendingCommit {
            sha: "b".into(),
            subject: String::new(),
            author_email: String::new(),
            committer_email: String::new(),
            author_mismatch: true,
            committer_mismatch: false,
        };
        let map: HashMap<&str, &OffendingCommit> = [("b", &off)].into_iter().collect();
        let set = rewrite_set(&commits, &map);
        let mut v: Vec<&str> = set.iter().map(String::as_str).collect();
        v.sort_unstable();
        assert_eq!(v, ["b", "c", "m"]);
    }

    #[test]
    fn hex_ids() {
        assert!(is_hex(&"a".repeat(40)));
        assert!(is_hex(&"0".repeat(64)));
        assert!(!is_hex("abc"));
        assert!(!is_hex(&"g".repeat(40)));
    }
}
