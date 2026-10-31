//! Isolated git environment for integration tests (CLAUDE.md rule 3).
//!
//! Every git process spawned through `TestEnv` sees a temp `HOME`/`USERPROFILE`,
//! a temp `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_NOSYSTEM=1` and a temp `GITHOP_HOME`,
//! so tests never read or write the developer's real config.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use githop_core::{Git, Paths};

/// Env vars cleared for every spawned process.
const CLEARED_ENV: &[&str] = &[
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
];

pub struct TestEnv {
    dir: tempfile::TempDir,
}

impl TestEnv {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("githop-test-")
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

    pub fn global_config(&self) -> PathBuf {
        self.home().join(".gitconfig")
    }

    /// GitHop data dir for this test.
    pub fn paths(&self) -> Paths {
        Paths::at(self.appdata())
    }

    /// Env vars to set on any child process (git, githop.exe, sh).
    pub fn envs(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("HOME", self.home()),
            ("USERPROFILE", self.home()),
            ("GIT_CONFIG_GLOBAL", self.global_config()),
            ("GITHOP_HOME", self.appdata()),
            ("APPDATA", self.appdata()),
        ]
    }

    /// Apply the isolated environment to a `Command`.
    pub fn apply(&self, cmd: &mut Command) {
        for (k, v) in self.envs() {
            cmd.env(k, v);
        }
        cmd.env("GIT_CONFIG_NOSYSTEM", "1");
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        for k in CLEARED_ENV {
            cmd.env_remove(k);
        }
    }

    /// A `githop_core::Git` handle wired to the isolated environment.
    pub fn git_handle(&self) -> Git {
        let mut g = Git::locate().expect("git on PATH");
        for (k, v) in self.envs() {
            g = g.env(k, v);
        }
        g = g
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        for k in CLEARED_ENV {
            g = g.env_remove(k);
        }
        g
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

    /// `git init` a working repo at `repos/<name>` (branch `main`).
    pub fn init_repo(&self, name: &str) -> PathBuf {
        let path = self.repos().join(name);
        std::fs::create_dir_all(&path).expect("create repo dir");
        self.git(&["init", "-q", "-b", "main"], &path);
        path
    }

    /// Bare repo at `remotes/<owner>/<repo>.git`, e.g. `bare_remote("office-org/billing-api")`.
    pub fn bare_remote(&self, owner_repo: &str) -> PathBuf {
        let path = self.remotes().join(format!("{owner_repo}.git"));
        std::fs::create_dir_all(&path).expect("create remote dir");
        self.git(&["init", "-q", "--bare", "-b", "main"], &path);
        path
    }

    /// Make `https://github.com/<owner>/<repo>.git` resolve to the local bare
    /// remotes via `url.<base>.insteadOf`, so tests can use real GitHub-style URLs
    /// without any network access.
    pub fn redirect_github_to_local(&self) {
        let base = format!("{}/", self.remotes().to_string_lossy().replace('\\', "/"));
        self.set_global(&format!("url.{base}.insteadOf"), "https://github.com/");
    }

    /// Write a file and commit it with an explicit identity.
    pub fn commit_as(&self, repo: &Path, file: &str, name: &str, email: &str) -> String {
        std::fs::write(repo.join(file), format!("{file}\n")).expect("write file");
        self.git(&["add", file], repo);
        self.git(
            &[
                "-c",
                &format!("user.name={name}"),
                "-c",
                &format!("user.email={email}"),
                "commit",
                "-q",
                "-m",
                &format!("add {file}"),
            ],
            repo,
        );
        self.git(&["rev-parse", "HEAD"], repo)
    }
}
