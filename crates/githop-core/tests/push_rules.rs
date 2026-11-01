//! Push check rules (Review 2026-11-01 #3, #6, #2 recording): whose commits
//! count as wrong, annotated tags' tagger email, and the pushed-sha record.
//! Isolated git environment and local bare remotes only (CLAUDE.md rule 3).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use githop_core::check::push::{read_pushed, PUSHED_FILE};
use githop_core::check::{self, CheckContext, Level, Outcome, Stage};
use support::TestEnv;

const OFFICE: &str = "harshal@office.example";
const PERSONAL: &str = "harshal@personal.example";
const ALICE: &str = "alice@contrib.example";
const Z40: &str = "0000000000000000000000000000000000000000";

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn write_profiles(env: &TestEnv) {
    let paths = env.paths();
    std::fs::create_dir_all(paths.root()).unwrap();
    let key = |id: &str| slash(&env.home().join(".ssh").join(format!("githop_{id}")));
    std::fs::write(
        paths.profiles_file(),
        format!(
            "version = 1\n\n\
             [[profile]]\nid = \"office\"\nlabel = \"Office\"\nname = \"Harshal Office\"\nemail = \"{OFFICE}\"\ncolor = \"blue\"\nssh_key = \"{}\"\nowners = [\"office-org\"]\n\n\
             [[profile]]\nid = \"personal\"\nlabel = \"Personal\"\nname = \"Harshal\"\nemail = \"{PERSONAL}\"\ncolor = \"violet\"\nssh_key = \"{}\"\nowners = [\"harshal\"]\n",
            key("office"),
            key("personal")
        ),
    )
    .unwrap();
}

struct Fx {
    env: TestEnv,
    repo: PathBuf,
    remote: PathBuf,
    n: std::cell::Cell<u32>,
}

impl Fx {
    /// Office repo (pinned) with a bare `origin` and one pushed office commit.
    fn new() -> Self {
        let env = TestEnv::new();
        write_profiles(&env);
        env.set_global("user.name", "Harshal");
        env.set_global("user.email", "harshal@old-laptop.example");
        let repo = env.init_repo("billing-api");
        let remote = env.bare_remote("office-org/billing-api");
        env.git(&["remote", "add", "origin", &slash(&remote)], &repo);
        env.git(&["config", "githop.profile", "office"], &repo);
        let fx = Self {
            env,
            repo,
            remote,
            n: std::cell::Cell::new(0),
        };
        fx.commit("Harshal Office", OFFICE);
        fx.git(&["push", "-q", "origin", "main"]);
        fx
    }

    fn git(&self, args: &[&str]) -> String {
        self.env.git(args, &self.repo)
    }

    /// New file committed with author = committer = `name <email>`.
    fn commit(&self, name: &str, email: &str) -> String {
        self.commit2((name, email), (name, email))
    }

    fn commit2(&self, author: (&str, &str), committer: (&str, &str)) -> String {
        self.n.set(self.n.get() + 1);
        let file = format!("f{}.txt", self.n.get());
        std::fs::write(self.repo.join(&file), "x\n").unwrap();
        self.git(&["add", &file]);
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.repo);
        self.env.apply(&mut cmd);
        let out = cmd
            .args(["commit", "-q", "-m", &format!("change {}", self.n.get())])
            .env("GIT_AUTHOR_NAME", author.0)
            .env("GIT_AUTHOR_EMAIL", author.1)
            .env("GIT_COMMITTER_NAME", committer.0)
            .env("GIT_COMMITTER_EMAIL", committer.1)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        self.git(&["rev-parse", "HEAD"])
    }

    fn ctx(&self) -> CheckContext {
        CheckContext {
            git: self.env.git_handle(),
            cwd: self.repo.clone(),
            paths: self.env.paths(),
        }
    }

    fn args(&self) -> Vec<String> {
        vec!["origin".to_string(), slash(&self.remote)]
    }

    fn origin_main(&self) -> String {
        self.git(&["rev-parse", "origin/main"])
    }

    fn push_main(&self) -> check::CheckResult {
        let stdin = format!(
            "refs/heads/main {} refs/heads/main {}\n",
            self.git(&["rev-parse", "HEAD"]),
            self.origin_main()
        );
        check::run_check(Stage::Push, &self.args(), &stdin, &self.ctx())
    }

    /// Alice's commit on a local `pr-1` branch off main, like
    /// `git fetch origin pull/1/head:pr-1`. Returns its sha.
    fn alice_pr(&self) -> String {
        self.git(&["checkout", "-q", "-b", "pr-1"]);
        let sha = self.commit("Alice", ALICE);
        self.git(&["checkout", "-q", "main"]);
        sha
    }

    fn merge_pr_as(&self, email: &str) {
        self.git(&[
            "-c",
            "user.name=Harshal",
            "-c",
            &format!("user.email={email}"),
            "merge",
            "--no-ff",
            "-q",
            "-m",
            "Merge pr-1",
            "pr-1",
        ]);
    }

    fn record(&self) -> Vec<githop_core::check::push::PushedEntry> {
        read_pushed(&self.repo.join(".git")).unwrap()
    }
}

