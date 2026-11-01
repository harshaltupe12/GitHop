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

#![allow(clippy::unwrap_used, clippy::expect_used)]

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
    /// The `settings.toml` the scripts read `enforce` from.
    settings: PathBuf,
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
        let prev = previous.map(slash).unwrap_or_default();
        Self::new_with(&prev, exe_override, false)
    }

    /// Like [`HookEnv::new`] with a raw previous value (may be relative) and
    /// the block-strict flag.
    fn new_with(prev: &str, exe_override: Option<&Path>, strict: bool) -> Self {
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
        let settings = env.root().join("settings.toml");
        let mode = if strict { "block-strict" } else { "block" };
        std::fs::write(&settings, format!("version = 1\nenforce = \"{mode}\"\n")).unwrap();
        let opts = hooks::RenderOptions {
            settings_file: slash(&settings),
            ..hooks::RenderOptions::new(&slash(&exe), prev)
        };
        for (name, body) in hooks::render_names_with(&hooks::all_hook_names(), &opts).unwrap() {
            assert!(!body.contains('\r'));
            std::fs::write(hooks_dir.join(name), body).unwrap();
        }
        env.set_global("core.hooksPath", &slash(&hooks_dir));
        let log = env.root().join("calls.log");
        Self {
            env,
            hooks_dir,
            log,
            settings,
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
    // Design §9.3 fail-open wording (regression D22).
    assert!(stderr(&out)
        .contains("! GitHop couldn't check this commit (it went through). Run: githop doctor"));
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

fn githop_commit_checks(calls: &str) -> usize {
    calls.matches("githop check --stage commit").count()
}

fn head(h: &HookEnv, repo: &Path) -> String {
    h.env.git(&["rev-parse", "HEAD"], repo)
}

/// main: a, c · feat: a, b (so `git merge feat` makes a merge commit).
fn diverged_repo(h: &HookEnv) -> PathBuf {
    let repo = h.env.init_repo("r");
    assert!(h.commit(&repo, "a.txt", &[]).status.success());
    h.env.git(&["checkout", "-q", "-b", "feat"], &repo);
    assert!(h.commit(&repo, "b.txt", &[]).status.success());
    h.env.git(&["checkout", "-q", "main"], &repo);
    assert!(h.commit(&repo, "c.txt", &[]).status.success());
    repo
}

/// Review 2026-11-01 #5: git runs pre-merge-commit (not pre-commit) for a
/// merge commit; GitHop checks there and its exit 1 stops the merge.
#[test]
fn merge_commit_runs_the_commit_check_and_can_block() {
    let h = HookEnv::new(None, None);
    let repo = diverged_repo(&h);
    let before = head(&h, &repo);
    h.clear();
    let out = h.git(
        &["merge", "-q", "--no-edit", "feat"],
        &repo,
        &[("STUB_RC", "1")],
    );
    assert!(!out.status.success(), "merge must be blocked");
    assert_eq!(head(&h, &repo), before, "no merge commit");
    assert_eq!(githop_commit_checks(&h.calls()), 1, "{}", h.calls());
    let _ = h.git(&["merge", "--abort"], &repo, &[]);
    h.env.git(&["reset", "-q", "--hard"], &repo);

    h.clear();
    let out = h.git(&["merge", "-q", "--no-edit", "feat"], &repo, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_ne!(head(&h, &repo), before);
    assert_eq!(githop_commit_checks(&h.calls()), 1, "{}", h.calls());
}

/// Review 2026-11-01 #5: revert and cherry-pick run no pre-commit; GitHop
/// checks them in prepare-commit-msg. A plain commit is still checked once,
/// `--no-verify` still skips GitHop (C15), and rebase is left to push.
#[test]
fn revert_and_cherry_pick_are_checked_once_and_no_verify_still_skips() {
    let h = HookEnv::new(None, None);
    let repo = diverged_repo(&h);
    let before = head(&h, &repo);

    h.clear();
    let out = h.git(&["revert", "--no-edit", "HEAD"], &repo, &[("STUB_RC", "1")]);
    assert!(!out.status.success(), "revert must be blocked");
    assert_eq!(head(&h, &repo), before);
    assert_eq!(githop_commit_checks(&h.calls()), 1, "{}", h.calls());
    h.env.git(&["reset", "-q", "--hard"], &repo);

    h.clear();
    let out = h.git(&["cherry-pick", "feat"], &repo, &[("STUB_RC", "1")]);
    assert!(!out.status.success(), "cherry-pick must be blocked");
    assert_eq!(head(&h, &repo), before);
    assert_eq!(githop_commit_checks(&h.calls()), 1, "{}", h.calls());
    let _ = h.git(&["cherry-pick", "--abort"], &repo, &[]);
    h.env.git(&["reset", "-q", "--hard"], &repo);

    h.clear();
    let out = h.git(&["cherry-pick", "feat"], &repo, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(githop_commit_checks(&h.calls()), 1, "{}", h.calls());

    h.clear();
    let out = h.commit(&repo, "d.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        githop_commit_checks(&h.calls()),
        1,
        "a plain commit is checked once (pre-commit only): {}",
        h.calls()
    );

    h.clear();
    std::fs::write(repo.join("e.txt"), "e\n").unwrap();
    h.env.git(&["add", "e.txt"], &repo);
    let out = h.git(
        &["commit", "-q", "--no-verify", "-m", "e"],
        &repo,
        &[("STUB_RC", "1")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        githop_commit_checks(&h.calls()),
        0,
        "--no-verify skips GitHop"
    );

    h.clear();
    h.env.git(&["checkout", "-q", "feat"], &repo);
    let out = h.git(
        &["rebase", "-q", "--force-rebase", "main"],
        &repo,
        &[("STUB_RC", "1")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        githop_commit_checks(&h.calls()),
        0,
        "rebase: push checks it"
    );
}

/// Review 2026-11-01 #8: previous global hooks path = the repo's own
/// `.git/hooks` must not run the repo hook twice.
#[test]
fn previous_hooks_path_equal_to_repo_hooks_runs_the_hook_once() {
    let h = HookEnv::new_with(".git/hooks", None, false);
    let repo = h.env.init_repo("r");
    write_repo_hook(&repo, "commit-msg", &marker_hook("repo-commit-msg", "NOPE"));
    write_repo_hook(&repo, "pre-commit", &marker_hook("repo-pre-commit", "NOPE"));
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = h.calls();
    assert_eq!(calls.matches("repo-commit-msg").count(), 1, "{calls}");
    assert_eq!(calls.matches("repo-pre-commit").count(), 1, "{calls}");

    // The same folder spelled differently (compared as files, not strings).
    let h = HookEnv::new_with("./.git/hooks/", None, false);
    let repo = h.env.init_repo("r");
    write_repo_hook(&repo, "commit-msg", &marker_hook("repo-commit-msg", "NOPE"));
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(h.calls().matches("repo-commit-msg").count(), 1);
}

/// Review 2026-11-01 #9: under block-strict, a missing or crashing githop
/// blocks (Plan §12) instead of failing open.
#[test]
fn block_strict_blocks_when_githop_is_missing_or_crashes() {
    let missing = PathBuf::from("C:/definitely/not/here/githop.exe");
    let h = HookEnv::new_with("", Some(&missing), true);
    let repo = h.env.init_repo("r");
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(!out.status.success(), "missing githop must block");
    assert!(
        stderr(&out).contains("so the commit was blocked (block-strict mode)"),
        "{}",
        stderr(&out)
    );

    let h = HookEnv::new_with("", None, true);
    let (repo, remote) = repo_with_remote(&h);
    for rc in ["101", "2"] {
        let out = h.commit(&repo, &format!("x{rc}.txt"), &[("STUB_RC", rc)]);
        assert!(!out.status.success(), "rc {rc} must block");
        assert!(stderr(&out).contains("couldn't check this commit, so it was blocked"));
    }
    // Exit 3 is GitHop's own explained failure. githop itself exits 1 for
    // that under block-strict, so a 3 always fails open.
    let out = h.commit(&repo, "x3.txt", &[("STUB_RC", "3")]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = h.git(
        &["push", "-q", "origin", "main"],
        &repo,
        &[("STUB_RC", "101")],
    );
    assert!(!out.status.success(), "push blocked too");
    assert!(
        h.env
            .git_output(&["rev-parse", "--verify", "main"], &remote)
            .status
            .code()
            != Some(0)
    );
    // Passing checks still pass.
    let out = h.git(&["push", "-q", "origin", "main"], &repo, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
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
fn pinned_repo_with_env_override_is_blocked() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    let _ = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo)); // pins
    let mut c = ctx(&env, &repo);
    c.git = c.git.env("GIT_AUTHOR_EMAIL", PERSONAL);
    let r = check::run_check(Stage::Commit, &[], "", &c);
    assert_eq!(r.outcome, Outcome::Mismatch { pinned_now: None }, "{r:?}");
    // Review #11: `--author` reaches the hook the same way, so name both.
    assert!(
        r.messages.iter().any(|m| m.text
            == format!(
                "This commit's author email is {PERSONAL} (from --author or GIT_AUTHOR_EMAIL)."
            )),
        "{r:?}"
    );
}

#[test]
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

/// ADR 0006: a fresh clone over GitHub HTTPS, policy `auto` → pinned, switched
/// to SSH, GitHub's host keys added (temp HOME). Policy `ask` → only a hint.
#[test]
fn clone_switches_https_remote_under_auto_and_seeds_known_hosts() {
    for policy in ["auto", "ask"] {
        let env = TestEnv::new();
        write_profiles(&env);
        std::fs::write(
            env.paths().settings_file(),
            format!("version = 1\nremote_https_policy = \"{policy}\"\n"),
        )
        .unwrap();
        let dest = env.init_repo("billing-api");
        env.git(
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/office-org/billing-api.git",
            ],
            &dest,
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
        let url = env.git(&["config", "--local", "remote.origin.url"], &dest);
        let texts: Vec<&str> = r.messages.iter().map(|m| m.text.as_str()).collect();
        if policy == "auto" {
            assert_eq!(url, "git@github.com:office-org/billing-api.git");
            assert!(
                texts.contains(&"GitHop: switched origin to SSH, so pushes use Office's key"),
                "{texts:?}"
            );
        } else {
            assert_eq!(url, "https://github.com/office-org/billing-api.git");
            assert!(
                texts
                    .iter()
                    .any(|t| t.contains("Uses HTTPS: Office's SSH key isn't used")),
                "{texts:?}"
            );
        }
        let kh = std::fs::read_to_string(env.home().join(".ssh").join("known_hosts")).unwrap();
        assert!(githop_core::ssh::known_hosts_trusts_github(&kh));
    }
}

/// ADR 0006: pushing to a GitHub HTTPS URL warns (never blocks) unless the
/// policy is `never`.
#[test]
fn push_over_https_warns_without_blocking() {
    for (policy, warned) in [("ask", true), ("auto", true), ("never", false)] {
        let env = TestEnv::new();
        write_profiles(&env);
        std::fs::write(
            env.paths().settings_file(),
            format!("version = 1\nremote_https_policy = \"{policy}\"\n"),
        )
        .unwrap();
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
        let head = env.commit_as(&repo, "a.txt", "H", OFFICE);
        let args = vec![
            "origin".to_string(),
            "https://github.com/office-org/billing-api.git".to_string(),
        ];
        let stdin = format!(
            "refs/heads/main {head} refs/heads/main {}\n",
            "0".repeat(40)
        );
        let r = check::run_check(Stage::Push, &args, &stdin, &ctx(&env, &repo));
        assert_eq!(r.exit_code(), 0, "{r:?}");
        let has = r
            .messages
            .iter()
            .any(|m| m.text.starts_with("GitHop: this push uses HTTPS"));
        assert_eq!(has, warned, "{policy}: {r:?}");
    }
}

#[test]
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
    assert_eq!(
        settings.hook_hashes.len(),
        hooks::installed_hook_names(false).len()
    );
    for optional in hooks::OPTIONAL_HOOKS {
        assert!(!settings.hook_hashes.contains_key(optional), "{optional}");
    }

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

// ---------------------------------------------------------------------------
// Review 2026-10-31 #8 — spawn budget (Plan §11): commit fast path = 1 spawn,
// slow path = 2, push = 2. Counted with `Git::spawn_count`.

/// Spawns made by one `run_check`.
fn spawns_for(stage: Stage, c: &CheckContext, args: &[String], stdin: &str) -> (usize, Outcome) {
    let before = c.git.spawn_count();
    let r = check::run_check(stage, args, stdin, c);
    (c.git.spawn_count() - before, r.outcome)
}

#[test]
fn commit_check_spawn_budget() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    // Unpinned, wrong identity: snapshot + git var, then the pin (1 local read
    // + 4 writes). No `rev-parse` anywhere.
    let (n, outcome) = spawns_for(Stage::Commit, &ctx(&env, &repo), &[], "");
    assert!(matches!(outcome, Outcome::Mismatch { .. }), "{outcome:?}");
    assert_eq!(n, 7);

    // Pinned and matching: fast path, 1 spawn.
    let (n, outcome) = spawns_for(Stage::Commit, &ctx(&env, &repo), &[], "");
    assert_eq!(outcome, Outcome::Pass);
    assert_eq!(n, 1, "fast path");

    // Slow path (`author.email` config forces `git var`): 2 spawns. An env value
    // equal to config no longer counts as an override (git exports it to hooks).
    env.git(&["config", "--local", "author.email", OFFICE], &repo);
    let c = ctx(&env, &repo);
    let (n, outcome) = spawns_for(Stage::Commit, &c, &[], "");
    assert_eq!(outcome, Outcome::Pass);
    assert_eq!(n, 2, "slow path");
}

#[test]
fn blocked_commit_records_last_block_for_the_tray() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    let marker = env.appdata().join(check::LAST_BLOCK_FILE);
    assert!(!marker.exists());
    let (_, outcome) = spawns_for(Stage::Commit, &ctx(&env, &repo), &[], "");
    assert!(matches!(outcome, Outcome::Mismatch { .. }), "{outcome:?}");
    let secs: u64 = std::fs::read_to_string(&marker).unwrap().parse().unwrap();
    assert!(secs > 1_700_000_000);
}

#[test]
fn commit_check_in_linked_worktree_spawn_budget() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    env.git(&["commit", "-q", "-m", "init"], &repo);
    let wt = env.repos().join("wt");
    env.git(&["worktree", "add", "-q", "-b", "wt", &slash(&wt)], &repo);
    env.git(&["config", "--local", "githop.profile", "office"], &repo);
    env.git(&["config", "--local", "user.email", OFFICE], &repo);

    // Via the worktree's `.git` file.
    let (n, outcome) = spawns_for(Stage::Commit, &ctx(&env, &wt), &[], "");
    assert_eq!(outcome, Outcome::Pass);
    assert_eq!(n, 1);

    // As git runs the hook in a linked worktree: GIT_DIR exported.
    let gitdir = env.git(&["rev-parse", "--absolute-git-dir"], &wt);
    let mut c = ctx(&env, &wt);
    c.git = c.git.env("GIT_DIR", &gitdir);
    let (n, outcome) = spawns_for(Stage::Commit, &c, &[], "");
    assert_eq!(outcome, Outcome::Pass);
    assert_eq!(n, 1);

    // Slow path in the worktree: 2.
    env.git(&["config", "--local", "author.email", OFFICE], &repo);
    let (n, outcome) = spawns_for(Stage::Commit, &c, &[], "");
    assert_eq!(outcome, Outcome::Pass);
    assert_eq!(n, 2);
}

#[test]
fn push_check_spawn_budget() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    env.git(&["config", "--local", "githop.profile", "office"], &repo);
    env.git(&["config", "--local", "user.email", OFFICE], &repo);
    env.git(&["commit", "-q", "-m", "init"], &repo);
    let head = env.git(&["rev-parse", "HEAD"], &repo);
    let stdin = format!(
        "refs/heads/main {head} refs/heads/main {}\n",
        "0".repeat(40)
    );
    let args = vec![
        "origin".to_string(),
        "git@github.com:office-org/billing-api.git".to_string(),
    ];
    let (n, outcome) = spawns_for(Stage::Push, &ctx(&env, &repo), &args, &stdin);
    assert_eq!(outcome, Outcome::Pass);
    assert_eq!(n, 2);
}

/// Review #2: a hook recording a pin must not wipe a tray/CLI change that
/// landed after the hook loaded the store (it uses `Store::update_settings`).
#[test]
fn hook_pin_record_keeps_concurrent_settings() {
    let env = TestEnv::new();
    let repo = office_repo(&env);
    let mut tray = githop_core::Store::open_at(env.paths()).unwrap();
    tray.update_settings(|s| {
        s.previous_hooks_path = "C:/my hooks".into();
        Ok(())
    })
    .unwrap();
    let r = check::run_check(Stage::Commit, &[], "", &ctx(&env, &repo));
    assert!(matches!(r.outcome, Outcome::Mismatch { .. }));
    let s = githop_core::Store::open_at(env.paths()).unwrap();
    assert_eq!(s.settings().previous_hooks_path, "C:/my hooks");
    assert_eq!(s.settings().pinned_repos.len(), 1);
}

// ---------------------------------------------------------------------------
// Review 2026-11-01 #1 — core.hooksPath from [include] / [includeIf]

/// git with the isolated env + the stub log.
fn git_logged(
    env: &TestEnv,
    log: &Path,
    args: &[&str],
    cwd: &Path,
    vars: &[(&str, &str)],
) -> Output {
    let mut c = Command::new("git");
    c.args(args).current_dir(cwd);
    env.apply(&mut c);
    c.env("STUB_LOG", slash(log));
    for (k, v) in vars {
        c.env(k, v);
    }
    c.output().unwrap()
}

/// Company hooks set through an `includeIf` for `<repos>/work/`, the way the
/// review reproduced it. Returns (stub githop, work dir, log).
fn corp_include_env(env: &TestEnv) -> (PathBuf, PathBuf, PathBuf) {
    env.set_global("user.name", "Test Person");
    env.set_global("user.email", "test@example.invalid");
    let bin = env.root().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("githop");
    std::fs::write(&stub, STUB).unwrap();
    let corp = env.root().join("corp-hooks");
    std::fs::create_dir_all(&corp).unwrap();
    std::fs::write(
        corp.join("pre-commit"),
        marker_hook("corp-pre-commit", "CORP_RC"),
    )
    .unwrap();
    std::fs::write(
        env.home().join(".gitconfig-work"),
        format!("[core]\n\thooksPath = {}\n", slash(&corp)),
    )
    .unwrap();
    let work = env.repos().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let log = env.root().join("calls.log");
    (stub, work, log)
}

fn append_global(env: &TestEnv, text: &str) {
    let mut g = std::fs::read_to_string(env.global_config()).unwrap();
    g.push_str(text);
    std::fs::write(env.global_config(), g).unwrap();
}

fn include_if_work(work: &Path) -> String {
    format!(
        "[includeIf \"gitdir/i:{}/\"]\n\tpath = ~/.gitconfig-work\n",
        slash(work)
    )
}

fn work_repo(env: &TestEnv, work: &Path) -> PathBuf {
    let repo = work.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    env.git(&["init", "-q", "-b", "main"], &repo);
    repo
}

fn commit_in(env: &TestEnv, log: &Path, repo: &Path, file: &str, vars: &[(&str, &str)]) -> Output {
    std::fs::write(repo.join(file), "x\n").unwrap();
    env.git(&["add", file], repo);
    git_logged(env, log, &["commit", "-q", "-m", file], repo, vars)
}

fn read_log(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

#[test]
fn include_if_hooks_path_keeps_githop_on_and_chains_company_hooks() {
    let env = TestEnv::new();
    let (stub, work, log) = corp_include_env(&env);
    append_global(&env, &include_if_work(&work));
    let before = std::fs::read(env.global_config()).unwrap();
    let git = env.git_handle();
    let paths = env.paths();
    let mut settings = githop_core::Settings::default();
    let rep = hooks::install_hooks(&paths, &git, &mut settings, &stub).unwrap();
    assert_eq!(rep.included_hooks_paths.len(), 1, "{rep:?}");
    assert!(rep.included_hooks_paths[0]
        .condition
        .as_deref()
        .unwrap()
        .starts_with("gitdir/i:"));
    let ours = hooks::hooks_dir_value(&paths);

    let repo = work_repo(&env, &work);
    let other = env.init_repo("other");
    // GitHop's value is what git uses, in and out of the includeIf folder.
    assert_eq!(env.git(&["config", "core.hooksPath"], &repo), ours);
    assert_eq!(env.git(&["config", "core.hooksPath"], &other), ours);

    let out = commit_in(&env, &log, &repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = read_log(&log);
    assert!(calls.contains("githop check --stage commit"), "{calls}");
    assert!(
        calls.contains("corp-pre-commit"),
        "company hook chained: {calls}"
    );

    let _ = std::fs::remove_file(&log);
    let out = commit_in(&env, &log, &other, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = read_log(&log);
    assert!(calls.contains("githop check --stage commit"), "{calls}");
    assert!(
        !calls.contains("corp-pre-commit"),
        "includeIf doesn't apply here: {calls}"
    );

    let out = commit_in(&env, &log, &repo, "b.txt", &[("STUB_RC", "1")]);
    assert!(
        !out.status.success(),
        "GitHop still blocks in the work folder"
    );
    let out = commit_in(&env, &log, &repo, "c.txt", &[("CORP_RC", "1")]);
    assert!(!out.status.success(), "company hook still blocks");

    let h = hooks::verify_hooks(&paths, &git, &settings);
    assert!(h.points_to_us && h.overridden_by.is_empty(), "{h:?}");
    assert!(
        h.missing.is_empty() && h.tampered.is_empty() && h.stale.is_empty(),
        "{h:?}"
    );
    assert_eq!(h.included_hooks_paths.len(), 1);

    // Re-install leaves the config file alone.
    let after_install = std::fs::read(env.global_config()).unwrap();
    hooks::install_hooks(&paths, &git, &mut settings, &stub).unwrap();
    assert_eq!(std::fs::read(env.global_config()).unwrap(), after_install);

    let u = hooks::uninstall_hooks(&paths, &git, &mut settings).unwrap();
    assert_eq!(u.hooks_path, hooks::HooksPathChange::Unset);
    assert_eq!(
        String::from_utf8_lossy(&std::fs::read(env.global_config()).unwrap()),
        String::from_utf8_lossy(&before),
        "uninstall leaves the global file exactly as it was"
    );
    assert!(!hooks::include_file_path(&paths).exists());
}

/// The user's own global value stays in place in include mode, keeps being
/// chained where the includeIf doesn't apply, and is in effect again after
/// uninstall.
#[test]
fn include_mode_keeps_the_users_own_value_in_place() {
    let env = TestEnv::new();
    let (stub, work, log) = corp_include_env(&env);
    let mine = env.root().join("my hooks");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(
        mine.join("pre-commit"),
        marker_hook("my-pre-commit", "NOPE"),
    )
    .unwrap();
    env.set_global("core.hooksPath", &slash(&mine));
    append_global(&env, &include_if_work(&work));
    let before = std::fs::read(env.global_config()).unwrap();
    let git = env.git_handle();
    let paths = env.paths();
    let mut settings = githop_core::Settings::default();
    let rep = hooks::install_hooks(&paths, &git, &mut settings, &stub).unwrap();
    assert_eq!(
        rep.previous_hooks_path.as_deref(),
        Some(slash(&mine).as_str())
    );
    // One value in the file itself, so `git config --global core.hooksPath X`
    // keeps working for the user.
    assert_eq!(
        env.git(&["config", "--global", "core.hooksPath"], &env.home()),
        slash(&mine)
    );

    let repo = work_repo(&env, &work);
    let other = env.init_repo("other");
    let out = commit_in(&env, &log, &other, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = read_log(&log);
    assert!(
        calls.contains("githop check") && calls.contains("my-pre-commit"),
        "{calls}"
    );
    let _ = std::fs::remove_file(&log);
    let out = commit_in(&env, &log, &repo, "a.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = read_log(&log);
    // Without GitHop git would use the includeIf value here, not mine.
    assert!(
        calls.contains("githop check") && calls.contains("corp-pre-commit"),
        "{calls}"
    );
    assert!(!calls.contains("my-pre-commit"), "{calls}");

    let u = hooks::uninstall_hooks(&paths, &git, &mut settings).unwrap();
    assert_eq!(
        u.hooks_path,
        hooks::HooksPathChange::Restored {
            value: slash(&mine)
        }
    );
    assert_eq!(std::fs::read(env.global_config()).unwrap(), before);
}

fn problem_in(r: &githop_core::doctor::DoctorReport, id: &str) -> bool {
    r.by_id(id)
        .any(|c| c.status == githop_core::doctor::CheckStatus::Problem)
}

/// An install made before the include existed (GitHop's value in the global
/// file, the includeIf after it): doctor flags it globally and in the repo,
/// and a re-install fixes it.
#[test]
fn doctor_flags_an_include_read_after_githop_and_reinstall_fixes_it() {
    let env = TestEnv::new();
    let (stub, work, log) = corp_include_env(&env);
    let git = env.git_handle();
    let paths = env.paths();
    let mut store = githop_core::Store::open_at(paths.clone()).unwrap();
    store
        .update_settings(|s| hooks::install_hooks(&paths, &git, s, &stub))
        .unwrap();
    append_global(&env, &include_if_work(&work));
    let repo = work_repo(&env, &work);

    let h = hooks::verify_hooks(&paths, &git, store.settings());
    assert_eq!(h.overridden_by.len(), 1, "{h:?}");
    assert!(!h.is_ok() && h.needs_reinstall());

    // The defect itself: GitHop's hooks don't run in the work repo.
    let out = commit_in(&env, &log, &repo, "a.txt", &[("STUB_RC", "1")]);
    assert!(out.status.success());
    assert!(!read_log(&log).contains("githop check"));

    let opts = githop_core::doctor::DoctorOptions {
        cwd: Some(repo.clone()),
        ..Default::default()
    };
    let report = githop_core::doctor::diagnose(&paths, Some(&git), &opts);
    assert!(problem_in(&report, "hooks-path"), "{:#?}", report.checks);
    assert!(
        problem_in(&report, "repo-hooks-path"),
        "{:#?}",
        report.checks
    );
    let repo_check = report.by_id("repo-hooks-path").next().unwrap();
    assert!(repo_check.detail.contains("corp-hooks"), "{repo_check:?}");
    assert!(repo_check
        .fix
        .as_deref()
        .unwrap()
        .contains("githop hooks install"));

    store
        .update_settings(|s| hooks::install_hooks(&paths, &git, s, &stub))
        .unwrap();
    let report = githop_core::doctor::diagnose(&paths, Some(&git), &opts);
    assert!(!problem_in(&report, "hooks-path"), "{:#?}", report.checks);
    assert!(
        !problem_in(&report, "repo-hooks-path"),
        "{:#?}",
        report.checks
    );
    assert!(report.by_id("hooks-included").next().is_some());
    let out = commit_in(&env, &log, &repo, "b.txt", &[("STUB_RC", "1")]);
    assert!(!out.status.success(), "GitHop runs again in the work repo");
}

/// Regression D5: the enforce mode is read at run time, so changing it
/// doesn't make the hooks stale; `refresh_if_installed` keeps working.
#[test]
fn enforce_change_leaves_hooks_current() {
    let env = TestEnv::new();
    let git = env.git_handle();
    let paths = env.paths();
    let exe = env.root().join("githop.exe");
    std::fs::write(&exe, "").unwrap();
    let mut settings = githop_core::Settings::default();
    hooks::install_hooks(&paths, &git, &mut settings, &exe).unwrap();
    assert!(hooks::verify_hooks(&paths, &git, &settings)
        .stale
        .is_empty());
    let config_before = std::fs::read(env.global_config()).unwrap();

    settings.enforce = githop_core::EnforceMode::BlockStrict;
    let h = hooks::verify_hooks(&paths, &git, &settings);
    assert!(h.stale.is_empty() && h.tampered.is_empty(), "{h:?}");
    assert!(hooks::refresh_if_installed(&paths, &git, &mut settings).unwrap());
    let h = hooks::verify_hooks(&paths, &git, &settings);
    assert!(h.stale.is_empty() && h.tampered.is_empty(), "{h:?}");
    let pre_commit = std::fs::read_to_string(paths.hooks_dir().join("pre-commit")).unwrap();
    assert!(pre_commit.contains(&format!("SETTINGS=\"{}\"\n", slash(&paths.settings_file()))));
    assert!(!pre_commit.contains("STRICT="));
    assert_eq!(
        std::fs::read(env.global_config()).unwrap(),
        config_before,
        "no git config write"
    );

    let mut off = githop_core::Settings::default();
    assert!(!hooks::refresh_if_installed(&paths, &git, &mut off).unwrap());
}

/// Review #4: a clone that only the default profile covers is not pinned, so
/// an owner rule added later still wins.
#[test]
fn clone_with_only_the_default_profile_is_not_pinned() {
    let env = TestEnv::new();
    write_profiles(&env);
    env.redirect_github_to_local();
    env.bare_remote("acme-corp/svc");
    let dest = env.repos().join("svc");
    env.git(
        &[
            "clone",
            "-q",
            "https://github.com/acme-corp/svc.git",
            &slash(&dest),
        ],
        env.root(),
    );
    let args = vec!["0".repeat(40), "0".repeat(40), "1".to_string()];
    let r = check::run_check(Stage::Clone, &args, "", &ctx(&env, &dest));
    assert_eq!(r.outcome, Outcome::Pass, "{r:?}");
    assert_eq!(r.profile_id.as_deref(), Some("personal"));
    assert!(r.messages[0].text.contains("no owner match"), "{r:?}");
    assert_eq!(
        r.messages[0].hint.as_deref(),
        Some("To keep it that way: githop pin personal")
    );
    assert_eq!(
        env.git_output(&["config", "--local", "githop.profile"], &dest)
            .status
            .code(),
        Some(1),
        "not pinned"
    );
    let s = githop_core::Store::open_at(env.paths()).unwrap();
    assert!(s.settings().pinned_repos.is_empty());
}

/// Regression D5: block-strict is read from settings.toml at run time. A
/// mode changed by hand applies at once, in both directions, without
/// re-rendering the hooks.
#[test]
fn block_strict_is_read_at_run_time() {
    let missing = PathBuf::from("C:/definitely/not/here/githop.exe");
    let h = HookEnv::new_with("", Some(&missing), true);
    let repo = h.env.init_repo("r");
    let out = h.commit(&repo, "a.txt", &[]);
    assert!(!out.status.success(), "strict + missing exe blocks");
    // The user switches back by hand.
    std::fs::write(&h.settings, "version = 1\nenforce = \"block\"\n").unwrap();
    let out = h.commit(&repo, "b.txt", &[]);
    assert!(
        out.status.success(),
        "block mode fails open: {}",
        stderr(&out)
    );
    assert!(!stderr(&out).contains("blocked"), "{}", stderr(&out));
    // ...and to strict again, CRLF and indentation as an editor may leave it.
    std::fs::write(
        &h.settings,
        "version = 1\r\n  enforce = 'block-strict'\r\n[hook_hashes]\r\n",
    )
    .unwrap();
    let out = h.commit(&repo, "c.txt", &[]);
    assert!(!out.status.success(), "strict again");
    // A key in a [table] doesn't count.
    std::fs::write(
        &h.settings,
        "version = 1\n[other]\nenforce = \"block-strict\"\n",
    )
    .unwrap();
    let out = h.commit(&repo, "d.txt", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
}

/// Regression D13: the scripts' own lines honour GITHOP_ASCII.
#[test]
fn hook_script_messages_honour_ascii_mode() {
    let missing = PathBuf::from("C:/definitely/not/here/githop.exe");
    let h = HookEnv::new_with("", Some(&missing), true);
    let repo = h.env.init_repo("r");
    let out = h.commit(&repo, "a.txt", &[("GITHOP_ASCII", "1")]);
    let err = stderr(&out);
    assert!(
        err.contains("[blocked] GitHop isn't installed correctly"),
        "{err}"
    );
    assert!(!err.contains('✗'), "{err}");
    std::fs::write(&h.settings, "version = 1\n").unwrap();
    let out = h.commit(&repo, "b.txt", &[("GITHOP_ASCII", "1")]);
    assert!(
        stderr(&out).contains("[warn] GitHop couldn't check this commit"),
        "{}",
        stderr(&out)
    );
}

/// Regression D4: with GITHOP_DISABLE=1 the pre-push hook still hands the
/// ref lines to githop (which only records them), and prints nothing.
#[test]
fn disabled_push_still_calls_githop_to_record() {
    let h = HookEnv::new(None, None);
    let (repo, _remote) = repo_with_remote(&h);
    h.clear();
    let out = h.git(
        &["push", "-q", "origin", "main"],
        &repo,
        &[("GITHOP_DISABLE", "1"), ("STUB_RC", "1")],
    );
    assert!(
        out.status.success(),
        "disabled push goes through: {}",
        stderr(&out)
    );
    let calls = h.calls();
    assert!(calls.contains("githop check --stage push --"), "{calls}");
    assert!(calls.contains("githop-stdin refs/heads/main"), "{calls}");
    // Commit hooks still skip githop entirely.
    h.clear();
    let out = h.commit(&repo, "z.txt", &[("GITHOP_DISABLE", "1"), ("STUB_RC", "1")]);
    assert!(out.status.success());
    assert!(!h.calls().contains("githop check"));
}

/// Security regression #3: a revert or cherry-pick made while a blocked
/// merge left MERGE_HEAD behind reaches prepare-commit-msg with source
/// "merge"; it is checked. `git merge` itself (GIT_REFLOG_ACTION=merge…) was
/// already checked in pre-merge-commit and isn't checked twice.
#[test]
fn prepare_commit_msg_checks_merge_source_from_the_sequencer() {
    let h = HookEnv::new(None, None);
    let repo = h.env.init_repo("r");
    let sh = hooks::find_sh(&h.env.git_handle()).expect("sh");
    let script = slash(&h.hooks_dir.join("prepare-commit-msg"));
    let run = |reflog: Option<&str>| {
        h.clear();
        let mut c = Command::new(&sh);
        c.args([script.as_str(), ".git/COMMIT_EDITMSG", "merge"])
            .current_dir(&repo);
        h.env.apply(&mut c);
        c.env("STUB_LOG", slash(&h.log)).env("STUB_RC", "1");
        match reflog {
            Some(r) => c.env("GIT_REFLOG_ACTION", r),
            None => c.env_remove("GIT_REFLOG_ACTION"),
        };
        let out = c.output().unwrap();
        (out.status.code(), githop_commit_checks(&h.calls()))
    };
    assert_eq!(
        run(None),
        (Some(1), 1),
        "sequencer commit checked and blocked"
    );
    assert_eq!(run(Some("revert")), (Some(1), 1));
    assert_eq!(
        run(Some("merge feat")).1,
        0,
        "git merge: checked in pre-merge-commit"
    );
    assert_eq!(run(Some("pull")).1, 0);
}
