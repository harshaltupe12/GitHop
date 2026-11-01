//! Happy path + main error path for every command (Phase 2 §6).
//!
//! These need the Phase 1 core bodies (store, gitconfig, pin, resolver, ssh, scan),
//! which land in a parallel branch — hence `#[ignore = "needs core merged"]`.
//! After integration: `cargo test -p githop-cli -- --include-ignored`.

mod support;

use predicates::prelude::*;
use predicates::str::contains;
use serde_json::Value;
use support::TestEnv;

const OFFICE_EMAIL: &str = "me@company.com";
const PERSONAL_EMAIL: &str = "12345+me@users.noreply.github.com";

fn json(env: &TestEnv, cwd: &std::path::Path, args: &[&str]) -> (i32, Value) {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let (code, out, err) = env.run(cwd, &full);
    let v: Value = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("bad JSON from {args:?}: {e}\nstdout: {out}\nstderr: {err}"));
    (code, v)
}

fn setup_two_profiles(env: &TestEnv) {
    env.add_profile("office", "Me (Work)", OFFICE_EMAIL, &["office-org"]);
    env.add_profile("personal", "Me", PERSONAL_EMAIL, &["me-personal"]);
}

// ---------------------------------------------------------------- profiles

#[test]
fn add_list_edit_remove_round_trip() {
    let env = TestEnv::new();
    setup_two_profiles(&env);

    let (code, v) = json(&env, &env.home(), &["list"]);
    assert_eq!(code, 0);
    let ids: Vec<&str> = v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["office", "personal"]);
    assert_eq!(v["profiles"][0]["owners"][0], "office-org");
    assert_ne!(v["profiles"][0]["color"], v["profiles"][1]["color"]);

    env.githop()
        .args([
            "edit",
            "personal",
            "--email",
            "999+me@users.noreply.github.com",
        ])
        .assert()
        .code(0)
        .stdout(contains("Profile updated"));
    let (_, v) = json(&env, &env.home(), &["list"]);
    assert_eq!(v["profiles"][1]["email"], "999+me@users.noreply.github.com");

    env.githop()
        .args(["remove", "personal", "--yes"])
        .assert()
        .code(0)
        .stdout(contains("Profile removed: Personal"));
    let (_, v) = json(&env, &env.home(), &["list"]);
    assert_eq!(v["profiles"].as_array().unwrap().len(), 1);
}

#[test]
fn list_with_no_profiles_points_to_init() {
    let env = TestEnv::new();
    env.githop()
        .arg("list")
        .assert()
        .code(0)
        .stdout(contains("No profiles yet"))
        .stdout(contains("githop init"));
}

#[test]
fn add_duplicate_id_is_usage_error() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    env.githop()
        .args(["add", "--id", "office", "--name", "X", "--email", "x@y.z"])
        .assert()
        .code(2)
        .stderr(contains("already exists"));
}

#[test]
fn add_invalid_profile_lists_issues_and_saves_nothing() {
    let env = TestEnv::new();
    env.githop()
        .args([
            "add",
            "--id",
            "Bad Id!",
            "--name",
            "X",
            "--email",
            "not-an-email",
        ])
        .assert()
        .code(2)
        .stderr(contains("isn't valid"))
        .stderr(contains("•"));
    assert!(!env.appdata().join("profiles.toml").exists());
}

