//! `githop remote ssh|undo|policy`, and the HTTPS policy on `pin`, `scan
//! --apply` and `status` (ADR 0006). Isolated env, no network.

mod support;

use predicates::str::contains;
use support::TestEnv;

const OFFICE_EMAIL: &str = "me@company.com";
const PERSONAL_EMAIL: &str = "12345+me@users.noreply.github.com";

fn setup(env: &TestEnv) {
    env.add_profile("office", "Me (Work)", OFFICE_EMAIL, &["office-org"]);
    env.add_profile("personal", "Me", PERSONAL_EMAIL, &["janedoe"]);
}

fn known_hosts(env: &TestEnv) -> String {
    std::fs::read_to_string(env.ssh_dir().join("known_hosts")).unwrap_or_default()
}

#[test]
fn pin_under_ask_without_a_terminal_warns_and_keeps_https() {
    let env = TestEnv::new();
    setup(&env);
    let repo = env.repo_with_origin("GitHop", "janedoe");
    env.githop_in(&repo)
        .args(["pin", "personal"])
        .assert()
        .code(0)
        .stdout(contains("Pinned to Personal"))
        .stdout(contains("Uses HTTPS: Personal's SSH key isn't used"))
        .stdout(contains("Run: githop remote ssh"));
    assert_eq!(
        env.get_local(&repo, "remote.origin.url").as_deref(),
        Some("https://github.com/janedoe/GitHop.git")
    );
    // Host keys are trusted in the temp home right away.
    assert!(githop_core::ssh::known_hosts_trusts_github(&known_hosts(
        &env
    )));
    // status shows the same problem and fix.
    env.githop_in(&repo)
        .arg("status")
        .assert()
        .code(0)
        .stdout(contains("Uses HTTPS: Personal's SSH key isn't used"))
        .stdout(contains("Run: githop remote ssh  (origin)"));
}

#[test]
fn pin_under_auto_switches_and_undo_restores() {
    let env = TestEnv::new();
    setup(&env);
    env.githop()
        .args(["remote", "policy", "auto"])
        .assert()
        .code(0)
        .stdout(contains("switch GitHub HTTPS remotes to SSH when pinning"));
    let repo = env.repo_with_origin("GitHop", "janedoe");
    env.githop_in(&repo)
        .args(["pin", "personal"])
        .assert()
        .code(0)
        .stdout(contains(
            "Switched origin to SSH: git@github.com:janedoe/GitHop.git",
        ))
        .stdout(contains("Pushes now use Personal's SSH key."));
    assert_eq!(
        env.get_local(&repo, "remote.origin.url").as_deref(),
        Some("git@github.com:janedoe/GitHop.git")
    );
    env.githop_in(&repo)
        .args(["remote", "undo"])
        .assert()
        .code(0)
        .stdout(contains("origin uses its HTTPS URL again"));
    assert_eq!(
        env.get_local(&repo, "remote.origin.url").as_deref(),
        Some("https://github.com/janedoe/GitHop.git")
    );
    env.githop_in(&repo)
        .args(["remote", "undo"])
        .assert()
        .code(0)
        .stdout(contains("nothing to undo"));
}

#[test]
fn remote_ssh_in_a_repo_and_all_in_a_folder() {
    let env = TestEnv::new();
    setup(&env);
    let a = env.repo_with_origin("api", "office-org");
    let b = env.repo_with_origin("site", "janedoe");
    let lab = env.init_repo("lab");
    env.git(
        &["remote", "add", "origin", "https://gitlab.com/me/lab.git"],
        &lab,
    );

    env.githop_in(&a)
        .args(["remote", "ssh"])
        .assert()
        .code(0)
        .stdout(contains(
            "Switched origin to SSH: git@github.com:office-org/api.git",
        ))
        .stdout(contains("isn't pinned"));
    assert!(githop_core::ssh::known_hosts_trusts_github(&known_hosts(
        &env
    )));

    env.githop()
        .args(["remote", "ssh", "--all-in", &TestEnv::slash(&env.repos())])
        .assert()
        .code(0)
        .stdout(contains("site: switched origin to SSH"));
    assert_eq!(
        env.get_local(&b, "remote.origin.url").as_deref(),
        Some("git@github.com:janedoe/site.git")
    );
    // Non-GitHub remotes are never touched.
    assert_eq!(
        env.get_local(&lab, "remote.origin.url").as_deref(),
        Some("https://gitlab.com/me/lab.git")
    );
    env.githop_in(&lab)
        .args(["remote", "ssh"])
        .assert()
        .code(0)
        .stdout(contains("nothing to switch"));
}

#[test]
fn scan_apply_switches_under_auto_and_hints_under_ask() {
    let env = TestEnv::new();
    setup(&env);
    let a = env.repo_with_origin("api", "office-org");
    let folder = TestEnv::slash(&env.repos());
    env.githop()
        .args(["scan", &folder, "--apply"])
        .assert()
        .code(0)
        .stdout(contains("1 pinned repo pushes to GitHub over HTTPS"))
        .stdout(contains(format!(
            "Run: githop remote ssh --all-in {folder}"
        )));
    assert_eq!(
        env.get_local(&a, "remote.origin.url").as_deref(),
        Some("https://github.com/office-org/api.git")
    );

    let env = TestEnv::new();
    setup(&env);
    let a = env.repo_with_origin("api", "office-org");
    env.githop()
        .args(["remote", "policy", "auto"])
        .assert()
        .code(0);
    env.githop()
        .args(["scan", &TestEnv::slash(&env.repos()), "--apply"])
        .assert()
        .code(0)
        .stdout(contains("Switched 1 repo from HTTPS to SSH"));
    assert_eq!(
        env.get_local(&a, "remote.origin.url").as_deref(),
        Some("git@github.com:office-org/api.git")
    );
}

#[test]
fn policy_shows_and_rejects_unknown_values() {
    let env = TestEnv::new();
    setup(&env);
    env.githop()
        .args(["remote", "policy"])
        .assert()
        .code(0)
        .stdout(contains("ask: ask before switching"));
    env.githop()
        .args(["remote", "policy", "always"])
        .assert()
        .code(2);
    env.githop()
        .args(["remote", "policy", "never"])
        .assert()
        .code(0);
    let repo = env.repo_with_origin("GitHop", "janedoe");
    env.githop_in(&repo)
        .args(["pin", "personal"])
        .assert()
        .code(0)
        .stdout(contains("Uses HTTPS"));
    assert_eq!(
        env.get_local(&repo, "remote.origin.url").as_deref(),
        Some("https://github.com/janedoe/GitHop.git")
    );
}
