//! Store + global git config backup/restore against the isolated `TestEnv`
//! (never the real `~/.gitconfig` or `%APPDATA%`, CLAUDE.md rule 3).

mod support;

use std::path::PathBuf;

use githop_core::backup::{
    backup_global_gitconfig, original_gitconfig, restore_global_gitconfig, ORIGINAL_GITCONFIG,
};
use githop_core::{CoreError, Profile, ProfileColor, ProfileSet, Settings, Store};
use support::TestEnv;

fn profiles() -> ProfileSet {
    ProfileSet {
        version: 1,
        default: Some("office".into()),
        profiles: vec![
            Profile {
                id: "office".into(),
                label: "Office".into(),
                name: "Harshal (Work)".into(),
                email: "harshal@company.com".into(),
                color: ProfileColor::Blue,
                ssh_key: PathBuf::from("C:/Users/Me/.ssh/githop_office"),
                owners: vec!["Office-Org".into()],
                folders: vec![r"C:\Office Work".into()],
            },
            Profile {
                id: "personal".into(),
                label: "Personal".into(),
                name: "H\u{e1}rshal".into(),
                email: "12345+harshal@users.noreply.github.com".into(),
                color: ProfileColor::Violet,
                ssh_key: PathBuf::from("C:/Users/Me/.ssh/githop_personal"),
                owners: vec!["harshal-personal".into()],
                folders: vec![],
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