#[test]
fn first_profile_needs_no_owner_but_the_second_does() {
    let env = TestEnv::new();
    // One profile: it's used everywhere, no rule needed.
    env.githop()
        .args([
            "add",
            "--id",
            "office",
            "--name",
            "Me",
            "--email",
            OFFICE_EMAIL,
        ])
        .assert()
        .code(0);

    // A second profile with no owner or folder: refused, naming both profiles.
    env.githop()
        .args([
            "add",
            "--id",
            "personal",
            "--name",
            "Me",
            "--email",
            PERSONAL_EMAIL,
        ])
        .assert()
        .code(2)
        .stderr(contains("isn't valid"))
        .stderr(contains("office: owners"))
        .stderr(contains("personal: owners"))
        .stderr(contains("needs a GitHub owner or a folder"))
        .stderr(contains("Run: githop edit office --owner <org>"))
        .stderr(contains(
            "Then run this command again with --owner <org> or --folder <path>.",
        ));

    // With an owner for the new one, the first (owner-less) profile still blocks.
    env.githop()
        .args([
            "add",
            "--id",
            "personal",
            "--name",
            "Me",
            "--email",
            PERSONAL_EMAIL,
            "--owner",
            "me-personal",
        ])
        .assert()
        .code(2)
        .stderr(contains("\"Office\" needs a GitHub owner or a folder"))
        .stderr(contains("Run: githop edit office --owner <org>"))
        .stderr(contains("Then run this command again."))
        .stderr(contains("personal: owners").not());
    let (_, v) = json(&env, &env.home(), &["list"]);
    assert_eq!(v["profiles"].as_array().unwrap().len(), 1);

    // Following the hint works.
    env.githop()
        .args(["edit", "office", "--owner", "office-org"])
        .assert()
        .code(0);
    env.githop()
        .args([
            "add",
            "--id",
            "personal",
            "--name",
            "Me",
            "--email",
            PERSONAL_EMAIL,
            "--owner",
            "me-personal",
        ])
        .assert()
        .code(0);

    // Taking the last owner away again is refused with the same hint.
    env.githop()
        .args(["edit", "office", "--no-owners"])
        .assert()
        .code(2)
        .stderr(contains("needs a GitHub owner or a folder"))
        .stderr(contains("Run: githop edit office --owner <org>"));
}

#[test]
fn add_derives_id_from_label() {
    let env = TestEnv::new();
    env.githop()
        .args([
            "add",
            "--label",
            "Side Work",
            "--name",
            "X",
            "--email",
            "x@example.com",
        ])
        .assert()
        .code(0);
    let (_, v) = json(&env, &env.home(), &["list"]);
    assert_eq!(v["profiles"][0]["id"], "side-work");
}

#[test]
fn edit_unknown_profile_is_usage_error() {
    let env = TestEnv::new();
    env.githop()
        .args(["edit", "ghost", "--name", "X"])
        .assert()
        .code(2)
        .stderr(contains("no profile called \"ghost\""));
}

#[test]
fn edit_without_flags_and_tty_is_usage_error() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    env.githop()
        .args(["edit", "office"])
        .assert()
        .code(2)
        .stderr(contains("Nothing to change"));
}

#[test]
fn edit_repin_updates_pinned_repos() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    let repo = env.init_repo("api");
    env.githop_in(&repo)
        .args(["pin", "office"])
        .assert()
        .code(0);
    env.githop()
        .args(["edit", "office", "--email", "new@company.com", "--repin"])
        .assert()
        .code(0)
        .stdout(contains("Re-pinned 1 repo to"));
    assert_eq!(
        env.get_local(&repo, "user.email").as_deref(),
        Some("new@company.com")
    );
}

#[test]
fn remove_default_needs_a_new_default() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    env.githop().args(["use", "office"]).assert().code(0);

    env.githop()
        .args(["remove", "office", "--yes"])
        .assert()
        .code(2)
        .stderr(contains("--new-default"));

    env.githop()
        .args(["remove", "office", "--yes", "--new-default", "personal"])
        .assert()
        .code(0)
        .stdout(contains("Default profile: Personal"));
    assert_eq!(
        env.get_global("user.email").as_deref(),
        Some(PERSONAL_EMAIL)
    );
    let (_, v) = json(&env, &env.home(), &["list"]);
    assert_eq!(v["default"], "personal");
}

#[test]
fn remove_without_tty_needs_yes() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    env.githop()
        .args(["remove", "office"])
        .assert()
        .code(2)
        .stderr(contains("--yes"));
}

// ---------------------------------------------------------------- use / wipe

