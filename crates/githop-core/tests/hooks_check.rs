//! Phase 3 integration tests: hook scripts under Git for Windows `sh`, and the
//! check logic against real git.
//!
//! Part 1 (always runs): the rendered hook scripts are installed as the
//! isolated global `core.hooksPath` and git runs them for real. `githop` is a
//! stub sh script whose exit code comes from `STUB_RC`, so these tests only
//! need `hooks::render_all` — not the Phase 1 code.
//!
//! Part 2 (`#[ignore = "needs Phase 1 merged"]`): install/uninstall/verify and
//! the clone/commit/push checks end to end via `check::run_check`.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use githop_core::check::{self, CheckContext, Outcome, Stage};
use githop_core::hooks;
use support::TestEnv;

// ---------------------------------------------------------------------------
// Part 1 — hook scripts, stub githop

const STUB: &str = r#"#!/bin/sh
echo "githop $*" >> "$STUB_LOG"
if [ "$3" = "push" ]; then
  while IFS= read -r line; do echo "githop-stdin $line" >> "$STUB_LOG"; done
fi
exit ${STUB_RC:-0}
"#;

struct HookEnv {
    env: TestEnv,
    hooks_dir: PathBuf,
    log: PathBuf,
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// A hook that logs `<tag> <args>` (and its stdin lines) and exits `$<rc_var>` (default 0).
fn marker_hook(tag: &str, rc_var: &str) -> String {
    format!(
        "#!/bin/sh\necho \"{tag} $*\" >> \"$STUB_LOG\"\n\
         if [ -n \"$MARKER_READS_STDIN\" ]; then while IFS= read -r line; do echo \"{tag}-stdin $line\" >> \"$STUB_LOG\"; done; fi\n\
         exit ${{{rc_var}:-0}}\n"
    )
}

impl HookEnv {
    /// Install rendered hooks with `githop` = stub (or `exe_override`) and `previous`.
    fn new(previous: Option<&Path>, exe_override: Option<&Path>) -> Self {
        let env = TestEnv::new();
        env.set_global("user.name", "Test Person");
        env.set_global("user.email", "test@example.invalid");
        let bin = env.root().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let stub = bin.join("githop");
        std::fs::write(&stub, STUB).unwrap();
        let exe = exe_override.map(Path::to_path_buf).unwrap_or(stub);
        let hooks_dir = env.root().join("githop-hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let prev = previous.map(slash).unwrap_or_default();
        for (name, body) in hooks::render_all(&slash(&exe), &prev).unwrap() {
            assert!(!body.contains('\r'));
            std::fs::write(hooks_dir.join(name), body).unwrap();
        }
        env.set_global("core.hooksPath", &slash(&hooks_dir));
        let log = env.root().join("calls.log");
        Self {
            env,
            hooks_dir,
            log,
        }
    }

    fn cmd(&self, args: &[&str], cwd: &Path, vars: &[(&str, &str)]) -> Command {
        let mut c = Command::new("git");
        c.args(args).current_dir(cwd);
        self.env.apply(&mut c);
        c.env("STUB_LOG", slash(&self.log));
        for (k, v) in vars {
            c.env(k, v);
        }
        c
    }

    fn git(&self, args: &[&str], cwd: &Path, vars: &[(&str, &str)]) -> Output {
        self.cmd(args, cwd, vars).output().unwrap()
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(&self.log);
    }

    fn commit(&self, repo: &Path, file: &str, vars: &[(&str, &str)]) -> Output {
        std::fs::write(repo.join(file), "x\n").unwrap();
        let add = self.git(&["add", file], repo, &[]);
        assert!(add.status.success(), "{}", stderr(&add));
        self.git(&["commit", "-q", "-m", &format!("add {file}")], repo, vars)
    }
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn write_repo_hook(repo: &Path, name: &str, body: &str) {
    let dir = repo.join(".git").join("hooks");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), body).unwrap();
}

#[test]
fn commit_pass_runs_githop_then_repo_hook() {
    let h = HookEnv::new(None, None);
    let repo = h.env.init_repo("r");
    write_repo_hook(
        &repo,
        "pre-commit",
        &marker_hook("repo-pre-commit", "REPO_RC"),
    );
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = h.calls();
    let g = calls
        .find("githop check --stage commit --")
        .expect("githop called");
    let r = calls.find("repo-pre-commit").expect("repo hook chained");
    assert!(g < r, "githop runs before the repo hook:\n{calls}");
}

#[test]
fn commit_blocked_when_githop_exits_1() {
    let h = HookEnv::new(None, None);
    let repo = h.env.init_repo("r");
    write_repo_hook(
        &repo,
        "pre-commit",
        &marker_hook("repo-pre-commit", "REPO_RC"),
    );
    let out = h.commit(&repo, "a.txt", &[("STUB_RC", "1")]);
    assert!(!out.status.success());
    assert!(
        !h.calls().contains("repo-pre-commit"),
        "chain stops after a block"
    );
    assert!(
        h.env
            .git_output(&["rev-parse", "HEAD"], &repo)
            .status
            .code()
            != Some(0)
    );
}

#[test]
fn internal_error_exit_3_fails_open_quietly() {
    let h = HookEnv::new(None, None);
    let repo = h.env.init_repo("r");
    write_repo_hook(
        &repo,
        "pre-commit",
        &marker_hook("repo-pre-commit", "REPO_RC"),
    );
    let out = h.commit(&repo, "a.txt", &[("STUB_RC", "3")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        !stderr(&out).contains("couldn't check"),
        "githop already explained exit 3"
    );
    assert!(h.calls().contains("repo-pre-commit"));
}

#[test]
fn crash_exit_code_fails_open_with_warning() {
    let h = HookEnv::new(None, None);
    let repo = h.env.init_repo("r");
    let out = h.commit(&repo, "a.txt", &[("STUB_RC", "101")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out)
        .contains("! GitHop couldn't check this commit (it went through). Run: githop doctor"));
}

#[test]
fn missing_githop_fails_open_with_warning() {
    let env_root_missing = PathBuf::from("C:/definitely/not/here/githop.exe");
    let h = HookEnv::new(None, Some(&env_root_missing));
    let repo = h.env.init_repo("r");
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("GitHop is not installed correctly (it went through)"));
}

#[test]
fn repo_hook_failure_still_blocks() {
    let h = HookEnv::new(None, None);
    let repo = h.env.init_repo("r");
    write_repo_hook(
        &repo,
        "pre-commit",
        &marker_hook("repo-pre-commit", "REPO_RC"),
    );
    let out = h.commit(&repo, "a.txt", &[("REPO_RC", "1")]);
    assert!(
        !out.status.success(),
        "lint-style repo hook failure must still block"
    );
}

#[test]
fn passthrough_stub_chains_commit_msg_and_post_commit() {
    let h = HookEnv::new(None, None);
    let repo = h.env.init_repo("r");
    write_repo_hook(
        &repo,
        "commit-msg",
        &marker_hook("repo-commit-msg", "MSG_RC"),
    );
    write_repo_hook(
        &repo,
        "post-commit",
        &marker_hook("repo-post-commit", "NOPE"),
    );
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(h.calls().contains("repo-commit-msg .git/COMMIT_EDITMSG"));
    assert!(h.calls().contains("repo-post-commit"));
    let out = h.commit(&repo, "b.txt", &[("MSG_RC", "1")]);
    assert!(!out.status.success(), "commit-msg exit 1 still blocks");
}

#[test]
fn previous_global_hooks_path_is_chained() {
    let prev_env = TestEnv::new();
    let prev = prev_env.root().join("old-hooks");
    std::fs::create_dir_all(&prev).unwrap();
    std::fs::write(
        prev.join("pre-commit"),
        marker_hook("prev-pre-commit", "PREV_RC"),
    )
    .unwrap();
    std::fs::write(
        prev.join("commit-msg"),
        marker_hook("prev-commit-msg", "PREV_RC"),
    )
    .unwrap();
    let h = HookEnv::new(Some(&prev), None);
    let repo = h.env.init_repo("r");
    write_repo_hook(
        &repo,
        "pre-commit",
        &marker_hook("repo-pre-commit", "REPO_RC"),
    );
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = h.calls();
    let r = calls.find("repo-pre-commit").expect("repo hook");
    let p = calls.find("prev-pre-commit").expect("previous hook");
    assert!(r < p, "repo hook runs before the previous global hook");
    assert!(calls.contains("prev-commit-msg"));
    let out = h.commit(&repo, "b.txt", &[("PREV_RC", "1")]);
    assert!(
        !out.status.success(),
        "previous hook exit code is propagated"
    );
}

#[test]
fn githop_disable_skips_check_but_keeps_chaining() {
    let h = HookEnv::new(None, None);
    let repo = h.env.init_repo("r");
    write_repo_hook(
        &repo,
        "pre-commit",
        &marker_hook("repo-pre-commit", "REPO_RC"),
    );
    let out = h.commit(&repo, "a.txt", &[("GITHOP_DISABLE", "1"), ("STUB_RC", "1")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!h.calls().contains("githop check"));
    assert!(h.calls().contains("repo-pre-commit"));
}

fn repo_with_remote(h: &HookEnv) -> (PathBuf, PathBuf) {
    let repo = h.env.init_repo("r");
    let remote = h.env.bare_remote("me/r");
    h.env
        .git(&["remote", "add", "origin", &slash(&remote)], &repo);
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    (repo, remote)
}

#[test]
fn pre_push_feeds_ref_lines_to_githop_and_repo_hook() {
    let h = HookEnv::new(None, None);
    let (repo, remote) = repo_with_remote(&h);
    write_repo_hook(&repo, "pre-push", &marker_hook("repo-pre-push", "REPO_RC"));
    h.clear();
    let out = h.git(
        &["push", "-q", "origin", "main"],
        &repo,
        &[("MARKER_READS_STDIN", "1")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let head = h.env.git(&["rev-parse", "HEAD"], &repo);
    let calls = h.calls();
    assert!(
        calls.contains("githop check --stage push -- origin "),
        "{calls}"
    );
    let line = format!("refs/heads/main {head} refs/heads/main {}", "0".repeat(40));
    assert!(calls.contains(&format!("githop-stdin {line}")), "{calls}");
    assert!(
        calls.contains(&format!("repo-pre-push-stdin {line}")),
        "{calls}"
    );
    assert_eq!(h.env.git(&["rev-parse", "main"], &remote), head);
}

#[test]
fn pre_push_without_chain_streams_stdin_and_blocks_on_1() {
    let h = HookEnv::new(None, None);
    let (repo, remote) = repo_with_remote(&h);
    h.clear();
    let out = h.git(
        &["push", "-q", "origin", "main"],
        &repo,
        &[("STUB_RC", "1")],
    );
    assert!(!out.status.success(), "push must be blocked");
    assert!(h.calls().contains("githop-stdin refs/heads/main "));
    assert!(
        h.env
            .git_output(&["rev-parse", "--verify", "main"], &remote)
            .status
            .code()
            != Some(0)
    );
}

#[test]
fn post_checkout_runs_check_only_for_a_fresh_clone() {
    let h = HookEnv::new(None, None);
    let (repo, remote) = repo_with_remote(&h);
    let push = h.git(&["push", "-q", "origin", "main"], &repo, &[]);
    assert!(push.status.success(), "{}", stderr(&push));
    h.clear();
    let dest = h.env.repos().join("cloned");
    let out = h.git(
        &["clone", "-q", &slash(&remote), &slash(&dest)],
        h.env.root(),
        &[("STUB_RC", "1")],
    );
    assert!(
        out.status.success(),
        "clone never blocked: {}",
        stderr(&out)
    );
    let calls = h.calls();
    assert!(
        calls.contains(&format!(
            "githop check --stage clone -- {} ",
            "0".repeat(40)
        )),
        "{calls}"
    );
    h.clear();
    let out = h.git(&["checkout", "-q", "-b", "feature"], &dest, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        !h.calls().contains("githop check"),
        "branch switch spawns no githop"
    );
}

#[test]
fn linked_worktree_chains_to_main_repo_hooks() {
    let h = HookEnv::new(None, None);
    let repo = h.env.init_repo("r");
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    write_repo_hook(
        &repo,
        "pre-commit",
        &marker_hook("repo-pre-commit", "REPO_RC"),
    );
    let wt = h.env.repos().join("wt");
    h.env
        .git(&["worktree", "add", "-q", "-b", "side", &slash(&wt)], &repo);
    h.clear();
    let out = h.commit(&wt, "b.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(h.calls().contains("repo-pre-commit"), "{}", h.calls());
}

#[test]
fn stdin_passthrough_buffers_for_both_hooks() {
    let prev_env = TestEnv::new();
    let prev = prev_env.root().join("old-hooks");
    std::fs::create_dir_all(&prev).unwrap();
    std::fs::write(
        prev.join("reference-transaction"),
        marker_hook("prev-rt", "NOPE"),
    )
    .unwrap();
    let h = HookEnv::new(Some(&prev), None);
    let repo = h.env.init_repo("r");
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    write_repo_hook(
        &repo,
        "reference-transaction",
        &marker_hook("repo-rt", "NOPE"),
    );
    h.clear();
    let out = h.git(&["branch", "topic"], &repo, &[("MARKER_READS_STDIN", "1")]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = h.calls();
    assert!(
        calls.contains("repo-rt-stdin ") && calls.contains("refs/heads/topic"),
        "{calls}"
    );
    assert!(calls.contains("prev-rt-stdin "), "{calls}");
}

#[test]
fn rendered_files_are_lf_and_hashable() {
    let h = HookEnv::new(None, None);
    for name in hooks::all_hook_names() {
        let bytes = std::fs::read(h.hooks_dir.join(name)).unwrap();
        assert!(!bytes.contains(&b'\r'));
        assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
        assert_eq!(hooks::sha256_hex(&bytes).len(), 64);
    }
}

// ---------------------------------------------------------------------------
// Part 2 — needs Phase 1 (gitconfig, resolver, store, pin, identity, repo)

const OFFICE: &str = "harshal@office.example";
const PERSONAL: &str = "harshal@personal.example";

fn write_profiles(env: &TestEnv) {
    let paths = env.paths();
    std::fs::create_dir_all(paths.root()).unwrap();
    let key = |id: &str| slash(&env.home().join(".ssh").join(format!("githop_{id}")));
    std::fs::write(
        paths.profiles_file(),
        format!(
            "version = 1\ndefault = \"personal\"\n\n\
             [[profile]]\nid = \"office\"\nlabel = \"Office\"\nname = \"Harshal Office\"\nemail = \"{OFFICE}\"\ncolor = \"blue\"\nssh_key = \"{}\"\nowners = [\"office-org\"]\n\n\
             [[profile]]\nid = \"personal\"\nlabel = \"Personal\"\nname = \"Harshal\"\nemail = \"{PERSONAL}\"\ncolor = \"violet\"\nssh_key = \"{}\"\nowners = [\"harshal\"]\n",
            key("office"),
            key("personal")
        ),
    )
    .unwrap();
}

fn ctx(env: &TestEnv, cwd: &Path) -> CheckContext {
    CheckContext {
        git: env.git_handle(),
        cwd: cwd.to_path_buf(),
        paths: env.paths(),
    }
}

/// Default = Personal globally; repo with an office-org origin.
fn office_repo(env: &TestEnv) -> PathBuf {
    env.set_global("user.name", "Harshal");
    env.set_global("user.email", PERSONAL);
    write_profiles(env);
    let repo = env.init_repo("billing-api");
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:office-org/billing-api.git",
        ],
        &repo,
    );
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    env.git(&["add", "a.txt"], &repo);
    repo
}

#[test]
#[ignore = "needs Phase 1 merged"]
fn plan_section_1_scenario_commit_blocked_then_passes() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    let r = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo));
    assert_eq!(
        r.outcome,
        Outcome::Mismatch {
            pinned_now: Some("office".into())
        },
        "{r:?}"
    );
    assert_eq!(r.exit_code(), 1);
    assert!(r
        .messages
        .iter()
        .any(|m| m.hint.as_deref() == Some("Run the commit again.")));
    assert_eq!(
        env.git(&["config", "--local", "githop.profile"], &repo),
        "office"
    );
    // Re-run: pinned, local email = Office → fast path pass.
    let r = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo));
    assert_eq!(r.outcome, Outcome::Pass, "{r:?}");
    assert_eq!(r.exit_code(), 0);
    env.git(&["commit", "-q", "-m", "fix bug"], &repo);
    assert_eq!(env.git(&["log", "-1", "--format=%ae"], &repo), OFFICE);
}

