//! CLI surface tests that need no core implementation: clap parsing, help text,
//! exit codes, non-TTY missing-flag errors and the Phase 3/4 stubs.

mod support;

use predicates::prelude::*;
use predicates::str::contains;
use support::TestEnv;

const SUBCOMMANDS: &[&str] = &[
    "init", "add", "edit", "remove", "list", "use", "status", "pin", "unpin", "scan", "keygen",
    "test", "doctor", "wipe", "check", "hooks", "fix", "export", "import",
];

/// Words Design §9.2 says to avoid (matched as whole words, case-insensitive).
const AVOID: &[&str] = &[
    "persona",
    "nuke",
    "rebase",
    "rewrite",
    "namespace",
    "rejected",
];

fn has_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric())
        .any(|w| w.eq_ignore_ascii_case(word))
}

/// Help lines must fit Design §9.4's 80 columns.
fn assert_fits_80(text: &str, what: &str) {
    for line in text.lines() {
        assert!(
            line.chars().count() <= 80,
            "{what}: line over 80 columns: {line}"
        );
    }
}

#[test]
fn version_flag() {
    let env = TestEnv::new();
    env.githop()
        .arg("--version")
        .assert()
        .code(0)
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_lists_every_command_and_uses_glossary() {
    let env = TestEnv::new();
    let out = env.githop().arg("--help").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    for cmd in SUBCOMMANDS {
        assert!(
            text.contains(&format!("  {cmd} ")),
            "help is missing {cmd}:\n{text}"
        );
    }
    for word in ["profile", "default profile", "Pin", "Wipe", "Fix"] {
        assert!(text.contains(word), "help should use glossary word {word}");
    }
    for word in AVOID {
        assert!(!has_word(&text, word), "help uses avoided word {word}");
    }
    assert_fits_80(&text, "githop --help");
    for flag in ["--json", "--no-color", "--quiet", "--verbose"] {
        assert!(text.contains(flag), "help is missing global flag {flag}");
    }
}

#[test]
fn every_subcommand_has_help() {
    let env = TestEnv::new();
    for cmd in SUBCOMMANDS {
        let out = env.githop().args([cmd, "--help"]).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "githop {cmd} --help failed");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains("Usage:"),
            "githop {cmd} --help has no usage line"
        );
        for word in AVOID {
            assert!(
                !has_word(&text, word),
                "githop {cmd} --help uses avoided word {word}"
            );
        }
        assert_fits_80(&text, &format!("githop {cmd} --help"));
        // Regression D14: the usage line names githop, not githop.exe.
        assert!(!text.contains("githop.exe"), "githop {cmd} --help: {text}");
    }
}

#[test]
fn no_subcommand_is_usage_error() {
    let env = TestEnv::new();
    env.githop().assert().code(2);
}

#[test]
fn unknown_subcommand_is_usage_error() {
    let env = TestEnv::new();
    env.githop().arg("frobnicate").assert().code(2);
}

#[test]
fn unknown_flag_is_usage_error() {
    let env = TestEnv::new();
    env.githop().args(["list", "--bogus"]).assert().code(2);
}

#[test]
fn missing_positional_is_usage_error() {
    let env = TestEnv::new();
    for cmd in ["edit", "remove", "use", "pin", "keygen", "test", "scan"] {
        env.githop().arg(cmd).assert().code(2);
    }
}

#[test]
fn bad_colour_is_usage_error() {
    let env = TestEnv::new();
    env.githop()
        .args(["add", "--id", "x", "--color", "green"])
        .assert()
        .code(2)
        .stderr(contains("blue"));
}

#[test]
fn quiet_and_verbose_conflict() {
    let env = TestEnv::new();
    env.githop().args(["-q", "-v", "list"]).assert().code(2);
}

#[test]
fn add_without_tty_lists_all_missing_fields() {
    let env = TestEnv::new();
    env.githop()
        .arg("add")
        .assert()
        .code(2)
        .stderr(contains("--id"))
        .stderr(contains("--name"))
        .stderr(contains("--email"))
        .stderr(contains("Pass them as flags"));
}