#[test]
fn use_sets_global_identity_and_backs_up_first() {
    let env = TestEnv::new();
    env.set_global("user.name", "Original Name");
    env.set_global("user.email", "original@example.com");
    setup_two_profiles(&env);

    env.githop()
        .args(["use", "personal"])
        .assert()
        .code(0)
        .stdout(contains("Backup saved"))
        .stdout(contains(format!(
            "Default profile: Personal ({PERSONAL_EMAIL})"
        )))
        .stdout(contains("Pinned repos keep their own profile"));

    assert_eq!(
        env.get_global("user.email").as_deref(),
        Some(PERSONAL_EMAIL)
    );
    assert_eq!(env.get_global("user.name").as_deref(), Some("Me"));
    let ssh = env.get_global("core.sshCommand").unwrap();
    assert!(ssh.contains("githop_personal") && ssh.contains("IdentitiesOnly=yes"));

    let original = env.appdata().join("backups").join("gitconfig.original");
    assert!(original.is_file());
    assert!(std::fs::read_to_string(&original)
        .unwrap()
        .contains("original@example.com"));

    // Second switch: no "Backup saved" message, original untouched.
    let before = std::fs::read(&original).unwrap();
    env.githop()
        .args(["use", "office"])
        .assert()
        .code(0)
        .stdout(contains("Backup saved").not());
    assert_eq!(std::fs::read(&original).unwrap(), before);
}

#[test]
fn use_unknown_profile_is_usage_error() {
    let env = TestEnv::new();
    env.githop()
        .args(["use", "ghost"])
        .assert()
        .code(2)
        .stderr(contains("no profile called"));
    assert_eq!(env.get_global("user.email"), None);
}

#[test]
fn use_then_wipe_restores_original_global_identity_exactly() {
    let env = TestEnv::new();
    env.set_global("user.name", "Original Name");
    env.set_global("user.email", "original@example.com");
    env.set_global("alias.st", "status");
    setup_two_profiles(&env);
    env.githop().args(["use", "office"]).assert().code(0);
    env.githop().args(["use", "personal"]).assert().code(0);
    // An unrelated change the user makes later must survive wipe.
    env.set_global("alias.co", "checkout");

    env.githop()
        .args(["wipe", "--yes"])
        .assert()
        .code(0)
        .stdout(contains("GitHop wiped"));

    assert_eq!(
        env.get_global("user.name").as_deref(),
        Some("Original Name")
    );
    assert_eq!(
        env.get_global("user.email").as_deref(),
        Some("original@example.com")
    );
    assert_eq!(env.get_global("core.sshCommand"), None);
    assert_eq!(env.get_global("alias.st").as_deref(), Some("status"));
    assert_eq!(env.get_global("alias.co").as_deref(), Some("checkout"));
    assert!(!env.appdata().exists());
}

#[test]
fn wipe_restores_absence_of_identity() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    env.githop().args(["use", "office"]).assert().code(0);
    env.githop().args(["wipe", "--yes"]).assert().code(0);
    assert_eq!(env.get_global("user.name"), None);
    assert_eq!(env.get_global("user.email"), None);
    assert_eq!(env.get_global("core.sshCommand"), None);
}

#[test]
fn wipe_unpins_repos_and_keeps_keys_by_default() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    let key = env.fake_key("githop_office");
    let repo = env.init_repo("api");
    env.githop_in(&repo)
        .args(["pin", "office"])
        .assert()
        .code(0);

    env.githop().args(["wipe", "--yes"]).assert().code(0);
    assert_eq!(env.get_local(&repo, "githop.profile"), None);
    assert_eq!(env.get_local(&repo, "user.email"), None);
    assert!(key.is_file());
}

