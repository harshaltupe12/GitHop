//! Phase 4 integration tests: `githop_core::fix` against real git in an
//! isolated environment (CLAUDE.md rule 3 — never the developer's config).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use githop_core::check::{self, CheckContext, Outcome, Stage};
use githop_core::fix::{self, Fault, FixBlocker};
use githop_core::{CoreError, Profile, ProfileColor, ProfileSet, Settings, Store};
use support::TestEnv;

const OFFICE: &str = "jane@company.com";
const OFFICE_NAME: &str = "Jane Office";
const PERSONAL: &str = "jane@personal.dev";
const PERSONAL_NAME: &str = "Jane";

// ------------------------------------------------------------------ helpers

fn profile(env: &TestEnv, id: &str, name: &str, email: &str, color: ProfileColor) -> Profile {
    Profile {
        id: id.into(),
        label: format!("{}{}", id[..1].to_uppercase(), &id[1..]),
        name: name.into(),
        email: email.into(),
        color,
        ssh_key: PathBuf::from(format!(
            "{}/.ssh/githop_{id}",
            env.home().to_string_lossy().replace('\\', "/")
        )),
        owners: vec![],
        // Two profiles each need an owner or folder rule: one no test repo is in.
        folders: vec![format!(
            "{}/no-repos-here/{id}",
            env.home().to_string_lossy().replace('\\', "/")
        )],
        sign_commits: false,
    }
}

fn save_profiles(env: &TestEnv, office_name: &str) {
    let set = ProfileSet {
        profiles: vec![
            profile(env, "office", office_name, OFFICE, ProfileColor::Blue),
            profile(
                env,
                "personal",
                PERSONAL_NAME,
                PERSONAL,
                ProfileColor::Violet,
            ),
        ],
        ..ProfileSet::default()
    };
    let mut store = Store::open_at(env.paths()).unwrap();
    store.save(&set, &Settings::default()).unwrap();
}

struct Fx {
    env: TestEnv,
    repo: PathBuf,
    n: std::cell::Cell<u32>,
}

impl Fx {
    /// Profiles saved; repo `repos/api` pinned to office; no commits.
    fn new() -> Self {
        Self::with_office_name(OFFICE_NAME)
    }

    fn with_office_name(office_name: &str) -> Self {
        let env = TestEnv::new();
        save_profiles(&env, office_name);
        let repo = env.init_repo("api");
        env.git(&["config", "githop.profile", "office"], &repo);
        Self {
            env,
            repo,
            n: std::cell::Cell::new(0),
        }
    }

    /// Repo with an `origin` bare remote and `pushed` office commits pushed.
    fn with_remote(pushed: usize) -> Self {
        let fx = Self::new();
        let remote = fx.env.bare_remote("office-org/api");
        fx.git(&["remote", "add", "origin", &remote.to_string_lossy()]);
        for _ in 0..pushed {
            fx.right();
        }
        fx.git(&["push", "-q", "origin", "main"]);
        fx
    }

    fn git(&self, args: &[&str]) -> String {
        self.env.git(args, &self.repo)
    }

    fn git_ok(&self, args: &[&str]) -> bool {
        self.env.git_output(args, &self.repo).status.success()
    }

    fn raw(&self, args: &[&str]) -> Vec<u8> {
        let out = self.env.git_output(args, &self.repo);
        assert!(out.status.success(), "git {args:?} failed");
        out.stdout
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn show(&self, rev: &str, fmt: &str) -> String {
        self.git(&["show", "-s", &format!("--format={fmt}"), rev])
    }

    fn next(&self) -> u32 {
        self.n.set(self.n.get() + 1);
        self.n.get()
    }

    /// Commit a new file with explicit author/committer, date and verbatim message.
    fn commit_full(
        &self,
        author: (&str, &str),
        committer: (&str, &str),
        date: &str,
        message: &[u8],
        allow_empty: bool,
    ) -> String {
        let n = self.next();
        if !allow_empty {
            let file = format!("f{n}.txt");
            std::fs::write(self.repo.join(&file), format!("content {n}\n")).unwrap();
            self.git(&["add", &file]);
        }
        let msg_file = self.env.root().join(format!("msg{n}.txt"));
        std::fs::write(&msg_file, message).unwrap();
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.repo);
        self.env.apply(&mut cmd);
        cmd.args(["commit", "-q", "--cleanup=verbatim", "--allow-empty", "-F"])
            .arg(&msg_file)
            .env("GIT_AUTHOR_NAME", author.0)
            .env("GIT_AUTHOR_EMAIL", author.1)
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_NAME", committer.0)
            .env("GIT_COMMITTER_EMAIL", committer.1)
            .env("GIT_COMMITTER_DATE", date);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        self.head()
    }

    fn commit_as(&self, name: &str, email: &str) -> String {
        let n = self.n.get() + 1;
        let date = format!("2026-10-{:02}T09:{:02}:00+05:30", 1 + n % 28, n % 60);
        self.commit_full(
            (name, email),
            (name, email),
            &date,
            format!("change {n}\n").as_bytes(),
            false,
        )
    }

    fn right(&self) -> String {
        self.commit_as(OFFICE_NAME, OFFICE)
    }

    fn wrong(&self) -> String {
        self.commit_as(PERSONAL_NAME, PERSONAL)
    }

    fn plan(&self) -> fix::FixPlan {
        fix::plan(&self.env.git_handle(), &self.repo, &self.env.paths()).unwrap()
    }

    fn apply(&self) -> githop_core::Result<fix::FixResult> {
        fix::apply(&self.env.git_handle(), &self.repo, &self.env.paths())
    }

    fn undo(&self, backup: &str) -> githop_core::Result<()> {
        fix::undo(&self.env.git_handle(), &self.repo, backup)
    }