#[test]
fn add_without_tty_lists_only_missing_fields() {
    let env = TestEnv::new();
    env.githop()
        .args(["add", "--id", "office", "--name", "Me"])
        .assert()
        .code(2)
        .stderr(contains("--email"))
        .stderr(contains("--name").not());
}

#[test]
fn json_is_refused_for_commands_without_json_output() {
    let env = TestEnv::new();
    env.githop()
        .args(["add", "--json"])
        .assert()
        .code(2)
        .stderr(contains("--json isn't available"));
    env.githop()
        .args(["--json", "use", "office"])
        .assert()
        .code(2);
}

#[test]
fn wipe_without_tty_needs_yes() {
    let env = TestEnv::new();
    env.githop()
        .arg("wipe")
        .assert()
        .code(2)
        .stderr(contains("githop wipe --yes"));
    // Nothing was touched.
    assert!(env.appdata().read_dir().unwrap().next().is_none());
}

#[test]
fn wipe_flags_conflict() {
    let env = TestEnv::new();
    env.githop()
        .args(["wipe", "--yes", "--keep-keys", "--delete-keys"])
        .assert()
        .code(2);
}

#[test]
fn scan_missing_folder_is_usage_error() {
    let env = TestEnv::new();
    let missing = env.root().join("no-such-folder");
    env.githop()
        .arg("scan")
        .arg(&missing)
        .assert()
        .code(2)
        .stderr(contains("There's no folder"));
}

#[test]
fn scan_include_default_requires_apply() {
    let env = TestEnv::new();
    env.githop()
        .arg("scan")
        .arg(env.repos())
        .arg("--include-default")
        .assert()
        .code(2);
}

#[test]
fn check_outside_a_repo_fails_open() {
    // Not a repo: GitHop can't judge, so it warns and exits 3 (hooks go on).
    let env = TestEnv::new();
    env.githop()
        .args(["check", "--stage", "commit"])
        .assert()
        .code(3)
        .stdout("")
        .stderr(contains(
            "! GitHop couldn't check this commit (it went through).",
        ))
        .stderr(contains("Run: githop doctor"));
}

#[test]
fn check_is_silent_and_passes_when_disabled() {
    let env = TestEnv::new();
    for stage in ["clone", "commit", "push"] {
        env.githop()
            .args(["check", "--stage", stage, "--", "origin", "x"])
            .env("GITHOP_DISABLE", "1")
            .write_stdin("")
            .assert()
            .code(0)
            .stdout("")
            .stderr("");
    }
}

#[test]
fn check_json_prints_the_result() {
    let env = TestEnv::new();
    let out = env
        .githop()
        .args(["--json", "check", "--stage", "push"])
        .env("GITHOP_DISABLE", "1")
        .write_stdin("")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["stage"], "push");
    assert_eq!(v["outcome"]["kind"], "disabled");
}

#[test]
fn check_needs_a_valid_stage() {
    let env = TestEnv::new();
    env.githop().arg("check").assert().code(2);
    env.githop()
        .args(["check", "--stage", "merge"])
        .assert()
        .code(2);
}

#[test]
fn fix_outside_a_repo_is_a_usage_error() {
    let env = TestEnv::new();
    env.githop()
        .args(["fix", "--dry-run"])
        .assert()
        .code(2)
        .stderr(contains("not a git repo"));
}

#[test]
fn errors_have_no_colour_codes_with_no_color() {
    let env = TestEnv::new();
    let out = env.githop().arg("add").output().unwrap();
    assert!(!String::from_utf8_lossy(&out.stderr).contains('\x1b'));
    let out = env
        .githop()
        .args(["--no-color", "add"])
        .env_remove("NO_COLOR")
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stderr).contains('\x1b'));
}

#[test]
fn ascii_fallback_for_errors() {
    let env = TestEnv::new();
    env.githop()
        .arg("add")
        .env("GITHOP_ASCII", "1")
        .assert()
        .code(2)
        .stderr(contains("[blocked]"));
    env.githop()
        .arg("add")
        .assert()
        .code(2)
        .stderr(contains("✗"));
}
