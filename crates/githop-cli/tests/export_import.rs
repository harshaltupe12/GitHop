//! `githop export` / `githop import` end to end, between two isolated temp
//! "machines" (CLAUDE.md rule 3: never the real `~/.ssh` or `%APPDATA%`).
//!
//! The password comes from `GITHOP_EXPORT_PASSWORD` (no terminal in tests).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};

use predicates::str::contains;
use serde_json::Value;
use support::TestEnv;

const PASSWORD: &str = "correct horse battery staple";
const OFFICE_EMAIL: &str = "me@company.com";
const PERSONAL_EMAIL: &str = "12345+me@users.noreply.github.com";

fn write_pem_key(env: &TestEnv, name: &str) -> PathBuf {
    std::fs::create_dir_all(env.ssh_dir()).unwrap();
    let key = env.ssh_dir().join(name);
    std::fs::write(
        &key,
        format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{name}\n-----END OPENSSH PRIVATE KEY-----\n"),
    )
    .unwrap();
    std::fs::write(
        env.ssh_dir().join(format!("{name}.pub")),
        format!("ssh-ed25519 AAAAC3Nz {name}\n"),
    )
    .unwrap();
    key
}

fn machine_a() -> TestEnv {
    let env = TestEnv::new();
    env.add_profile("office", "Me (Work)", OFFICE_EMAIL, &["office-org"]);
    env.add_profile("personal", "Me", PERSONAL_EMAIL, &["me-personal"]);
    write_pem_key(&env, "githop_office");
    write_pem_key(&env, "githop_personal");
    env
}