#[test]
fn wipe_delete_keys_only_removes_githop_keys() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    let githop_key = env.fake_key("githop_office");
    let own_key = env.fake_key("id_ed25519");
    env.githop()
        .args([
            "add",
            "--id",
            "own",
            "--name",
            "Me",
            "--email",
            "own@example.com",
            "--folder",
            "C:/no-repos-here/own",
            "--key",
        ])
        .arg(&own_key)
        .assert()
        .code(0);

    env.githop()
        .args(["wipe", "--yes", "--delete-keys"])
        .assert()
        .code(0);
    assert!(!githop_key.exists());
    assert!(!env.ssh_dir().join("githop_office.pub").exists());
    assert!(own_key.is_file());
    assert!(env.ssh_dir().join("id_ed25519.pub").is_file());
}

// ---------------------------------------------------------------- init

#[test]
fn init_creates_profile_from_global_identity_and_is_idempotent() {
    let env = TestEnv::new();
    env.set_global("user.name", "Harshal (Work)");
    env.set_global("user.email", "harshal@company.com");

    env.githop()
        .args(["init", "--id", "office", "--yes"])
        .assert()
        .code(0)
        .stdout(contains("Backup saved"))
        .stdout(contains("Profile created: Office (harshal@company.com)"));

    let profiles = env.appdata().join("profiles.toml");
    let original = env.appdata().join("backups").join("gitconfig.original");
    assert!(original.is_file());
    let (_, v) = json(&env, &env.home(), &["list"]);
    assert_eq!(v["default"], "office");
    assert_eq!(v["profiles"][0]["name"], "Harshal (Work)");

    let profiles_before = std::fs::read(&profiles).unwrap();
    let original_before = std::fs::read(&original).unwrap();
    env.githop()
        .args(["init", "--id", "other", "--yes"])
        .assert()
        .code(0)
        .stdout(contains("already set up"));
    assert_eq!(std::fs::read(&profiles).unwrap(), profiles_before);
    assert_eq!(std::fs::read(&original).unwrap(), original_before);
    // init never rewrites the global identity.
    assert_eq!(
        env.get_global("user.email").as_deref(),
        Some("harshal@company.com")
    );
}

#[test]
fn init_without_identity_or_tty_lists_missing_fields() {
    let env = TestEnv::new();
    env.githop()
        .arg("init")
        .assert()
        .code(2)
        .stderr(contains("--id"))
        .stderr(contains("--name"))
        .stderr(contains("--email"));
}

// ---------------------------------------------------------------- status / pin

#[test]
fn status_explains_owner_resolution() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    let repo = env.repo_with_origin("billing-api", "office-org");
    // Not pinned: the owner rule decides.
    let (code, v) = json(&env, &repo, &["status"]);
    assert_eq!(code, 0);
    assert_eq!(v["profile"]["id"], "office");
    assert_eq!(v["source"], "owner");
    assert_eq!(v["pinned"], Value::Null);

    env.githop_in(&repo)
        .arg("status")
        .assert()
        .code(0)
        .stdout(contains("Office — owner office-org on origin"))
        .stdout(contains("not pinned"));
}

#[test]
fn status_flags_wrong_identity() {
    let env = TestEnv::new();
    env.set_global("user.name", "Me");
    env.set_global("user.email", PERSONAL_EMAIL);
    setup_two_profiles(&env);
    let repo = env.repo_with_origin("billing-api", "office-org");
    env.githop_in(&repo)
        .arg("status")
        .assert()
        .code(0)
        .stdout(contains(
            "✗ This repo belongs to Office, but commits would use",
        ))
        .stdout(contains("githop pin office"));
}

#[test]
fn status_reports_local_hooks_path() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    let repo = env.repo_with_origin("web", "office-org");
    env.git(&["config", "--local", "core.hooksPath", ".husky"], &repo);
    let (_, v) = json(&env, &repo, &["status"]);
    let kinds: Vec<&str> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"local-hooks-path"), "{v}");
}

#[test]
fn status_outside_a_repo_is_usage_error() {
    let env = TestEnv::new();
    env.githop()
        .arg("status")
        .assert()
        .code(2)
        .stderr(contains("not a git repo"));
}