    fn githop_refs(&self) -> Vec<String> {
        self.git(&["for-each-ref", "--format=%(refname)", "refs/githop/"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn assert_clean(&self) {
        assert_eq!(
            self.git(&["status", "--porcelain", "--untracked-files=no"]),
            ""
        );
        assert!(!self.repo.join(".git/rebase-merge").exists());
        assert!(!self.repo.join(".git/rebase-apply").exists());
    }

    /// Emails (author, committer) of every commit in `range`.
    fn emails(&self, range: &str) -> Vec<(String, String)> {
        self.git(&["log", "--format=%ae %ce", range])
            .lines()
            .map(|l| {
                let (a, c) = l.split_once(' ').unwrap();
                (a.to_string(), c.to_string())
            })
            .collect()
    }

    /// Message bytes of a commit, exactly as stored.
    fn message(&self, rev: &str) -> Vec<u8> {
        let raw = self.raw(&["cat-file", "commit", rev]);
        let i = raw.windows(2).position(|w| w == b"\n\n").unwrap();
        raw[i + 2..].to_vec()
    }

    fn rev_list(&self, args: &[&str]) -> Vec<String> {
        let mut a = vec!["rev-list"];
        a.extend_from_slice(args);
        self.git(&a).lines().map(str::to_string).collect()
    }
}

fn refused(r: githop_core::Result<impl std::fmt::Debug>) -> String {
    match r {
        Err(CoreError::FixRefused(msg)) => msg,
        other => panic!("expected FixRefused, got {other:?}"),
    }
}

fn has_lock(env: &TestEnv, repo: &Path) -> bool {
    let _ = env;
    repo.join(".git/index.lock").exists()
}

// -------------------------------------------------------------------- tests

/// Plan §1: two wrong-identity commits on top of pushed history.
#[test]
fn unpushed_wrong_commits_are_fixed_and_pushed_history_untouched() {
    let fx = Fx::with_remote(2);
    let pushed: Vec<String> = fx.rev_list(&["origin/main"]);
    let origin = fx.git(&["rev-parse", "origin/main"]);
    fx.wrong();
    fx.wrong();
    let old_head = fx.head();
    let old_tree = fx.git(&["rev-parse", "HEAD^{tree}"]);
    let old_dates = fx.git(&["log", "--format=%ad", "--date=raw", "origin/main..HEAD"]);
    let old_msgs: Vec<Vec<u8>> = fx
        .rev_list(&["origin/main..HEAD"])
        .iter()
        .map(|s| fx.message(s))
        .collect();

    let plan = fx.plan();
    assert_eq!(plan.branch.as_deref(), Some("main"));
    assert_eq!(plan.profile_id.as_deref(), Some("office"));
    assert_eq!(plan.profile_email.as_deref(), Some(OFFICE));
    assert_eq!(plan.offenders.len(), 2);
    assert!(plan
        .offenders
        .iter()
        .all(|o| o.author_mismatch && o.committer_mismatch));
    assert_eq!(plan.offenders[0].sha, old_head, "newest first");
    assert!(plan.pushed_offenders.is_empty());
    assert_eq!(plan.blocked_reason, None);
    assert_eq!((plan.unpushed, plan.rewrite_count, plan.signed), (2, 2, 0));
    assert!(plan.has_remote_refs);
    assert!(plan.can_apply());

    let res = fx.apply().unwrap();
    assert_eq!(res.rewritten, 2);
    assert_eq!(res.recreated, 2);
    assert_eq!(res.old_head, old_head);
    assert_eq!(res.new_head, fx.head());
    assert!(res.backup_ref.starts_with("refs/githop/backup/main/"));
    assert_eq!(
        res.undo_hint,
        format!("githop fix --undo {}", res.backup_ref)
    );
    assert_eq!(fx.git(&["rev-parse", &res.backup_ref]), old_head);

    // Pushed history: same hashes, remote-tracking ref unchanged.
    assert_eq!(fx.git(&["rev-parse", "origin/main"]), origin);
    assert_eq!(fx.rev_list(&["HEAD~2"]), pushed);
    // New commits: office identity, same author dates, messages and tree.
    for (a, c) in fx.emails("origin/main..HEAD") {
        assert_eq!((a.as_str(), c.as_str()), (OFFICE, OFFICE));
    }
    assert_eq!(
        fx.show("HEAD", "%an|%cn"),
        format!("{OFFICE_NAME}|{OFFICE_NAME}")
    );
    assert_eq!(
        fx.git(&["log", "--format=%ad", "--date=raw", "origin/main..HEAD"]),
        old_dates
    );
    assert!(old_dates.contains("+0530"));
    let new_msgs: Vec<Vec<u8>> = fx
        .rev_list(&["origin/main..HEAD"])
        .iter()
        .map(|s| fx.message(s))
        .collect();
    assert_eq!(new_msgs, old_msgs);
    assert_eq!(fx.git(&["rev-parse", "HEAD^{tree}"]), old_tree);
    fx.assert_clean();

    // Nothing left to fix, and the pre-push check now passes.
    assert!(fx.plan().offenders.is_empty());
    let ctx = CheckContext {
        git: fx.env.git_handle(),
        cwd: fx.repo.clone(),
        paths: fx.env.paths(),
    };
    let stdin = format!("refs/heads/main {} refs/heads/main {origin}\n", fx.head());
    let result = check::run_check(
        Stage::Push,
        &[
            "origin".to_string(),
            "https://github.com/office-org/api.git".to_string(),
        ],
        &stdin,
        &ctx,
    );
    assert_eq!(result.outcome, Outcome::Pass, "{:?}", result.messages);
    fx.git(&["push", "-q", "origin", "main"]);
}

#[test]
fn pushed_offender_is_reported_and_never_touched() {
    let fx = Fx::new();
    let remote = fx.env.bare_remote("office-org/api");
    fx.git(&["remote", "add", "origin", &remote.to_string_lossy()]);
    fx.right();
    let bad = fx.wrong();
    fx.git(&["push", "-q", "origin", "main"]);

    let plan = fx.plan();
    assert!(plan.offenders.is_empty());
    assert_eq!(plan.pushed_offenders.len(), 1);
    assert_eq!(plan.pushed_offenders[0].sha, bad);
    assert_eq!(plan.pushed_offenders[0].author_email, PERSONAL);
    assert!(!plan.can_apply());

    let msg = refused(fx.apply());
    assert!(msg.contains("Nothing to fix"), "{msg}");
    assert_eq!(fx.head(), bad);
    assert!(fx.githop_refs().is_empty());
}

#[test]
fn colleagues_pushed_commits_are_not_reported() {
    let fx = Fx::new();
    let remote = fx.env.bare_remote("office-org/api");
    fx.git(&["remote", "add", "origin", &remote.to_string_lossy()]);
    fx.commit_as("Colleague", "colleague@company.com");
    fx.git(&["push", "-q", "origin", "main"]);
    let plan = fx.plan();
    assert!(plan.pushed_offenders.is_empty());
    assert!(plan.offenders.is_empty());
}

#[test]
fn mixed_pushed_and_unpushed_fixes_only_unpushed() {
    let fx = Fx::new();
    let remote = fx.env.bare_remote("office-org/api");
    fx.git(&["remote", "add", "origin", &remote.to_string_lossy()]);
    fx.right();
    let pushed_bad = fx.wrong();
    fx.git(&["push", "-q", "origin", "main"]);
    fx.right();
    let local_bad = fx.wrong();

    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 1);
    assert_eq!(plan.offenders[0].sha, local_bad);
    assert_eq!(plan.pushed_offenders.len(), 1);
    assert_eq!(plan.pushed_offenders[0].sha, pushed_bad);
    assert_eq!(plan.unpushed, 2);
    assert_eq!(
        plan.rewrite_count, 1,
        "the good commit before the offender keeps its hash"
    );
    let good_local = fx.git(&["rev-parse", "HEAD~1"]);

    let res = fx.apply().unwrap();
    assert_eq!(res.recreated, 1);
    assert_eq!(fx.git(&["rev-parse", "HEAD~1"]), good_local);
    assert_eq!(fx.git(&["rev-parse", "origin/main"]), pushed_bad);
    assert!(fx.git_ok(&["merge-base", "--is-ancestor", &pushed_bad, "HEAD"]));
    assert_eq!(fx.show("HEAD", "%ae"), OFFICE);
    assert_eq!(fx.show(&pushed_bad, "%ae"), PERSONAL);
    // The pushed offender is still reported (it can't be fixed), unpushed ones are gone.
    let after = fx.plan();
    assert!(after.offenders.is_empty());
    assert_eq!(after.pushed_offenders.len(), 1);
}

#[test]
fn dirty_tree_is_refused_untracked_files_are_fine() {
    let fx = Fx::new();
    fx.right();
    fx.wrong();
    let head = fx.head();
    std::fs::write(fx.repo.join("f1.txt"), "edited\n").unwrap();

    let plan = fx.plan();
    assert_eq!(plan.blocker, Some(FixBlocker::DirtyTree));
    assert!(plan.blocked_reason.unwrap().contains("Commit or stash"));
    assert_eq!(plan.offenders.len(), 1, "offenders still listed for the UI");
    let msg = refused(fx.apply());
    assert!(msg.contains("uncommitted changes"));
    assert_eq!(fx.head(), head);
    assert!(fx.githop_refs().is_empty());
    assert_eq!(
        std::fs::read_to_string(fx.repo.join("f1.txt")).unwrap(),
        "edited\n"
    );

    // Staged changes count too.
    fx.git(&["add", "f1.txt"]);
    assert_eq!(fx.plan().blocker, Some(FixBlocker::DirtyTree));

    fx.git(&["reset", "-q", "--hard"]);
    std::fs::write(fx.repo.join("untracked.txt"), "x").unwrap();
    assert!(fx.plan().can_apply());
    fx.apply().unwrap();
    assert!(fx.repo.join("untracked.txt").exists());
}

#[test]
fn detached_head_is_refused() {
    let fx = Fx::new();
    fx.wrong();
    fx.git(&["checkout", "-q", "--detach"]);
    let plan = fx.plan();
    assert_eq!(plan.branch, None);
    assert_eq!(plan.blocker, Some(FixBlocker::DetachedHead));
    let head = fx.head();
    assert!(refused(fx.apply()).contains("detached"));
    assert_eq!(fx.head(), head);
    assert!(fx.githop_refs().is_empty());
}

#[test]
fn rebase_in_progress_is_refused() {
    let fx = Fx::new();
    fx.right();
    std::fs::write(fx.repo.join("same.txt"), "base\n").unwrap();
    fx.git(&["add", "same.txt"]);
    fx.git(&[
        "-c",
        "user.name=x",
        "-c",
        &format!("user.email={OFFICE}"),
        "commit",
        "-q",
        "-m",
        "base",
    ]);
    fx.git(&["checkout", "-q", "-b", "side"]);
    std::fs::write(fx.repo.join("same.txt"), "side\n").unwrap();
    fx.git(&[
        "-c",
        "user.name=x",
        "-c",
        &format!("user.email={PERSONAL}"),
        "commit",
        "-q",
        "-am",
        "side",
    ]);
    fx.git(&["checkout", "-q", "main"]);
    std::fs::write(fx.repo.join("same.txt"), "main\n").unwrap();
    fx.git(&[
        "-c",
        "user.name=x",
        "-c",
        &format!("user.email={OFFICE}"),
        "commit",
        "-q",
        "-am",
        "main",
    ]);
    fx.git(&["checkout", "-q", "side"]);
    let out = fx.env.git_output(
        &[
            "-c",
            "user.name=x",
            "-c",
            "user.email=x@x",
            "rebase",
            "main",
        ],
        &fx.repo,
    );
    assert!(!out.status.success(), "rebase should stop on the conflict");

    let plan = fx.plan();
    assert_eq!(plan.blocker, Some(FixBlocker::OperationInProgress));
    let reason = plan.blocked_reason.unwrap();
    assert!(reason.contains("rebase"), "{reason}");
    assert!(refused(fx.apply()).contains("rebase"));
    assert!(fx.githop_refs().is_empty());
    fx.git(&["rebase", "--abort"]);
}

#[test]
fn merges_keep_topology_and_conflict_resolutions() {
    let fx = Fx::with_remote(1);
    let base = fx.head();
    fx.git(&["checkout", "-q", "-b", "side"]);
    std::fs::write(fx.repo.join("shared.txt"), "side\n").unwrap();
    fx.git(&["add", "shared.txt"]);
    let side = fx.wrong();
    fx.git(&["checkout", "-q", "main"]);
    std::fs::write(fx.repo.join("shared.txt"), "main\n").unwrap();
    fx.git(&["add", "shared.txt"]);
    let main_c = fx.wrong();
    // Conflicting merge, resolved by hand ("evil" content in neither parent).
    let out = fx.env.git_output(
        &[
            "-c",
            "user.name=x",
            "-c",
            "user.email=x@x",
            "merge",
            "--no-ff",
            "-q",
            "side",
        ],
        &fx.repo,
    );
    assert!(!out.status.success(), "expected a conflict");
    std::fs::write(fx.repo.join("shared.txt"), "resolved by hand\n").unwrap();
    fx.git(&["add", "shared.txt"]);
    fx.git(&[
        "-c",
        &format!("user.name={PERSONAL_NAME}"),
        "-c",
        &format!("user.email={PERSONAL}"),
        "commit",
        "-q",
        "--no-edit",
    ]);
    let old_tree = fx.git(&["rev-parse", "HEAD^{tree}"]);
    let old_subjects = fx.git(&["log", "--format=%s %P", "--topo-order", "origin/main..HEAD"]);
    let _ = (side, main_c);

    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 3);
    let res = fx.apply().unwrap();
    assert_eq!(res.recreated, 3);
    assert_eq!(fx.git(&["rev-parse", "HEAD^{tree}"]), old_tree);
    assert_eq!(
        std::fs::read_to_string(fx.repo.join("shared.txt")).unwrap(),
        "resolved by hand\n"
    );
    let parents = fx.show("HEAD", "%P");
    assert_eq!(parents.split(' ').count(), 2, "still a merge");
    assert_eq!(fx.rev_list(&["origin/main..HEAD"]).len(), 3);
    assert_eq!(fx.git(&["merge-base", "HEAD^1", "HEAD^2"]), base);
    assert_eq!(fx.show("HEAD^1", "%s"), "change 3");
    assert_eq!(fx.show("HEAD^2", "%s"), "change 2");
    for (a, c) in fx.emails("origin/main..HEAD") {
        assert_eq!((a.as_str(), c.as_str()), (OFFICE, OFFICE));
    }
    assert_eq!(old_subjects.lines().count(), 3);
    fx.assert_clean();
}

#[test]
fn merge_of_a_pushed_branch_keeps_the_pushed_parent() {
    let fx = Fx::with_remote(1);
    fx.git(&["checkout", "-q", "-b", "side"]);
    let side = fx.wrong();
    fx.git(&["push", "-q", "origin", "side"]);
    fx.git(&["checkout", "-q", "main"]);
    fx.right();
    fx.git(&[
        "-c",
        &format!("user.name={PERSONAL_NAME}"),
        "-c",
        &format!("user.email={PERSONAL}"),
        "merge",
        "--no-ff",
        "-q",
        "-m",
        "merge side",
        "side",
    ]);
    let plan = fx.plan();
    assert_eq!(
        plan.offenders.len(),
        1,
        "only the merge is unpushed and wrong"
    );
    assert_eq!(plan.offenders[0].subject, "merge side");
    assert_eq!(plan.pushed_offenders.len(), 1);
    assert_eq!(plan.pushed_offenders[0].sha, side);

    let first_parent = fx.git(&["rev-parse", "HEAD^1"]);
    fx.apply().unwrap();
    assert_eq!(
        fx.git(&["rev-parse", "HEAD^2"]),
        side,
        "pushed parent untouched"
    );
    assert_eq!(fx.git(&["rev-parse", "HEAD^1"]), first_parent);
    assert_eq!(fx.git(&["rev-parse", "origin/side"]), side);
    assert_eq!(fx.show("HEAD", "%ae %ce"), format!("{OFFICE} {OFFICE}"));
}

#[test]
fn root_commit_in_repo_without_remote() {
    let fx = Fx::new();
    let old_root = fx.wrong();
    fx.wrong();
    fx.right();
    let plan = fx.plan();
    assert!(!plan.has_remote_refs);
    assert_eq!(
        (plan.unpushed, plan.offenders.len(), plan.rewrite_count),
        (3, 2, 3)
    );
    let old_tree = fx.git(&["rev-parse", "HEAD^{tree}"]);
    fx.apply().unwrap();
    let roots = fx.rev_list(&["--max-parents=0", "HEAD"]);
    assert_eq!(roots.len(), 1);
    assert_ne!(roots[0], old_root);
    assert_eq!(fx.show(&roots[0], "%ae %s"), format!("{OFFICE} change 1"));
    assert_eq!(fx.rev_list(&["HEAD"]).len(), 3);
    assert_eq!(fx.git(&["rev-parse", "HEAD^{tree}"]), old_tree);
}

fn assert_untouched(fx: &Fx, head: &str) {
    assert_eq!(fx.head(), head);
    assert!(fx.githop_refs().is_empty(), "{:?}", fx.githop_refs());
    fx.assert_clean();
    assert!(!has_lock(&fx.env, &fx.repo));
}

#[test]
fn failure_while_rewriting_leaves_repo_as_before() {
    let fx = Fx::new();
    fx.right();
    fx.wrong();
    fx.wrong();
    let head = fx.head();
    let r = fix::apply_with_fault(
        &fx.env.git_handle(),
        &fx.repo,
        &fx.env.paths(),
        Some(Fault::DuringRewrite),
    );
    assert!(r.is_err());
    assert_untouched(&fx, &head);
    assert_eq!(fx.plan().offenders.len(), 2);
}

#[test]
fn failure_after_branch_update_rolls_back() {
    let fx = Fx::new();
    fx.right();
    fx.wrong();
    let head = fx.head();
    let r = fix::apply_with_fault(
        &fx.env.git_handle(),
        &fx.repo,
        &fx.env.paths(),
        Some(Fault::AfterBranchUpdate),
    );
    assert!(r.is_err());
    assert_untouched(&fx, &head);
}

#[test]
fn locked_branch_ref_fails_cleanly() {
    let fx = Fx::new();
    fx.right();
    fx.wrong();
    let head = fx.head();
    let lock = fx.repo.join(".git/refs/heads/main.lock");
    std::fs::write(&lock, "").unwrap();
    let r = fx.apply();
    assert!(matches!(r, Err(CoreError::GitFailed { .. })), "{r:?}");
    std::fs::remove_file(&lock).unwrap();
    assert_untouched(&fx, &head);
    fx.apply().unwrap();
}

#[test]
fn undo_restores_the_original_commits_exactly() {
    let fx = Fx::with_remote(1);
    fx.wrong();
    fx.wrong();
    let head = fx.head();
    let res = fx.apply().unwrap();
    assert_ne!(fx.head(), head);
    assert_eq!(
        fix::list_backups(&fx.env.git_handle(), &fx.repo).unwrap(),
        vec![res.backup_ref.clone()]
    );
    assert_eq!(
        fix::latest_backup(&fx.env.git_handle(), &fx.repo).unwrap(),
        Some(res.backup_ref.clone())
    );

    fx.undo(&res.backup_ref).unwrap();
    assert_eq!(fx.head(), head);
    assert_eq!(fx.show("HEAD", "%ae"), PERSONAL);
    assert!(fx.githop_refs().is_empty());
    fx.assert_clean();
    assert_eq!(
        fix::latest_backup(&fx.env.git_handle(), &fx.repo).unwrap(),
        None
    );
    // A second undo has nothing to restore.
    assert!(refused(fx.undo(&res.backup_ref)).contains("no backup"));
}

#[test]
fn undo_is_refused_after_new_commits() {
    let fx = Fx::new();
    fx.wrong();
    let res = fx.apply().unwrap();
    let latest = fx.right();
    let msg = refused(fx.undo(&res.backup_ref));
    assert!(msg.contains("changed since the fix"), "{msg}");
    assert!(msg.contains("git reset --keep"), "{msg}");
    assert_eq!(fx.head(), latest);
    assert_eq!(fx.githop_refs().len(), 2, "backup and result kept");
}

#[test]
fn undo_is_refused_on_another_branch_or_dirty_tree() {
    let fx = Fx::new();
    fx.wrong();
    let res = fx.apply().unwrap();
    fx.git(&["checkout", "-q", "-b", "other"]);
    assert!(refused(fx.undo(&res.backup_ref)).contains("git switch main"));
    fx.git(&["checkout", "-q", "main"]);
    std::fs::write(fx.repo.join("f1.txt"), "edited\n").unwrap();
    assert!(refused(fx.undo(&res.backup_ref)).contains("uncommitted"));
    fx.git(&["checkout", "--", "f1.txt"]);
    fx.undo(&res.backup_ref).unwrap();
}

#[test]
fn undo_rejects_refs_that_are_not_githop_backups() {
    let fx = Fx::new();
    fx.wrong();
    let head = fx.head();
    for bad in [
        "refs/heads/main",
        "main",
        "--hard",
        "refs/githop/backup/../heads/main/20261031T101500Z",
        "refs/githop/backup/main/20261031T101500Z..HEAD",
        "refs/githop/backup/main/HEAD",
        "refs/githop/fixed/main/20261031T101500Z",
    ] {
        assert!(
            refused(fx.undo(bad)).contains("isn't a GitHop backup"),
            "{bad}"
        );
    }
    assert!(refused(fx.undo("refs/githop/backup/main/20990101T000000Z")).contains("no backup"));
    assert_eq!(fx.head(), head);
}

#[test]
fn branch_names_with_slashes() {
    let fx = Fx::new();
    fx.right();
    fx.git(&["checkout", "-q", "-b", "feature/login"]);
    fx.wrong();
    let head = fx.head();
    let res = fx.apply().unwrap();
    assert!(res
        .backup_ref
        .starts_with("refs/githop/backup/feature/login/"));
    fx.undo(&res.backup_ref).unwrap();
    assert_eq!(fx.head(), head);
}

#[test]
fn backups_are_pruned_to_ten_per_branch() {
    let fx = Fx::new();
    fx.wrong();
    let head = fx.head();
    for day in 1..=12 {
        let stamp = format!("202501{day:02}T000000Z");
        fx.git(&[
            "update-ref",
            &format!("refs/githop/backup/main/{stamp}"),
            &head,
        ]);
        fx.git(&[
            "update-ref",
            &format!("refs/githop/fixed/main/{stamp}"),
            &head,
        ]);
    }
    fx.git(&[
        "update-ref",
        "refs/githop/backup/other/20250101T000000Z",
        &head,
    ]);
    fx.git(&[
        "update-ref",
        "refs/githop/fixed/orphan/20250101T000000Z",
        &head,
    ]);

    let res = fx.apply().unwrap();
    let backups = fix::list_backups(&fx.env.git_handle(), &fx.repo).unwrap();
    let main: Vec<&String> = backups
        .iter()
        .filter(|b| b.starts_with("refs/githop/backup/main/"))
        .collect();
    assert_eq!(main.len(), fix::BACKUPS_KEPT);
    assert_eq!(main[0], &res.backup_ref, "newest first");
    assert_eq!(main[9], "refs/githop/backup/main/20250104T000000Z");
    assert!(backups.contains(&"refs/githop/backup/other/20250101T000000Z".to_string()));
    let refs = fx.githop_refs();
    assert!(!refs.contains(&"refs/githop/fixed/main/20250103T000000Z".to_string()));
    assert!(refs.contains(&"refs/githop/fixed/main/20250104T000000Z".to_string()));
    assert!(!refs.iter().any(|r| r.contains("orphan")));
}

#[test]
fn two_fixes_in_the_same_second_get_distinct_backups() {
    let fx = Fx::new();
    fx.wrong();
    let a = fx.apply().unwrap();
    fx.wrong();
    let b = fx.apply().unwrap();
    assert_ne!(a.backup_ref, b.backup_ref);
    assert_eq!(
        fix::list_backups(&fx.env.git_handle(), &fx.repo).unwrap()[0],
        b.backup_ref
    );
}

#[test]
fn messages_are_preserved_byte_for_byte() {
    let fx = Fx::with_remote(1);
    let messages: [&[u8]; 5] = [
        b"He said \"hi\" & 'bye' $HOME `whoami` $(echo pwned) %PATH% \\n\n",
        "Ünïcødé 🚀 — 日本語\n\nBody with trailing spaces   \n\n\n".as_bytes(),
        b"# starts with a hash\n\n    indented\n\ttabbed\n",
        b"no trailing newline",
        b"Subject\r\n\r\nCRLF body\r\n",
    ];
    for m in messages {
        fx.commit_full(
            (PERSONAL_NAME, PERSONAL),
            (PERSONAL_NAME, PERSONAL),
            "2026-10-01T10:00:00-0700",
            m,
            false,
        );
    }
    let before: Vec<Vec<u8>> = fx
        .rev_list(&["--reverse", "origin/main..HEAD"])
        .iter()
        .map(|s| fx.message(s))
        .collect();
    assert_eq!(before.len(), 5);
    fx.apply().unwrap();
    let after: Vec<Vec<u8>> = fx
        .rev_list(&["--reverse", "origin/main..HEAD"])
        .iter()
        .map(|s| fx.message(s))
        .collect();
    assert_eq!(after, before);
    assert_eq!(
        fx.git(&["log", "-1", "--format=%ad", "--date=raw", "HEAD"]),
        "1790874000 -0700"
    );
}

#[test]
fn profile_name_with_shell_characters_is_used_literally() {
    // (git itself trims "crud" like a trailing `\` or `"` from names, so end on a letter.)
    let evil = r#"Har"sh'al $(touch pwned1) `touch pwned2` ; touch pwned3 & | %PATH% \n \" end"#;
    let fx = Fx::with_office_name(evil);
    fx.wrong();
    fx.wrong();
    fx.apply().unwrap();
    assert_eq!(fx.show("HEAD", "%an"), evil);
    assert_eq!(fx.show("HEAD", "%cn"), evil);
    assert_eq!(fx.show("HEAD~1", "%an|%ae"), format!("{evil}|{OFFICE}"));
    for dir in [
        fx.repo.clone(),
        fx.env.root().to_path_buf(),
        std::env::current_dir().unwrap(),
    ] {
        for f in ["pwned1", "pwned2", "pwned3"] {
            assert!(!dir.join(f).exists(), "{f} created in {}", dir.display());
        }
    }
}

#[test]
fn committer_only_mismatch_keeps_the_author() {
    let fx = Fx::with_remote(1);
    fx.commit_full(
        ("Pair Partner", OFFICE),
        (PERSONAL_NAME, PERSONAL),
        "2026-09-15T08:30:00+0200",
        b"pairing\n",
        false,
    );
    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 1);
    assert!(!plan.offenders[0].author_mismatch);
    assert!(plan.offenders[0].committer_mismatch);
    fx.apply().unwrap();
    assert_eq!(
        fx.show("HEAD", "%an|%ae|%ad|%cn|%ce"),
        format!("Pair Partner|{OFFICE}|Tue Sep 15 08:30:00 2026 +0200|{OFFICE_NAME}|{OFFICE}")
    );
}

