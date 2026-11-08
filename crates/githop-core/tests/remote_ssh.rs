//! Integration tests for HTTPS → SSH remote switching and the pinned
//! `core.sshCommand` (ADR 0006). Isolated env, no network: ssh is only run
//! with `-V` / `-G`, which never connect.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::process::Command;

use githop_core::gitconfig::ConfigScope;
use githop_core::profile::{Profile, ProfileColor};
use githop_core::remote_ssh::{self, HttpsAction};
use githop_core::{hooks, pin, RemoteHttpsPolicy, Repo};
use support::TestEnv;

fn local(env: &TestEnv, repo: &Path, key: &str) -> Option<String> {
    let out = env.git_output(&["config", "--local", "--get", key], repo);
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
fn switch_and_undo_round_trip() {
    let env = TestEnv::new();
    let dir = env.init_repo("app");
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "https://me:ghp_secret@github.com/janedoe/GitHop.git",
        ],
        &dir,
    );
    env.git(
        &["remote", "add", "lab", "https://gitlab.com/me/app.git"],
        &dir,
    );
    env.git(
        &[
            "config",
            "--local",
            "remote.origin.pushurl",
            "https://github.com/janedoe/GitHop",
        ],
        &dir,
    );
    let repo = Repo::discover(&env.git_handle(), &dir).unwrap();

    let found = remote_ssh::https_remotes(&repo.config_snapshot().unwrap());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "origin");
    assert_eq!(found[0].url, "https://me@github.com/janedoe/GitHop.git");
    assert_eq!(found[0].ssh_url, "git@github.com:janedoe/GitHop.git");

    let switched = remote_ssh::switch_all_to_ssh(&repo).unwrap();
    assert_eq!(switched.len(), 1);
    assert_eq!(
        local(&env, &dir, "remote.origin.url").as_deref(),
        Some("git@github.com:janedoe/GitHop.git")
    );
    assert_eq!(
        local(&env, &dir, "remote.origin.pushurl").as_deref(),
        Some("git@github.com:janedoe/GitHop.git")
    );
    // Non-GitHub remote untouched.
    assert_eq!(
        local(&env, &dir, "remote.lab.url").as_deref(),
        Some("https://gitlab.com/me/app.git")
    );
    // Saved for undo, without the token (CLAUDE.md rule 8).
    let saved = local(&env, &dir, "githop.saved-remote-url-origin").unwrap();
    assert_eq!(saved, "https://me@github.com/janedoe/GitHop.git");
    let config = std::fs::read_to_string(dir.join(".git").join("config")).unwrap();
    assert!(!config.contains("ghp_secret"), "{config}");

    let snap = repo.config_snapshot().unwrap();
    assert!(remote_ssh::https_remotes(&snap).is_empty());
    assert_eq!(
        remote_ssh::switched_remotes(&snap),
        vec!["origin".to_string()]
    );

    assert_eq!(
        remote_ssh::restore_all(&repo).unwrap(),
        vec!["origin".to_string()]
    );
    assert_eq!(
        local(&env, &dir, "remote.origin.url").as_deref(),
        Some("https://me@github.com/janedoe/GitHop.git")
    );
    assert_eq!(
        local(&env, &dir, "remote.origin.pushurl").as_deref(),
        Some("https://github.com/janedoe/GitHop")
    );
    assert_eq!(local(&env, &dir, "githop.saved-remote-url-origin"), None);
    assert_eq!(
        local(&env, &dir, "githop.saved-remote-pushurl-origin"),
        None
    );
    assert!(remote_ssh::switched_remotes(&repo.config_snapshot().unwrap()).is_empty());
}

#[test]
fn undo_leaves_a_url_the_user_changed_since() {
    let env = TestEnv::new();
    let dir = env.init_repo("changed");
    env.git(
        &["remote", "add", "my-fork", "https://github.com/me/app"],
        &dir,
    );
    let repo = Repo::discover(&env.git_handle(), &dir).unwrap();
    assert!(remote_ssh::switch_remote_to_ssh(&repo, "my-fork")
        .unwrap()
        .is_some());
    assert!(local(&env, &dir, "githop.saved-remote-url-my-2dfork").is_some());
    env.git(
        &[
            "remote",
            "set-url",
            "my-fork",
            "git@github.com:other/app.git",
        ],
        &dir,
    );
    assert!(!remote_ssh::restore_remote_url(&repo, "my-fork").unwrap());
    assert_eq!(
        local(&env, &dir, "remote.my-fork.url").as_deref(),
        Some("git@github.com:other/app.git")
    );
    assert_eq!(local(&env, &dir, "githop.saved-remote-url-my-2dfork"), None);
    // A non-GitHub remote can't be switched.
    env.git(
        &["remote", "add", "lab", "https://gitlab.com/me/app.git"],
        &dir,
    );
    assert_eq!(
        remote_ssh::switch_remote_to_ssh(&repo, "lab").unwrap(),
        None
    );
}