#[test]
fn pin_and_unpin_current_repo() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    let repo = env.init_repo("side");

    env.githop_in(&repo)
        .args(["pin", "personal"])
        .assert()
        .code(0)
        .stdout(contains("✓ Pinned to Personal"));
    assert_eq!(
        env.get_local(&repo, "githop.profile").as_deref(),
        Some("personal")
    );
    assert_eq!(
        env.get_local(&repo, "user.email").as_deref(),
        Some(PERSONAL_EMAIL)
    );
    let settings = std::fs::read_to_string(env.appdata().join("settings.toml")).unwrap();
    assert!(
        settings.contains("side"),
        "pinned_repos not recorded:\n{settings}"
    );

    let (_, v) = json(&env, &repo, &["status"]);
    assert_eq!(v["source"], "pin");

    env.githop_in(&repo)
        .arg("unpin")
        .assert()
        .code(0)
        .stdout(contains("Unpinned"));
    assert_eq!(env.get_local(&repo, "githop.profile"), None);
    assert_eq!(env.get_local(&repo, "user.email"), None);

    env.githop_in(&repo)
        .arg("unpin")
        .assert()
        .code(0)
        .stdout(contains("isn't pinned"));
}

#[test]
fn pin_unknown_profile_is_usage_error() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    let repo = env.init_repo("x");
    env.githop_in(&repo)
        .args(["pin", "ghost"])
        .assert()
        .code(2)
        .stderr(contains("Profiles: office"));
    assert_eq!(env.get_local(&repo, "githop.profile"), None);
}

// ---------------------------------------------------------------- scan

#[test]
fn scan_shows_and_apply_pins_owner_matched_repos() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    let office = env.repo_with_origin("billing-api", "office-org");
    let stranger = env.repo_with_origin("lib", "someone-else");
    std::fs::create_dir_all(env.repos().join("node_modules").join("dep")).unwrap();

    let (code, v) = json(&env, &env.home(), &["scan", &TestEnv::slash(&env.repos())]);
    assert_eq!(code, 0);
    let repos = v["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 2, "{v}");
    let status_of = |name: &str| {
        repos
            .iter()
            .find(|r| r["root"].as_str().unwrap().ends_with(name))
            .map(|r| r["status"].as_str().unwrap().to_string())
            .unwrap()
    };
    assert_eq!(status_of("billing-api"), "not-pinned");
    assert_eq!(status_of("lib"), "unresolved");

    let (code, v) = json(
        &env,
        &env.home(),
        &["scan", &TestEnv::slash(&env.repos()), "--apply"],
    );
    assert_eq!(code, 0);
    assert_eq!(v["applied"].as_array().unwrap().len(), 1);
    assert_eq!(
        env.get_local(&office, "githop.profile").as_deref(),
        Some("office")
    );
    assert_eq!(env.get_local(&stranger, "githop.profile"), None);

    env.githop()
        .arg("scan")
        .arg(env.repos())
        .assert()
        .code(0)
        .stdout(contains("billing-api"))
        .stdout(contains("pinned"));
}

#[test]
fn scan_apply_skips_default_only_repos_unless_asked() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    env.githop().args(["use", "personal"]).assert().code(0);
    let repo = env.init_repo("scratch");
    env.githop()
        .arg("scan")
        .arg(env.repos())
        .arg("--apply")
        .assert()
        .code(0)
        .stdout(contains("only matched the default profile"));
    assert_eq!(env.get_local(&repo, "githop.profile"), None);
    env.githop()
        .arg("scan")
        .arg(env.repos())
        .args(["--apply", "--include-default"])
        .assert()
        .code(0);
    assert_eq!(
        env.get_local(&repo, "githop.profile").as_deref(),
        Some("personal")
    );
}

// ---------------------------------------------------------------- doctor

