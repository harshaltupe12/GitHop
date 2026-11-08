//! Store + global git config backup/restore against the isolated `TestEnv`
//! (never the real `~/.gitconfig` or `%APPDATA%`, CLAUDE.md rule 3).

mod support;

use std::path::PathBuf;

use githop_core::backup::{
    backup_global_gitconfig, original_gitconfig, restore_global_gitconfig, restore_managed_keys,
    ORIGINAL_GITCONFIG,
};
use githop_core::{gitconfig, CoreError, Profile, ProfileColor, ProfileSet, Settings, Store};
use support::TestEnv;

fn profiles() -> ProfileSet {
    ProfileSet {
        version: 1,
        default: Some("office".into()),
        profiles: vec![
            Profile {
                id: "office".into(),
                label: "Office".into(),
                name: "Jane (Work)".into(),
                email: "jane@company.com".into(),
                color: ProfileColor::Blue,
                ssh_key: PathBuf::from("C:/Users/Me/.ssh/githop_office"),
                owners: vec!["Office-Org".into()],
                folders: vec![r"C:\Office Work".into()],
                sign_commits: false,
            },
            Profile {
                id: "personal".into(),
                label: "Personal".into(),
                name: "H\u{e1}rshal".into(),
                email: "12345+jane@users.noreply.github.com".into(),
                color: ProfileColor::Violet,
                ssh_key: PathBuf::from("C:/Users/Me/.ssh/githop_personal"),
                owners: vec!["jane-personal".into()],
                folders: vec![],
                sign_commits: false,
            },
        ],
    }
}

#[test]
fn store_round_trips_in_isolated_data_dir() {
    let env = TestEnv::new();
    let paths = env.paths();
    assert!(paths.root().starts_with(env.root()));

    let mut store = Store::open_at(paths.clone()).expect("open");
    assert!(store.profiles().profiles.is_empty());
    store.save(&profiles(), &Settings::default()).expect("save");

    let reopened = Store::open_at(paths).expect("reopen");
    let office = reopened.profiles().get("office").expect("office");
    assert_eq!(office.owners, vec!["office-org"]);
    assert_eq!(office.folders, vec!["C:/Office Work/"]);
    assert_eq!(
        reopened.profiles().default_profile().map(|p| p.id.as_str()),
        Some("office")
    );
}

#[test]
fn store_rejects_invalid_profiles_with_all_issues() {
    let env = TestEnv::new();
    let mut store = Store::open_at(env.paths()).expect("open");
    let mut bad = profiles();
    bad.profiles[1].owners = vec!["office-org".into()];
    bad.profiles[1].color = ProfileColor::Blue;
    bad.default = Some("missing".into());
    match store.save(&bad, &Settings::default()) {
        Err(CoreError::Validation(issues)) => {
            let fields: Vec<&str> = issues.iter().map(|i| i.field.as_str()).collect();
            assert_eq!(fields, ["color", "owners", "default"], "{issues:?}");
        }
        other => panic!("expected Validation, got {other:?}"),
    }
    assert!(!env.paths().profiles_file().exists());
}

#[test]
fn backup_and_restore_isolated_global_config() {
    let env = TestEnv::new();
    let git = env.git_handle();
    let paths = env.paths();
    assert_eq!(
        git.global_config_path().expect("global path"),
        env.global_config()
    );

    env.set_global("user.name", "Original Person");
    env.set_global("user.email", "original@example.invalid");
    let first = backup_global_gitconfig(&paths, &git).expect("backup 1");
    assert!(first.starts_with(paths.backups_dir()));
    assert_eq!(
        original_gitconfig(&paths),
        Some(paths.backups_dir().join(ORIGINAL_GITCONFIG))
    );

    // GitHop changes the global identity, takes another backup, changes it again.
    env.set_global("user.email", "second@example.invalid");
    backup_global_gitconfig(&paths, &git).expect("backup 2");
    env.set_global("user.email", "third@example.invalid");

    let home = env.home();
    restore_global_gitconfig(&paths, &git, false).expect("restore newest");
    assert_eq!(
        env.git(&["config", "--global", "user.email"], &home),
        "second@example.invalid"
    );

    restore_global_gitconfig(&paths, &git, true).expect("restore original");
    assert_eq!(
        env.git(&["config", "--global", "user.email"], &home),
        "original@example.invalid"
    );
    assert_eq!(
        env.git(&["config", "--global", "user.name"], &home),
        "Original Person"
    );
}

