//! Isolated environment for CLI integration tests (CLAUDE.md rule 3).
//!
//! Mirrors `crates/githop-core/tests/support/mod.rs`: every spawned `githop` or
//! `git` sees a temp `HOME`/`USERPROFILE`, a temp `GIT_CONFIG_GLOBAL`,
//! `GIT_CONFIG_NOSYSTEM=1`, a temp `GITHOP_HOME`/`APPDATA`, `NO_COLOR=1`, and has
//! the identity/ssh override variables cleared — tests never touch real config.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Env vars cleared for every spawned process.
pub const CLEARED_ENV: &[&str] = &[
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
    "GIT_SSH_COMMAND",
    "GIT_SSH",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "EMAIL",
    "GITHOP_DISABLE",
    "GITHOP_ASCII",
    "GITHOP_DEBUG",
    "CLICOLOR_FORCE",
];

pub struct TestEnv {
    dir: tempfile::TempDir,
}

impl TestEnv {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("githop-cli-test-")
            .tempdir()
            .expect("create temp dir");
        for sub in ["home", "appdata", "repos", "remotes"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("create sub dir");
        }
        // Empty global config so git never falls back to the real one.
        std::fs::write(dir.path().join("home").join(".gitconfig"), "").expect("write .gitconfig");
        Self { dir }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    pub fn appdata(&self) -> PathBuf {
        self.root().join("appdata")
    }

    pub fn repos(&self) -> PathBuf {
        self.root().join("repos")
    }

    pub fn remotes(&self) -> PathBuf {
        self.root().join("remotes")
    }

    pub fn ssh_dir(&self) -> PathBuf {
        self.home().join(".ssh")
    }

    pub fn global_config(&self) -> PathBuf {
        self.home().join(".gitconfig")
    }

    /// Forward-slash string form of a path (as GitHop stores paths).
    pub fn slash(p: &Path) -> String {
        p.to_string_lossy().replace('\\', "/")
    }

    /// Env vars to set on any child process.
    pub fn envs(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("HOME", self.home()),
            ("USERPROFILE", self.home()),
            ("GIT_CONFIG_GLOBAL", self.global_config()),
            ("GITHOP_HOME", self.appdata()),
            ("APPDATA", self.appdata()),
        ]
    }

    /// Apply the isolated environment to a std `Command`.
    pub fn apply(&self, cmd: &mut Command) {
        for (k, v) in self.envs() {
            cmd.env(k, v);
        }
        cmd.env("GIT_CONFIG_NOSYSTEM", "1");
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.env("NO_COLOR", "1");
        for k in CLEARED_ENV {
            cmd.env_remove(k);
        }
    }

    /// `githop` (the binary under test) in `cwd`, with the isolated environment.
    pub fn githop_in(&self, cwd: &Path) -> assert_cmd::Command {
        let mut std_cmd = Command::new(env!("CARGO_BIN_EXE_githop"));
        std_cmd.current_dir(cwd);
        self.apply(&mut std_cmd);
        assert_cmd::Command::from_std(std_cmd)
    }

    /// `githop` in the temp home dir.
    pub fn githop(&self) -> assert_cmd::Command {
        self.githop_in(&self.home())
    }

    /// Run git in `cwd`; returns the full output (does not assert success).
    pub fn git_output(&self, args: &[&str], cwd: &Path) -> Output {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(cwd);
        self.apply(&mut cmd);
        cmd.output().expect("spawn git")
    }

    /// Run git in `cwd`, assert success, return trimmed stdout.
    pub fn git(&self, args: &[&str], cwd: &Path) -> String {
        let out = self.git_output(args, cwd);
        assert!(
            out.status.success(),
            "git {:?} failed in {}:\nstdout: {}\nstderr: {}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Set a value in the isolated global config.
    pub fn set_global(&self, key: &str, value: &str) {
        let home = self.home();
        self.git(&["config", "--global", key, value], &home);
    }

    /// Read a value from the isolated global config (`None` when unset).
    pub fn get_global(&self, key: &str) -> Option<String> {
        let home = self.home();
        let out = self.git_output(&["config", "--global", "--get", key], &home);
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Read a value from a repo's local config (`None` when unset).
    pub fn get_local(&self, repo: &Path, key: &str) -> Option<String> {
        let out = self.git_output(&["config", "--local", "--get", key], repo);
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// `git init` a working repo at `repos/<name>` (branch `main`).
    pub fn init_repo(&self, name: &str) -> PathBuf {
        let path = self.repos().join(name);
        std::fs::create_dir_all(&path).expect("create repo dir");
        self.git(&["init", "-q", "-b", "main"], &path);
        path
    }

    /// Repo at `repos/<name>` with `origin` = `https://github.com/<owner>/<name>.git`
    /// (never fetched — tests stay offline).
    pub fn repo_with_origin(&self, name: &str, owner: &str) -> PathBuf {
        let path = self.init_repo(name);
        self.git(
            &[
                "remote",
                "add",
                "origin",
                &format!("https://github.com/{owner}/{name}.git"),
            ],
            &path,
        );
        path
    }

    /// Write `~/.ssh/config` in the temp home.
    pub fn write_ssh_config(&self, contents: &str) {
        std::fs::create_dir_all(self.ssh_dir()).expect("create .ssh");
        std::fs::write(self.ssh_dir().join("config"), contents).expect("write ssh config");
    }

    /// Create a placeholder key pair (not a real key) for profiles that need a file.
    pub fn fake_key(&self, name: &str) -> PathBuf {
        std::fs::create_dir_all(self.ssh_dir()).expect("create .ssh");
        let key = self.ssh_dir().join(name);
        std::fs::write(&key, "placeholder\n").expect("write key");
        std::fs::write(
            self.ssh_dir().join(format!("{name}.pub")),
            "ssh-ed25519 AAAA test\n",
        )
        .expect("write pub");
        key
    }

    /// Run `githop` and return (exit code, stdout, stderr).
    pub fn run(&self, cwd: &Path, args: &[&str]) -> (i32, String, String) {
        let out = self.githop_in(cwd).args(args).output().expect("run githop");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// `githop add` with flags; asserts success.
    pub fn add_profile(&self, id: &str, name: &str, email: &str, owners: &[&str]) {
        let key = Self::slash(&self.ssh_dir().join(format!("githop_{id}")));
        let mut args = vec![
            "add", "--id", id, "--name", name, "--email", email, "--key", &key,
        ];
        for o in owners {
            args.push("--owner");
            args.push(o);
        }
        let (code, out, err) = self.run(&self.home(), &args);
        assert_eq!(code, 0, "add {id} failed:\n{out}\n{err}");
    }
}