#[test]
fn good_commits_after_an_offender_keep_their_author() {
    let fx = Fx::new();
    fx.right();
    fx.wrong();
    fx.commit_full(
        ("Someone Else", "else@company.com"),
        (OFFICE_NAME, OFFICE),
        "2026-08-01T12:00:00+0000",
        b"by someone else\n",
        false,
    );
    fx.right();
    let plan = fx.plan();
    // "Someone Else" isn't one of the user's emails: not an offender (Review #3).
    assert_eq!(plan.offenders.len(), 1);
    assert_eq!(plan.foreign, 1);
    assert_eq!(plan.rewrite_count, 3);
    let last = fx.show("HEAD", "%an|%ae|%ad|%cn|%ce|%cd");
    let other = fx.show("HEAD~1", "%an|%ae|%ad|%cn|%ce|%cd");
    fx.apply().unwrap();
    assert_eq!(fx.show("HEAD", "%an|%ae|%ad|%cn|%ce|%cd"), last);
    assert_eq!(
        fx.show("HEAD~1", "%an|%ae|%ad|%cn|%ce|%cd"),
        other,
        "someone else's commit is copied exactly"
    );
    assert_eq!(
        fx.show("HEAD~2", "%an|%ae"),
        format!("{OFFICE_NAME}|{OFFICE}")
    );
}

