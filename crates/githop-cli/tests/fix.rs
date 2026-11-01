//! `githop fix` end to end (Phase 4 §4.2). Isolated environment: see
//! `tests/support/mod.rs` (CLAUDE.md rule 3).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};

use serde_json::Value;
use support::TestEnv;

const OFFICE: &str = "me@company.com";
const PERSONAL: &str = "me@personal.dev";

struct Fx {
    env: TestEnv,
    repo: PathBuf,
}

impl Fx {
    /// Office + Personal profiles; `repos/api` pinned to office with one good commit.
    fn new() -> Self {
        let env = TestEnv::new();
        env.add_profile("office", "Me Office", OFFICE, &["office-org"]);
        env.add_profile("personal", "Me", PERSONAL, &["me-personal"]);
        let repo = env.init_repo("api");
        env.git(&["config", "githop.profile", "office"], &repo);
        let fx = Self { env, repo };
        fx.commit("Me Office", OFFICE, "first");
        fx
    }

    /// Like `new`, plus an `origin` bare remote with the first commit pushed.
    fn with_remote() -> Self {
        let fx = Self::new();
        let remote = fx.env.remotes().join("api.git");
        std::fs::create_dir_all(&remote).unwrap();
        fx.env.git(&["init", "-q", "--bare", "-b", "main"], &remote);
        fx.git(&["remote", "add", "origin", &remote.to_string_lossy()]);
        fx.git(&["push", "-q", "origin", "main"]);
        fx
    }

    fn git(&self, args: &[&str]) -> String {
        self.env.git(args, &self.repo)
    }

    fn commit(&self, name: &str, email: &str, msg: &str) -> String {
        let file = format!("{}.txt", msg.replace(' ', "_"));
        std::fs::write(self.repo.join(&file), msg).unwrap();
        self.git(&["add", &file]);
        self.git(&[
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
            "commit",
            "-q",
            "-m",
            msg,
        ]);
        self.head()
    }

    fn wrong(&self, msg: &str) -> String {
        self.commit("Me", PERSONAL, msg)
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        self.env.run(&self.repo, args)
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let (code, out, err) = self.run(&full);
        let v = serde_json::from_str(&out)
            .unwrap_or_else(|e| panic!("bad JSON: {e}\nstdout: {out}\nstderr: {err}"));
        (code, v)
    }

    fn emails(&self) -> String {
        self.git(&["log", "--format=%ae %ce"])
    }
}

fn short(sha: &str) -> &str {
    &sha[..7]
}

fn _unused(_: &Path) {}

#[test]
fn dry_run_lists_offenders_and_changes_nothing() {
    let fx = Fx::with_remote();
    let a = fx.wrong("add billing");
    let b = fx.wrong("fix totals");
    let (code, out, err) = fx.run(&["fix", "--dry-run"]);
    assert_eq!(code, 1, "{out}\n{err}");
    assert!(
        out.contains("! 2 unpushed commits use the Personal email in this Office repo."),
        "{out}"
    );
    assert!(out.contains(short(&a)) && out.contains(short(&b)), "{out}");
    assert!(out.contains("add billing"));
    assert!(out.contains(&format!("{PERSONAL} → {OFFICE}")), "{out}");
    assert!(out.contains("Only unpushed commits are changed. A backup is saved first"));
    assert!(out.contains("Run: githop fix"));
    assert_eq!(fx.head(), b);
}