/// Review #1: no `~/.gitconfig` but an XDG `git/config` → git writes the XDG
/// file, so GitHop must back up and write that same file.
#[test]
fn xdg_global_config_is_backed_up_and_written() {
    let env = TestEnv::new();
    std::fs::remove_file(env.global_config()).expect("remove ~/.gitconfig");
    let xdg = env.xdg_global_config();
    std::fs::create_dir_all(xdg.parent().expect("parent")).expect("mkdir xdg");
    std::fs::write(&xdg, "[user]\n\temail = xdg@example.invalid\n").expect("write xdg");
    let git = env.git_handle_native_global();
    let paths = env.paths();

    assert_eq!(git.global_config_path().expect("path"), xdg);
    let backup = backup_global_gitconfig(&paths, &git).expect("backup");
    assert_eq!(
        std::fs::read_to_string(&backup).expect("read backup"),
        "[user]\n\temail = xdg@example.invalid\n"
    );

    gitconfig::set_global(&git, "user.email", "githop@example.invalid").expect("set");
    gitconfig::set_global(&git, "user.name", "GitHop Person").expect("set");
    assert!(
        !env.global_config().exists(),
        "~/.gitconfig must not appear"
    );
    let text = std::fs::read_to_string(&xdg).expect("read xdg");
    assert!(text.contains("githop@example.invalid"), "{text}");
    // git itself agrees on the effective value.
    assert_eq!(
        gitconfig::get_global(&git, "user.email")
            .expect("get")
            .as_deref(),
        Some("githop@example.invalid")
    );
    let out = git
        .run_text(
            &env.home(),
            &["config", "--global", "--show-origin", "user.email"],
        )
        .expect("show-origin");
    assert!(out.contains("xdg"), "{out}");

    gitconfig::unset_global(&git, "user.name").expect("unset");
    assert!(!std::fs::read_to_string(&xdg)
        .expect("read")
        .contains("GitHop Person"));

    // Restore targets the same file.
    restore_global_gitconfig(&paths, &git, true).expect("restore");
    assert_eq!(
        std::fs::read_to_string(&xdg).expect("read xdg"),
        "[user]\n\temail = xdg@example.invalid\n"
    );
    assert!(!env.global_config().exists());

    // Restoring the keys only also writes the XDG file.
    gitconfig::set_global(&git, "user.email", "again@example.invalid").expect("set");
    restore_managed_keys(&paths, &git, &["user.email".to_string()]).expect("keys");
    assert_eq!(
        gitconfig::get_global(&git, "user.email")
            .expect("get")
            .as_deref(),
        Some("xdg@example.invalid")
    );
    assert!(!env.global_config().exists());
}