#[test]
fn empty_commits_and_submodule_pointers_are_kept() {
    let fx = Fx::with_remote(1);
    fx.commit_full(
        (PERSONAL_NAME, PERSONAL),
        (PERSONAL_NAME, PERSONAL),
        "2026-10-01T10:00:00+0000",
        b"empty\n",
        true,
    );
    let pointer = "1234567890abcdef1234567890abcdef12345678";
    // An uninitialised submodule is an empty directory.
    std::fs::create_dir_all(fx.repo.join("vendor/lib")).unwrap();
    fx.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{pointer},vendor/lib"),
    ]);
    fx.git(&[
        "-c",
        &format!("user.name={PERSONAL_NAME}"),
        "-c",
        &format!("user.email={PERSONAL}"),
        "commit",
        "-q",
        "-m",
        "add submodule pointer",
    ]);
    let old_tree = fx.git(&["rev-parse", "HEAD^{tree}"]);
    fx.apply().unwrap();
    assert_eq!(fx.git(&["rev-parse", "HEAD^{tree}"]), old_tree);
    assert_eq!(fx.rev_list(&["origin/main..HEAD"]).len(), 2);
    assert_eq!(fx.show("HEAD~1", "%s|%ae"), format!("empty|{OFFICE}"));
    assert!(fx.git(&["ls-tree", "HEAD", "vendor/lib"]).contains(pointer));
}

