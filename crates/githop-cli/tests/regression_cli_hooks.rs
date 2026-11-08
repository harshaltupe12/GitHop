//! Regression 2026-11-01 (CLI and hooks, D1–D23) and the security regression
//! of the same day: each defect's repro, run end to end with the *built*
//! `githop` installed as the hooks of an isolated global git config and real
//! `git` + Git for Windows' `sh`.
//!
//! Isolation (CLAUDE.md rule 3): temp HOME/USERPROFILE, GIT_CONFIG_GLOBAL,
//! GIT_CONFIG_NOSYSTEM=1, GITHOP_HOME, GITHOP_NO_REGISTRY=1 (see `support`).
//! Remotes are local bare repos reached through `url.<path>.insteadOf`.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use support::TestEnv;

const OFFICE: &str = "jane@office.example";
const PERSONAL: &str = "12345+jane@users.noreply.github.com";
const PASSWORD: &str = "correct horse battery staple";

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

/// Personal is the global identity and default profile; Office owns
/// `office-org`. Hooks installed by `githop init`.
fn setup_with(before_init: impl FnOnce(&TestEnv)) -> TestEnv {
    let env = TestEnv::new();
    env.set_global("user.name", "Jane");
    env.set_global("user.email", PERSONAL);
    before_init(&env);
    let (code, out, err) = env.run(
        &env.home(),
        &["init", "--id", "personal", "--label", "Personal", "--yes"],
    );
    assert_eq!(code, 0, "init failed:\n{out}\n{err}");
    env.add_profile("office", "Jane Office", OFFICE, &["office-org"]);
    env
}

fn setup() -> TestEnv {
    setup_with(|_| {})
}

fn stage(env: &TestEnv, repo: &Path, file: &str) {
    std::fs::write(repo.join(file), format!("{file}\n")).unwrap();
    env.git(&["add", file], repo);
}

fn commit(env: &TestEnv, repo: &Path, file: &str) -> Output {
    stage(env, repo, file);
    git_env(
        env,
        &["commit", "-q", "-m", &format!("add {file}")],
        repo,
        &[],
    )
}

fn settings(env: &TestEnv) -> String {
    std::fs::read_to_string(env.appdata().join("settings.toml")).unwrap_or_default()
}

/// Pinned Office repo with a bare remote and one pushed Office commit.
fn office_repo_with_remote(env: &TestEnv, name: &str) -> (PathBuf, PathBuf) {
    env.redirect_github_to_local();
    let remote = env.bare_remote(&format!("office-org/{name}"));
    let repo = env.repo_with_origin(name, "office-org");
    env.githop_in(&repo)
        .args(["pin", "office"])
        .assert()
        .code(0);
    env.commit_as(&repo, "a.txt", OFFICE);
    let out = git_env(env, &["push", "-q", "-u", "origin", "main"], &repo, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    (repo, remote)
}

// ------------------------------------------------------------------- D1

/// D1: an SSH host alias for github.com works without a prior scan/doctor:
/// `init`/`add` fill the cache, and hooks re-read ~/.ssh/config when it
/// changed (no `ssh -G` in hooks).
#[test]
fn d1_ssh_alias_works_without_scan_and_when_added_later() {
    let env = setup_with(|env| {
        env.write_ssh_config("Host gh-office\n  HostName github.com\n  User git\n");
    });
    assert!(
        settings(&env).contains("\"gh-office\""),
        "{}",
        settings(&env)
    );

    let repo = env.init_repo("a");
    env.git(
        &["remote", "add", "origin", "gh-office:office-org/a.git"],
        &repo,
    );
    let (_, out, _) = env.run(&repo, &["status"]);
    assert!(out.contains("Office — owner office-org"), "{out}");
    let out = commit(&env, &repo, "x.txt");
    assert!(
        !out.status.success(),
        "Personal commit to an office repo is blocked"
    );
    assert!(
        stderr(&out).contains("This repo belongs to Office."),
        "{}",
        stderr(&out)
    );

    // An alias added later: the hook sees it (the file changed since).
    env.write_ssh_config(
        "Host gh-office\n  HostName github.com\n\nHost gh-work\n  HostName github.com\n",
    );
    let repo = env.init_repo("b");
    env.git(
        &["remote", "add", "origin", "gh-work:office-org/b.git"],
        &repo,
    );
    let out = commit(&env, &repo, "y.txt");
    assert!(!out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("This repo belongs to Office."),
        "{}",
        stderr(&out)
    );
    assert!(
        settings(&env).contains("\"gh-work\""),
        "cache refreshed by the hook"
    );
}

// ------------------------------------------------------------------- D2

/// D2: a commit matched only by the default profile is blocked but never
/// pinned, and the user's own local user.email stays.
#[test]
fn d2_default_only_commit_is_blocked_without_pinning() {
    let env = setup();
    let repo = env.init_repo("gl");
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "https://gitlab.com/acme-corp/svc.git",
        ],
        &repo,
    );
    env.git(&["config", "user.email", OFFICE], &repo);
    let out = commit(&env, &repo, "x.txt");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(
        err.contains("No owner or folder rule matches this repo"),
        "{err}"
    );
    assert!(
        err.contains("githop pin <profile> (personal, office)"),
        "{err}"
    );
    assert!(!err.contains("pinned to"), "{err}");
    assert_eq!(env.get_local(&repo, "user.email").as_deref(), Some(OFFICE));
    assert_eq!(env.get_local(&repo, "githop.profile"), None);
    // Owner match still pins (Plan §1).
    let repo = env.repo_with_origin("billing", "office-org");
    let out = commit(&env, &repo, "x.txt");
    assert!(stderr(&out).contains("Identity fixed for this repo: pinned to Office."));
}