#[test]
fn doctor_detects_planted_github_identity_file() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    env.write_ssh_config(
        "# work key\nHost github.com\n    HostName github.com\n    IdentityFile ~/.ssh/id_work\n",
    );
    let (_, v) = json(&env, &env.home(), &["doctor"]);
    let hit = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "ssh-config" && c["status"] == "warning")
        .cloned();
    let hit = hit.unwrap_or_else(|| panic!("no ssh-config warning: {v}"));
    assert!(hit["detail"].as_str().unwrap().contains("line 4"));
    assert!(hit["fix"]
        .as_str()
        .unwrap()
        .contains("different Host alias"));

    env.githop()
        .arg("doctor")
        .assert()
        .stdout(contains("Host github.com"))
        .stdout(contains("Fix: Scope it under a different Host alias"));
}

#[test]
fn doctor_warns_about_env_overrides() {
    let env = TestEnv::new();
    let out = env
        .githop()
        .args(["--json", "doctor"])
        .env("GIT_SSH_COMMAND", "ssh -i C:/x")
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == "env" && c["detail"].as_str().unwrap().contains("GIT_SSH_COMMAND")));
}

#[test]
fn doctor_json_summary_and_exit_code() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    let (code, v) = json(&env, &env.home(), &["doctor"]);
    for key in ["ok", "warning", "problem"] {
        assert!(v["summary"][key].is_u64(), "summary.{key} missing: {v}");
    }
    let problems = v["summary"]["problem"].as_u64().unwrap();
    assert_eq!(code, if problems > 0 { 1 } else { 0 });
    assert!(v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == "git-version" && c["status"] == "ok"));
}

// ---------------------------------------------------------------- keys

#[test]
fn keygen_creates_key_and_refuses_to_overwrite() {
    let env = TestEnv::new();
    env.add_profile("personal", "Me", PERSONAL_EMAIL, &[]);
    env.githop()
        .args(["keygen", "personal", "--no-passphrase"])
        .assert()
        .code(0)
        .stdout(contains("Key created"))
        .stdout(contains("ssh-ed25519"))
        .stdout(contains("no passphrase"));
    let key = env.ssh_dir().join("githop_personal");
    assert!(key.is_file());
    assert!(env.ssh_dir().join("githop_personal.pub").is_file());

    env.githop()
        .args(["keygen", "personal", "--no-passphrase"])
        .assert()
        .code(2)
        .stderr(contains("won't overwrite"));
}

#[test]
fn test_without_key_points_to_keygen() {
    let env = TestEnv::new();
    env.add_profile("office", "Me", OFFICE_EMAIL, &[]);
    env.githop()
        .args(["test", "office"])
        .assert()
        .code(2)
        .stderr(contains("githop keygen office"));
}

// ---------------------------------------------------------------- snapshots (Design §9.3)

#[test]
fn snapshot_use_output() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    let (_, out, _) = env.run(&env.home(), &["use", "personal"]);
    insta::assert_snapshot!(out, @r"
    ✓ Backup saved. You can restore your original settings any time.
    ✓ Default profile: Personal (12345+me@users.noreply.github.com)
    Pinned repos keep their own profile.
    ");
}

#[test]
fn snapshot_pin_output() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    let repo = env.init_repo("api");
    let (_, out, _) = env.run(&repo, &["pin", "office"]);
    insta::assert_snapshot!(out, @r"
    ✓ Pinned to Office
    Commits in this repo now use me@company.com.
    ");
}

#[test]
fn snapshot_list_output() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    env.githop().args(["use", "office"]).assert().code(0);
    let (_, out, _) = env.run(&env.home(), &["list"]);
    for line in out.lines() {
        assert!(line.chars().count() <= 80, "line over 80 columns: {line}");
    }
    insta::assert_snapshot!("list_two_profiles", out);
}

#[test]
fn snapshot_unknown_profile_error() {
    let env = TestEnv::new();
    setup_two_profiles(&env);
    let (code, _, err) = env.run(&env.home(), &["use", "ghost"]);
    assert_eq!(code, 2);
    insta::assert_snapshot!(err, @r#"
    ✗ There's no profile called "ghost".
    Profiles: office, personal. Run: githop list
    "#);
}