// ------------------------------------------------------------ #3 whose commits

/// Review #3 scenario: reviewing a PR locally and merging it with the right
/// identity must not block on Alice's commit.
#[test]
fn merged_pr_from_someone_else_is_not_blocked() {
    let fx = Fx::new();
    fx.alice_pr();
    fx.merge_pr_as(OFFICE);
    let r = fx.push_main();
    assert_eq!(r.outcome, Outcome::Pass, "{:?}", r.messages);
    assert!(r.offending.is_empty());
}

#[test]
fn merge_with_the_wrong_identity_blocks_only_the_merge() {
    let fx = Fx::new();
    let alice = fx.alice_pr();
    fx.merge_pr_as(PERSONAL);
    let merge = fx.git(&["rev-parse", "HEAD"]);
    let r = fx.push_main();
    assert_eq!(r.exit_code(), 1, "{r:?}");
    let shas: Vec<&str> = r.offending.iter().map(|o| o.sha.as_str()).collect();
    assert_eq!(shas, [merge.as_str()]);
    assert!(!shas.contains(&alice.as_str()));
    assert_eq!(
        r.messages[0].text,
        "GitHop: push blocked — 1 commit uses the Personal email in an Office repo."
    );
}

/// `git am` / cherry-pick: Alice's authorship, the user's wrong committer.
#[test]
fn foreign_author_with_the_users_wrong_committer_is_committer_only() {
    let fx = Fx::new();
    fx.commit2(("Alice", ALICE), ("Harshal", PERSONAL));
    let r = fx.push_main();
    assert_eq!(r.exit_code(), 1);
    assert_eq!(r.offending.len(), 1);
    assert!(!r.offending[0].author_mismatch);
    assert!(r.offending[0].committer_mismatch);
    // Alice also as committer: not the user's → passes.
    let fx = Fx::new();
    fx.commit2(("Alice", ALICE), ("Alice", ALICE));
    assert_eq!(fx.push_main().outcome, Outcome::Pass);
}

/// Global `user.email` (not a profile) and git's auto-detected ident are the
/// user's own, so they still block.
#[test]
fn global_email_and_auto_detected_ident_are_the_users() {
    let fx = Fx::new();
    fx.commit("Harshal", "Harshal@Old-Laptop.example");
    let r = fx.push_main();
    assert_eq!(r.exit_code(), 1, "{r:?}");
    assert_eq!(
        r.messages[0].text,
        "GitHop: push blocked — 1 commit doesn't use the Office email."
    );

    let fx = Fx::new();
    fx.commit("Harshal", "hdev@buildbox7.corp.example");
    let mut ctx = fx.ctx();
    ctx.git = ctx
        .git
        .env("USERNAME", "hdev")
        .env("COMPUTERNAME", "BUILDBOX7");
    let stdin = format!(
        "refs/heads/main {} refs/heads/main {}\n",
        fx.git(&["rev-parse", "HEAD"]),
        fx.origin_main()
    );
    let r = check::run_check(Stage::Push, &fx.args(), &stdin, &ctx);
    assert_eq!(r.exit_code(), 1, "{r:?}");
    // Same email on another machine name: someone else's.
    ctx.git = ctx.git.env("COMPUTERNAME", "OTHERBOX");
    let r = check::run_check(Stage::Push, &fx.args(), &stdin, &ctx);
    assert_eq!(r.outcome, Outcome::Pass, "{r:?}");
}

/// The email in `gitconfig.original` (the identity before GitHop) is the user's.
#[test]
fn identity_from_gitconfig_original_is_the_users() {
    let fx = Fx::new();
    let backups = fx.env.paths().backups_dir();
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::write(
        backups.join("gitconfig.original"),
        "[user]\n\tname = H\n\temail = h@before-githop.example\n",
    )
    .unwrap();
    fx.commit("H", "h@before-githop.example");
    assert_eq!(fx.push_main().exit_code(), 1);
}

// ------------------------------------------------------------ #6 annotated tags

fn tag_stdin(fx: &Fx, tag: &str) -> String {
    format!(
        "refs/tags/{tag} {} refs/tags/{tag} {Z40}\n",
        fx.git(&["rev-parse", &format!("refs/tags/{tag}")])
    )
}