#[test]
#[ignore = "needs Phase 1 merged"]
fn pinned_repo_with_env_override_is_blocked() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    let _ = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo)); // pins
    let mut c = ctx(&env, &repo);
    c.git = c.git.env("GIT_AUTHOR_EMAIL", PERSONAL);
    let r = check::run_check(Stage::Commit, &[], "", &c);
    assert_eq!(r.outcome, Outcome::Mismatch { pinned_now: None }, "{r:?}");
    assert!(r
        .messages
        .iter()
        .any(|m| m.text.starts_with("GIT_AUTHOR_EMAIL is set")));
}

#[test]
#[ignore = "needs Phase 1 merged"]
fn warn_mode_lets_the_commit_through() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    std::fs::write(
        env.paths().settings_file(),
        "version = 1\nenforce = \"warn\"\n",
    )
    .unwrap();
    let r = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo));
    assert!(matches!(r.outcome, Outcome::Mismatch { .. }));
    assert_eq!(r.exit_code(), 0);
}

#[test]
#[ignore = "needs Phase 1 merged"]
fn corrupt_profiles_fail_open_unless_block_strict() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    std::fs::write(env.paths().profiles_file(), "[[[ not toml").unwrap();
    let r = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo));
    assert!(matches!(r.outcome, Outcome::InternalError { .. }), "{r:?}");
    assert_eq!(r.exit_code(), 3);
    std::fs::write(
        env.paths().settings_file(),
        "version = 1\nenforce = \"block-strict\"\n",
    )
    .unwrap();
    let r = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo));
    assert_eq!(r.exit_code(), 1);
}