#[test]
fn signed_commits_are_counted_and_lose_their_signature() {
    let fx = Fx::new();
    fx.right();
    let tree = fx.git(&["rev-parse", "HEAD^{tree}"]);
    let parent = fx.head();
    let object = format!(
        "tree {tree}\nparent {parent}\nauthor {PERSONAL_NAME} <{PERSONAL}> 1759312800 +0000\n\
         committer {PERSONAL_NAME} <{PERSONAL}> 1759312800 +0000\n\
         gpgsig -----BEGIN PGP SIGNATURE-----\n \n fake\n -----END PGP SIGNATURE-----\n\nsigned\n"
    );
    let file = fx.env.root().join("signed.obj");
    std::fs::write(&file, object).unwrap();
    let signed = fx.git(&["hash-object", "-t", "commit", "-w", &file.to_string_lossy()]);
    fx.git(&["update-ref", "refs/heads/main", &signed]);

    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 1);
    assert_eq!(plan.signed, 1);
    fx.apply().unwrap();
    let raw = String::from_utf8(fx.raw(&["cat-file", "commit", "HEAD"])).unwrap();
    assert!(!raw.contains("gpgsig"));
    assert!(raw.ends_with("\n\nsigned\n"));
    assert_eq!(
        fx.show("HEAD", "%ae %ad"),
        format!("{OFFICE} Wed Oct 1 10:00:00 2025 +0000")
    );
}

