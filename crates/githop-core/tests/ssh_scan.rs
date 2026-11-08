//! Integration tests for `ssh::keygen` and `scan::scan` (isolated env, no network).

// Test crate: panicking helpers are fine (also covers the shared `support` module,
// whose non-`#[test]` helpers use `expect`).
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::path::PathBuf;

use githop_core::profile::{Profile, ProfileColor, ProfileSet};
use githop_core::resolver::ResolutionSource;
use githop_core::{scan, ssh, CoreError};
use support::TestEnv;

fn have_ssh_keygen() -> bool {
    githop_core::git::find_on_path("ssh-keygen").is_some()
        || std::env::var_os("SystemRoot")
            .map(|r| {
                PathBuf::from(r)
                    .join("System32/OpenSSH/ssh-keygen.exe")
                    .is_file()
            })
            .unwrap_or(false)
}

#[test]
fn keygen_creates_key_restricts_acl_and_refuses_overwrite() {
    if !have_ssh_keygen() {
        eprintln!("skipping: ssh-keygen not found");
        return;
    }
    let env = TestEnv::new();
    // Key goes into the temp HOME's .ssh (dir created by keygen) â€” never the real ~/.ssh.
    let key = env.home().join(".ssh").join("githop_test");
    ssh::keygen(&key, "githop-test@example.invalid", true).expect("keygen");

    assert!(key.is_file(), "private key created");
    let public = ssh::read_public_key(&key).expect("read .pub");
    assert!(public.starts_with("ssh-ed25519 "), "got {public}");
    assert!(public.ends_with("githop-test@example.invalid"));

    #[cfg(windows)]
    {
        // icacls output is locale-dependent; "(I)" (inherited) markers and the
        // user name are not.
        let out = std::process::Command::new("icacls")
            .arg(&key)
            .output()
            .expect("run icacls");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains("(I)"), "inheritance removed:\n{text}");
        let user = std::env::var("USERNAME").unwrap_or_default();
        assert!(
            text.to_lowercase().contains(&user.to_lowercase()),
            "user granted:\n{text}"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let before = std::fs::read(&key).unwrap();
    match ssh::keygen(&key, "again", true) {
        Err(CoreError::Io { source, .. }) => {
            assert_eq!(source.kind(), std::io::ErrorKind::AlreadyExists)
        }
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
    assert_eq!(std::fs::read(&key).unwrap(), before, "key untouched");
}

#[test]
fn ssh_config_conflicts_in_temp_home() {
    let env = TestEnv::new();
    let git = env.git_handle();
    std::fs::create_dir_all(env.home().join(".ssh")).unwrap();
    std::fs::write(
        env.home().join(".ssh").join("config"),
        "Host github-personal\n  HostName github.com\n  IdentityFile ~/.ssh/p\n\nHost github.com\n  IdentityFile ~/.ssh/id_work\n",
    )
    .unwrap();
    let got = ssh::ssh_config_conflicts(&git).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].host_pattern, "github.com");
    assert_eq!(got[0].line, 6);
}

fn profile(id: &str, email: &str, owner: &str, color: ProfileColor) -> Profile {
    Profile {
        id: id.into(),
        label: id.into(),
        name: format!("{id} person"),
        email: email.into(),
        color,
        ssh_key: PathBuf::from(format!("C:/keys/githop_{id}")),
        owners: vec![owner.into()],
        folders: vec![],
    }
}

#[test]
fn scan_resolves_repos_in_temp_tree() {
    let env = TestEnv::new();
    env.set_global("user.name", "Default Person");
    env.set_global("user.email", "default@example.invalid");
    let git = env.git_handle();

    let office = env.init_repo("office-api");
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:office-org/billing-api.git",
        ],
        &office,
    );
    let personal = env.init_repo("side/personal-site");
    env.git(
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/jane/site.git",
        ],
        &personal,
    );
    let _bare_folder = env.init_repo("no-remote");
    std::fs::create_dir_all(env.repos().join("node_modules/x/.git")).unwrap();

    let set = ProfileSet {
        version: 1,
        default: Some("personal".into()),
        profiles: vec![
            profile(
                "office",
                "me@office.example",
                "office-org",
                ProfileColor::Blue,
            ),
            profile(
                "personal",
                "me@personal.example",
                "jane",
                ProfileColor::Violet,
            ),
        ],
    };

    let out = scan::scan(&git, &env.repos(), &set, &[], scan::DEFAULT_MAX_DEPTH);
    let roots: Vec<&str> = out
        .iter()
        .map(|r| r.root.rsplit('/').next().unwrap_or(""))
        .collect();
    assert_eq!(roots, vec!["no-remote", "office-api", "personal-site"]);
    for r in &out {
        assert!(r.error.is_none(), "{}: {:?}", r.root, r.error);
        assert!(!r.root.contains('\\'));
        let id = r.identity.as_ref().expect("identity");
        assert_eq!(id.author.email, "default@example.invalid");
    }
    assert_eq!(out[0].resolution.source, Some(ResolutionSource::Default));
    assert_eq!(out[0].resolution.profile_id.as_deref(), Some("personal"));
    assert_eq!(out[1].resolution.source, Some(ResolutionSource::Owner));
    assert_eq!(out[1].resolution.profile_id.as_deref(), Some("office"));
    assert_eq!(out[2].resolution.profile_id.as_deref(), Some("personal"));
}