#[test]
#[ignore = "needs Phase 1 merged"]
fn unresolved_repo_is_allowed_with_one_time_hint() {
    let env = TestEnv::new();
    env.set_global("user.email", PERSONAL);
    let paths = env.paths();
    std::fs::create_dir_all(paths.root()).unwrap();
    std::fs::write(paths.profiles_file(), "version = 1\n").unwrap();
    let repo = env.init_repo("loose");
    let r = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo));
    assert_eq!(r.outcome, Outcome::Unresolved);
    assert_eq!(r.exit_code(), 0);
    assert_eq!(r.messages.len(), 1);
    let r = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo));
    assert!(r.messages.is_empty(), "hint shown once per repo");
}

#[test]
#[ignore = "needs Phase 1 merged"]
fn push_blocks_exactly_the_wrong_commit() {
    let env = TestEnv::new();
    write_profiles(&env);
    env.redirect_github_to_local();
    let remote = env.bare_remote("office-org/billing-api");
    let repo = env.init_repo("billing-api");
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/office-org/billing-api.git",
        ],
        &repo,
    );
    env.commit_as(&repo, "a.txt", "H", OFFICE);
    env.git(&["push", "-q", "origin", "main"], &repo);
    let base = env.git(&["rev-parse", "HEAD"], &repo);
    env.commit_as(&repo, "b.txt", "H", OFFICE);
    let bad = env.commit_as(&repo, "c.txt", "H", PERSONAL);
    let head = env.commit_as(&repo, "d.txt", "H", OFFICE);
    let url = slash(&remote);
    let args = vec!["origin".to_string(), url];

    // Existing branch.
    let stdin = format!("refs/heads/main {head} refs/heads/main {base}\n");
    let r = check::run_check(Stage::Push, &args, &stdin, &ctx(&env, &repo));
    assert_eq!(r.exit_code(), 1, "{r:?}");
    assert_eq!(r.offending.len(), 1);
    assert_eq!(r.offending[0].sha, bad);

    // New branch.
    let stdin = format!(
        "refs/heads/topic {head} refs/heads/topic {}\n",
        "0".repeat(40)
    );
    let r = check::run_check(Stage::Push, &args, &stdin, &ctx(&env, &repo));
    assert_eq!(
        r.offending
            .iter()
            .map(|o| o.sha.as_str())
            .collect::<Vec<_>>(),
        vec![bad.as_str()]
    );

    // Delete only → ignored.
    let stdin = format!("(delete) {} refs/heads/old {base}\n", "0".repeat(40));
    let r = check::run_check(Stage::Push, &args, &stdin, &ctx(&env, &repo));
    assert_eq!(r.outcome, Outcome::Pass);

    // Remote sha we don't have (force push over unfetched work) → fallback range.
    let stdin = format!(
        "refs/heads/main {head} refs/heads/main {}\n",
        "e".repeat(40)
    );
    let r = check::run_check(Stage::Push, &args, &stdin, &ctx(&env, &repo));
    assert_eq!(r.offending.len(), 1, "{r:?}");
}