#[test]
fn unresolved_repo_is_blocked() {
    let fx = Fx::new();
    fx.git(&["config", "--unset", "githop.profile"]);
    fx.wrong();
    let plan = fx.plan();
    assert_eq!(plan.blocker, Some(FixBlocker::Unresolved));
    assert!(plan.blocked_reason.unwrap().contains("githop pin"));
    assert_eq!(plan.profile_id, None);
    assert!(refused(fx.apply()).contains("githop pin"));
}

#[test]
fn nothing_to_fix() {
    let fx = Fx::with_remote(1);
    fx.right();
    let plan = fx.plan();
    assert!(plan.offenders.is_empty());
    assert_eq!(plan.unpushed, 1);
    assert!(!plan.can_apply());
    assert!(refused(fx.apply()).contains("Nothing to fix"));
    assert!(fx.githop_refs().is_empty());
}

#[test]
fn unborn_branch_has_nothing_to_fix() {
    let fx = Fx::new();
    let plan = fx.plan();
    assert!(plan.offenders.is_empty());
    assert_eq!(plan.branch.as_deref(), Some("main"));
    assert_eq!(plan.blocked_reason, None);
}

#[test]
fn not_a_repo_is_an_error() {
    let env = TestEnv::new();
    save_profiles(&env, OFFICE_NAME);
    let r = fix::plan(&env.git_handle(), &env.home(), &env.paths());
    assert!(matches!(r, Err(CoreError::NotARepo)), "{r:?}");
}

/// The remote double-check passes several `--contains`; git treats them as "any".
#[test]
fn for_each_ref_contains_means_any() {
    let fx = Fx::new();
    let a = fx.right();
    let b = fx.right();
    fx.git(&["update-ref", "refs/remotes/origin/x", &a]);
    let out = fx.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "--contains",
        &a,
        "--contains",
        &b,
        "refs/remotes/",
    ]);
    assert_eq!(out, "refs/remotes/origin/x");
}

// ------------------------------------------- Review 2026-11-01 #2: "pushed"