// ------------------------------------------------------------------- D3

/// D3: undo is refused once the fixed commits are pushed.
#[test]
fn d3_fix_undo_refused_after_the_fix_was_pushed() {
    let env = setup();
    let (repo, _remote) = office_repo_with_remote(&env, "undo");
    env.commit_as(&repo, "b.txt", PERSONAL);
    env.commit_as(&repo, "c.txt", PERSONAL);
    let (code, out, err) = env.run(&repo, &["fix", "--yes"]);
    assert_eq!(code, 0, "{out}\n{err}");
    let out = git_env(&env, &["push", "-q", "origin", "main"], &repo, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let head = env.git(&["rev-parse", "HEAD"], &repo);
    let (code, out, err) = env.run(&repo, &["fix", "--undo", "--yes"]);
    assert_ne!(code, 0, "{out}\n{err}");
    assert!(
        format!("{out}{err}").contains("already pushed, so undo would split your branch"),
        "{out}\n{err}"
    );
    assert_eq!(
        env.git(&["rev-parse", "HEAD"], &repo),
        head,
        "nothing changed"
    );
}

/// Security #6: a hand-made backup/result pair is refused.
#[test]
fn sec6_fix_undo_refuses_a_hand_made_backup_pair() {
    let env = setup();
    let (repo, _remote) = office_repo_with_remote(&env, "forged");
    let old = env.git(&["rev-parse", "HEAD"], &repo);
    env.commit_as(&repo, "b.txt", OFFICE);
    env.commit_as(&repo, "c.txt", OFFICE);
    let head = env.git(&["rev-parse", "HEAD"], &repo);
    let backup = "refs/githop/backup/main/20261031T101500Z";
    env.git(&["update-ref", backup, &old], &repo);
    env.git(
        &[
            "update-ref",
            "refs/githop/fixed/main/20261031T101500Z",
            &head,
        ],
        &repo,
    );
    let (code, out, err) = env.run(&repo, &["fix", &format!("--undo={backup}"), "--yes"]);
    assert_ne!(code, 0, "{out}\n{err}");
    assert!(format!("{out}{err}").contains("doesn't match the fix GitHop recorded"));
    assert_eq!(env.git(&["rev-parse", "HEAD"], &repo), head);
}

// ------------------------------------------------------------------- D4

/// D4: a push with GITHOP_DISABLE=1 is still recorded, so Fix never rewrites
/// what it published (rule 7).
#[test]
fn d4_disabled_push_is_recorded_and_fix_leaves_it_alone() {
    let env = setup();
    let (repo, remote) = office_repo_with_remote(&env, "dis");
    env.git(&["checkout", "-q", "-b", "feature"], &repo);
    let bad = env.commit_as(&repo, "b.txt", PERSONAL);
    // Push to a URL: no remote-tracking ref is created.
    let url = TestEnv::slash(&remote);
    let out = git_env(
        &env,
        &["push", "-q", &url, "feature"],
        &repo,
        &[("GITHOP_DISABLE", "1")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).is_empty(),
        "disabled push is silent: {}",
        stderr(&out)
    );
    let pushed = std::fs::read_to_string(repo.join(".git").join("githop").join("pushed")).unwrap();
    assert!(pushed.contains(&bad), "{pushed}");
    let (_, out, err) = env.run(&repo, &["fix", "--yes"]);
    assert!(
        out.contains("already pushed"),
        "listed as pushed:\n{out}\n{err}"
    );
    assert_eq!(
        env.git(&["rev-parse", "HEAD"], &repo),
        bad,
        "published commit kept"
    );
}

// ------------------------------------------------------------------- D5

/// D5: `hooks install` works while profiles.toml is broken, and an enforce
/// mode changed by hand applies at once (no stale STRICT in the hooks).
#[test]
fn d5_enforce_by_hand_and_install_with_corrupt_profiles() {
    let env = setup();
    let repo = env.init_repo("side");
    let set_mode = |mode: &str| {
        let s = settings(&env);
        let s: String = s
            .lines()
            .map(|l| {
                if l.starts_with("enforce") {
                    format!("enforce = \"{mode}\"\n")
                } else {
                    format!("{l}\n")
                }
            })
            .collect();
        std::fs::write(env.appdata().join("settings.toml"), s).unwrap();
    };
    set_mode("block-strict");
    std::fs::write(env.appdata().join("profiles.toml"), "this is [[[ not toml").unwrap();
    let out = commit(&env, &repo, "a.txt");
    assert!(!out.status.success(), "block-strict blocks a GitHop error");
    set_mode("block");
    let out = commit(&env, &repo, "b.txt");
    assert!(out.status.success(), "block fails open: {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("couldn't check this commit (it went through)"),
        "{err}"
    );
    assert!(
        !err.contains("blocked"),
        "no contradicting second line: {err}"
    );
    // One sentence, forward slashes (D16).
    assert!(!err.contains("githop\\profiles.toml"), "{err}");

    let (code, out, err) = env.run(&env.home(), &["hooks", "install"]);
    assert_eq!(code, 0, "install needs only settings:\n{out}\n{err}");
}

// ------------------------------------------------------------------- D6

/// D6: the husky pre-push snippet hands the ref lines on to the rest of the
/// repo's hook.
#[test]
fn d6_push_snippet_keeps_stdin_for_the_rest_of_the_hook() {
    let env = setup();
    let (repo, _remote) = office_repo_with_remote(&env, "husky");
    let (code, snippet, _) = env.run(&repo, &["hooks", "snippet", "push"]);
    assert_eq!(code, 0);
    let husky = repo.join(".husky");
    std::fs::create_dir_all(&husky).unwrap();
    std::fs::write(
        husky.join("pre-push"),
        format!("#!/bin/sh\n{}echo HUSKY-PUSH >&2\ncat >&2\n", snippet),
    )
    .unwrap();
    env.git(&["config", "core.hooksPath", ".husky"], &repo);
    env.commit_as(&repo, "b.txt", OFFICE);
    let out = git_env(&env, &["push", "-q", "origin", "main"], &repo, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let err = stderr(&out);
    let after = err.split("HUSKY-PUSH").nth(1).unwrap_or_default();
    assert!(
        after.contains("refs/heads/main"),
        "rest of hook got the refs:\n{err}"
    );
}

// ------------------------------------------------------------------- D7

/// D7: a pin that disagrees with the owner rule shows in status and doctor.
#[test]
fn d7_pin_owner_disagreement_is_reported() {
    let env = setup();
    let repo = env.repo_with_origin("svc", "office-org");
    env.githop_in(&repo)
        .args(["pin", "personal"])
        .assert()
        .code(0);
    let (_, out, _) = env.run(&repo, &["status"]);
    assert!(
        out.contains("Pinned to Personal, but owner office-org belongs to Office."),
        "{out}"
    );
    assert!(out.contains("Run: githop pin office"), "{out}");
    let (_, out, _) = env.run(&repo, &["--json", "doctor"]);
    assert!(out.contains("\"pin-owner\""), "{out}");
}

// ------------------------------------------------------------------- D8

/// D8: a global author.email that overrides the pin is named, and `pin`
/// doesn't claim the wrong email.
#[test]
fn d8_global_author_email_overriding_the_pin_is_named() {
    let env = setup();
    let repo = env.repo_with_origin("billing", "office-org");
    env.githop_in(&repo)
        .args(["pin", "office"])
        .assert()
        .code(0);
    env.set_global("author.email", PERSONAL);
    let out = commit(&env, &repo, "a.txt");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("author.email is set in"), "{err}");
    assert!(
        err.contains("git config --global --unset author.email"),
        "{err}"
    );
    assert!(
        !err.contains("Run: githop pin office"),
        "no no-op pin advice: {err}"
    );
    let (_, out, _) = env.run(&repo, &["pin", "office"]);
    assert!(out.contains("still won't use"), "{out}");
    assert!(!out.contains("now use"), "{out}");
}

// ------------------------------------------------------------------- D10

/// D10: a submodule of an office repo is pinned when git checks it out.
#[test]
fn d10_submodule_clone_is_pinned() {
    let env = setup();
    env.redirect_github_to_local();
    env.bare_remote("office-org/svc");
    let seed = env.init_repo("seed");
    env.commit_as(&seed, "readme.txt", OFFICE);
    env.git(
        &[
            "push",
            "-q",
            "--no-verify",
            &TestEnv::slash(&env.remotes().join("office-org/svc.git")),
            "main",
        ],
        &seed,
    );
    let sup = env.repo_with_origin("super", "office-org");
    let out = git_env(
        &env,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            "https://github.com/office-org/svc.git",
            "libs/svc",
        ],
        &sup,
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        env.get_local(&sup.join("libs").join("svc"), "githop.profile")
            .as_deref(),
        Some("office"),
        "{}",
        stderr(&out)
    );
}