#[test]
fn yes_fixes_and_prints_backup_and_undo() {
    let fx = Fx::with_remote();
    fx.wrong("add billing");
    fx.wrong("fix totals");
    let (code, out, err) = fx.run(&["fix", "--yes"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("✓ 2 commits fixed. You can push now."),
        "{out}"
    );
    assert!(out.contains("Backup: refs/githop/backup/main/"), "{out}");
    assert!(out.contains("Undo: githop fix --undo"), "{out}");
    for line in fx.emails().lines() {
        assert_eq!(line, format!("{OFFICE} {OFFICE}"));
    }
    // Pushing now passes the identity rule; a second fix has nothing to do.
    let (code, out, _) = fx.run(&["fix"]);
    assert_eq!(code, 0);
    assert!(out.contains("Nothing to fix. All unpushed commits on main use the Office email."));
}

#[test]
fn without_a_terminal_confirmation_is_required() {
    let fx = Fx::new();
    let head = fx.wrong("oops");
    let (code, _, err) = fx.run(&["fix"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("githop fix --yes"), "{err}");
    assert_eq!(fx.head(), head);
}

#[test]
fn undo_restores_the_newest_backup() {
    let fx = Fx::new();
    let head = fx.wrong("oops");
    assert_eq!(fx.run(&["fix", "--yes"]).0, 0);
    assert_ne!(fx.head(), head);
    let (code, out, err) = fx.run(&["fix", "--undo", "--yes"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("This restores refs/githop/backup/main/"),
        "{out}"
    );
    assert!(out.contains("✓ Fix undone."), "{out}");
    assert_eq!(fx.head(), head);

    let (code, out, _) = fx.run(&["fix", "--undo", "--yes"]);
    assert_eq!(code, 1);
    assert!(
        out.contains("There's no fix to undo on this branch."),
        "{out}"
    );
}

#[test]
fn undo_with_an_explicit_ref_and_bad_refs() {
    let fx = Fx::new();
    let head = fx.wrong("oops");
    let (_, v) = fx.json(&["fix", "--yes"]);
    let backup = v["result"]["backup_ref"].as_str().unwrap().to_string();

    let (code, out, _) = fx.run(&["fix", "--undo", "refs/heads/main", "--yes"]);
    assert_eq!(code, 1);
    assert!(out.contains("isn't a GitHop backup"), "{out}");

    let (code, v) = fx.json(&["fix", "--undo", &backup, "--yes"]);
    assert_eq!(code, 0);
    assert_eq!(v["undone"], backup.as_str());
    assert_eq!(fx.head(), head);
}

#[test]
fn undo_is_refused_after_new_commits() {
    let fx = Fx::new();
    fx.wrong("oops");
    assert_eq!(fx.run(&["fix", "--yes"]).0, 0);
    let latest = fx.commit("Me Office", OFFICE, "more work");
    let (code, out, _) = fx.run(&["fix", "--undo", "--yes"]);
    assert_eq!(code, 1);
    assert!(out.contains("changed since the fix"), "{out}");
    assert_eq!(fx.head(), latest);
}

#[test]
fn json_dry_run_matches_the_ipc_shape() {
    let fx = Fx::with_remote();
    let bad = fx.wrong("oops");
    let (code, v) = fx.json(&["fix", "--dry-run"]);
    assert_eq!(code, 1);
    assert!(v["repo"].as_str().unwrap().ends_with("/repos/api"), "{v}");
    assert!(!v["repo"].as_str().unwrap().contains('\\'));
    assert_eq!(v["branch"], "main");
    assert_eq!(v["profile_id"], "office");
    assert_eq!(v["profile_email"], OFFICE);
    assert_eq!(v["offenders"][0]["sha"], bad.as_str());
    assert_eq!(v["offenders"][0]["subject"], "oops");
    assert_eq!(v["offenders"][0]["author_email"], PERSONAL);
    assert_eq!(v["offenders"][0]["author_mismatch"], true);
    assert_eq!(v["pushed_offenders"].as_array().unwrap().len(), 0);
    assert!(v.get("blocked_reason").is_none(), "{v}");
    assert_eq!(v["has_remote_refs"], true);
    assert_eq!(fx.head(), bad);
}

#[test]
fn json_yes_returns_plan_and_result() {
    let fx = Fx::new();
    fx.wrong("oops");
    let (code, v) = fx.json(&["fix", "--yes"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["plan"]["offenders"].as_array().unwrap().len(), 1);
    let r = &v["result"];
    assert_eq!(r["rewritten"], 1);
    let backup = r["backup_ref"].as_str().unwrap();
    assert!(backup.starts_with("refs/githop/backup/main/"));
    assert_eq!(r["undo_hint"], format!("githop fix --undo {backup}"));
    assert_eq!(r["fast_forward"], true);
    assert_eq!(v["plan"]["foreign"], 0);
}

/// Review 2026-11-01 #3: a PR from someone else merged with the wrong identity.
/// Only the merge is fixed; the colleague's commit keeps its id and author.
#[test]
fn someone_elses_commits_are_kept() {
    let fx = Fx::with_remote();
    fx.git(&["checkout", "-q", "-b", "pr-1"]);
    let alice = fx.commit("Alice", "alice@contrib.example", "from alice");
    fx.git(&["checkout", "-q", "main"]);
    fx.git(&[
        "-c",
        "user.name=Me",
        "-c",
        &format!("user.email={PERSONAL}"),
        "merge",
        "--no-ff",
        "-q",
        "-m",
        "merge pr",
        "pr-1",
    ]);
    let (code, out, err) = fx.run(&["fix", "--yes"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("! 1 unpushed commit uses the Personal email in this Office repo."),
        "{out}"
    );
    assert!(
        out.contains("1 commit by someone else keeps its author."),
        "{out}"
    );
    assert!(
        !out.contains(short(&alice)),
        "Alice's commit isn't listed: {out}"
    );
    assert_eq!(fx.git(&["rev-parse", "HEAD^2"]), alice);
    assert_eq!(
        fx.git(&["show", "-s", "--format=%ae", "HEAD^2"]),
        "alice@contrib.example"
    );
}

/// Review 2026-11-01 #2: no "You can push now" when the remote moved on.
#[test]
fn diverged_branch_does_not_say_push_now() {
    let fx = Fx::with_remote();
    fx.git(&["branch", "-q", "--set-upstream-to=origin/main"]);
    let remote = fx.env.remotes().join("api.git");
    let other = fx.env.repos().join("other");
    fx.env.git(
        &[
            "clone",
            "-q",
            &remote.to_string_lossy(),
            &other.to_string_lossy(),
        ],
        fx.env.root(),
    );
    std::fs::write(other.join("theirs.txt"), "x").unwrap();
    fx.env.git(&["add", "theirs.txt"], &other);
    fx.env.git(
        &[
            "-c",
            "user.name=C",
            "-c",
            "user.email=colleague@company.com",
            "commit",
            "-q",
            "-m",
            "theirs",
        ],
        &other,
    );
    fx.env.git(&["push", "-q", "origin", "main"], &other);
    fx.git(&["fetch", "-q", "origin"]);
    fx.wrong("oops");
    let (code, out, err) = fx.run(&["fix", "--yes"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("✓ 1 commit fixed."), "{out}");
    assert!(!out.contains("You can push now"), "{out}");
    assert!(out.contains("a plain push will be rejected"), "{out}");
    assert!(out.contains("git pull --rebase"), "{out}");
}

#[test]
fn json_needs_dry_run_or_yes() {
    let fx = Fx::new();
    let (code, _, err) = fx.run(&["--json", "fix"]);
    assert_eq!(code, 2);
    assert!(err.contains("--dry-run or --yes"), "{err}");
}

#[test]
fn dry_run_and_undo_conflict() {
    let fx = Fx::new();
    assert_eq!(fx.run(&["fix", "--dry-run", "--undo"]).0, 2);
}

#[test]
fn dirty_tree_is_blocked() {
    let fx = Fx::new();
    let head = fx.wrong("oops");
    std::fs::write(fx.repo.join("oops.txt"), "edited").unwrap();
    let (code, out, _) = fx.run(&["fix", "--yes"]);
    assert_eq!(code, 1);
    assert!(
        out.contains("✗ This repo has uncommitted changes. Commit or stash"),
        "{out}"
    );
    assert!(
        out.contains("1 unpushed commit uses the Personal email"),
        "{out}"
    );
    assert_eq!(fx.head(), head);
}

#[test]
fn unresolved_repo_suggests_pin() {
    let fx = Fx::new();
    fx.git(&["config", "--unset", "githop.profile"]);
    fx.wrong("oops");
    let (code, out, _) = fx.run(&["fix", "--yes"]);
    assert_eq!(code, 1);
    assert!(out.contains("githop pin <profile>"), "{out}");
}

#[test]
fn nothing_to_fix_exits_zero() {
    let fx = Fx::new();
    let (code, out, _) = fx.run(&["fix", "--dry-run"]);
    assert_eq!(code, 0);
    assert!(out.contains("✓ Nothing to fix."), "{out}");
}

#[test]
fn pushed_offenders_are_listed_but_not_touched() {
    let fx = Fx::new();
    let remote = fx.env.remotes().join("api.git");
    std::fs::create_dir_all(&remote).unwrap();
    fx.env.git(&["init", "-q", "--bare", "-b", "main"], &remote);
    fx.git(&["remote", "add", "origin", &remote.to_string_lossy()]);
    let pushed = fx.wrong("pushed oops");
    fx.git(&["push", "-q", "origin", "main"]);
    fx.wrong("local oops");
    let (code, out, err) = fx.run(&["fix", "--yes"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("! 1 commit already pushed uses another profile's email. GitHop never changes pushed commits."),
        "{out}"
    );
    assert!(out.contains(short(&pushed)));
    assert!(out.contains("1 commit fixed."), "{out}");
    assert_eq!(fx.git(&["rev-parse", "HEAD~1"]), pushed);
}

#[test]
fn repo_without_remote_says_so() {
    let fx = Fx::new();
    fx.wrong("oops");
    let (_, out, _) = fx.run(&["fix", "--dry-run"]);
    assert!(
        out.contains("This repo has no remote, so all 2 commits were checked."),
        "{out}"
    );
}

#[test]
fn ascii_and_quiet_modes() {
    let fx = Fx::new();
    fx.wrong("oops");
    let out = fx
        .env
        .githop_in(&fx.repo)
        .env("GITHOP_ASCII", "1")
        .args(["fix", "--dry-run"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(&format!("{PERSONAL} -> {OFFICE}")), "{text}");
    assert!(text.contains("[warn]"), "{text}");
}