/// Run the pre-push check like the hook would (it records the push when it
/// goes through), then really push. `enforce = warn` so a wrong commit can be
/// published, which is how wrong commits end up pushed in real life.
fn push_through_hook(fx: &Fx, remote: &str, branch: &str) {
    std::fs::write(
        fx.env.paths().settings_file(),
        "version = 1\nenforce = \"warn\"\n",
    )
    .unwrap();
    let local = fx.git(&["rev-parse", branch]);
    let stdin = format!(
        "refs/heads/{branch} {local} refs/heads/{branch} {}\n",
        "0".repeat(40)
    );
    let ctx = CheckContext {
        git: fx.env.git_handle(),
        cwd: fx.repo.clone(),
        paths: fx.env.paths(),
    };
    let r = check::run_check(
        Stage::Push,
        &[remote.to_string(), remote.to_string()],
        &stdin,
        &ctx,
    );
    assert_eq!(r.exit_code(), 0, "{:?}", r.messages);
    fx.git(&["push", "-q", remote, branch]);
}

fn file_url(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy().replace('\\', "/"))
}

/// A `git clone --single-branch` of a remote with one office commit on
/// main, pinned to office, on a new branch `feature`.
fn single_branch_fixture() -> Fx {
    let fx = Fx::new();
    let remote = fx.env.bare_remote("office-org/api");
    let seed = fx.env.repos().join("seed");
    let seed_s = seed.to_string_lossy().to_string();
    fx.env.git(
        &["clone", "-q", &remote.to_string_lossy(), &seed_s],
        fx.env.root(),
    );
    fx.env.commit_as(&seed, "base.txt", OFFICE_NAME, OFFICE);
    fx.env.git(&["push", "-q", "origin", "main"], &seed);
    std::fs::remove_dir_all(&fx.repo).unwrap();
    let dest = fx.repo.to_string_lossy().to_string();
    fx.env.git(
        &["clone", "-q", "--single-branch", &file_url(&remote), &dest],
        fx.env.root(),
    );
    fx.git(&["config", "githop.profile", "office"]);
    fx.git(&["checkout", "-q", "-b", "feature"]);
    fx
}

/// Review #2 scenario 1: `git push -u origin feature` in a single-branch clone
/// creates no remote-tracking ref. Without a record, Fix must refuse.
#[test]
fn single_branch_clone_unrecorded_push_is_refused() {
    let fx = single_branch_fixture();
    let pushed = fx.wrong();
    fx.git(&["push", "-q", "-u", "origin", "feature"]);
    assert!(!fx.git_ok(&["rev-parse", "--verify", "-q", "origin/feature"]));

    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 1, "still listed for the UI");
    assert_eq!(plan.blocker, Some(FixBlocker::PushedUnknown));
    let reason = plan.blocked_reason.unwrap();
    assert!(
        reason.contains("git remote set-branches --add origin feature"),
        "{reason}"
    );
    assert!(refused(fx.apply()).contains("can't tell"));
    assert_eq!(fx.head(), pushed);
    assert!(fx.githop_refs().is_empty());
}

/// Same clone, but the push went through GitHop's pre-push check: the record
/// marks it as pushed, later commits are fixable, and the result fast-forwards.
#[test]
fn single_branch_clone_recorded_push_is_never_rewritten() {
    let fx = single_branch_fixture();
    let pushed = fx.wrong();
    push_through_hook(&fx, "origin", "feature");
    fx.git(&["config", "branch.feature.remote", "origin"]);
    fx.git(&["config", "branch.feature.merge", "refs/heads/feature"]);
    let local = fx.wrong();

    let plan = fx.plan();
    assert_eq!(plan.blocked_reason, None);
    assert_eq!(plan.offenders.len(), 1);
    assert_eq!(plan.offenders[0].sha, local);
    assert_eq!(plan.pushed_offenders.len(), 1);
    assert_eq!(plan.pushed_offenders[0].sha, pushed);
    let res = fx.apply().unwrap();
    assert_eq!(res.recreated, 1);
    assert_eq!(
        fx.git(&["rev-parse", "HEAD~1"]),
        pushed,
        "pushed commit kept"
    );
    assert!(res.fast_forward);
    // A plain (non-force) push is accepted.
    fx.git(&["push", "-q", "origin", "feature"]);
}

/// Review #2 scenario 2: a commit published only through a tag.
#[test]
fn commit_published_only_by_a_tag_is_never_rewritten() {
    let fx = Fx::with_remote(1);
    let tagged = fx.wrong();
    fx.git(&["tag", "rel-1"]);
    fx.git(&["push", "-q", "origin", "rel-1"]);
    let plan = fx.plan();
    assert!(plan.offenders.is_empty(), "{:?}", plan.offenders);
    assert_eq!(plan.pushed_offenders.len(), 1);
    assert_eq!(plan.pushed_offenders[0].sha, tagged);

    let local = fx.wrong();
    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 1);
    assert_eq!(plan.offenders[0].sha, local);
    fx.apply().unwrap();
    assert_eq!(fx.git(&["rev-parse", "HEAD~1"]), tagged);
    assert!(fx.git_ok(&["merge-base", "--is-ancestor", "rel-1", "main"]));
}

/// Even a local-only tag counts as pushed (conservative).
#[test]
fn local_tags_count_as_pushed() {
    let fx = Fx::new();
    let tagged = fx.wrong();
    fx.git(&[
        "-c",
        &format!("user.email={OFFICE}"),
        "tag",
        "-a",
        "-m",
        "x",
        "v0",
    ]);
    let plan = fx.plan();
    assert!(plan.offenders.is_empty());
    assert_eq!(plan.pushed_offenders[0].sha, tagged);
}

/// Review #2 scenario 3: a push to a URL leaves no remote-tracking ref; the
/// pre-push record covers it.
#[test]
fn push_to_a_url_is_recorded_and_respected() {
    let fx = Fx::new();
    let remote = fx.env.bare_remote("office-org/api");
    let url = remote.to_string_lossy().replace('\\', "/");
    fx.right();
    let pushed = fx.wrong();
    push_through_hook(&fx, &url, "main");
    let local = fx.wrong();

    let plan = fx.plan();
    assert!(!plan.has_remote_refs);
    assert_eq!(plan.offenders.len(), 1);
    assert_eq!(plan.offenders[0].sha, local);
    assert_eq!(plan.unpushed, 1);
    fx.apply().unwrap();
    assert_eq!(fx.git(&["rev-parse", "HEAD~1"]), pushed);
}

/// Another local branch whose upstream has no tracking ref counts as pushed
/// up to its tip.
#[test]
fn other_branch_with_untracked_upstream_counts_as_pushed() {
    let fx = single_branch_fixture();
    let pushed = fx.wrong();
    fx.git(&["push", "-q", "-u", "origin", "feature"]);
    fx.git(&["checkout", "-q", "-b", "next"]);
    let local = fx.wrong();
    let plan = fx.plan();
    assert_eq!(plan.blocked_reason, None, "next has no upstream");
    assert_eq!(plan.offenders.len(), 1);
    assert_eq!(plan.offenders[0].sha, local);
    fx.apply().unwrap();
    assert_eq!(fx.git(&["rev-parse", "HEAD~1"]), pushed);
}