// ------------------------------------------------------------------- D12 / D14

/// D12: `doctor -q` prints no Passing/Notes headers and keeps each Fix line.
#[test]
fn d12_doctor_quiet_shows_only_warnings_and_problems_with_fixes() {
    let env = setup();
    let (_, out, _) = env.run(&env.home(), &["doctor", "-q"]);
    assert!(!out.contains("Passing") && !out.contains("Notes"), "{out}");
    if out.contains("Warnings") {
        assert!(out.contains("Fix:"), "{out}");
    }
    assert!(!out.contains("(s)"), "D14 plurals: {out}");
}

// ------------------------------------------------------------------- D17

/// D17: init refuses bad input before backing anything up.
#[test]
fn d17_init_validates_before_writing() {
    let env = TestEnv::new();
    let (code, out, err) = env.run(&env.home(), &["init"]);
    assert_eq!(code, 2, "{out}\n{err}");
    assert!(!out.contains("Backup saved"), "{out}");
    assert!(
        !env.appdata().join("backups").exists(),
        "no backup for refused input"
    );
    let (code, out, err) = env.run(
        &env.home(),
        &[
            "init",
            "--id",
            "BAD ID",
            "--name",
            "x",
            "--email",
            "x@y.example",
            "--yes",
        ],
    );
    assert_ne!(code, 0, "{out}\n{err}");
    assert!(!env.appdata().join("backups").exists());
}