/// `githop <args>` with the password in the environment.
fn run_with_password(
    env: &TestEnv,
    password: Option<&str>,
    args: &[&str],
) -> (i32, String, String) {
    let mut cmd = env.githop();
    match password {
        Some(p) => cmd.env("GITHOP_EXPORT_PASSWORD", p),
        None => cmd.env_remove("GITHOP_EXPORT_PASSWORD"),
    };
    let out = cmd.args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn export(env: &TestEnv, file: &Path, keys: bool) {
    let file = TestEnv::slash(file);
    let mut args = vec!["export", file.as_str()];
    if keys {
        args.push("--include-keys");
    }
    let (code, out, err) = run_with_password(env, Some(PASSWORD), &args);
    assert_eq!(code, 0, "export failed:\n{out}\n{err}");
    assert!(out.contains("Exported 2 profiles"), "{out}");
}

fn ids(env: &TestEnv) -> Vec<String> {
    let (code, out, err) = env.run(&env.home(), &["--json", "list"]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn round_trip_with_keys() {
    let a = machine_a();
    let file = a.root().join("my profiles.githop");
    export(&a, &file, true);
    let bytes = std::fs::read(&file).unwrap();
    assert!(bytes.starts_with(b"age-encryption.org/v1"));
    assert!(!String::from_utf8_lossy(&bytes).contains(OFFICE_EMAIL));

    let b = TestEnv::new();
    let file_arg = TestEnv::slash(&file);
    let (code, out, err) = run_with_password(&b, Some(PASSWORD), &["import", &file_arg, "--yes"]);
    assert_eq!(code, 0, "import failed:\n{out}\n{err}");
    assert!(out.contains("This file has 2 profiles"), "{out}");
    assert!(out.contains("Imported 2 profiles"), "{out}");
    assert!(out.contains("SSH key saved"), "{out}");
    assert_eq!(ids(&b), ["office", "personal"]);
    for name in ["githop_office", "githop_personal"] {
        assert_eq!(
            std::fs::read(b.ssh_dir().join(name)).unwrap(),
            std::fs::read(a.ssh_dir().join(name)).unwrap()
        );
    }

    // Same file again: every profile clashes now and there's no terminal to ask.
    let (code, _, err) = run_with_password(&b, Some(PASSWORD), &["import", &file_arg, "--yes"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("clash"), "{err}");

    // Skip them all: nothing to do, nothing changed.
    let (code, out, err) = run_with_password(
        &b,
        Some(PASSWORD),
        &["import", &file_arg, "--yes", "--on-conflict", "skip"],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Nothing to import"), "{out}");
    assert_eq!(ids(&b), ["office", "personal"]);
}

#[test]
fn round_trip_without_keys_points_at_this_pcs_ssh_folder() {
    let a = machine_a();
    let file = a.root().join("profiles.githop");
    export(&a, &file, false);

    let b = TestEnv::new();
    let (code, out, err) = run_with_password(
        &b,
        Some(PASSWORD),
        &["import", &TestEnv::slash(&file), "--yes"],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("has no SSH key on this PC yet"), "{out}");
    assert!(out.contains("githop keygen office"), "{out}");
    let (_, out, _) = b.run(&b.home(), &["--json", "list"]);
    let v: Value = serde_json::from_str(&out).unwrap();
    let key = v["profiles"][0]["ssh_key"]
        .as_str()
        .unwrap()
        .replace('\\', "/");
    assert_eq!(key, TestEnv::slash(&b.ssh_dir().join("githop_office")));
    assert!(!b.ssh_dir().join("githop_office").exists());
}

#[test]
fn rename_on_conflict() {
    let a = machine_a();
    let file = a.root().join("profiles.githop");
    export(&a, &file, true);

    // B already has an "office" profile with another email and its own key.
    let b = TestEnv::new();
    b.add_profile("office", "Someone", "someone@else.com", &[]);
    let own_key = write_pem_key(&b, "githop_office");
    std::fs::write(
        &own_key,
        "-----BEGIN OPENSSH PRIVATE KEY-----\nB's own key\n-----END OPENSSH PRIVATE KEY-----\n",
    )
    .unwrap();
    let before = std::fs::read(b.ssh_dir().join("githop_office")).unwrap();

    let (code, out, err) = run_with_password(
        &b,
        Some(PASSWORD),
        &[
            "import",
            &TestEnv::slash(&file),
            "--yes",
            "--on-conflict",
            "rename",
        ],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("clashes with your profiles"), "{out}");
    assert!(out.contains("office is now office-2"), "{out}");
    assert_eq!(ids(&b), ["office", "office-2", "personal"]);
    assert_eq!(
        std::fs::read(b.ssh_dir().join("githop_office")).unwrap(),
        before
    );
    assert!(b.ssh_dir().join("githop_office-2").exists());
}

#[test]
fn short_password_is_refused_and_nothing_written() {
    let a = machine_a();
    let file = a.root().join("p.githop");
    let (code, _, err) =
        run_with_password(&a, Some("too short"), &["export", &TestEnv::slash(&file)]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("at least 12 characters"), "{err}");
    assert!(!file.exists());
}

#[test]
fn wrong_password_and_garbage_files() {
    let a = machine_a();
    let file = a.root().join("p.githop");
    export(&a, &file, false);
    let b = TestEnv::new();

    let (code, _, err) = run_with_password(
        &b,
        Some("this is not the password"),
        &["import", &TestEnv::slash(&file), "--yes"],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("password is wrong"), "{err}");

    let junk = b.root().join("junk.githop");
    std::fs::write(&junk, "hello, I am not encrypted").unwrap();
    let (code, _, err) = run_with_password(
        &b,
        Some(PASSWORD),
        &["import", &TestEnv::slash(&junk), "--yes"],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("isn't a GitHop export"), "{err}");

    let (code, _, err) = run_with_password(
        &b,
        Some(PASSWORD),
        &["import", &TestEnv::slash(&b.root().join("missing.githop"))],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no file"), "{err}");
    assert!(ids(&b).is_empty());
}

#[test]
fn no_terminal_and_no_password_env_is_usage_error() {
    let a = machine_a();
    let file = a.root().join("p.githop");
    let (code, _, err) = run_with_password(&a, None, &["export", &TestEnv::slash(&file)]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no terminal"), "{err}");
    assert!(err.contains("GITHOP_EXPORT_PASSWORD"), "{err}");
}

#[test]
fn existing_export_file_needs_yes() {
    let a = machine_a();
    let file = a.root().join("p.githop");
    export(&a, &file, false);
    let (code, _, err) = run_with_password(&a, Some(PASSWORD), &["export", &TestEnv::slash(&file)]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--yes"), "{err}");
    let (code, out, err) = run_with_password(
        &a,
        Some(PASSWORD),
        &["export", &TestEnv::slash(&file), "--yes"],
    );
    assert_eq!(code, 0, "{out}\n{err}");
}

#[test]
fn json_is_not_offered() {
    let a = machine_a();
    a.githop()
        .env("GITHOP_EXPORT_PASSWORD", PASSWORD)
        .args(["--json", "export", "x.githop"])
        .assert()
        .code(2)
        .stderr(contains("--json isn't available"));
}