#[test]
fn annotated_tag_with_the_wrong_tagger_is_blocked() {
    let fx = Fx::new();
    fx.git(&[
        "-c",
        "user.name=Harshal",
        "-c",
        &format!("user.email={PERSONAL}"),
        "tag",
        "-a",
        "v1",
        "-m",
        "release",
    ]);
    let ctx = fx.ctx();
    let before = ctx.git.spawn_count();
    let r = check::run_check(Stage::Push, &fx.args(), &tag_stdin(&fx, "v1"), &ctx);
    assert_eq!(ctx.git.spawn_count() - before, 3, "config + log + tags");
    assert_eq!(r.exit_code(), 1, "{r:?}");
    assert!(r.offending.is_empty(), "the tagged commit itself is fine");
    assert_eq!(r.messages[0].level, Level::Blocked);
    assert_eq!(
        r.messages[0].text,
        "GitHop: push blocked — tag v1 uses the Personal email in an Office repo."
    );
    assert!(r.messages.iter().any(|m| m.text.contains(PERSONAL)));
    assert_eq!(
        r.messages.last().unwrap().hint.as_deref(),
        Some("Run: git tag -f -a v1 v1^{}")
    );
    assert!(fx.record().is_empty(), "a blocked push isn't recorded");
}

#[test]
fn good_lightweight_and_foreign_tags_pass() {
    let fx = Fx::new();
    fx.git(&["tag", "light"]);
    fx.git(&[
        "-c",
        &format!("user.email={OFFICE}"),
        "tag",
        "-a",
        "v2",
        "-m",
        "ok",
    ]);
    fx.git(&[
        "-c",
        &format!("user.email={ALICE}"),
        "tag",
        "-a",
        "v3",
        "-m",
        "alice",
    ]);
    for tag in ["light", "v2", "v3"] {
        let r = check::run_check(Stage::Push, &fx.args(), &tag_stdin(&fx, tag), &fx.ctx());
        assert_eq!(r.outcome, Outcome::Pass, "{tag}: {:?}", r.messages);
    }
    // A branch-only push keeps the 2-spawn budget.
    fx.commit("Harshal Office", OFFICE);
    let ctx = fx.ctx();
    let before = ctx.git.spawn_count();
    assert_eq!(
        check::run_check(
            Stage::Push,
            &fx.args(),
            &format!(
                "refs/heads/main {} refs/heads/main {}\n",
                fx.git(&["rev-parse", "HEAD"]),
                fx.origin_main()
            ),
            &ctx
        )
        .outcome,
        Outcome::Pass
    );
    assert_eq!(ctx.git.spawn_count() - before, 2);
}

/// A branch and its tag in one push (`git push --follow-tags`): both reported.
#[test]
fn wrong_tag_and_wrong_commit_both_reported() {
    let fx = Fx::new();
    let bad = fx.commit("Harshal", PERSONAL);
    fx.git(&[
        "-c",
        &format!("user.email={PERSONAL}"),
        "tag",
        "-a",
        "v4",
        "-m",
        "r",
    ]);
    let stdin = format!(
        "refs/heads/main {bad} refs/heads/main {}\n{}",
        fx.origin_main(),
        tag_stdin(&fx, "v4")
    );
    let r = check::run_check(Stage::Push, &fx.args(), &stdin, &fx.ctx());
    assert_eq!(r.exit_code(), 1);
    assert_eq!(r.offending.len(), 1);
    let blocked: Vec<&str> = r
        .messages
        .iter()
        .filter(|m| m.level == Level::Blocked)
        .map(|m| m.text.as_str())
        .collect();
    assert_eq!(blocked.len(), 2, "{blocked:?}");
    assert!(blocked[1].contains("tag v4"));
}

// ------------------------------------------------------------ #2 recording

#[test]
fn allowed_push_is_recorded_in_the_common_git_dir() {
    let fx = Fx::new();
    let good = fx.commit("Harshal Office", OFFICE);
    assert_eq!(fx.push_main().outcome, Outcome::Pass);
    let rec = fx.record();
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].sha, good);
    assert_eq!(rec[0].remote_ref, "refs/heads/main");
    assert_eq!(rec[0].remote, "origin");
    assert!(fx.repo.join(".git").join(PUSHED_FILE).is_file());

    // Blocked → not recorded; warn mode → goes through → recorded.
    let bad = fx.commit("Harshal", PERSONAL);
    assert_eq!(fx.push_main().exit_code(), 1);
    assert_eq!(fx.record().len(), 1);
    std::fs::write(
        fx.env.paths().settings_file(),
        "version = 1\nenforce = \"warn\"\n",
    )
    .unwrap();
    let r = fx.push_main();
    assert_eq!(r.exit_code(), 0);
    assert!(matches!(r.outcome, Outcome::Mismatch { .. }));
    assert_eq!(fx.record().last().unwrap().sha, bad);
}

#[test]
fn unresolved_push_is_recorded_too() {
    let fx = Fx::new();
    fx.git(&["config", "--unset", "githop.profile"]);
    fx.git(&[
        "remote",
        "set-url",
        "origin",
        "https://example.invalid/x/y.git",
    ]);
    let c = fx.commit("Harshal", PERSONAL);
    let stdin = format!("refs/heads/topic {c} refs/heads/topic {Z40}\n");
    let r = check::run_check(Stage::Push, &fx.args(), &stdin, &fx.ctx());
    assert_eq!(r.outcome, Outcome::Unresolved);
    assert_eq!(fx.record()[0].sha, c);
}