// ------------------------------------------------------------------- D19 / D23

fn export_file(env: &TestEnv, file: &Path) {
    let out = env
        .githop()
        .env("GITHOP_EXPORT_PASSWORD", PASSWORD)
        .args(["export", &TestEnv::slash(file), "--include-keys"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
}

fn import_file(env: &TestEnv, file: &Path, on_conflict: &str) -> (i32, String) {
    let out = env
        .githop()
        .env("GITHOP_EXPORT_PASSWORD", PASSWORD)
        .args([
            "import",
            &TestEnv::slash(file),
            "--on-conflict",
            on_conflict,
            "--yes",
        ])
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        format!("{}{}", stdout(&out), stderr(&out)),
    )
}

/// D19: `import --on-conflict replace` warns about pinned repos (and the
/// global identity) still using the old details. D23: a renamed profile
/// with the same key says it reuses it.
#[test]
fn d19_d23_import_replace_warns_and_rename_explains_shared_key() {
    let a = setup();
    std::fs::create_dir_all(a.ssh_dir()).unwrap();
    for id in ["office", "personal"] {
        std::fs::write(
            a.ssh_dir().join(format!("githop_{id}")),
            format!(
                "-----BEGIN OPENSSH PRIVATE KEY-----\n{id}\n-----END OPENSSH PRIVATE KEY-----\n"
            ),
        )
        .unwrap();
        std::fs::write(
            a.ssh_dir().join(format!("githop_{id}.pub")),
            "ssh-ed25519 AAAA x\n",
        )
        .unwrap();
    }
    let file = a.root().join("a.githop");
    export_file(&a, &file);
    // Same PC: office changes its email, a repo is pinned to it.
    let repo = a.repo_with_origin("billing", "office-org");
    a.githop_in(&repo).args(["pin", "office"]).assert().code(0);
    let (code, out, err) = a.run(
        &a.home(),
        &["edit", "office", "--email", "changed@office.example"],
    );
    assert_eq!(code, 0, "{out}{err}");
    let (code, out) = import_file(&a, &file, "replace");
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("1 repo pinned to Office still uses the old details."),
        "{out}"
    );
    assert!(out.contains("githop edit office --repin"), "{out}");

    // Rename needs other emails here; the key files stay the same.
    for (id, email) in [
        ("office", "o2@office.example"),
        ("personal", "p2@x.example"),
    ] {
        let (code, out, err) = a.run(&a.home(), &["edit", id, "--email", email]);
        assert_eq!(code, 0, "{out}{err}");
    }
    let (code, out) = import_file(&a, &file, "rename");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("(same key)"), "{out}");
}