/// Review #4a: wipe restores GitHop's keys only and keeps everything the user
/// changed after the original backup.
#[test]
fn restore_managed_keys_keeps_later_user_edits() {
    let env = TestEnv::new();
    let git = env.git_handle();
    let paths = env.paths();
    let home = env.home();
    env.set_global("user.name", "Original Person");
    env.set_global("user.email", "original@example.invalid");
    env.git(
        &["config", "--global", "--add", "url.a.insteadOf", "x"],
        &home,
    );
    env.git(
        &["config", "--global", "--add", "url.a.insteadOf", "y"],
        &home,
    );
    backup_global_gitconfig(&paths, &git).expect("original");

    // GitHop switches the default identity…
    gitconfig::set_global(&git, "user.name", "Work").expect("set");
    gitconfig::set_global(&git, "user.email", "work@example.invalid").expect("set");
    gitconfig::set_global(&git, "core.sshCommand", "ssh -i \"C:/k\"").expect("set");
    gitconfig::set_global(&git, "url.a.insteadOf", "z").expect("set");
    // …and later the user edits their config themselves.
    env.set_global("alias.st", "status");
    env.set_global("pull.rebase", "true");

    let keys: Vec<String> = [
        "user.name",
        "user.email",
        "core.sshCommand",
        "url.a.insteadOf",
        "user.email",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let done = restore_managed_keys(&paths, &git, &keys).expect("restore keys");
    assert_eq!(done.len(), 4, "duplicates are skipped: {done:?}");
    assert_eq!(done[2].key, "core.sshCommand");
    assert!(done[2].values.is_empty());
    assert_eq!(done[3].values, vec!["x", "y"]);

    let get = |k: &str| gitconfig::get_global(&git, k).expect("get");
    assert_eq!(get("user.name").as_deref(), Some("Original Person"));
    assert_eq!(
        get("user.email").as_deref(),
        Some("original@example.invalid")
    );
    assert_eq!(get("core.sshCommand"), None);
    assert_eq!(
        env.git(
            &["config", "--global", "--get-all", "url.a.insteadOf"],
            &home
        ),
        "x\ny"
    );
    // Later edits survive.
    assert_eq!(get("alias.st").as_deref(), Some("status"));
    assert_eq!(get("pull.rebase").as_deref(), Some("true"));
    // The state before the restore was backed up.
    let backups = std::fs::read_dir(paths.backups_dir())
        .expect("backups")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("gitconfig.2"))
        .count();
    assert_eq!(backups, 2);
}

#[test]
fn restore_managed_keys_from_absent_original_unsets() {
    let env = TestEnv::new();
    std::fs::remove_file(env.global_config()).expect("remove");
    let git = env.git_handle();
    let paths = env.paths();
    backup_global_gitconfig(&paths, &git).expect("original = absent");
    gitconfig::set_global(&git, "user.email", "work@example.invalid").expect("set");
    env.set_global("alias.st", "status");
    let done = restore_managed_keys(&paths, &git, &["user.email".to_string()]).expect("restore");
    assert_eq!(done[0].values, Vec::<String>::new());
    assert_eq!(
        gitconfig::get_global(&git, "user.email").expect("get"),
        None
    );
    assert_eq!(
        gitconfig::get_global(&git, "alias.st")
            .expect("get")
            .as_deref(),
        Some("status")
    );
}

#[test]
fn restore_managed_keys_without_original_fails() {
    let env = TestEnv::new();
    let r = restore_managed_keys(&env.paths(), &env.git_handle(), &["user.email".into()]);
    assert!(matches!(r, Err(CoreError::BackupFailed(_))), "{r:?}");
}

#[test]
fn get_file_reads_a_backup() {
    let env = TestEnv::new();
    let git = env.git_handle();
    let file = env.root().join("some config");
    std::fs::write(
        &file,
        "[user]\n\temail = a@x\n\temail = b@x\n[x]\n\tml = \"l1\\nl2\"\n",
    )
    .expect("write");
    assert_eq!(
        gitconfig::get_file(&git, &file, "user.email")
            .expect("get")
            .as_deref(),
        Some("b@x")
    );
    assert_eq!(
        gitconfig::get_all_file(&git, &file, "user.email").expect("get all"),
        vec!["a@x", "b@x"]
    );
    assert_eq!(
        gitconfig::get_file(&git, &file, "x.ml")
            .expect("get")
            .as_deref(),
        Some("l1\nl2")
    );
    assert_eq!(
        gitconfig::get_file(&git, &file, "user.name").expect("get"),
        None
    );
    assert_eq!(
        gitconfig::get_file(&git, &env.root().join("missing"), "user.name").expect("get"),
        None
    );
}

#[test]
fn missing_global_config_is_restored_as_missing() {
    let env = TestEnv::new();
    std::fs::remove_file(env.global_config()).expect("remove empty global config");
    let git = env.git_handle();
    let paths = env.paths();

    backup_global_gitconfig(&paths, &git).expect("backup");
    env.set_global("user.email", "added-later@example.invalid");
    assert!(env.global_config().exists());

    restore_global_gitconfig(&paths, &git, true).expect("restore");
    assert!(!env.global_config().exists());
}
