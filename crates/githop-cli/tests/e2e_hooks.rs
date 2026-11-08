//! End-to-end: the *built* `githop` binary installed as the hooks of an
//! isolated global git config, driven by real `git` (and Git for Windows' `sh`
//! on Windows). Phase 3 §6 / §7 acceptance tests.
//!
//! Every process gets the isolated env from `support` (temp HOME, temp
//! GIT_CONFIG_GLOBAL, GIT_CONFIG_NOSYSTEM=1, temp GITHOP_HOME,
//! GITHOP_NO_REGISTRY=1) — nothing here touches the real machine config.

//!
//! Windows only: v1 ships for Windows, where Git for Windows' `sh` runs hooks
//! regardless of file mode. (On Unix, core `install_hooks` doesn't set the
//! executable bit yet, so git would skip the hooks.)

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use support::TestEnv;

const OFFICE: &str = "jane@office.example";
const PERSONAL: &str = "jane@personal.example";

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// git with the isolated env plus `vars`.
fn git_env(env: &TestEnv, args: &[&str], cwd: &Path, vars: &[(&str, &str)]) -> Output {
    let mut c = Command::new("git");
    c.args(args).current_dir(cwd);
    env.apply(&mut c);
    for (k, v) in vars {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn built_githop() -> String {
    TestEnv::slash(Path::new(env!("CARGO_BIN_EXE_githop")))
}

/// Personal is the global identity and default profile; Office owns
/// `office-org`. Hooks installed by `githop init` with the built binary.
fn setup() -> TestEnv {
    let env = TestEnv::new();
    env.set_global("user.name", "Jane");
    env.set_global("user.email", PERSONAL);
    let (code, out, err) = env.run(
        &env.home(),
        &["init", "--id", "personal", "--label", "Personal", "--yes"],
    );
    assert_eq!(code, 0, "init failed:\n{out}\n{err}");
    assert!(out.contains("Hooks installed"), "{out}");
    env.add_profile("office", "Jane Office", OFFICE, &["office-org"]);
    env
}

fn hooks_dir(env: &TestEnv) -> String {
    TestEnv::slash(&env.appdata().join("hooks"))
}

fn stage(env: &TestEnv, repo: &Path, file: &str) {
    std::fs::write(repo.join(file), format!("{file}\n")).unwrap();
    env.git(&["add", file], repo);
}

fn commit(env: &TestEnv, repo: &Path, msg: &str) -> Output {
    git_env(env, &["commit", "-q", "-m", msg], repo, &[])
}

fn settings(env: &TestEnv) -> String {
    std::fs::read_to_string(env.appdata().join("settings.toml")).unwrap_or_default()
}

#[test]
fn init_installs_hooks_with_the_built_binary() {
    let env = setup();
    assert_eq!(
        env.get_global("core.hooksPath").as_deref(),
        Some(hooks_dir(&env).as_str())
    );
    let pre_commit =
        std::fs::read_to_string(env.appdata().join("hooks").join("pre-commit")).unwrap();
    assert!(
        pre_commit.contains(&format!("GITHOP=\"{}\"", built_githop())),
        "{pre_commit}"
    );
    assert!(!pre_commit.contains('\r'));
    // Original identity untouched by init.
    assert_eq!(env.get_global("user.email").as_deref(), Some(PERSONAL));

    // Rerun is idempotent and repairs a hand-edited hook.
    std::fs::write(
        env.appdata().join("hooks").join("pre-commit"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    let (code, out, _) = env.run(&env.home(), &["init", "--yes"]);
    assert_eq!(code, 0);
    // Regression D11: say what install did.
    assert!(
        out.contains("already set up") && out.contains("Hooks repaired: pre-commit."),
        "{out}"
    );
    let (code, out, _) = env.run(&env.home(), &["hooks", "install"]);
    assert_eq!(code, 0);
    assert!(out.contains("Hooks are up to date."), "{out}");
    env.run(&env.home(), &["hooks", "uninstall"]);
    let (code, out, _) = env.run(&env.home(), &["hooks", "install", "--yes"]);
    assert_eq!(code, 0);
    assert!(out.contains("Hooks installed."), "{out}");
    let (code, _, _) = env.run(&env.home(), &["hooks", "status"]);
    assert_eq!(code, 0, "hooks healthy after rerun");
}

#[test]
fn plan_section_1_commit_blocked_then_rerun_passes_with_office_email() {
    let env = setup();
    let repo = env.repo_with_origin("billing-api", "office-org");
    stage(&env, &repo, "fix.txt");

    let out = commit(&env, &repo, "fix office bug");
    assert!(!out.status.success(), "commit must be blocked");
    let err = stderr(&out);
    for line in [
        "✗ GitHop: commit blocked",
        "This repo belongs to Office.",
        &format!("Commit would use {PERSONAL}."),
    ] {
        assert!(err.contains(line), "missing {line:?} in:\n{err}");
    }
    // Pinned now + "Run the commit again." (exact wording: see the test below).
    assert!(err.contains("to Office."), "{err}");
    assert!(err.contains("Run the commit again."), "{err}");
    assert!(
        !err.contains("\x1b["),
        "no colour when stderr isn't a terminal"
    );
    let none = git_env(&env, &["rev-parse", "--verify", "-q", "HEAD"], &repo, &[]);
    assert!(!none.status.success(), "no commit was made");
    assert_eq!(
        env.get_local(&repo, "githop.profile").as_deref(),
        Some("office")
    );

    let out = commit(&env, &repo, "fix office bug");
    assert!(out.status.success(), "rerun passes:\n{}", stderr(&out));
    assert_eq!(
        env.git(&["log", "-1", "--format=%ae %ce"], &repo),
        format!("{OFFICE} {OFFICE}")
    );
}

/// `git commit` exports GIT_AUTHOR_NAME/EMAIL/DATE to its hooks with the
/// configured identity; that must not be read as a user override (C9).
#[test]
fn plan_section_1_exact_design_wording_under_a_real_hook() {
    let env = setup();
    let repo = env.repo_with_origin("billing-api", "office-org");
    stage(&env, &repo, "fix.txt");
    let err = stderr(&commit(&env, &repo, "fix office bug"));
    assert!(
        err.contains("✓ Identity fixed for this repo: pinned to Office.\nRun the commit again."),
        "{err}"
    );
    assert!(!err.contains("GIT_AUTHOR_EMAIL is set"), "{err}");
    assert!(!err.contains("from --author or GIT_AUTHOR_EMAIL"), "{err}");
}

#[test]
fn push_of_a_wrong_email_commit_is_blocked_listing_it() {
    let env = setup();
    env.redirect_github_to_local();
    let remote = env.bare_remote("office-org/billing-api");
    let repo = env.repo_with_origin("billing-api", "office-org");
    env.githop_in(&repo)
        .args(["pin", "office"])
        .assert()
        .code(0);

    let first = env.commit_as(&repo, "a.txt", OFFICE);
    let out = git_env(&env, &["push", "-q", "-u", "origin", "main"], &repo, &[]);
    assert!(out.status.success(), "clean push passes:\n{}", stderr(&out));

    let good = env.commit_as(&repo, "b.txt", OFFICE);
    let bad = env.commit_as(&repo, "c.txt", PERSONAL);
    env.commit_as(&repo, "d.txt", OFFICE);
    let out = git_env(&env, &["push", "-q", "origin", "main"], &repo, &[]);
    assert!(!out.status.success(), "push must be blocked");
    let err = stderr(&out);
    assert!(err.contains("✗ GitHop: push blocked"), "{err}");
    assert!(err.contains("1 commit uses the Personal email"), "{err}");
    assert!(err.contains(&bad[..7]), "lists the bad commit:\n{err}");
    assert!(!err.contains(&good[..7]), "only the bad commit:\n{err}");
    assert!(err.contains(PERSONAL));
    assert!(err.contains("Run: githop fix"), "{err}");
    let remote_head = env.git(&["rev-parse", "main"], &remote);
    assert_eq!(remote_head, first, "remote unchanged");

    // New branch path: same commit is still caught.
    let out = git_env(&env, &["push", "-q", "origin", "main:topic"], &repo, &[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains(&bad[..7]));
}

#[test]
fn clone_through_insteadof_auto_pins_to_office() {
    let env = setup();
    env.redirect_github_to_local();
    env.bare_remote("office-org/x");
    // Seed the remote (setup only: skip hooks).
    let seed = env.init_repo("seed");
    env.commit_as(&seed, "readme.txt", OFFICE);
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/office-org/x.git",
        ],
        &seed,
    );
    env.git(&["push", "-q", "--no-verify", "origin", "main"], &seed);

    let dest = env.repos().join("x");
    let out = git_env(
        &env,
        &[
            "clone",
            "-q",
            "https://github.com/office-org/x.git",
            &TestEnv::slash(&dest),
        ],
        &env.repos(),
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("✓ GitHop: pinned to Office"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        env.get_local(&dest, "githop.profile").as_deref(),
        Some("office")
    );
    assert_eq!(env.get_local(&dest, "user.email").as_deref(), Some(OFFICE));
    assert!(
        settings(&env).contains("/repos/x/"),
        "recorded in pinned_repos"
    );
}

#[test]
fn repo_own_pre_commit_hook_still_runs_and_can_block() {
    let env = setup();
    let repo = env.init_repo("side"); // no remote → default profile (Personal) → match
    let hook = repo.join(".git").join("hooks").join("pre-commit");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho ran >> .git/marker\nexit ${REPO_RC:-0}\n",
    )
    .unwrap();

    stage(&env, &repo, "a.txt");
    let out = commit(&env, &repo, "a");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(repo.join(".git").join("marker").is_file(), "repo hook ran");

    stage(&env, &repo, "b.txt");
    let out = git_env(
        &env,
        &["commit", "-q", "-m", "b"],
        &repo,
        &[("REPO_RC", "1")],
    );
    assert!(!out.status.success(), "repo hook failure still blocks");
    assert_eq!(env.git(&["rev-list", "--count", "HEAD"], &repo), "1");
}

#[test]
fn corrupt_profiles_let_the_commit_through_with_a_warning() {
    let env = setup();
    let repo = env.repo_with_origin("billing-api", "office-org");
    std::fs::write(env.appdata().join("profiles.toml"), "[[[ this is not toml").unwrap();
    stage(&env, &repo, "a.txt");
    let out = commit(&env, &repo, "a");
    assert!(out.status.success(), "fail open:\n{}", stderr(&out));
    assert!(
        stderr(&out).contains("! GitHop couldn't check this commit (it went through)."),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("Run: githop doctor"));
}

#[test]
fn githop_disable_skips_the_check() {
    let env = setup();
    let repo = env.repo_with_origin("billing-api", "office-org");
    stage(&env, &repo, "a.txt");
    let out = git_env(
        &env,
        &["commit", "-q", "-m", "a"],
        &repo,
        &[("GITHOP_DISABLE", "1")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stderr(&out).contains("GitHop"), "{}", stderr(&out));
    assert_eq!(
        env.get_local(&repo, "githop.profile"),
        None,
        "nothing pinned"
    );
    assert_eq!(env.git(&["log", "-1", "--format=%ae"], &repo), PERSONAL);
}

#[test]
fn wipe_restores_previous_hooks_path_and_original_identity_exactly() {
    let env = TestEnv::new();
    env.set_global("user.name", "Original Name");
    env.set_global("user.email", "original@example.com");
    env.set_global("alias.st", "status");
    let prev = env.root().join("my hooks");
    std::fs::create_dir_all(&prev).unwrap();
    std::fs::write(
        prev.join("pre-commit"),
        "#!/bin/sh\necho prev >> .git/prev-marker\n",
    )
    .unwrap();
    let prev_value = TestEnv::slash(&prev);
    env.set_global("core.hooksPath", &prev_value);
    let home = env.home();
    let before = env.git(&["config", "--global", "--list"], &home);

    env.githop()
        .args(["init", "--id", "office", "--yes"])
        .assert()
        .code(0);
    assert_eq!(
        env.get_global("core.hooksPath").as_deref(),
        Some(hooks_dir(&env).as_str())
    );
    env.add_profile("personal", "Me", PERSONAL, &[]);
    env.githop().args(["use", "personal"]).assert().code(0);
    assert_eq!(env.get_global("user.email").as_deref(), Some(PERSONAL));

    // The previous global hook is chained while GitHop is installed.
    let repo = env.init_repo("r");
    stage(&env, &repo, "a.txt");
    let out = commit(&env, &repo, "a");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        repo.join(".git").join("prev-marker").is_file(),
        "previous hook chained"
    );

    let out = env.githop().args(["wipe", "--yes"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}\n{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("core.hooksPath is back to"),
        "{}",
        stdout(&out)
    );
    assert!(stdout(&out).contains("GitHop wiped"));

    assert_eq!(
        env.get_global("core.hooksPath").as_deref(),
        Some(prev_value.as_str())
    );
    assert_eq!(
        env.get_global("user.name").as_deref(),
        Some("Original Name")
    );
    assert_eq!(
        env.get_global("user.email").as_deref(),
        Some("original@example.com")
    );
    assert_eq!(env.get_global("core.sshCommand"), None);
    assert_eq!(env.git(&["config", "--global", "--list"], &home), before);
    assert!(!env.appdata().exists());

    // Running it again is a clean no-op.
    env.githop().args(["wipe", "--yes"]).assert().code(0);
    assert_eq!(
        env.get_global("core.hooksPath").as_deref(),
        Some(prev_value.as_str())
    );
}

#[test]
fn wipe_without_previous_hooks_path_unsets_it_and_json_lists_steps() {
    let env = setup();
    let out = env
        .githop()
        .args(["--json", "wipe", "--from-uninstaller"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let steps: Vec<&str> = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["step"].as_str().unwrap())
        .collect();
    assert_eq!(
        steps,
        [
            "backup",
            "hooks",
            "identity",
            "unpin",
            "autostart",
            "data",
            "keys"
        ]
    );
    assert!(
        v["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["ok"] == true),
        "{v}"
    );
    assert_eq!(env.get_global("core.hooksPath"), None);
    assert_eq!(env.get_global("user.email").as_deref(), Some(PERSONAL));
    assert!(!env.appdata().exists());
}

#[test]
fn wipe_keep_data_keeps_settings_but_removes_hooks() {
    let env = setup();
    env.githop()
        .args(["wipe", "--yes", "--keep-data"])
        .assert()
        .code(0);
    assert_eq!(env.get_global("core.hooksPath"), None);
    assert!(env.appdata().join("profiles.toml").is_file());
    assert!(!env.appdata().join("hooks").join("pre-commit").exists());
    assert!(
        settings(&env).contains("hooks_installed = false"),
        "{}",
        settings(&env)
    );
}

#[test]
fn wipe_with_broken_settings_still_restores_previous_hooks_path() {
    let env = TestEnv::new();
    env.set_global("user.name", "Jane");
    env.set_global("user.email", PERSONAL);
    env.set_global("core.hooksPath", "C:/old-hooks");
    env.githop()
        .args(["init", "--id", "personal", "--yes"])
        .assert()
        .code(0);
    std::fs::write(env.appdata().join("settings.toml"), "= broken").unwrap();
    std::fs::write(env.appdata().join("profiles.toml"), "= broken").unwrap();
    env.githop().args(["wipe", "--yes"]).assert().code(0);
    assert_eq!(
        env.get_global("core.hooksPath").as_deref(),
        Some("C:/old-hooks")
    );
    assert!(!env.appdata().exists());
}

#[test]
fn doctor_reports_hook_problems_and_install_repairs_them() {
    let env = setup();
    let check = |id: &str| -> Vec<Value> {
        let out = env.githop().args(["--json", "doctor"]).output().unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["id"] == id)
            .cloned()
            .collect()
    };
    for id in ["hooks-path", "hooks-files", "hooks-exe", "hooks-sh"] {
        let c = check(id);
        assert!(
            !c.is_empty() && c.iter().all(|c| c["status"] == "ok"),
            "{id}: {c:?}"
        );
    }

    std::fs::write(
        env.appdata().join("hooks").join("pre-push"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    env.set_global("core.hooksPath", "C:/somewhere-else");
    let files = check("hooks-files");
    assert!(
        files.iter().any(|c| c["status"] == "problem"
            && c["detail"].as_str().unwrap().contains("pre-push")
            && c["fix"].as_str().unwrap().contains("githop hooks install")),
        "{files:?}"
    );
    let path = check("hooks-path");
    assert_eq!(path[0]["status"], "problem");
    assert!(path[0]["detail"]
        .as_str()
        .unwrap()
        .contains("C:/somewhere-else"));
    let (code, out, _) = env.run(&env.home(), &["doctor"]);
    assert_eq!(code, 1);
    assert!(out.contains("Fix: Run: githop hooks install"), "{out}");

    env.githop()
        .args(["hooks", "install", "--yes"])
        .assert()
        .code(0);
    assert!(check("hooks-files").iter().all(|c| c["status"] == "ok"));
    assert_eq!(check("hooks-path")[0]["status"], "ok");
    // Previous path recorded at repair is the stray value, so it's chained.
    assert!(
        settings(&env).contains("previous_hooks_path = \"C:/somewhere-else\""),
        "{}",
        settings(&env)
    );
}

#[test]
fn hooks_uninstall_status_and_snippet() {
    let env = setup();
    let (code, out, err) = env.run(&env.home(), &["hooks", "snippet", "commit"]);
    assert_eq!(code, 0);
    assert_eq!(
        out.trim(),
        format!(
            "\"{}\" check --stage commit -- \"$@\"; [ $? -ne 1 ] || exit 1",
            built_githop()
        )
    );
    assert!(err.contains("pre-commit"));

    env.githop().args(["hooks", "uninstall"]).assert().code(0);
    assert_eq!(env.get_global("core.hooksPath"), None);
    let (code, out, _) = env.run(&env.home(), &["hooks", "status"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Hooks aren't installed"), "{out}");
    let out = env
        .githop()
        .args(["--json", "hooks", "status"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["health"]["installed"], false);
}

#[test]
fn doctor_flags_husky_repo_with_snippet() {
    let env = setup();
    let repo = env.init_repo("husky");
    env.git(&["config", "--local", "core.hooksPath", ".husky"], &repo);
    let out = env
        .githop_in(&repo)
        .args(["--json", "doctor"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let hit = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "local-hooks-path")
        .cloned();
    let hit = hit.unwrap_or_else(|| panic!("no local-hooks-path: {v}"));
    assert_eq!(hit["status"], "warning");
    assert!(hit["fix"]
        .as_str()
        .unwrap()
        .contains("check --stage commit"));
}

#[test]
fn doctor_caches_github_ssh_aliases_for_owner_rules() {
    let env = setup();
    env.write_ssh_config(
        "Host github-work\n  HostName github.com\n  IdentityFile ~/.ssh/githop_office\n",
    );
    let _ = env.githop().args(["doctor"]).output().unwrap();
    assert!(settings(&env).contains("github-work"), "{}", settings(&env));

    let repo = env.init_repo("aliased");
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "git@github-work:office-org/aliased.git",
        ],
        &repo,
    );
    let out = env
        .githop_in(&repo)
        .args(["--json", "status"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["resolution"]["profile_id"], "office", "{v}");
}

/// Pinned Office repo with diverged `main` / `feat` (commits made without hooks).
fn office_repo_with_branches(env: &TestEnv) -> std::path::PathBuf {
    let repo = env.repo_with_origin("billing-api", "office-org");
    env.githop_in(&repo)
        .args(["pin", "office"])
        .assert()
        .code(0);
    env.commit_as(&repo, "a.txt", OFFICE);
    env.git(&["checkout", "-q", "-b", "feat"], &repo);
    env.commit_as(&repo, "b.txt", OFFICE);
    env.git(&["checkout", "-q", "main"], &repo);
    env.commit_as(&repo, "c.txt", OFFICE);
    repo
}

/// Review 2026-11-01 #5: merge, revert and cherry-pick commits made with the
/// wrong identity are blocked at commit time, like `git commit`.
#[test]
fn merge_revert_and_cherry_pick_with_the_wrong_identity_are_blocked() {
    let env = setup();
    let repo = office_repo_with_branches(&env);
    let head = env.git(&["rev-parse", "HEAD"], &repo);
    let wrong = format!("user.email={PERSONAL}");

    let out = git_env(
        &env,
        &["-c", &wrong, "merge", "-q", "--no-edit", "feat"],
        &repo,
        &[],
    );
    assert!(!out.status.success(), "merge must be blocked");
    assert!(
        stderr(&out).contains("✗ GitHop: commit blocked"),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.git(&["rev-parse", "HEAD"], &repo), head);
    let _ = git_env(&env, &["merge", "--abort"], &repo, &[]);
    env.git(&["reset", "-q", "--hard"], &repo);

    let out = git_env(
        &env,
        &["-c", &wrong, "revert", "--no-edit", "HEAD"],
        &repo,
        &[],
    );
    assert!(!out.status.success(), "revert must be blocked");
    assert!(
        stderr(&out).contains("✗ GitHop: commit blocked"),
        "{}",
        stderr(&out)
    );
    assert_eq!(env.git(&["rev-parse", "HEAD"], &repo), head);
    env.git(&["reset", "-q", "--hard"], &repo);

    let out = git_env(&env, &["-c", &wrong, "cherry-pick", "feat"], &repo, &[]);
    assert!(!out.status.success(), "cherry-pick must be blocked");
    assert_eq!(env.git(&["rev-parse", "HEAD"], &repo), head);
    let _ = git_env(&env, &["cherry-pick", "--abort"], &repo, &[]);
    env.git(&["reset", "-q", "--hard"], &repo);

    // Right identity: all three go through. Cherry-picking a colleague's
    // commit keeps their authorship and isn't blocked.
    env.git(&["checkout", "-q", "-b", "alice", "main~1"], &repo);
    std::fs::write(repo.join("z.txt"), "z\n").unwrap();
    env.git(&["add", "z.txt"], &repo);
    env.git(
        &[
            "-c",
            "user.name=Alice",
            "-c",
            "user.email=alice@contrib.example",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            "alice",
        ],
        &repo,
    );
    env.git(&["checkout", "-q", "main"], &repo);
    for args in [
        vec!["revert", "--no-edit", "HEAD"],
        vec!["cherry-pick", "alice"],
    ] {
        let out = git_env(&env, &args, &repo, &[]);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
    }
    assert_eq!(
        env.git(&["log", "-1", "--format=%ae %ce"], &repo),
        format!("alice@contrib.example {OFFICE}")
    );
    let out = git_env(&env, &["merge", "-q", "--no-edit", "feat"], &repo, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        env.git(&["log", "-1", "--format=%p"], &repo)
            .split(' ')
            .count(),
        2,
        "a merge commit was made"
    );
}

/// Doctor (JSON) in `cwd`: (id, status) pairs.
fn doctor_ids(env: &TestEnv, cwd: &Path) -> Vec<(String, String)> {
    let out = env
        .githop_in(cwd)
        .args(["--json", "doctor"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["id"].as_str().unwrap().to_string(),
                c["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn hook_problems(ids: &[(String, String)]) -> Vec<&(String, String)> {
    ids.iter()
        .filter(|(id, st)| {
            st == "problem" && (id.starts_with("hooks") || id.contains("hooks-path"))
        })
        .collect()
}

/// Review 2026-11-01 #11: `--author` reaches the hook as GIT_AUTHOR_EMAIL; the
/// message must cover both, not claim the variable was set.
#[test]
fn commit_with_a_wrong_author_flag_names_author_and_env() {
    let env = setup();
    let repo = env.repo_with_origin("billing-api", "office-org");
    env.githop_in(&repo)
        .args(["pin", "office"])
        .assert()
        .code(0);
    stage(&env, &repo, "a.txt");
    let author = format!("--author=Someone <{PERSONAL}>");
    let out = git_env(&env, &["commit", "-q", &author, "-m", "a"], &repo, &[]);
    assert!(!out.status.success(), "blocked");
    let err = stderr(&out);
    assert!(
        err.contains(&format!(
            "This commit's author email is {PERSONAL} (from --author or GIT_AUTHOR_EMAIL)."
        )),
        "{err}"
    );
    assert!(
        err.contains("Drop --author or unset GIT_AUTHOR_EMAIL, then commit again."),
        "{err}"
    );
    assert!(!err.contains("GIT_AUTHOR_EMAIL is set"), "{err}");
}

/// Company hooks through an `includeIf` for `<repos>/work/`. Returns the work dir.
fn add_corp_include_if(env: &TestEnv) -> std::path::PathBuf {
    let corp = env.root().join("corp-hooks");
    std::fs::create_dir_all(&corp).unwrap();
    std::fs::write(
        corp.join("pre-commit"),
        "#!/bin/sh\necho corp >> .git/corp-marker\n",
    )
    .unwrap();
    std::fs::write(
        env.home().join(".gitconfig-work"),
        format!("[core]\n\thooksPath = {}\n", TestEnv::slash(&corp)),
    )
    .unwrap();
    let work = env.repos().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut g = std::fs::read_to_string(env.global_config()).unwrap();
    g.push_str(&format!(
        "[includeIf \"gitdir/i:{}/\"]\n\tpath = ~/.gitconfig-work\n",
        TestEnv::slash(&work)
    ));
    std::fs::write(env.global_config(), g).unwrap();
    work
}

/// Office repo inside the includeIf folder.
fn office_repo_in(env: &TestEnv, dir: &Path) -> std::path::PathBuf {
    let repo = dir.join("billing-api");
    std::fs::create_dir_all(&repo).unwrap();
    env.git(&["init", "-q", "-b", "main"], &repo);
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/office-org/billing-api.git",
        ],
        &repo,
    );
    repo
}

/// Review 2026-11-01 #1, exactly as reproduced: company hooks via includeIf,
/// then `githop init`. A Personal-email commit in an Office repo in that
/// folder is blocked, and the company hook still runs.
#[test]
fn include_if_company_hooks_path_does_not_turn_githop_off() {
    let env = TestEnv::new();
    env.set_global("user.name", "Jane");
    env.set_global("user.email", PERSONAL);
    env.set_global("core.autocrlf", "false");
    let work = add_corp_include_if(&env);
    let (code, out, err) = env.run(
        &env.home(),
        &["init", "--id", "personal", "--label", "Personal", "--yes"],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        format!("{out}{err}").contains("for repos matching gitdir/i:"),
        "{out}\n{err}"
    );
    env.add_profile("office", "Jane Office", OFFICE, &["office-org"]);
    let repo = office_repo_in(&env, &work);
    env.githop_in(&repo)
        .args(["pin", "office"])
        .assert()
        .code(0);

    stage(&env, &repo, "a.txt");
    let wrong = format!("user.email={PERSONAL}");
    let out = git_env(&env, &["-c", &wrong, "commit", "-q", "-m", "a"], &repo, &[]);
    assert!(!out.status.success(), "must be blocked: {}", stderr(&out));
    assert!(stderr(&out).contains("✗ GitHop: commit blocked"));

    let out = commit(&env, &repo, "a");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        repo.join(".git").join("corp-marker").is_file(),
        "company hook ran"
    );

    let ids = doctor_ids(&env, &repo);
    assert!(hook_problems(&ids).is_empty(), "{ids:?}");
    assert!(
        ids.iter()
            .any(|(id, st)| id == "hooks-included" && st == "ok"),
        "{ids:?}"
    );
    let (code, out, _) = env.run(&env.home(), &["hooks", "status"]);
    assert_eq!(code, 0, "{out}");
}

/// An install made before the includeIf existed: `status` and `doctor` in the
/// repo report that GitHop's hooks don't run there; `hooks install` fixes it.
#[test]
fn status_and_doctor_report_an_include_that_overrides_githop() {
    let env = setup();
    let work = add_corp_include_if(&env);
    let repo = office_repo_in(&env, &work);

    let (code, out, err) = env.run(&repo, &["status"]);
    assert_eq!(code, 0);
    let all = format!("{out}{err}");
    assert!(
        all.contains("In this repo, core.hooksPath is") && all.contains("corp-hooks"),
        "{all}"
    );
    let out = env
        .githop_in(&repo)
        .args(["--json", "status"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["hooks_override"]["scope"], "global", "{v}");

    let (code, _, _) = env.run(&repo, &["doctor"]);
    assert_eq!(code, 1);
    let ids = doctor_ids(&env, &repo);
    let has = |id: &str| ids.iter().any(|(i, st)| i == id && st == "problem");
    assert!(has("repo-hooks-path"), "{ids:?}");
    assert!(has("hooks-path"), "{ids:?}");

    env.githop()
        .args(["hooks", "install", "--yes"])
        .assert()
        .code(0);
    let ids = doctor_ids(&env, &repo);
    assert!(hook_problems(&ids).is_empty(), "{ids:?}");
    let (_, out, err) = env.run(&repo, &["status"]);
    assert!(!format!("{out}{err}").contains("In this repo, core.hooksPath"));
    stage(&env, &repo, "a.txt");
    let out = commit(&env, &repo, "a");
    assert!(!out.status.success(), "GitHop checks again (pins + blocks)");
    assert!(stderr(&out).contains("Identity fixed for this repo"));
}

#[cfg(windows)]
#[test]
fn wipe_removes_the_autostart_entry_from_a_test_key() {
    // A throwaway key under HKCU\Software\GitHopTest-* — never the real Run key.
    let key = format!(r"HKCU\Software\GitHopTest-{}", std::process::id());
    let reg = |args: &[&str]| -> i32 {
        Command::new("reg")
            .args(args)
            .output()
            .unwrap()
            .status
            .code()
            .unwrap_or(-1)
    };
    assert_eq!(reg(&["add", &key, "/v", "GitHop", "/d", "x", "/f"]), 0);
    let env = setup();
    let out = env
        .githop()
        .args(["--json", "wipe", "--yes"])
        .env_remove("GITHOP_NO_REGISTRY")
        .env("GITHOP_AUTOSTART_KEY", &key)
        .output()
        .unwrap();
    let gone = reg(&["query", &key, "/v", "GitHop"]);
    let _ = reg(&["delete", &key, "/f"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let step = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step"] == "autostart")
        .cloned()
        .unwrap();
    assert_eq!(step["ok"], true);
    assert!(
        step["detail"]
            .as_str()
            .unwrap()
            .contains("no longer starts"),
        "{step}"
    );
    assert_eq!(gone, 1, "value deleted");
}