#[test]
#[ignore = "needs Phase 1 merged"]
fn clone_pins_to_owner_profile() {
    let env = TestEnv::new();
    write_profiles(&env);
    env.redirect_github_to_local();
    env.bare_remote("office-org/billing-api");
    let dest = env.repos().join("billing-api");
    // Clone without hooks, then run the clone check as post-checkout would.
    env.git(
        &[
            "clone",
            "-q",
            "https://github.com/office-org/billing-api.git",
            &slash(&dest),
        ],
        env.root(),
    );
    let args = vec!["0".repeat(40), "0".repeat(40), "1".to_string()];
    let r = check::run_check(Stage::Clone, &args, "", &ctx(&env, &dest));
    assert_eq!(
        r.outcome,
        Outcome::Pinned {
            profile_id: "office".into()
        },
        "{r:?}"
    );
    assert_eq!(r.messages[0].text, "GitHop: pinned to Office");
    assert_eq!(env.git(&["config", "--local", "user.email"], &dest), OFFICE);
}

#[test]
#[ignore = "needs Phase 1 merged"]
fn install_reinstall_verify_uninstall_roundtrip() {
    let env = TestEnv::new();
    let git = env.git_handle();
    let paths = env.paths();
    let prev = env.root().join("old hooks");
    std::fs::create_dir_all(&prev).unwrap();
    env.set_global("core.hooksPath", &slash(&prev));
    let exe = env.root().join("bin").join("githop.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "").unwrap();

    let mut settings = githop_core::Settings::default();
    let rep = hooks::install_hooks(&paths, &git, &mut settings, &exe).unwrap();
    assert_eq!(
        rep.previous_hooks_path.as_deref(),
        Some(slash(&prev).as_str())
    );
    let ours = hooks::hooks_dir_value(&paths);
    assert_eq!(
        env.git(&["config", "--global", "core.hooksPath"], &env.home()),
        ours
    );
    assert!(settings.hooks_installed);
    assert_eq!(settings.hook_hashes.len(), hooks::all_hook_names().len());

    let h = hooks::verify_hooks(&paths, &git, &settings);
    assert!(
        h.points_to_us && h.missing.is_empty() && h.tampered.is_empty() && !h.exe_missing,
        "{h:?}"
    );

    // Re-install must not chain to itself.
    let rep = hooks::install_hooks(&paths, &git, &mut settings, &exe).unwrap();
    assert!(rep.reinstall);
    assert_eq!(settings.previous_hooks_path, slash(&prev));

    // Tamper + delete.
    std::fs::write(paths.hooks_dir().join("pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::remove_file(paths.hooks_dir().join("commit-msg")).unwrap();
    let h = hooks::verify_hooks(&paths, &git, &settings);
    assert_eq!(h.tampered, vec!["pre-commit".to_string()]);
    assert_eq!(h.missing, vec!["commit-msg".to_string()]);
    assert!(h.needs_reinstall());

    let u = hooks::uninstall_hooks(&paths, &git, &mut settings).unwrap();
    assert_eq!(
        u.hooks_path,
        hooks::HooksPathChange::Restored {
            value: slash(&prev)
        }
    );
    assert_eq!(
        env.git(&["config", "--global", "core.hooksPath"], &env.home()),
        slash(&prev)
    );
    assert!(!paths.hooks_dir().join("pre-push").exists());
    assert!(!settings.hooks_installed);
}

#[test]
#[ignore = "needs Phase 1 merged"]
fn uninstall_without_previous_unsets_hooks_path() {
    let env = TestEnv::new();
    let git = env.git_handle();
    let paths = env.paths();
    let exe = env.root().join("githop.exe");
    let mut settings = githop_core::Settings::default();
    hooks::install_hooks(&paths, &git, &mut settings, &exe).unwrap();
    hooks::uninstall_hooks(&paths, &git, &mut settings).unwrap();
    let out = env.git_output(&["config", "--global", "core.hooksPath"], &env.home());
    assert_eq!(out.status.code(), Some(1), "unset");
}