#[test]
fn shallow_clone_needs_a_tracked_upstream() {
    let fx = Fx::with_remote(3);
    let url = file_url(&fx.env.remotes().join("office-org/api.git"));
    let shallow = fx.env.repos().join("shallow");
    let dest = shallow.to_string_lossy().to_string();
    fx.env
        .git(&["clone", "-q", "--depth", "1", &url, &dest], fx.env.root());
    assert!(shallow.join(".git/shallow").exists());
    fx.env
        .git(&["config", "githop.profile", "office"], &shallow);
    // On main (tracks origin/main): allowed.
    fx.env.commit_as(&shallow, "w.txt", PERSONAL_NAME, PERSONAL);
    let plan = |repo: &Path| fix::plan(&fx.env.git_handle(), repo, &fx.env.paths()).unwrap();
    let p = plan(&shallow);
    assert_eq!(p.offenders.len(), 1);
    assert_eq!(p.blocked_reason, None, "{:?}", p.blocked_reason);
    // A new branch without upstream: refused.
    fx.env.git(&["checkout", "-q", "-b", "topic"], &shallow);
    let p = plan(&shallow);
    assert_eq!(p.blocker, Some(FixBlocker::Shallow));
    assert!(p.blocked_reason.unwrap().contains("git fetch --unshallow"));
}

/// An unpushed merge whose side branch was published only by a tag: the side
/// commit is pushed, so only the merge is rewritten (no unpushed offender can
/// have a pushed descendant).
#[test]
fn merge_of_a_tag_published_branch_keeps_the_pushed_parent() {
    let fx = Fx::with_remote(1);
    fx.git(&["checkout", "-q", "-b", "side"]);
    let side = fx.wrong();
    fx.git(&["tag", "side-1"]);
    fx.git(&["push", "-q", "origin", "side-1"]);
    fx.git(&["checkout", "-q", "main"]);
    fx.git(&["branch", "-q", "-D", "side"]);
    fx.right();
    fx.git(&[
        "-c",
        &format!("user.name={PERSONAL_NAME}"),
        "-c",
        &format!("user.email={PERSONAL}"),
        "merge",
        "--no-ff",
        "-q",
        "-m",
        "merge side",
        "side-1",
    ]);
    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 1);
    assert_eq!(plan.offenders[0].subject, "merge side");
    assert_eq!(plan.rewrite_count, 1);
    fx.apply().unwrap();
    assert_eq!(fx.git(&["rev-parse", "HEAD^2"]), side);
}

/// "You can push now" only when the fixed branch fast-forwards from its remote.
#[test]
fn diverged_branch_is_not_reported_as_ready_to_push() {
    let fx = Fx::with_remote(1);
    fx.git(&["branch", "-q", "--set-upstream-to=origin/main"]);
    let other = fx.env.repos().join("other");
    let remote = fx.env.remotes().join("office-org/api.git");
    let other_s = other.to_string_lossy().to_string();
    fx.env.git(
        &["clone", "-q", &remote.to_string_lossy(), &other_s],
        fx.env.root(),
    );
    fx.env
        .commit_as(&other, "theirs.txt", "Colleague", "colleague@company.com");
    fx.env.git(&["push", "-q", "origin", "main"], &other);
    fx.git(&["fetch", "-q", "origin"]);
    fx.wrong();
    assert!(!fx.apply().unwrap().fast_forward);

    let fx = Fx::with_remote(1);
    fx.git(&["branch", "-q", "--set-upstream-to=origin/main"]);
    fx.wrong();
    assert!(fx.apply().unwrap().fast_forward);
}

// ----------------------------- Review 2026-11-01 #3: other people's commits

const ALICE: &str = "alice@contrib.example";

/// Review #3 scenario: a PR from Alice fetched into a local branch and merged
/// with the wrong identity. Only the merge is the user's; Alice's commit is
/// never listed and keeps its hash and authorship.
#[test]
fn merged_pr_from_someone_else_is_never_reauthored() {
    let fx = Fx::with_remote(1);
    fx.git(&["checkout", "-q", "-b", "pr-1"]);
    let alice = fx.commit_as("Alice", ALICE);
    let alice_ident = fx.show(&alice, "%an|%ae|%ad|%cn|%ce|%cd");
    fx.git(&["checkout", "-q", "main"]);
    fx.git(&[
        "-c",
        &format!("user.name={PERSONAL_NAME}"),
        "-c",
        &format!("user.email={PERSONAL}"),
        "merge",
        "--no-ff",
        "-q",
        "-m",
        "Merge pr-1",
        "pr-1",
    ]);
    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 1, "{:?}", plan.offenders);
    assert_eq!(plan.offenders[0].subject, "Merge pr-1");
    assert_eq!(plan.foreign, 1);
    assert_eq!(plan.rewrite_count, 1);
    fx.apply().unwrap();
    assert_eq!(
        fx.git(&["rev-parse", "HEAD^2"]),
        alice,
        "Alice's commit untouched"
    );
    assert_eq!(fx.show("HEAD^2", "%an|%ae|%ad|%cn|%ce|%cd"), alice_ident);
    assert_eq!(fx.show("HEAD", "%ae %ce"), format!("{OFFICE} {OFFICE}"));
}

/// `git am` / cherry-pick from a fork: Alice's authorship, the user's wrong
/// committer. Only the committer changes.
#[test]
fn foreign_author_with_wrong_committer_keeps_the_author_exactly() {
    let fx = Fx::with_remote(1);
    fx.commit_full(
        ("Alice", ALICE),
        (PERSONAL_NAME, PERSONAL),
        "2026-07-04T10:00:00+0100",
        b"patch from alice\n",
        false,
    );
    let before = fx.show("HEAD", "%an|%ae|%ad");
    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 1);
    assert!(!plan.offenders[0].author_mismatch);
    assert!(plan.offenders[0].committer_mismatch);
    fx.apply().unwrap();
    assert_eq!(fx.show("HEAD", "%an|%ae|%ad"), before);
    assert_eq!(
        fx.show("HEAD", "%cn|%ce"),
        format!("{OFFICE_NAME}|{OFFICE}")
    );
}

/// The global `user.email` is the user's even when no profile uses it.
#[test]
fn global_user_email_is_an_offender() {
    let fx = Fx::with_remote(1);
    fx.env.set_global("user.email", "jane@old-job.example");
    fx.commit_as("Jane", "jane@old-job.example");
    let plan = fx.plan();
    assert_eq!(plan.offenders.len(), 1);
    assert_eq!(plan.foreign, 0);
}