#[test]
fn apply_policy_switches_only_under_auto() {
    let env = TestEnv::new();
    for (name, policy) in [
        ("ask", RemoteHttpsPolicy::Ask),
        ("never", RemoteHttpsPolicy::Never),
        ("auto", RemoteHttpsPolicy::Auto),
    ] {
        let dir = env.init_repo(name);
        env.git(
            &["remote", "add", "origin", "https://github.com/o/r.git"],
            &dir,
        );
        let repo = Repo::discover(&env.git_handle(), &dir).unwrap();
        let action = remote_ssh::apply_policy(&repo, policy).unwrap();
        let url = local(&env, &dir, "remote.origin.url").unwrap();
        match policy {
            RemoteHttpsPolicy::Auto => {
                assert!(matches!(action, HttpsAction::Switched(ref l) if l.len() == 1));
                assert_eq!(url, "git@github.com:o/r.git");
            }
            RemoteHttpsPolicy::Ask => {
                assert!(matches!(action, HttpsAction::Ask(_)));
                assert_eq!(url, "https://github.com/o/r.git");
            }
            RemoteHttpsPolicy::Never => {
                assert!(matches!(action, HttpsAction::Warn(_)));
                assert_eq!(url, "https://github.com/o/r.git");
            }
        }
        // Second time: nothing left to do.
        assert_eq!(
            remote_ssh::apply_policy(&repo, RemoteHttpsPolicy::Auto).unwrap()
                == HttpsAction::Nothing,
            policy == RemoteHttpsPolicy::Auto
        );
    }
}

/// Git runs `core.sshCommand` through its own `sh` for terminal git and for
/// VS Code's Git alike (same config). Run the pinned value exactly that way,
/// with `-G` (print the resolved config, no connection) and an empty `-F`
/// config file, and check the program, key and IdentitiesOnly.
#[test]
fn pinned_ssh_command_runs_under_gits_sh() {
    let env = TestEnv::new();
    let git = env.git_handle();
    let Some(sh) = hooks::find_sh(&git) else {
        eprintln!("skipping: Git's sh not found");
        return;
    };
    let dir = env.init_repo("pinned");
    let key = env
        .home()
        .join("Jane Doe")
        .join(".ssh")
        .join("githop_personal");
    // ssh only lists an -i file that exists (contents don't matter for -G).
    std::fs::create_dir_all(key.parent().unwrap()).unwrap();
    std::fs::write(&key, "not a real key").unwrap();
    let profile = Profile {
        id: "personal".into(),
        label: "Personal".into(),
        name: "Me".into(),
        email: "me@users.noreply.github.com".into(),
        color: ProfileColor::Violet,
        ssh_key: key.clone(),
        owners: vec!["janedoe".into()],
        folders: vec![],
        sign_commits: false,
    };
    let repo = Repo::discover(&git, &dir).unwrap();
    pin::pin_repo(&repo, &profile).unwrap();
    let cmd = repo
        .config_snapshot()
        .unwrap()
        .get_scoped("core.sshCommand", ConfigScope::Local)
        .unwrap()
        .to_string();
    assert_eq!(cmd, pin::ssh_command_for(&key));

    let empty_cfg = env.root().join("ssh_config_empty");
    std::fs::write(&empty_cfg, "").unwrap();
    let cfg = empty_cfg.to_string_lossy().replace('\\', "/");
    let run = |extra: &str| {
        let mut c = Command::new(&sh);
        c.arg("-c").arg(format!("{cmd} -F \"{cfg}\" {extra}"));
        env.apply(&mut c);
        c.current_dir(&dir).output().unwrap()
    };
    let out = run("-G git@github.com");
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    assert!(
        out.status.success(),
        "{cmd}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("identitiesonly yes"), "{text}");
    let key_slash = key.to_string_lossy().replace('\\', "/").to_lowercase();
    let key_back = key.to_string_lossy().to_lowercase();
    assert!(
        text.lines()
            .any(|l| l.starts_with("identityfile ")
                && (l.contains(&key_slash) || l.contains(&key_back))),
        "{text}"
    );
    if let Some(prog) = githop_core::ssh::git_ssh_program() {
        // The agent-capable Windows OpenSSH is what actually runs.
        assert!(cmd.starts_with(&format!(
            "\"{}\"",
            prog.to_string_lossy().replace('\\', "/")
        )));
        let v = run("-V");
        let banner = format!(
            "{}{}",
            String::from_utf8_lossy(&v.stdout),
            String::from_utf8_lossy(&v.stderr)
        );
        assert!(banner.contains("OpenSSH_for_Windows"), "{banner}");
    }
}