// ------------------------------------------------------------------- D20

/// D20: wipe removes refs/githop/* and .git/githop/pushed from the repos it
/// pinned, and says so.
#[test]
fn d20_wipe_removes_githop_data_from_pinned_repos() {
    let env = setup();
    let (repo, _remote) = office_repo_with_remote(&env, "w");
    env.commit_as(&repo, "b.txt", PERSONAL);
    let (code, out, err) = env.run(&repo, &["fix", "--yes"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(repo.join(".git").join("githop").join("pushed").is_file());
    let (code, out, err) = env.run(&env.home(), &["wipe", "--yes"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Fix backups (refs/githop/…)"), "{out}");
    assert_eq!(env.git(&["for-each-ref", "refs/githop/"], &repo), "");
    assert!(!repo.join(".git").join("githop").exists());
}

// ------------------------------------------------------------------- D21

/// D21: status doesn't push a default-only pin.
#[test]
fn d21_status_does_not_push_default_only_pins() {
    let env = setup();
    let repo = env.init_repo("plain");
    let (_, out, _) = env.run(&repo, &["status"]);
    assert!(out.contains("Only the default profile matches."), "{out}");
    assert!(!out.contains("Pin it now"), "{out}");
}

// ------------------------------------------------------------------- security

/// Security #1: a replace ref can't hide a wrong-identity commit from the
/// push check.
#[test]
fn sec1_replace_refs_do_not_hide_a_bad_commit() {
    let env = setup();
    let (repo, _remote) = office_repo_with_remote(&env, "rep");
    env.commit_as(&repo, "b.txt", PERSONAL);
    let fake = env.git(
        &[
            "-c",
            &format!("user.email={OFFICE}"),
            "commit-tree",
            "HEAD^{tree}",
            "-p",
            "HEAD~1",
            "-m",
            "x",
        ],
        &repo,
    );
    env.git(&["replace", "HEAD", &fake], &repo);
    let out = git_env(&env, &["push", "-q", "origin", "HEAD:main"], &repo, &[]);
    assert!(!out.status.success(), "push must be blocked");
    assert!(stderr(&out).contains("push blocked"), "{}", stderr(&out));
}

/// Security #2: escape sequences in a commit subject never reach the terminal.
#[test]
fn sec2_commit_subject_escapes_are_neutralised() {
    let env = setup();
    let (repo, _remote) = office_repo_with_remote(&env, "esc");
    std::fs::write(repo.join("e.txt"), "e\n").unwrap();
    env.git(&["add", "e.txt"], &repo);
    env.git(
        &[
            "-c",
            &format!("user.email={PERSONAL}"),
            "commit",
            "-q",
            "--no-verify",
            "-m",
            "fix typo\x1b]0;OWNED\x07\x1b[2J\x1b[31mFAKE: push allowed\x1b[0m",
        ],
        &repo,
    );
    let out = git_env(&env, &["push", "-q", "origin", "main"], &repo, &[]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("push blocked"), "{err}");
    assert!(!err.contains('\x1b') && !err.contains('\x07'), "{err:?}");
    let (_, out, _) = env.run(&repo, &["fix", "--dry-run"]);
    assert!(!out.contains('\x1b') && !out.contains('\x07'), "{out:?}");
}

/// Security #4: control characters in a label are refused, and `pin`
/// refuses a hand-edited profile that doesn't validate.
#[test]
fn sec4_profile_validation_and_pin_refuses_invalid_profiles() {
    let env = setup();
    let key = TestEnv::slash(&env.ssh_dir().join("githop_e"));
    let (code, _, err) = env.run(
        &env.home(),
        &[
            "add",
            "--id",
            "e",
            "--label",
            "Lab\x1b[31mRED",
            "--name",
            "E",
            "--email",
            "e@x.example",
            "--key",
            &key,
        ],
    );
    assert_ne!(code, 0, "{err}");
    let path = env.appdata().join("profiles.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let text = text.replace(OFFICE, "bad@office.example\\n[core]");
    std::fs::write(&path, text).unwrap();
    let repo = env.repo_with_origin("billing", "office-org");
    let (code, out, err) = env.run(&repo, &["pin", "office"]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("isn't valid"), "{err}");
    assert_eq!(env.get_local(&repo, "githop.profile"), None);
}

/// Security #5: doctor flags files in the hooks folder GitHop didn't write.
#[test]
fn sec5_doctor_flags_planted_hook_files() {
    let env = setup();
    std::fs::write(
        env.appdata().join("hooks").join("reference-transaction"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    let (_, out, _) = env.run(&env.home(), &["--json", "doctor"]);
    assert!(out.contains("\"hooks-unexpected\""), "{out}");
    assert!(out.contains("reference-transaction"), "{out}");
}

/// Security 9.8: all colours taken → a clear message.
#[test]
fn sec9_no_free_colour_message() {
    let env = setup();
    for i in 0..6 {
        env.add_profile(&format!("p{i}"), "P", &format!("p{i}@x.example"), &[]);
    }
    let key = TestEnv::slash(&env.ssh_dir().join("githop_p9"));
    let (code, _, err) = env.run(
        &env.home(),
        &[
            "add",
            "--id",
            "p9",
            "--name",
            "P",
            "--email",
            "p9@x.example",
            "--key",
            &key,
        ],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("All 8 profile colours are taken"), "{err}");
}

/// Plan §11: the commit fast path still spawns exactly one git after the
/// alias and strict changes (GITHOP_DEBUG_TIMING prints one line per spawn).
#[test]
fn perf_commit_fast_path_spawns_one_git() {
    let env = setup();
    let repo = env.repo_with_origin("billing", "office-org");
    env.githop_in(&repo)
        .args(["pin", "office"])
        .assert()
        .code(0);
    let out = env
        .githop_in(&repo)
        .env("GITHOP_DEBUG_TIMING", "1")
        .args(["check", "--stage", "commit"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let spawns = stderr(&out).lines().filter(|l| l.contains("git ")).count();
    assert_eq!(spawns, 1, "{}", stderr(&out));
}

/// Security #7: a graft that hides a wrong-identity parent doesn't make the
/// push check pass (checks read real history).
#[test]
fn sec7_grafts_do_not_hide_a_bad_commit() {
    let env = setup();
    let (repo, _remote) = office_repo_with_remote(&env, "graft");
    let base = env.git(&["rev-parse", "HEAD"], &repo);
    env.commit_as(&repo, "b.txt", PERSONAL);
    let good = env.commit_as(&repo, "c.txt", OFFICE);
    let info = repo.join(".git").join("info");
    std::fs::create_dir_all(&info).unwrap();
    std::fs::write(info.join("grafts"), format!("{good} {base}\n")).unwrap();
    let out = git_env(&env, &["push", "-q", "origin", "main"], &repo, &[]);
    assert!(!out.status.success(), "push must be blocked");
    assert!(stderr(&out).contains("push blocked"), "{}", stderr(&out));
}
